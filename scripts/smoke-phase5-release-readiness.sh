#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
POSTGRES_PORT="${OPENPR_SMOKE_PG_PORT:-5366}"
PG_SUPERUSER="${OPENPR_SMOKE_PG_SUPERUSER:-postgres}"
DB_NAME="openpr_phase5_smoke_$$_$(date +%s)"
DB_USER="openpr_phase5_smoke_$$_$(date +%s)"
DB_PASSWORD="$(openssl rand -hex 12)"
API_PORT="${OPENPR_SMOKE_API_PORT:-$((17480 + ($$ % 1000)))}"
TMP_DIR="$(mktemp -d /tmp/openpr-phase5-smoke.XXXXXX)"
API_LOG="$TMP_DIR/api.log"
SMOKE_JWT_SECRET="openpr-phase5-smoke-secret"

OWNER_ID="11111111-1111-4111-8111-111111111111"
WORKSPACE_ID="22222222-2222-4222-8222-222222222222"
BOT_ID="33333333-3333-4333-8333-333333333333"
BOT_TOKEN="opr_phase5_workspace_$$_$(date +%s)"

api_pid=""

cleanup() {
  local exit_code=$?
  if [[ -n "$api_pid" ]] && kill -0 "$api_pid" 2>/dev/null; then
    kill "$api_pid" 2>/dev/null || true
    wait "$api_pid" 2>/dev/null || true
  fi
  sudo -n -u "$PG_SUPERUSER" psql -p "$POSTGRES_PORT" -d postgres -v ON_ERROR_STOP=1 -q <<SQL >/dev/null 2>&1 || true
SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '$DB_NAME';
DROP DATABASE IF EXISTS "$DB_NAME";
DROP ROLE IF EXISTS "$DB_USER";
SQL
  if [[ $exit_code -ne 0 ]]; then
    echo "Smoke failed. API log: $API_LOG" >&2
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

cargo build -q -p api --bin api -p mcp-server --bin mcp-server

sudo -n -u "$PG_SUPERUSER" psql -p "$POSTGRES_PORT" -d postgres -v ON_ERROR_STOP=1 -q <<SQL
CREATE ROLE "$DB_USER" LOGIN PASSWORD '$DB_PASSWORD';
CREATE DATABASE "$DB_NAME" OWNER "$DB_USER";
SQL

SMOKE_DATABASE_URL="postgres://$DB_USER:$DB_PASSWORD@127.0.0.1:$POSTGRES_PORT/$DB_NAME"

# The binaries read no environment variables; this file is their only configuration, and they
# refuse to start without one. It is written inside the 0700 directory mktemp made for this run
# and is removed with it, so the generated database password never lands in the repository.
# text logging rather than json: the only reader of the log file is a human debugging a failure.
APP_CONFIG="$TMP_DIR/openpr.toml"
cat >"$APP_CONFIG" <<EOF
[server]
app_name = "api"
bind_addr = "127.0.0.1:$API_PORT"

[database]
url = "$SMOKE_DATABASE_URL"

[auth]
jwt_secret = "$SMOKE_JWT_SECRET"

[logging]
filter = "${OPENPR_SMOKE_LOG_FILTER:-api=info,openpr=info}"
format = "text"
EOF

"$ROOT_DIR/target/debug/api" --config "$APP_CONFIG" >"$API_LOG" 2>&1 &
api_pid=$!
wait_http "http://127.0.0.1:$API_PORT/health" "OpenPR API"

# The mcp-server reads no environment variables either, and refuses to start without a
# configuration file. Its identity still arrives per call through --api-url/--bot-token/
# --workspace-id, so this file only has to keep the log stream out of the JSON-RPC client's way.
MCP_CONFIG="$TMP_DIR/openpr.mcp.toml"
cat >"$MCP_CONFIG" <<'TOML'
[logging]
filter = "error"
format = "text"
TOML

BOT_HASH="$(sha256_hex "$BOT_TOKEN")"

psql_smoke -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES
  ('$OWNER_ID', 'owner-phase5@example.local', '', 'Phase 5 Owner', 'admin', true, 'human', NULL, now(), now()),
  ('$BOT_ID', 'bot-phase5@example.local', '', 'Phase 5 MCP Bot', 'user', true, 'bot', 'mcp', now(), now());

INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE_ID', 'phase5-smoke', 'Phase 5 Smoke', '$OWNER_ID', now(), now());

INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES
  ('$WORKSPACE_ID', '$OWNER_ID', 'owner', now()),
  ('$WORKSPACE_ID', '$BOT_ID', 'member', now());

INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by, is_active, created_at, updated_at)
VALUES ('$BOT_ID', '$WORKSPACE_ID', 'Phase 5 MCP Bot', '$BOT_HASH', substring('$BOT_TOKEN' from 1 for 8), '["read","write","admin"]'::jsonb, '$OWNER_ID', true, now(), now());
SQL

API_URL="http://127.0.0.1:$API_PORT" \
ROOT_DIR="$ROOT_DIR" \
SMOKE_DATABASE_URL="$SMOKE_DATABASE_URL" \
SMOKE_JWT_SECRET="$SMOKE_JWT_SECRET" \
OWNER_ID="$OWNER_ID" \
WORKSPACE_ID="$WORKSPACE_ID" \
BOT_ID="$BOT_ID" \
BOT_TOKEN="$BOT_TOKEN" \
MCP_BIN="$ROOT_DIR/target/debug/mcp-server" \
MCP_CONFIG="$MCP_CONFIG" \
node --input-type=commonjs <<'NODE'
const crypto = require('crypto');
const fs = require('fs');
const { spawnSync } = require('child_process');

const apiUrl = process.env.API_URL;
const rootDir = process.env.ROOT_DIR;
const workspaceId = process.env.WORKSPACE_ID;
const jwtSecret = process.env.SMOKE_JWT_SECRET;
const botToken = process.env.BOT_TOKEN;
const mcpBin = process.env.MCP_BIN;
const mcpConfig = process.env.MCP_CONFIG;
const releaseSchemaPath = 'docs/schemas/openpr-project-release-readiness.schema.json';
const releaseSchema = JSON.parse(fs.readFileSync(`${rootDir}/${releaseSchemaPath}`, 'utf8'));

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

const ownerToken = jwt(process.env.OWNER_ID, 'owner-phase5@example.local');

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

function mcpRequest(method, params) {
  const requestPayload = JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }) + '\n';
  const result = spawnSync(
    mcpBin,
    ['--config', mcpConfig, '--api-url', apiUrl, '--bot-token', botToken, '--workspace-id', workspaceId, 'serve', '--transport', 'stdio'],
    { input: requestPayload, encoding: 'utf8', env: process.env, timeout: 20000 },
  );
  if (result.status !== 0) {
    throw new Error(`MCP ${method} exited ${result.status}: ${result.stderr}`);
  }
  const line = result.stdout.split('\n').find((item) => item.trim().startsWith('{'));
  if (!line) throw new Error(`MCP ${method} produced no JSON: ${result.stdout} ${result.stderr}`);
  return JSON.parse(line);
}

function mcpTool(name, args = {}) {
  const response = mcpRequest('tools/call', { name, arguments: args });
  const text = response.result?.content?.[0]?.text;
  if (!text) throw new Error(`MCP ${name} response has no text content: ${JSON.stringify(response)}`);
  return JSON.parse(text);
}

async function main() {
  const project = await request(ownerToken, 'POST', `/api/v1/workspaces/${workspaceId}/projects`, {
    name: 'Phase 5 Release Project',
    key: 'P5READY',
    description: 'Release readiness smoke project',
    type_key: 'code_project',
  });

  const invocation = await request(ownerToken, 'POST', `/api/v1/projects/${project.id}/invocations`, {
    trigger_kind: 'mcp',
    payload: { source: 'phase5-release-readiness' },
  });
  await request(ownerToken, 'POST', `/api/v1/invocations/${invocation.id}/complete`, {
    result: { status: 'success', source: 'phase5-release-readiness' },
  });
  await request(ownerToken, 'POST', `/api/v1/projects/${project.id}/check-results`, {
    invocation_id: invocation.id,
    action_class: 'comment_result',
    risk_level: 'low',
    title: 'Phase 5 release evidence',
    summary: 'Low-risk evidence artifact for release readiness.',
    result: { source: 'phase5-release-readiness' },
  });

  const psql = spawnSync('psql', [process.env.SMOKE_DATABASE_URL, '-v', 'ON_ERROR_STOP=1', '-q'], {
    input: `
      INSERT INTO governance_audit_logs (project_id, actor_id, action, resource_type, resource_id, old_value, new_value, metadata, created_at)
      VALUES ('${project.id}', '${process.env.OWNER_ID}', 'release.evidence.recorded', 'project', '${project.id}', NULL, '{"status":"reviewed"}'::jsonb, '{}'::jsonb, now());
    `,
    encoding: 'utf8',
  });
  if (psql.status !== 0) {
    throw new Error(`psql seed failed: ${psql.stderr}`);
  }

  const ready = await request(ownerToken, 'GET', `/api/v1/projects/${project.id}/release-readiness`);
  assert(ready.schema_version === releaseSchema.properties.schema_version.const, 'release readiness schema version should match schema');
  assert(ready.schema_path === releaseSchemaPath, 'release readiness schema path should be stable');
  assert(ready.status === 'ready', `release readiness should be ready: ${JSON.stringify(ready)}`);
  assert(ready.metrics.some((metric) => metric.key === 'result_artifacts' && metric.value >= 2), 'readiness should count result artifacts');
  assert(
    ready.next_actions?.some(
      (action) =>
        action.review_order === 1 &&
        action.key === 'review_release_evidence' &&
        action.source_gate === 'all_required_gates' &&
        action.blocking === false,
    ),
    'ready release readiness should include reviewer next action',
  );

  const tools = mcpRequest('tools/list', { project_id: project.id });
  const toolNames = (tools.result?.tools ?? []).map((tool) => tool.name);
  assert(toolNames.includes('release.readiness.get'), 'project-aware tools/list should include release.readiness.get');

  const mcpReady = mcpTool('release.readiness.get', { project_id: project.id });
  assert(mcpReady.data?.schema_version === 'openpr.project.release_readiness.v1', 'MCP release.readiness.get should expose schema version');
  assert(mcpReady.data?.status === 'ready', 'MCP release.readiness.get should return ready status');
  assert(
    mcpReady.data?.next_actions?.some((action) => action.key === 'review_release_evidence'),
    'MCP release.readiness.get should expose release next actions',
  );

  const resource = mcpRequest('resources/read', { uri: `openpr://projects/${project.id}/release-readiness` });
  assert(JSON.stringify(resource).includes('openpr.project.release_readiness.v1'), 'release readiness resource should expose schema version');
  assert(JSON.stringify(resource).includes('governance_audit_events'), 'release readiness resource should expose audit evidence counts');

  const pendingTask = spawnSync('psql', [process.env.SMOKE_DATABASE_URL, '-v', 'ON_ERROR_STOP=1', '-q'], {
    input: `
      INSERT INTO ai_tasks (project_id, ai_participant_id, task_type, status, payload, created_at, updated_at)
      VALUES ('${project.id}', '${process.env.BOT_ID}', 'release_gate_check', 'pending', '{}'::jsonb, now(), now());
    `,
    encoding: 'utf8',
  });
  if (pendingTask.status !== 0) {
    throw new Error(`psql pending task seed failed: ${pendingTask.stderr}`);
  }

  const blocked = await request(ownerToken, 'GET', `/api/v1/projects/${project.id}/release-readiness`);
  assert(blocked.status === 'blocked', 'pending AI task should block release readiness');
  assert(blocked.blockers.includes('no_pending_ai_tasks'), 'pending AI task blocker should be reported');
  assert(
    blocked.next_actions?.some(
      (action) =>
        action.key === 'resolve_no_pending_ai_tasks' &&
        action.source_gate === 'no_pending_ai_tasks' &&
        action.review_order === 1 &&
        action.blocking === true &&
        action.recommended_tool === 'context.get_governance',
    ),
    'blocked release readiness should expose actionable pending AI task remediation',
  );

  console.log('Phase 5 release readiness live API+MCP smoke passed');
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE
