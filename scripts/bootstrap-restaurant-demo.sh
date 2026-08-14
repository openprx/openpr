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
  OPENPR_DEMO_WRITE_CONFIG=0    Do not write the demo bot credentials into the MCP
                                configuration file. Default: auto, write only when
                                that file already exists.
  OPENPR_DEMO_CONFIG_PATH       MCP configuration file carrying [mcp].
                                Default: repo config/openpr.compose.mcp.toml.
  OPENPR_DEMO_RESTART_MCP=0     Do not recreate running compose mcp-server.
                                Default: 1.
  OPENPR_DEMO_VERIFY_MCP_HTTP   Verify local MCP JSON-RPC after bootstrap.
                                Values: auto, 1, 0. Default: auto.
  OPENPR_DEMO_MCP_URL           MCP JSON-RPC URL.
                                Default: http://localhost:8090/mcp/rpc
  OPENPR_MCP_BOT_TOKEN          Workspace bot token the MCP JSON-RPC verification calls
                                as. Defaults to the demo bot this run created, which is
                                what makes the verification prove the demo bot works.

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

# The MCP server reads no environment variables; its settings live in a TOML file. Under compose
# that is the file docker-compose.yml mounts into the mcp-server container.
DEMO_CONFIG_PATH="${OPENPR_DEMO_CONFIG_PATH:-$ROOT_DIR/config/openpr.compose.mcp.toml}"

require_cmd python3

# Reads one dotted key out of the TOML configuration file, using the tomllib parser in python3
# (3.11+) rather than a grep that would mis-handle quoting and section scoping. Prints nothing
# when the file or the key is absent, so the caller decides whether that is fatal. The value goes
# to stdout only, never to the log.
read_config_value() {
  local key="$1"
  local file="$2"
  [ -f "$file" ] || return 0
  OPENPR_CONFIG_KEY="$key" OPENPR_CONFIG_PATH="$file" python3 -c '
import os
import sys
import tomllib

try:
    with open(os.environ["OPENPR_CONFIG_PATH"], "rb") as handle:
        data = tomllib.load(handle)
except (OSError, tomllib.TOMLDecodeError):
    raise SystemExit(0)

node = data
for part in os.environ["OPENPR_CONFIG_KEY"].split("."):
    if not isinstance(node, dict) or part not in node:
        raise SystemExit(0)
    node = node[part]
if isinstance(node, str):
    sys.stdout.write(node)
'
}

# The MCP verification below posts to /mcp/rpc, which is served as whoever called it: the request
# carries a workspace bot token in `Authorization: Bearer opr_...` and the MCP server forwards it
# to the API. There is no shared inbound secret. /health is exempt and stays unauthenticated.
# The default identity is the demo bot this run creates or reuses, so the verification proves the
# credentials it just wrote actually work; OPENPR_MCP_BOT_TOKEN overrides it for this script only.
MCP_CALLER_BOT_TOKEN="${OPENPR_MCP_BOT_TOKEN:-}"

# Credentials already in the file. When they still address the demo workspace the bootstrap keeps
# them instead of minting a second bot on every run.
EXISTING_BOT_TOKEN="$(read_config_value mcp.bot_token "$DEMO_CONFIG_PATH")"
EXISTING_WORKSPACE_ID="$(read_config_value mcp.workspace_id "$DEMO_CONFIG_PATH")"

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
OPENPR_DEMO_EXISTING_BOT_TOKEN="$EXISTING_BOT_TOKEN" \
OPENPR_DEMO_EXISTING_WORKSPACE_ID="$EXISTING_WORKSPACE_ID" \
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
// Handed in by the wrapper, which reads them out of the TOML configuration file. This block never
// touches that file: node has no TOML parser, and the wrapper writes any new credentials back
// through python3's tomllib so a hand-edited file keeps its comments and its structure.
const existingBotToken = process.env.OPENPR_DEMO_EXISTING_BOT_TOKEN ?? '';
const existingWorkspaceId = process.env.OPENPR_DEMO_EXISTING_WORKSPACE_ID ?? '';
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
  if (existingWorkspaceId === workspaceId && await tokenCanAccessWorkspace(existingBotToken, workspaceId)) {
    return {
      token: existingBotToken,
      token_source: 'existing_config',
      credentials_changed: false,
      mcp_ready: true,
    };
  }

  const bot = await request('POST', `/api/v1/workspaces/${workspaceId}/bots`, {
    name: 'Local Restaurant Demo MCP Bot',
    permissions: ['read', 'write', 'admin'],
  }, token);

  assert(await tokenCanAccessWorkspace(bot.token, workspaceId), 'created MCP bot token cannot access demo workspace');

  return {
    token: bot.token,
    token_source: 'created',
    token_prefix: bot.token_prefix,
    credentials_changed: true,
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
    mcp_credentials_changed: mcp.credentials_changed,
    mcp_ready: mcp.mcp_ready,
  };
  if (stateFile) {
    // The bot token goes to the state file only, so the wrapper can write it into the MCP
    // configuration file. That file is created with mode 0600 and removed when this script exits;
    // `result`, which is printed below, carries only the token's first 8 characters.
    fs.writeFileSync(
      stateFile,
      JSON.stringify({ ...result, mcp_bot_token: mcp.token }, null, 2),
      { mode: 0o600 },
    );
  }
  console.log(JSON.stringify(result, null, 2));
  console.log(`restaurant demo ready: http://localhost:3000${formsUrl}`);
}

