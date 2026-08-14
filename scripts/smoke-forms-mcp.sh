#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
POSTGRES_PORT="${OPENPR_SMOKE_PG_PORT:-5366}"
PG_SUPERUSER="${OPENPR_SMOKE_PG_SUPERUSER:-postgres}"
DB_NAME="openpr_forms_mcp_smoke_$$_$(date +%s)"
DB_USER="openpr_forms_mcp_smoke_$$_$(date +%s)"
DB_PASSWORD="$(openssl rand -hex 12)"
API_PORT="${OPENPR_SMOKE_API_PORT:-$((20180 + ($$ % 1000)))}"
TMP_DIR="$(mktemp -d /tmp/openpr-forms-mcp-smoke.XXXXXX)"
API_LOG="$TMP_DIR/api.log"
SMOKE_JWT_SECRET="openpr-forms-mcp-smoke-secret"

OWNER_ID="11111111-1111-4111-8111-111111111111"
WORKSPACE_ID="33333333-3333-4333-8333-333333333333"
BOT_ID="55555555-5555-4555-8555-555555555555"
BOT_TOKEN="opr_forms_mcp_$$_$(date +%s)"

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
VALUES ('$OWNER_ID', 'forms-mcp-owner@example.local', '', 'Forms MCP Owner', 'admin', true, 'human', NULL, now(), now());

INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE_ID', 'forms-mcp-smoke', 'Forms MCP Smoke', '$OWNER_ID', now(), now());

INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES ('$WORKSPACE_ID', '$OWNER_ID', 'owner', now());

INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by, is_active, created_at, updated_at)
VALUES (
  '$BOT_ID', '$WORKSPACE_ID', 'Forms MCP Bot', '$BOT_HASH',
  substring('$BOT_TOKEN' from 1 for 8), '["read","write","admin"]'::jsonb,
  '$OWNER_ID', true, now(), now()
);
SQL

API_URL="http://127.0.0.1:$API_PORT" \
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

const ownerToken = jwt(process.env.OWNER_ID, 'forms-mcp-owner@example.local');

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

function dataOf(payload) {
  return payload?.data ?? payload;
}

function itemsOf(payload) {
  const data = dataOf(payload);
  if (Array.isArray(data)) return data;
  return data?.items ?? [];
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
  let payload = null;
  try {
    payload = JSON.parse(text);
  } catch {
    payload = text;
  }
  return { response, text, payload };
}

function cliTool(name, args) {
  const result = spawnSync(
    mcpBin,
    [
      '--config',
      mcpConfig,
      '--api-url',
      apiUrl,
      '--bot-token',
      botToken,
      '--workspace-id',
      workspaceId,
      'tools',
      'call',
      '--name',
      name,
      '--args-json',
      JSON.stringify(args),
    ],
    {
      encoding: 'utf8',
      env: process.env,
      timeout: 20000,
    },
  );
  if (result.status !== 0) {
    throw new Error(`CLI tool call ${name} exited ${result.status}: ${result.stderr}`);
  }
  const text = result.stdout.trim();
  if (!text) {
    throw new Error(`CLI tool call ${name} produced no output`);
  }
  return JSON.parse(text);
}

