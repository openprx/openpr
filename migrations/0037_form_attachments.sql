-- Form attachment metadata for universal form attachment/image fields.

CREATE TABLE IF NOT EXISTS form_attachments (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  form_id UUID NOT NULL REFERENCES project_forms(id) ON DELETE CASCADE,
  record_id UUID REFERENCES form_records(id) ON DELETE SET NULL,
  field_id TEXT NOT NULL,
  field_key TEXT NOT NULL,
  file_name TEXT NOT NULL,
  content_type TEXT NOT NULL DEFAULT '',
  byte_size BIGINT NOT NULL DEFAULT 0,
  storage_key TEXT NOT NULL,
  url TEXT NOT NULL DEFAULT '',
  created_by UUID REFERENCES users(id) ON DELETE SET NULL,
  archived_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT form_attachments_field_key_check CHECK (field_key ~ '^[a-z][a-z0-9_]*$'),
  CONSTRAINT form_attachments_byte_size_check CHECK (byte_size >= 0),
  CONSTRAINT form_attachments_storage_key_check CHECK (length(trim(storage_key)) > 0),
  CONSTRAINT form_attachments_file_name_check CHECK (length(trim(file_name)) > 0)
);

CREATE INDEX IF NOT EXISTS idx_form_attachments_form_field
  ON form_attachments(form_id, field_key, created_at DESC)
  WHERE archived_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_form_attachments_record
  ON form_attachments(record_id, created_at DESC)
  WHERE record_id IS NOT NULL AND archived_at IS NULL;
