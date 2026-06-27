CREATE TABLE IF NOT EXISTS agent_invocation_tool_calls (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  invocation_id UUID NOT NULL REFERENCES agent_invocations(id) ON DELETE CASCADE,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
  actor_id UUID REFERENCES users(id) ON DELETE SET NULL,
  tool_name TEXT NOT NULL,
  transport TEXT NOT NULL DEFAULT 'mcp',
  status TEXT NOT NULL,
  arguments JSONB NOT NULL DEFAULT '{}'::jsonb,
  result_summary TEXT,
  error_message TEXT,
  duration_ms BIGINT NOT NULL DEFAULT 0,
  started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  completed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT agent_invocation_tool_calls_status_check CHECK (status IN ('succeeded', 'failed')),
  CONSTRAINT agent_invocation_tool_calls_arguments_object CHECK (jsonb_typeof(arguments) = 'object'),
  CONSTRAINT agent_invocation_tool_calls_duration_nonnegative CHECK (duration_ms >= 0)
);

CREATE INDEX IF NOT EXISTS idx_agent_invocation_tool_calls_invocation
  ON agent_invocation_tool_calls(invocation_id, started_at);

CREATE INDEX IF NOT EXISTS idx_agent_invocation_tool_calls_project
  ON agent_invocation_tool_calls(project_id, started_at DESC)
  WHERE project_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_agent_invocation_tool_calls_tool_name
  ON agent_invocation_tool_calls(tool_name, started_at DESC);
