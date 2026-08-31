#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${1:?repo root required}"
DATABASE_URL="${2:?database URL required}"
WORK_DIR="${3:?work directory required}"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PROBE="$ROOT_DIR/scripts/lib/flow_authz_baseline_live_probe.py"
MCP_PROBE="$ROOT_DIR/scripts/lib/mcp_transport_probe.py"

failure() {
  jq -n -c --arg reason "$1" '{status:"failed",observations:{},negative_controls:{},violations:[$reason]}'
  exit 1
}

for tool in cargo psql curl jq python3 sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || failure "missing required command: $tool"
done
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atc 'SELECT 1' >/dev/null 2>&1 || failure "configured PostgreSQL environment is unreachable"

echo "=== prerequisite: building isolated apply worker (debug) ===" >&2
( cd "$REPO_ROOT" && cargo build -q -p collab-core --bin collab-isolated-apply-worker ) || failure "debug isolated apply worker failed to build"
echo "=== building authz live-surface binaries ===" >&2
( cd "$REPO_ROOT" && cargo build -q -p api --bin api -p mcp-server --bin mcp-server --bin sylvode ) || failure "authz live-surface binaries failed to build"

TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
API_BIN="$TARGET_DIR/debug/api"
MCP_BIN="$TARGET_DIR/debug/mcp-server"
CLI_BIN="$TARGET_DIR/debug/sylvode"
[[ -x "$API_BIN" && -x "$MCP_BIN" && -x "$CLI_BIN" ]] || failure "one or more authz live-surface binaries are missing"

RUN_ID="$(python3 -c 'import uuid; print(uuid.uuid4().hex[:8])')"
SCRATCH_DB="openpr_flow_authz_member_$RUN_ID"
SCRATCH_URL="${DATABASE_URL%/*}/$SCRATCH_DB"
API_PORT=$((34000 + RANDOM % 8000))
API_PID=""

# shellcheck disable=SC2317
cleanup() {
  local ec=$?
  if [[ -n "$API_PID" ]] && kill -0 "$API_PID" 2>/dev/null; then
    kill "$API_PID" 2>/dev/null || true
    wait "$API_PID" 2>/dev/null || true
  fi
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "DROP DATABASE IF EXISTS \"$SCRATCH_DB\" WITH (FORCE)" >/dev/null 2>&1 || true
  exit "$ec"
}
trap cleanup EXIT

psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "CREATE DATABASE \"$SCRATCH_DB\"" >/dev/null || failure "could not create authz scratch database"
JWT_SECRET="authz-member-baseline-not-a-real-secret"
API_CONFIG="$WORK_DIR/member-api.toml"
cat > "$API_CONFIG" <<EOF
[server]
app_name = "api"
bind_addr = "127.0.0.1:$API_PORT"
[database]
url = "$SCRATCH_URL"
[auth]
jwt_secret = "$JWT_SECRET"
[logging]
filter = "api=warn,openpr=warn"
format = "text"
EOF
"$API_BIN" --config "$API_CONFIG" >"$WORK_DIR/member-api.log" 2>&1 & API_PID=$!
for _ in $(seq 1 120); do
  curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1 && break
  kill -0 "$API_PID" 2>/dev/null || break
  sleep 0.25
done
curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1 || failure "authz live API did not become healthy"

OWNER_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
BOT_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
WORKSPACE_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
BOT_TOKEN="opr_authz_member_$RUN_ID"
BOT_HASH="$(printf '%s' "$BOT_TOKEN" | sha256sum | awk '{print $1}')"
FIXTURE_MEMBER_LEVEL="edit"
if [[ "${FLOW_AUTHZ_ANTIPROOF_MEMBER_LEVEL_VIEW:-0}" == 1 ]]; then
  echo "=== anti-proof: seeding tested default_member_level as view ===" >&2
  FIXTURE_MEMBER_LEVEL="view"
fi
psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id,email,password_hash,name,role,is_active,entity_type,created_at,updated_at)
VALUES ('$OWNER_ID','authz-owner-$RUN_ID@example.local','','Authz Owner','user',true,'human',now(),now());
INSERT INTO users (id,email,password_hash,name,role,is_active,entity_type,agent_type,created_at,updated_at)
VALUES ('$BOT_ID','$BOT_ID@bot.openpr.local','!','Default Edit Member','user',true,'bot_mcp','mcp',now(),now());
INSERT INTO workspaces (id,slug,name,created_by,created_at,updated_at)
VALUES ('$WORKSPACE_ID','authz-member-$RUN_ID','Authz Member Baseline','$OWNER_ID',now(),now());
INSERT INTO workspace_members (workspace_id,user_id,role,created_at)
VALUES ('$WORKSPACE_ID','$OWNER_ID','owner',now()),('$WORKSPACE_ID','$BOT_ID','member',now());
INSERT INTO flow_workspace_settings (workspace_id,flow_enabled,default_member_level,authz_epoch,updated_at)
VALUES ('$WORKSPACE_ID',true,'$FIXTURE_MEMBER_LEVEL',0,now());
INSERT INTO workspace_bots (id,workspace_id,name,token_hash,token_prefix,permissions,created_by,is_active,created_at,updated_at)
VALUES ('$BOT_ID','$WORKSPACE_ID','Default Edit Member','$BOT_HASH','${BOT_TOKEN:0:8}','["read","write"]'::jsonb,'$OWNER_ID',true,now(),now());
SQL

CLI_CONFIG="$WORK_DIR/member-cli.toml"
cat > "$CLI_CONFIG" <<EOF
[database]
url = "postgres://unused:unused@127.0.0.1:5432/unused"
[auth]
jwt_secret = "unused-by-authz-verifier"
[logging]
filter = "error"
format = "text"
[mcp]
api_url = "http://127.0.0.1:$API_PORT"
bot_token = "$BOT_TOKEN"
workspace_id = "$WORKSPACE_ID"
transport = "http"
EOF

set +e
RESULT="$(python3 "$PROBE" \
  --api "http://127.0.0.1:$API_PORT" \
  --bot-token "$BOT_TOKEN" \
  --workspace "$WORKSPACE_ID" \
  --database-url "$SCRATCH_URL" \
  --mcp-binary "$MCP_BIN" \
  --cli-binary "$CLI_BIN" \
  --mcp-probe "$MCP_PROBE" \
  --cli-config "$CLI_CONFIG" \
  --work-dir "$WORK_DIR" 2>"$WORK_DIR/member-probe.log")"
RESULT_EXIT=$?
set -e
if ! jq -e . >/dev/null 2>&1 <<<"$RESULT"; then
  failure "member baseline probe produced invalid JSON: $(tail -30 "$WORK_DIR/member-probe.log" | tr '\n' ' ')"
fi
printf '%s\n' "$RESULT"
exit "$RESULT_EXIT"
