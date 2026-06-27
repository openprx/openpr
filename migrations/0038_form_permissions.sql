-- Minimal role-based permission policies for universal forms.

CREATE TABLE IF NOT EXISTS form_permissions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  form_id UUID NOT NULL REFERENCES project_forms(id) ON DELETE CASCADE,
  subject_type TEXT NOT NULL,
  subject_id TEXT NOT NULL,
  policy JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT form_permissions_subject_type_check CHECK (subject_type IN ('role')),
  CONSTRAINT form_permissions_subject_id_check CHECK (subject_id IN ('owner', 'admin', 'member')),
  CONSTRAINT form_permissions_policy_object_check CHECK (jsonb_typeof(policy) = 'object'),
  UNIQUE(form_id, subject_type, subject_id)
);

CREATE INDEX IF NOT EXISTS idx_form_permissions_form
  ON form_permissions(form_id, subject_type, subject_id);

CREATE INDEX IF NOT EXISTS idx_form_permissions_workspace
  ON form_permissions(workspace_id, project_id, form_id);
