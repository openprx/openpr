CREATE TABLE IF NOT EXISTS plugins (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    manifest JSONB NOT NULL DEFAULT '{}'::jsonb,
    wasm_bytes BYTEA,
    wasm_sha256 TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    installed_by UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT plugins_key_format CHECK (key ~ '^[a-z][a-z0-9_]*$'),
    CONSTRAINT plugins_status_check CHECK (status IN ('active', 'disabled', 'failed')),
    CONSTRAINT plugins_version_present CHECK (length(trim(version)) > 0),
    CONSTRAINT plugins_project_key_version_unique UNIQUE (project_id, key, version)
);

CREATE INDEX IF NOT EXISTS idx_plugins_workspace_project ON plugins(workspace_id, project_id);
CREATE INDEX IF NOT EXISTS idx_plugins_status ON plugins(status);
CREATE INDEX IF NOT EXISTS idx_plugins_manifest_gin ON plugins USING GIN (manifest);

CREATE TABLE IF NOT EXISTS plugin_invocations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    plugin_id UUID REFERENCES plugins(id) ON DELETE SET NULL,
    plugin_key TEXT NOT NULL,
    hook_kind TEXT NOT NULL,
    status TEXT NOT NULL,
    input JSONB NOT NULL DEFAULT '{}'::jsonb,
    output JSONB NOT NULL DEFAULT '{}'::jsonb,
    error_message TEXT,
    duration_ms BIGINT NOT NULL DEFAULT 0,
    fuel_consumed BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT plugin_invocations_status_check CHECK (status IN ('completed', 'failed', 'timeout'))
);

CREATE INDEX IF NOT EXISTS idx_plugin_invocations_workspace_project ON plugin_invocations(workspace_id, project_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_plugin_invocations_plugin ON plugin_invocations(plugin_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_plugin_invocations_hook_kind ON plugin_invocations(hook_kind, created_at DESC);

UPDATE project_types
SET enabled_capabilities = CASE
    WHEN enabled_capabilities ? 'plugins' THEN enabled_capabilities
    ELSE enabled_capabilities || '["plugins"]'::jsonb
END
WHERE NOT (enabled_capabilities ? 'plugins');
