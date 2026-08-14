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

  const connector = await request('POST', `/api/v1/workspaces/${workspaceId}/connectors`, {
    project_id: projectId,
    kind: 'rest',
    name: 'Forms event receiver',
    endpoint: receiverUrl,
    capability_manifest: {
      events: ['form.record.created', 'form.record.linked'],
    },
  });
  assert(connector.data.kind === 'rest', 'rest connector should be created');

  const printConnector = await request('POST', `/api/v1/workspaces/${workspaceId}/connectors`, {
    project_id: projectId,
    kind: 'print',
    name: 'Kitchen print receiver',
    endpoint: receiverUrl,
    capability_manifest: {
      events: ['form.record.created', 'form.record.linked'],
      project_types: ['custom_form'],
      form_keys: ['order'],
      connector_kinds: ['print'],
      device_class: 'thermal_receipt_printer',
    },
  });
  assert(printConnector.data.kind === 'print', 'print connector should be created');

  const printJobConnector = await request('POST', `/api/v1/workspaces/${workspaceId}/connectors`, {
    project_id: projectId,
    kind: 'print',
    name: 'Print job receiver',
    endpoint: receiverUrl,
    capability_manifest: {
      events: ['print_job.created'],
      project_types: ['custom_form'],
      form_keys: ['print_job'],
      connector_kinds: ['print'],
      device_class: 'thermal_receipt_printer',
    },
  });
  assert(printJobConnector.data.kind === 'print', 'print_job connector should be created');

  const deviceConnector = await request('POST', `/api/v1/workspaces/${workspaceId}/connectors`, {
    project_id: projectId,
    kind: 'device',
    name: 'Kitchen display device receiver',
    endpoint: receiverUrl,
    capability_manifest: {
      events: ['form.record.created'],
      project_types: ['custom_form'],
      form_keys: ['order_line'],
      connector_kinds: ['device'],
      device_class: 'kitchen_display',
    },
  });
  assert(deviceConnector.data.kind === 'device', 'device connector should be created');

  const wildcardConnector = await request('POST', `/api/v1/workspaces/${workspaceId}/connectors`, {
    project_id: projectId,
    kind: 'rest',
    name: 'Wildcard receiver without lifecycle subscription',
    endpoint: receiverUrl,
    capability_manifest: {},
  });
  assert(wildcardConnector.data.kind === 'rest', 'wildcard rest connector should be created');

  const auditConnector = await request('POST', `/api/v1/workspaces/${workspaceId}/connectors`, {
    project_id: projectId,
    kind: 'webhook',
    name: 'Connector audit receiver',
    endpoint: receiverUrl,
    capability_manifest: {
      events: ['connector.created'],
    },
  });
  assert(auditConnector.data.kind === 'webhook', 'connector audit webhook should be created');
  const updatedAuditConnector = await request(
    'PATCH',
    `/api/v1/workspaces/${workspaceId}/connectors/${auditConnector.data.id}`,
    {
      name: 'Connector audit receiver updated',
      is_active: false,
    },
  );
  assert(updatedAuditConnector.data.is_active === false, 'connector update should persist');
  await request('DELETE', `/api/v1/workspaces/${workspaceId}/connectors/${auditConnector.data.id}`, undefined);

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

CONNECTOR_WRITE_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type IN ('connector.created', 'connector.updated', 'connector.deleted')")"
if [[ "$CONNECTOR_WRITE_EVENT_COUNT" -lt 10 ]]; then
  echo "Expected at least 10 connector write business events, got $CONNECTOR_WRITE_EVENT_COUNT" >&2
  exit 1
fi

LEGACY_WEBHOOK_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type IN ('webhook.created', 'webhook.updated', 'webhook.deleted') AND metadata->>'legacy_webhook' = 'true'")"
if [[ "$LEGACY_WEBHOOK_EVENT_COUNT" -lt 3 ]]; then
  echo "Expected at least 3 legacy webhook business events, got $LEGACY_WEBHOOK_EVENT_COUNT" >&2
  exit 1
fi

LEGACY_WEBHOOK_CONNECTOR_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type IN ('connector.created', 'connector.updated', 'connector.deleted') AND metadata->>'legacy_webhook' = 'true'")"
if [[ "$LEGACY_WEBHOOK_CONNECTOR_EVENT_COUNT" -lt 3 ]]; then
  echo "Expected at least 3 legacy webhook connector business events, got $LEGACY_WEBHOOK_CONNECTOR_EVENT_COUNT" >&2
  exit 1
fi

OUTBOX_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM event_outbox WHERE event_type IN ('form.record.created', 'form.record.linked')")"
if [[ "$OUTBOX_COUNT" -lt 3 ]]; then
  echo "Expected at least 3 outbox rows, got $OUTBOX_COUNT" >&2
  exit 1
fi

wait_sql_count "SELECT COUNT(*) FROM event_outbox WHERE status = 'dispatched' AND event_type IN ('form.record.created', 'form.record.linked')" 3 "dispatched form outbox rows"
wait_sql_count "SELECT COUNT(*) FROM event_outbox WHERE status = 'dispatched' AND event_type IN ('form.created', 'form.updated', 'form.archived', 'form.view.created', 'form.view.archived')" 9 "dispatched form definition/view outbox rows"
wait_sql_count "SELECT COUNT(*) FROM event_outbox WHERE status = 'dispatched' AND event_type IN ('connector.created', 'connector.updated', 'connector.deleted')" 10 "dispatched connector write outbox rows"
wait_sql_count "SELECT COUNT(*) FROM event_outbox WHERE status = 'dispatched' AND event_type IN ('webhook.created', 'webhook.updated', 'webhook.deleted')" 3 "dispatched legacy webhook outbox rows"
wait_sql_count "SELECT COUNT(*) FROM event_outbox eo JOIN business_events be ON be.id = eo.business_event_id WHERE eo.status = 'dispatched' AND eo.event_type IN ('connector.created', 'connector.updated', 'connector.deleted') AND be.metadata->>'legacy_webhook' = 'true'" 3 "dispatched legacy webhook connector outbox rows"
wait_sql_count "SELECT COUNT(*) FROM agent_invocations WHERE trigger_kind = 'workflow' AND payload->>'event' IN ('form.record.created', 'form.record.linked')" 7 "connector invocations from event outbox"
wait_sql_count "SELECT COUNT(*) FROM agent_invocations WHERE status = 'dispatched' AND trigger_kind = 'workflow' AND payload->>'event' IN ('form.record.created', 'form.record.linked')" 7 "dispatched connector invocations"
wait_sql_count "SELECT COUNT(*) FROM business_events WHERE event_type = 'invocation.created' AND aggregate_type = 'invocation' AND metadata->>'worker_event' = 'true' AND metadata->>'trigger_kind' = 'workflow'" 7 "workflow invocation.created business events"
wait_sql_count "SELECT COUNT(*) FROM business_events WHERE event_type = 'invocation.running' AND aggregate_type = 'invocation' AND metadata->>'worker_event' = 'true' AND metadata->>'trigger_kind' = 'workflow'" 7 "workflow invocation.running business events"
wait_sql_count "SELECT COUNT(*) FROM business_events WHERE event_type = 'invocation.dispatched' AND aggregate_type = 'invocation' AND metadata->>'worker_event' = 'true' AND metadata->>'trigger_kind' = 'workflow'" 7 "workflow invocation.dispatched business events"
wait_sql_count "SELECT COUNT(*) FROM event_outbox WHERE event_type IN ('invocation.created', 'invocation.running', 'invocation.dispatched') AND aggregate_type = 'invocation' AND payload->'metadata'->>'worker_event' = 'true' AND payload->'metadata'->>'trigger_kind' = 'workflow'" 21 "workflow invocation lifecycle outbox rows"
sleep 2
WILDCARD_LIFECYCLE_INVOCATION_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM agent_invocations WHERE connector_id = (SELECT id FROM connectors WHERE name = 'Wildcard receiver without lifecycle subscription' LIMIT 1) AND payload->>'event' LIKE 'invocation.%'")"
if [[ "$WILDCARD_LIFECYCLE_INVOCATION_COUNT" -ne 0 ]]; then
  echo "Expected wildcard connector without explicit events to skip invocation lifecycle events, got $WILDCARD_LIFECYCLE_INVOCATION_COUNT" >&2
  exit 1
