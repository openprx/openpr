-- Sylvode Flow v0.4 data layer.
--
-- Builds the Flow aggregate tables from `contracts/domain-model-v1.md` plus the platform
-- delivery substrate from `contracts/events-v1.md` / `ADR-0011`. Column shapes and invariants
-- are taken verbatim from those two documents and from `ADR-0012` / `ADR-0013`; see the inline
-- comments for the exact clause each constraint encodes.
--
-- Scope notes:
--   * `parent_id`, `inherit_from_parent` (on flow_objects) and the whole `flow_object_grants`
--     table are reserved columns/table per ADR-0012: v0.4 does not implement authorization
--     logic, it only lands the schema so v0.5 does not need a second migration for it.
--   * `event_dispatch` / `event_deliveries` / `event_delivery_sources` are platform tables
--     (not Flow-private), but Flow is their first and only v0.4 producer/consumer, so they are
--     created here per `events-v1.md`'s "v0.4 就建、不推到 v0.8" instruction.
--   * `flow_import_jobs` / `flow_import_lineage` are intentionally NOT created here: they only
--     matter on the ADR-0003 non-zero legacy `pages` branch, which is not triggered (0 rows in
--     every measured environment). This migration does not touch `pages` at all.
--   * Numeric limits that `limits-v1.md` marks `status: unset` (dispatch/delivery lease TTLs,
--     dispatch retry/backoff counts, retention windows, coalescing caps, etc.) are deliberately
--     left with no baked-in default: the application must supply them from config so a later
--     freeze does not require an ALTER. The two limits `limits-v1.md` does freeze today
--     (`delivery_max_attempts = 10`, `collab_tickets` TTL = 60s per `ADR-0007`) are encoded below.

-- ============================================================================================
-- flow_objects — PostgreSQL governance identity and lifecycle (domain-model-v1 "flow_objects").
-- `parent_id` is the ADR-0012 authorization-inheritance-chain authority; the navigator CRDT
-- document holds only ordering, never parent/child structure.
-- ============================================================================================

