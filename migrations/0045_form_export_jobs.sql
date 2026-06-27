-- Persist universal-form export jobs.

CREATE TABLE IF NOT EXISTS form_export_jobs (
  id uuid PRIMARY KEY,
  workspace_id uuid NOT NULL,
  project_id uuid NOT NULL,
  form_id uuid NOT NULL REFERENCES project_forms(id) ON DELETE CASCADE,
  status text NOT NULL DEFAULT 'queued',
  input jsonb NOT NULL DEFAULT '{}'::jsonb,
  result jsonb,
  error text,
  created_by uuid,
  started_at timestamptz,
  completed_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT form_export_jobs_status_check CHECK (status IN ('queued', 'running', 'completed', 'failed')),
  CONSTRAINT form_export_jobs_input_object CHECK (jsonb_typeof(input) = 'object'),
  CONSTRAINT form_export_jobs_result_object CHECK (result IS NULL OR jsonb_typeof(result) = 'object')
);

CREATE INDEX IF NOT EXISTS idx_form_export_jobs_form
  ON form_export_jobs(form_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_form_export_jobs_workspace
  ON form_export_jobs(workspace_id, project_id, form_id, created_at DESC);
