-- Flow v0.6-B Collections core: canonical object types and synchronous projections.

ALTER TABLE flow_objects
  DROP CONSTRAINT IF EXISTS flow_objects_object_type_check;

ALTER TABLE flow_objects
  ADD CONSTRAINT flow_objects_object_type_check
  CHECK (object_type IN ('navigator', 'page', 'collection', 'record'));

CREATE TABLE IF NOT EXISTS flow_collection_projections (
  collection_id UUID PRIMARY KEY REFERENCES flow_objects(id) ON DELETE CASCADE,
  document_id UUID NOT NULL UNIQUE REFERENCES collab_documents(id) ON DELETE CASCADE,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
  schema_seq BIGINT NOT NULL DEFAULT 0,
  display_settings JSONB NOT NULL DEFAULT '{}'::jsonb,
  client_crdt_enabled BOOLEAN NOT NULL DEFAULT true,
  field_secrecy_enabled BOOLEAN NOT NULL DEFAULT false,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT flow_collection_projections_schema_seq_check CHECK (schema_seq >= 0),
  CONSTRAINT flow_collection_projections_display_settings_check
    CHECK (jsonb_typeof(display_settings) = 'object'),
  CONSTRAINT flow_collection_projections_secrecy_crdt_check
    CHECK (NOT (client_crdt_enabled AND field_secrecy_enabled))
);

CREATE TABLE IF NOT EXISTS flow_field_projections (
  collection_id UUID NOT NULL REFERENCES flow_collection_projections(collection_id) ON DELETE CASCADE,
  field_id UUID NOT NULL,
  field_type TEXT NOT NULL,
  label TEXT NOT NULL,
  config JSONB NOT NULL DEFAULT '{}'::jsonb,
  position BIGINT NOT NULL,
  document_seq BIGINT NOT NULL,
  archived_at TIMESTAMPTZ,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (collection_id, field_id),
  CONSTRAINT flow_field_projections_type_check CHECK (
    field_type IN ('text', 'number', 'boolean', 'date', 'select', 'multi_select', 'relation')
  ),
  CONSTRAINT flow_field_projections_label_check CHECK (length(trim(label)) > 0),
  CONSTRAINT flow_field_projections_config_check CHECK (jsonb_typeof(config) = 'object'),
  CONSTRAINT flow_field_projections_position_check CHECK (position >= 0),
  CONSTRAINT flow_field_projections_document_seq_check CHECK (document_seq >= 0)
);

CREATE TABLE IF NOT EXISTS flow_view_projections (
  collection_id UUID NOT NULL REFERENCES flow_collection_projections(collection_id) ON DELETE CASCADE,
  view_id UUID NOT NULL,
  view_type TEXT NOT NULL,
  name TEXT NOT NULL,
  config JSONB NOT NULL DEFAULT '{}'::jsonb,
  position BIGINT NOT NULL,
  document_seq BIGINT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (collection_id, view_id),
  CONSTRAINT flow_view_projections_type_check CHECK (view_type IN ('table', 'board')),
  CONSTRAINT flow_view_projections_name_check CHECK (length(trim(name)) > 0),
  CONSTRAINT flow_view_projections_config_check CHECK (jsonb_typeof(config) = 'object'),
  CONSTRAINT flow_view_projections_position_check CHECK (position >= 0),
  CONSTRAINT flow_view_projections_document_seq_check CHECK (document_seq >= 0)
);

CREATE TABLE IF NOT EXISTS flow_record_projections (
  record_id UUID PRIMARY KEY REFERENCES flow_objects(id) ON DELETE CASCADE,
  collection_id UUID NOT NULL REFERENCES flow_collection_projections(collection_id) ON DELETE CASCADE,
  document_id UUID NOT NULL UNIQUE REFERENCES collab_documents(id) ON DELETE CASCADE,
  properties JSONB NOT NULL DEFAULT '{}'::jsonb,
  has_body BOOLEAN NOT NULL DEFAULT false,
  document_seq BIGINT NOT NULL DEFAULT 0,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT flow_record_projections_properties_check CHECK (jsonb_typeof(properties) = 'object'),
  CONSTRAINT flow_record_projections_document_seq_check CHECK (document_seq >= 0)
);

-- Typed values are materialized synchronously now. Query-oriented secondary indexes are
-- intentionally deferred to the v0.6 query/index package.
CREATE TABLE IF NOT EXISTS flow_record_value_projections (
  record_id UUID NOT NULL REFERENCES flow_record_projections(record_id) ON DELETE CASCADE,
  collection_id UUID NOT NULL REFERENCES flow_collection_projections(collection_id) ON DELETE CASCADE,
  field_id UUID NOT NULL,
  field_type TEXT NOT NULL,
  text_value TEXT,
  number_value NUMERIC,
  boolean_value BOOLEAN,
  date_value TIMESTAMPTZ,
  select_value TEXT,
  multi_select_value JSONB,
  relation_value JSONB,
  document_seq BIGINT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (record_id, field_id),
  CONSTRAINT flow_record_value_projections_type_check CHECK (
    field_type IN ('text', 'number', 'boolean', 'date', 'select', 'multi_select', 'relation')
  ),
  CONSTRAINT flow_record_value_projections_multi_select_check
    CHECK (multi_select_value IS NULL OR jsonb_typeof(multi_select_value) = 'array'),
  CONSTRAINT flow_record_value_projections_relation_check
    CHECK (relation_value IS NULL OR jsonb_typeof(relation_value) = 'array'),
  CONSTRAINT flow_record_value_projections_document_seq_check CHECK (document_seq >= 0),
  CONSTRAINT flow_record_value_projections_field_fk
    FOREIGN KEY (collection_id, field_id)
    REFERENCES flow_field_projections(collection_id, field_id) ON DELETE CASCADE
);

COMMENT ON TABLE flow_collection_projections IS
  'Synchronous read model for one Collection document; canonical schema and views remain in collab_documents';
COMMENT ON TABLE flow_record_projections IS
  'One row per independently addressable Record document; a Collection document never contains the record set';
COMMENT ON TABLE flow_record_value_projections IS
  'Synchronous typed record values; query-oriented secondary indexes are added by the later query/index package';

-- A single migration probe that structurally depends on every new projection relation. If any
-- table is absent, this view cannot be created (or retained after a DROP without CASCADE).
CREATE OR REPLACE VIEW flow_collections_core_schema_guard AS
SELECT collection.collection_id, field.field_id, view_row.view_id, record.record_id,
       typed.field_type
  FROM flow_collection_projections collection
  JOIN flow_field_projections field ON field.collection_id = collection.collection_id
  JOIN flow_view_projections view_row ON view_row.collection_id = collection.collection_id
  JOIN flow_record_projections record ON record.collection_id = collection.collection_id
  JOIN flow_record_value_projections typed
    ON typed.record_id = record.record_id AND typed.field_id = field.field_id
 WHERE false;
