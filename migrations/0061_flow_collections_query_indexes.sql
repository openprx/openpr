-- Flow v0.6-C: typed Collection query indexes. Canonical state remains in collab documents;
-- these indexes only accelerate the synchronous, rebuildable projection tables.

CREATE INDEX IF NOT EXISTS idx_flow_records_collection_record
  ON flow_record_projections (collection_id, record_id);

CREATE INDEX IF NOT EXISTS idx_flow_records_collection_seq
  ON flow_record_projections (collection_id, document_seq DESC, record_id);

CREATE INDEX IF NOT EXISTS idx_flow_record_values_text
  ON flow_record_value_projections (collection_id, field_id, text_value, record_id)
  WHERE text_value IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_flow_record_values_number
  ON flow_record_value_projections (collection_id, field_id, number_value, record_id)
  WHERE number_value IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_flow_record_values_boolean
  ON flow_record_value_projections (collection_id, field_id, boolean_value, record_id)
  WHERE boolean_value IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_flow_record_values_date
  ON flow_record_value_projections (collection_id, field_id, date_value, record_id)
  WHERE date_value IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_flow_record_values_select
  ON flow_record_value_projections (collection_id, field_id, select_value, record_id)
  WHERE select_value IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_flow_record_values_multi_select
  ON flow_record_value_projections USING GIN (multi_select_value jsonb_path_ops)
  WHERE multi_select_value IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_flow_record_values_relation
  ON flow_record_value_projections USING GIN (relation_value jsonb_path_ops)
  WHERE relation_value IS NOT NULL;

CREATE OR REPLACE VIEW flow_collections_query_schema_guard AS
SELECT records.collection_id, records.record_id, values.field_id, values.document_seq
  FROM flow_record_projections records
  JOIN flow_record_value_projections values ON values.record_id = records.record_id
 WHERE false;
