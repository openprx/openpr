-- Governed AI/MCP check-result records for high-risk actions.

CREATE TABLE IF NOT EXISTS check_results (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  invocation_id UUID NULL REFERENCES agent_invocations(id) ON DELETE SET NULL,
  connector_id UUID NULL REFERENCES connectors(id) ON DELETE SET NULL,
  action_class TEXT NOT NULL DEFAULT 'high_risk_mutation',
  risk_level TEXT NOT NULL DEFAULT 'high',
  title TEXT NOT NULL,
  summary TEXT NOT NULL,
  result JSONB NOT NULL DEFAULT '{}'::jsonb,
  status TEXT NOT NULL DEFAULT 'requires_proposal',
  proposal_id TEXT NULL REFERENCES proposals(id) ON DELETE SET NULL,
  created_by UUID NULL REFERENCES users(id) ON DELETE SET NULL,
  created_by_kind TEXT NOT NULL DEFAULT 'human',
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  CONSTRAINT check_results_result_object CHECK (jsonb_typeof(result) = 'object'),
  CONSTRAINT check_results_action_class_check CHECK (
    action_class IN (
      'read_only',
      'comment_result',
      'low_risk_mutation',
      'high_risk_mutation',
      'external_side_effect',
      'financial_legal_compliance'
    )
  ),
  CONSTRAINT check_results_risk_level_check CHECK (
    risk_level IN ('low', 'medium', 'high', 'critical')
  ),
  CONSTRAINT check_results_status_check CHECK (
    status IN ('recorded', 'requires_proposal', 'proposed', 'dismissed', 'applied')
  ),
  CONSTRAINT check_results_created_by_kind_check CHECK (
    created_by_kind IN ('human', 'ai', 'bot')
  )
);

CREATE INDEX IF NOT EXISTS idx_check_results_project_created_at
  ON check_results(project_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_check_results_invocation_id
  ON check_results(invocation_id);

CREATE INDEX IF NOT EXISTS idx_check_results_status
  ON check_results(project_id, status);

CREATE INDEX IF NOT EXISTS idx_check_results_proposal_id
  ON check_results(proposal_id);
