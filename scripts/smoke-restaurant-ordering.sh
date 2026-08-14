#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
POSTGRES_PORT="${OPENPR_SMOKE_PG_PORT:-5366}"
PG_SUPERUSER="${OPENPR_SMOKE_PG_SUPERUSER:-postgres}"
DB_NAME="openpr_restaurant_smoke_$$_$(date +%s)"
DB_USER="openpr_restaurant_smoke_$$_$(date +%s)"
DB_PASSWORD="$(openssl rand -hex 12)"
API_PORT="${OPENPR_SMOKE_API_PORT:-$((23180 + ($$ % 1000)))}"
RECEIVER_PORT="${OPENPR_SMOKE_RECEIVER_PORT:-$((24180 + ($$ % 1000)))}"
TMP_DIR="$(mktemp -d /tmp/openpr-restaurant-smoke.XXXXXX)"
API_LOG="$TMP_DIR/api.log"
WORKER_LOG="$TMP_DIR/worker.log"
RECEIVER_LOG="$TMP_DIR/receiver.ndjson"
SMOKE_JWT_SECRET="openpr-restaurant-smoke-secret"

OWNER_ID="11111111-1111-4111-8111-111111111111"
WORKSPACE_ID="33333333-3333-4333-8333-333333333333"
BOT_ID="55555555-5555-4555-8555-555555555555"
BOT_TOKEN="opr_restaurant_mcp_$$_$(date +%s)"

api_pid=""
worker_pid=""
receiver_pid=""

cleanup() {
  local exit_code=$?
  if [[ -n "$worker_pid" ]] && kill -0 "$worker_pid" 2>/dev/null; then
    kill "$worker_pid" 2>/dev/null || true
    wait "$worker_pid" 2>/dev/null || true
  fi
  if [[ -n "$api_pid" ]] && kill -0 "$api_pid" 2>/dev/null; then
    kill "$api_pid" 2>/dev/null || true
    wait "$api_pid" 2>/dev/null || true
  fi
  if [[ -n "$receiver_pid" ]] && kill -0 "$receiver_pid" 2>/dev/null; then
    kill "$receiver_pid" 2>/dev/null || true
    wait "$receiver_pid" 2>/dev/null || true
  fi
  sudo -n -u "$PG_SUPERUSER" psql -p "$POSTGRES_PORT" -d postgres -v ON_ERROR_STOP=1 -q <<SQL >/dev/null 2>&1 || true
SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '$DB_NAME';
DROP DATABASE IF EXISTS "$DB_NAME";
DROP ROLE IF EXISTS "$DB_USER";
SQL
  if [[ $exit_code -ne 0 ]]; then
    echo "Smoke failed. API log: $API_LOG" >&2
    echo "Worker log: $WORKER_LOG" >&2
    echo "Receiver log: $RECEIVER_LOG" >&2
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
# app_name is deliberately unset: this file also serves the worker, and a shared name would make
# the worker log itself as the api. bind_addr is simply unread there.
bind_addr = "127.0.0.1:$API_PORT"

[database]
url = "$SMOKE_DATABASE_URL"

[auth]
jwt_secret = "$SMOKE_JWT_SECRET"

[logging]
filter = "${OPENPR_SMOKE_LOG_FILTER:-api=info,worker=info,openpr=info}"
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

: >"$RECEIVER_LOG"
RECEIVER_LOG="$RECEIVER_LOG" \
RECEIVER_PORT="$RECEIVER_PORT" \
node --input-type=commonjs <<'NODE' &
const fs = require('fs');
const http = require('http');

const logPath = process.env.RECEIVER_LOG;
const port = Number(process.env.RECEIVER_PORT);

const server = http.createServer((req, res) => {
  if (req.method === 'GET') {
    res.writeHead(200, { 'Content-Type': 'text/plain' });
    res.end('ok');
    return;
  }

  let body = '';
  req.on('data', (chunk) => {
    body += chunk;
  });
  req.on('end', () => {
    fs.appendFileSync(logPath, `${body}\n`);
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ ok: true }));
  });
});

server.listen(port, '127.0.0.1');
NODE
receiver_pid=$!
wait_http "http://127.0.0.1:$RECEIVER_PORT/health" "connector receiver"

