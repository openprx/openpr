#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
POSTGRES_PORT="${OPENPR_SMOKE_PG_PORT:-5366}"
PG_SUPERUSER="${OPENPR_SMOKE_PG_SUPERUSER:-postgres}"
DB_NAME="openpr_phase3_smoke_$$_$(date +%s)"
DB_USER="openpr_phase3_smoke_$$_$(date +%s)"
DB_PASSWORD="$(openssl rand -hex 12)"
API_PORT="${OPENPR_SMOKE_API_PORT:-$((17380 + ($$ % 1000)))}"
TMP_DIR="$(mktemp -d /tmp/openpr-phase3-smoke.XXXXXX)"
API_LOG="$TMP_DIR/api.log"
SMOKE_JWT_SECRET="openpr-phase3-smoke-secret"

OWNER_ID="11111111-1111-4111-8111-111111111111"
WORKSPACE_ID="22222222-2222-4222-8222-222222222222"
BOT_ID="33333333-3333-4333-8333-333333333333"
BOT_TOKEN="opr_phase3_workspace_$$_$(date +%s)"

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
  ('$OWNER_ID', 'owner-phase3@example.local', '', 'Phase 3 Owner', 'admin', true, 'human', NULL, now(), now()),
  ('$BOT_ID', 'bot-phase3@example.local', '', 'Phase 3 MCP Bot', 'user', true, 'bot', 'mcp', now(), now());

INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE_ID', 'phase3-smoke', 'Phase 3 Smoke', '$OWNER_ID', now(), now());

INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES
  ('$WORKSPACE_ID', '$OWNER_ID', 'owner', now()),
  ('$WORKSPACE_ID', '$BOT_ID', 'member', now());

INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by, is_active, created_at, updated_at)
VALUES ('$BOT_ID', '$WORKSPACE_ID', 'Phase 3 MCP Bot', '$BOT_HASH', substring('$BOT_TOKEN' from 1 for 8), '["read","write","admin"]'::jsonb, '$OWNER_ID', true, now(), now());

INSERT INTO project_types (
  key, workspace_id, name, description, domain, enabled_capabilities,
  field_schema, artifact_schema, default_connectors, created_at, updated_at
)
VALUES (
  'phase3_restricted', '$WORKSPACE_ID', 'Phase 3 Restricted', 'Restricted MCP policy smoke type', 'test',
  '["issues","mcp"]'::jsonb, '{}'::jsonb, '{}'::jsonb, '["mcp"]'::jsonb, now(), now()
)
ON CONFLICT (key) DO UPDATE
SET enabled_capabilities = EXCLUDED.enabled_capabilities,
    updated_at = now();
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
const fs = require('fs');

const apiUrl = process.env.API_URL;
const workspaceId = process.env.WORKSPACE_ID;
const jwtSecret = process.env.SMOKE_JWT_SECRET;
const botToken = process.env.BOT_TOKEN;
const mcpBin = process.env.MCP_BIN;
const mcpConfig = process.env.MCP_CONFIG;

// The correlation id that stamps an MCP tool call onto an invocation ledger row is a
// configuration key (mcp.invocation_id), not an environment variable, so a call that has to be
// audited runs against its own copy of the configuration file rather than its own environment.
function invocationConfig(invocationId) {
  const path = mcpConfig.replace(/\.toml$/, '.invocation.toml');
  fs.writeFileSync(path, `[logging]\nfilter = "error"\nformat = "text"\n\n[mcp]\ninvocation_id = "${invocationId}"\n`);
  return path;
}

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

const ownerToken = jwt(process.env.OWNER_ID, 'owner-phase3@example.local');

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

function mcpRequest(method, params, configPath = mcpConfig) {
  const requestPayload = JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }) + '\n';
  const result = spawnSync(
    mcpBin,
    ['--config', configPath, '--api-url', apiUrl, '--bot-token', botToken, '--workspace-id', workspaceId, 'serve', '--transport', 'stdio'],
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
  if (!line) throw new Error(`MCP ${method} produced no JSON: ${result.stdout} ${result.stderr}`);
  return JSON.parse(line);
}

function mcpTool(name, args = {}, configPath = mcpConfig) {
  const response = mcpRequest('tools/call', { name, arguments: args }, configPath);
  const text = response.result?.content?.[0]?.text;
  if (!text) throw new Error(`MCP ${name} response has no text content: ${JSON.stringify(response)}`);
  if (response.result?.isError) return { isError: true, text };
  try {
    return JSON.parse(text);
  } catch {
    return { isError: true, text };
  }
}

function toolNames(response) {
  return (response.result?.tools ?? []).map((tool) => tool.name);
}

