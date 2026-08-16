#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
POSTGRES_PORT="${OPENPR_SMOKE_PG_PORT:-5366}"
PG_SUPERUSER="${OPENPR_SMOKE_PG_SUPERUSER:-postgres}"
DB_NAME="openpr_forms_smoke_$$_$(date +%s)"
DB_USER="openpr_forms_smoke_$$_$(date +%s)"
DB_PASSWORD="$(openssl rand -hex 12)"
API_PORT="${OPENPR_SMOKE_API_PORT:-$((18180 + ($$ % 1000)))}"
TMP_DIR="$(mktemp -d /tmp/openpr-forms-smoke.XXXXXX)"
API_LOG="$TMP_DIR/api.log"
WORKER_LOG="$TMP_DIR/worker.log"
RECEIVER_LOG="$TMP_DIR/receiver.ndjson"
SMOKE_JWT_SECRET="openpr-forms-smoke-secret"
RECEIVER_PORT="${OPENPR_SMOKE_RECEIVER_PORT:-$((19180 + ($$ % 1000)))}"

OWNER_ID="11111111-1111-4111-8111-111111111111"
MEMBER_ID="22222222-2222-4222-8222-222222222222"
WORKSPACE_ID="33333333-3333-4333-8333-333333333333"

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

wait_sql_count() {
  local sql="$1"
  local expected="$2"
  local label="$3"
  local count
  for _ in $(seq 1 60); do
    count="$(psql_smoke -Atqc "$sql")"
    if [[ "$count" -ge "$expected" ]]; then
      return 0
    fi
    sleep 1
  done
  echo "Timed out waiting for $label, last count: $count" >&2
  return 1
}

require_cmd curl
require_cmd node
require_cmd openssl
require_cmd psql
require_cmd sudo

cargo build -q -p api --bin api -p worker --bin worker

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

psql_smoke -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES
  ('$OWNER_ID', 'forms-owner@example.local', '', 'Forms Owner', 'admin', true, 'human', NULL, now(), now()),
  ('$MEMBER_ID', 'forms-member@example.local', '', 'Forms Member', 'user', true, 'human', NULL, now(), now());

INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE_ID', 'forms-smoke', 'Forms Smoke', '$OWNER_ID', now(), now());

INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES
  ('$WORKSPACE_ID', '$OWNER_ID', 'owner', now()),
  ('$WORKSPACE_ID', '$MEMBER_ID', 'member', now());
SQL

API_URL="http://127.0.0.1:$API_PORT" \
RECEIVER_URL="http://127.0.0.1:$RECEIVER_PORT/events" \
SMOKE_JWT_SECRET="$SMOKE_JWT_SECRET" \
OWNER_ID="$OWNER_ID" \
WORKSPACE_ID="$WORKSPACE_ID" \
node --input-type=commonjs <<'NODE'
const crypto = require('crypto');

const apiUrl = process.env.API_URL;
const workspaceId = process.env.WORKSPACE_ID;
const jwtSecret = process.env.SMOKE_JWT_SECRET;
const receiverUrl = process.env.RECEIVER_URL;

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

const ownerToken = jwt(process.env.OWNER_ID, 'forms-owner@example.local');

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

