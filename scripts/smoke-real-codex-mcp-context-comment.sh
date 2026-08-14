#!/usr/bin/env bash
set -euo pipefail

if [[ "${OPENPR_REAL_CODEX_SMOKE:-}" != "1" ]]; then
  echo "Set OPENPR_REAL_CODEX_SMOKE=1 to run this smoke; it invokes a real Codex model." >&2
  exit 2
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEBHOOK_DIR="${OPENPR_WEBHOOK_DIR:-/opt/worker/code/openpr-webhook}"
CODEX_ENTRYPOINT="${CODEX_ENTRYPOINT:-/home/ck/.nvm/versions/node/v22.22.1/lib/node_modules/@openai/codex/bin/codex.js}"
POSTGRES_PORT="${OPENPR_SMOKE_PG_PORT:-5366}"
PG_SUPERUSER="${OPENPR_SMOKE_PG_SUPERUSER:-postgres}"
DB_NAME="openpr_real_codex_smoke_$$_$(date +%s)"
DB_USER="openpr_real_codex_smoke_$$_$(date +%s)"
DB_PASSWORD="$(openssl rand -hex 12)"
API_PORT="${OPENPR_SMOKE_API_PORT:-$((18081 + ($$ % 1000)))}"
WEBHOOK_PORT="${OPENPR_SMOKE_WEBHOOK_PORT:-$((19091 + ($$ % 1000)))}"
TMP_DIR="$(mktemp -d /tmp/openpr-real-codex-smoke.XXXXXX)"
API_LOG="$TMP_DIR/api.log"
WEBHOOK_LOG="$TMP_DIR/webhook.log"
MCP_CONFIG="$TMP_DIR/openpr.mcp.toml"
BOT_TOKEN="opr_real_codex_smoke_token"
COMMENT_MARKER="REAL_CODEX_MCP_COMMENT_OK"

OWNER_ID="11111111-1111-4111-8111-111111111111"
BOT_ID="22222222-2222-4222-8222-222222222222"
WORKSPACE_ID="33333333-3333-4333-8333-333333333333"
PROJECT_ID="44444444-4444-4444-8444-444444444444"
WORK_ITEM_ID="55555555-5555-4555-8555-555555555555"
TASK_ID="66666666-6666-4666-8666-666666666666"
INVOCATION_ID="77777777-7777-4777-8777-777777777777"

api_pid=""
webhook_pid=""

cleanup() {
  local exit_code=$?
  if [[ -n "$webhook_pid" ]] && kill -0 "$webhook_pid" 2>/dev/null; then
    kill "$webhook_pid" 2>/dev/null || true
    wait "$webhook_pid" 2>/dev/null || true
  fi
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
    echo "Smoke failed. Logs are in $TMP_DIR" >&2
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

require_cmd curl
require_cmd jq
require_cmd node
require_cmd openssl
require_cmd sha256sum
require_cmd sudo

if [[ ! -f "$CODEX_ENTRYPOINT" ]]; then
  echo "Codex entrypoint not found: $CODEX_ENTRYPOINT" >&2
  exit 2
fi
if [[ ! -f /home/ck/.codex/auth.json ]]; then
  echo "Codex auth file not found: /home/ck/.codex/auth.json" >&2
  exit 2
fi

cargo build -q -p api --bin api -p mcp-server --bin mcp-server
(cd "$WEBHOOK_DIR" && cargo build -q)

sudo -n -u "$PG_SUPERUSER" psql -p "$POSTGRES_PORT" -d postgres -v ON_ERROR_STOP=1 -q <<SQL
CREATE ROLE "$DB_USER" LOGIN PASSWORD '$DB_PASSWORD';
CREATE DATABASE "$DB_NAME" OWNER "$DB_USER";
SQL

SMOKE_DATABASE_URL="postgres://$DB_USER:$DB_PASSWORD@127.0.0.1:$POSTGRES_PORT/$DB_NAME"
SMOKE_JWT_SECRET="openpr-real-codex-smoke-secret"

# The api reads no environment variables; this file is its only configuration, and it refuses to
# start without one. It is written inside the 0700 directory mktemp made for this run and is
# removed with it, so the generated database password never lands in the repository.
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

BOT_TOKEN_HASH="$(printf %s "$BOT_TOKEN" | sha256sum | awk '{print $1}')"
sudo -n -u "$PG_SUPERUSER" psql -p "$POSTGRES_PORT" -d "$DB_NAME" -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, name, password_hash, created_at, updated_at)
VALUES
  ('$OWNER_ID', 'owner-real-codex-smoke@example.local', 'Smoke Owner', '', now(), now()),
  ('$BOT_ID', 'bot-real-codex-smoke@example.local', 'OpenPR Real Codex', '', now(), now());

INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE_ID', 'real-codex-smoke', 'Real Codex MCP Smoke', '$OWNER_ID', now(), now());

INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES
  ('$WORKSPACE_ID', '$OWNER_ID', 'owner', now()),
  ('$WORKSPACE_ID', '$BOT_ID', 'member', now());

INSERT INTO workspace_bots (
  id, workspace_id, name, token_hash, token_prefix, permissions, created_by, is_active, created_at, updated_at
)
VALUES (
  '$BOT_ID',
  '$WORKSPACE_ID',
  'OpenPR Real Codex',
  '$BOT_TOKEN_HASH',
  'opr_real',
  '["read","write","admin"]'::jsonb,
  '$OWNER_ID',
  true,
  now(),
  now()
);

INSERT INTO projects (id, workspace_id, key, name, description, created_by, type_key, type_settings, created_at, updated_at)
VALUES (
  '$PROJECT_ID',
  '$WORKSPACE_ID',
  'RCX',
  'Real Codex MCP Project',
  'Smoke project proving real AI executor through OpenPR MCP.',
  '$OWNER_ID',
  'code_project',
  '{"smoke":"real_codex_mcp"}'::jsonb,
  now(),
  now()
);

INSERT INTO work_items (id, project_id, title, description, state, priority, created_by, created_at, updated_at)
VALUES (
  '$WORK_ITEM_ID',
  '$PROJECT_ID',
  'Real Codex MCP workflow',
  'Use MCP context, read the work item, write a comment, and complete the invocation.',
  'todo',
  'medium',
  '$OWNER_ID',
  now(),
  now()
);

INSERT INTO ai_tasks (
  id, project_id, ai_participant_id, task_type, reference_type, reference_id, status, priority, payload, created_at, updated_at
)
VALUES (
  '$TASK_ID',
  '$PROJECT_ID',
  '$BOT_ID',
  'issue_assigned',
  'work_item',
  '$WORK_ITEM_ID',
  'pending',
  10,
  jsonb_build_object('smoke', 'real_codex_mcp'),
  now(),
  now()
);

INSERT INTO agent_invocations (
  id, workspace_id, project_id, actor_id, target_agent_id, source_task_id,
  trigger_kind, trigger_ref_type, trigger_ref_id, connector_kind, status, payload, audit_chain_id,
  created_at, updated_at
)
VALUES (
  '$INVOCATION_ID',
  '$WORKSPACE_ID',
  '$PROJECT_ID',
  '$OWNER_ID',
  '$BOT_ID',
  '$TASK_ID',
  'assigned',
  'work_item',
  '$WORK_ITEM_ID',
  'cli',
  'pending',
  jsonb_build_object('smoke', 'real_codex_mcp'),
  '$INVOCATION_ID',
  now(),
  now()
);
SQL

mkdir -p "$TMP_DIR/bin" "$TMP_DIR/codex-home"
ln -s /home/ck/.codex/auth.json "$TMP_DIR/codex-home/auth.json"
cat >"$TMP_DIR/bin/codex" <<EOF
#!/usr/bin/env bash
exec node "$CODEX_ENTRYPOINT" "\$@"
EOF
chmod +x "$TMP_DIR/bin/codex"

cat >"$TMP_DIR/codex-home/config.toml" <<EOF
model = "gpt-5.5"
model_reasoning_effort = "low"

[projects."/tmp"]
trust_level = "trusted"

[mcp_servers.openpr]
command = "$ROOT_DIR/target/debug/mcp-server"
args = ["serve", "--transport", "stdio", "--config", "$MCP_CONFIG"]
EOF

# The MCP server reads no environment variables, so codex cannot hand it an identity through
# [mcp_servers.openpr.env]; it gets a configuration file instead. invocation_id is what stamps
# every tool call codex makes onto this run's invocation ledger row, and the transport label the
# API sees follows from mcp.transport rather than from a variable naming it.
cat >"$MCP_CONFIG" <<EOF
[logging]
filter = "error"
format = "text"

[mcp]
api_url = "http://127.0.0.1:$API_PORT"
bot_token = "$BOT_TOKEN"
workspace_id = "$WORKSPACE_ID"
invocation_id = "$INVOCATION_ID"
transport = "stdio"
EOF

cat >"$TMP_DIR/webhook-config.toml" <<EOF
[server]
listen = "127.0.0.1:$WEBHOOK_PORT"

[security]
allow_unsigned = true

[features]
cli_enabled = true
callback_enabled = false

[runtime]
http_timeout_secs = 10
cli_max_concurrency = 1

[[agents]]
id = "openpr-codex-real"
name = "OpenPR Real Codex"
agent_type = "cli"

