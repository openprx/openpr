-- Idempotency ledger for the v0.8 workspace-wide delivery replay operation.

CREATE TABLE IF NOT EXISTS flow_replay_requests (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  principal_kind TEXT NOT NULL,
  principal_id UUID NOT NULL,
  idempotency_key TEXT NOT NULL,
  request_hash CHAR(64) NOT NULL,
  status TEXT NOT NULL DEFAULT 'running',
  result_redacted JSONB,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  finished_at TIMESTAMPTZ,
  CONSTRAINT flow_replay_requests_principal_kind_check CHECK (principal_kind IN ('user', 'bot')),
  CONSTRAINT flow_replay_requests_key_check CHECK (length(trim(idempotency_key)) > 0),
  CONSTRAINT flow_replay_requests_hash_check CHECK (request_hash ~ '^[0-9a-f]{64}$'),
  CONSTRAINT flow_replay_requests_status_check CHECK (status IN ('running', 'completed')),
  CONSTRAINT flow_replay_requests_result_check
    CHECK ((status = 'running' AND result_redacted IS NULL AND finished_at IS NULL)
        OR (status = 'completed' AND jsonb_typeof(result_redacted) = 'object' AND finished_at IS NOT NULL)),
  CONSTRAINT flow_replay_requests_idempotency UNIQUE
    (workspace_id, principal_kind, principal_id, idempotency_key)
);

COMMENT ON TABLE flow_replay_requests IS
  'Exact-request idempotency ledger for delivery replay; result contains ids and counts, never event content';
