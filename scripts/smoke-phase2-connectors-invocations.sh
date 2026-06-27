#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEBHOOK_DIR="${OPENPR_WEBHOOK_DIR:-/opt/worker/code/openpr-webhook}"
POSTGRES_PORT="${OPENPR_SMOKE_PG_PORT:-5366}"
PG_SUPERUSER="${OPENPR_SMOKE_PG_SUPERUSER:-postgres}"
DB_NAME="openpr_phase2_smoke_$$_$(date +%s)"
DB_USER="openpr_phase2_smoke_$$_$(date +%s)"
DB_PASSWORD="$(openssl rand -hex 12)"
API_PORT="${OPENPR_SMOKE_API_PORT:-$((17280 + ($$ % 1000)))}"
WEBHOOK_PORT="${OPENPR_SMOKE_WEBHOOK_PORT:-$((19280 + ($$ % 1000)))}"
TMP_DIR="$(mktemp -d /tmp/openpr-phase2-smoke.XXXXXX)"
API_LOG="$TMP_DIR/api.log"
WORKER_LOG="$TMP_DIR/worker.log"
WEBHOOK_LOG="$TMP_DIR/webhook.log"
JWT_SECRET="openpr-phase2-smoke-secret"

OWNER_ID="11111111-1111-4111-8111-111111111111"
WORKSPACE_ID="22222222-2222-4222-8222-222222222222"
BOT_ID="33333333-3333-4333-8333-333333333333"
WEBHOOK_ID="44444444-4444-4444-8444-444444444444"
BOT_TOKEN="opr_phase2_workspace_$$_$(date +%s)"

api_pid=""
worker_pid=""
webhook_pid=""

cleanup() {
  local exit_code=$?
  for pid in "$worker_pid" "$webhook_pid" "$api_pid"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  sudo -n -u "$PG_SUPERUSER" psql -p "$POSTGRES_PORT" -d postgres -v ON_ERROR_STOP=1 -q <<SQL >/dev/null 2>&1 || true
SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '$DB_NAME';
DROP DATABASE IF EXISTS "$DB_NAME";
DROP ROLE IF EXISTS "$DB_USER";
SQL
  if [[ $exit_code -ne 0 ]]; then
    echo "Smoke failed. Logs are in $TMP_DIR" >&2
  else
    rm -rf "$TMP_DIR"
  fi
}
trap cleanup EXIT

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "Missing required command: $1" >&2
    exit 2
  }
}

wait_http() {
  local url="$1"
  local name="$2"
  for _ in $(seq 1 120); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "Timed out waiting for $name at $url" >&2
  return 1
}

psql_smoke() {
  PGPASSWORD="$DB_PASSWORD" psql -h 127.0.0.1 -p "$POSTGRES_PORT" -U "$DB_USER" -d "$DB_NAME" "$@"
}

sha256_hex() {
  TOKEN_VALUE="$1" node -e "const crypto=require('crypto'); process.stdout.write(crypto.createHash('sha256').update(process.env.TOKEN_VALUE).digest('hex'))"
}

require_cmd curl
require_cmd node
require_cmd openssl
require_cmd psql
require_cmd sudo

cargo build -q -p api --bin api -p worker --bin worker -p mcp-server --bin mcp-server
(cd "$WEBHOOK_DIR" && cargo build -q)

sudo -n -u "$PG_SUPERUSER" psql -p "$POSTGRES_PORT" -d postgres -v ON_ERROR_STOP=1 -q <<SQL
CREATE ROLE "$DB_USER" LOGIN PASSWORD '$DB_PASSWORD';
CREATE DATABASE "$DB_NAME" OWNER "$DB_USER";
SQL

DATABASE_URL="postgres://$DB_USER:$DB_PASSWORD@127.0.0.1:$POSTGRES_PORT/$DB_NAME"

BIND_ADDR="127.0.0.1:$API_PORT" \
DATABASE_URL="$DATABASE_URL" \
JWT_SECRET="$JWT_SECRET" \
RUST_LOG="${RUST_LOG:-api=info,openpr=info}" \
"$ROOT_DIR/target/debug/api" >"$API_LOG" 2>&1 &
api_pid=$!
wait_http "http://127.0.0.1:$API_PORT/health" "OpenPR API"

