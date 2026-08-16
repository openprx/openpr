CREATE TABLE IF NOT EXISTS bot_operation_logs (
  id UUID PRIMARY KEY,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  bot_id UUID NOT NULL,
  tool_name TEXT,
  surface TEXT NOT NULL,
  method TEXT NOT NULL,
  path TEXT NOT NULL,
  business_code INTEGER NOT NULL,
  outcome TEXT NOT NULL,
  error_message TEXT,
  duration_ms BIGINT NOT NULL,
  request_id UUID NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT bot_operation_logs_surface_check
    CHECK (surface IN ('mcp_http', 'mcp_sse', 'mcp_stdio', 'cli', 'rest')),
  CONSTRAINT bot_operation_logs_outcome_check CHECK (outcome IN ('ok', 'error')),
  CONSTRAINT bot_operation_logs_method_check CHECK (method ~ '^[A-Z]{3,10}$'),
  CONSTRAINT bot_operation_logs_path_check CHECK (path LIKE '/%' AND position('?' IN path) = 0),
  CONSTRAINT bot_operation_logs_tool_name_check
    CHECK (tool_name IS NULL OR (length(tool_name) BETWEEN 1 AND 128 AND tool_name ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$')),
  CONSTRAINT bot_operation_logs_error_message_check
    CHECK (error_message IS NULL OR length(error_message) <= 256),
  CONSTRAINT bot_operation_logs_duration_check CHECK (duration_ms >= 0)
);

CREATE INDEX IF NOT EXISTS idx_bot_operation_logs_workspace_created
  ON bot_operation_logs(workspace_id, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_bot_operation_logs_bot_created
  ON bot_operation_logs(bot_id, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_bot_operation_logs_workspace_tool_created
  ON bot_operation_logs(workspace_id, tool_name, created_at DESC)
  WHERE tool_name IS NOT NULL;

COMMENT ON TABLE bot_operation_logs IS
  'Metadata-only ledger of authenticated bot API operations; never stores credentials or request/response bodies';
COMMENT ON COLUMN bot_operation_logs.tool_name IS
  'Untrusted caller-supplied observability label; never an authentication or authorization input';
COMMENT ON COLUMN bot_operation_logs.path IS
  'Request path without query string';
