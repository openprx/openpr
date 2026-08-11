-- Delivery reliability: lease ownership, pickup indexes and a de-duplication backstop.

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

-- Fan-out de-duplication was a non atomic NOT EXISTS probe, so two workers processing the same
-- outbox row created two invocations for one connector and delivered the event twice. Existing
-- duplicates are collapsed onto the oldest row before the unique index is created; the rows
-- removed here are exactly those accidental copies.
DELETE FROM agent_invocations duplicate
USING agent_invocations kept
WHERE duplicate.audit_chain_id IS NOT NULL
  AND duplicate.connector_id IS NOT NULL
  AND kept.audit_chain_id = duplicate.audit_chain_id
  AND kept.connector_id = duplicate.connector_id
  AND (kept.created_at, kept.id) < (duplicate.created_at, duplicate.id);

CREATE UNIQUE INDEX IF NOT EXISTS uq_agent_invocations_audit_chain_connector
  ON agent_invocations(audit_chain_id, connector_id)
  WHERE audit_chain_id IS NOT NULL AND connector_id IS NOT NULL;