BOT_HASH="$(sha256_hex "$BOT_TOKEN")"

psql_smoke -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES
  ('$OWNER_ID', 'owner-phase2@example.local', '', 'Phase 2 Owner', 'admin', true, 'human', NULL, now(), now()),
  ('$BOT_ID', 'bot-phase2@example.local', '', 'Phase 2 MCP Bot', 'user', true, 'bot', 'mcp', now(), now());

INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE_ID', 'phase2-smoke', 'Phase 2 Smoke', '$OWNER_ID', now(), now());

INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES
  ('$WORKSPACE_ID', '$OWNER_ID', 'owner', now()),
  ('$WORKSPACE_ID', '$BOT_ID', 'member', now());

INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by, is_active, created_at, updated_at)
VALUES ('$BOT_ID', '$WORKSPACE_ID', 'Phase 2 MCP Bot', '$BOT_HASH', substring('$BOT_TOKEN' from 1 for 8), '["read","write","admin"]'::jsonb, '$OWNER_ID', true, now(), now());

INSERT INTO webhooks (id, workspace_id, name, url, secret, events, active, created_by, bot_user_id, metadata, created_at, updated_at)
VALUES (
  '$WEBHOOK_ID',
  '$WORKSPACE_ID',
  'Phase 2 AI Task Webhook',
  'http://127.0.0.1:$WEBHOOK_PORT/webhook',
  'phase2-smoke-secret',
  '["comment.created"]'::jsonb,
  true,
  '$OWNER_ID',
  '$BOT_ID',
  '{"source":"phase2_ai_task_worker_smoke"}'::jsonb,
  now(),
  now()
);
SQL

cat >"$TMP_DIR/openpr-webhook.toml" <<EOF
[server]
listen = "127.0.0.1:$WEBHOOK_PORT"

[security]
allow_unsigned = true

[[agents]]
id = "phase2-connector-agent"
name = "Phase 2 Connector Agent"
agent_type = "custom"
message_template = "{event}: {title}"

[agents.route]
bot_names = ["Phase 2 Connector Agent", "Phase 2 MCP Bot"]
bot_agent_types = ["webhook", "mcp"]
project_types = ["contract_review"]
trigger_kinds = ["manual", "mention"]

[agents.custom]
command = "/bin/echo"
args = ["phase2-connector-openpr-webhook-ok"]
EOF

"$WEBHOOK_DIR/target/debug/openpr-webhook" "$TMP_DIR/openpr-webhook.toml" >"$WEBHOOK_LOG" 2>&1 &
webhook_pid=$!
wait_http "http://127.0.0.1:$WEBHOOK_PORT/health" "openpr-webhook"

DATABASE_URL="$DATABASE_URL" \
JWT_SECRET="$JWT_SECRET" \
RUST_LOG="${RUST_LOG:-worker=info,openpr=info}" \
"$ROOT_DIR/target/debug/worker" --concurrency 1 >"$WORKER_LOG" 2>&1 &
worker_pid=$!

API_URL="http://127.0.0.1:$API_PORT" \
JWT_SECRET="$JWT_SECRET" \
OWNER_ID="$OWNER_ID" \
WORKSPACE_ID="$WORKSPACE_ID" \
BOT_ID="$BOT_ID" \
BOT_TOKEN="$BOT_TOKEN" \
MCP_BIN="$ROOT_DIR/target/debug/mcp-server" \
WEBHOOK_PORT="$WEBHOOK_PORT" \
WEBHOOK_LOG="$WEBHOOK_LOG" \
node --input-type=commonjs <<'NODE'
const crypto = require('crypto');
const fs = require('fs');
const { spawnSync } = require('child_process');

const apiUrl = process.env.API_URL;
const workspaceId = process.env.WORKSPACE_ID;
const botId = process.env.BOT_ID;
const jwtSecret = process.env.JWT_SECRET;
const botToken = process.env.BOT_TOKEN;
const mcpBin = process.env.MCP_BIN;
const webhookPort = process.env.WEBHOOK_PORT;
const webhookLog = process.env.WEBHOOK_LOG;

