#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEBHOOK_DIR="${OPENPR_WEBHOOK_DIR:-/opt/worker/code/openpr-webhook}"
POSTGRES_PORT="${OPENPR_SMOKE_PG_PORT:-5366}"
PG_SUPERUSER="${OPENPR_SMOKE_PG_SUPERUSER:-postgres}"
DB_NAME="openpr_mention_smoke_$$_$(date +%s)"
DB_USER="openpr_mention_smoke_$$_$(date +%s)"
DB_PASSWORD="$(openssl rand -hex 12)"
API_PORT="${OPENPR_SMOKE_API_PORT:-$((18180 + ($$ % 1000)))}"
WEBHOOK_PORT="${OPENPR_SMOKE_WEBHOOK_PORT:-$((19180 + ($$ % 1000)))}"
BROWSER_PORT="${OPENPR_SMOKE_BROWSER_PORT:-$((20180 + ($$ % 1000)))}"
TMP_DIR="$(mktemp -d /tmp/openpr-browser-mention-smoke.XXXXXX)"
API_LOG="$TMP_DIR/api.log"
WORKER_LOG="$TMP_DIR/worker.log"
WEBHOOK_LOG="$TMP_DIR/webhook.log"
BROWSER_LOG="$TMP_DIR/browser.log"
BROWSER_DOM="$TMP_DIR/browser.dom"
SMOKE_JWT_SECRET="openpr-browser-mention-smoke-secret"

OWNER_ID="11111111-1111-4111-8111-111111111111"
BOT_ID="22222222-2222-4222-8222-222222222222"
WORKSPACE_ID="33333333-3333-4333-8333-333333333333"
PROJECT_ID="44444444-4444-4444-8444-444444444444"
WORK_ITEM_ID="55555555-5555-4555-8555-555555555555"
WEBHOOK_ID="66666666-6666-4666-8666-666666666666"

api_pid=""
worker_pid=""
webhook_pid=""
browser_server_pid=""

cleanup() {
  local exit_code=$?
  for pid in "$browser_server_pid" "$worker_pid" "$webhook_pid" "$api_pid"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
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

psql_smoke() {
  PGPASSWORD="$DB_PASSWORD" psql -h 127.0.0.1 -p "$POSTGRES_PORT" -U "$DB_USER" -d "$DB_NAME" "$@"
}

require_cmd curl
require_cmd node
require_cmd openssl
require_cmd psql
require_cmd sudo

CHROMIUM_BIN="${CHROMIUM_BIN:-/usr/bin/chromium}"
if [[ ! -x "$CHROMIUM_BIN" ]]; then
  echo "Chromium binary not found: $CHROMIUM_BIN" >&2
  exit 2
fi

cargo build -q -p api --bin api -p worker --bin worker
(cd "$WEBHOOK_DIR" && cargo build -q)

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

psql_smoke -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, name, password_hash, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES
  ('$OWNER_ID', 'owner-browser-mention@example.local', 'Smoke Owner', '', 'admin', true, 'human', NULL, now(), now()),
  ('$BOT_ID', 'bot-browser-mention@example.local', 'Document review assistant', '', 'user', true, 'bot', 'webhook', now(), now());

INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE_ID', 'browser-mention-smoke', 'Browser Mention Smoke', '$OWNER_ID', now(), now());

INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES
  ('$WORKSPACE_ID', '$OWNER_ID', 'owner', now()),
  ('$WORKSPACE_ID', '$BOT_ID', 'member', now());

INSERT INTO projects (id, workspace_id, key, name, description, created_by, type_key, type_settings, created_at, updated_at)
VALUES (
  '$PROJECT_ID',
  '$WORKSPACE_ID',
  'LEGAL',
  'Browser Mention Contract Review',
  'Smoke project for browser-originated @mention.',
  '$OWNER_ID',
  'contract_review',
  '{"scenario_template_key":"contract_review_default"}'::jsonb,
  now(),
  now()
);

INSERT INTO work_items (id, project_id, title, description, state, priority, created_by, created_at, updated_at)
VALUES (
  '$WORK_ITEM_ID',
  '$PROJECT_ID',
  'Review supplier framework agreement',
  'Browser smoke work item.',
  'intake',
  'medium',
  '$OWNER_ID',
  now(),
  now()
);

INSERT INTO webhooks (id, workspace_id, name, url, secret, events, active, created_by, bot_user_id, metadata, created_at, updated_at)
VALUES (
  '$WEBHOOK_ID',
  '$WORKSPACE_ID',
  'Document review assistant webhook',
  'http://127.0.0.1:$WEBHOOK_PORT/webhook',
  'smoke-secret',
  '["comment.created"]'::jsonb,
  true,
  '$OWNER_ID',
  '$BOT_ID',
  '{"source":"browser_mention_smoke"}'::jsonb,
  now(),
  now()
);
SQL

ACCESS_TOKEN="$(OWNER_ID="$OWNER_ID" SMOKE_JWT_SECRET="$SMOKE_JWT_SECRET" node <<'NODE'
const crypto = require('crypto');
const secret = process.env.SMOKE_JWT_SECRET;
const now = Math.floor(Date.now() / 1000);
const payload = {
  sub: process.env.OWNER_ID,
  email: 'owner-browser-mention@example.local',
  token_type: 'access',
  iat: now,
  exp: now + 3600,
};
function b64url(value) {
  return Buffer.from(JSON.stringify(value)).toString('base64url');
}
const header = b64url({ alg: 'HS256', typ: 'JWT' });
const body = b64url(payload);
const sig = crypto.createHmac('sha256', secret).update(`${header}.${body}`).digest('base64url');
console.log(`${header}.${body}.${sig}`);
NODE
)"

cat >"$TMP_DIR/openpr-webhook.toml" <<EOF
[server]
listen = "127.0.0.1:$WEBHOOK_PORT"

[security]
allow_unsigned = true

[[agents]]
id = "document-review-assistant"
name = "Document review assistant"
agent_type = "custom"
message_template = "{event}: {title}"

[agents.route]
bot_ids = ["$BOT_ID"]
bot_agent_types = ["webhook"]
project_types = ["contract_review"]
trigger_kinds = ["mention"]

[agents.custom]
command = "/bin/echo"
args = ["browser-mention-route-ok"]
EOF

"$WEBHOOK_DIR/target/debug/openpr-webhook" "$TMP_DIR/openpr-webhook.toml" >"$WEBHOOK_LOG" 2>&1 &
webhook_pid=$!
wait_http "http://127.0.0.1:$WEBHOOK_PORT/health" "openpr-webhook"

"$ROOT_DIR/target/debug/worker" --config "$APP_CONFIG" --concurrency 1 >"$WORKER_LOG" 2>&1 &
worker_pid=$!

cat >"$TMP_DIR/browser-server.mjs" <<'NODE'
import http from 'node:http';

const apiPort = Number(process.env.API_PORT);
const browserPort = Number(process.env.BROWSER_PORT);
const token = process.env.ACCESS_TOKEN;
const issueId = process.env.WORK_ITEM_ID;
const botId = process.env.BOT_ID;

const page = `<!doctype html>
<meta charset="utf-8">
<body>
<script>
(async () => {
  try {
    localStorage.setItem('auth_token', ${JSON.stringify(token)});
    const response = await fetch('/api/v1/issues/${issueId}/comments', {
      method: 'POST',
      headers: {
        'authorization': 'Bearer ${token}',
        'content-type': 'application/json'
      },
      body: JSON.stringify({
        content: '@Document review assistant please review this contract',
        mentions: ['${botId}']
      })
    });
    const body = await response.json();
    document.body.setAttribute('data-status', String(response.status));
    document.body.setAttribute('data-comment-id', body?.data?.id || '');
    document.body.setAttribute('data-browser-mention-smoke', response.ok && body?.code === 0 ? 'done' : 'failed');
  } catch (error) {
    document.body.setAttribute('data-browser-mention-smoke', 'failed');
    document.body.setAttribute('data-browser-mention-error', error instanceof Error ? error.message : String(error));
  }
})();
</script>
</body>`;

