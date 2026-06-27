#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
POSTGRES_PORT="${OPENPR_SMOKE_PG_PORT:-5366}"
PG_SUPERUSER="${OPENPR_SMOKE_PG_SUPERUSER:-postgres}"
DB_NAME="openpr_phase1_smoke_$$_$(date +%s)"
DB_USER="openpr_phase1_smoke_$$_$(date +%s)"
DB_PASSWORD="$(openssl rand -hex 12)"
API_PORT="${OPENPR_SMOKE_API_PORT:-$((17180 + ($$ % 1000)))}"
TMP_DIR="$(mktemp -d /tmp/openpr-phase1-smoke.XXXXXX)"
API_LOG="$TMP_DIR/api.log"
JWT_SECRET="openpr-phase1-smoke-secret"

OWNER_ID="11111111-1111-4111-8111-111111111111"
MEMBER_ID="22222222-2222-4222-8222-222222222222"
WORKSPACE_ID="33333333-3333-4333-8333-333333333333"
OTHER_WORKSPACE_ID="44444444-4444-4444-8444-444444444444"
BOT_A_ID="55555555-5555-4555-8555-555555555555"
BOT_B_ID="66666666-6666-4666-8666-666666666666"
BOT_A_TOKEN="opr_phase1_workspace_a_$$_$(date +%s)"
BOT_B_TOKEN="opr_phase1_workspace_b_$$_$(date +%s)"

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

DATABASE_URL="postgres://$DB_USER:$DB_PASSWORD@127.0.0.1:$POSTGRES_PORT/$DB_NAME"

BIND_ADDR="127.0.0.1:$API_PORT" \
DATABASE_URL="$DATABASE_URL" \
JWT_SECRET="$JWT_SECRET" \
RUST_LOG="${RUST_LOG:-api=info,openpr=info}" \
"$ROOT_DIR/target/debug/api" >"$API_LOG" 2>&1 &
api_pid=$!
wait_http "http://127.0.0.1:$API_PORT/health" "OpenPR API"

BOT_A_HASH="$(sha256_hex "$BOT_A_TOKEN")"
BOT_B_HASH="$(sha256_hex "$BOT_B_TOKEN")"

psql_smoke -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES
  ('$OWNER_ID', 'owner-phase1@example.local', '', 'Phase 1 Owner', 'admin', true, 'human', NULL, now(), now()),
  ('$MEMBER_ID', 'member-phase1@example.local', '', 'Phase 1 Member', 'user', true, 'human', NULL, now(), now());

INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES
  ('$WORKSPACE_ID', 'phase1-smoke', 'Phase 1 Smoke', '$OWNER_ID', now(), now()),
  ('$OTHER_WORKSPACE_ID', 'phase1-other-smoke', 'Phase 1 Other Smoke', '$OWNER_ID', now(), now());

INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES
  ('$WORKSPACE_ID', '$OWNER_ID', 'owner', now()),
  ('$WORKSPACE_ID', '$MEMBER_ID', 'member', now()),
  ('$OTHER_WORKSPACE_ID', '$OWNER_ID', 'owner', now());

INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by, is_active, created_at, updated_at)
VALUES
  ('$BOT_A_ID', '$WORKSPACE_ID', 'Phase 1 Bot A', '$BOT_A_HASH', substring('$BOT_A_TOKEN' from 1 for 8), '["read","write","admin"]'::jsonb, '$OWNER_ID', true, now(), now()),
  ('$BOT_B_ID', '$OTHER_WORKSPACE_ID', 'Phase 1 Bot B', '$BOT_B_HASH', substring('$BOT_B_TOKEN' from 1 for 8), '["read"]'::jsonb, '$OWNER_ID', true, now(), now());
SQL

API_URL="http://127.0.0.1:$API_PORT" \
JWT_SECRET="$JWT_SECRET" \
OWNER_ID="$OWNER_ID" \
MEMBER_ID="$MEMBER_ID" \
WORKSPACE_ID="$WORKSPACE_ID" \
OTHER_WORKSPACE_ID="$OTHER_WORKSPACE_ID" \
BOT_A_TOKEN="$BOT_A_TOKEN" \
BOT_B_TOKEN="$BOT_B_TOKEN" \
MCP_BIN="$ROOT_DIR/target/debug/mcp-server" \
node --input-type=commonjs <<'NODE'
const crypto = require('crypto');
const { spawnSync } = require('child_process');

const apiUrl = process.env.API_URL;
const workspaceId = process.env.WORKSPACE_ID;
const otherWorkspaceId = process.env.OTHER_WORKSPACE_ID;
const jwtSecret = process.env.JWT_SECRET;
const mcpBin = process.env.MCP_BIN;

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

