-- Durable import job records for universal form record imports.

CREATE TABLE IF NOT EXISTS form_import_jobs (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  form_id UUID NOT NULL REFERENCES project_forms(id) ON DELETE CASCADE,
  status TEXT NOT NULL DEFAULT 'queued',
  input JSONB NOT NULL DEFAULT '{}'::jsonb,
  result JSONB,
  error TEXT,
  created_by UUID REFERENCES users(id) ON DELETE SET NULL,
  started_at TIMESTAMPTZ,
  completed_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT form_import_jobs_status_check CHECK (status IN ('queued', 'running', 'completed', 'failed')),
  CONSTRAINT form_import_jobs_input_object CHECK (jsonb_typeof(input) = 'object'),
  CONSTRAINT form_import_jobs_result_object CHECK (result IS NULL OR jsonb_typeof(result) = 'object')
);

CREATE INDEX IF NOT EXISTS idx_form_import_jobs_form
  ON form_import_jobs(form_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_form_import_jobs_workspace
  ON form_import_jobs(workspace_id, project_id, form_id, created_at DESC);