main().catch((error) => {
  console.error(error.message);
  process.exit(1);
});
NODE

# Write the bot credentials the run just created into [mcp] of the configuration file. Done here
# rather than in node: this rewrites only the two values and leaves every comment and every other
# key in place, and it verifies the result by parsing it back.
WRITE_CONFIG_MODE="${OPENPR_DEMO_WRITE_CONFIG:-auto}"
CONFIG_WRITTEN=0
if [[ "$WRITE_CONFIG_MODE" != "0" ]]; then
  if OPENPR_DEMO_CONFIG_PATH="$DEMO_CONFIG_PATH" \
     OPENPR_DEMO_STATE_FILE="$DEMO_STATE_FILE" \
     OPENPR_DEMO_WRITE_CONFIG="$WRITE_CONFIG_MODE" \
     python3 -c '
import json
import stat
import os
import re
import sys
import tomllib

config_path = os.environ["OPENPR_DEMO_CONFIG_PATH"]
mode = os.environ["OPENPR_DEMO_WRITE_CONFIG"]

# "auto" only touches a file that is already there; "1" also creates a missing one.
if not os.path.exists(config_path) and mode != "1":
    raise SystemExit(2)

with open(os.environ["OPENPR_DEMO_STATE_FILE"], "r", encoding="utf-8") as handle:
    state = json.load(handle)
if not state.get("mcp_credentials_changed"):
    raise SystemExit(2)

updates = {"bot_token": state["mcp_bot_token"], "workspace_id": state["mcp_workspace_id"]}


def toml_string(value):
    escaped = value.replace("\\", "\\\\").replace(chr(34), "\\" + chr(34))
    return chr(34) + escaped + chr(34)


try:
    with open(config_path, "r", encoding="utf-8") as handle:
        lines = handle.read().splitlines()
except FileNotFoundError:
    lines = ["[mcp]"]

# Locate [mcp] and the line after its last entry, so a key that is absent is appended inside the
# section rather than under whatever table happens to come next.
start = None
end = len(lines)
for index, line in enumerate(lines):
    stripped = line.strip()
    if start is None:
        if stripped == "[mcp]":
            start = index
    elif stripped.startswith("[") and not stripped.startswith("[["):
        end = index
        break
if start is None:
    lines.append("[mcp]")
    start, end = len(lines) - 1, len(lines)

for key, value in updates.items():
    pattern = re.compile(r"^\s*#?\s*" + re.escape(key) + r"\s*=")
    for index in range(start + 1, end):
        if pattern.match(lines[index]):
            lines[index] = f"{key} = {toml_string(value)}"
            break
    else:
        insert_at = end
        while insert_at > start + 1 and not lines[insert_at - 1].strip():
            insert_at -= 1
        lines.insert(insert_at, f"{key} = {toml_string(value)}")
        end += 1

rendered = "\n".join(lines) + "\n"