[agents.cli]
executor = "codex"
workdir = "/tmp"
timeout_secs = 360
max_output_chars = 24000
skip_callback_state = true
prompt_template = "Run the OpenPR MCP persistence validation for issue {issue_id}: {title}"
mcp_instructions = """You are validating a live OpenPR MCP workflow.
Use the OpenPR MCP server tools. Do not edit files. Do not run shell commands.

Required MCP tool calls, in this exact order:
1. context.get_project with project_id="{project_id}".
2. work_items.get with work_item_id="{issue_id}".
3. comments.create with work_item_id="{issue_id}" and content exactly "$COMMENT_MARKER".
4. invocations.complete with the invocation_id from the Invocation Ledger and result {"status":"success","summary":"real Codex used OpenPR MCP context and wrote comment"}.

After all four MCP calls succeed, reply exactly REAL_CODEX_MCP_DONE.
"""

[agents.cli.env_vars]
PATH = "$TMP_DIR/bin:$PATH"
CODEX_HOME = "$TMP_DIR/codex-home"
CODEX_DISABLE_UPDATE_CHECK = "1"
EOF

RUST_LOG="${RUST_LOG:-openpr_webhook=info}" \
"$WEBHOOK_DIR/target/debug/openpr-webhook" "$TMP_DIR/webhook-config.toml" >"$WEBHOOK_LOG" 2>&1 &
webhook_pid=$!
wait_http "http://127.0.0.1:$WEBHOOK_PORT/health" "openpr-webhook"

payload="$(jq -n \
  --arg task_id "$TASK_ID" \
  --arg bot_id "$BOT_ID" \
  --arg project_id "$PROJECT_ID" \
  --arg work_item_id "$WORK_ITEM_ID" \
  --arg invocation_id "$INVOCATION_ID" \
  '{
    event: "ai_task.assigned",
    task_id: $task_id,
    ai_participant_id: $bot_id,
    ai_participant_agent_type: "cli",
    task_type: "issue_assigned",
    reference_type: "work_item",
    reference_id: $work_item_id,
    project_id: $project_id,
    invocation_id: $invocation_id,
    payload: {
      issue_title: "Real Codex MCP workflow",
      trigger: "issue.assigned",
      project_id: $project_id,
      invocation_id: $invocation_id
    }
  }')"

curl -fsS \
  -H "Content-Type: application/json" \
  --data "$payload" \
  "http://127.0.0.1:$WEBHOOK_PORT/webhook" >"$TMP_DIR/webhook-response.json"

verification=""
for _ in $(seq 1 420); do
  verification="$(
  sudo -n -u "$PG_SUPERUSER" psql -p "$POSTGRES_PORT" -d "$DB_NAME" -At -v ON_ERROR_STOP=1 <<SQL
SELECT concat_ws('|',
  COALESCE((SELECT status FROM agent_invocations WHERE id = '$INVOCATION_ID'), 'missing_invocation'),
  COALESCE((SELECT status FROM ai_tasks WHERE id = '$TASK_ID'), 'missing_task'),
  (SELECT COUNT(*) FROM comments WHERE work_item_id = '$WORK_ITEM_ID' AND body = '$COMMENT_MARKER'),
  COALESCE((SELECT author_id::text FROM comments WHERE work_item_id = '$WORK_ITEM_ID' AND body = '$COMMENT_MARKER' ORDER BY created_at DESC LIMIT 1), 'missing_author'),
  COALESCE((SELECT result->>'status' FROM agent_invocations WHERE id = '$INVOCATION_ID'), 'missing_result_status'),
  COALESCE((SELECT result->>'summary' FROM agent_invocations WHERE id = '$INVOCATION_ID'), 'missing_result_summary'),
  (SELECT COUNT(DISTINCT tool_name)
   FROM agent_invocation_tool_calls
   WHERE invocation_id = '$INVOCATION_ID'
     AND tool_name IN ('context.get_project', 'work_items.get', 'comments.create', 'invocations.complete')),
  (SELECT COUNT(*)
   FROM agent_invocation_tool_calls
   WHERE invocation_id = '$INVOCATION_ID'
     AND status = 'failed')
);
SQL
  )"
  if [[ "$verification" == completed\|completed\|1\|* ]]; then
    break
  fi
  if [[ "$verification" == failed\|* || "$verification" == cancelled\|* ]]; then
    break
  fi
  sleep 1
done

expected="completed|completed|1|$BOT_ID|success|real Codex used OpenPR MCP context and wrote comment|4|0"
if [[ "$verification" != "$expected" ]]; then
  echo "Unexpected DB verification:" >&2
  echo "  expected: $expected" >&2
  echo "  actual:   $verification" >&2
  echo "Webhook response:" >&2
  cat "$TMP_DIR/webhook-response.json" >&2 || true
  echo >&2
  exit 1
fi

printf '%s\n' "$verification"