const ownerToken = jwt(process.env.OWNER_ID, 'owner-phase1@example.local');
const memberToken = jwt(process.env.MEMBER_ID, 'member-phase1@example.local');
const botAToken = process.env.BOT_A_TOKEN;
const botBToken = process.env.BOT_B_TOKEN;

async function request(token, method, path, body, expectedStatus = 200) {
  const response = await fetch(`${apiUrl}${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${token}`,
      ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  let payload = null;
  try {
    payload = text ? JSON.parse(text) : null;
  } catch {
    payload = text;
  }
  if (response.status !== expectedStatus) {
    throw new Error(`${method} ${path} expected HTTP ${expectedStatus}, got ${response.status}: ${text}`);
  }
  return payload;
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function assertApiCode(payload, code, message) {
  assert(payload?.code === code, `${message}: expected API code ${code}, got ${JSON.stringify(payload)}`);
}

function mcpCall(token, workspace, method, params) {
  const requestPayload = JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }) + '\n';
  const result = spawnSync(mcpBin, ['--api-url', apiUrl, '--bot-token', token, '--workspace-id', workspace, 'serve', '--transport', 'stdio'], {
    input: requestPayload,
    encoding: 'utf8',
    env: { ...process.env, RUST_LOG: 'error' },
    timeout: 20000,
  });
  if (result.status !== 0) {
    throw new Error(`MCP ${method} exited ${result.status}: ${result.stderr}`);
  }
  const line = result.stdout.split('\n').find((item) => item.trim().startsWith('{'));
  if (!line) {
    throw new Error(`MCP ${method} produced no JSON: ${result.stdout} ${result.stderr}`);
  }
  return JSON.parse(line);
}

