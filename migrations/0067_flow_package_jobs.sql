-- Isolated v0.8 Flow package artifacts plus export/preview job ledgers.

CREATE TABLE IF NOT EXISTS flow_package_artifacts (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  actor_kind TEXT NOT NULL,
  actor_id UUID NOT NULL,
  purpose TEXT NOT NULL,
  package_sha256 CHAR(64) NOT NULL,
  size_bytes BIGINT NOT NULL,
  package_bytes BYTEA NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT flow_package_artifacts_actor_kind_check CHECK (actor_kind IN ('user', 'bot')),
  CONSTRAINT flow_package_artifacts_purpose_check CHECK (purpose IN ('export', 'import')),
  CONSTRAINT flow_package_artifacts_hash_check CHECK (package_sha256 ~ '^[0-9a-f]{64}$'),
  CONSTRAINT flow_package_artifacts_size_check
    CHECK (size_bytes >= 0 AND size_bytes = octet_length(package_bytes)),
  CONSTRAINT flow_package_artifacts_expiry_check CHECK (expires_at > created_at)
);

CREATE INDEX IF NOT EXISTS idx_flow_package_artifacts_expiry
  ON flow_package_artifacts(expires_at);

CREATE TABLE IF NOT EXISTS flow_export_jobs (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  object_id UUID REFERENCES flow_objects(id) ON DELETE CASCADE,
  project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
  scope_kind TEXT NOT NULL,
  format TEXT NOT NULL,
  at_seq BIGINT,
  include_history BOOLEAN NOT NULL DEFAULT false,
  actor_kind TEXT NOT NULL,
  actor_id UUID NOT NULL,
  idempotency_key TEXT NOT NULL,
  request_hash CHAR(64) NOT NULL,
  status TEXT NOT NULL DEFAULT 'completed',
  artifact_id UUID REFERENCES flow_package_artifacts(id) ON DELETE SET NULL,
  package_sha256 CHAR(64),
  size_bytes BIGINT,
  error TEXT,
  expires_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT flow_export_jobs_scope_check CHECK (
    (scope_kind = 'object' AND object_id IS NOT NULL AND project_id IS NULL) OR
    (scope_kind = 'workspace' AND object_id IS NULL)
  ),
  CONSTRAINT flow_export_jobs_format_check CHECK (format IN ('json', 'markdown', 'csv', 'package')),
  CONSTRAINT flow_export_jobs_actor_kind_check CHECK (actor_kind IN ('user', 'bot')),
  CONSTRAINT flow_export_jobs_status_check CHECK (status IN ('queued', 'running', 'completed', 'failed')),
  CONSTRAINT flow_export_jobs_completed_check CHECK (
    status <> 'completed' OR
    (artifact_id IS NOT NULL AND package_sha256 IS NOT NULL AND size_bytes IS NOT NULL AND expires_at IS NOT NULL)
  ),
  CONSTRAINT flow_export_jobs_idempotency_key
    UNIQUE (workspace_id, actor_kind, actor_id, idempotency_key)
);