fi
echo "Universal forms invocation lifecycle fanout guard smoke passed"
echo "Universal forms workflow invocation business events and outbox smoke passed"
wait_sql_count "SELECT COUNT(*) FROM agent_invocations WHERE status = 'dispatched' AND connector_kind = 'print' AND trigger_kind = 'workflow' AND payload->>'event' IN ('form.record.created', 'form.record.linked')" 2 "dispatched print connector invocations"
wait_sql_count "SELECT COUNT(*) FROM agent_invocations WHERE status = 'dispatched' AND connector_kind = 'device' AND trigger_kind = 'workflow' AND payload->>'event' = 'form.record.created'" 1 "dispatched device connector invocation"
wait_sql_count "SELECT COUNT(*) FROM event_outbox WHERE status = 'dispatched' AND event_type = 'print_job.created'" 1 "dispatched print_job.created outbox rows"
wait_sql_count "SELECT COUNT(*) FROM agent_invocations WHERE status = 'dispatched' AND connector_kind = 'print' AND payload->>'event' = 'print_job.created'" 1 "dispatched print_job.created connector invocation"

RETRY_EVENT_ID="$(psql_smoke -Atqc "SELECT gen_random_uuid()")"
RETRY_OUTBOX_ID="$(psql_smoke -Atqc "SELECT gen_random_uuid()")"
RETRY_CORRELATION_ID="$(psql_smoke -Atqc "SELECT gen_random_uuid()")"
RETRY_CAUSATION_ID="$(psql_smoke -Atqc "SELECT gen_random_uuid()")"
RETRY_IDEMPOTENCY_KEY="forms-retry-$DB_NAME"
LINE_RECORD_ID="$(psql_smoke -Atqc "SELECT id FROM form_records WHERE form_id = (SELECT id FROM project_forms WHERE project_id = '$PROJECT_ID' AND key = 'order_line') ORDER BY created_at LIMIT 1")"

psql_smoke -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO business_events (
  id, workspace_id, project_id, event_type, aggregate_type, aggregate_id,
  actor_id, source, payload, metadata, correlation_id, causation_id,
  idempotency_key, created_at
)
VALUES (
  '$RETRY_EVENT_ID', '$WORKSPACE_ID', '$PROJECT_ID', 'form.record.created',
  'form_record', '$LINE_RECORD_ID', '$OWNER_ID', '{"kind":"smoke"}'::jsonb,
  '{"values":{"sku_name":"Retry Noodles"}}'::jsonb,
  '{"form_key":"order_line"}'::jsonb,
  '$RETRY_CORRELATION_ID', '$RETRY_CAUSATION_ID',
  '$RETRY_IDEMPOTENCY_KEY', now()
);

INSERT INTO event_outbox (
  id, business_event_id, workspace_id, project_id, event_type,
  aggregate_type, aggregate_id, payload, headers, status,
  attempts, max_attempts, available_at, leased_until, last_error,
  created_at, updated_at
)
VALUES (
  '$RETRY_OUTBOX_ID', '$RETRY_EVENT_ID', '$WORKSPACE_ID', '$PROJECT_ID',
  'form.record.created', 'form_record', '$LINE_RECORD_ID',
  jsonb_build_object(
    'version', 'openpr.event.v1',
    'event_id', '$RETRY_EVENT_ID',
    'event_type', 'form.record.created',
    'workspace_id', '$WORKSPACE_ID',
    'project_id', '$PROJECT_ID',
    'aggregate', jsonb_build_object('type', 'form_record', 'id', '$LINE_RECORD_ID'),
    'actor_id', '$OWNER_ID',
    'source', jsonb_build_object('kind', 'smoke'),
    'payload', jsonb_build_object('values', jsonb_build_object('sku_name', 'Retry Noodles')),
    'metadata', jsonb_build_object('form_key', 'order_line'),
    'correlation_id', '$RETRY_CORRELATION_ID',
    'causation_id', '$RETRY_CAUSATION_ID'
  ),
  jsonb_build_object(
    'schema', 'openpr.event.v1',
    'idempotency_key', '$RETRY_IDEMPOTENCY_KEY',
    'correlation_id', '$RETRY_CORRELATION_ID',
    'causation_id', '$RETRY_CAUSATION_ID'
  ),
  'failed', 1, 10, now() - interval '1 second', NULL, 'synthetic retry smoke',
  now(), now()
);
SQL

if psql_smoke -v ON_ERROR_STOP=1 -q <<SQL >/dev/null 2>&1
INSERT INTO business_events (
  id, workspace_id, project_id, event_type, aggregate_type, aggregate_id,
  source, payload, metadata, idempotency_key, created_at
)
VALUES (
  gen_random_uuid(), '$WORKSPACE_ID', '$PROJECT_ID', 'form.record.created',
  'form_record', '$LINE_RECORD_ID', '{}'::jsonb, '{}'::jsonb, '{}'::jsonb,
  '$RETRY_IDEMPOTENCY_KEY', now()
);
SQL
then
  echo "Expected duplicate business event idempotency key to be rejected" >&2
  exit 1
fi

wait_sql_count "SELECT COUNT(*) FROM event_outbox WHERE id = '$RETRY_OUTBOX_ID' AND status = 'dispatched' AND attempts = 2 AND last_error IS NULL" 1 "retried failed event_outbox row"
wait_sql_count "SELECT COUNT(*) FROM agent_invocations WHERE status = 'dispatched' AND connector_kind = 'device' AND audit_chain_id = '$RETRY_EVENT_ID'" 1 "device connector invocation from retried event"

RETRY_HEADER_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM event_outbox WHERE id = '$RETRY_OUTBOX_ID' AND headers->>'correlation_id' = '$RETRY_CORRELATION_ID' AND headers->>'causation_id' = '$RETRY_CAUSATION_ID' AND headers->>'idempotency_key' = '$RETRY_IDEMPOTENCY_KEY'")"
if [[ "$RETRY_HEADER_COUNT" -ne 1 ]]; then
  echo "Expected retry outbox headers to retain correlation, causation, and idempotency metadata" >&2
  exit 1
fi

PRINT_ORDER_LINE_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM agent_invocations WHERE connector_kind = 'print' AND payload->>'event' = 'form.record.created' AND payload->'envelope'->'metadata'->>'form_key' = 'order_line'")"
if [[ "$PRINT_ORDER_LINE_COUNT" -ne 0 ]]; then
  echo "Expected print connector routing to exclude order_line form events, got $PRINT_ORDER_LINE_COUNT" >&2
  exit 1
fi

DEVICE_NON_ORDER_LINE_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM agent_invocations WHERE connector_kind = 'device' AND payload->>'event' = 'form.record.created' AND COALESCE(payload->'envelope'->'metadata'->>'form_key', '') <> 'order_line'")"
if [[ "$DEVICE_NON_ORDER_LINE_COUNT" -ne 0 ]]; then
  echo "Expected device connector routing to include only order_line form events, got $DEVICE_NON_ORDER_LINE_COUNT non-order_line events" >&2
  exit 1
fi

for _ in $(seq 1 30); do
  if [[ -f "$RECEIVER_LOG" ]] && [[ "$(grep -c 'form.record' "$RECEIVER_LOG" || true)" -ge 7 ]] && [[ "$(grep -c 'print_job.created' "$RECEIVER_LOG" || true)" -ge 1 ]]; then
    break
  fi
  sleep 1
