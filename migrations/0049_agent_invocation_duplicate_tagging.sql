-- Fan-out de-duplication without deleting anything.
--
-- The connector fan-out used a non atomic NOT EXISTS probe, so two workers processing the same
-- outbox row created two invocations for one connector and delivered the event twice. A unique
-- index closes that race, but the accidental copies already in the table have to go somewhere
-- first. The previous attempt deleted them, which cascaded into invocation_tool_calls and, thanks
-- to the pre-ledger replay-everything runner, ran on every API start.
--
-- A copy is now tagged with the row it duplicates:
--   * idempotent - only untagged rows are considered, so a re-run tags nothing
--   * auditable  - SELECT count(*) FROM agent_invocations WHERE duplicate_of IS NOT NULL
--   * reversible - UPDATE agent_invocations SET duplicate_of = NULL WHERE ...
-- The worker pickup query skips tagged rows, so a copy is still never delivered.

ALTER TABLE agent_invocations
  ADD COLUMN IF NOT EXISTS duplicate_of UUID REFERENCES agent_invocations(id) ON DELETE SET NULL;

DO $$
DECLARE
  tagged bigint;
BEGIN
  WITH kept AS (
    SELECT DISTINCT ON (audit_chain_id, connector_id)
           audit_chain_id, connector_id, id, created_at
    FROM agent_invocations
    WHERE audit_chain_id IS NOT NULL
      AND connector_id IS NOT NULL
      AND duplicate_of IS NULL
    ORDER BY audit_chain_id, connector_id, created_at, id
  ),
  marked AS (
    UPDATE agent_invocations duplicate
    SET duplicate_of = kept.id
    FROM kept
    WHERE duplicate.audit_chain_id = kept.audit_chain_id
      AND duplicate.connector_id = kept.connector_id
      AND duplicate.duplicate_of IS NULL
      AND (duplicate.created_at, duplicate.id) > (kept.created_at, kept.id)
    RETURNING duplicate.id
  )
  SELECT count(*) INTO tagged FROM marked;

  RAISE WARNING 'agent_invocations de-duplication: tagged % duplicate fan-out row(s) via duplicate_of, deleted 0', tagged;
END $$;

CREATE INDEX IF NOT EXISTS idx_agent_invocations_duplicate_of
  ON agent_invocations(duplicate_of)
  WHERE duplicate_of IS NOT NULL;

-- An earlier build created this index without the duplicate_of predicate. The worker fan-out
-- insert infers the predicate in its ON CONFLICT clause, so the old shape has to go rather than
-- survive under IF NOT EXISTS.
DROP INDEX IF EXISTS uq_agent_invocations_audit_chain_connector;

CREATE UNIQUE INDEX uq_agent_invocations_audit_chain_connector
  ON agent_invocations(audit_chain_id, connector_id)
  WHERE audit_chain_id IS NOT NULL AND connector_id IS NOT NULL AND duplicate_of IS NULL;
