-- v0.8 cross-instance authorization invalidation (ADR-0016).
--
-- NOTIFY is deliberately only a doorbell.  This table is the durable, per-workspace ordered
-- source of truth, and the composite primary key prevents an instance from skipping an epoch
-- when notifications are lost or observed out of order.
CREATE TABLE IF NOT EXISTS flow_authz_revocations (
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  authz_epoch BIGINT NOT NULL,
  -- Empty means the authorization change applies to the whole workspace (membership, baseline,
  -- or feature state). Otherwise these are caller-known subtree roots; no recursive expansion is
  -- performed while the authorization transaction holds the epoch lock.
  subtree_root_ids UUID[] NOT NULL DEFAULT '{}'::uuid[],
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (workspace_id, authz_epoch),
  CONSTRAINT flow_authz_revocations_epoch_check CHECK (authz_epoch >= 0),
  CONSTRAINT flow_authz_revocations_roots_no_null_check CHECK (array_position(subtree_root_ids, NULL) IS NULL)
);

CREATE INDEX IF NOT EXISTS idx_flow_authz_revocations_created
  ON flow_authz_revocations(created_at);

COMMENT ON TABLE flow_authz_revocations IS
  'Durable per-workspace authorization epochs. NOTIFY is only a redacted wakeup hint.';