done

RECEIVED_COUNT="$(grep -c 'form.record' "$RECEIVER_LOG" || true)"
if [[ "$RECEIVED_COUNT" -lt 7 ]]; then
  echo "Expected at least 7 received connector events, got $RECEIVED_COUNT" >&2
  exit 1
fi

PRINT_RECEIVED_COUNT="$(grep -c '"connector_kind":"print"' "$RECEIVER_LOG" || true)"
if [[ "$PRINT_RECEIVED_COUNT" -lt 2 ]]; then
  echo "Expected at least 2 received print connector events, got $PRINT_RECEIVED_COUNT" >&2
  exit 1
fi

DEVICE_RECEIVED_COUNT="$(grep -c '"connector_kind":"device"' "$RECEIVER_LOG" || true)"
if [[ "$DEVICE_RECEIVED_COUNT" -lt 1 ]]; then
  echo "Expected at least 1 received device connector event, got $DEVICE_RECEIVED_COUNT" >&2
  exit 1
fi

PRINT_JOB_RECEIVED_COUNT="$(grep -c 'print_job.created' "$RECEIVER_LOG" || true)"
if [[ "$PRINT_JOB_RECEIVED_COUNT" -lt 1 ]]; then
  echo "Expected at least 1 received print_job.created connector event, got $PRINT_JOB_RECEIVED_COUNT" >&2
  exit 1
fi

PRINT_INVOCATION_ID="$(psql_smoke -Atqc "SELECT id FROM agent_invocations WHERE status = 'dispatched' AND connector_kind = 'print' AND payload->>'event' = 'form.record.created' ORDER BY created_at LIMIT 1")"
if [[ -z "$PRINT_INVOCATION_ID" ]]; then
  echo "Expected one dispatched print invocation for receipt test" >&2
  exit 1
fi

API_URL="http://127.0.0.1:$API_PORT" \
SMOKE_JWT_SECRET="$SMOKE_JWT_SECRET" \
OWNER_ID="$OWNER_ID" \
PRINT_INVOCATION_ID="$PRINT_INVOCATION_ID" \
node --input-type=commonjs <<'NODE'
const crypto = require('crypto');

const apiUrl = process.env.API_URL;
const jwtSecret = process.env.SMOKE_JWT_SECRET;
const invocationId = process.env.PRINT_INVOCATION_ID;

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

const token = jwt(process.env.OWNER_ID, 'forms-owner@example.local');

async function postReceipt() {
  const response = await fetch(`${apiUrl}/api/v1/invocations/${invocationId}/receipt`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${token}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      status: 'completed',
      idempotency_key: `print-receipt-${invocationId}`,
      payload: {
        printer: 'kitchen-01',
        printed: true,
      },
    }),
  });
  const text = await response.text();
  const payload = text ? JSON.parse(text) : null;
  if (response.status !== 200 || payload?.data?.status !== 'completed') {
    throw new Error(`receipt failed: HTTP ${response.status} ${text}`);
  }
}

postReceipt().then(postReceipt).catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

RECEIPT_INBOX_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM event_inbox WHERE idempotency_key = 'print-receipt-$PRINT_INVOCATION_ID'")"
if [[ "$RECEIPT_INBOX_COUNT" -ne 1 ]]; then
  echo "Expected idempotent connector receipt inbox count 1, got $RECEIPT_INBOX_COUNT" >&2
  exit 1
fi

PRINT_COMPLETED_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM agent_invocations WHERE id = '$PRINT_INVOCATION_ID' AND status = 'completed'")"
if [[ "$PRINT_COMPLETED_COUNT" -ne 1 ]]; then
  echo "Expected print invocation receipt to mark invocation completed" >&2
  exit 1
fi

echo "Universal forms API smoke passed (rest, print, device, retry, and idempotency covered)"
