-- Sylvode Flow v0.5 accepted-projection full-text index (ADR-0009).
--
-- The index is a worker-owned copy instead of a generated column on
-- flow_object_projections. That separation is intentional: search freshness must be observable,
-- so the API needs both the accepted projection's current document_seq and the last sequence the
-- asynchronous search worker actually indexed. A generated column on the source row would make
-- those two frontiers indistinguishable and could not represent lag.
--
-- PostgreSQL's `simple` text-search configuration is used because Flow content is multilingual
-- and no single language stemmer is correct for a workspace. The trade-off is that this first
-- index provides token matching and ranking without language-specific stemming. GIN makes reads
-- fast, at the cost of additional write/storage amplification in this rebuildable table.

CREATE TABLE IF NOT EXISTS flow_search_index (
  object_id UUID PRIMARY KEY REFERENCES flow_objects(id) ON DELETE CASCADE,
  indexed_seq BIGINT NOT NULL,
  indexed_frontier BYTEA NOT NULL,
  title TEXT NOT NULL,
  plain_text TEXT NOT NULL,
  search_vector TSVECTOR GENERATED ALWAYS AS (
    setweight(to_tsvector('simple', coalesce(title, '')), 'A') ||
    setweight(to_tsvector('simple', coalesce(plain_text, '')), 'B')
  ) STORED,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT flow_search_index_seq_check CHECK (indexed_seq >= 0)
);

CREATE INDEX IF NOT EXISTS idx_flow_search_index_vector
  ON flow_search_index USING GIN (search_vector);

COMMENT ON TABLE flow_search_index IS
  'Worker-owned full-text copy sourced only from accepted flow_object_projections; indexed_seq/frontier are the observable search frontier';
COMMENT ON COLUMN flow_search_index.indexed_seq IS
  'Exact flow_object_projections.document_seq consumed by the worker; may move backward during an accepted projection rebuild';
COMMENT ON COLUMN flow_search_index.indexed_frontier IS
  'Exact accepted flow_object_projections.document_frontier consumed for this search row';

