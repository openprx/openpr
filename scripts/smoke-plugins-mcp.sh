#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
POSTGRES_PORT="${OPENPR_SMOKE_PG_PORT:-5366}"
PG_SUPERUSER="${OPENPR_SMOKE_PG_SUPERUSER:-postgres}"
DB_NAME="openpr_plugins_mcp_smoke_$$_$(date +%s)"
DB_USER="openpr_plugins_mcp_smoke_$$_$(date +%s)"
DB_PASSWORD="$(openssl rand -hex 12)"
API_PORT="${OPENPR_SMOKE_API_PORT:-$((22180 + ($$ % 1000)))}"
TMP_DIR="$(mktemp -d /tmp/openpr-plugins-mcp-smoke.XXXXXX)"
API_LOG="$TMP_DIR/api.log"
JWT_SECRET="openpr-plugins-mcp-smoke-secret"

OWNER_ID="11111111-1111-4111-8111-111111111111"
WORKSPACE_ID="33333333-3333-4333-8333-333333333333"
BOT_ID="55555555-5555-4555-8555-555555555555"
BOT_TOKEN="opr_plugins_mcp_$$_$(date +%s)"

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

BOT_HASH="$(sha256_hex "$BOT_TOKEN")"

psql_smoke -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES ('$OWNER_ID', 'plugins-mcp-owner@example.local', '', 'Plugins MCP Owner', 'admin', true, 'human', NULL, now(), now());

INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE_ID', 'plugins-mcp-smoke', 'Plugins MCP Smoke', '$OWNER_ID', now(), now());

INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES ('$WORKSPACE_ID', '$OWNER_ID', 'owner', now());

INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by, is_active, created_at, updated_at)
VALUES (
  '$BOT_ID', '$WORKSPACE_ID', 'Plugins MCP Bot', '$BOT_HASH',
  substring('$BOT_TOKEN' from 1 for 8), '["read","write","admin"]'::jsonb,
  '$OWNER_ID', true, now(), now()
);
SQL

API_URL="http://127.0.0.1:$API_PORT" \
JWT_SECRET="$JWT_SECRET" \
OWNER_ID="$OWNER_ID" \
WORKSPACE_ID="$WORKSPACE_ID" \
BOT_TOKEN="$BOT_TOKEN" \
MCP_BIN="$ROOT_DIR/target/debug/mcp-server" \
node --input-type=commonjs <<'NODE'
const crypto = require('crypto');
const { spawnSync } = require('child_process');

const apiUrl = process.env.API_URL;
const workspaceId = process.env.WORKSPACE_ID;
const jwtSecret = process.env.JWT_SECRET;
const mcpBin = process.env.MCP_BIN;
const botToken = process.env.BOT_TOKEN;

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

const ownerToken = jwt(process.env.OWNER_ID, 'plugins-mcp-owner@example.local');

