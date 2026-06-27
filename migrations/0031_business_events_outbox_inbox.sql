-- Unified business event fabric for API, MCP, webhook, connector, CLI, and plugins.

CREATE TABLE IF NOT EXISTS business_events (
  id UUID PRIMARY KEY,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
  event_type TEXT NOT NULL,
  aggregate_type TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  actor_id UUID REFERENCES users(id) ON DELETE SET NULL,
  source JSONB NOT NULL DEFAULT '{}'::jsonb,
  payload JSONB NOT NULL DEFAULT '{}'::jsonb,
  metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
  correlation_id UUID,
  causation_id UUID,
  idempotency_key TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT business_events_event_type_check CHECK (event_type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
  CONSTRAINT business_events_aggregate_type_check CHECK (aggregate_type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$'),
  CONSTRAINT business_events_aggregate_id_check CHECK (length(trim(aggregate_id)) > 0),
  CONSTRAINT business_events_source_object CHECK (jsonb_typeof(source) = 'object'),
  CONSTRAINT business_events_payload_object CHECK (jsonb_typeof(payload) = 'object'),
  CONSTRAINT business_events_metadata_object CHECK (jsonb_typeof(metadata) = 'object')
);

CREATE INDEX IF NOT EXISTS idx_business_events_project_created
  ON business_events(project_id, created_at DESC)
  WHERE project_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_business_events_workspace_created
  ON business_events(workspace_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_business_events_type_created
  ON business_events(event_type, created_at DESC);

CREATE UNIQUE INDEX IF NOT EXISTS idx_business_events_idempotency
  ON business_events(workspace_id, idempotency_key)
  WHERE idempotency_key IS NOT NULL;

CREATE TABLE IF NOT EXISTS event_outbox (
  id UUID PRIMARY KEY,
  business_event_id UUID NOT NULL UNIQUE REFERENCES business_events(id) ON DELETE CASCADE,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
  event_type TEXT NOT NULL,
  aggregate_type TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  payload JSONB NOT NULL,
  headers JSONB NOT NULL DEFAULT '{}'::jsonb,
  status TEXT NOT NULL DEFAULT 'pending',
  attempts INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER NOT NULL DEFAULT 10,
  available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  leased_until TIMESTAMPTZ,
  last_error TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  dispatched_at TIMESTAMPTZ,
  CONSTRAINT event_outbox_status_check CHECK (status IN ('pending', 'leased', 'dispatched', 'failed', 'cancelled')),
  CONSTRAINT event_outbox_attempts_check CHECK (attempts >= 0 AND max_attempts > 0),
  CONSTRAINT event_outbox_payload_object CHECK (jsonb_typeof(payload) = 'object'),
  CONSTRAINT event_outbox_headers_object CHECK (jsonb_typeof(headers) = 'object')
);

CREATE INDEX IF NOT EXISTS idx_event_outbox_pickup
  ON event_outbox(status, available_at, created_at)
  WHERE status IN ('pending', 'failed');

CREATE INDEX IF NOT EXISTS idx_event_outbox_project_created
  ON event_outbox(project_id, created_at DESC)
  WHERE project_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS event_inbox (
  id UUID PRIMARY KEY,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
  source_kind TEXT NOT NULL,
  source_id TEXT,
  idempotency_key TEXT NOT NULL,
  event_type TEXT NOT NULL,
  payload JSONB NOT NULL DEFAULT '{}'::jsonb,
  status TEXT NOT NULL DEFAULT 'received',
  attempts INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  processed_at TIMESTAMPTZ,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT event_inbox_status_check CHECK (status IN ('received', 'processing', 'processed', 'failed')),
  CONSTRAINT event_inbox_payload_object CHECK (jsonb_typeof(payload) = 'object'),
  CONSTRAINT event_inbox_idempotency_key_check CHECK (length(trim(idempotency_key)) > 0)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_event_inbox_idempotency
  ON event_inbox(source_kind, idempotency_key);

CREATE INDEX IF NOT EXISTS idx_event_inbox_project_received
  ON event_inbox(project_id, received_at DESC)
  WHERE project_id IS NOT NULL;