async function main() {
  const project = await request('POST', `/api/v1/workspaces/${workspaceId}/projects`, {
    name: 'Forms MCP Project',
    key: 'FMCP',
    type_key: 'custom_form',
  });
  const projectId = dataOf(project).id;

  const tools = mcp('tools/list', {});
  const toolNames = (tools.result?.tools ?? []).map((tool) => tool.name);
  for (const name of [
    'forms.list',
    'forms.get',
    'forms.create',
    'forms.create_from_template',
    'scenario_templates.install',
    'forms.update_schema',
    'forms.duplicate',
    'forms.schema_summary',
    'forms.field_usage',
    'forms.field_dependencies',
    'form_schema_versions.list',
    'form_schema_versions.get',
    'form_permissions.get',
    'form_permissions.update',
    'form_views.list',
    'form_attachments.list',
    'form_attachments.create',
    'form_attachments.archive',
    'form_attachments.restore',
    'form_records.list',
    'form_records.export',
    'form_records.import_preview',
    'form_records.import_commit',
    'form_records.get',
    'form_records.create',
    'form_records.update',
    'form_records.link',
    'form_records.relation_targets',
    'form_records.children',
    'form_records.child_create',
    'form_records.child_update',
    'form_records.child_archive',
    'form_records.child_restore',
    'form_records.aggregate',
    'events.tail',
  ]) {
    assert(toolNames.includes(name), `MCP tools/list missing ${name}`);
  }

  const templates = mcp('resources/templates/list', {});
  const templateNames = (templates.result?.resourceTemplates ?? []).map((template) => template.uriTemplate);
  for (const template of [
    'openpr://projects/{project_id}/forms',
    'openpr://forms/{form_id}',
    'openpr://forms/{form_id}/records',
    'openpr://forms/{form_id}/events',
    'openpr://form-records/{record_id}',
    'openpr://form-records/{record_id}/events',
  ]) {
    assert(templateNames.includes(template), `MCP resources/templates/list missing ${template}`);
  }

  const installedScenario = callTool('scenario_templates.install', {
    project_id: projectId,
    template_key: 'contract_review_default',
  }).payload;
  const installedScenarioData = dataOf(installedScenario);
  for (const formKey of ['contract', 'risk_clause', 'approval_record']) {
    assert(
      (installedScenarioData?.installed?.form_keys ?? []).includes(formKey),
      `MCP scenario install should report ${formKey}: ${JSON.stringify(installedScenario)}`,
    );
  }
  assert(
    installedScenarioData?.project?.type_settings?.scenario_template_key === 'contract_review_default',
    'MCP scenario install should merge scenario metadata into project type_settings',
  );
  const scenarioForms = callTool('forms.list', { project_id: projectId }).payload;
  const scenarioFormKeys = itemsOf(scenarioForms).map((form) => form.key);
  for (const formKey of ['contract', 'risk_clause', 'approval_record']) {
    assert(scenarioFormKeys.includes(formKey), `MCP scenario install should create ${formKey}`);
  }

  const customerForm = callTool('forms.create', {
    project_id: projectId,
    key: 'customer',
    name: 'Customer',
    title_template: '{name}',
    schema: {
      version: 'openpr.form.schema.v1',
      fields: [
        { key: 'name', label: 'Name', type: 'text', required: true },
      ],
    },
  }).payload;
  const customerFormData = dataOf(customerForm);
  assert(customerFormData?.key === 'customer', 'MCP should create relation target form');

  const templatedForm = callTool('forms.create_from_template', {
    project_id: projectId,
    template_key: 'restaurant_ordering_default',
    key: 'templated_menu_smoke',
    name: 'Templated Menu Smoke',
    title_template: '{name}',
  }).payload;
  const templatedFormData = dataOf(templatedForm);
  assert(
    templatedFormData?.key === 'templated_menu_smoke',
    `MCP should create form from scenario template: ${JSON.stringify(templatedForm)}`,
  );
  assert(
    (templatedFormData?.schema?.fields ?? []).length > 0,
    'MCP template-created form should include template fields',
  );

  const customerRecord = callTool('form_records.create', {
    form_id: customerFormData.id,
    values: {
      name: 'Alice',
    },
  }).payload;
  const customerRecordData = dataOf(customerRecord);
  assert(customerRecordData?.title?.includes('Alice'), 'MCP should create relation target record');

  const createdForm = callTool('forms.create', {
    project_id: projectId,
    key: 'order',
    name: 'Order',
    title_template: '{table_no} {total_amount}',
    schema: {
      version: 'openpr.form.schema.v1',
      fields: [
        { key: 'table_no', label: 'Table No', type: 'text', required: true },
        {
          key: 'customer',
          label: 'Customer',
          type: 'relation',
          relation: { form_key: 'customer', display_field: 'name' },
        },
        { key: 'total_amount', label: 'Total Amount', type: 'amount', required: true },
        { key: 'receipt', label: 'Receipt', type: 'attachment' },
      ],
    },
  }).payload;
  const createdFormData = dataOf(createdForm);
  assert(createdFormData?.key === 'order', 'MCP should create form');
  const formId = createdFormData.id;

  const schemaVersionsBeforeUpdate = callTool('form_schema_versions.list', { form_id: formId }).payload;
  assert(
    itemsOf(schemaVersionsBeforeUpdate).some((item) => item.version === 1),
    'MCP should list baseline schema version',
  );

  const baselineSchemaVersion = callTool('form_schema_versions.get', {
    form_id: formId,
    version: 1,
  }).payload;
  assert(dataOf(baselineSchemaVersion)?.change_summary === 'initial schema', 'MCP should get baseline schema version');

  const updatedForm = callTool('forms.update_schema', {
    form_id: formId,
    schema: {
      version: 'openpr.form.schema.v1',
      fields: [
        { key: 'table_no', label: 'Table No', type: 'text', required: true },
        {
          key: 'customer',
          label: 'Customer',
          type: 'relation',
          relation: { form_key: 'customer', display_field: 'name' },
        },
        { key: 'total_amount', label: 'Total Amount', type: 'amount', required: true },
        { key: 'receipt', label: 'Receipt', type: 'attachment' },
        { key: 'status', label: 'Status', type: 'single_select', options: ['open', 'paid'] },
      ],
    },
  }).payload;
  assert(dataOf(updatedForm)?.schema_version === 2, 'MCP should update form schema version');

  const schemaVersionsAfterUpdate = callTool('form_schema_versions.list', { form_id: formId }).payload;
  assert(
    itemsOf(schemaVersionsAfterUpdate).some((item) => item.version === 2),
    'MCP should list updated schema version',
  );

  const updatedSchemaVersion = callTool('form_schema_versions.get', {
    form_id: formId,
    version: 2,
  }).payload;
  assert(
    (dataOf(updatedSchemaVersion)?.schema?.fields ?? []).some((field) => field.key === 'status'),
    'MCP should get updated schema version fields',
  );

  await request('POST', `/api/v1/forms/${formId}/views`, {
    key: 'grid',
    name: 'Grid',
    view_type: 'grid',
    config: {
      columns: ['table_no', 'customer', 'total_amount'],
      sort: [],
      filters: [],
    },
  });

  const listedForms = callTool('forms.list', { project_id: projectId }).payload;
  assert(
    itemsOf(listedForms).some((item) => item.id === formId),
    `MCP should list created form: ${JSON.stringify(listedForms)}`,
  );

  const schemaSummary = callTool('forms.schema_summary', { form_id: formId }).payload;
  assert(dataOf(schemaSummary)?.field_count >= 3, 'MCP should read form schema summary');

  const fieldUsage = callTool('forms.field_usage', { form_id: formId }).payload;
  assert((dataOf(fieldUsage)?.fields ?? []).some((field) => field.field_key === 'table_no'), 'MCP should read form field usage');

  const fieldDependencies = callTool('forms.field_dependencies', { form_id: formId }).payload;
  assert(
    (dataOf(fieldDependencies)?.dependencies ?? []).some((dependency) => dependency.field_key === 'table_no'),
    'MCP should read form field dependencies',
  );

  const permissionsBefore = callTool('form_permissions.get', { form_id: formId }).payload;
  assert(
    dataOf(permissionsBefore)?.effective?.actions?.['form.view'] === true,
    'MCP should read effective form permissions',
  );

  const permissionsAfter = callTool('form_permissions.update', {
    form_id: formId,
    policies: [
      {
        subject_type: 'role',
        subject_id: 'member',
        policy: {
          actions: {
            'form.view': true,
            'record.create': false,
            'record.export': false,
            'form.design': false,
          },
        },
      },
    ],
  }).payload;
  assert(
    (dataOf(permissionsAfter)?.policies ?? []).some(
      (policy) => policy.subject_type === 'role' && policy.subject_id === 'member',
    ),
    'MCP should update form permission policies',
  );

  const relationTargets = callTool('form_records.relation_targets', {
    form_id: formId,
    field_key: 'customer',
    q: 'Alice',
  }).payload;
  assert(
    itemsOf(relationTargets).some((item) => item.record_id === customerRecordData.id),
    'MCP should list relation target records',
  );

  const formsResource = mcp('resources/read', { uri: `openpr://projects/${projectId}/forms` });
  assert(JSON.stringify(formsResource).includes('order'), 'MCP project forms resource should include form key');

  const formResource = mcp('resources/read', { uri: `openpr://forms/${formId}` });
  assert(JSON.stringify(formResource).includes('total_amount'), 'MCP form resource should include schema field');

  const createdRecord = callTool('form_records.create', {
    form_id: formId,
    idempotency_key: 'mcp-smoke-record-create-b01',
    values: {
      table_no: 'B01',
      customer: customerRecordData.id,
      total_amount: '0.30',
    },
  }).payload;
  const createdRecordData = dataOf(createdRecord);
  assert(createdRecordData?.values?.total_amount?.decimal === '0.30', 'MCP should create decimal string amount record');
  const recordId = createdRecordData.id;

  const retriedRecordCreate = callTool('form_records.create', {
    form_id: formId,
    idempotency_key: 'mcp-smoke-record-create-b01',
    values: {
      table_no: 'B01 duplicate retry should not write',
      customer: customerRecordData.id,
      total_amount: '99.99',
    },
  }).payload;
  assert(
    dataOf(retriedRecordCreate)?.id === recordId,
    'MCP create idempotency should return the original record on retry',
  );

  const exportedRecords = callTool('form_records.export', {
    form_id: formId,
    columns: 'table_no,total_amount',
  }).payload;
  assert(dataOf(exportedRecords)?.csv?.includes('B01'), 'MCP should export current form records');
  assert(dataOf(exportedRecords)?.csv?.includes('0.30'), 'MCP export should preserve amount decimal display');

  const importPreview = callTool('form_records.import_preview', {
    form_id: formId,
    rows: [
      {
        row_number: 1,
        values: {
          table_no: 'B03',
          total_amount: '2.20',
        },
      },
    ],
  }).payload;
  assert(dataOf(importPreview)?.valid_rows === 1, 'MCP should preview valid import rows');

  const attachment = callTool('form_attachments.create', {
    form_id: formId,
    record_id: recordId,
    field_key: 'receipt',
    file_name: 'receipt-b01.pdf',
    content_type: 'application/pdf',
    byte_size: 128,
    storage_key: `smoke/${recordId}/receipt-b01.pdf`,
    url: 'https://example.invalid/receipt-b01.pdf',
  }).payload;
  const attachmentData = dataOf(attachment);
  assert(attachmentData?.field_key === 'receipt', 'MCP should create attachment metadata');

  const attachmentsBeforeArchive = callTool('form_attachments.list', {
    form_id: formId,
    record_id: recordId,
    field_key: 'receipt',
  }).payload;
  assert(
    itemsOf(attachmentsBeforeArchive).some((item) => item.id === attachmentData.id),
    'MCP should list attachment metadata',
  );

  const archivedAttachment = callTool('form_attachments.archive', {
    attachment_id: attachmentData.id,
  }).payload;
  assert(dataOf(archivedAttachment)?.archived === true, 'MCP should archive attachment metadata');

  const attachmentsWithArchived = callTool('form_attachments.list', {
    form_id: formId,
    record_id: recordId,
    field_key: 'receipt',
    include_archived: true,
  }).payload;
  assert(
    itemsOf(attachmentsWithArchived).some((item) => item.id === attachmentData.id && item.archived_at),
    'MCP should list archived attachment metadata when requested',
  );

  const restoredAttachment = callTool('form_attachments.restore', {
    attachment_id: attachmentData.id,
  }).payload;
  assert(dataOf(restoredAttachment)?.archived_at === null, 'MCP should restore attachment metadata');

  const lineForm = callTool('forms.create', {
    project_id: projectId,
    key: 'order_line',
    name: 'Order Line',
    title_template: '{item}',
    schema: {
      version: 'openpr.form.schema.v1',
      fields: [
        { key: 'item', label: 'Item', type: 'text', required: true },
        { key: 'qty', label: 'Qty', type: 'integer', required: true },
      ],
    },
  }).payload;
  const lineFormData = dataOf(lineForm);
  assert(lineFormData?.key === 'order_line', 'MCP should create child form');

  const parentWithChildRelation = callTool('forms.update_schema', {
    form_id: formId,
    schema: {
      version: 'openpr.form.schema.v1',
      fields: [
        { key: 'table_no', label: 'Table No', type: 'text', required: true },
        {
          key: 'customer',
          label: 'Customer',
          type: 'relation',
          relation: { form_key: 'customer', display_field: 'name' },
        },
        { key: 'total_amount', label: 'Total Amount', type: 'amount', required: true },
        { key: 'receipt', label: 'Receipt', type: 'attachment' },
        { key: 'status', label: 'Status', type: 'single_select', options: ['open', 'paid'] },
        {
          key: 'lines',
          label: 'Lines',
          type: 'relation',
          relation: { form_key: 'order_line', display_field: 'item', relation_type: 'parent_child' },
        },
      ],
    },
  }).payload;
  assert(
    (dataOf(parentWithChildRelation)?.schema?.fields ?? []).some((field) => field.key === 'lines'),
    'MCP should update parent form with child relation field',
  );

  const childCreated = callTool('form_records.child_create', {
    record_id: recordId,
    relation_key: 'lines',
    child_form_key: 'order_line',
    values: {
      item: 'Noodles',
      qty: 2,
    },
  }).payload;
  const lineRecordData = dataOf(childCreated)?.record;
  assert(lineRecordData?.values?.item === 'Noodles', 'MCP should create child record through parent_child tool');
  assert(dataOf(childCreated)?.link?.relation_key === 'lines', 'MCP child create should return parent-child link');

  const childUpdated = callTool('form_records.child_update', {
    record_id: recordId,
    child_record_id: lineRecordData.id,
    relation_key: 'lines',
    values: {
      item: 'Noodles',
      qty: 3,
    },
  }).payload;
  assert(
    dataOf(childUpdated)?.record?.values?.qty?.value === 3,
    `MCP should update linked child record: ${JSON.stringify(childUpdated)}`,
  );

  const childArchived = callTool('form_records.child_archive', {
    record_id: recordId,
    child_record_id: lineRecordData.id,
    relation_key: 'lines',
  }).payload;
  assert(dataOf(childArchived)?.archived === true, 'MCP should archive linked child record');

  const archivedChildRecords = callTool('form_records.children', {
    record_id: recordId,
    relation_key: 'lines',
  }).payload;
  assert(
    !itemsOf(archivedChildRecords).some((item) => item.record.id === lineRecordData.id),
    'MCP child archive should hide archived child record from active children',
  );

  const childRestored = callTool('form_records.child_restore', {
    record_id: recordId,
    child_record_id: lineRecordData.id,
    relation_key: 'lines',
  }).payload;
  assert(dataOf(childRestored)?.record?.archived_at === null, 'MCP should restore linked child record');

  const childRecords = callTool('form_records.children', {
    record_id: recordId,
    relation_key: 'lines',
  }).payload;
  assert(
    itemsOf(childRecords).some((item) => item.record.id === lineRecordData.id && item.record.values.qty?.value === 3),
    'MCP should list child records',
  );

  const duplicatedForm = callTool('forms.duplicate', {
    form_id: formId,
    key: 'order_copy',
    name: 'Order Copy',
  }).payload;
  const duplicatedFormData = dataOf(duplicatedForm);
  assert(duplicatedFormData?.key === 'order_copy', 'MCP should duplicate form');
  assert(duplicatedFormData?.title_template === createdFormData.title_template, 'MCP duplicate should copy title template');

  const duplicateViews = callTool('form_views.list', { form_id: duplicatedFormData.id }).payload;
  assert(itemsOf(duplicateViews).length >= 1, 'MCP duplicate should copy active views');

  const duplicateRecords = callTool('form_records.list', { form_id: duplicatedFormData.id }).payload;
  assert(itemsOf(duplicateRecords).length === 0, 'MCP duplicate should not copy records');

  const recordsResource = mcp('resources/read', { uri: `openpr://forms/${formId}/records` });
  assert(JSON.stringify(recordsResource).includes('B01'), 'MCP form records resource should include created record');

  const recordResource = mcp('resources/read', { uri: `openpr://form-records/${recordId}` });
  assert(JSON.stringify(recordResource).includes('0.30'), 'MCP form record resource should preserve amount decimal');

  const invalidRecord = callTool('form_records.create', {
    form_id: formId,
    values: {
      table_no: 'B02',
      total_amount: 0.3,
    },
  }).payload;
  assert(invalidRecord.code === 400, 'MCP JSON number amount should be rejected');

  const updatedRecord = callTool('form_records.update', {
    record_id: recordId,
    values: {
      table_no: 'B01',
      total_amount: '1.30',
    },
  }).payload;
  assert(dataOf(updatedRecord)?.values?.total_amount?.decimal === '1.30', 'MCP should update amount record');

  const aggregate = callTool('form_records.aggregate', {
    form_id: formId,
    field_key: 'total_amount',
    aggregate: 'sum',
  }).payload;
  assert(dataOf(aggregate)?.decimal === '1.30', 'MCP aggregate should return decimal string');

  const importCommit = callTool('form_records.import_commit', {
    form_id: formId,
    idempotency_key: 'mcp-smoke-import-b03',
    rows: [
      {
        row_number: 1,
        values: {
          table_no: 'B03',
          total_amount: '2.20',
        },
      },
    ],
  }).payload;
  const importedRecord = dataOf(importCommit)?.records?.[0];
  assert(dataOf(importCommit)?.created_count === 1, 'MCP should commit valid import rows');
  assert(importedRecord?.values?.total_amount?.decimal === '2.20', 'MCP import should normalize decimal amount strings');

  const importRetry = callTool('form_records.import_commit', {
    form_id: formId,
    idempotency_key: 'mcp-smoke-import-b03',
    rows: [
      {
        row_number: 1,
        values: {
          table_no: 'B03 retry should not write',
          total_amount: '88.88',
        },
      },
    ],
  }).payload;
  assert(
    dataOf(importRetry)?.records?.[0]?.id === importedRecord.id,
    'MCP import idempotency should return the original imported record on retry',
  );

  const exportedAfterImport = callTool('form_records.export', {
    form_id: formId,
    columns: 'table_no,total_amount',
  }).payload;
  assert(dataOf(exportedAfterImport)?.csv?.includes('B03'), 'MCP export should include imported records');
  assert(
    !dataOf(exportedAfterImport)?.csv?.includes('B03 retry should not write'),
    'MCP import idempotency should not write duplicate retry rows',
  );

  const cliListedForms = cliTool('forms.list', { project_id: projectId });
  assert(
    itemsOf(cliListedForms).some((item) => item.id === formId),
    'generic CLI tools call should list created form',
  );

  const cliAggregate = cliTool('form_records.aggregate', {
    form_id: formId,
    field_key: 'total_amount',
    aggregate: 'sum',
  });
  assert(
    dataOf(cliAggregate)?.decimal === '3.50',
    'generic CLI tools call should return MCP aggregate decimal after import',
  );

  const events = callTool('events.tail', {
    record_id: recordId,
    event_type: 'form.record.created',
  }).payload;
  assert(itemsOf(events).length >= 1, 'MCP events.tail should read record events');
  assert(
    itemsOf(events)[0].payload?.values?.total_amount?.decimal === '0.30',
    'MCP event payload should preserve decimal string',
  );
  assert(
    itemsOf(events)[0].idempotency_key === 'mcp-smoke-record-create-b01',
    'MCP create event should expose idempotency receipt key',
  );

  const updateEvents = callTool('events.tail', {
    record_id: recordId,
    event_type: 'form.record.updated',
  }).payload;
  assert(itemsOf(updateEvents)[0]?.payload?.values?.total_amount?.decimal === '1.30', 'MCP update should emit business event');

  const recordEventsResource = mcp('resources/read', { uri: `openpr://form-records/${recordId}/events` });
  assert(JSON.stringify(recordEventsResource).includes('form.record.created'), 'MCP record events resource should include creation event');

  console.log('Forms MCP smoke passed');
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE
