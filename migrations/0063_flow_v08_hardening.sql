-- Sylvode Flow v0.8 hardening state.
--
-- The rows below are durability facts.  PostgreSQL remains authoritative for document sequence;
-- fan-out only points at already committed rows and may be lost or duplicated safely.

ALTER TABLE collab_documents
  ADD COLUMN IF NOT EXISTS snapshot_checksum CHAR(64),
  ADD COLUMN IF NOT EXISTS compaction_boundary_seq BIGINT NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS compaction_boundary_frontier BYTEA NOT NULL DEFAULT ''::bytea,
  ADD COLUMN IF NOT EXISTS last_compacted_at TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS compaction_generation BIGINT NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS shallow_snapshot_enabled BOOLEAN NOT NULL DEFAULT false;

CREATE OR REPLACE FUNCTION flow_set_initial_snapshot_checksum()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.snapshot_checksum IS NULL THEN
    NEW.snapshot_checksum := encode(digest(NEW.snapshot, 'sha256'), 'hex');
  END IF;
  RETURN NEW;
END;
$$;

DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM pg_trigger
     WHERE tgrelid = 'collab_documents'::regclass
       AND tgname = 'collab_documents_initial_snapshot_checksum'
       AND NOT tgisinternal
  ) THEN
    CREATE TRIGGER collab_documents_initial_snapshot_checksum
      BEFORE INSERT ON collab_documents
      FOR EACH ROW EXECUTE FUNCTION flow_set_initial_snapshot_checksum();
  END IF;
END;
$$;

UPDATE collab_documents
   SET snapshot_checksum = encode(digest(snapshot, 'sha256'), 'hex')
 WHERE snapshot_checksum IS NULL;

ALTER TABLE collab_documents
  ALTER COLUMN snapshot_checksum SET NOT NULL;

CREATE TABLE IF NOT EXISTS flow_collab_client_acks (
  document_id UUID NOT NULL REFERENCES collab_documents(id) ON DELETE CASCADE,
  client_id TEXT NOT NULL,
  ack_seq BIGINT NOT NULL,
  ack_frontier BYTEA NOT NULL,
  resync_required BOOLEAN NOT NULL DEFAULT false,
  resync_reason TEXT,
  last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (document_id, client_id),
  CONSTRAINT flow_collab_client_acks_client_check CHECK (length(trim(client_id)) > 0),
  CONSTRAINT flow_collab_client_acks_seq_check CHECK (ack_seq >= 0),
  CONSTRAINT flow_collab_client_acks_resync_reason_check
    CHECK (resync_required OR resync_reason IS NULL)
);

CREATE INDEX IF NOT EXISTS idx_flow_collab_client_acks_active
  ON flow_collab_client_acks(document_id, ack_seq, last_seen_at)
  WHERE NOT resync_required;

CREATE TABLE IF NOT EXISTS flow_fanout_notices (
  id BIGSERIAL PRIMARY KEY,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  document_id UUID NOT NULL REFERENCES collab_documents(id) ON DELETE CASCADE,
  notice_kind TEXT NOT NULL,
  document_seq BIGINT,
  authz_epoch BIGINT,
  payload_redacted JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT flow_fanout_notices_kind_check
    CHECK (notice_kind IN ('document_update', 'authorization_revoked', 'forced_resync')),
  CONSTRAINT flow_fanout_notices_seq_check CHECK (document_seq IS NULL OR document_seq >= 0),
  CONSTRAINT flow_fanout_notices_payload_check CHECK (jsonb_typeof(payload_redacted) = 'object')
);

CREATE INDEX IF NOT EXISTS idx_flow_fanout_notices_document
  ON flow_fanout_notices(document_id, id);
CREATE INDEX IF NOT EXISTS idx_flow_fanout_notices_workspace
  ON flow_fanout_notices(workspace_id, id);

CREATE TABLE IF NOT EXISTS flow_operation_runs (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  operation TEXT NOT NULL,
  scope_kind TEXT NOT NULL,
  scope_id UUID NOT NULL,
  dry_run BOOLEAN NOT NULL,
  expected_head_seq BIGINT,
  expected_head_frontier BYTEA,
  status TEXT NOT NULL,
  result_redacted JSONB NOT NULL DEFAULT '{}'::jsonb,
  actor_id UUID REFERENCES users(id) ON DELETE SET NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  finished_at TIMESTAMPTZ,
  CONSTRAINT flow_operation_runs_operation_check
    CHECK (operation IN ('compact', 'rebuild_projection', 'rebuild_search', 'verify_document',
                         'repair_quarantine', 'retention_cleanup')),
  CONSTRAINT flow_operation_runs_scope_check CHECK (scope_kind IN ('workspace', 'document')),
  CONSTRAINT flow_operation_runs_status_check
    CHECK (status IN ('planned', 'running', 'completed', 'failed', 'quarantined')),
  CONSTRAINT flow_operation_runs_result_check CHECK (jsonb_typeof(result_redacted) = 'object'),
  CONSTRAINT flow_operation_runs_finished_check
    CHECK (status IN ('planned', 'running') OR finished_at IS NOT NULL)
);

CREATE INDEX IF NOT EXISTS idx_flow_operation_runs_scope
  ON flow_operation_runs(workspace_id, scope_kind, scope_id, created_at DESC);

COMMENT ON COLUMN collab_documents.snapshot_checksum IS
  'SHA-256 of the exact persisted snapshot bytes, checked before compaction advances its boundary';
COMMENT ON COLUMN collab_documents.compaction_boundary_seq IS
  'Highest update seq made unnecessary by a verified snapshot; deletion may lag this boundary for client ack safety';
COMMENT ON COLUMN collab_documents.shallow_snapshot_enabled IS
  'v0.8 safety switch; false by default and never inferred from compaction eligibility';
COMMENT ON TABLE flow_fanout_notices IS
  'Durable pointers to committed PostgreSQL facts; the message adapter is notification-only, never document durability';
COMMENT ON TABLE flow_operation_runs IS
  'Redacted audit ledger for scoped dry-run and execute maintenance operations';
