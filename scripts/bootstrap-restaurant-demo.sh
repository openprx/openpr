#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'EOF'
Usage: scripts/bootstrap-restaurant-demo.sh

Creates a local restaurant ordering demo through the public OpenPR API:
first user registration/login, workspace creation, restaurant scenario project
creation, and sample universal-form records for menu, tables, order, order
line, parent-child link, and business report.

Environment:
  OPENPR_API_URL                 API base URL. Default: http://localhost:8081
  OPENPR_DEMO_EMAIL             Demo login email. Default: demo@openpr.local
  OPENPR_DEMO_PASSWORD          Demo login password. Default: OpenPRDemo123!
  OPENPR_DEMO_NAME              Demo user name. Default: OpenPR Demo
  OPENPR_DEMO_WORKSPACE_SLUG    Workspace slug. Default: restaurant-demo
  OPENPR_DEMO_WORKSPACE_NAME    Workspace name. Default: Restaurant Demo
  OPENPR_DEMO_PROJECT_KEY       Project key. Default: RESTDEMO
  OPENPR_DEMO_PROJECT_NAME      Project name. Default: Restaurant Ordering Demo
  OPENPR_DEMO_ALLOW_REMOTE=1    Allow non-localhost API URLs.
  OPENPR_DEMO_WRITE_ENV=0       Do not update local .env with MCP credentials.
                                Default: auto, write only when .env exists.
  OPENPR_DEMO_ENV_PATH          Env file path for MCP credentials.
                                Default: repo .env.
  OPENPR_DEMO_RESTART_MCP=0     Do not recreate running compose mcp-server.
                                Default: 1.
  OPENPR_DEMO_VERIFY_MCP_HTTP   Verify local MCP JSON-RPC after bootstrap.
                                Values: auto, 1, 0. Default: auto.
  OPENPR_DEMO_MCP_URL           MCP JSON-RPC URL.
                                Default: http://localhost:8090/mcp/rpc
  OPENPR_MCP_AUTH_TOKEN         Bearer token for the MCP JSON-RPC endpoint.
                                Read from OPENPR_DEMO_ENV_PATH when unset.

This is a local onboarding helper, not a production seeding tool.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "Missing required command: $1" >&2
    exit 2
  }
}

require_cmd node

DEMO_ENV_PATH="${OPENPR_DEMO_ENV_PATH:-$ROOT_DIR/.env}"

# Reads one KEY=value from an env file, without sourcing it.
read_env_value() {
  local key="$1"
  local file="$2"
  local line=""
  [ -f "$file" ] || return 0
  line="$(grep -E "^${key}=.+" "$file" | tail -n 1 || true)"
  [ -n "$line" ] || return 0
  line="${line#*=}"
  # Strip one layer of surrounding quotes, the way compose reads .env.
  case "$line" in
    \"*\") line="${line#\"}"; line="${line%\"}" ;;
    \'*\') line="${line#\'}"; line="${line%\'}" ;;
  esac
  printf '%s' "$line"
}

# The MCP verification below posts to /mcp/rpc, which requires a bearer token when the server
# was started with OPENPR_MCP_AUTH_TOKEN. /health is exempt and stays unauthenticated.
MCP_AUTH_TOKEN="${OPENPR_MCP_AUTH_TOKEN:-}"
if [ -z "$MCP_AUTH_TOKEN" ]; then
  MCP_AUTH_TOKEN="$(read_env_value OPENPR_MCP_AUTH_TOKEN "$DEMO_ENV_PATH")"
fi

DEMO_STATE_FILE="$(mktemp "${TMPDIR:-/tmp}/openpr-restaurant-demo.XXXXXX.json")"
trap 'rm -f "$DEMO_STATE_FILE"' EXIT

API_URL="${OPENPR_API_URL:-http://localhost:8081}"
case "$API_URL" in
  http://localhost:*|http://127.0.0.1:*|http://[::1]:*)
    ;;
  *)
    if [[ "${OPENPR_DEMO_ALLOW_REMOTE:-0}" != "1" ]]; then
      echo "Refusing to seed a non-local API URL: $API_URL" >&2
      echo "Set OPENPR_DEMO_ALLOW_REMOTE=1 only if this is an intentional demo environment." >&2
      exit 2
    fi
    ;;
esac

OPENPR_API_URL="$API_URL" \
OPENPR_DEMO_EMAIL="${OPENPR_DEMO_EMAIL:-demo@openpr.local}" \
OPENPR_DEMO_PASSWORD="${OPENPR_DEMO_PASSWORD:-OpenPRDemo123!}" \
OPENPR_DEMO_NAME="${OPENPR_DEMO_NAME:-OpenPR Demo}" \
OPENPR_DEMO_WORKSPACE_SLUG="${OPENPR_DEMO_WORKSPACE_SLUG:-restaurant-demo}" \
OPENPR_DEMO_WORKSPACE_NAME="${OPENPR_DEMO_WORKSPACE_NAME:-Restaurant Demo}" \
OPENPR_DEMO_PROJECT_KEY="${OPENPR_DEMO_PROJECT_KEY:-RESTDEMO}" \
OPENPR_DEMO_PROJECT_NAME="${OPENPR_DEMO_PROJECT_NAME:-Restaurant Ordering Demo}" \
OPENPR_DEMO_WRITE_ENV="${OPENPR_DEMO_WRITE_ENV:-auto}" \
OPENPR_DEMO_ENV_PATH="$DEMO_ENV_PATH" \
OPENPR_DEMO_STATE_FILE="$DEMO_STATE_FILE" \
node --input-type=commonjs <<'NODE'
const fs = require('fs');
const apiUrl = process.env.OPENPR_API_URL.replace(/\/+$/, '');
const email = process.env.OPENPR_DEMO_EMAIL;
const password = process.env.OPENPR_DEMO_PASSWORD;
const name = process.env.OPENPR_DEMO_NAME;
const workspaceSlug = process.env.OPENPR_DEMO_WORKSPACE_SLUG;
const workspaceName = process.env.OPENPR_DEMO_WORKSPACE_NAME;
const projectKey = process.env.OPENPR_DEMO_PROJECT_KEY;
const projectName = process.env.OPENPR_DEMO_PROJECT_NAME;
const envPath = process.env.OPENPR_DEMO_ENV_PATH;
const writeEnvMode = process.env.OPENPR_DEMO_WRITE_ENV ?? 'auto';
const stateFile = process.env.OPENPR_DEMO_STATE_FILE;