CREATE TABLE IF NOT EXISTS flow_objects (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
  object_type TEXT NOT NULL,
  parent_id UUID,
  inherit_from_parent BOOLEAN NOT NULL DEFAULT true,
  governance_metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
  lifecycle_status TEXT NOT NULL DEFAULT 'active',
  created_by UUID REFERENCES users(id) ON DELETE SET NULL,
  updated_by UUID REFERENCES users(id) ON DELETE SET NULL,
  archived_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  -- v0.4 object type registry (domain-model-v1 "Object types"); collection/record arrive in v0.6
  -- via a widening migration, mirroring how 0039/0040 widened form_views_type_check.
  CONSTRAINT flow_objects_object_type_check CHECK (object_type IN ('navigator', 'page')),
  CONSTRAINT flow_objects_lifecycle_status_check CHECK (lifecycle_status IN ('active', 'archived')),
  CONSTRAINT flow_objects_archived_at_check CHECK ((lifecycle_status = 'archived') = (archived_at IS NOT NULL)),
  CONSTRAINT flow_objects_governance_metadata_object_check CHECK (jsonb_typeof(governance_metadata) = 'object'),
  CONSTRAINT flow_objects_parent_not_self_check CHECK (parent_id IS NULL OR parent_id <> id),
  -- Lets `flow_relations` and the self FK below prove "same workspace" as a database constraint
  -- instead of an application check (domain-model-v1 "同 workspace guard").
  CONSTRAINT flow_objects_workspace_id_key UNIQUE (workspace_id, id),
  -- ADR-0012 §1: self-referential, same-workspace composite guard; root objects have parent_id
  -- NULL, which a composite FK never checks (MATCH SIMPLE), so roots are unaffected.
  CONSTRAINT flow_objects_parent_workspace_fk
    FOREIGN KEY (workspace_id, parent_id) REFERENCES flow_objects (workspace_id, id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_flow_objects_parent ON flow_objects(workspace_id, parent_id);

CREATE INDEX IF NOT EXISTS idx_flow_objects_project
  ON flow_objects(workspace_id, project_id)
  WHERE project_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_flow_objects_type_status
  ON flow_objects(workspace_id, object_type, lifecycle_status);

COMMENT ON TABLE flow_objects IS
  'Flow object governance identity (ADR-0012 s1); does not hold the collaborative title, which lives in the object''s collab_documents row';
COMMENT ON COLUMN flow_objects.parent_id IS
  'ADR-0012 authorization-inheritance-chain authority; v0.4 only sets it via same-level navigator reorder, never cross-parent move (that is v0.5)';
COMMENT ON COLUMN flow_objects.inherit_from_parent IS
  'ADR-0012 authorization boundary flag; v0.4 always true, reserved column, no v0.4 code path flips it';

-- ============================================================================================
-- flow_workspace_settings — one row per workspace, Flow's workspace-level governance state
-- (domain-model-v1 "flow_workspace_settings"; ADR-0012 s3.1 authz_epoch fencing row).
-- ============================================================================================

CREATE TABLE IF NOT EXISTS flow_workspace_settings (
  workspace_id UUID PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  flow_enabled BOOLEAN NOT NULL DEFAULT false,
  -- v0.4 always the default value; only the v0.5 authorization surface may change it.
  default_member_level TEXT NOT NULL DEFAULT 'edit',
  -- ADR-0012 s3.1: the row every grant/inheritance/membership/parent_id change takes
  -- `FOR UPDATE` on to advance; content writes take `FOR SHARE` on it held to commit.
  authz_epoch BIGINT NOT NULL DEFAULT 0,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by UUID REFERENCES users(id) ON DELETE SET NULL,
  CONSTRAINT flow_workspace_settings_default_member_level_check
    CHECK (default_member_level IN ('full_access', 'edit', 'comment', 'view')),
  CONSTRAINT flow_workspace_settings_authz_epoch_check CHECK (authz_epoch >= 0)
);

COMMENT ON TABLE flow_workspace_settings IS
  'One row per workspace; read/write goes through the existing GET|PUT /workspaces/{id}/features/flow endpoint, no new endpoint per ADR-0012';
COMMENT ON COLUMN flow_workspace_settings.authz_epoch IS
  'ADR-0012 s3.1 linearization fence; not a cache-invalidation counter, it is the CAS/FOR SHARE barrier itself';

-- ============================================================================================
-- collab_documents — one bounded CRDT document per flow_object (domain-model-v1
-- "collab_documents"); ADR-0010 durability basis; snapshot bytes use PostgreSQL BYTEA per
-- v0.4-flow-alpha "Data layer" ("snapshot 初期使用 PostgreSQL BYTEA").
-- ============================================================================================

CREATE TABLE IF NOT EXISTS collab_documents (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  object_id UUID NOT NULL UNIQUE REFERENCES flow_objects(id) ON DELETE CASCADE,
  -- v0.4-flow-alpha: "只暴露选定 engine adapter" (v0.3 selected loro).
  engine TEXT NOT NULL DEFAULT 'loro',
  format_version TEXT NOT NULL,
  snapshot BYTEA NOT NULL,
  snapshot_frontier BYTEA NOT NULL,
  snapshot_seq BIGINT NOT NULL DEFAULT 0,
  head_frontier BYTEA NOT NULL,
  head_seq BIGINT NOT NULL DEFAULT 0,
  byte_count BIGINT NOT NULL DEFAULT 0,
  update_count BIGINT NOT NULL DEFAULT 0,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT collab_documents_engine_check CHECK (engine IN ('loro')),
  -- domain-model-v1 "collab_documents": "snapshot_seq <= head_seq".
  CONSTRAINT collab_documents_seq_check CHECK (snapshot_seq >= 0 AND head_seq >= 0 AND snapshot_seq <= head_seq),
  CONSTRAINT collab_documents_byte_count_check CHECK (byte_count >= 0),
  CONSTRAINT collab_documents_update_count_check CHECK (update_count >= 0)
);

COMMENT ON TABLE collab_documents IS
  'Exactly one canonical document per flow_object (object/document 一对一); head_seq/head_frontier are cross-instance authority per ADR-0010, not the warm cache';

-- ============================================================================================
-- collab_updates — accepted updates ordered by seq within a document (domain-model-v1
-- "collab_updates"); seq is allocated inside the ADR-0010 row-locked transaction, never a
-- sequence/identity column.
-- ============================================================================================

CREATE TABLE IF NOT EXISTS collab_updates (
  document_id UUID NOT NULL REFERENCES collab_documents(id) ON DELETE CASCADE,
  seq BIGINT NOT NULL,
  update_id UUID NOT NULL,
  content_hash CHAR(64) NOT NULL,
  idempotency_key TEXT,
  before_frontier BYTEA NOT NULL,
  after_frontier BYTEA NOT NULL,
  bytes BYTEA NOT NULL,
  actor_id UUID REFERENCES users(id) ON DELETE SET NULL,
  origin_surface TEXT NOT NULL,
  origin_client_id TEXT,
  -- Light projection is completed synchronously in the same commit
  -- (v0.4-flow-alpha "Worker...": "保存所需轻量 projection 同步完成").
  projection_seq BIGINT NOT NULL,
  -- Points at the same-transaction flow.content.accepted business event fact.
  event_id UUID NOT NULL REFERENCES business_events(id),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (document_id, seq),
  CONSTRAINT collab_updates_seq_check CHECK (seq >= 0),
  CONSTRAINT collab_updates_projection_seq_check CHECK (projection_seq >= 0),
  CONSTRAINT collab_updates_origin_surface_check CHECK (
    origin_surface IN ('web', 'rest', 'mcp_http', 'mcp_sse', 'mcp_stdio', 'cli', 'cli_tools_call', 'worker', 'system')
  ),
  CONSTRAINT collab_updates_update_id_key UNIQUE (document_id, update_id),
  CONSTRAINT collab_updates_content_hash_key UNIQUE (document_id, content_hash)
);

-- idempotency_key can be absent for updates that carry no client-supplied dedup key.
CREATE UNIQUE INDEX IF NOT EXISTS idx_collab_updates_idempotency
  ON collab_updates(document_id, idempotency_key)
  WHERE idempotency_key IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_collab_updates_event ON collab_updates(event_id);

COMMENT ON TABLE collab_updates IS
  'seq is allocated as head_seq+1 inside the ADR-0010 locked transaction; egress sequencer rebuilds accepted frames from these rows on gap/reorder instead of re-applying CRDT updates';

-- ============================================================================================
-- collab_tickets — one-time, 60s TTL WebSocket grant (domain-model-v1 "collab_tickets";
-- ADR-0007). Only the SHA-256 hash of the raw 256-bit ticket is ever stored.
-- ============================================================================================

CREATE TABLE IF NOT EXISTS collab_tickets (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  ticket_hash CHAR(64) NOT NULL UNIQUE,
  user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  document_id UUID NOT NULL REFERENCES collab_documents(id) ON DELETE CASCADE,
  client_id TEXT NOT NULL,
  origin TEXT NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL,
  consumed_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT collab_tickets_client_id_check CHECK (length(trim(client_id)) > 0),
  CONSTRAINT collab_tickets_origin_check CHECK (length(trim(origin)) > 0),
  -- ADR-0007: TTL is frozen at 60 seconds.
  CONSTRAINT collab_tickets_expiry_check
    CHECK (expires_at > created_at AND expires_at <= created_at + interval '60 seconds'),
  CONSTRAINT collab_tickets_consumed_at_check CHECK (consumed_at IS NULL OR consumed_at >= created_at)
);

CREATE INDEX IF NOT EXISTS idx_collab_tickets_expiry ON collab_tickets(expires_at);
CREATE INDEX IF NOT EXISTS idx_collab_tickets_document ON collab_tickets(document_id);

COMMENT ON TABLE collab_tickets IS
  'Raw ticket bytes never stored and never logged; only the SHA-256 hex digest lives here, consumed exactly once via a conditional update on consumed_at';

-- ============================================================================================
-- flow_relations — typed, directed relations between objects (domain-model-v1 "flow_relations");
-- PostgreSQL rows, not a collab document; cross-workspace relations are rejected at the FK level.
-- ============================================================================================

CREATE TABLE IF NOT EXISTS flow_relations (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  relation_type TEXT NOT NULL,
  source_object_id UUID NOT NULL,
  target_object_id UUID NOT NULL,
  position_key TEXT NOT NULL DEFAULT '',
  properties JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_by UUID REFERENCES users(id) ON DELETE SET NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT flow_relations_relation_type_check CHECK (relation_type ~ '^[a-z][a-z0-9_]*$'),
  CONSTRAINT flow_relations_properties_object_check CHECK (jsonb_typeof(properties) = 'object'),
  CONSTRAINT flow_relations_unique_link UNIQUE (workspace_id, source_object_id, target_object_id, relation_type),
  -- domain-model-v1 "flow_relations": source/target constrained into the same workspace via
  -- composite FK against flow_objects(workspace_id, id); a cross-workspace insert simply fails.
  CONSTRAINT flow_relations_source_workspace_fk
    FOREIGN KEY (workspace_id, source_object_id) REFERENCES flow_objects (workspace_id, id) ON DELETE CASCADE,
  CONSTRAINT flow_relations_target_workspace_fk
    FOREIGN KEY (workspace_id, target_object_id) REFERENCES flow_objects (workspace_id, id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_flow_relations_source ON flow_relations(workspace_id, source_object_id, relation_type);
CREATE INDEX IF NOT EXISTS idx_flow_relations_target ON flow_relations(workspace_id, target_object_id, relation_type);

COMMENT ON TABLE flow_relations IS
  'link/unlink only ever touch this table (ADR-0013): no document head is advanced, existing_document_cardinality = 0';

-- ============================================================================================
-- flow_object_projections — current read model per document (domain-model-v1
-- "flow_object_projections"); never a write source back into the CRDT.
-- ============================================================================================

CREATE TABLE IF NOT EXISTS flow_object_projections (
  object_id UUID PRIMARY KEY REFERENCES flow_objects(id) ON DELETE CASCADE,
  document_seq BIGINT NOT NULL,
  document_frontier BYTEA NOT NULL,
  title TEXT NOT NULL DEFAULT '',
  state JSONB NOT NULL DEFAULT '{}'::jsonb,
  plain_text TEXT NOT NULL DEFAULT '',
  projection_version INTEGER NOT NULL DEFAULT 1,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT flow_object_projections_document_seq_check CHECK (document_seq >= 0),
  CONSTRAINT flow_object_projections_version_check CHECK (projection_version >= 1),
  CONSTRAINT flow_object_projections_state_object_check CHECK (jsonb_typeof(state) = 'object')
);

COMMENT ON TABLE flow_object_projections IS
  'Read-only materialized projection at a specific document_seq/frontier; REST/MCP/CLI/Web all read this row, none of them write it directly';

-- ============================================================================================
-- flow_projection_jobs — durable retry metadata for high-cost projection rebuilds and full-text
-- indexing (v0.4-flow-alpha "Data layer": "高成本重建任务与 retry metadata"); mirrors the
-- existing form_import_jobs job-table shape.
-- ============================================================================================

CREATE TABLE IF NOT EXISTS flow_projection_jobs (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  object_id UUID NOT NULL REFERENCES flow_objects(id) ON DELETE CASCADE,
  document_id UUID NOT NULL REFERENCES collab_documents(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'queued',
  target_seq BIGINT,
  attempts INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER NOT NULL DEFAULT 10,
  next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_error TEXT,
  requested_by UUID REFERENCES users(id) ON DELETE SET NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  completed_at TIMESTAMPTZ,
  CONSTRAINT flow_projection_jobs_kind_check CHECK (kind IN ('projection_rebuild', 'full_text_index')),
  CONSTRAINT flow_projection_jobs_status_check CHECK (status IN ('queued', 'running', 'completed', 'failed')),
  CONSTRAINT flow_projection_jobs_attempts_check CHECK (attempts >= 0 AND max_attempts > 0),
  CONSTRAINT flow_projection_jobs_target_seq_check CHECK (target_seq IS NULL OR target_seq >= 0),
  CONSTRAINT flow_projection_jobs_completed_at_check
    CHECK (completed_at IS NULL OR status IN ('completed', 'failed'))
);

CREATE INDEX IF NOT EXISTS idx_flow_projection_jobs_pickup
  ON flow_projection_jobs(status, next_attempt_at)
  WHERE status IN ('queued', 'running');

CREATE INDEX IF NOT EXISTS idx_flow_projection_jobs_object ON flow_projection_jobs(object_id, created_at DESC);

-- ============================================================================================
-- flow_integrity_records — invariant drift a database constraint cannot catch by itself
-- (ADR-0013 s4, verbatim column list); operational fact, never a business_events row, never
-- delivered or replayed.
-- ============================================================================================

CREATE TABLE IF NOT EXISTS flow_integrity_records (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  subject_kind TEXT NOT NULL,
  subject_id TEXT NOT NULL,
  detected_by TEXT NOT NULL,
  detected_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  details_redacted JSONB NOT NULL DEFAULT '{}'::jsonb,
  status TEXT NOT NULL DEFAULT 'open',
  resolved_at TIMESTAMPTZ,
  CONSTRAINT flow_integrity_records_kind_check CHECK (kind ~ '^[a-z][a-z0-9_]*$'),
  CONSTRAINT flow_integrity_records_subject_kind_check CHECK (subject_kind ~ '^[a-z][a-z0-9_]*$'),
  CONSTRAINT flow_integrity_records_subject_id_check CHECK (length(trim(subject_id)) > 0),
  CONSTRAINT flow_integrity_records_detected_by_check CHECK (length(trim(detected_by)) > 0),
  CONSTRAINT flow_integrity_records_status_check CHECK (status IN ('open', 'resolved', 'ignored')),
  CONSTRAINT flow_integrity_records_resolved_at_check CHECK ((status = 'open') = (resolved_at IS NULL)),
  CONSTRAINT flow_integrity_records_details_object_check CHECK (jsonb_typeof(details_redacted) = 'object')
);

CREATE INDEX IF NOT EXISTS idx_flow_integrity_records_workspace
  ON flow_integrity_records(workspace_id, status, detected_at DESC);

CREATE INDEX IF NOT EXISTS idx_flow_integrity_records_kind ON flow_integrity_records(kind, status);

COMMENT ON TABLE flow_integrity_records IS
  'Operational fact, not a business fact (ADR-0013 s4): never joins business_events, never delivered, never replayed; details_redacted follows events-v1 redaction rules and never holds body text or CRDT bytes';

-- ============================================================================================
-- flow_object_grants — RESERVED per ADR-0012: v0.4 lands the schema only, no v0.4 code path
-- reads or writes it. Authorization logic (effective-permission computation, boundary,
-- self-lockout guard, epoch linearization) is v0.5 scope.
-- ============================================================================================

CREATE TABLE IF NOT EXISTS flow_object_grants (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  object_id UUID NOT NULL REFERENCES flow_objects(id) ON DELETE CASCADE,
  principal_kind TEXT NOT NULL,
  principal_id UUID NOT NULL,
  level TEXT NOT NULL,
  granted_by UUID REFERENCES users(id) ON DELETE SET NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT flow_object_grants_principal_kind_check CHECK (principal_kind IN ('user', 'bot')),
  CONSTRAINT flow_object_grants_level_check CHECK (level IN ('full_access', 'edit', 'comment', 'view')),
  CONSTRAINT flow_object_grants_unique_principal UNIQUE (object_id, principal_kind, principal_id),
  -- Same "no cross-workspace grant" guard as flow_relations, via flow_objects(workspace_id, id).
  CONSTRAINT flow_object_grants_object_workspace_fk
    FOREIGN KEY (workspace_id, object_id) REFERENCES flow_objects (workspace_id, id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_flow_object_grants_object ON flow_object_grants(workspace_id, object_id);
CREATE INDEX IF NOT EXISTS idx_flow_object_grants_principal ON flow_object_grants(principal_kind, principal_id);

COMMENT ON TABLE flow_object_grants IS
  'RESERVED (ADR-0012): v0.4 creates this table but no application code reads or writes it; principal_id intentionally has no FK because it addresses either users or workspace_bots';

-- ============================================================================================
-- event_dispatch / event_deliveries / event_delivery_sources — ADR-0011 single-row dispatch
-- work + dispatcher-side fan-out. Platform tables; Flow is their only v0.4 producer/consumer.
-- Column shapes, CHECKs and indexes below are the literal schema block in events-v1.md
-- ("投递" section) plus the constraints its surrounding prose derives ("计时锚点", "lease 的成对
-- 状态不变量", "event_dispatch 的终态保留期与索引"). events-v1.md is this schema's sole
-- normative source; do not hand-copy a column list from anywhere else.
-- ============================================================================================

CREATE TABLE IF NOT EXISTS event_dispatch (
  id UUID PRIMARY KEY,
  event_id UUID NOT NULL UNIQUE REFERENCES business_events(id),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  event_type TEXT NOT NULL,
  -- No FK: events-v1.md's FK table intentionally does not constrain this column, keeping the
  -- table reusable beyond Flow's collab_documents.
  document_id UUID,
  accepted_seq BIGINT,
  status TEXT NOT NULL DEFAULT 'pending',
  expanded_at TIMESTAMPTZ,
  attempts INTEGER NOT NULL DEFAULT 0,
  -- dispatch_max_attempts is `status: unset` in limits-v1.md: the application must supply it,
  -- no self-invented default is baked in here.
  max_attempts INTEGER NOT NULL,
  next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  lease_token TEXT,
  lease_expires_at TIMESTAMPTZ,
  lease_reclaims INTEGER NOT NULL DEFAULT 0,
  last_error_code TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT event_dispatch_event_type_check CHECK (event_type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
  CONSTRAINT event_dispatch_status_check CHECK (status IN ('pending', 'expanded', 'no_subscribers', 'failed')),
  -- "展开必须按 seq 有序" s1: filled iff event_type = flow.content.accepted, NULL otherwise.
  CONSTRAINT event_dispatch_document_id_fill_check
    CHECK ((event_type = 'flow.content.accepted') = (document_id IS NOT NULL)),
  CONSTRAINT event_dispatch_accepted_seq_fill_check
    CHECK ((event_type = 'flow.content.accepted') = (accepted_seq IS NOT NULL)),
  CONSTRAINT event_dispatch_accepted_seq_check CHECK (accepted_seq IS NULL OR accepted_seq >= 0),
  -- "计时锚点" dual constraint: expanded_at set iff status is one of the three terminal states.
  CONSTRAINT event_dispatch_expanded_at_check
    CHECK ((status IN ('expanded', 'no_subscribers', 'failed')) = (expanded_at IS NOT NULL)),
  -- "lease 的成对状态不变量": lease_token and lease_expires_at are set and cleared together.
  CONSTRAINT event_dispatch_lease_pair_check CHECK ((lease_token IS NULL) = (lease_expires_at IS NULL)),
  CONSTRAINT event_dispatch_attempts_check CHECK (attempts >= 0 AND max_attempts > 0 AND lease_reclaims >= 0)
);

-- Head-of-queue lookup: "MIN(accepted_seq) WHERE document_id=? AND status='pending'".
CREATE INDEX IF NOT EXISTS idx_event_dispatch_head
  ON event_dispatch(document_id, accepted_seq)
  WHERE status = 'pending' AND accepted_seq IS NOT NULL;

-- The two lease-claim queries (worker-crash reclaim uses the same predicate shape).
CREATE INDEX IF NOT EXISTS idx_event_dispatch_lease ON event_dispatch(status, next_attempt_at);

-- Retention scan anchor is expanded_at, not created_at (see "计时锚点").
CREATE INDEX IF NOT EXISTS idx_event_dispatch_retention ON event_dispatch(status, expanded_at);

COMMENT ON TABLE event_dispatch IS
  'Exactly one row per business transition with delivery_class=business; written in the same domain transaction, zero subscription-directory queries in that transaction (ADR-0011)';
COMMENT ON COLUMN event_dispatch.document_id IS
  'Filled only for flow.content.accepted so the dispatcher can order same-document work by accepted_seq without parsing payload; NULL for every other event type';

CREATE TABLE IF NOT EXISTS event_deliveries (
  id UUID PRIMARY KEY,
  -- Nullable: replay-rebuilt rows and requeue_failed-revived rows carry no dispatch lineage.
  dispatch_id UUID REFERENCES event_dispatch(id) ON DELETE SET NULL,
  event_id UUID NOT NULL REFERENCES business_events(id),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  subscriber_kind TEXT NOT NULL,
  -- No FK (刻意): a deleted subscriber must not cascade-delete its delivery audit trail.
  subscriber_id UUID NOT NULL,
  document_id UUID,
  status TEXT NOT NULL DEFAULT 'pending',
  attempts INTEGER NOT NULL DEFAULT 0,
  -- delivery_max_attempts is frozen at 10 in limits-v1.md.
  max_attempts INTEGER NOT NULL DEFAULT 10,
  next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  lease_token TEXT,
  lease_expires_at TIMESTAMPTZ,
  terminated_at TIMESTAMPTZ,
  last_error_code TEXT,
  first_seq BIGINT,
  latest_seq BIGINT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  -- v0.4 subscriber universe is webhook-only (plugin_hook was removed from the registry).
  CONSTRAINT event_deliveries_subscriber_kind_check CHECK (subscriber_kind IN ('webhook')),
  CONSTRAINT event_deliveries_status_check
    CHECK (status IN ('pending', 'sealed', 'leased', 'dispatched', 'failed', 'cancelled')),
  CONSTRAINT event_deliveries_attempts_check CHECK (attempts >= 0 AND max_attempts > 0),
  -- "计时锚点": terminated_at set iff status has reached one of the three terminal states.
  CONSTRAINT event_deliveries_terminated_at_check
    CHECK ((status IN ('dispatched', 'failed', 'cancelled')) = (terminated_at IS NOT NULL)),
  CONSTRAINT event_deliveries_lease_pair_check CHECK ((lease_token IS NULL) = (lease_expires_at IS NULL)),
  CONSTRAINT event_deliveries_seq_pair_check CHECK ((first_seq IS NULL) = (latest_seq IS NULL)),
  CONSTRAINT event_deliveries_seq_order_check CHECK (first_seq IS NULL OR latest_seq >= first_seq),
  -- Plain fan-out expansion idempotency: re-expanding the same work must not mint a new delivery_id.
  CONSTRAINT event_deliveries_fanout_key UNIQUE (dispatch_id, subscriber_kind, subscriber_id)
);

-- Coalescing key: at most one *pending* row per (subscriber, document); terminal/sealed rows do
-- not block a new pending row from opening.
CREATE UNIQUE INDEX IF NOT EXISTS idx_event_deliveries_coalesce_pending
  ON event_deliveries(subscriber_kind, subscriber_id, document_id)
  WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS idx_event_deliveries_lease_claim ON event_deliveries(status, next_attempt_at);

-- Dedicated reaper predicate for stuck `leased` rows (the claim query above cannot reach them).
CREATE INDEX IF NOT EXISTS idx_event_deliveries_leased_expiry
  ON event_deliveries(status, lease_expires_at)
  WHERE status = 'leased';

CREATE INDEX IF NOT EXISTS idx_event_deliveries_retention ON event_deliveries(status, terminated_at);

COMMENT ON TABLE event_deliveries IS
  'id is the immutable consumer dedup key delivery_id; coalesced flow.content.accepted rows cover multiple event ids, so consumers must dedup on delivery_id, never event_id';
COMMENT ON COLUMN event_deliveries.document_id IS
  'NULL for a non-merged delivery and for replay/requeue_failed rows (deliberately, so the partial coalescing unique index never applies to them); set only for a merged flow.content.accepted delivery';

CREATE TABLE IF NOT EXISTS event_delivery_sources (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  -- Nullable: retention may clear the delivery this source pointed at while the tombstone stays.
  delivery_id UUID REFERENCES event_deliveries(id) ON DELETE SET NULL,
  subscriber_kind TEXT NOT NULL,
  subscriber_id UUID NOT NULL,
  source_event_id UUID NOT NULL REFERENCES business_events(id),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT event_delivery_sources_subscriber_kind_check CHECK (subscriber_kind IN ('webhook')),
  -- The sole expansion-idempotency and replay dedup key (ON CONFLICT DO NOTHING target).
  CONSTRAINT event_delivery_sources_dedup_key UNIQUE (subscriber_kind, subscriber_id, source_event_id)
);

-- Tombstone retention scan anchor.
CREATE INDEX IF NOT EXISTS idx_event_delivery_sources_created ON event_delivery_sources(created_at);

COMMENT ON TABLE event_delivery_sources IS
  'The only source of truth for what one delivery covers (event_deliveries.dispatch_id/event_id are lineage only); insert always via ON CONFLICT DO NOTHING, in the same transaction as the delivery row it reserves for, per events-v1.md "展开是一个事务"';
