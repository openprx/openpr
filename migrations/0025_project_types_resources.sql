-- AI-native project type and scenario resource support.

CREATE TABLE IF NOT EXISTS project_types (
  key TEXT PRIMARY KEY,
  workspace_id UUID NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  domain TEXT NOT NULL DEFAULT 'general',
  default_workflow_id UUID NULL REFERENCES workflows(id) ON DELETE SET NULL,
  default_governance_policy_id UUID NULL,
  enabled_capabilities JSONB NOT NULL DEFAULT '[]'::jsonb,
  field_schema JSONB NOT NULL DEFAULT '{}'::jsonb,
  artifact_schema JSONB NOT NULL DEFAULT '{}'::jsonb,
  default_connectors JSONB NOT NULL DEFAULT '[]'::jsonb,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_project_types_workspace_id
  ON project_types(workspace_id);

ALTER TABLE projects
  ADD COLUMN IF NOT EXISTS type_key TEXT NULL REFERENCES project_types(key) ON DELETE SET NULL;

ALTER TABLE projects
  ADD COLUMN IF NOT EXISTS type_settings JSONB NOT NULL DEFAULT '{}'::jsonb;

CREATE INDEX IF NOT EXISTS idx_projects_type_key
  ON projects(type_key);

CREATE TABLE IF NOT EXISTS project_resources (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  locator JSONB NOT NULL DEFAULT '{}'::jsonb,
  permission_policy JSONB NOT NULL DEFAULT '{}'::jsonb,
  sync_status TEXT NOT NULL DEFAULT 'manual',
  created_by UUID REFERENCES users(id) ON DELETE SET NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  CONSTRAINT project_resources_kind_check CHECK (
    kind IN (
      'repo',
      'directory',
      'document_library',
      'crm_account',
      'erp_order',
      'equipment',
      'site',
      'custom'
    )
  )
);

CREATE INDEX IF NOT EXISTS idx_project_resources_project_id
  ON project_resources(project_id);

CREATE INDEX IF NOT EXISTS idx_project_resources_kind
  ON project_resources(project_id, kind);

WITH default_workflow AS (
  SELECT id
  FROM workflows
  WHERE is_system_default = TRUE
  LIMIT 1
)
INSERT INTO project_types (
  key,
  name,
  description,
  domain,
  default_workflow_id,
  enabled_capabilities,
  field_schema,
  artifact_schema,
  default_connectors
)
SELECT
  seed.key,
  seed.name,
  seed.description,
  seed.domain,
  default_workflow.id,
  seed.enabled_capabilities::jsonb,
  seed.field_schema::jsonb,
  seed.artifact_schema::jsonb,
  seed.default_connectors::jsonb
FROM (
  VALUES
    (
      'code_project',
      'Code Project',
      'Software delivery project with repositories, directories, CI, code review, and AI coding agents.',
      'software',
      '["issues","board","sprints","mcp","webhook","code_context","governance"]',
      '{"resources":[{"kind":"repo","required":false},{"kind":"directory","required":false}]}',
      '{"primary":["comment","check_result","change_proposal","pull_request"]}',
      '["mcp","webhook","cli"]'
    ),
    (
      'custom_form',
      'Custom Form',
      'Universal form project for custom business data, records, links, automation, MCP, webhooks, and plugins.',
      'custom',
      '["forms","mcp","webhook","plugins","reporting"]',
      '{"resources":[],"fields":[]}',
      '{"primary":["form","form_record","business_event"]}',
      '["mcp","webhook","rest"]'
    )
) AS seed(key, name, description, domain, enabled_capabilities, field_schema, artifact_schema, default_connectors)
CROSS JOIN default_workflow
ON CONFLICT (key) DO UPDATE
SET
  name = EXCLUDED.name,
  description = EXCLUDED.description,
  domain = EXCLUDED.domain,
  default_workflow_id = COALESCE(project_types.default_workflow_id, EXCLUDED.default_workflow_id),
  enabled_capabilities = EXCLUDED.enabled_capabilities,
  field_schema = EXCLUDED.field_schema,
  artifact_schema = EXCLUDED.artifact_schema,
  default_connectors = EXCLUDED.default_connectors,
  updated_at = NOW();

UPDATE projects
SET type_key = 'code_project'
WHERE type_key IS NULL;
