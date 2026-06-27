CREATE TABLE IF NOT EXISTS form_record_comments (
  id UUID PRIMARY KEY,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  form_id UUID NOT NULL REFERENCES project_forms(id) ON DELETE CASCADE,
  record_id UUID NOT NULL REFERENCES form_records(id) ON DELETE CASCADE,
  author_id UUID REFERENCES users(id) ON DELETE SET NULL,
  body TEXT NOT NULL,
  metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  archived_at TIMESTAMPTZ,
  CONSTRAINT form_record_comments_body_check CHECK (length(trim(body)) > 0),
  CONSTRAINT form_record_comments_metadata_object CHECK (jsonb_typeof(metadata) = 'object')
);

CREATE INDEX IF NOT EXISTS idx_form_record_comments_record
  ON form_record_comments(record_id, created_at DESC)
  WHERE archived_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_form_record_comments_form
  ON form_record_comments(form_id, created_at DESC)
  WHERE archived_at IS NULL;