async function main() {
const createdType = await request(ownerToken, 'POST', `/api/v1/workspaces/${workspaceId}/project-types`, {
  key: 'field_audit',
  name: 'Field Audit',
  description: 'On-site audit workflow',
  domain: 'operations',
});
assert(createdType.data?.key === 'field_audit', 'owner should create workspace project type');

const updatedType = await request(ownerToken, 'PATCH', '/api/v1/project-types/field_audit', {
  name: 'Field Audit Updated',
  domain: 'audit',
});
assert(updatedType.data?.name === 'Field Audit Updated', 'owner should update workspace project type');

assertApiCode(
  await request(ownerToken, 'PATCH', '/api/v1/project-types/code_project', { name: 'Should Not Update' }),
  403,
  'system project type update should be rejected'
);

const typeList = await request(memberToken, 'GET', `/api/v1/workspaces/${workspaceId}/project-types`);
assert(typeList.data?.items?.some((item) => item.key === 'field_audit'), 'member should read workspace project types');

const codeProject = await request(ownerToken, 'POST', `/api/v1/workspaces/${workspaceId}/projects`, {
  name: 'Phase 1 Code Project',
  key: 'P1CODE',
  description: 'Code project smoke',
  type_key: 'code_project',
});
assert(codeProject.data?.type_key === 'code_project', 'code project should preserve type_key');
await request(ownerToken, 'DELETE', `/api/v1/projects/${codeProject.data.id}`);

const contractProject = await request(ownerToken, 'POST', `/api/v1/workspaces/${workspaceId}/projects`, {
  name: 'Phase 1 Contract Project',
  key: 'P1LEGAL',
  description: 'Contract project smoke',
  type_key: 'contract_review',
});
const projectId = contractProject.data?.id;
assert(projectId && contractProject.data?.type_key === 'contract_review', 'contract project should be scenario typed');
const updatedProject = await request(ownerToken, 'PUT', `/api/v1/projects/${projectId}`, {
  name: 'Phase 1 Contract Project Updated',
  description: 'Contract project smoke updated',
});
assert(updatedProject.data?.name === 'Phase 1 Contract Project Updated', 'project should update');

const repo = await request(ownerToken, 'POST', `/api/v1/projects/${projectId}/resources`, {
  kind: 'repo',
  name: 'Product Repository',
  locator: { url: 'https://git.example/product' },
});
const repoId = repo.data?.id;
assert(repoId, 'repo resource should be created');

const equipment = await request(ownerToken, 'POST', `/api/v1/projects/${projectId}/resources`, {
  kind: 'equipment',
  name: 'Pump A17',
  locator: { asset_id: 'PUMP-A17' },
});
assert(equipment.data?.kind === 'equipment', 'equipment resource should be created');

await request(memberToken, 'GET', `/api/v1/projects/${projectId}/resources`);
assertApiCode(await request(memberToken, 'POST', `/api/v1/projects/${projectId}/resources`, {
  kind: 'custom',
  name: 'Forbidden member write',
}), 403, 'member resource write should be rejected');

const updatedRepo = await request(ownerToken, 'PATCH', `/api/v1/projects/${projectId}/resources/${repoId}`, {
  name: 'Product Repository Updated',
  sync_status: 'synced',
});
assert(updatedRepo.data?.name === 'Product Repository Updated', 'resource should update');

await request(ownerToken, 'DELETE', `/api/v1/projects/${projectId}/resources/${equipment.data.id}`);
const afterDelete = await request(ownerToken, 'GET', `/api/v1/projects/${projectId}/resources`);
assert(!afterDelete.data.items.some((item) => item.id === equipment.data.id), 'deleted resource should be absent');

const botRead = await request(botAToken, 'GET', `/api/v1/projects/${projectId}/resources`);
assert(botRead.data.items.some((item) => item.id === repoId), 'workspace bot should read project resources');
assertApiCode(
  await request(botBToken, 'GET', `/api/v1/projects/${projectId}/resources`),
  403,
  'cross-workspace bot resource read should be rejected'
);

const mcpTools = mcpCall(botAToken, workspaceId, 'tools/list', {});
const toolNames = (mcpTools.result?.tools ?? []).map((tool) => tool.name);
for (const name of ['project_types.list', 'project_types.get', 'project_resources.list', 'project_resources.create', 'project_resources.update', 'project_resources.delete']) {
  assert(toolNames.includes(name), `MCP tools/list missing ${name}`);
}

const mcpType = mcpCall(botAToken, workspaceId, 'resources/read', { uri: `openpr://projects/${projectId}/type` });
assert(JSON.stringify(mcpType).includes('contract_review'), 'MCP type resource should include project type');

const mcpResources = mcpCall(botAToken, workspaceId, 'resources/read', { uri: `openpr://projects/${projectId}/resources` });
assert(JSON.stringify(mcpResources).includes('Product Repository Updated'), 'MCP resources read should include updated resource');

const mcpDenied = mcpCall(botBToken, otherWorkspaceId, 'resources/read', { uri: `openpr://projects/${projectId}/resources` });
assert(JSON.stringify(mcpDenied).includes('bot not authorized') || JSON.stringify(mcpDenied).includes('Forbidden'), 'MCP should deny cross-workspace bot resource read');

console.log('Phase 1 project types/resources live API+MCP smoke passed');
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

PROJECT_TYPE_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type IN ('project_type.created', 'project_type.updated') AND aggregate_type = 'project_type' AND aggregate_id = 'field_audit'")"
if [[ "$PROJECT_TYPE_EVENT_COUNT" -lt 2 ]]; then
  echo "Expected at least 2 project type business events, got $PROJECT_TYPE_EVENT_COUNT" >&2
  exit 1
fi

PROJECT_RESOURCE_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type IN ('project_resource.created', 'project_resource.updated', 'project_resource.deleted') AND aggregate_type = 'project_resource'")"
if [[ "$PROJECT_RESOURCE_EVENT_COUNT" -lt 4 ]]; then
  echo "Expected at least 4 project resource business events, got $PROJECT_RESOURCE_EVENT_COUNT" >&2
  exit 1
fi

PROJECT_TYPE_RESOURCE_OUTBOX_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM event_outbox WHERE event_type IN ('project_type.created', 'project_type.updated', 'project_resource.created', 'project_resource.updated', 'project_resource.deleted')")"
if [[ "$PROJECT_TYPE_RESOURCE_OUTBOX_COUNT" -lt 6 ]]; then
  echo "Expected at least 6 project type/resource outbox rows, got $PROJECT_TYPE_RESOURCE_OUTBOX_COUNT" >&2
  exit 1
fi

PROJECT_LIFECYCLE_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type IN ('project.created', 'project.updated', 'project.deleted') AND aggregate_type = 'project'")"
if [[ "$PROJECT_LIFECYCLE_EVENT_COUNT" -lt 3 ]]; then
  echo "Expected at least 3 project lifecycle business events, got $PROJECT_LIFECYCLE_EVENT_COUNT" >&2
  exit 1
fi

PROJECT_LIFECYCLE_OUTBOX_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM event_outbox WHERE event_type IN ('project.created', 'project.updated', 'project.deleted')")"
if [[ "$PROJECT_LIFECYCLE_OUTBOX_COUNT" -lt 3 ]]; then
  echo "Expected at least 3 project lifecycle outbox rows, got $PROJECT_LIFECYCLE_OUTBOX_COUNT" >&2
  exit 1
fi

echo "Project type/resource business events and outbox smoke passed"
echo "Project lifecycle business events and outbox smoke passed"
