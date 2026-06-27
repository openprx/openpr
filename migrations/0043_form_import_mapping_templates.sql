-- Server-backed reusable import mapping templates for universal forms.

CREATE TABLE IF NOT EXISTS form_import_mapping_templates (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  form_id UUID NOT NULL REFERENCES project_forms(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  header_signature TEXT NOT NULL,
  headers JSONB NOT NULL DEFAULT '[]'::jsonb,
  selections JSONB NOT NULL DEFAULT '[]'::jsonb,
  transforms JSONB NOT NULL DEFAULT '[]'::jsonb,
  shared BOOLEAN NOT NULL DEFAULT true,
  created_by UUID REFERENCES users(id) ON DELETE SET NULL,
  archived_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT form_import_mapping_templates_headers_array CHECK (jsonb_typeof(headers) = 'array'),
  CONSTRAINT form_import_mapping_templates_selections_array CHECK (jsonb_typeof(selections) = 'array'),
  CONSTRAINT form_import_mapping_templates_transforms_array CHECK (jsonb_typeof(transforms) = 'array')
);

CREATE INDEX IF NOT EXISTS idx_form_import_mapping_templates_form
  ON form_import_mapping_templates(form_id, header_signature, updated_at DESC)
  WHERE archived_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_form_import_mapping_templates_workspace
  ON form_import_mapping_templates(workspace_id, project_id, form_id)
  WHERE archived_at IS NULL;