# Parse before replacing the file: a write that produced something the MCP server cannot read
# would take the stack down at the next restart.
parsed = tomllib.loads(rendered).get("mcp", {})
for key, value in updates.items():
    if parsed.get(key) != value:
        print(f"rewriting {key} in {config_path} did not round-trip", file=sys.stderr)
        raise SystemExit(1)

# Write in place rather than rename over the path. compose bind-mounts this file into the
# mcp-server container, and a bind mount follows the inode it was created with: an atomic
# rename swaps in a new inode, so the container would keep serving the old contents through
# restarts while the host shows the new ones. Overwriting keeps the inode, and with it the
# mode the container needs to read the file at all. The rendered text was already parsed and
# round-tripped above, so the window where a crash could leave a truncated file is small and
# recoverable by rerunning this script.
with open(config_path, "w", encoding="utf-8") as handle:
    handle.write(rendered)
'; then
    CONFIG_WRITTEN=1
    echo "Demo MCP bot credentials written to $DEMO_CONFIG_PATH (values not printed)."
  else
    status=$?
    # 2 means "nothing to do": credentials unchanged, or the file is absent and mode is auto.
    if [[ "$status" != "2" ]]; then
      echo "Failed to write demo MCP credentials to $DEMO_CONFIG_PATH" >&2
      exit "$status"
    fi
  fi
fi

if [[ "${OPENPR_DEMO_RESTART_MCP:-1}" == "1" ]] && [[ "$CONFIG_WRITTEN" == "1" ]]; then
  if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
    if docker compose ps --services --filter status=running 2>/dev/null | grep -qx 'mcp-server'; then
      echo "Recreating mcp-server so it reloads the demo MCP credentials from its configuration file..."
      docker compose up -d --no-deps --force-recreate mcp-server
    fi
  fi
fi

OPENPR_DEMO_STATE_FILE="$DEMO_STATE_FILE" \
OPENPR_DEMO_VERIFY_MCP_HTTP="${OPENPR_DEMO_VERIFY_MCP_HTTP:-auto}" \
OPENPR_DEMO_MCP_URL="${OPENPR_DEMO_MCP_URL:-http://localhost:8090/mcp/rpc}" \
OPENPR_DEMO_CONFIG_PATH="$DEMO_CONFIG_PATH" \
OPENPR_MCP_BOT_TOKEN="$MCP_CALLER_BOT_TOKEN" \
node --input-type=commonjs <<'NODE'
const fs = require('fs');

const verifyMode = process.env.OPENPR_DEMO_VERIFY_MCP_HTTP ?? 'auto';
const mcpUrl = process.env.OPENPR_DEMO_MCP_URL ?? 'http://localhost:8090/mcp/rpc';
const stateFile = process.env.OPENPR_DEMO_STATE_FILE;
const configPath = process.env.OPENPR_DEMO_CONFIG_PATH ?? 'config/openpr.compose.mcp.toml';
const overrideBotToken = process.env.OPENPR_MCP_BOT_TOKEN ?? '';

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
  // The demo bot this run created or reused. It is the caller identity the verification asserts
  // with, so a 401 here means the credentials just written into the MCP configuration file are
  // not usable — which is the failure this whole check exists to catch.
  const callerBotToken = overrideBotToken || state.mcp_bot_token || '';
  if (!callerBotToken) {
    const message = 'MCP HTTP verification has no caller bot token: /mcp/rpc rejects a request without an Authorization: Bearer opr_... header';
    if (verifyMode === '1') throw new Error(message);
    console.log(`${message}. The demo bootstrap normally supplies the demo bot it created; export OPENPR_MCP_BOT_TOKEN to name another one.`);
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
      Authorization: `Bearer ${callerBotToken}`,
    },
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'tools/call',
      params: { name: 'projects.list', arguments: {} },
    }),
  }, 5000);
  if (response.status === 401 || response.status === 403) {
    throw new Error(`MCP HTTP verification was rejected with ${response.status}: the API refused the demo bot token this run presented. The MCP server forwards the caller token unchanged, so check that the bot still exists in the demo workspace named by mcp.workspace_id in ${configPath}.`);
  }
  const payload = await response.json();
  // The wire field is isError; is_error is accepted too so the check survives a serialisation
  // change rather than quietly passing every failed call.
  if (!response.ok || payload.error || payload?.result?.isError === true || payload?.result?.is_error === true) {
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