async function request(method, path, body, expectedStatus = 200) {
  const response = await fetch(`${apiUrl}${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${ownerToken}`,
      ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  const payload = text ? JSON.parse(text) : null;
  if (response.status !== expectedStatus) {
    throw new Error(`${method} ${path} expected HTTP ${expectedStatus}, got ${response.status}: ${text}`);
  }
  return payload;
}

async function apiOk(method, path, body) {
  const payload = await request(method, path, body);
  if (payload.code !== 0) {
    throw new Error(`${method} ${path} expected API code 0, got ${payload.code}: ${payload.message}`);
  }
  return payload;
}

async function apiError(method, path, body, expectedCode) {
  const payload = await request(method, path, body);
  if (payload.code !== expectedCode) {
    throw new Error(`${method} ${path} expected API code ${expectedCode}, got ${payload.code}: ${payload.message}`);
  }
  return payload;
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function bytesOf(value) {
  return [...Buffer.from(value, 'utf8')];
}

function u32(value) {
  const bytes = [];
  let current = value >>> 0;
  do {
    let byte = current & 0x7f;
    current >>>= 7;
    if (current !== 0) byte |= 0x80;
    bytes.push(byte);
  } while (current !== 0);
  return bytes;
}

function i64(value) {
  const bytes = [];
  let current = BigInt(value);
  let more = true;
  while (more) {
    let byte = Number(current & 0x7fn);
    current >>= 7n;
    const signBit = (byte & 0x40) !== 0;
    more = !((current === 0n && !signBit) || (current === -1n && signBit));
    if (more) byte |= 0x80;
    bytes.push(byte);
  }
  return bytes;
}

function vec(items) {
  return [...u32(items.length), ...items.flat()];
}

function section(id, content) {
  return [id, ...u32(content.length), ...content];
}

function str(value) {
  const bytes = bytesOf(value);
  return [...u32(bytes.length), ...bytes];
}

function body(instructions) {
  const bodyBytes = [0x00, ...instructions, 0x0b];
  return [...u32(bodyBytes.length), ...bodyBytes];
}

function validatorWasmBase64(outputJson) {
  const output = bytesOf(outputJson);
  const outputPtr = 1024;
  const inputPtr = 2048;
  const handle = (BigInt(outputPtr) << 32n) | BigInt(output.length);
  const typeSection = vec([
    [0x60, ...vec([[0x7f]]), ...vec([[0x7f]])],
    [0x60, ...vec([[0x7f], [0x7f]]), ...vec([[0x7e]])],
    [0x60, ...vec([]), ...vec([[0x7f]])],
  ]);
  const functionSection = vec([[0x02], [0x00], [0x01]]);
  const memorySection = vec([[0x00, 0x01]]);
  const exportSection = vec([
    [...str('memory'), 0x02, 0x00],
    [...str('openpr_plugin_abi_version'), 0x00, 0x00],
    [...str('openpr_alloc'), 0x00, 0x01],
    [...str('openpr_invoke'), 0x00, 0x02],
  ]);
  const codeSection = vec([
    body([0x41, 0x01]),
    body([0x41, ...u32(inputPtr)]),
    body([0x42, ...i64(handle)]),
  ]);
  const dataSection = vec([
    [0x00, 0x41, ...u32(outputPtr), 0x0b, ...u32(output.length), ...output],
  ]);
  const moduleBytes = [
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00,
    ...section(1, typeSection),
    ...section(3, functionSection),
    ...section(5, memorySection),
    ...section(7, exportSection),
    ...section(10, codeSection),
    ...section(11, dataSection),
  ];
  return Buffer.from(moduleBytes).toString('base64');
}

function mcp(method, params) {
  const requestPayload = JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }) + '\n';
  const result = spawnSync(
    mcpBin,
    ['--api-url', apiUrl, '--bot-token', botToken, '--workspace-id', workspaceId, 'serve', '--transport', 'stdio'],
    {
      input: requestPayload,
      encoding: 'utf8',
      env: { ...process.env, RUST_LOG: 'error' },
      timeout: 20000,
    },
  );
  if (result.status !== 0) {
    throw new Error(`MCP ${method} exited ${result.status}: ${result.stderr}`);
  }
  const line = result.stdout.split('\n').find((item) => item.trim().startsWith('{'));
  if (!line) {
    throw new Error(`MCP ${method} produced no JSON: ${result.stdout} ${result.stderr}`);
  }
  return JSON.parse(line);
}

function callTool(name, args) {
  const response = mcp('tools/call', { name, arguments: args });
  const text = response.result?.content?.[0]?.text ?? '';
  const payload = JSON.parse(text);
  assert(response.result?.is_error !== true, `${name} failed: ${text}`);
  return payload;
}

