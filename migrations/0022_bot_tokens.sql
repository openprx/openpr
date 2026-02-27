-- Migration: 0022_bot_tokens.sql
-- Bot tokens for MCP server and external API access

CREATE TABLE workspace_bots (
    id           UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID         NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name         VARCHAR(100) NOT NULL,
    token_hash   CHAR(64)     NOT NULL UNIQUE,  -- SHA-256 hex of raw token
    token_prefix CHAR(8)      NOT NULL,          -- First 8 chars of raw token for display
    permissions  JSONB        NOT NULL DEFAULT '["read"]'::jsonb,
    created_by   UUID         REFERENCES users(id) ON DELETE SET NULL,
    last_used_at TIMESTAMPTZ,
    expires_at   TIMESTAMPTZ,                    -- NULL = never expires
    is_active    BOOLEAN      NOT NULL DEFAULT true,
    created_at   TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ  NOT NULL DEFAULT now()
);

CREATE INDEX idx_workspace_bots_workspace_id ON workspace_bots(workspace_id);
CREATE INDEX idx_workspace_bots_token_hash ON workspace_bots(token_hash);
CREATE INDEX idx_workspace_bots_is_active ON workspace_bots(is_active);

COMMENT ON TABLE workspace_bots IS 'Bot tokens for MCP server and external API access';
COMMENT ON COLUMN workspace_bots.token_hash IS 'SHA-256(raw_token) stored as hex, raw token never stored';
COMMENT ON COLUMN workspace_bots.token_prefix IS 'First 8 chars of raw token for display/identification';
COMMENT ON COLUMN workspace_bots.permissions IS 'Array: ["read"], ["read","write"], ["read","write","admin"]';
