-- Universal forms core data model.

CREATE TABLE IF NOT EXISTS project_forms (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  icon TEXT,
  color TEXT,
  title_template TEXT NOT NULL DEFAULT '{id}',
  schema JSONB NOT NULL DEFAULT '{}'::jsonb,
  detail_layout JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_by UUID REFERENCES users(id) ON DELETE SET NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT project_forms_key_check CHECK (key ~ '^[a-z][a-z0-9_]*$'),
  CONSTRAINT project_forms_schema_object CHECK (jsonb_typeof(schema) = 'object'),
  CONSTRAINT project_forms_detail_layout_object CHECK (jsonb_typeof(detail_layout) = 'object'),
  UNIQUE(project_id, key)
);

CREATE INDEX IF NOT EXISTS idx_project_forms_workspace
  ON project_forms(workspace_id, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_project_forms_project
  ON project_forms(project_id, key);

CREATE TABLE IF NOT EXISTS form_views (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  form_id UUID NOT NULL REFERENCES project_forms(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  name TEXT NOT NULL,
  view_type TEXT NOT NULL,
  config JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_by UUID REFERENCES users(id) ON DELETE SET NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT form_views_key_check CHECK (key ~ '^[a-z][a-z0-9_]*$'),
  CONSTRAINT form_views_type_check CHECK (view_type IN ('grid', 'detail', 'kanban', 'calendar', 'card', 'timeline')),
  CONSTRAINT form_views_config_object CHECK (jsonb_typeof(config) = 'object'),
  UNIQUE(form_id, key)
);

CREATE INDEX IF NOT EXISTS idx_form_views_form
  ON form_views(form_id, view_type);

CREATE TABLE IF NOT EXISTS form_records (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  form_id UUID NOT NULL REFERENCES project_forms(id) ON DELETE CASCADE,
  title TEXT NOT NULL DEFAULT '',
  values JSONB NOT NULL DEFAULT '{}'::jsonb,
  source JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_by UUID REFERENCES users(id) ON DELETE SET NULL,
  updated_by UUID REFERENCES users(id) ON DELETE SET NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT form_records_values_object CHECK (jsonb_typeof(values) = 'object'),
  CONSTRAINT form_records_source_object CHECK (jsonb_typeof(source) = 'object')
);

CREATE INDEX IF NOT EXISTS idx_form_records_workspace
  ON form_records(workspace_id, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_form_records_project_form
  ON form_records(project_id, form_id, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_form_records_values_gin
  ON form_records USING gin(values jsonb_path_ops);

CREATE TABLE IF NOT EXISTS form_record_links (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  source_record_id UUID NOT NULL REFERENCES form_records(id) ON DELETE CASCADE,
  target_type TEXT NOT NULL,
  target_id TEXT NOT NULL,
  relation_key TEXT NOT NULL,
  relation_type TEXT NOT NULL,
  metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_by UUID REFERENCES users(id) ON DELETE SET NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT form_record_links_target_type_check CHECK (
    target_type IN ('form_record', 'work_item', 'project_resource', 'external_object')
  ),
  CONSTRAINT form_record_links_relation_type_check CHECK (
    relation_type IN ('parent_child', 'relation', 'external_relation')
  ),
  CONSTRAINT form_record_links_metadata_object CHECK (jsonb_typeof(metadata) = 'object')
);

CREATE INDEX IF NOT EXISTS idx_form_record_links_source
  ON form_record_links(source_record_id, relation_type);

CREATE INDEX IF NOT EXISTS idx_form_record_links_target
  ON form_record_links(target_type, target_id);

CREATE TABLE IF NOT EXISTS form_events (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  form_id UUID REFERENCES project_forms(id) ON DELETE SET NULL,
  record_id UUID REFERENCES form_records(id) ON DELETE SET NULL,
  event_type TEXT NOT NULL,
  actor_id UUID REFERENCES users(id) ON DELETE SET NULL,
  source JSONB NOT NULL DEFAULT '{}'::jsonb,
  payload JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT form_events_source_object CHECK (jsonb_typeof(source) = 'object'),
  CONSTRAINT form_events_payload_object CHECK (jsonb_typeof(payload) = 'object')
);

CREATE INDEX IF NOT EXISTS idx_form_events_record
  ON form_events(record_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_form_events_project
  ON form_events(project_id, created_at DESC);

CREATE TABLE IF NOT EXISTS form_record_field_index (
  record_id UUID NOT NULL REFERENCES form_records(id) ON DELETE CASCADE,
  form_id UUID NOT NULL REFERENCES project_forms(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  field_key TEXT NOT NULL,
  value_text TEXT,
  value_decimal NUMERIC,
  value_datetime TIMESTAMPTZ,
  value_bool BOOLEAN,
  PRIMARY KEY (record_id, field_key)
);

CREATE INDEX IF NOT EXISTS idx_form_field_index_decimal
  ON form_record_field_index(form_id, field_key, value_decimal)
  WHERE value_decimal IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_form_field_index_text
  ON form_record_field_index(form_id, field_key, value_text)
  WHERE value_text IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_form_field_index_datetime
  ON form_record_field_index(form_id, field_key, value_datetime)
  WHERE value_datetime IS NOT NULL;

UPDATE project_types
SET enabled_capabilities = (
  SELECT jsonb_agg(DISTINCT value)
  FROM jsonb_array_elements_text(enabled_capabilities || '["forms"]'::jsonb) AS value
)
WHERE NOT enabled_capabilities ? 'forms';