async function main() {
  const project = await apiOk('POST', `/api/v1/workspaces/${workspaceId}/projects`, {
    name: 'Plugins MCP Project',
    key: 'PMCP',
    type_key: 'custom_form',
  });
  const projectId = project.data.id;

  const tools = mcp('tools/list', {});
  const toolNames = (tools.result?.tools ?? []).map((tool) => tool.name);
  for (const name of ['plugins.list', 'plugins.get', 'plugins.install', 'plugins.invoke', 'plugin_invocations.list']) {
    assert(toolNames.includes(name), `tools/list missing ${name}`);
  }

  const manifest = {
    schema_version: 'openpr.plugin.v1',
    key: 'contract_risk',
    name: 'Contract Risk',
    version: '0.1.0',
    description: 'Manifest-only smoke plugin',
    capabilities: {
      hooks: [
        { kind: 'field_validator', form_key: 'contract', field_key: 'amount' },
        { kind: 'event_handler', event_type: 'form.record.created' },
      ],
      tools: [{ name: 'contract_risk.score', description: 'Score contract risk' }],
      runtime: { timeout_ms: 500, fuel: 100000, memory_bytes: 1048576 },
    },
  };

  const installed = callTool('plugins.install', { project_id: projectId, manifest });
  const pluginId = installed.data.id;
  assert(installed.data.key === 'contract_risk', 'installed plugin key mismatch');
  assert(installed.data.manifest.capabilities.tools[0].name === 'contract_risk.score', 'plugin tool manifest missing');

  const listed = callTool('plugins.list', { project_id: projectId });
  assert(listed.data.items.some((item) => item.id === pluginId), 'plugins.list did not include installed plugin');

  const got = callTool('plugins.get', { plugin_id: pluginId });
  assert(got.data.manifest.capabilities.hooks.length === 2, 'plugins.get did not return hooks');

  const disabled = await apiOk('PATCH', `/api/v1/plugins/${pluginId}`, { status: 'disabled' });
  assert(disabled.data.status === 'disabled', 'plugin status should update to disabled');
  const reactivated = await apiOk('PATCH', `/api/v1/plugins/${pluginId}`, { status: 'active' });
  assert(reactivated.data.status === 'active', 'plugin status should update to active');

  const invocation = callTool('plugins.invoke', {
    plugin_id: pluginId,
    hook_kind: 'field_validator',
    input: { value: '100.00' },
  });
  assert(invocation.data.status === 'failed', 'manifest-only invocation should be logged as failed');
  assert(invocation.data.error_message.includes('no wasm'), 'missing wasm error not logged');

  const invocations = callTool('plugin_invocations.list', { plugin_id: pluginId });
  assert(invocations.data.items.length === 1, 'plugin invocation log missing');

  const riskTool = callTool('plugins.install', {
    project_id: projectId,
    manifest: {
      schema_version: 'openpr.plugin.v1',
      key: 'risk_tool',
      name: 'Risk Tool',
      version: '0.1.0',
      capabilities: {
        tools: [{ name: 'risk_tool.score', description: 'Score contract risk' }],
        runtime: { timeout_ms: 500, fuel: 100000, memory_bytes: 1048576 },
      },
    },
    wasm_base64: validatorWasmBase64('{"score":7,"risk":"medium"}'),
  });
  const riskResult = callTool('plugins.invoke', {
    plugin_id: riskTool.data.id,
    hook_kind: 'risk_tool.score',
    input: { contract_amount: '100.00' },
  });
  assert(riskResult.data.status === 'completed', 'plugin-provided MCP tool should complete');
  assert(riskResult.data.output.score === 7, 'plugin-provided MCP tool output mismatch');

  const form = await apiOk('POST', `/api/v1/projects/${projectId}/forms`, {
    key: 'contract',
    name: 'Contract',
    schema: {
      fields: [
        { key: 'amount', label: 'Amount', type: 'amount', amount: { currency: 'CNY', scale: 2 } },
      ],
    },
  });
  const formId = form.data.id;

  const allowPlugin = await apiOk('POST', `/api/v1/projects/${projectId}/plugins`, {
    manifest: {
      schema_version: 'openpr.plugin.v1',
      key: 'allow_validator',
      name: 'Allow Validator',
      version: '0.1.0',
      capabilities: {
        hooks: [{ kind: 'field_validator', form_key: 'contract', field_key: 'amount' }],
        runtime: { timeout_ms: 500, fuel: 100000, memory_bytes: 1048576 },
      },
    },
    wasm_base64: validatorWasmBase64('{"ok":true}'),
  });

  const accepted = await apiOk('POST', `/api/v1/forms/${formId}/records`, {
    values: { amount: '100.00' },
    title: 'Accepted contract',
  });
  assert(accepted.data.values.amount.decimal === '100.00', 'validator allow path did not create record');

  const allowInvocations = await apiOk('GET', `/api/v1/plugins/${allowPlugin.data.id}/invocations`);
  assert(allowInvocations.data.items.length === 1, 'allow validator invocation missing');
  assert(allowInvocations.data.items[0].status === 'completed', 'allow validator should complete');

  const rejectPlugin = await apiOk('POST', `/api/v1/projects/${projectId}/plugins`, {
    manifest: {
      schema_version: 'openpr.plugin.v1',
      key: 'reject_validator',
      name: 'Reject Validator',
      version: '0.1.0',
      capabilities: {
        hooks: [{ kind: 'field_validator', form_key: 'contract', field_key: 'amount' }],
        runtime: { timeout_ms: 500, fuel: 100000, memory_bytes: 1048576 },
      },
    },
    wasm_base64: validatorWasmBase64('{"ok":false,"error":"amount too high"}'),
  });

  const rejected = await apiError('POST', `/api/v1/forms/${formId}/records`, {
    values: { amount: '200.00' },
    title: 'Rejected contract',
  }, 400);
  assert(rejected.message.includes('amount too high'), 'validator rejection message missing');

  const rejectInvocations = await apiOk('GET', `/api/v1/plugins/${rejectPlugin.data.id}/invocations`);
  assert(rejectInvocations.data.items.length === 1, 'reject validator invocation missing');
  assert(rejectInvocations.data.items[0].status === 'failed', 'reject validator should fail');
  assert(rejectInvocations.data.items[0].error_message === 'amount too high', 'reject validator error not logged');

  const invoiceForm = await apiOk('POST', `/api/v1/projects/${projectId}/forms`, {
    key: 'invoice',
    name: 'Invoice',
    schema: {
      fields: [
        { key: 'subtotal', label: 'Subtotal', type: 'amount', amount: { currency: 'CNY', scale: 2 } },
        { key: 'tax', label: 'Tax', type: 'amount', amount: { currency: 'CNY', scale: 2 } },
        { key: 'total', label: 'Total', type: 'amount', amount: { currency: 'CNY', scale: 2 } },
      ],
    },
  });
  const formulaPlugin = await apiOk('POST', `/api/v1/projects/${projectId}/plugins`, {
    manifest: {
      schema_version: 'openpr.plugin.v1',
      key: 'invoice_formula',
      name: 'Invoice Formula',
      version: '0.1.0',
      capabilities: {
        hooks: [{ kind: 'formula', form_key: 'invoice', field_key: 'total' }],
        runtime: { timeout_ms: 500, fuel: 100000, memory_bytes: 1048576 },
      },
    },
    wasm_base64: validatorWasmBase64('{"patch":{"total":"0.30"}}'),
  });
  const eventPlugin = await apiOk('POST', `/api/v1/projects/${projectId}/plugins`, {
    manifest: {
      schema_version: 'openpr.plugin.v1',
      key: 'invoice_event',
      name: 'Invoice Event Handler',
      version: '0.1.0',
      capabilities: {
        hooks: [{ kind: 'event_handler', form_key: 'invoice', event_type: 'form.record.created' }],
        runtime: { timeout_ms: 500, fuel: 100000, memory_bytes: 1048576 },
      },
    },
    wasm_base64: validatorWasmBase64('{"ok":true,"handled":true}'),
  });
  const invoice = await apiOk('POST', `/api/v1/forms/${invoiceForm.data.id}/records`, {
    values: { subtotal: '0.10', tax: '0.20' },
    title: 'Formula invoice',
  });
  assert(invoice.data.values.total.decimal === '0.30', 'formula hook did not write decimal total');
  const formulaInvocations = await apiOk('GET', `/api/v1/plugins/${formulaPlugin.data.id}/invocations`);
  assert(formulaInvocations.data.items.length === 1, 'formula invocation missing');
  assert(formulaInvocations.data.items[0].status === 'completed', 'formula invocation should complete');
  const eventInvocations = await apiOk('GET', `/api/v1/plugins/${eventPlugin.data.id}/invocations`);
  assert(eventInvocations.data.items.length === 1, 'event handler invocation missing');
  assert(eventInvocations.data.items[0].status === 'completed', 'event handler invocation should complete');
  assert(
    eventInvocations.data.items[0].input.payload.event_type === 'form.record.created',
    'event handler did not receive event payload',
  );

  console.log('Plugins MCP smoke passed');
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

PLUGIN_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type IN ('plugin.installed', 'plugin.updated', 'plugin.invoked')")"
if [[ "$PLUGIN_EVENT_COUNT" -lt 14 ]]; then
  echo "Expected at least 14 plugin business events, got $PLUGIN_EVENT_COUNT" >&2
  exit 1
fi

PLUGIN_OUTBOX_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM event_outbox WHERE event_type IN ('plugin.installed', 'plugin.updated', 'plugin.invoked')")"
if [[ "$PLUGIN_OUTBOX_COUNT" -lt 14 ]]; then
  echo "Expected at least 14 plugin event_outbox rows, got $PLUGIN_OUTBOX_COUNT" >&2
  exit 1
fi

AUTOMATIC_HOOK_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type = 'plugin.invoked' AND metadata->>'automatic_hook' = 'true' AND metadata->>'hook_kind' IN ('field_validator', 'formula', 'event_handler')")"
if [[ "$AUTOMATIC_HOOK_EVENT_COUNT" -lt 4 ]]; then
  echo "Expected at least 4 automatic hook plugin.invoked events, got $AUTOMATIC_HOOK_EVENT_COUNT" >&2
  exit 1
fi

AUTOMATIC_HOOK_OUTBOX_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM event_outbox WHERE event_type = 'plugin.invoked' AND payload->'metadata'->>'automatic_hook' = 'true' AND payload->'metadata'->>'hook_kind' IN ('field_validator', 'formula', 'event_handler')")"
if [[ "$AUTOMATIC_HOOK_OUTBOX_COUNT" -lt 4 ]]; then
  echo "Expected at least 4 automatic hook plugin.invoked outbox rows, got $AUTOMATIC_HOOK_OUTBOX_COUNT" >&2
  exit 1
fi

echo "Plugin business events and outbox smoke passed"