"$ROOT_DIR/target/debug/worker" --config "$APP_CONFIG" --concurrency 2 >"$WORKER_LOG" 2>&1 &
worker_pid=$!

BOT_HASH="$(sha256_hex "$BOT_TOKEN")"

psql_smoke -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES ('$OWNER_ID', 'restaurant-owner@example.local', '', 'Restaurant Owner', 'admin', true, 'human', NULL, now(), now());

INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE_ID', 'restaurant-smoke', 'Restaurant Smoke', '$OWNER_ID', now(), now());

INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES ('$WORKSPACE_ID', '$OWNER_ID', 'owner', now());

INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by, is_active, created_at, updated_at)
VALUES (
  '$BOT_ID', '$WORKSPACE_ID', 'Restaurant MCP Bot', '$BOT_HASH',
  substring('$BOT_TOKEN' from 1 for 8), '["read","write","admin"]'::jsonb,
  '$OWNER_ID', true, now(), now()
);
SQL

API_URL="http://127.0.0.1:$API_PORT" \
RECEIVER_URL="http://127.0.0.1:$RECEIVER_PORT/events" \
SMOKE_JWT_SECRET="$SMOKE_JWT_SECRET" \
OWNER_ID="$OWNER_ID" \
WORKSPACE_ID="$WORKSPACE_ID" \
BOT_TOKEN="$BOT_TOKEN" \
MCP_BIN="$ROOT_DIR/target/debug/mcp-server" \
MCP_CONFIG="$MCP_CONFIG" \
node --input-type=commonjs <<'NODE'
const crypto = require('crypto');
const { spawnSync } = require('child_process');

const apiUrl = process.env.API_URL;
const workspaceId = process.env.WORKSPACE_ID;
const jwtSecret = process.env.SMOKE_JWT_SECRET;
const receiverUrl = process.env.RECEIVER_URL;
const mcpBin = process.env.MCP_BIN;
const mcpConfig = process.env.MCP_CONFIG;
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

const ownerToken = jwt(process.env.OWNER_ID, 'restaurant-owner@example.local');

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

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function formByKey(forms, key) {
  const form = forms.find((item) => item.key === key);
  assert(form, `missing form ${key}`);
  return form;
}

