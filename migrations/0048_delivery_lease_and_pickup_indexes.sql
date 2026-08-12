-- Delivery reliability: lease ownership and pickup indexes.
--
-- Every statement in this file is idempotent, which is why the adoption cutoff in the migration
-- runner stops at 0047: a database that predates the ledger re-executes this file once so a
-- previously failed 0048 heals itself instead of being claimed as applied.
--
-- The first version of this file also carried a de-duplication DELETE. It was removed: the
-- pre-ledger runner replayed migrations on every API start, so that DELETE was not a one-time
-- repair but a permanent background deleter, and agent_invocations.id is the parent of
-- invocation_tool_calls ON DELETE CASCADE, so it silently took audit records with it. The
-- de-duplication now lives in 0049 and tags rows instead of deleting them.

-- Ownership token handed out by the outbox pickup query. A completion write must present the
-- token it was given, so a worker whose lease already expired cannot overwrite the row that
-- another worker owns now (which would re-queue an already dispatched event).
ALTER TABLE event_outbox ADD COLUMN IF NOT EXISTS lease_token UUID;

-- The pickup query reclaims expired leases. That branch is outside the predicate of
-- idx_event_outbox_pickup (status IN ('pending','failed')), which made every poll a sequential
-- scan over a table that only ever grows.
CREATE INDEX IF NOT EXISTS idx_event_outbox_lease_recovery
  ON event_outbox(leased_until, created_at)
  WHERE status = 'leased';

-- Connector delivery pickup: the same poll scans agent_invocations by status.
CREATE INDEX IF NOT EXISTS idx_agent_invocations_connector_pickup
  ON agent_invocations(status, created_at)
  WHERE connector_id IS NOT NULL AND source_task_id IS NULL;