CREATE INDEX IF NOT EXISTS idx_flow_export_jobs_workspace
  ON flow_export_jobs(workspace_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_flow_export_jobs_object
  ON flow_export_jobs(object_id, created_at DESC) WHERE object_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS flow_import_previews (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  artifact_id UUID NOT NULL REFERENCES flow_package_artifacts(id) ON DELETE CASCADE,
  actor_kind TEXT NOT NULL,
  actor_id UUID NOT NULL,
  package_sha256 CHAR(64) NOT NULL,
  mapping_hash CHAR(64) NOT NULL,
  mapping JSONB NOT NULL,
  request JSONB NOT NULL,
  manifest_summary JSONB NOT NULL,
  conflicts JSONB NOT NULL DEFAULT '[]'::jsonb,
  warnings JSONB NOT NULL DEFAULT '[]'::jsonb,
  estimated_changes JSONB NOT NULL,
  idempotency_key TEXT NOT NULL,
  request_hash CHAR(64) NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL,
  committed_import_id UUID REFERENCES flow_import_jobs(id),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT flow_import_previews_actor_kind_check CHECK (actor_kind IN ('user', 'bot')),
  CONSTRAINT flow_import_previews_json_check CHECK (
    jsonb_typeof(mapping) = 'object' AND jsonb_typeof(request) = 'object' AND
    jsonb_typeof(manifest_summary) = 'object' AND jsonb_typeof(conflicts) = 'array' AND
    jsonb_typeof(warnings) = 'array' AND jsonb_typeof(estimated_changes) = 'object'
  ),
  CONSTRAINT flow_import_previews_expiry_check CHECK (expires_at > created_at),
  CONSTRAINT flow_import_previews_idempotency_key
    UNIQUE (workspace_id, actor_kind, actor_id, idempotency_key)
);

CREATE INDEX IF NOT EXISTS idx_flow_import_previews_expiry
  ON flow_import_previews(expires_at);

-- Widen legacy-only lineage into the v0.8 heterogeneous object/document/relation mapping shape.
ALTER TABLE flow_import_lineage
  DROP CONSTRAINT IF EXISTS flow_import_lineage_source_kind_check,
  DROP CONSTRAINT IF EXISTS flow_import_lineage_source_table_check,
  DROP CONSTRAINT IF EXISTS flow_import_lineage_same_workspace_check,
  DROP CONSTRAINT IF EXISTS flow_import_lineage_idempotency_key;

-- Package provenance may name a workspace from another installation. It is an opaque source id,
-- never a local canonical workspace reference; the target workspace keeps its foreign key.
ALTER TABLE flow_import_jobs
  DROP CONSTRAINT IF EXISTS flow_import_jobs_source_workspace_id_fkey;
ALTER TABLE flow_import_lineage
  DROP CONSTRAINT IF EXISTS flow_import_lineage_source_workspace_id_fkey;

ALTER TABLE flow_import_lineage
  ADD COLUMN IF NOT EXISTS package_sha256 CHAR(64),
  ADD COLUMN IF NOT EXISTS target_kind TEXT,
  ADD COLUMN IF NOT EXISTS target_id UUID;

ALTER TABLE flow_import_lineage
  ALTER COLUMN source_table DROP NOT NULL,
  ALTER COLUMN source_workspace_id DROP NOT NULL,
  ALTER COLUMN target_object_id DROP NOT NULL,
  ALTER COLUMN target_document_id DROP NOT NULL;

ALTER TABLE flow_import_lineage
  DROP CONSTRAINT IF EXISTS flow_import_lineage_target_shape_check,
  DROP CONSTRAINT IF EXISTS flow_import_lineage_same_workspace_check,
  ADD CONSTRAINT flow_import_lineage_source_kind_check
    CHECK (source_kind IN ('legacy_pages', 'flow_package')),
  ADD CONSTRAINT flow_import_lineage_source_table_check
    CHECK (
      (source_kind = 'legacy_pages' AND source_table = 'pages') OR
      (source_kind = 'flow_package' AND source_table IS NULL)
    ),
  ADD CONSTRAINT flow_import_lineage_same_workspace_check
    CHECK (source_kind = 'flow_package' OR source_workspace_id = target_workspace_id),
  ADD CONSTRAINT flow_import_lineage_target_shape_check CHECK (
    (source_kind = 'legacy_pages' AND target_object_id IS NOT NULL AND target_document_id IS NOT NULL
      AND package_sha256 IS NULL AND target_kind IS NULL AND target_id IS NULL) OR
    (source_kind = 'flow_package' AND package_sha256 ~ '^[0-9a-f]{64}$'
      AND target_kind IN ('object', 'document', 'relation') AND target_id IS NOT NULL
      AND target_object_id IS NULL AND target_document_id IS NULL)
  );

CREATE UNIQUE INDEX IF NOT EXISTS idx_flow_import_lineage_legacy_idempotency
  ON flow_import_lineage(target_workspace_id, source_kind, source_id, source_content_hash)
  WHERE source_kind = 'legacy_pages';
CREATE UNIQUE INDEX IF NOT EXISTS idx_flow_import_lineage_package_idempotency
  ON flow_import_lineage(target_workspace_id, package_sha256, source_kind, source_id)
  WHERE source_kind = 'flow_package';