function bytesOf(value) {
  return Array.from(Buffer.from(value));
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
  let current = BigInt.asIntN(64, BigInt(value));
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

function constantWasmBase64(outputJson) {
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
  return Buffer.from([
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00,
    ...section(1, typeSection),
    ...section(3, functionSection),
    ...section(5, memorySection),
    ...section(7, exportSection),
    ...section(10, codeSection),
    ...section(11, dataSection),
  ]).toString('base64');
}

function mcp(method, params) {
  const requestPayload = JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }) + '\n';
  const result = spawnSync(
    mcpBin,
    ['--config', mcpConfig, '--api-url', apiUrl, '--bot-token', botToken, '--workspace-id', workspaceId, 'serve', '--transport', 'stdio'],
    {
      input: requestPayload,
      encoding: 'utf8',
      env: process.env,
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
  return JSON.parse(text);
}

async function waitForInvocations(projectId, predicate, label) {
  for (let i = 0; i < 90; i += 1) {
    const invocations = await request('GET', `/api/v1/projects/${projectId}/invocations?per_page=100`);
    const items = invocations.data.items ?? [];
    const matched = items.filter(predicate);
    if (matched.length > 0) return matched;
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  throw new Error(`timed out waiting for ${label}`);
}

async function main() {
  const project = await request('POST', `/api/v1/workspaces/${workspaceId}/projects`, {
    name: 'Restaurant Ordering E2E',
    key: 'RESTE2E',
    scenario_template_key: 'restaurant_ordering_default',
  });
  const projectId = project.data.id;

  const forms = (await request('GET', `/api/v1/projects/${projectId}/forms?per_page=100`)).data.items;
  const menuCategoryForm = formByKey(forms, 'menu_category');
  const skuForm = formByKey(forms, 'sku');
  const tableForm = formByKey(forms, 'table');
  const orderForm = formByKey(forms, 'order');
  const orderLineForm = formByKey(forms, 'order_line');
  const printJobForm = formByKey(forms, 'print_job');
  const reportForm = formByKey(forms, 'business_report');

  const connectors = (await request('GET', `/api/v1/workspaces/${workspaceId}/connectors?project_id=${projectId}`)).data;
  assert(connectors.some((connector) => connector.kind === 'print'), 'restaurant template should create print connectors');
  for (const connector of connectors) {
    if (['print', 'webhook'].includes(connector.kind)) {
      await request('PATCH', `/api/v1/workspaces/${workspaceId}/connectors/${connector.id}`, {
        endpoint: receiverUrl,
        is_active: true,
      });
    }
  }

  const plugins = await request('GET', `/api/v1/projects/${projectId}/plugins`);
  const formulaPlugin = plugins.data.items.find((plugin) => plugin.key === 'restaurant_calc');
  assert(formulaPlugin, 'restaurant template should auto-install restaurant_calc plugin');
  assert(formulaPlugin.status === 'active', 'restaurant_calc plugin should be active');
  assert(formulaPlugin.wasm_sha256, 'restaurant_calc plugin should include wasm module');

  const category = await request('POST', `/api/v1/forms/${menuCategoryForm.id}/records`, {
    title: 'Noodles',
    values: { name: 'Noodles', sort_order: '1', status: 'active' },
    source: { type: 'smoke' },
  });
  const sku = await request('POST', `/api/v1/forms/${skuForm.id}/records`, {
    title: 'Beef Noodles',
    values: {
      sku: 'SKU-BEEF-NOODLE',
      name: 'Beef Noodles',
      category_id: { record_id: category.data.id },
      price: '9.99',
      kitchen_station: 'hot',
      status: 'available',
    },
    source: { type: 'smoke' },
  });
  const tableA = await request('POST', `/api/v1/forms/${tableForm.id}/records`, {
    title: 'A03',
    values: { table_no: 'A03', seat_count: '4', status: 'occupied' },
    source: { type: 'smoke' },
  });
  const tableB = await request('POST', `/api/v1/forms/${tableForm.id}/records`, {
    title: 'B02',
    values: { table_no: 'B02', seat_count: '2', status: 'empty' },
    source: { type: 'smoke' },
  });
  const order = await request('POST', `/api/v1/forms/${orderForm.id}/records`, {
    title: 'ORD-1001',
    values: {
      order_no: 'ORD-1001',
      table_id: { record_id: tableA.data.id },
      status: 'draft',
      total_amount: '0.00',
      opened_at: '2026-05-31T12:00',
    },
    source: { type: 'smoke' },
  });
  const orderLine = await request('POST', `/api/v1/forms/${orderLineForm.id}/records`, {
    title: 'Beef Noodles x2',
    values: {
      order_id: { record_id: order.data.id },
      sku_id: { record_id: sku.data.id },
      sku_name: 'Beef Noodles',
      quantity: '2',
      unit_price: '9.99',
      seat_no: '1',
      status: 'sent_to_kitchen',
    },
    source: { type: 'smoke' },
  });
  assert(orderLine.data.values.line_total.decimal === '19.98', 'WASM formula should patch decimal line_total');

  await request('POST', `/api/v1/form-records/${order.data.id}/links`, {
    target_type: 'form_record',
    target_id: orderLine.data.id,
    relation_key: 'order_lines',
    relation_type: 'parent_child',
    metadata: { source: 'restaurant_smoke' },
  });

  const movedOrder = await request('PATCH', `/api/v1/form-records/${order.data.id}`, {
    values: {
      order_no: 'ORD-1001',
      table_id: { record_id: tableB.data.id },
      status: 'sent_to_kitchen',
      total_amount: '19.98',
      opened_at: '2026-05-31T12:00',
    },
    source: { type: 'smoke', action: 'change_table' },
  });
  assert(movedOrder.data.values.total_amount.decimal === '19.98', 'order total should be decimal string');
  const tableChanged = await request('GET', `/api/v1/form-records/${order.data.id}/events?event_type=order.table_changed`);
  assert(tableChanged.data.items.length === 1, 'table change should emit order.table_changed');

  const failedKitchenPrint = await request('POST', `/api/v1/forms/${printJobForm.id}/records`, {
    title: 'Kitchen ticket failed attempt',
    values: {
      order_id: { record_id: order.data.id },
      job_type: 'kitchen',
      status: 'pending',
      printer: 'kitchen-01',
      payload: 'ORD-1001 Beef Noodles x2',
      retry_count: '0',
    },
    source: { type: 'smoke' },
  });
  const failedPrintInvocations = await waitForInvocations(
    projectId,
    (item) => item.connector_kind === 'print' && JSON.stringify(item.payload).includes(failedKitchenPrint.data.id),
    'failed kitchen print invocation',
  );
  await request('POST', `/api/v1/invocations/${failedPrintInvocations[0].id}/receipt`, {
    status: 'failed',
    idempotency_key: `restaurant-smoke-failed-${failedPrintInvocations[0].id}`,
    payload: { printer: 'kitchen-01', reason: 'paper_out' },
    error_message: 'paper_out',
  });

  const kitchenPrint = await request('POST', `/api/v1/forms/${printJobForm.id}/records`, {
    title: 'Kitchen ticket retry',
    values: {
      order_id: { record_id: order.data.id },
      job_type: 'kitchen',
      status: 'pending',
      printer: 'kitchen-01',
      payload: 'ORD-1001 Beef Noodles x2',
      retry_count: '1',
    },
    source: { type: 'smoke' },
  });
  const receiptPrint = await request('POST', `/api/v1/forms/${printJobForm.id}/records`, {
    title: 'Cashier receipt',
    values: {
      order_id: { record_id: order.data.id },
      job_type: 'receipt',
      status: 'pending',
      printer: 'cashier-01',
      payload: 'Receipt ORD-1001 total 19.98',
      retry_count: '0',
    },
    source: { type: 'smoke' },
  });
  assert(kitchenPrint.data.id !== receiptPrint.data.id, 'kitchen ticket and receipt should be different print jobs');
  const completedPrintInvocations = await waitForInvocations(
    projectId,
    (item) =>
      item.connector_kind === 'print' &&
      item.status !== 'failed' &&
      item.status !== 'cancelled' &&
      (JSON.stringify(item.payload).includes(kitchenPrint.data.id) || JSON.stringify(item.payload).includes(receiptPrint.data.id)),
    'completed print invocations',
  );
  const completedReceipt = await request('POST', `/api/v1/invocations/${completedPrintInvocations[0].id}/receipt`, {
    status: 'completed',
    idempotency_key: `restaurant-smoke-completed-${completedPrintInvocations[0].id}`,
    payload: { printer: 'kitchen-01', printed: true },
  });
  assert(completedReceipt.data.status === 'completed', 'print receipt should mark the selected print invocation completed');
  assert(completedReceipt.data.connector_kind === 'print', 'print receipt should keep the print connector kind');

  const report = await request('POST', `/api/v1/forms/${reportForm.id}/records`, {
    title: '2026-05-31',
    values: {
      report_date: '2026-05-31',
      gross_revenue: '19.98',
      order_count: '1',
      item_count: '2',
      printed_receipts: '1',
    },
    source: { type: 'smoke' },
  });
  assert(report.data.values.gross_revenue.decimal === '19.98', 'business report revenue should be decimal');

  const revenue = callTool('form_records.aggregate', {
    form_id: reportForm.id,
    field_key: 'gross_revenue',
    aggregate: 'sum',
  });
  assert(revenue.data.decimal === '19.98', 'MCP aggregate should query today revenue');

  const pluginInvocations = await request('GET', `/api/v1/plugins/${formulaPlugin.id}/invocations`);
  assert(pluginInvocations.data.items.some((item) => item.hook_kind === 'formula' && item.status === 'completed'), 'restaurant formula plugin invocation missing');
  const printJobEvents = await request('GET', `/api/v1/forms/${printJobForm.id}/events?event_type=print_job.created`);
  assert(printJobEvents.data.items.length >= 3, 'print_job.created events should exist for failed, retry and receipt jobs');
  const confirmedReceiptInvocation = await request('GET', `/api/v1/invocations/${completedReceipt.data.id}`);
  assert(confirmedReceiptInvocation.data.status === 'completed', 'print receipt should persist completed invocation status');
  assert(confirmedReceiptInvocation.data.connector_kind === 'print', 'print receipt should persist print connector kind');

  console.log('restaurant ordering smoke passed');
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE
