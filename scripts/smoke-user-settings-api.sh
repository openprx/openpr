#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
POSTGRES_PORT="${OPENPR_SMOKE_PG_PORT:-5366}"
PG_SUPERUSER="${OPENPR_SMOKE_PG_SUPERUSER:-postgres}"
DB_NAME="openpr_user_settings_smoke_$$_$(date +%s)"
DB_USER="openpr_user_settings_smoke_$$_$(date +%s)"
DB_PASSWORD="$(openssl rand -hex 12)"
API_PORT="${OPENPR_SMOKE_API_PORT:-$((25180 + ($$ % 1000)))}"
TMP_DIR="$(mktemp -d /tmp/openpr-user-settings-smoke.XXXXXX)"
API_LOG="$TMP_DIR/api.log"
JWT_SECRET="openpr-user-settings-smoke-secret"

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

require_cmd curl
require_cmd node
require_cmd openssl
require_cmd psql
require_cmd sudo

cargo build -q -p api --bin api

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

API_URL="http://127.0.0.1:$API_PORT" node --input-type=commonjs <<'NODE'
const apiUrl = process.env.API_URL;

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function request(method, path, body, token, expectedStatus = 200) {
  const response = await fetch(`${apiUrl}${path}`, {
    method,
    headers: {
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
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

function assertApiOk(payload, message) {
  assert(payload?.code === 0, `${message}: ${JSON.stringify(payload)}`);
  assert(payload.data !== null, `${message}: missing data`);
}

async function main() {
  const initialEmail = `settings-${Date.now()}@example.local`;
  const updatedEmail = `settings-updated-${Date.now()}@example.local`;
  const oldPassword = 'initial-password-123';
  const newPassword = 'updated-password-456';

  const registered = await request('POST', '/api/v1/auth/register', {
    email: initialEmail,
    password: oldPassword,
    name: 'Settings User',
  });
  assertApiOk(registered, 'register');
  const token = registered.data.tokens.access_token;

  const profile = await request('PUT', '/api/v1/auth/me/profile', {
    name: 'Settings User Updated',
    email: updatedEmail,
    avatar_url: 'https://example.local/avatar.png',
  }, token);
  assertApiOk(profile, 'update profile');
  assert(profile.data.user.name === 'Settings User Updated', 'profile name should update');
  assert(profile.data.user.email === updatedEmail, 'profile email should update');
  assert(profile.data.user.avatar_url === 'https://example.local/avatar.png', 'avatar URL should update');

  const prefs = await request('PUT', '/api/v1/auth/me/preferences', {
    notification_prefs: {
      email_notification: false,
      mention_only: true,
      daily_digest: false,
    },
  }, token);
  assertApiOk(prefs, 'update preferences');
  assert(prefs.data.notification_prefs.mention_only === true, 'mention_only should persist in response');

  const loadedPrefs = await request('GET', '/api/v1/auth/me/preferences', undefined, token);
  assertApiOk(loadedPrefs, 'load preferences');
  assert(loadedPrefs.data.notification_prefs.email_notification === false, 'email_notification should persist');
  assert(loadedPrefs.data.notification_prefs.mention_only === true, 'mention_only should persist');
  assert(loadedPrefs.data.notification_prefs.daily_digest === false, 'daily_digest should persist');

  const wrongPassword = await request('PUT', '/api/v1/auth/me/password', {
    current_password: 'not-the-current-password',
    new_password: 'should-not-save-123',
  }, token);
  assert(wrongPassword.code !== 0, 'wrong current password should fail');

  const changed = await request('PUT', '/api/v1/auth/me/password', {
    current_password: oldPassword,
    new_password: newPassword,
  }, token);
  assert(changed.code === 0, 'password update should succeed');

  const oldLogin = await request('POST', '/api/v1/auth/login', {
    email: updatedEmail,
    password: oldPassword,
  });
  assert(oldLogin.code !== 0, 'old password login should fail');

  const newLogin = await request('POST', '/api/v1/auth/login', {
    email: updatedEmail,
    password: newPassword,
  });
  assertApiOk(newLogin, 'new password login');
  assert(newLogin.data.user.email === updatedEmail, 'new login should use updated email');

  console.log(JSON.stringify({
    user_id: profile.data.user.id,
    email: updatedEmail,
    preferences_verified: true,
    password_rotated: true,
  }));
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

echo "user settings API smoke passed"
