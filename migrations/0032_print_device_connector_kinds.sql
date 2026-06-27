-- Extend the connector layer for physical/business side effects such as
-- thermal printing, kitchen tickets, device gateways, and shop-floor equipment.

ALTER TABLE connectors
  DROP CONSTRAINT IF EXISTS connectors_kind_check;

ALTER TABLE connectors
  ADD CONSTRAINT connectors_kind_check CHECK (
    kind IN ('webhook', 'mcp', 'rest', 'cli', 'openprx_tunnel', 'print', 'device')
  );

ALTER TABLE agent_invocations
  DROP CONSTRAINT IF EXISTS agent_invocations_connector_kind_check;

ALTER TABLE agent_invocations
  ADD CONSTRAINT agent_invocations_connector_kind_check CHECK (
    connector_kind IS NULL OR connector_kind IN ('webhook', 'mcp', 'rest', 'cli', 'openprx_tunnel', 'print', 'device')
  );
