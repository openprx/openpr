CREATE TABLE IF NOT EXISTS connectors (
  id UUID PRIMARY KEY,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
  webhook_id UUID UNIQUE REFERENCES webhooks(id) ON DELETE SET NULL,
  kind VARCHAR(32) NOT NULL,
  name VARCHAR(255) NOT NULL,
  description TEXT,
  endpoint TEXT,
  auth_policy JSONB NOT NULL DEFAULT '{}'::jsonb,
  capability_manifest JSONB NOT NULL DEFAULT '{}'::jsonb,
  is_active BOOLEAN NOT NULL DEFAULT TRUE,
  created_by UUID REFERENCES users(id) ON DELETE SET NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT connectors_kind_check CHECK (kind IN ('webhook', 'mcp', 'rest', 'cli', 'openprx_tunnel')),
  CONSTRAINT connectors_auth_policy_object CHECK (jsonb_typeof(auth_policy) = 'object'),
  CONSTRAINT connectors_capability_manifest_object CHECK (jsonb_typeof(capability_manifest) = 'object')
);

CREATE INDEX IF NOT EXISTS idx_connectors_workspace
  ON connectors(workspace_id, is_active, kind);

CREATE INDEX IF NOT EXISTS idx_connectors_project
  ON connectors(project_id, is_active, kind)
  WHERE project_id IS NOT NULL;

INSERT INTO connectors (
  id,
  workspace_id,
  webhook_id,
  kind,
  name,
  endpoint,
  auth_policy,
  capability_manifest,
  is_active,
  created_by,
  created_at,
  updated_at
)
SELECT
  gen_random_uuid(),
  w.workspace_id,
  w.id,
  'webhook',
  COALESCE(w.name, w.url, 'Webhook'),
  w.url,
  jsonb_build_object('mode', 'hmac', 'legacy_webhook', true),
  jsonb_build_object('events', w.events, 'bot_user_id', w.bot_user_id),
  w.active,
  w.created_by,
  w.created_at,
  w.updated_at
FROM webhooks w
WHERE NOT EXISTS (
  SELECT 1 FROM connectors c WHERE c.webhook_id = w.id
);

CREATE TABLE IF NOT EXISTS agent_invocations (
  id UUID PRIMARY KEY,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
  actor_id UUID REFERENCES users(id) ON DELETE SET NULL,
  target_agent_id UUID REFERENCES users(id) ON DELETE SET NULL,
  source_task_id UUID UNIQUE REFERENCES ai_tasks(id) ON DELETE SET NULL,
  trigger_kind VARCHAR(32) NOT NULL,
  trigger_ref_type VARCHAR(64),
  trigger_ref_id UUID,
  connector_id UUID REFERENCES connectors(id) ON DELETE SET NULL,
  connector_kind VARCHAR(32),
  status VARCHAR(32) NOT NULL DEFAULT 'pending',
  payload JSONB NOT NULL DEFAULT '{}'::jsonb,
  result JSONB,
  error_message TEXT,
  audit_chain_id UUID,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT agent_invocations_trigger_kind_check CHECK (
    trigger_kind IN ('mention', 'assigned', 'workflow', 'proposal_vote', 'mcp', 'schedule', 'manual')
  ),
  CONSTRAINT agent_invocations_connector_kind_check CHECK (
    connector_kind IS NULL OR connector_kind IN ('webhook', 'mcp', 'rest', 'cli', 'openprx_tunnel')
  ),
  CONSTRAINT agent_invocations_status_check CHECK (
    status IN ('pending', 'dispatched', 'running', 'completed', 'failed', 'cancelled')
  ),
  CONSTRAINT agent_invocations_payload_object CHECK (jsonb_typeof(payload) = 'object')
);

CREATE INDEX IF NOT EXISTS idx_agent_invocations_workspace
  ON agent_invocations(workspace_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_agent_invocations_project
  ON agent_invocations(project_id, created_at DESC)
  WHERE project_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_agent_invocations_status
  ON agent_invocations(status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_agent_invocations_target_agent
  ON agent_invocations(target_agent_id, status);

INSERT INTO agent_invocations (
  id,
  workspace_id,
  project_id,
  target_agent_id,
  source_task_id,
  trigger_kind,
  trigger_ref_type,
  trigger_ref_id,
  status,
  payload,
  result,
  error_message,
  audit_chain_id,
  created_at,
  updated_at
)
SELECT
  gen_random_uuid(),
  p.workspace_id,
  t.project_id,
  t.ai_participant_id,
  t.id,
  CASE
    WHEN t.task_type = 'issue_assigned' THEN 'assigned'
    WHEN t.task_type = 'vote_requested' THEN 'proposal_vote'
    WHEN t.task_type IN ('review_requested', 'comment_requested') THEN 'mention'
    ELSE 'manual'
  END,
  t.reference_type,
  t.reference_id,
  CASE
    WHEN t.status = 'processing' THEN 'running'
    WHEN t.status IN ('completed', 'failed', 'cancelled') THEN t.status
    ELSE 'pending'
  END,
  t.payload,
  t.result,
  t.error_message,
  t.id,
  t.created_at,
  t.updated_at
FROM ai_tasks t
INNER JOIN projects p ON p.id = t.project_id
WHERE NOT EXISTS (
  SELECT 1 FROM agent_invocations ai WHERE ai.source_task_id = t.id
);
