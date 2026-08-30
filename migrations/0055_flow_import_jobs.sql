-- Sylvode Flow v0.4 import job ledger and lineage.
--
-- `0054_flow_data_layer.sql` deliberately left `flow_import_jobs`/`flow_import_lineage` out: the
-- ADR-0003 legacy-pages inventory measured `total_rows=0` in every environment, so
-- `legacy-pages-import-v1.md`'s non-zero branch (the REST/CLI/UI importer surface) is not
-- required yet. That exemption covers the importer *endpoints*, not the migration contract:
-- `versions/v0.4-flow-alpha.md`'s own aggregate list ("目标：冻结对象身份、文档边界、权威字段和
-- v0.4 migration contract") names `FlowImportJob` as a v0.4 aggregate, and
-- `contracts/domain-model-v1.md` gives both tables their column shape as part of that same
-- contract. This migration lands the schema so a future non-zero inventory (or the v0.8
-- `flow_package` importer) never needs a second migration for the tables themselves — only for
-- whatever columns v0.8 genuinely widens (see the per-table notes below).
--
-- Column shapes are taken from `contracts/domain-model-v1.md` ("`flow_import_jobs` 与
-- `flow_import_lineage`"), `contracts/legacy-pages-import-v1.md` (the frozen v0.4 `legacy_pages`
-- lineage row shape) and `contracts/export-package-v1.md` (the v0.8 `flow_package` job shape,
-- reserved-but-unused here, same pattern `0054` already uses for `flow_object_grants`).