async function rawRequest(method, path, body, token) {
  const response = await fetch(`${apiUrl}${path}`, {
    method,
    headers: {
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  let payload = null;
  try {
    payload = text ? JSON.parse(text) : null;
  } catch {
    payload = { code: response.status, message: text || response.statusText };
  }
  return { response, payload };
}

async function request(method, path, body, token, expectedCode = 0) {
  const { response, payload } = await rawRequest(method, path, body, token);
  if (!response.ok || payload?.code !== expectedCode) {
    throw new Error(`${method} ${path} failed: http=${response.status} code=${payload?.code} message=${payload?.message ?? '<none>'}`);
  }
  return payload.data;
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function formByKey(forms, key) {
  const form = forms.find((item) => item.key === key);
  assert(form, `restaurant demo missing form ${key}`);
  return form;
}

function readEnvFile() {
  if (!envPath || !fs.existsSync(envPath)) return {};
  const values = {};
  for (const line of fs.readFileSync(envPath, 'utf8').split(/\r?\n/)) {
    const match = line.match(/^([A-Za-z_][A-Za-z0-9_]*)=(.*)$/);
    if (match) values[match[1]] = match[2];
  }
  return values;
}

function upsertEnvFile(updates) {
  if (!envPath) return false;
  const shouldWrite = writeEnvMode === '1' || (writeEnvMode === 'auto' && fs.existsSync(envPath));
  if (!shouldWrite) return false;

  const original = fs.existsSync(envPath) ? fs.readFileSync(envPath, 'utf8') : '';
  const seen = new Set();
  const lines = original.split(/\r?\n/).map((line) => {
    for (const [key, value] of Object.entries(updates)) {
      if (line.startsWith(`${key}=`) || line.startsWith(`#${key}=`)) {
        seen.add(key);
        return `${key}=${value}`;
      }
    }
    return line;
  });

  for (const [key, value] of Object.entries(updates)) {
    if (!seen.has(key)) lines.push(`${key}=${value}`);
  }

  fs.writeFileSync(envPath, lines.join('\n').replace(/\n*$/, '\n'));
  return true;
}

async function authenticate() {
  const health = await rawRequest('GET', '/health');
  if (!health.response.ok) {
    throw new Error(`OpenPR API is not reachable at ${apiUrl}/health`);
  }

  const register = await rawRequest('POST', '/api/v1/auth/register', {
    email,
    password,
    name,
  });
  if (register.response.ok && register.payload?.code === 0) {
    return register.payload.data.tokens.access_token;
  }

  const login = await rawRequest('POST', '/api/v1/auth/login', {
    email,
    password,
  });
  if (login.response.ok && login.payload?.code === 0) {
    return login.payload.data.tokens.access_token;
  }

  throw new Error(
    `Could not register or login demo user ${email}. If this instance already has users, set OPENPR_DEMO_EMAIL and OPENPR_DEMO_PASSWORD to an existing account. Last register message: ${register.payload?.message ?? '<none>'}; login message: ${login.payload?.message ?? '<none>'}`,
  );
}

async function ensureWorkspace(token) {
  const workspaces = await request('GET', '/api/v1/workspaces', undefined, token);
  const existing = workspaces.items.find((item) => item.slug === workspaceSlug);
  if (existing) return existing;
  return request('POST', '/api/v1/workspaces', {
    slug: workspaceSlug,
    name: workspaceName,
  }, token);
}

async function ensureProject(token, workspaceId) {
  const projects = await request('GET', `/api/v1/workspaces/${workspaceId}/projects`, undefined, token);
  const existing = projects.items.find((item) => item.key === projectKey);
  if (existing) return existing;
  return request('POST', `/api/v1/workspaces/${workspaceId}/projects`, {
    key: projectKey,
    name: projectName,
    description: 'Local demo project created from restaurant_ordering_default.',
    scenario_template_key: 'restaurant_ordering_default',
  }, token);
}

async function tokenCanAccessWorkspace(botToken, workspaceId) {
  if (!botToken || !botToken.startsWith('opr_')) return false;
  const result = await rawRequest('GET', `/api/v1/workspaces/${workspaceId}/projects`, undefined, botToken);
  return result.response.ok && result.payload?.code === 0;
}

async function ensureMcpBot(token, workspaceId) {
  const envValues = readEnvFile();
  const envToken = envValues.OPENPR_BOT_TOKEN;
  const envWorkspaceId = envValues.OPENPR_WORKSPACE_ID;

  if (envWorkspaceId === workspaceId && await tokenCanAccessWorkspace(envToken, workspaceId)) {
    return {
      token: envToken,
      token_source: 'existing_env',
      env_written: false,
      mcp_ready: true,
    };
  }

  const bot = await request('POST', `/api/v1/workspaces/${workspaceId}/bots`, {
    name: 'Local Restaurant Demo MCP Bot',
    permissions: ['read', 'write', 'admin'],
  }, token);

  assert(await tokenCanAccessWorkspace(bot.token, workspaceId), 'created MCP bot token cannot access demo workspace');
  const envWritten = upsertEnvFile({
    OPENPR_BOT_TOKEN: bot.token,
    OPENPR_WORKSPACE_ID: workspaceId,
  });

  return {
    token: bot.token,
    token_source: 'created',
    token_prefix: bot.token_prefix,
    env_written: envWritten,
    mcp_ready: true,
  };
}

async function createRecord(token, formId, title, values) {
  return request('POST', `/api/v1/forms/${formId}/records`, {
    title,
    values,
    source: { type: 'local_demo' },
  }, token);
}

async function main() {
  const token = await authenticate();
  const workspace = await ensureWorkspace(token);
  const mcp = await ensureMcpBot(token, workspace.id);
  const project = await ensureProject(token, workspace.id);
  const forms = (await request('GET', `/api/v1/projects/${project.id}/forms?per_page=100`, undefined, token)).items;

  const menuCategoryForm = formByKey(forms, 'menu_category');
  const skuForm = formByKey(forms, 'sku');
  const tableForm = formByKey(forms, 'table');
  const orderForm = formByKey(forms, 'order');
  const orderLineForm = formByKey(forms, 'order_line');
  const reportForm = formByKey(forms, 'business_report');

  const suffix = Date.now().toString().slice(-6);
  const category = await createRecord(token, menuCategoryForm.id, `Noodles ${suffix}`, {
    name: `Noodles ${suffix}`,
    sort_order: '1',
    status: 'active',
  });
  const sku = await createRecord(token, skuForm.id, `Beef Noodles ${suffix}`, {
    sku: `SKU-BEEF-${suffix}`,
    name: `Beef Noodles ${suffix}`,
    category_id: { record_id: category.id },
    price: '9.99',
    kitchen_station: 'hot',
    status: 'available',
  });
  const table = await createRecord(token, tableForm.id, `A-${suffix}`, {
    table_no: `A-${suffix}`,
    seat_count: '4',
    status: 'occupied',
  });
  const order = await createRecord(token, orderForm.id, `ORD-${suffix}`, {
    order_no: `ORD-${suffix}`,
    table_id: { record_id: table.id },
    status: 'sent_to_kitchen',
    total_amount: '19.98',
    opened_at: '2026-05-31T12:00',
  });
  const orderLine = await createRecord(token, orderLineForm.id, `Beef Noodles x2 ${suffix}`, {
    order_id: { record_id: order.id },
    sku_id: { record_id: sku.id },
    sku_name: `Beef Noodles ${suffix}`,
    quantity: '2',
    unit_price: '9.99',
    seat_no: '1',
    status: 'sent_to_kitchen',
  });
  assert(orderLine.values.line_total?.decimal === '19.98', 'restaurant_calc should calculate order_line.line_total = 19.98');

  await request('POST', `/api/v1/form-records/${order.id}/links`, {
    target_type: 'form_record',
    target_id: orderLine.id,
    relation_key: 'order_lines',
    relation_type: 'parent_child',
    metadata: { source: 'local_demo' },
  }, token);

  const report = await createRecord(token, reportForm.id, `Demo report ${suffix}`, {
    report_date: '2026-05-31',
    gross_revenue: '19.98',
    order_count: '1',
    item_count: '2',
    printed_receipts: '0',
  });
  assert(report.values.gross_revenue?.decimal === '19.98', 'business_report.gross_revenue should be decimal 19.98');

  const formsUrl = `/workspace/${workspace.id}/projects/${project.id}/forms`;
  const result = {
    api_url: apiUrl,
    login_email: email,
    workspace_id: workspace.id,
    workspace_slug: workspace.slug,
    project_id: project.id,
    project_key: project.key,
    forms_url: formsUrl,
    sample_order: `ORD-${suffix}`,
    sample_total: '19.98',
    mcp_workspace_id: workspace.id,
    mcp_token_source: mcp.token_source,
    mcp_token_prefix: mcp.token.slice(0, 8),
    mcp_env_written: mcp.env_written,
    mcp_ready: mcp.mcp_ready,
  };
  if (stateFile) {
    fs.writeFileSync(stateFile, JSON.stringify(result, null, 2), { mode: 0o600 });
  }
  console.log(JSON.stringify(result, null, 2));
  console.log(`restaurant demo ready: http://localhost:3000${formsUrl}`);
  if (mcp.env_written) {
    console.log('MCP demo credentials written to .env. Recreate mcp-server to load them if it is already running.');
  }
}

main().catch((error) => {
  console.error(error.message);
  process.exit(1);
});
NODE

if [[ "${OPENPR_DEMO_RESTART_MCP:-1}" == "1" ]] && [[ "${OPENPR_DEMO_WRITE_ENV:-auto}" != "0" ]]; then
  if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
    if docker compose ps --services --filter status=running 2>/dev/null | grep -qx 'mcp-server'; then
      echo "Recreating mcp-server so it can load demo MCP credentials from .env..."
      docker compose up -d --no-deps --force-recreate mcp-server
    fi
  fi
fi

OPENPR_DEMO_STATE_FILE="$DEMO_STATE_FILE" \
OPENPR_DEMO_VERIFY_MCP_HTTP="${OPENPR_DEMO_VERIFY_MCP_HTTP:-auto}" \
OPENPR_DEMO_MCP_URL="${OPENPR_DEMO_MCP_URL:-http://localhost:8090/mcp/rpc}" \
OPENPR_DEMO_ENV_PATH="$DEMO_ENV_PATH" \
OPENPR_MCP_AUTH_TOKEN="$MCP_AUTH_TOKEN" \
node --input-type=commonjs <<'NODE'
const fs = require('fs');

const verifyMode = process.env.OPENPR_DEMO_VERIFY_MCP_HTTP ?? 'auto';
const mcpUrl = process.env.OPENPR_DEMO_MCP_URL ?? 'http://localhost:8090/mcp/rpc';
const stateFile = process.env.OPENPR_DEMO_STATE_FILE;
const envPath = process.env.OPENPR_DEMO_ENV_PATH ?? '.env';
const mcpAuthToken = process.env.OPENPR_MCP_AUTH_TOKEN ?? '';

if (verifyMode === '0' || verifyMode === 'false') {
  console.log('MCP HTTP verification skipped: OPENPR_DEMO_VERIFY_MCP_HTTP=0');
  process.exit(0);
}

if (!stateFile || !fs.existsSync(stateFile)) {
  if (verifyMode === '1') {
    throw new Error('MCP HTTP verification requires the restaurant demo state file');
  }
  console.log('MCP HTTP verification skipped: restaurant demo state is unavailable');
  process.exit(0);
}

const state = JSON.parse(fs.readFileSync(stateFile, 'utf8'));

function healthUrlFromRpcUrl(value) {
  const url = new URL(value);
  if (url.pathname.endsWith('/mcp/rpc')) {
    url.pathname = `${url.pathname.slice(0, -'/mcp/rpc'.length)}/health`;
  } else {
    url.pathname = '/health';
  }
  url.search = '';
  url.hash = '';
  return url.toString();
}

async function fetchWithTimeout(url, options = {}, timeoutMs = 2500) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetch(url, { ...options, signal: controller.signal });
  } finally {
    clearTimeout(timeout);
  }
}

async function waitForHealth(healthUrl) {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    try {
      const response = await fetchWithTimeout(healthUrl, {}, 1500);
      if (response.ok) return true;
    } catch {
      // Keep polling while compose is recreating the local MCP server.
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  return false;
}

function extractTextContent(payload) {
  const content = payload?.result?.content;
  if (!Array.isArray(content)) return '';
  return content
    .filter((item) => item?.type === 'text' && typeof item.text === 'string')
    .map((item) => item.text)
    .join('\n');
}

async function verify() {
  if (!mcpAuthToken) {
    const message = 'MCP HTTP verification needs OPENPR_MCP_AUTH_TOKEN: /mcp/rpc rejects requests without an Authorization: Bearer header';
    if (verifyMode === '1') throw new Error(message);
    console.log(`${message}. Export OPENPR_MCP_AUTH_TOKEN or set it in ${envPath} (bash scripts/start.sh generates one).`);
    return;
  }

  const healthUrl = healthUrlFromRpcUrl(mcpUrl);
  const reachable = await waitForHealth(healthUrl);
  if (!reachable) {
    const message = `MCP HTTP verification skipped: ${healthUrl} is not reachable`;
    if (verifyMode === '1') throw new Error(message);
    console.log(`${message}. Start mcp-server or set OPENPR_DEMO_VERIFY_MCP_HTTP=1 to require it.`);
    return;
  }

  const response = await fetchWithTimeout(mcpUrl, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${mcpAuthToken}`,
    },
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'tools/call',
      params: { name: 'projects.list', arguments: {} },
    }),
  }, 5000);
  if (response.status === 401 || response.status === 403) {
    throw new Error(`MCP HTTP verification was rejected with ${response.status}: OPENPR_MCP_AUTH_TOKEN does not match the token the MCP server runs with. Recreate mcp-server after changing .env.`);
  }
  const payload = await response.json();
  if (!response.ok || payload.error || payload?.result?.is_error === true) {
    throw new Error(`MCP HTTP projects.list failed: ${JSON.stringify(payload)}`);
  }

  const text = extractTextContent(payload);
  if (!text.includes(state.project_key) || !text.includes(state.project_id)) {
    throw new Error(`MCP HTTP projects.list did not include demo project ${state.project_key}`);
  }

  console.log(`MCP HTTP verification passed: projects.list includes ${state.project_key} via ${mcpUrl}`);
}

verify().catch((error) => {
  console.error(error.message);
  process.exit(1);
});
NODE