async function main() {
  const project = await request('POST', `/api/v1/workspaces/${workspaceId}/projects`, {
    name: 'Restaurant Forms Smoke',
    key: 'RFS',
    description: 'Universal form API smoke',
    type_key: 'custom_form',
  });
  const projectId = project.data.id;


  const legacyWebhook = await request('POST', `/api/v1/workspaces/${workspaceId}/webhooks`, {
    name: 'Legacy webhook receiver',
    url: receiverUrl,
    events: ['issue.created'],
    active: true,
  });
  assert(legacyWebhook.data.active === true, 'legacy webhook should be active');
  const updatedLegacyWebhook = await request(
    'PATCH',
    `/api/v1/workspaces/${workspaceId}/webhooks/${legacyWebhook.data.id}`,
    {
      name: 'Legacy webhook receiver updated',
      active: false,
    },
  );
  assert(updatedLegacyWebhook.data.active === false, 'legacy webhook update should persist');
  await request('DELETE', `/api/v1/workspaces/${workspaceId}/webhooks/${legacyWebhook.data.id}`, undefined);

  const orderForm = await request('POST', `/api/v1/projects/${projectId}/forms`, {
    key: 'order',
    name: 'Order',
    title_template: '{table_no} {total_amount}',
    schema: {
      version: 'openpr.form.schema.v1',
      fields: [
        { key: 'table_no', label: 'Table No', type: 'text', required: true },
        {
          key: 'total_amount',
          label: 'Total Amount',
          type: 'amount',
          required: true,
          config: { currency: 'CNY', scale: 2, rounding: 'round_half_up' },
        },
        { key: 'status', label: 'Status', type: 'single_select', options: ['open', 'paid'], required: true },
      ],
    },
  });
  assert(orderForm.data.key === 'order', 'order form should be created');

  const lineForm = await request('POST', `/api/v1/projects/${projectId}/forms`, {
    key: 'order_line',
    name: 'Order Line',
    title_template: '{sku_name}',
    schema: {
      version: 'openpr.form.schema.v1',
      fields: [
        { key: 'sku_name', label: 'SKU', type: 'text', required: true },
        { key: 'quantity', label: 'Quantity', type: 'integer', required: true },
        {
          key: 'line_amount',
          label: 'Line Amount',
          type: 'amount',
          required: true,
          config: { currency: 'CNY', scale: 2, rounding: 'round_half_up' },
        },
      ],
    },
  });

  const printJobForm = await request('POST', `/api/v1/projects/${projectId}/forms`, {
    key: 'print_job',
    name: 'Print Job',
    title_template: '{job_type} {status}',
    schema: {
      version: 'openpr.form.schema.v1',
      fields: [
        { key: 'order_id', label: 'Order ID', type: 'text', required: true },
        { key: 'job_type', label: 'Job Type', type: 'single_select', options: ['kitchen', 'receipt'], required: true },
        { key: 'status', label: 'Status', type: 'single_select', options: ['pending', 'printed', 'failed'], required: true },
      ],
    },
  });

  const view = await request('POST', `/api/v1/forms/${orderForm.data.id}/views`, {
    key: 'default_grid',
    name: 'Default Grid',
    view_type: 'grid',
    config: { columns: ['table_no', 'total_amount', 'status'] },
  });
  assert(view.data.view_type === 'grid', 'grid view should be created');

  const auditForm = await request('POST', `/api/v1/projects/${projectId}/forms`, {
    key: 'write_audit',
    name: 'Write Audit',
    title_template: '{id}',
    schema: {
      version: 'openpr.form.schema.v1',
      fields: [{ key: 'label', label: 'Label', type: 'text', required: true }],
    },
  });
  const updatedAuditForm = await request('PATCH', `/api/v1/forms/${auditForm.data.id}`, {
    name: 'Write Audit Updated',
    description: 'Verifies form write events',
  });
  assert(updatedAuditForm.data.name === 'Write Audit Updated', 'form update should be persisted');
  const auditView = await request('POST', `/api/v1/forms/${auditForm.data.id}/views`, {
    key: 'audit_grid',
    name: 'Audit Grid',
    view_type: 'grid',
    config: { columns: ['label'] },
  });
  await request('DELETE', `/api/v1/form-views/${auditView.data.id}`, undefined);
  await request('DELETE', `/api/v1/forms/${auditForm.data.id}`, undefined);

  const order = await request('POST', `/api/v1/forms/${orderForm.data.id}/records`, {
    values: {
      table_no: 'A01',
      total_amount: '0.30',
      status: 'open',
    },
  });
  assert(order.data.values.total_amount.decimal === '0.30', 'amount should remain a decimal string');
  assert(order.data.title.includes('0.30 CNY'), 'title template should render amount display');

  const invalidAmount = await request('POST', `/api/v1/forms/${orderForm.data.id}/records`, {
    values: {
      table_no: 'A02',
      total_amount: 0.3,
      status: 'open',
    },
  });
  assert(invalidAmount.code === 400, 'JSON number amount should be rejected by API code');

  const line = await request('POST', `/api/v1/forms/${lineForm.data.id}/records`, {
    values: {
      sku_name: 'Noodles',
      quantity: 2,
      line_amount: '24.00',
    },
  });

  const printJob = await request('POST', `/api/v1/forms/${printJobForm.data.id}/records`, {
    values: {
      order_id: order.data.id,
      job_type: 'kitchen',
      status: 'pending',
    },
  });
  assert(printJob.data.values.job_type === 'kitchen', 'print_job record should be created');

  const link = await request('POST', `/api/v1/form-records/${order.data.id}/links`, {
    target_type: 'form_record',
    target_id: line.data.id,
    relation_key: 'order_lines',
    relation_type: 'parent_child',
  });
  assert(link.data.relation_type === 'parent_child', 'child record link should be created');

  const links = await request('GET', `/api/v1/form-records/${order.data.id}/links`);
  assert(links.data.some((item) => item.target_id === line.data.id), 'child record link should be listed');

  const fetchedOrder = await request('GET', `/api/v1/form-records/${order.data.id}`);
  assert(fetchedOrder.data.values.total_amount.decimal === '0.30', 'record detail should preserve decimal string');

  const amountAggregate = await request(
    'GET',
    `/api/v1/forms/${orderForm.data.id}/aggregate?field_key=total_amount&aggregate=sum`,
  );
  assert(amountAggregate.data.decimal === '0.30', 'amount aggregate should return a decimal string');
  assert(amountAggregate.data.currency === 'CNY', 'amount aggregate should return currency');
  assert(amountAggregate.data.count === 1, 'amount aggregate should return counted projection rows');

  const invalidAggregate = await request('GET', `/api/v1/forms/${orderForm.data.id}/aggregate?field_key=table_no`);
  assert(invalidAggregate.code === 400, 'text field aggregate should be rejected');

  const formEvents = await request(
    'GET',
    `/api/v1/forms/${orderForm.data.id}/events?event_type=form.record.created`,
  );
  assert(formEvents.data.items.length >= 1, 'form events should include record creation');
  assert(
    formEvents.data.items.every((item) => item.payload?.values?.total_amount?.decimal === '0.30'),
    'form events should keep amount decimal as a string',
  );

  const recordEvents = await request('GET', `/api/v1/form-records/${order.data.id}/events`);
  assert(
    recordEvents.data.items.some((item) => item.event_type === 'form.record.linked'),
    'record events should include child link event',
  );

  console.log(JSON.stringify({
    project_id: projectId,
    order_form_id: orderForm.data.id,
    line_form_id: lineForm.data.id,
    order_record_id: order.data.id,
    line_record_id: line.data.id,
  }));
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM form_events WHERE event_type IN ('form.record.created', 'form.record.linked')")"
if [[ "$EVENT_COUNT" -lt 3 ]]; then
  echo "Expected at least 3 form events, got $EVENT_COUNT" >&2
  exit 1
fi

DECIMAL_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM form_record_field_index WHERE field_key IN ('total_amount', 'line_amount') AND value_decimal IS NOT NULL")"
if [[ "$DECIMAL_COUNT" -lt 2 ]]; then
  echo "Expected decimal projection rows, got $DECIMAL_COUNT" >&2
  exit 1
fi

LINK_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM form_record_links WHERE relation_type = 'parent_child'")"
if [[ "$LINK_COUNT" -ne 1 ]]; then
  echo "Expected one parent_child link, got $LINK_COUNT" >&2
  exit 1
fi

PROJECT_ID="$(psql_smoke -Atqc "SELECT id FROM projects WHERE key = 'RFS' LIMIT 1")"
INBOX_KEY="forms-smoke-inbox-$DB_NAME"
psql_smoke -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO event_inbox (
  id, workspace_id, project_id, source_kind, source_id, idempotency_key,
  event_type, payload, status, received_at, updated_at
)
VALUES (
  gen_random_uuid(), '$WORKSPACE_ID', '$PROJECT_ID', 'smoke_receiver', 'local',
  '$INBOX_KEY', 'form.receipt.received', '{"ok":true}'::jsonb, 'received', now(), now()
)
ON CONFLICT (source_kind, idempotency_key) DO NOTHING;

INSERT INTO event_inbox (
  id, workspace_id, project_id, source_kind, source_id, idempotency_key,
  event_type, payload, status, received_at, updated_at
)
VALUES (
  gen_random_uuid(), '$WORKSPACE_ID', '$PROJECT_ID', 'smoke_receiver', 'local',
  '$INBOX_KEY', 'form.receipt.received', '{"ok":true}'::jsonb, 'received', now(), now()
)
ON CONFLICT (source_kind, idempotency_key) DO NOTHING;
SQL

INBOX_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM event_inbox WHERE source_kind = 'smoke_receiver' AND idempotency_key = '$INBOX_KEY'")"
if [[ "$INBOX_COUNT" -ne 1 ]]; then
  echo "Expected idempotent inbox insert count 1, got $INBOX_COUNT" >&2
  exit 1
fi

BUSINESS_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type IN ('form.record.created', 'form.record.linked')")"
if [[ "$BUSINESS_EVENT_COUNT" -lt 3 ]]; then
  echo "Expected at least 3 business events, got $BUSINESS_EVENT_COUNT" >&2
  exit 1
fi

FORM_WRITE_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type IN ('form.created', 'form.updated', 'form.archived', 'form.view.created', 'form.view.archived')")"
if [[ "$FORM_WRITE_EVENT_COUNT" -lt 9 ]]; then
  echo "Expected at least 9 form definition/view write business events, got $FORM_WRITE_EVENT_COUNT" >&2
  exit 1
fi

WEBHOOK_WRITE_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type IN ('webhook.created', 'webhook.updated', 'webhook.deleted')")"
if [[ "$WEBHOOK_WRITE_EVENT_COUNT" -lt 3 ]]; then
  echo "Expected webhook create/update/delete business events, got $WEBHOOK_WRITE_EVENT_COUNT" >&2
  exit 1
fi
echo "Legacy webhook business events passed"

echo "Universal forms API smoke passed (retry and idempotency covered)"
