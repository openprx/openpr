-- Configurable workflow support (project > workspace > system)

CREATE TABLE IF NOT EXISTS workflows (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  is_system_default BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_workflows_system_default_unique
  ON workflows(is_system_default)
  WHERE is_system_default = TRUE;

CREATE INDEX IF NOT EXISTS idx_workflows_workspace_id
  ON workflows(workspace_id);

CREATE TABLE IF NOT EXISTS workflow_states (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workflow_id UUID NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  display_name TEXT NOT NULL,
  category TEXT NOT NULL DEFAULT 'active',
  position INT NOT NULL,
  color TEXT NULL,
  is_initial BOOLEAN NOT NULL DEFAULT FALSE,
  is_terminal BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  UNIQUE(workflow_id, key),
  UNIQUE(workflow_id, position)
);

CREATE INDEX IF NOT EXISTS idx_workflow_states_workflow_position
  ON workflow_states(workflow_id, position);

ALTER TABLE projects ADD COLUMN IF NOT EXISTS workflow_id UUID NULL REFERENCES workflows(id) ON DELETE SET NULL;
ALTER TABLE workspaces ADD COLUMN IF NOT EXISTS workflow_id UUID NULL REFERENCES workflows(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_projects_workflow_id ON projects(workflow_id);
CREATE INDEX IF NOT EXISTS idx_workspaces_workflow_id ON workspaces(workflow_id);

-- Initialize default 4-state system workflow once.
DO $$
DECLARE
  default_workflow_id UUID;
BEGIN
  SELECT id INTO default_workflow_id FROM workflows WHERE is_system_default = TRUE LIMIT 1;

  IF default_workflow_id IS NULL THEN
    INSERT INTO workflows (id, workspace_id, name, description, is_system_default)
    VALUES (
      gen_random_uuid(),
      NULL,
      'Default workflow',
      'System default 4-state workflow',
      TRUE
    )
    RETURNING id INTO default_workflow_id;
  END IF;

  INSERT INTO workflow_states (workflow_id, key, display_name, category, position, is_initial, is_terminal)
  VALUES
    (default_workflow_id, 'backlog', 'Backlog', 'active', 1, TRUE, FALSE),
    (default_workflow_id, 'todo', 'Todo', 'active', 2, FALSE, FALSE),
    (default_workflow_id, 'in_progress', 'In Progress', 'active', 3, FALSE, FALSE),
    (default_workflow_id, 'done', 'Done', 'done', 4, FALSE, TRUE)
  ON CONFLICT (workflow_id, key) DO NOTHING;

  UPDATE workspaces
  SET workflow_id = default_workflow_id
  WHERE workflow_id IS NULL;

  UPDATE projects
  SET workflow_id = default_workflow_id
  WHERE workflow_id IS NULL;
END $$;