async function main() {
  const init = mcpRequest('initialize', {
    protocolVersion: '2025-06-18',
    capabilities: {},
    clientInfo: { name: 'phase3-smoke', version: '1.0.0' },
  });
  assert(init.result?.serverInfo?.name, 'initialize should return serverInfo');

  const resourcesList = mcpRequest('resources/list', {});
  const staticResourceUris = (resourcesList.result?.resources ?? []).map((resource) => resource.uriTemplate ?? resource.uri);
  assert(staticResourceUris.includes('openpr://scenario-templates'), 'resources/list should include static scenario templates');
  const templatesList = mcpRequest('resources/templates/list', {});
  const resourceUris = (templatesList.result?.resourceTemplates ?? templatesList.result?.resources ?? []).map(
    (resource) => resource.uriTemplate ?? resource.uri,
  );
  assert(resourceUris.includes('openpr://projects/{project_id}/context'), 'resources/list should include project context');
  assert(resourceUris.includes('openpr://projects/{project_id}/agent-policy'), 'resources/list should include agent policy');

  const codeProject = await request(ownerToken, 'POST', `/api/v1/workspaces/${workspaceId}/projects`, {
    name: 'Phase 3 Code MCP Project',
    key: 'P3CODE',
    description: 'Phase 3 MCP code project',
    type_key: 'code_project',
  });
  const contractProject = await request(ownerToken, 'POST', `/api/v1/workspaces/${workspaceId}/projects`, {
    name: 'Phase 3 Contract MCP Project',
    key: 'P3DOC',
    description: 'Phase 3 MCP contract project',
    type_key: 'contract_review',
  });
  const restrictedProject = await request(ownerToken, 'POST', `/api/v1/workspaces/${workspaceId}/projects`, {
    name: 'Phase 3 Restricted MCP Project',
    key: 'P3LIMIT',
    description: 'Phase 3 policy restricted project',
    type_key: 'phase3_restricted',
  });

  await request(ownerToken, 'POST', `/api/v1/projects/${codeProject.id}/resources`, {
    kind: 'repo',
    name: 'openpr repository',
    locator: { url: 'https://git.example/openpr' },
  });
  await request(ownerToken, 'POST', `/api/v1/projects/${codeProject.id}/resources`, {
    kind: 'directory',
    name: 'apps/api',
    locator: { path: 'apps/api' },
  });
  await request(ownerToken, 'POST', `/api/v1/projects/${contractProject.id}/resources`, {
    kind: 'document_library',
    name: 'Contract library',
    locator: { url: 'https://docs.example/contracts' },
  });
  const connector = await request(ownerToken, 'POST', `/api/v1/workspaces/${workspaceId}/connectors`, {
    project_id: codeProject.id,
    kind: 'mcp',
    name: 'Phase 3 Dynamic MCP Connector',
    description: 'Dynamic connector tool smoke',
    endpoint: null,
    auth_policy: { mode: 'none' },
    capability_manifest: {
      mcp_tools: [{
        name: 'vendor.phase3_check',
        description: 'Phase 3 dynamic connector tool',
        input_schema: { type: 'object', properties: { payload: { type: 'object' } } },
        action_class: 'external_side_effect',
      }],
    },
    is_active: true,
  });

  const staticTools = toolNames(mcpRequest('tools/list', {}));
  assert(staticTools.includes('context.get_project'), 'static tools/list should include context.get_project');
  assert(staticTools.includes('check_results.create'), 'static tools/list should include check_results.create');

  const codeTools = toolNames(mcpRequest('tools/list', { project_id: codeProject.id }));
  assert(codeTools.includes('code.resources.list'), 'code project tools/list should include code.resources.list');
  assert(codeTools.includes('check_results.create'), 'code project tools/list should include check_results.create');
  assert(codeTools.includes('vendor.phase3_check'), 'code project tools/list should include dynamic connector tool');

  const contractTools = toolNames(mcpRequest('tools/list', { project_id: contractProject.id }));
  assert(contractTools.includes('documents.review_risk'), 'contract project tools/list should include document risk tool');
  assert(contractTools.includes('approval.request'), 'contract project tools/list should include approval tool');

  const restrictedTools = toolNames(mcpRequest('tools/list', { project_id: restrictedProject.id }));
  assert(restrictedTools.includes('work_items.list'), 'restricted project should keep issue tools');
  assert(!restrictedTools.includes('sprints.create'), 'restricted project should hide planning tools');
  assert(!restrictedTools.includes('proposals.create'), 'restricted project should hide governance proposal tools');

  const denied = mcpTool('sprints.create', { project_id: restrictedProject.id, name: 'Should be denied' });
  assert(denied.isError && denied.text.includes('disabled by project agent policy'), 'disabled project tool call should be rejected by policy');

  const context = mcpTool('context.get_project', { project_id: codeProject.id });
  assert(JSON.stringify(context).includes('Phase 3 Code MCP Project'), 'context.get_project should read project context');
  const contextResource = mcpRequest('resources/read', { uri: `openpr://projects/${codeProject.id}/context` });
  assert(JSON.stringify(contextResource).includes('openpr repository'), 'project context resource should include project resources');
  const policyResource = mcpRequest('resources/read', { uri: `openpr://projects/${codeProject.id}/agent-policy` });
  assert(JSON.stringify(policyResource).includes('vendor.phase3_check'), 'agent policy resource should include dynamic connector tool');

  const beforeRead = (await request(ownerToken, 'GET', `/api/v1/projects/${codeProject.id}/invocations`)).items.length;
  mcpTool('code.resources.list', { project_id: codeProject.id });
  mcpTool('code.directory.get', { project_id: codeProject.id, name: 'apps/api' });
  const afterRead = (await request(ownerToken, 'GET', `/api/v1/projects/${codeProject.id}/invocations`)).items.length;
  assert(afterRead === beforeRead, 'read-oriented MCP tools should not create noisy invocations');

  const invocation = mcpTool('invocations.create', {
    project_id: codeProject.id,
    trigger_kind: 'mcp',
    payload: { source: 'phase3-acceptance-smoke' },
  });
  const invocationId = invocation.data?.id;
  assert(invocationId, 'invocations.create should return an invocation id');

  const auditedContext = mcpTool('context.get_project', { project_id: codeProject.id }, invocationConfig(invocationId));
  assert(JSON.stringify(auditedContext).includes('Phase 3 Code MCP Project'), 'audited context call should still succeed');
  const toolCalls = await request(ownerToken, 'GET', `/api/v1/invocations/${invocationId}/tool-calls`);
  assert(toolCalls.items?.some((call) => call.tool_name === 'context.get_project' && call.status === 'succeeded'), 'mcp.invocation_id should audit MCP tool calls');

  const dynamicInvocation = mcpTool('vendor.phase3_check', {
    project_id: codeProject.id,
    payload: { source: 'phase3-dynamic-tool' },
  });
  assert(dynamicInvocation.data?.connector_id === connector.id, 'dynamic connector tool should create connector-bound invocation');
  assert(dynamicInvocation.data?.trigger_ref_type === 'connector_tool', 'dynamic connector tool should use connector_tool trigger ref');

  const checkResult = mcpTool('check_results.create', {
    project_id: codeProject.id,
    invocation_id: invocationId,
    action_class: 'high_risk_mutation',
    risk_level: 'high',
    title: 'Phase 3 governed high-risk action',
    summary: 'This high-risk action must become a proposal before side effects.',
    result: { tool: 'phase3.high_risk', arguments: { target: 'demo' } },
  });
  const checkResultId = checkResult.data?.id;
  assert(checkResultId, 'check_results.create should return id');
  assert(checkResult.data?.status === 'requires_proposal', 'high-risk check result should require proposal');
  assert(checkResult.data?.created_by_kind === 'ai', 'MCP-created check result should be attributed to AI');

  const proposal = mcpTool('proposals.create_from_result', {
    check_result_id: checkResultId,
    title: 'Phase 3 governed proposal',
    proposal_type: 'governance',
    content: 'Approve the governed action from the Phase 3 smoke after human review of the recorded MCP result, audit trail, and proposed side effects.',
    domains: ['mcp', 'governance'],
    submit: false,
  });
  assert(proposal.data?.id, `proposals.create_from_result should create proposal: ${JSON.stringify(proposal)}`);
  const proposedResults = await request(ownerToken, 'GET', `/api/v1/projects/${codeProject.id}/check-results?status=proposed`);
  assert(proposedResults.items?.some((item) => item.id === checkResultId && item.proposal_id === proposal.data.id), 'check result should move to proposed');

  const scenarioCheck = mcpTool('documents.review_risk', {
    project_id: contractProject.id,
    title: 'Phase 3 contract risk review',
    summary: 'Risk review should be represented as governed check result.',
    result: { risk: 'medium' },
  });
  assert(scenarioCheck.data?.status === 'requires_proposal', 'document risk review should produce governed check result');

  const completed = mcpTool('invocations.complete', {
    invocation_id: invocationId,
    result: { status: 'success', source: 'phase3-acceptance-smoke' },
  });
  assert(completed.data?.status === 'completed', 'invocation should be completed through MCP');

  console.log('Phase 3 MCP governance acceptance smoke passed');
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

CHECK_RESULT_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type IN ('check_result.created', 'check_result.proposed')")"
if [[ "$CHECK_RESULT_EVENT_COUNT" -lt 3 ]]; then
  echo "Expected at least 3 check result business events, got $CHECK_RESULT_EVENT_COUNT" >&2
  exit 1
fi

PROPOSAL_FROM_RESULT_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type = 'proposal.created_from_check_result'")"
if [[ "$PROPOSAL_FROM_RESULT_EVENT_COUNT" -lt 1 ]]; then
  echo "Expected at least 1 proposal.created_from_check_result business event, got $PROPOSAL_FROM_RESULT_EVENT_COUNT" >&2
  exit 1
fi

CHECK_RESULT_OUTBOX_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM event_outbox WHERE event_type IN ('check_result.created', 'check_result.proposed', 'proposal.created_from_check_result')")"
if [[ "$CHECK_RESULT_OUTBOX_COUNT" -lt 4 ]]; then
  echo "Expected at least 4 check result/proposal outbox rows, got $CHECK_RESULT_OUTBOX_COUNT" >&2
  exit 1
fi

echo "Phase 3 check result business events and outbox smoke passed"