function b64url(value) {
  return Buffer.from(JSON.stringify(value)).toString('base64url');
}

function jwt(sub, email) {
  const now = Math.floor(Date.now() / 1000);
  const header = b64url({ alg: 'HS256', typ: 'JWT' });
  const body = b64url({ sub, email, token_type: 'access', iat: now, exp: now + 3600 });
  const sig = crypto.createHmac('sha256', jwtSecret).update(`${header}.${body}`).digest('base64url');
  return `${header}.${body}.${sig}`;
}

const ownerToken = jwt(process.env.OWNER_ID, 'owner-phase2@example.local');

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function request(token, method, path, body) {
  const response = await fetch(`${apiUrl}${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${token}`,
      ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  const payload = text ? JSON.parse(text) : null;
  if (!response.ok || payload?.code !== 0) {
    throw new Error(`${method} ${path} failed: HTTP ${response.status} ${text}`);
  }
  return payload.data;
}

function extractToolText(response) {
  const text = response.result?.content?.[0]?.text;
  if (!text) throw new Error(`MCP response has no text content: ${JSON.stringify(response)}`);
  return JSON.parse(text);
}

function mcpRequest(method, params) {
  const requestPayload = JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }) + '\n';
  const result = spawnSync(mcpBin, ['--api-url', apiUrl, '--bot-token', botToken, '--workspace-id', workspaceId, 'serve', '--transport', 'stdio'], {
    input: requestPayload,
    encoding: 'utf8',
    env: { ...process.env, RUST_LOG: 'error' },
    timeout: 20000,
  });
  if (result.status !== 0) {
    throw new Error(`MCP ${method} exited ${result.status}: ${result.stderr}`);
  }
  const line = result.stdout.split('\n').find((item) => item.trim().startsWith('{'));
  if (!line) throw new Error(`MCP ${method} produced no JSON: ${result.stdout} ${result.stderr}`);
  return JSON.parse(line);
}

function mcpTool(name, args = {}) {
  return extractToolText(mcpRequest('tools/call', { name, arguments: args }));
}

