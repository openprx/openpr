#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
POSTGRES_PORT="${OPENPR_SMOKE_PG_PORT:-5366}"
PG_SUPERUSER="${OPENPR_SMOKE_PG_SUPERUSER:-postgres}"
DB_NAME="openpr_bootstrap_mcp_smoke_$$_$(date +%s)"
DB_USER="openpr_bootstrap_mcp_smoke_$$_$(date +%s)"
DB_PASSWORD="$(openssl rand -hex 12)"
API_PORT="${OPENPR_SMOKE_API_PORT:-$((26180 + ($$ % 1000)))}"
MCP_PORT="${OPENPR_SMOKE_MCP_PORT:-$((27180 + ($$ % 1000)))}"
TMP_DIR="$(mktemp -d /tmp/openpr-bootstrap-mcp-smoke.XXXXXX)"
API_LOG="$TMP_DIR/api.log"
MCP_LOG="$TMP_DIR/mcp.log"
FIRST_BOOTSTRAP_LOG="$TMP_DIR/bootstrap-first.log"
SECOND_BOOTSTRAP_LOG="$TMP_DIR/bootstrap-second.log"
ENV_PATH="$TMP_DIR/.env"
JWT_SECRET="openpr-bootstrap-mcp-smoke-secret"

api_pid=""
mcp_pid=""

cleanup() {
  local exit_code=$?
  if [[ -n "$mcp_pid" ]] && kill -0 "$mcp_pid" 2>/dev/null; then
    kill "$mcp_pid" 2>/dev/null || true
    wait "$mcp_pid" 2>/dev/null || true
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
    echo "Smoke failed. API log: $API_LOG" >&2
    echo "Smoke failed. MCP log: $MCP_LOG" >&2
    echo "Smoke failed. First bootstrap log: $FIRST_BOOTSTRAP_LOG" >&2
    echo "Smoke failed. Second bootstrap log: $SECOND_BOOTSTRAP_LOG" >&2
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

env_value() {
  local key="$1"
  awk -F= -v key="$key" '$1 == key { print substr($0, length(key) + 2); exit }' "$ENV_PATH"
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

DATABASE_URL="postgres://$DB_USER:$DB_PASSWORD@127.0.0.1:$POSTGRES_PORT/$DB_NAME"

BIND_ADDR="127.0.0.1:$API_PORT" \
DATABASE_URL="$DATABASE_URL" \
JWT_SECRET="$JWT_SECRET" \
RUST_LOG="${RUST_LOG:-api=info,openpr=info}" \
"$ROOT_DIR/target/debug/api" >"$API_LOG" 2>&1 &
api_pid=$!
wait_http "http://127.0.0.1:$API_PORT/health" "OpenPR API"

: >"$ENV_PATH"
OPENPR_API_URL="http://127.0.0.1:$API_PORT" \
OPENPR_DEMO_WRITE_ENV=1 \
OPENPR_DEMO_ENV_PATH="$ENV_PATH" \
OPENPR_DEMO_RESTART_MCP=0 \
OPENPR_DEMO_VERIFY_MCP_HTTP=0 \
"$ROOT_DIR/scripts/bootstrap-restaurant-demo.sh" >"$FIRST_BOOTSTRAP_LOG"

OPENPR_BOT_TOKEN="$(env_value OPENPR_BOT_TOKEN)"
OPENPR_WORKSPACE_ID="$(env_value OPENPR_WORKSPACE_ID)"

if [[ -z "$OPENPR_BOT_TOKEN" || -z "$OPENPR_WORKSPACE_ID" ]]; then
  echo "Demo bootstrap did not write MCP credentials to the temporary env file" >&2
  exit 1
fi

OPENPR_API_URL="http://127.0.0.1:$API_PORT" \
OPENPR_BOT_TOKEN="$OPENPR_BOT_TOKEN" \
OPENPR_WORKSPACE_ID="$OPENPR_WORKSPACE_ID" \
"$ROOT_DIR/target/debug/mcp-server" serve --transport http --bind-addr "127.0.0.1:$MCP_PORT" >"$MCP_LOG" 2>&1 &
mcp_pid=$!
wait_http "http://127.0.0.1:$MCP_PORT/health" "OpenPR MCP"

OPENPR_API_URL="http://127.0.0.1:$API_PORT" \
OPENPR_DEMO_WRITE_ENV=1 \
OPENPR_DEMO_ENV_PATH="$ENV_PATH" \
OPENPR_DEMO_RESTART_MCP=0 \
OPENPR_DEMO_VERIFY_MCP_HTTP=1 \
OPENPR_DEMO_MCP_URL="http://127.0.0.1:$MCP_PORT/mcp/rpc" \
"$ROOT_DIR/scripts/bootstrap-restaurant-demo.sh" >"$SECOND_BOOTSTRAP_LOG"

if ! rg -q "MCP HTTP verification passed: projects.list includes RESTDEMO" "$SECOND_BOOTSTRAP_LOG"; then
  echo "MCP HTTP verification did not prove RESTDEMO is visible through projects.list" >&2
  exit 1
fi

echo "restaurant demo bootstrap MCP HTTP smoke passed"
