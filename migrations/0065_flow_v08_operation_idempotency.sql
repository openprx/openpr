-- Idempotency identity for v0.8 scoped maintenance operations.

ALTER TABLE flow_operation_runs
  ADD COLUMN IF NOT EXISTS principal_kind TEXT,
  ADD COLUMN IF NOT EXISTS principal_id UUID,
  ADD COLUMN IF NOT EXISTS idempotency_key TEXT,
  ADD COLUMN IF NOT EXISTS request_hash CHAR(64);

ALTER TABLE flow_operation_runs
  DROP CONSTRAINT IF EXISTS flow_operation_runs_principal_kind_check,
  ADD CONSTRAINT flow_operation_runs_principal_kind_check
    CHECK (principal_kind IS NULL OR principal_kind IN ('user', 'bot')),
  DROP CONSTRAINT IF EXISTS flow_operation_runs_idempotency_shape_check,
  ADD CONSTRAINT flow_operation_runs_idempotency_shape_check
    CHECK ((principal_kind IS NULL) = (principal_id IS NULL)
       AND (principal_id IS NULL) = (idempotency_key IS NULL)
       AND (idempotency_key IS NULL) = (request_hash IS NULL)
       AND (idempotency_key IS NULL OR length(trim(idempotency_key)) > 0)
       AND (request_hash IS NULL OR request_hash ~ '^[0-9a-f]{64}$'));

CREATE UNIQUE INDEX IF NOT EXISTS idx_flow_operation_runs_idempotency
  ON flow_operation_runs(workspace_id, operation, principal_kind, principal_id, idempotency_key)
  WHERE idempotency_key IS NOT NULL;