-- ============================================================================================
-- flow_import_jobs — one row per accepted (post-`confirm=true`) import commit. Preview/staging
-- is never written here (domain-model-v1: "preview/staging 不是 canonical Flow state"); a job
-- row only exists once a commit has been accepted, matching `form_import_jobs`' shape
-- (`0044_form_import_jobs.sql`) with Flow-specific columns swapped in per `domain-model-v1.md`'s
-- field list ("workspace、kind、artifact/package hash、preview mapping hash、mode/status、可信
-- actor、idempotency、expiry、report、audit event").
-- ============================================================================================

CREATE TABLE IF NOT EXISTS flow_import_jobs (
  -- Also the `import_id` a preview response freezes and the `job_id` a commit response returns
  -- (`rest-api-v1.md`'s legacy-admin and package-import routes both thread one id from
  -- preview/commit through to the status `GET`); this package never allocates a second id for it.
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  -- The route's `{workspace_id}` — the *target* workspace content lands in. For `flow_package`
  -- this can differ from `source_workspace_id` below (`export-package-v1.md`'s cross-workspace
  -- `workspace_map`); for `legacy_pages` it always equals `source_workspace_id`
  -- (`legacy-pages-import-v1.md`: "source workspace 必须等于 route workspace").
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  -- `flow_package` rows are reserved for the v0.8 export/import package importer
  -- (`export-package-v1.md`, "状态：Frozen for v0.8 implementation"); no v0.4 code path creates
  -- one — same reserved-value pattern `0054` uses for `flow_object_grants`. Unlike that table's
  -- columns, every other column on this row is already shaped to hold a `flow_package` job
  -- without a widening migration; only `flow_import_lineage` (below) needs one when v0.8 lands.
  kind TEXT NOT NULL,
  source_workspace_id UUID REFERENCES workspaces(id) ON DELETE CASCADE,
  -- `legacy_pages`: `legacy-pages-import-v1.md`'s `source_set_hash` (the exact selected-row set
  -- both preview and commit must agree on). `flow_package`: `export-package-v1.md`'s frozen
  -- `mapping_hash` (the object/document/project map preview froze). One column because both are
  -- the same role — "the exact preview this commit is allowed to promote" — under different names
  -- per source kind.
  mapping_hash TEXT NOT NULL,
  -- `flow_package` only: `export-package-v1.md`'s exact archive `package_sha256`. Always NULL for
  -- `legacy_pages`, which has no uploaded artifact.
  package_sha256 CHAR(64),
  -- `flow_package` only: the staging `package_artifact_id` upload produced. Package bytes and
  -- unzipped staging are never canonical rows (`domain-model-v1.md`: "Package bytes 与解压
  -- staging 不进入上述 canonical tables"), so this is a bare id, not a foreign key.
  artifact_id UUID,
  conflict_policy TEXT,
  external_reference_policy TEXT,
  -- The commit-time request body, kind-specific (`legacy_pages`: `source_page_ids`;
  -- `flow_package`: `project_mapping`, `include_history`, ...). Kept as JSONB rather than one
  -- nullable column per kind, matching `form_import_jobs.input`.
  request JSONB NOT NULL DEFAULT '{}'::jsonb,
  status TEXT NOT NULL DEFAULT 'pending',
  -- The finished `ImportReport` (`export-package-v1.md`) or legacy status payload
  -- (`counts`/`object_mapping`/`warnings`/...). NULL while `pending`/`running`.
  report JSONB,
  error TEXT,
  idempotency_key TEXT NOT NULL,
  -- Hash of the exact commit request body this `idempotency_key` was first accepted with, so a
  -- replay with the same key can be told apart from "body drift" (`export-package-v1.md`: "相同
  -- key+body ... 返回原 job/report/event id；body drift 为 conflict") without re-hashing `request`
  -- against a mutable JSONB comparison at lookup time.
  request_body_hash CHAR(64) NOT NULL,
  -- The reauthorized, trusted actor who committed the job (`domain-model-v1.md`'s "可信 actor";
  -- `legacy-pages-import-v1.md`: "只有 workspace admin principal 可 preview/commit"). Nullable and
  -- `ON DELETE SET NULL` like every other actor column in this schema (`business_events.actor_id`,
  -- `collab_updates.actor_id`) so a deleted user does not erase the job's audit trail; a bot
  -- committer leaves this NULL, its identity carried by `business_events`/`event_dispatch` instead.
  actor_user_id UUID REFERENCES users(id) ON DELETE SET NULL,
  -- Points at `flow.import.completed` (or `.failed`) once the job reaches a terminal state; NULL
  -- while `pending`/`running`. `flow.import.started`/`.previewed` are audit-only and not linked
  -- here (`legacy-pages-import-v1.md`: "preview 是 audit-only `flow.import.previewed`").
  audit_event_id UUID REFERENCES business_events(id),
  started_at TIMESTAMPTZ,
  finished_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT flow_import_jobs_kind_check CHECK (kind IN ('legacy_pages', 'flow_package')),
  CONSTRAINT flow_import_jobs_status_check
    CHECK (status IN ('pending', 'running', 'completed', 'failed')),
  CONSTRAINT flow_import_jobs_conflict_policy_check
    CHECK (conflict_policy IS NULL OR conflict_policy IN ('reject_existing', 'reuse_import_lineage')),
  CONSTRAINT flow_import_jobs_external_reference_policy_check
    CHECK (external_reference_policy IS NULL OR external_reference_policy IN ('reject', 'detach')),
  CONSTRAINT flow_import_jobs_request_object_check CHECK (jsonb_typeof(request) = 'object'),
  CONSTRAINT flow_import_jobs_report_object_check CHECK (report IS NULL OR jsonb_typeof(report) = 'object'),
  -- `legacy_pages` never has an uploaded package artifact or archive hash.
  CONSTRAINT flow_import_jobs_legacy_pages_no_package_check
    CHECK (kind = 'flow_package' OR (package_sha256 IS NULL AND artifact_id IS NULL)),
  -- `legacy-pages-import-v1.md`: "source workspace 必须等于 route workspace" — enforced as a
  -- database constraint rather than trusted to the command layer.
  CONSTRAINT flow_import_jobs_legacy_pages_same_workspace_check
    CHECK (kind = 'flow_package' OR source_workspace_id IS NULL OR source_workspace_id = workspace_id),
  CONSTRAINT flow_import_jobs_terminal_status_check
    CHECK (status NOT IN ('completed', 'failed') OR finished_at IS NOT NULL),
  -- One accepted job per idempotency key per workspace+kind; a replay of the same key looks this
  -- up and compares `request_body_hash` instead of racing a second commit into existence.
  CONSTRAINT flow_import_jobs_idempotency_key_key UNIQUE (workspace_id, kind, idempotency_key)
);

CREATE INDEX IF NOT EXISTS idx_flow_import_jobs_workspace
  ON flow_import_jobs(workspace_id, kind, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_flow_import_jobs_status
  ON flow_import_jobs(workspace_id, status)
  WHERE status IN ('pending', 'running');

COMMENT ON TABLE flow_import_jobs IS
  'Accepted (post-confirm) Flow import commits; preview/staging is never written here (domain-model-v1: "preview/staging 不是 canonical Flow state")';
COMMENT ON COLUMN flow_import_jobs.id IS
  'Same id returned as import_id by preview/status and as job_id by commit (rest-api-v1.md)';
COMMENT ON COLUMN flow_import_jobs.mapping_hash IS
  'legacy_pages: the frozen source_set_hash; flow_package: the frozen preview mapping_hash';

-- ============================================================================================
-- flow_import_lineage — idempotent record of what each source row/object was imported to
-- (domain-model-v1: "source kind/id/content hash、target kind/id/result").
--
-- v0.4 only ever populates `source_kind = 'legacy_pages'` rows, and this row shape is the exact
-- one `legacy-pages-import-v1.md` freezes for that kind: a single `pages` row always maps to
-- exactly one target Page object + its one document. `flow_package` lineage (v0.8) needs a
-- different target shape (`export-package-v1.md`'s object/document/relation mapping is
-- many-rows-per-job, not fixed to one object+one document per source row) and a `package_sha256`
-- component in its own idempotency key, so — like `flow_objects.object_type`'s `collection`/
-- `record` values in `0054` — it gets a widening migration when v0.8 actually implements it
-- instead of a shape landed early and guessed at now.
-- ============================================================================================

CREATE TABLE IF NOT EXISTS flow_import_lineage (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  import_id UUID NOT NULL REFERENCES flow_import_jobs(id) ON DELETE CASCADE,
  source_kind TEXT NOT NULL,
  -- Fixed per `legacy-pages-import-v1.md`: `source_table="pages"`.
  source_table TEXT NOT NULL,
  -- Not a foreign key: `pages.id` today, but the column is source-kind-typed, not `pages`-typed
  -- (same reasoning as `business_events.aggregate_id`, which is TEXT for the same "heterogeneous
  -- referent by declared type" reason).
  source_id UUID NOT NULL,
  source_workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  target_workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  -- `sha256(canonical(title,body_md))` per `legacy-pages-import-v1.md`.
  source_content_hash CHAR(64) NOT NULL,
  target_object_id UUID NOT NULL REFERENCES flow_objects(id) ON DELETE CASCADE,
  target_document_id UUID NOT NULL REFERENCES collab_documents(id) ON DELETE CASCADE,
  result TEXT NOT NULL,
  imported_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT flow_import_lineage_source_kind_check CHECK (source_kind IN ('legacy_pages')),
  CONSTRAINT flow_import_lineage_source_table_check CHECK (source_table = 'pages'),
  -- No 'failed' value: a lineage row only exists once the job's single commit transaction has
  -- committed (`legacy-pages-import-v1.md`: "任一失败全部回滚"), so every row here already denotes
  -- a target that was actually created or reused. Per-item problems that never made it into the
  -- atomic write are reported in `flow_import_jobs.report`/`.error`, not as a lineage row.
  CONSTRAINT flow_import_lineage_result_check CHECK (result IN ('created', 'reused')),
  CONSTRAINT flow_import_lineage_same_workspace_check CHECK (source_workspace_id = target_workspace_id),
  -- domain-model-v1.md's general idempotency key: "(target_workspace_id,source_kind,source_id,
  -- source_content_hash) 幂等唯一". Same content re-imported returns the original target instead
  -- of creating a duplicate; drifted content for the same source_id is a preview-time conflict,
  -- never a second row here.
  CONSTRAINT flow_import_lineage_idempotency_key
    UNIQUE (target_workspace_id, source_kind, source_id, source_content_hash)
);

CREATE INDEX IF NOT EXISTS idx_flow_import_lineage_import ON flow_import_lineage(import_id);

CREATE INDEX IF NOT EXISTS idx_flow_import_lineage_target_object ON flow_import_lineage(target_object_id);

COMMENT ON TABLE flow_import_lineage IS
  'Idempotent source-row -> target-object/document mapping for accepted imports (legacy-pages-import-v1.md); v0.4 only ever writes source_kind=legacy_pages rows';
COMMENT ON COLUMN flow_import_lineage.source_id IS
  'Not a foreign key: heterogeneous by source_kind (pages.id for legacy_pages today), same pattern as business_events.aggregate_id';
