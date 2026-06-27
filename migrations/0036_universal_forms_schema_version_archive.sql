-- Universal forms schema versioning, archive model, and stable field identity.

ALTER TABLE project_forms
  ADD COLUMN IF NOT EXISTS schema_version INTEGER NOT NULL DEFAULT 1,
  ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ;

ALTER TABLE form_views
  ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ;

ALTER TABLE form_records
  ADD COLUMN IF NOT EXISTS schema_version INTEGER,
  ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ;

ALTER TABLE form_record_field_index
  ADD COLUMN IF NOT EXISTS field_id TEXT;

CREATE TABLE IF NOT EXISTS form_schema_versions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  form_id UUID NOT NULL REFERENCES project_forms(id) ON DELETE CASCADE,
  version INTEGER NOT NULL,
  schema JSONB NOT NULL,
  detail_layout JSONB NOT NULL DEFAULT '{}'::jsonb,
  changed_by UUID REFERENCES users(id) ON DELETE SET NULL,
  change_summary TEXT NOT NULL DEFAULT '',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(form_id, version)
);

CREATE INDEX IF NOT EXISTS idx_project_forms_active
  ON project_forms(project_id, updated_at DESC)
  WHERE archived_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_form_views_active
  ON form_views(form_id, view_type, updated_at DESC)
  WHERE archived_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_form_records_active
  ON form_records(form_id, updated_at DESC)
  WHERE archived_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_form_schema_versions_form
  ON form_schema_versions(form_id, version DESC);

CREATE INDEX IF NOT EXISTS idx_form_field_index_field_id
  ON form_record_field_index(form_id, field_id)
  WHERE field_id IS NOT NULL;

UPDATE form_records records
SET schema_version = forms.schema_version
FROM project_forms forms
WHERE records.form_id = forms.id
  AND records.schema_version IS NULL;

WITH forms_with_fields AS (
  SELECT
    forms.id,
    forms.schema,
    jsonb_agg(
      CASE
        WHEN field.value ? 'field_id' THEN field.value
        ELSE jsonb_set(
          field.value,
          '{field_id}',
          to_jsonb(
            'fld_' ||
            substr(md5(forms.id::text || ':' || COALESCE(field.value->>'key', field.ordinality::text)), 1, 16)
          ),
          true
        )
      END
      ORDER BY field.ordinality
    ) AS fields
  FROM project_forms forms
  CROSS JOIN LATERAL jsonb_array_elements(
    CASE
      WHEN jsonb_typeof(forms.schema->'fields') = 'array' THEN forms.schema->'fields'
      ELSE '[]'::jsonb
    END
  )
    WITH ORDINALITY AS field(value, ordinality)
  GROUP BY forms.id, forms.schema
)
UPDATE project_forms forms
SET schema = jsonb_set(forms.schema, '{fields}', forms_with_fields.fields, true)
FROM forms_with_fields
WHERE forms.id = forms_with_fields.id
  AND forms.schema IS DISTINCT FROM jsonb_set(forms.schema, '{fields}', forms_with_fields.fields, true);

INSERT INTO form_schema_versions (
  form_id, version, schema, detail_layout, changed_by, change_summary, created_at
)
SELECT
  id,
  schema_version,
  schema,
  detail_layout,
  created_by,
  'baseline schema version',
  created_at
FROM project_forms
ON CONFLICT (form_id, version) DO NOTHING;
