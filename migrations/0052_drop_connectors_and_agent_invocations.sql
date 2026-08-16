-- Operation logs replace the connector/invocation execution ledger. Remove dependent
-- references before dropping the child and parent relations.
ALTER TABLE check_results DROP COLUMN IF EXISTS invocation_id;
ALTER TABLE check_results DROP COLUMN IF EXISTS connector_id;

DROP TABLE IF EXISTS agent_invocation_tool_calls;
DROP TABLE IF EXISTS agent_invocations;
DROP TABLE IF EXISTS connectors;
