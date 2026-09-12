-- v0.7 Flow <-> Forms bridge. The bridge stores identities, previews, jobs and
-- lineage only; neither module's canonical payload is copied for background sync.

ALTER TABLE workspace_bots
  ADD COLUMN IF NOT EXISTS transport_surface TEXT NOT NULL DEFAULT 'rest';

ALTER TABLE workspace_bots
  DROP CONSTRAINT IF EXISTS workspace_bots_transport_surface_check;
ALTER TABLE workspace_bots
  ADD CONSTRAINT workspace_bots_transport_surface_check CHECK (
    transport_surface IN ('rest', 'mcp_http', 'mcp_sse', 'mcp_stdio', 'cli', 'cli_tools_call')
  );

ALTER TABLE flow_workspace_settings
  ADD COLUMN IF NOT EXISTS bridge_enabled BOOLEAN NOT NULL DEFAULT true;

CREATE TABLE IF NOT EXISTS flow_bridge_references (
  id UUID PRIMARY KEY,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  source_object_id UUID NOT NULL REFERENCES flow_objects(id) ON DELETE CASCADE,
  target_type TEXT NOT NULL CHECK (target_type IN ('form', 'form_record')),
  target_id UUID NOT NULL,
  display JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(display) = 'object'),
  idempotency_key TEXT NOT NULL,
  created_by UUID NOT NULL,
  created_by_kind TEXT NOT NULL CHECK (created_by_kind IN ('user', 'bot')),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  removed_at TIMESTAMPTZ,
  UNIQUE (workspace_id, idempotency_key)
);

CREATE INDEX IF NOT EXISTS idx_flow_bridge_references_source
  ON flow_bridge_references(source_object_id, created_at, id)
  WHERE removed_at IS NULL;

CREATE TABLE IF NOT EXISTS flow_conversion_previews (
  id UUID PRIMARY KEY,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  actor_id UUID NOT NULL,
  actor_kind TEXT NOT NULL CHECK (actor_kind IN ('user', 'bot')),
  source_object_id UUID NOT NULL REFERENCES flow_objects(id) ON DELETE CASCADE,
  source_frontier TEXT NOT NULL,
  target_type TEXT NOT NULL CHECK (target_type IN ('form', 'form_record')),
  target_project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  target_form_id UUID REFERENCES project_forms(id) ON DELETE CASCADE,
  target_schema_version INTEGER NOT NULL,
  mapping JSONB NOT NULL CHECK (jsonb_typeof(mapping) = 'object'),
  permission_snapshot JSONB NOT NULL CHECK (jsonb_typeof(permission_snapshot) = 'object'),
  idempotency_key TEXT NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, idempotency_key)
);

CREATE TABLE IF NOT EXISTS flow_object_lineage (
  id UUID PRIMARY KEY,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  source_object_id UUID NOT NULL REFERENCES flow_objects(id) ON DELETE RESTRICT,
  source_frontier TEXT NOT NULL,
  target_type TEXT NOT NULL CHECK (target_type IN ('form', 'form_record')),
  target_id UUID NOT NULL,
  target_version INTEGER NOT NULL,
  relation TEXT NOT NULL CHECK (relation IN ('derived_from', 'published_as')),
  created_by UUID NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, source_object_id, target_type, target_id, relation)
);

CREATE INDEX IF NOT EXISTS idx_flow_object_lineage_source
  ON flow_object_lineage(source_object_id, created_at, id);
CREATE INDEX IF NOT EXISTS idx_flow_object_lineage_target
  ON flow_object_lineage(workspace_id, target_type, target_id);

CREATE TABLE IF NOT EXISTS flow_conversion_jobs (
  id UUID PRIMARY KEY,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  preview_id UUID NOT NULL REFERENCES flow_conversion_previews(id) ON DELETE RESTRICT,
  actor_id UUID NOT NULL,
  actor_kind TEXT NOT NULL CHECK (actor_kind IN ('user', 'bot')),
  source_object_id UUID NOT NULL REFERENCES flow_objects(id) ON DELETE RESTRICT,
  source_frontier TEXT NOT NULL,
  target_type TEXT NOT NULL CHECK (target_type IN ('form', 'form_record')),
  target_schema_version INTEGER NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('started', 'completed', 'failed')),
  lineage_id UUID REFERENCES flow_object_lineage(id) ON DELETE RESTRICT,
  created_target_ids UUID[] NOT NULL DEFAULT ARRAY[]::uuid[],
  warnings JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(warnings) = 'array'),
  error_code TEXT,
  idempotency_key TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, idempotency_key),
  UNIQUE (preview_id)
);

CREATE INDEX IF NOT EXISTS idx_flow_conversion_jobs_source
  ON flow_conversion_jobs(source_object_id, created_at, id);

CREATE OR REPLACE VIEW flow_forms_bridge_schema_guard AS
SELECT
  to_regclass('public.flow_bridge_references') IS NOT NULL AS has_references,
  to_regclass('public.flow_conversion_previews') IS NOT NULL AS has_previews,
  to_regclass('public.flow_conversion_jobs') IS NOT NULL AS has_jobs,
  to_regclass('public.flow_object_lineage') IS NOT NULL AS has_lineage;