async function waitFor(check, label) {
  const deadline = Date.now() + 30000;
  let last = '';
  while (Date.now() < deadline) {
    try {
      const value = await check();
      if (value) return value;
    } catch (error) {
      last = error.message;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(`Timed out waiting for ${label}${last ? `: ${last}` : ''}`);
}

async function main() {
  const project = await request(ownerToken, 'POST', `/api/v1/workspaces/${workspaceId}/projects`, {
    name: 'Phase 2 Connector Project',
    key: 'P2CONN',
    description: 'Connector invocation smoke project',
    type_key: 'contract_review',
  });
  assert(project.type_key === 'contract_review', 'project should be contract_review');

  const connector = await request(ownerToken, 'POST', `/api/v1/workspaces/${workspaceId}/connectors`, {
    project_id: project.id,
    kind: 'webhook',
    name: 'Phase 2 Connector Agent',
    description: 'Routes connector invocation to openpr-webhook',
    endpoint: `http://127.0.0.1:${webhookPort}/webhook`,
    auth_policy: { mode: 'none' },
    capability_manifest: { capabilities: ['phase2.dispatch'] },
    is_active: true,
  });
  assert(connector.kind === 'webhook', 'connector should be webhook kind');

  const invocation = await request(ownerToken, 'POST', `/api/v1/projects/${project.id}/invocations`, {
    trigger_kind: 'manual',
    connector_id: connector.id,
    payload: {
      event: 'comment.created',
      title: 'Phase 2 connector invocation',
      project_type: 'contract_review',
      bot_context: {
        is_bot_task: true,
        bot_name: 'Phase 2 Connector Agent',
        bot_agent_type: 'webhook',
        project_type: 'contract_review',
        trigger_kind: 'manual',
      },
    },
  });
  assert(invocation.status === 'pending', 'new connector invocation should start pending');

  const dispatched = await waitFor(async () => {
    const current = await request(ownerToken, 'GET', `/api/v1/invocations/${invocation.id}`);
    return current.status === 'dispatched' ? current : null;
  }, 'worker-dispatched connector invocation');
  assert(dispatched.connector_id === connector.id, 'dispatched invocation should retain connector id');
  assert(dispatched.connector_kind === 'webhook', 'dispatched invocation should retain connector kind');

  await waitFor(() => {
    const log = fs.existsSync(webhookLog) ? fs.readFileSync(webhookLog, 'utf8') : '';
    return log.includes('Dispatching to agent: Phase 2 Connector Agent') || log.includes('phase2-connector-agent');
  }, 'openpr-webhook agent dispatch log');

  const beforeRead = (await request(ownerToken, 'GET', `/api/v1/projects/${project.id}/invocations`)).items.length;
  const mcpTools = mcpRequest('tools/list', {});
  const toolNames = (mcpTools.result?.tools ?? []).map((tool) => tool.name);
  for (const name of ['connectors.list', 'connectors.get', 'invocations.list', 'invocations.get', 'invocations.create', 'invocations.complete']) {
    assert(toolNames.includes(name), `MCP tools/list missing ${name}`);
  }

  const mcpConnectors = mcpTool('connectors.list', { project_id: project.id });
  assert(JSON.stringify(mcpConnectors).includes('Phase 2 Connector Agent'), 'MCP connectors.list should include created connector');
  const mcpInvocation = mcpTool('invocations.get', { invocation_id: invocation.id });
  assert(JSON.stringify(mcpInvocation).includes('dispatched'), 'MCP invocations.get should include dispatched status');
  const connectorResource = mcpRequest('resources/read', { uri: `openpr://projects/${project.id}/connectors` });
  assert(JSON.stringify(connectorResource).includes('Phase 2 Connector Agent'), 'MCP connectors resource should include connector metadata');

  const afterRead = (await request(ownerToken, 'GET', `/api/v1/projects/${project.id}/invocations`)).items.length;
  assert(afterRead === beforeRead, 'MCP read actions should not create noisy invocations');

  const mcpCreated = mcpTool('invocations.create', {
    project_id: project.id,
    trigger_kind: 'mcp',
    payload: { source: 'phase2-mcp-write-smoke' },
  });
  const mcpInvocationId = mcpCreated.data?.id;
  assert(mcpInvocationId, 'MCP invocations.create should return invocation id');
  const mcpCompleted = mcpTool('invocations.complete', {
    invocation_id: mcpInvocationId,
    result: { status: 'success', source: 'phase2-mcp-write-smoke' },
  });
  assert(mcpCompleted.data?.status === 'completed', 'MCP invocations.complete should complete invocation');

  await request(ownerToken, 'POST', `/api/v1/projects/${project.id}/ai-participants`, {
    id: botId,
    name: 'Phase 2 AI Task Bot',
    model: 'phase2-smoke-model',
    provider: 'local',
    capabilities: ['comment_requested'],
    max_domain_level: 'voter',
  });
  const aiTask = await request(ownerToken, 'POST', `/api/v1/projects/${project.id}/ai/tasks`, {
    ai_participant_id: botId,
    task_type: 'comment_requested',
    reference_type: 'comment',
    priority: 3,
    payload: { source: 'phase2-ai-task-invocation-smoke' },
    idempotency_key: `phase2-ai-task-invocation-${Date.now()}`,
  });
  assert(aiTask.status === 'pending', 'AI task should start pending');
  await request(ownerToken, 'POST', `/api/v1/ai/callbacks/task/${aiTask.id}/progress`, {
    phase: 'phase2-ai-task-invocation-smoke',
  });

  console.log('Phase 2 connectors/invocations live API+MCP+openpr-webhook smoke passed');
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

INVOCATION_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type IN ('invocation.created', 'invocation.completed') AND metadata->>'trigger_kind' = 'mcp'")"
if [[ "$INVOCATION_EVENT_COUNT" -lt 2 ]]; then
  echo "Expected at least 2 MCP invocation lifecycle business events, got $INVOCATION_EVENT_COUNT" >&2
  exit 1
fi

INVOCATION_OUTBOX_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM event_outbox WHERE event_type IN ('invocation.created', 'invocation.completed')")"
if [[ "$INVOCATION_OUTBOX_COUNT" -lt 2 ]]; then
  echo "Expected at least 2 MCP invocation lifecycle outbox rows, got $INVOCATION_OUTBOX_COUNT" >&2
  exit 1
fi

echo "Phase 2 invocation lifecycle business events and outbox smoke passed"

AI_TASK_INVOCATION_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type IN ('invocation.created', 'invocation.running') AND metadata->>'ai_task_invocation' = 'true' AND metadata->>'trigger_kind' = 'mention'")"
if [[ "$AI_TASK_INVOCATION_EVENT_COUNT" -lt 2 ]]; then
  echo "Expected at least 2 AI task invocation business events, got $AI_TASK_INVOCATION_EVENT_COUNT" >&2
  exit 1
fi

AI_TASK_INVOCATION_OUTBOX_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM event_outbox WHERE event_type IN ('invocation.created', 'invocation.running') AND payload->'metadata'->>'ai_task_invocation' = 'true'")"
if [[ "$AI_TASK_INVOCATION_OUTBOX_COUNT" -lt 2 ]]; then
  echo "Expected at least 2 AI task invocation outbox rows, got $AI_TASK_INVOCATION_OUTBOX_COUNT" >&2
  exit 1
fi

echo "Phase 2 AI task invocation business events and outbox smoke passed"

AI_TASK_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type IN ('ai_task.created', 'ai_task.progress') AND aggregate_type = 'ai_task' AND metadata->>'task_type' = 'comment_requested'")"
if [[ "$AI_TASK_EVENT_COUNT" -lt 2 ]]; then
  echo "Expected at least 2 AI task business events, got $AI_TASK_EVENT_COUNT" >&2
  exit 1
fi

AI_TASK_OUTBOX_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM event_outbox WHERE event_type IN ('ai_task.created', 'ai_task.progress') AND aggregate_type = 'ai_task' AND payload->'metadata'->>'task_type' = 'comment_requested'")"
if [[ "$AI_TASK_OUTBOX_COUNT" -lt 2 ]]; then
  echo "Expected at least 2 AI task outbox rows, got $AI_TASK_OUTBOX_COUNT" >&2
  exit 1
fi

echo "Phase 2 AI task business events and outbox smoke passed"

for _ in $(seq 1 60); do
  WORKER_AI_TASK_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type = 'ai_task.picked_up' AND aggregate_type = 'ai_task' AND metadata->>'worker_event' = 'true' AND metadata->>'task_type' = 'comment_requested'")"
  WORKER_INVOCATION_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type = 'invocation.dispatched' AND aggregate_type = 'invocation' AND metadata->>'worker_event' = 'true' AND metadata->>'trigger_kind' = 'mention'")"
  if [[ "$WORKER_AI_TASK_EVENT_COUNT" -ge 1 && "$WORKER_INVOCATION_EVENT_COUNT" -ge 1 ]]; then
    break
  fi
  sleep 1
done

if [[ "${WORKER_AI_TASK_EVENT_COUNT:-0}" -lt 1 ]]; then
  echo "Expected worker ai_task.picked_up business event, got ${WORKER_AI_TASK_EVENT_COUNT:-0}" >&2
  exit 1
fi

if [[ "${WORKER_INVOCATION_EVENT_COUNT:-0}" -lt 1 ]]; then
  echo "Expected worker invocation.dispatched business event, got ${WORKER_INVOCATION_EVENT_COUNT:-0}" >&2
  exit 1
fi

WORKER_OUTBOX_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM event_outbox WHERE event_type IN ('ai_task.picked_up', 'invocation.dispatched') AND payload->'metadata'->>'worker_event' = 'true'")"
if [[ "$WORKER_OUTBOX_COUNT" -lt 2 ]]; then
  echo "Expected at least 2 worker lifecycle outbox rows, got $WORKER_OUTBOX_COUNT" >&2
  exit 1
fi

echo "Phase 2 worker AI task dispatch business events and outbox smoke passed"