const server = http.createServer((req, res) => {
  if (req.url === '/smoke') {
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
    res.end(page);
    return;
  }

  if (req.url?.startsWith('/api/')) {
    const proxyReq = http.request(
      {
        hostname: '127.0.0.1',
        port: apiPort,
        path: req.url,
        method: req.method,
        headers: req.headers,
      },
      (proxyRes) => {
        res.writeHead(proxyRes.statusCode || 500, proxyRes.headers);
        proxyRes.pipe(res);
      }
    );
    proxyReq.on('error', (error) => {
      res.writeHead(502, { 'content-type': 'text/plain; charset=utf-8' });
      res.end(error.message);
    });
    req.pipe(proxyReq);
    return;
  }

  res.writeHead(404);
  res.end('not found');
});

server.listen(browserPort, '127.0.0.1');
NODE

API_PORT="$API_PORT" \
BROWSER_PORT="$BROWSER_PORT" \
ACCESS_TOKEN="$ACCESS_TOKEN" \
WORK_ITEM_ID="$WORK_ITEM_ID" \
BOT_ID="$BOT_ID" \
node "$TMP_DIR/browser-server.mjs" >"$BROWSER_LOG" 2>&1 &
browser_server_pid=$!
wait_http "http://127.0.0.1:$BROWSER_PORT/smoke" "browser smoke server"

"$CHROMIUM_BIN" \
  --headless=new \
  --disable-gpu \
  --disable-dev-shm-usage \
  --no-sandbox \
  --hide-scrollbars \
  --run-all-compositor-stages-before-draw \
  --virtual-time-budget=8000 \
  --dump-dom \
  "http://127.0.0.1:$BROWSER_PORT/smoke" >"$BROWSER_DOM" 2>>"$BROWSER_LOG"

if ! grep -q 'data-browser-mention-smoke="done"' "$BROWSER_DOM"; then
  echo "Browser-originated mention did not complete" >&2
  cat "$BROWSER_DOM" >&2
  exit 1
fi

for _ in $(seq 1 60); do
  task_count="$(psql_smoke -tAc "SELECT COUNT(*) FROM ai_tasks WHERE ai_participant_id = '$BOT_ID' AND task_type = 'comment_requested' AND payload->>'project_type' = 'contract_review';" | tr -d '[:space:]')"
  dispatched_count="$(psql_smoke -tAc "SELECT COUNT(*) FROM agent_invocations ai INNER JOIN ai_tasks t ON t.id = ai.source_task_id WHERE t.ai_participant_id = '$BOT_ID' AND t.task_type = 'comment_requested' AND ai.status = 'dispatched';" | tr -d '[:space:]')"
  if [[ "$task_count" == "1" && "$dispatched_count" == "1" ]]; then
    echo "Browser mention to openpr-webhook smoke passed"
    exit 0
  fi
  sleep 1
done

echo "Timed out waiting for comment_requested task dispatch" >&2
psql_smoke -x -c "SELECT t.id, t.status, t.task_type, t.payload, ai.status AS invocation_status, ai.connector_kind, ai.error_message FROM ai_tasks t LEFT JOIN agent_invocations ai ON ai.source_task_id = t.id ORDER BY t.created_at DESC LIMIT 5;" >&2 || true
echo "--- worker log ---" >&2
tail -120 "$WORKER_LOG" >&2 || true
echo "--- webhook log ---" >&2
tail -120 "$WEBHOOK_LOG" >&2 || true
exit 1
