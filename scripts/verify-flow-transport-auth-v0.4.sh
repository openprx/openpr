#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 collab transport-auth verifier.
#
# Contract: /opt/working/sylvode-flow/decisions/ADR-0007-collab-transport-auth.md
# ("签发契约", "Upgrade 与一次性消费", "Cookie `Secure` 补齐", "Gate"),
# /opt/working/sylvode-flow/security/threat-model.md rows "长期 token 泄漏",
# "ticket 重放/串用", "WebSocket CSRF/cross-origin", "伪造 actor/origin", and
# /opt/working/sylvode-flow/gates/gate-commands.md:232-247 (why
# `unauthorized_update_rejected` moved from v0.3 to v0.4: only v0.4's
# ticket-bound session has an assertable authorization semantic at all).
#
# Backs three of v0.4's 52 hard gates:
#   ticket_single_use_origin_bot_exclusion
#   secure_cookie_and_local_dev_guard
#   unauthorized_update_rejected
#
# THESE ARE NEGATIVE GATES. Every one of them is decided by a fixture that
# constructs a request which MUST be refused (bot issuance, replayed ticket,
# wrong Origin, wrong client_id, expired ticket, revoked member's update,
# a ticket-mismatched document, an insecure-cookie config on a non-loopback
# bind) and asserts both the refusal AND that no side effect was produced
# before it -- the consumed_at column stays NULL, collab_updates /
# business_events / event_dispatch stay at their pre-request counts, the
# process never binds its port. Each negative is paired with a positive
# control run through the identical path, so a fixture that is merely broken
# (bad bytes, wrong document, dead server) cannot pass as a security
# property.
#
# Static source inspection is used only as corroboration; it never decides a
# gate on its own.
#
# Exit codes: 0 = all three gates passed, 1 = a gate failed,
# 2 = usage/tool/environment error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
ADR_PATH=""
DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-}"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-transport-auth-v0.4.sh --adr PATH --json [OPTIONS]

Verifies ADR-0007's one-time collab ticket (TTL, single use, Origin and
client binding, bot exclusion, no ticket in logs), the auth cookie `Secure`
attribute with its loopback-only development exception, and v0.4's
unauthorized-update rejection, by driving a real `api` binary against a real
PostgreSQL database over real HTTP and real WebSocket connections.

Writes evidence/v0.4/transport-auth-result.json.

Options:
  --adr PATH             Path to ADR-0007. Required.
  --database-url URL     Postgres DSN. Default: $OPENPR_TEST_DATABASE_URL
  --repo-root DIR        Repository containing apps/api. Default: this checkout.
  --evidence-root DIR    Where transport-auth-result.json is written.
                         Default: /opt/working/sylvode-flow/evidence/v0.4
  --json                 Required for CLI-contract compatibility.
  -h, --help             Show this help and exit 0.

Exit codes: 0 all three gates passed, 1 a gate failed, 2 usage/tool error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --adr) ADR_PATH="${2:?--adr requires a PATH argument}"; shift 2 ;;
    --database-url) DATABASE_URL="${2:?--database-url requires a value}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$ADR_PATH" ]]; then echo "FAIL: --adr is required" >&2; usage >&2; exit 2; fi
if [[ ! -f "$ADR_PATH" ]]; then echo "FAIL: --adr file not found: $ADR_PATH" >&2; exit 2; fi
if [[ $JSON_MODE -ne 1 ]]; then echo "FAIL: --json is required" >&2; usage >&2; exit 2; fi
if [[ -z "$DATABASE_URL" ]]; then
  echo "FAIL: no database URL configured (set --database-url or OPENPR_TEST_DATABASE_URL)" >&2
  exit 2
fi
for tool in jq sha256sum git psql curl python3 cargo; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    exit 2
  fi
done
if ! python3 -c 'import bcrypt' >/dev/null 2>&1; then
  echo "FAIL: python3 'bcrypt' module is required (the login fixture needs a real password hash)" >&2
  exit 2
fi
WS_PROBE="$ROOT_DIR/scripts/lib/flow_ws_probe.py"
if [[ ! -f "$WS_PROBE" ]]; then
  echo "FAIL: WebSocket probe helper not found: $WS_PROBE" >&2
  exit 2
fi
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi
if ! psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atc "SELECT 1" >/dev/null 2>&1; then
  echo "FAIL: database is not reachable: $DATABASE_URL" >&2
  exit 2
fi

mkdir -p "$EVIDENCE_ROOT"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# ---------------------------------------------------------------- check log
# Each check is one assertion with an explicit expectation. `record` is the
# only way a check enters the evidence, so no assertion can be silently
# dropped: an unrecorded check id simply never appears, and the gate rollup
# below requires every id it names to be present AND passed.
CHECKS_JSON="[]"

record() {
  local id="$1" gate="$2" ok="$3" expectation="$4" observed="$5"
  CHECKS_JSON="$(jq -n --argjson acc "$CHECKS_JSON" \
    --arg id "$id" --arg gate "$gate" --argjson passed "$ok" \
    --arg expectation "$expectation" --arg observed "$observed" \
    '$acc + [{id:$id, gate:$gate, passed:$passed, expectation:$expectation, observed:$observed}]')"
  if [[ "$ok" == "true" ]]; then
    echo "  PASS  $id" >&2
  else
    echo "  FAIL  $id -- expected: $expectation | observed: $observed" >&2
  fi
}

assert_eq() {
  local id="$1" gate="$2" want="$3" got="$4" what="$5"
  if [[ "$want" == "$got" ]]; then
    record "$id" "$gate" true "$what == $want" "$got"
  else
    record "$id" "$gate" false "$what == $want" "$got"
  fi
}

# --------------------------------------------------------------- build api
BIN_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
echo "=== building api binary (cargo build -p api --bin api) ===" >&2
( cd "$REPO_ROOT" && cargo build -q -p api --bin api ) || {
  echo "FAIL: api binary failed to build" >&2
  exit 2
}
API_BIN="$BIN_TARGET_DIR/debug/api"
if [[ ! -x "$API_BIN" ]]; then
  echo "FAIL: built api binary not found at $API_BIN" >&2
  exit 2
fi

# -------------------------------------------------------------- fixture ids
uuid() { python3 -c 'import uuid; print(uuid.uuid4())'; }
RUN_ID="$(python3 -c 'import uuid; print(uuid.uuid4().hex[:8])')"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/openpr-transport-auth-verify.XXXXXX")"
WORKSPACE_ID="$(uuid)"
OWNER_USER="$(uuid)"
BOT_ID="$(uuid)"
BOT_TOKEN="opr_transport_verify_${RUN_ID}"
USER_EMAIL="transport-verify-$RUN_ID@example.local"
USER_PASSWORD="TransportVerify!${RUN_ID}"
ORIGIN_A="https://flow-verify-a.test"
ORIGIN_B="https://flow-verify-b.test"
ORIGIN_EVIL="https://flow-verify-evil.test"
API_PORT=$((22000 + RANDOM % 20000))
API_LOG="$TMP_DIR/api.log"
API_PID=""
DEV_PID=""
ENV_PID=""

# shellcheck disable=SC2317  # invoked only via `trap ... EXIT` below.
cleanup() {
  local ec=$?
  for pid in "$API_PID" "$DEV_PID" "$ENV_PID"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  psql "$DATABASE_URL" -q >/dev/null 2>&1 <<SQL || true
DELETE FROM collab_updates WHERE document_id IN (
  SELECT id FROM collab_documents WHERE object_id IN (SELECT id FROM flow_objects WHERE workspace_id='$WORKSPACE_ID'));
DELETE FROM event_dispatch WHERE event_id IN (SELECT id FROM business_events WHERE workspace_id='$WORKSPACE_ID');
DELETE FROM business_events WHERE workspace_id='$WORKSPACE_ID';
DELETE FROM collab_tickets WHERE workspace_id='$WORKSPACE_ID';
DELETE FROM flow_integrity_records WHERE workspace_id='$WORKSPACE_ID';
DELETE FROM flow_object_projections WHERE object_id IN (SELECT id FROM flow_objects WHERE workspace_id='$WORKSPACE_ID');
DELETE FROM collab_documents WHERE object_id IN (SELECT id FROM flow_objects WHERE workspace_id='$WORKSPACE_ID');
DELETE FROM flow_objects WHERE workspace_id='$WORKSPACE_ID';
DELETE FROM flow_workspace_settings WHERE workspace_id='$WORKSPACE_ID';
DELETE FROM workspace_bots WHERE id='$BOT_ID';
DELETE FROM workspace_members WHERE workspace_id='$WORKSPACE_ID';
DELETE FROM workspaces WHERE id='$WORKSPACE_ID';
DELETE FROM users WHERE id IN ('$OWNER_USER','$BOT_ID');
SQL
  rm -rf "$TMP_DIR"
  exit "$ec"
}
trap cleanup EXIT

write_config() {
  # $1 = path, $2 = bind_addr, $3 = allow_insecure_cookies (true/false)
  cat > "$1" <<EOF
[server]
app_name = "api"
bind_addr = "$2"

[database]
url = "$DATABASE_URL"

[auth]
jwt_secret = "transport-auth-verify-not-a-real-secret"
allow_insecure_cookies = $3

[flow]
collab_allowed_origins = ["$ORIGIN_A", "$ORIGIN_B"]

[logging]
filter = "api=info,openpr=info"
format = "text"
EOF
}

wait_healthy() {
  local port="$1" i
  for ((i = 0; i < 60; i++)); do
    if curl -fsS "http://127.0.0.1:$port/health" >/dev/null 2>&1; then return 0; fi
    sleep 0.5
  done
  return 1
}

# --------------------------------------------------------------- seed rows
export PW="$USER_PASSWORD"
PW_HASH="$(python3 -c "import bcrypt,os; print(bcrypt.hashpw(os.environ['PW'].encode(), bcrypt.gensalt(rounds=6)).decode())")"
unset PW
BOT_TOKEN_HASH="$(printf '%s' "$BOT_TOKEN" | sha256sum | awk '{print $1}')"
BOT_TOKEN_PREFIX="${BOT_TOKEN:0:8}"

psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES ('$OWNER_USER', '$USER_EMAIL', '$PW_HASH', 'Transport Verify Owner', 'user', true, 'human', NULL, now(), now());
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES ('$BOT_ID', 'transport-verify-bot-$RUN_ID@bot.openpr.local', '!', 'Transport Verify Bot', 'user', true, 'bot_mcp', 'mcp', now(), now());
INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE_ID', 'transport-verify-$RUN_ID', 'Transport Verify', '$OWNER_USER', now(), now());
INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES ('$WORKSPACE_ID', '$OWNER_USER', 'owner', now()),
       ('$WORKSPACE_ID', '$BOT_ID', 'member', now());
INSERT INTO flow_workspace_settings (workspace_id, flow_enabled, default_member_level, authz_epoch, updated_at)
VALUES ('$WORKSPACE_ID', true, 'edit', 0, now());
INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by, is_active, created_at, updated_at)
VALUES ('$BOT_ID', '$WORKSPACE_ID', 'Transport Verify Bot', '$BOT_TOKEN_HASH', '$BOT_TOKEN_PREFIX', '["read","write"]'::jsonb, '$OWNER_USER', true, now(), now());
SQL

# --------------------------------------------------------- start main api
MAIN_CONFIG="$TMP_DIR/api.toml"
write_config "$MAIN_CONFIG" "127.0.0.1:$API_PORT" "false"
"$API_BIN" --config "$MAIN_CONFIG" > "$API_LOG" 2>&1 &
API_PID=$!
if ! wait_healthy "$API_PORT"; then
  echo "FAIL: api did not become healthy within 30s; log follows" >&2
  cat "$API_LOG" >&2
  exit 2
fi
BASE="http://127.0.0.1:$API_PORT"

# ------------------------------------------------------------------ login
LOGIN_HEADERS="$TMP_DIR/login.headers"
LOGIN_BODY="$(curl -sS -D "$LOGIN_HEADERS" -X POST "$BASE/api/v1/auth/login" \
  -H 'Content-Type: application/json' \
  -d "$(jq -n --arg e "$USER_EMAIL" --arg p "$USER_PASSWORD" '{email:$e, password:$p}')")"
ACCESS_TOKEN="$(jq -r '.data.tokens.access_token // empty' <<<"$LOGIN_BODY")"
if [[ -z "$ACCESS_TOKEN" ]]; then
  echo "FAIL: could not log the fixture user in: $LOGIN_BODY" >&2
  cat "$API_LOG" >&2
  exit 2
fi

# ============================================================ COOKIE GATE
GATE_COOKIE="secure_cookie_and_local_dev_guard"

ACCESS_COOKIE_LINE="$(grep -i '^set-cookie: *access_token=' "$LOGIN_HEADERS" | head -n1 | tr -d '\r')"
REFRESH_COOKIE_LINE="$(grep -i '^set-cookie: *refresh_token=' "$LOGIN_HEADERS" | head -n1 | tr -d '\r')"
cookie_attrs_ok() {
  local line="$1"
  [[ "$line" == *"; Secure"* ]] && [[ "$line" == *"HttpOnly"* ]] \
    && [[ "$line" == *"SameSite=Lax"* ]] && [[ "$line" == *"Path=/"* ]]
}
if cookie_attrs_ok "$ACCESS_COOKIE_LINE" && cookie_attrs_ok "$REFRESH_COOKIE_LINE"; then
  record "default_cookies_secure_httponly_samesite" "$GATE_COOKIE" true \
    "access_token and refresh_token Set-Cookie carry Secure + HttpOnly + SameSite=Lax + Path=/" \
    "both cookies carry all four attributes"
else
  record "default_cookies_secure_httponly_samesite" "$GATE_COOKIE" false \
    "access_token and refresh_token Set-Cookie carry Secure + HttpOnly + SameSite=Lax + Path=/" \
    "access=[${ACCESS_COOKIE_LINE#*: }] refresh=[${REFRESH_COOKIE_LINE#*: }]"
fi

LOGOUT_HEADERS="$TMP_DIR/logout.headers"
curl -sS -D "$LOGOUT_HEADERS" -o /dev/null -X POST "$BASE/api/v1/auth/logout" \
  -H "Authorization: Bearer $ACCESS_TOKEN" >/dev/null 2>&1 || true
CLEAR_COUNT="$(grep -ci '^set-cookie:.*Max-Age=0.*Secure' "$LOGOUT_HEADERS" 2>/dev/null || true)"
assert_eq "clear_cookies_secure" "$GATE_COOKIE" "2" "${CLEAR_COUNT:-0}" \
  "logout emits 2 Max-Age=0 Set-Cookie headers that still carry Secure"

# NEGATIVE: a request header must not be able to talk the server out of
# `Secure`. `X-Forwarded-Proto: http` is exactly the downgrade a
# reverse-proxy-aware implementation is tempted to honour, and ADR-0007
# forbids it ("不通过请求 header 或环境变量自动降级").
FWD_HEADERS="$TMP_DIR/login-forwarded.headers"
curl -sS -D "$FWD_HEADERS" -o /dev/null -X POST "$BASE/api/v1/auth/login" \
  -H 'Content-Type: application/json' \
  -H 'X-Forwarded-Proto: http' -H 'Forwarded: proto=http' -H 'X-Forwarded-Ssl: off' \
  -d "$(jq -n --arg e "$USER_EMAIL" --arg p "$USER_PASSWORD" '{email:$e, password:$p}')" >/dev/null
FWD_INSECURE="$(grep -ci '^set-cookie: *\(access\|refresh\)_token=' "$FWD_HEADERS" 2>/dev/null || true)"
FWD_SECURE="$(grep -ci '^set-cookie: *\(access\|refresh\)_token=.*Secure' "$FWD_HEADERS" 2>/dev/null || true)"
if [[ "${FWD_INSECURE:-0}" == "2" && "${FWD_SECURE:-0}" == "2" ]]; then
  record "forwarded_proto_header_cannot_downgrade_cookie" "$GATE_COOKIE" true \
    "X-Forwarded-Proto/Forwarded/X-Forwarded-Ssl cannot drop Secure" \
    "2 of 2 auth cookies still carry Secure"
else
  record "forwarded_proto_header_cannot_downgrade_cookie" "$GATE_COOKIE" false \
    "X-Forwarded-Proto/Forwarded/X-Forwarded-Ssl cannot drop Secure" \
    "auth cookies=${FWD_INSECURE:-0} with Secure=${FWD_SECURE:-0}"
fi

# NEGATIVE: the same for environment variables. This deployment reads
# configuration from a TOML file only, so an env var named after the field
# must have no effect at all -- asserted by observation, not by trusting
# that no env reader exists.
ENV_PORT=$((API_PORT + 1))
ENV_CONFIG="$TMP_DIR/api-env.toml"
ENV_LOG="$TMP_DIR/api-env.log"
write_config "$ENV_CONFIG" "127.0.0.1:$ENV_PORT" "false"
env OPENPR_AUTH__ALLOW_INSECURE_COOKIES=true \
    AUTH_ALLOW_INSECURE_COOKIES=true \
    OPENPR__AUTH__ALLOW_INSECURE_COOKIES=true \
    ALLOW_INSECURE_COOKIES=true \
    "$API_BIN" --config "$ENV_CONFIG" > "$ENV_LOG" 2>&1 &
ENV_PID=$!
if wait_healthy "$ENV_PORT"; then
  ENV_HEADERS="$TMP_DIR/login-env.headers"
  curl -sS -D "$ENV_HEADERS" -o /dev/null -X POST "http://127.0.0.1:$ENV_PORT/api/v1/auth/login" \
    -H 'Content-Type: application/json' \
    -d "$(jq -n --arg e "$USER_EMAIL" --arg p "$USER_PASSWORD" '{email:$e, password:$p}')" >/dev/null
  ENV_SECURE="$(grep -ci '^set-cookie: *\(access\|refresh\)_token=.*Secure' "$ENV_HEADERS" 2>/dev/null || true)"
  assert_eq "env_var_cannot_downgrade_cookie" "$GATE_COOKIE" "2" "${ENV_SECURE:-0}" \
    "with ALLOW_INSECURE_COOKIES=true in the environment and allow_insecure_cookies=false in the file, auth cookies still carrying Secure"
else
  record "env_var_cannot_downgrade_cookie" "$GATE_COOKIE" false \
    "api starts with the env override present and ignores it" \
    "api never became healthy on port $ENV_PORT; log: $(tail -n 5 "$ENV_LOG" | tr '\n' ' ')"
fi
kill "$ENV_PID" 2>/dev/null || true
wait "$ENV_PID" 2>/dev/null || true
ENV_PID=""

# NEGATIVE (the core one): allow_insecure_cookies=true on a NON-loopback
# bind must fail closed at configuration validation -- the process must not
# start at all, so the insecure cookie is never emitted onto a reachable
# listener.
BAD_PORT=$((API_PORT + 2))
BAD_CONFIG="$TMP_DIR/api-nonloopback-insecure.toml"
BAD_LOG="$TMP_DIR/api-nonloopback-insecure.log"
write_config "$BAD_CONFIG" "0.0.0.0:$BAD_PORT" "true"
set +e
timeout 30 "$API_BIN" --config "$BAD_CONFIG" > "$BAD_LOG" 2>&1
BAD_EXIT=$?
set -e
BAD_LISTENING="no"
if curl -fsS --max-time 2 "http://127.0.0.1:$BAD_PORT/health" >/dev/null 2>&1; then
  BAD_LISTENING="yes"
fi
BAD_MENTIONS_GUARD="no"
if grep -qi "allow_insecure_cookies" "$BAD_LOG"; then BAD_MENTIONS_GUARD="yes"; fi
# `124` is `timeout`'s own kill code: it means the process was still running
# when the deadline hit, i.e. it DID start. That must not be accepted as
# "nonzero exit", or an implementation that ignores the guard entirely would
# still satisfy this check by being killed from outside.
BAD_EXPECT="allow_insecure_cookies=true + bind_addr=0.0.0.0 => the process exits on its own with a nonzero code that is not timeout's 124, never binds its port, and names the guard in the error"
if [[ $BAD_EXIT -ne 0 && $BAD_EXIT -ne 124 && "$BAD_LISTENING" == "no" && "$BAD_MENTIONS_GUARD" == "yes" ]]; then
  record "non_loopback_insecure_cookie_refuses_to_start" "$GATE_COOKIE" true "$BAD_EXPECT" \
    "exit=$BAD_EXIT listening=$BAD_LISTENING guard_named=$BAD_MENTIONS_GUARD"
else
  record "non_loopback_insecure_cookie_refuses_to_start" "$GATE_COOKIE" false "$BAD_EXPECT" \
    "exit=$BAD_EXIT listening=$BAD_LISTENING guard_named=$BAD_MENTIONS_GUARD log=$(tail -n 3 "$BAD_LOG" | tr '\n' ' ')"
fi

# POSITIVE CONTROL for the guard above: the loopback development exception
# must actually work, and must actually drop `Secure`. Without this the
# previous check could pass simply because the binary refuses every config.
DEV_PORT=$((API_PORT + 3))
DEV_CONFIG="$TMP_DIR/api-loopback-insecure.toml"
DEV_LOG="$TMP_DIR/api-loopback-insecure.log"
write_config "$DEV_CONFIG" "127.0.0.1:$DEV_PORT" "true"
"$API_BIN" --config "$DEV_CONFIG" > "$DEV_LOG" 2>&1 &
DEV_PID=$!
if wait_healthy "$DEV_PORT"; then
  DEV_HEADERS="$TMP_DIR/login-dev.headers"
  curl -sS -D "$DEV_HEADERS" -o /dev/null -X POST "http://127.0.0.1:$DEV_PORT/api/v1/auth/login" \
    -H 'Content-Type: application/json' \
    -d "$(jq -n --arg e "$USER_EMAIL" --arg p "$USER_PASSWORD" '{email:$e, password:$p}')" >/dev/null
  DEV_TOTAL="$(grep -ci '^set-cookie: *\(access\|refresh\)_token=' "$DEV_HEADERS" 2>/dev/null || true)"
  DEV_SECURE="$(grep -ci '^set-cookie: *\(access\|refresh\)_token=.*Secure' "$DEV_HEADERS" 2>/dev/null || true)"
  if [[ "${DEV_TOTAL:-0}" == "2" && "${DEV_SECURE:-0}" == "0" ]]; then
    record "loopback_dev_exception_scoped_and_effective" "$GATE_COOKIE" true \
      "allow_insecure_cookies=true + loopback bind => starts and emits 2 auth cookies WITHOUT Secure" \
      "auth cookies=2 with Secure=0"
  else
    record "loopback_dev_exception_scoped_and_effective" "$GATE_COOKIE" false \
      "allow_insecure_cookies=true + loopback bind => starts and emits 2 auth cookies WITHOUT Secure" \
      "auth cookies=${DEV_TOTAL:-0} with Secure=${DEV_SECURE:-0}"
  fi
else
  record "loopback_dev_exception_scoped_and_effective" "$GATE_COOKIE" false \
    "allow_insecure_cookies=true + loopback bind starts" \
    "api never became healthy on port $DEV_PORT; log: $(tail -n 5 "$DEV_LOG" | tr '\n' ' ')"
fi
kill "$DEV_PID" 2>/dev/null || true
wait "$DEV_PID" 2>/dev/null || true
DEV_PID=""

# ============================================================ TICKET GATE
GATE_TICKET="ticket_single_use_origin_bot_exclusion"

create_object() {
  local title="$1"
  curl -sS -X POST "$BASE/api/v1/workspaces/$WORKSPACE_ID/flow/objects" \
    -H "Authorization: Bearer $ACCESS_TOKEN" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg t "$title" --arg k "$(uuid)" '{object_type:"page", title:$t, idempotency_key:$k}')"
}

issue_ticket() {
  # $1 = document_id, $2 = client_id, $3 = origin, $4 = auth header value
  curl -sS -X POST "$BASE/api/v1/collab/tickets" \
    -H "Authorization: $4" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg w "$WORKSPACE_ID" --arg d "$1" --arg c "$2" --arg o "$3" \
      '{workspace_id:$w, document_id:$d, client_id:$c, origin:$o}')"
}

sql1() { psql "$DATABASE_URL" -Atc "$1"; }

# One document per fixture, so a check can never be contaminated by another
# check's writes.
declare -A DOC OBJ
for name in bot_probe ttl replay origin client expiry ctrl revoke mismatch_a mismatch_b relay_src; do
  resp="$(create_object "transport-verify-$name")"
  OBJ[$name]="$(jq -r '.data.object.id // empty' <<<"$resp")"
  DOC[$name]="$(jq -r '.data.object.document_id // empty' <<<"$resp")"
  if [[ -z "${DOC[$name]}" ]]; then
    echo "FAIL: could not create fixture object '$name': $resp" >&2
    cat "$API_LOG" >&2
    exit 2
  fi
done

# --- NEGATIVE: a bot token may not obtain a direct-WS ticket at all -------
BOT_TICKET_RESP="$(issue_ticket "${DOC[bot_probe]}" "bot-client-$RUN_ID" "$ORIGIN_A" "Bearer $BOT_TOKEN")"
BOT_CODE="$(jq -r '.code // "missing"' <<<"$BOT_TICKET_RESP")"
BOT_ROWS="$(sql1 "SELECT count(*) FROM collab_tickets WHERE document_id='${DOC[bot_probe]}'")"
if [[ "$BOT_CODE" == "403" && "$BOT_ROWS" == "0" ]]; then
  record "bot_ticket_issuance_rejected_zero_write" "$GATE_TICKET" true \
    "bot token POST /collab/tickets => envelope code 403 AND 0 collab_tickets rows" \
    "code=$BOT_CODE rows=$BOT_ROWS"
else
  record "bot_ticket_issuance_rejected_zero_write" "$GATE_TICKET" false \
    "bot token POST /collab/tickets => envelope code 403 AND 0 collab_tickets rows" \
    "code=$BOT_CODE rows=$BOT_ROWS body=$(head -c 200 <<<"$BOT_TICKET_RESP")"
fi

# --- POSITIVE CONTROL: the same call as a user succeeds -------------------
TTL_CLIENT="ttl-client-$RUN_ID"
TTL_RESP="$(issue_ticket "${DOC[ttl]}" "$TTL_CLIENT" "$ORIGIN_A" "Bearer $ACCESS_TOKEN")"
TTL_TICKET="$(jq -r '.data.ticket // empty' <<<"$TTL_RESP")"
if [[ -z "$TTL_TICKET" ]]; then
  record "user_ticket_issuance_control" "$GATE_TICKET" false \
    "an authorized user CAN issue a ticket (control for the bot negative above)" \
    "no ticket in response: $(head -c 200 <<<"$TTL_RESP")"
else
  record "user_ticket_issuance_control" "$GATE_TICKET" true \
    "an authorized user CAN issue a ticket (control for the bot negative above)" \
    "ticket issued for document ${DOC[ttl]}"
fi

TTL_SECONDS="$(sql1 "SELECT round(extract(epoch FROM (expires_at - created_at)))::text FROM collab_tickets WHERE document_id='${DOC[ttl]}' ORDER BY created_at DESC LIMIT 1")"
if [[ "$TTL_SECONDS" == "60" ]]; then
  record "ticket_ttl_is_60s" "$GATE_TICKET" true "expires_at - created_at == 60s (ADR-0007)" "$TTL_SECONDS"
else
  record "ticket_ttl_is_60s" "$GATE_TICKET" false "expires_at - created_at == 60s (ADR-0007)" "${TTL_SECONDS:-missing}"
fi

TTL_HASH_DB="$(sql1 "SELECT ticket_hash FROM collab_tickets WHERE document_id='${DOC[ttl]}' ORDER BY created_at DESC LIMIT 1")"
TTL_HASH_LOCAL="$(printf '%s' "$TTL_TICKET" | sha256sum | awk '{print $1}')"
TTL_ROW_TEXT="$(sql1 "SELECT collab_tickets::text FROM collab_tickets WHERE document_id='${DOC[ttl]}' ORDER BY created_at DESC LIMIT 1")"
RAW_IN_ROW="no"
if [[ -n "$TTL_TICKET" && "$TTL_ROW_TEXT" == *"$TTL_TICKET"* ]]; then RAW_IN_ROW="yes"; fi
if [[ "$TTL_HASH_DB" == "$TTL_HASH_LOCAL" && "$RAW_IN_ROW" == "no" ]]; then
  record "ticket_persisted_hash_only" "$GATE_TICKET" true \
    "collab_tickets stores sha256(raw) and no column holds the raw secret" \
    "ticket_hash matches sha256(raw); raw absent from the whole row"
else
  record "ticket_persisted_hash_only" "$GATE_TICKET" false \
    "collab_tickets stores sha256(raw) and no column holds the raw secret" \
    "db_hash=$TTL_HASH_DB local_hash=$TTL_HASH_LOCAL raw_present_in_row=$RAW_IN_ROW"
fi

# --- NEGATIVE: an Origin outside the allowlist cannot get a ticket --------
EVIL_RESP="$(issue_ticket "${DOC[origin]}" "origin-client-$RUN_ID" "$ORIGIN_EVIL" "Bearer $ACCESS_TOKEN")"
EVIL_CODE="$(jq -r '.code // "missing"' <<<"$EVIL_RESP")"
EVIL_ROWS="$(sql1 "SELECT count(*) FROM collab_tickets WHERE document_id='${DOC[origin]}'")"
if [[ "$EVIL_CODE" == "403" && "$EVIL_ROWS" == "0" ]]; then
  record "non_allowlisted_origin_issuance_rejected_zero_write" "$GATE_TICKET" true \
    "issuance with an Origin outside flow.collab_allowed_origins => envelope code 403 AND 0 rows" \
    "code=$EVIL_CODE rows=$EVIL_ROWS"
else
  record "non_allowlisted_origin_issuance_rejected_zero_write" "$GATE_TICKET" false \
    "issuance with an Origin outside flow.collab_allowed_origins => envelope code 403 AND 0 rows" \
    "code=$EVIL_CODE rows=$EVIL_ROWS body=$(head -c 200 <<<"$EVIL_RESP")"
fi

ws_probe() {
  # stdin: the probe spec. Fails hard on a probe-level error so a broken
  # probe is never mistaken for an observed rejection.
  local out
  out="$(python3 "$WS_PROBE")" || true
  if [[ -z "$out" ]]; then
    echo '{"upgrade":null,"events":[],"error":"probe produced no output"}'
    return 0
  fi
  printf '%s\n' "$out"
}

upgrade_only() {
  # $1 = ticket, $2 = client_id, $3 = Origin header
  jq -n --arg t "$1" --arg c "$2" --arg o "$3" --argjson p "$API_PORT" \
    '{host:"127.0.0.1", port:$p, origin:$o, read_timeout:8,
      path:("/api/v1/collab/ws?ticket=" + ($t|@uri) + "&client_id=" + ($c|@uri)), steps:[]}' \
    | ws_probe
}

# --- NEGATIVE: the ticket's Origin binding, isolated from the allowlist ---
# ORIGIN_B is itself allowlisted, so the only thing this fixture can be
# rejected for is the per-ticket binding.
ORIGIN_CLIENT="origin-bind-client-$RUN_ID"
ORIGIN_TICKET="$(jq -r '.data.ticket // empty' <<<"$(issue_ticket "${DOC[client]}" "$ORIGIN_CLIENT" "$ORIGIN_A" "Bearer $ACCESS_TOKEN")")"
ORIGIN_RESULT="$(upgrade_only "$ORIGIN_TICKET" "$ORIGIN_CLIENT" "$ORIGIN_B")"
ORIGIN_STATUS="$(jq -r '.upgrade.http_status // "none"' <<<"$ORIGIN_RESULT")"
ORIGIN_ENVELOPE="$(jq -r '.upgrade.body_json.code // "none"' <<<"$ORIGIN_RESULT")"
ORIGIN_CONSUMED="$(sql1 "SELECT coalesce(consumed_at::text,'NULL') FROM collab_tickets WHERE document_id='${DOC[client]}' ORDER BY created_at DESC LIMIT 1")"
if [[ "$ORIGIN_STATUS" != "101" && "$ORIGIN_ENVELOPE" == "401" && "$ORIGIN_CONSUMED" == "NULL" ]]; then
  record "wrong_origin_upgrade_rejected_before_consumption" "$GATE_TICKET" true \
    "upgrade with a different (but allowlisted) Origin => no 101, envelope code 401, consumed_at still NULL" \
    "http=$ORIGIN_STATUS envelope=$ORIGIN_ENVELOPE consumed_at=$ORIGIN_CONSUMED"
else
  record "wrong_origin_upgrade_rejected_before_consumption" "$GATE_TICKET" false \
    "upgrade with a different (but allowlisted) Origin => no 101, envelope code 401, consumed_at still NULL" \
    "http=$ORIGIN_STATUS envelope=$ORIGIN_ENVELOPE consumed_at=$ORIGIN_CONSUMED probe_error=$(jq -r '.error // "none"' <<<"$ORIGIN_RESULT")"
fi

# --- NEGATIVE: the ticket's client_id binding ----------------------------
CID_CLIENT="cid-client-$RUN_ID"
CID_TICKET="$(jq -r '.data.ticket // empty' <<<"$(issue_ticket "${DOC[expiry]}" "$CID_CLIENT" "$ORIGIN_A" "Bearer $ACCESS_TOKEN")")"
CID_RESULT="$(upgrade_only "$CID_TICKET" "someone-elses-client-$RUN_ID" "$ORIGIN_A")"
CID_STATUS="$(jq -r '.upgrade.http_status // "none"' <<<"$CID_RESULT")"
CID_ENVELOPE="$(jq -r '.upgrade.body_json.code // "none"' <<<"$CID_RESULT")"
CID_CONSUMED="$(sql1 "SELECT coalesce(consumed_at::text,'NULL') FROM collab_tickets WHERE document_id='${DOC[expiry]}' ORDER BY created_at DESC LIMIT 1")"
if [[ "$CID_STATUS" != "101" && "$CID_ENVELOPE" == "401" && "$CID_CONSUMED" == "NULL" ]]; then
  record "wrong_client_id_upgrade_rejected_before_consumption" "$GATE_TICKET" true \
    "upgrade with a client_id the ticket was not bound to => no 101, envelope code 401, consumed_at still NULL" \
    "http=$CID_STATUS envelope=$CID_ENVELOPE consumed_at=$CID_CONSUMED"
else
  record "wrong_client_id_upgrade_rejected_before_consumption" "$GATE_TICKET" false \
    "upgrade with a client_id the ticket was not bound to => no 101, envelope code 401, consumed_at still NULL" \
    "http=$CID_STATUS envelope=$CID_ENVELOPE consumed_at=$CID_CONSUMED probe_error=$(jq -r '.error // "none"' <<<"$CID_RESULT")"
fi

# --- NEGATIVE: an expired ticket -----------------------------------------
EXP_CLIENT="exp-client-$RUN_ID"
EXP_TICKET="$(jq -r '.data.ticket // empty' <<<"$(issue_ticket "${DOC[origin]}" "$EXP_CLIENT" "$ORIGIN_A" "Bearer $ACCESS_TOKEN")")"
EXP_HASH="$(printf '%s' "$EXP_TICKET" | sha256sum | awk '{print $1}')"
# Shift the whole row back in time rather than only `expires_at`: the table's
# own `collab_tickets_expiry_check` pins the TTL window to 60s, so the only
# way to model "issued two minutes ago and now expired" is to move issuance
# and expiry together -- which is exactly what such a row looks like.
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c \
  "UPDATE collab_tickets SET created_at = created_at - interval '120 seconds', expires_at = expires_at - interval '120 seconds' WHERE ticket_hash = '$EXP_HASH'"
EXP_RESULT="$(upgrade_only "$EXP_TICKET" "$EXP_CLIENT" "$ORIGIN_A")"
EXP_STATUS="$(jq -r '.upgrade.http_status // "none"' <<<"$EXP_RESULT")"
EXP_ENVELOPE="$(jq -r '.upgrade.body_json.code // "none"' <<<"$EXP_RESULT")"
EXP_CONSUMED="$(sql1 "SELECT coalesce(consumed_at::text,'NULL') FROM collab_tickets WHERE ticket_hash='$EXP_HASH'")"
if [[ "$EXP_STATUS" != "101" && "$EXP_ENVELOPE" == "401" && "$EXP_CONSUMED" == "NULL" ]]; then
  record "expired_ticket_upgrade_rejected_before_consumption" "$GATE_TICKET" true \
    "an expired ticket => no 101, envelope code 401, consumed_at still NULL" \
    "http=$EXP_STATUS envelope=$EXP_ENVELOPE consumed_at=$EXP_CONSUMED"
else
  record "expired_ticket_upgrade_rejected_before_consumption" "$GATE_TICKET" false \
    "an expired ticket => no 101, envelope code 401, consumed_at still NULL" \
    "http=$EXP_STATUS envelope=$EXP_ENVELOPE consumed_at=$EXP_CONSUMED probe_error=$(jq -r '.error // "none"' <<<"$EXP_RESULT")"
fi

# --- NEGATIVE: single use. First upgrade is the positive control ---------
RP_CLIENT="replay-client-$RUN_ID"
RP_TICKET="$(jq -r '.data.ticket // empty' <<<"$(issue_ticket "${DOC[replay]}" "$RP_CLIENT" "$ORIGIN_A" "Bearer $ACCESS_TOKEN")")"
RP_HASH="$(printf '%s' "$RP_TICKET" | sha256sum | awk '{print $1}')"
RP_FIRST="$(upgrade_only "$RP_TICKET" "$RP_CLIENT" "$ORIGIN_A")"
RP_FIRST_STATUS="$(jq -r '.upgrade.http_status // "none"' <<<"$RP_FIRST")"
RP_FIRST_ACCEPT="$(jq -r '.upgrade.accept_ok // false' <<<"$RP_FIRST")"
RP_CONSUMED_1="$(sql1 "SELECT coalesce(consumed_at::text,'NULL') FROM collab_tickets WHERE ticket_hash='$RP_HASH'")"
if [[ "$RP_FIRST_STATUS" == "101" && "$RP_FIRST_ACCEPT" == "true" && "$RP_CONSUMED_1" != "NULL" ]]; then
  record "valid_ticket_upgrade_control" "$GATE_TICKET" true \
    "a correctly bound ticket upgrades: 101, RFC 6455 Sec-WebSocket-Accept matches, consumed_at set" \
    "http=$RP_FIRST_STATUS accept_ok=$RP_FIRST_ACCEPT consumed_at=$RP_CONSUMED_1"
else
  record "valid_ticket_upgrade_control" "$GATE_TICKET" false \
    "a correctly bound ticket upgrades: 101, RFC 6455 Sec-WebSocket-Accept matches, consumed_at set" \
    "http=$RP_FIRST_STATUS accept_ok=$RP_FIRST_ACCEPT consumed_at=$RP_CONSUMED_1 probe_error=$(jq -r '.error // "none"' <<<"$RP_FIRST")"
fi
RP_SECOND="$(upgrade_only "$RP_TICKET" "$RP_CLIENT" "$ORIGIN_A")"
RP_SECOND_STATUS="$(jq -r '.upgrade.http_status // "none"' <<<"$RP_SECOND")"
RP_SECOND_ENVELOPE="$(jq -r '.upgrade.body_json.code // "none"' <<<"$RP_SECOND")"
RP_CONSUMED_2="$(sql1 "SELECT coalesce(consumed_at::text,'NULL') FROM collab_tickets WHERE ticket_hash='$RP_HASH'")"
if [[ "$RP_SECOND_STATUS" != "101" && "$RP_SECOND_ENVELOPE" == "401" && "$RP_CONSUMED_2" == "$RP_CONSUMED_1" ]]; then
  record "ticket_replay_rejected_and_not_reconsumed" "$GATE_TICKET" true \
    "replaying an already consumed ticket => no 101, envelope code 401, consumed_at unchanged" \
    "http=$RP_SECOND_STATUS envelope=$RP_SECOND_ENVELOPE consumed_at unchanged ($RP_CONSUMED_2)"
else
  record "ticket_replay_rejected_and_not_reconsumed" "$GATE_TICKET" false \
    "replaying an already consumed ticket => no 101, envelope code 401, consumed_at unchanged" \
    "http=$RP_SECOND_STATUS envelope=$RP_SECOND_ENVELOPE consumed_at before=$RP_CONSUMED_1 after=$RP_CONSUMED_2"
fi

# --- ticket must not reach the log ---------------------------------------
LEAKED="no"
for t in "$TTL_TICKET" "$RP_TICKET" "$EXP_TICKET" "$ORIGIN_TICKET" "$CID_TICKET"; do
  if [[ -n "$t" ]] && grep -qF -- "$t" "$API_LOG"; then LEAKED="yes"; fi
done
assert_eq "raw_ticket_absent_from_server_log" "$GATE_TICKET" "no" "$LEAKED" \
  "none of the 5 raw tickets issued this run appears anywhere in the api log"

# ============================================== UNAUTHORIZED UPDATE GATE
GATE_UPDATE="unauthorized_update_rejected"

# A CRDT update this server will genuinely accept, produced by the product's
# own REST command path on a sibling document. Using real engine output --
# rather than random bytes -- is what makes the rejection fixtures below
# non-vacuous: the ONLY reason the revoked session's write can fail is
# authorization, because the identical bytes are accepted on the control
# path first.
CMD_RESP="$(curl -sS -X POST "$BASE/api/v1/flow/objects/${OBJ[relay_src]}/commands" \
  -H "Authorization: Bearer $ACCESS_TOKEN" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg k "$(uuid)" '{command:{type:"set_title", payload:{title:"transport verify payload"}}, idempotency_key:$k}')")"
if [[ "$(jq -r '.code // "missing"' <<<"$CMD_RESP")" != "0" ]]; then
  echo "FAIL: could not produce a real CRDT update via the REST command path: $CMD_RESP" >&2
  exit 2
fi
UPDATE_BYTES="$(sql1 "SELECT encode(bytes,'base64') FROM collab_updates WHERE document_id='${DOC[relay_src]}' ORDER BY seq DESC LIMIT 1" | tr -d '\n')"
if [[ -z "$UPDATE_BYTES" ]]; then
  echo "FAIL: the REST command produced no collab_updates row to harvest bytes from" >&2
  exit 2
fi

doc_write_counts() {
  # collab_updates + content business_events + their dispatch rows, for one document.
  psql "$DATABASE_URL" -Atc "
    SELECT (SELECT count(*) FROM collab_updates WHERE document_id='$1')
        || '/' || (SELECT count(*) FROM business_events WHERE aggregate_id='$1' AND event_type='flow.content.accepted')
        || '/' || (SELECT count(*) FROM event_dispatch d JOIN business_events e ON e.id=d.event_id WHERE e.aggregate_id='$1')
        || '/' || (SELECT head_seq::text FROM collab_documents WHERE id='$1')"
}

session_spec() {
  # $1 ticket, $2 client_id, $3 open document_id, $4 update document_id,
  # $5 update bytes, $6 extra SQL to run between open and update ('' = none),
  # $7 the update frame's self-reported `origin`
  local extra_step='[]'
  if [[ -n "$6" ]]; then
    extra_step="$(jq -n --arg sql "$6" '[{op:"run_sql", sql:$sql}]')"
  fi
  jq -n --arg t "$1" --arg c "$2" --arg opendoc "$3" --arg upddoc "$4" \
    --arg bytes "$5" --arg origin "$7" --arg db "$DATABASE_URL" \
    --arg sid "$(uuid)" --arg uid "$(uuid)" --argjson p "$API_PORT" \
    --arg wsorigin "$ORIGIN_A" --argjson extra "$extra_step" \
    '{host:"127.0.0.1", port:$p, origin:$wsorigin, read_timeout:12, database_url:$db,
      path:("/api/v1/collab/ws?ticket=" + ($t|@uri) + "&client_id=" + ($c|@uri)),
      steps: ([{op:"send", frame:{type:"hello", protocol_version:1, capabilities:[], client_id:$c, session_id:$sid}},
               {op:"recv", count:1},
               {op:"send", frame:{type:"open", protocol_version:1, document_id:$opendoc, known_seq:null, known_frontier:null}},
               {op:"recv", count:1}]
              + $extra
              + [{op:"send", frame:{type:"update", protocol_version:1, document_id:$upddoc, update_id:$uid,
                                   base_frontier:"", bytes:$bytes, idempotency_key:null, origin:$origin, message:null}},
                 {op:"recv", count:2}])}' \
    | ws_probe
}

last_rejected_code() { jq -r '[.events[] | select(.op=="recv" and .kind=="text") | .frame | select(.type=="rejected") | .code] | last // "none"' <<<"$1"; }
has_accepted() { jq -r '[.events[] | select(.op=="recv" and .kind=="text") | .frame | select(.type=="accepted")] | length' <<<"$1"; }

# --- POSITIVE CONTROL: an authorized session's update is accepted --------
CTRL_CLIENT="ctrl-client-$RUN_ID"
CTRL_TICKET="$(jq -r '.data.ticket // empty' <<<"$(issue_ticket "${DOC[ctrl]}" "$CTRL_CLIENT" "$ORIGIN_A" "Bearer $ACCESS_TOKEN")")"
CTRL_BEFORE="$(doc_write_counts "${DOC[ctrl]}")"
CTRL_RESULT="$(session_spec "$CTRL_TICKET" "$CTRL_CLIENT" "${DOC[ctrl]}" "${DOC[ctrl]}" "$UPDATE_BYTES" "" "cli")"
CTRL_ACCEPTED="$(has_accepted "$CTRL_RESULT")"
CTRL_AFTER="$(doc_write_counts "${DOC[ctrl]}")"
if [[ "$CTRL_ACCEPTED" == "1" && "$CTRL_BEFORE" == "0/0/0/0" && "$CTRL_AFTER" == "1/1/1/1" ]]; then
  record "authorized_update_accepted_control" "$GATE_UPDATE" true \
    "an authorized ticket-bound session's update with these exact bytes IS accepted and writes exactly one update/event/dispatch row" \
    "accepted=$CTRL_ACCEPTED counts updates/events/dispatch/head_seq before=$CTRL_BEFORE after=$CTRL_AFTER"
else
  record "authorized_update_accepted_control" "$GATE_UPDATE" false \
    "an authorized ticket-bound session's update with these exact bytes IS accepted and writes exactly one update/event/dispatch row" \
    "accepted=$CTRL_ACCEPTED counts before=$CTRL_BEFORE after=$CTRL_AFTER rejected=$(last_rejected_code "$CTRL_RESULT") probe_error=$(jq -r '.error // "none"' <<<"$CTRL_RESULT")"
fi

# The same control run also proves the caller cannot dictate its own actor
# or origin: the update frame above self-reported origin "cli", and the
# session never told the server who the actor was.
CTRL_STORED="$(sql1 "SELECT actor_id::text || '/' || origin_surface || '/' || coalesce(origin_client_id,'NULL') FROM collab_updates WHERE document_id='${DOC[ctrl]}' ORDER BY seq DESC LIMIT 1")"
if [[ "$CTRL_STORED" == "$OWNER_USER/web/$CTRL_CLIENT" ]]; then
  record "actor_and_origin_come_from_the_ticket_not_the_frame" "$GATE_UPDATE" true \
    "stored actor_id/origin_surface/origin_client_id == ticket user / 'web' / ticket client_id, despite the frame self-reporting origin='cli'" \
    "$CTRL_STORED"
else
  record "actor_and_origin_come_from_the_ticket_not_the_frame" "$GATE_UPDATE" false \
    "stored actor_id/origin_surface/origin_client_id == $OWNER_USER/web/$CTRL_CLIENT despite the frame self-reporting origin='cli'" \
    "$CTRL_STORED"
fi

# --- NEGATIVE: a member revoked mid-session may not write ----------------
# The revocation happens AFTER `open` has already banked this session's
# `checked_epoch`, which is the only ordering under which the write path's
# commit-time fence is what rejects the update.
REV_CLIENT="revoke-client-$RUN_ID"
REV_TICKET="$(jq -r '.data.ticket // empty' <<<"$(issue_ticket "${DOC[revoke]}" "$REV_CLIENT" "$ORIGIN_A" "Bearer $ACCESS_TOKEN")")"
REV_BEFORE="$(doc_write_counts "${DOC[revoke]}")"
REVOKE_SQL="DELETE FROM workspace_members WHERE workspace_id='$WORKSPACE_ID' AND user_id='$OWNER_USER'; UPDATE flow_workspace_settings SET authz_epoch = authz_epoch + 1 WHERE workspace_id='$WORKSPACE_ID'"
REV_RESULT="$(session_spec "$REV_TICKET" "$REV_CLIENT" "${DOC[revoke]}" "${DOC[revoke]}" "$UPDATE_BYTES" "$REVOKE_SQL" "web")"
REV_CODE="$(last_rejected_code "$REV_RESULT")"
REV_ACCEPTED="$(has_accepted "$REV_RESULT")"
REV_AFTER="$(doc_write_counts "${DOC[revoke]}")"
# Restore membership so the remaining fixtures still have an authorized user.
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c \
  "INSERT INTO workspace_members (workspace_id, user_id, role, created_at) VALUES ('$WORKSPACE_ID','$OWNER_USER','owner',now()) ON CONFLICT DO NOTHING"
if [[ "$REV_CODE" == "policy_rejected" && "$REV_ACCEPTED" == "0" && "$REV_AFTER" == "$REV_BEFORE" ]]; then
  record "revoked_member_update_rejected_zero_write" "$GATE_UPDATE" true \
    "membership deleted + authz_epoch bumped after open => rejected.code=policy_rejected, no accepted frame, and updates/events/dispatch/head_seq unchanged" \
    "code=$REV_CODE accepted=$REV_ACCEPTED counts before=$REV_BEFORE after=$REV_AFTER"
else
  record "revoked_member_update_rejected_zero_write" "$GATE_UPDATE" false \
    "membership deleted + authz_epoch bumped after open => rejected.code=policy_rejected, no accepted frame, and updates/events/dispatch/head_seq unchanged" \
    "code=$REV_CODE accepted=$REV_ACCEPTED counts before=$REV_BEFORE after=$REV_AFTER probe_error=$(jq -r '.error // "none"' <<<"$REV_RESULT")"
fi

# --- NEGATIVE: opening a document the ticket was not issued for ----------
MM_CLIENT="mismatch-client-$RUN_ID"
MM_TICKET="$(jq -r '.data.ticket // empty' <<<"$(issue_ticket "${DOC[mismatch_a]}" "$MM_CLIENT" "$ORIGIN_A" "Bearer $ACCESS_TOKEN")")"
MM_B_BEFORE="$(doc_write_counts "${DOC[mismatch_b]}")"
MM_RESULT="$(session_spec "$MM_TICKET" "$MM_CLIENT" "${DOC[mismatch_b]}" "${DOC[mismatch_b]}" "$UPDATE_BYTES" "" "web")"
MM_CODE="$(jq -r '[.events[] | select(.op=="recv" and .kind=="text") | .frame | select(.type=="rejected") | .code] | first // "none"' <<<"$MM_RESULT")"
MM_ACCEPTED="$(has_accepted "$MM_RESULT")"
MM_B_AFTER="$(doc_write_counts "${DOC[mismatch_b]}")"
if [[ "$MM_CODE" == "forbidden" && "$MM_ACCEPTED" == "0" && "$MM_B_AFTER" == "$MM_B_BEFORE" ]]; then
  record "open_document_not_bound_to_ticket_rejected_zero_write" "$GATE_UPDATE" true \
    "open.document_id != the ticket's document => rejected.code=forbidden, no accepted frame, target document untouched" \
    "code=$MM_CODE accepted=$MM_ACCEPTED counts before=$MM_B_BEFORE after=$MM_B_AFTER"
else
  record "open_document_not_bound_to_ticket_rejected_zero_write" "$GATE_UPDATE" false \
    "open.document_id != the ticket's document => rejected.code=forbidden, no accepted frame, target document untouched" \
    "code=$MM_CODE accepted=$MM_ACCEPTED counts before=$MM_B_BEFORE after=$MM_B_AFTER probe_error=$(jq -r '.error // "none"' <<<"$MM_RESULT")"
fi

# --- NEGATIVE: an update frame aimed at another document ------------------
UM_CLIENT="updmismatch-client-$RUN_ID"
UM_TICKET="$(jq -r '.data.ticket // empty' <<<"$(issue_ticket "${DOC[mismatch_a]}" "$UM_CLIENT" "$ORIGIN_A" "Bearer $ACCESS_TOKEN")")"
UM_A_BEFORE="$(doc_write_counts "${DOC[mismatch_a]}")"
UM_B_BEFORE="$(doc_write_counts "${DOC[mismatch_b]}")"
UM_RESULT="$(session_spec "$UM_TICKET" "$UM_CLIENT" "${DOC[mismatch_a]}" "${DOC[mismatch_b]}" "$UPDATE_BYTES" "" "web")"
UM_CODE="$(last_rejected_code "$UM_RESULT")"
UM_ACCEPTED="$(has_accepted "$UM_RESULT")"
UM_A_AFTER="$(doc_write_counts "${DOC[mismatch_a]}")"
UM_B_AFTER="$(doc_write_counts "${DOC[mismatch_b]}")"
if [[ "$UM_CODE" == "invalid_update" && "$UM_ACCEPTED" == "0" && "$UM_A_AFTER" == "$UM_A_BEFORE" && "$UM_B_AFTER" == "$UM_B_BEFORE" ]]; then
  record "update_frame_for_another_document_rejected_zero_write" "$GATE_UPDATE" true \
    "update.document_id != the session's document => rejected.code=invalid_update, no accepted frame, neither document written" \
    "code=$UM_CODE accepted=$UM_ACCEPTED a=$UM_A_BEFORE->$UM_A_AFTER b=$UM_B_BEFORE->$UM_B_AFTER"
else
  record "update_frame_for_another_document_rejected_zero_write" "$GATE_UPDATE" false \
    "update.document_id != the session's document => rejected.code=invalid_update, no accepted frame, neither document written" \
    "code=$UM_CODE accepted=$UM_ACCEPTED a=$UM_A_BEFORE->$UM_A_AFTER b=$UM_B_BEFORE->$UM_B_AFTER probe_error=$(jq -r '.error // "none"' <<<"$UM_RESULT")"
fi

# ============================================================ gate rollup
# A gate passes only when every check id it requires is present in the
# evidence AND passed. A missing id is a failure, never an absence: that is
# what stops a fixture that silently never ran from reading as green.
gate_verdict() {
  local gate="$1"; shift
  local required=("$@") missing=() failed=()
  for id in "${required[@]}"; do
    local entry
    entry="$(jq -r --arg id "$id" '[.[] | select(.id==$id)] | if length==0 then "missing" else (.[0].passed|tostring) end' <<<"$CHECKS_JSON")"
    case "$entry" in
      missing) missing+=("$id") ;;
      false) failed+=("$id") ;;
      true) ;;
      *) failed+=("$id(unparsable)") ;;
    esac
  done
  if [[ ${#missing[@]} -eq 0 && ${#failed[@]} -eq 0 ]]; then
    jq -n --arg g "$gate" --argjson n "${#required[@]}" \
      '{status:"passed", reason:("all " + ($n|tostring) + " required negative/control checks passed")}'
  else
    jq -n --arg missing "${missing[*]-}" --arg failed "${failed[*]-}" \
      '{status:"failed", reason:("failed=[" + $failed + "] missing=[" + $missing + "]")}'
  fi
}

TICKET_VERDICT="$(gate_verdict "$GATE_TICKET" \
  bot_ticket_issuance_rejected_zero_write \
  user_ticket_issuance_control \
  ticket_ttl_is_60s \
  ticket_persisted_hash_only \
  non_allowlisted_origin_issuance_rejected_zero_write \
  wrong_origin_upgrade_rejected_before_consumption \
  wrong_client_id_upgrade_rejected_before_consumption \
  expired_ticket_upgrade_rejected_before_consumption \
  valid_ticket_upgrade_control \
  ticket_replay_rejected_and_not_reconsumed \
  raw_ticket_absent_from_server_log)"

COOKIE_VERDICT="$(gate_verdict "$GATE_COOKIE" \
  default_cookies_secure_httponly_samesite \
  clear_cookies_secure \
  forwarded_proto_header_cannot_downgrade_cookie \
  env_var_cannot_downgrade_cookie \
  non_loopback_insecure_cookie_refuses_to_start \
  loopback_dev_exception_scoped_and_effective)"

UPDATE_VERDICT="$(gate_verdict "$GATE_UPDATE" \
  authorized_update_accepted_control \
  actor_and_origin_come_from_the_ticket_not_the_frame \
  revoked_member_update_rejected_zero_write \
  open_document_not_bound_to_ticket_rejected_zero_write \
  update_frame_for_another_document_rejected_zero_write)"

RESULT="$(jq -n \
  --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" --arg adr "$ADR_PATH" \
  --argjson checks "$CHECKS_JSON" \
  --argjson ticket "$TICKET_VERDICT" --argjson cookie "$COOKIE_VERDICT" --argjson upd "$UPDATE_VERDICT" \
  '{
    schema_version: "sylvode.flow.transport-auth-result.v1",
    source_head: $head,
    generated_at: $generated_at,
    adr: $adr,
    gates: {
      ticket_single_use_origin_bot_exclusion: $ticket,
      secure_cookie_and_local_dev_guard: $cookie,
      unauthorized_update_rejected: $upd
    },
    checks: $checks,
    counts: {
      checks_total: ($checks | length),
      checks_passed: ([$checks[] | select(.passed)] | length),
      checks_failed: ([$checks[] | select(.passed | not)] | length)
    },
    passed: ([$ticket, $cookie, $upd] | all(.status == "passed"))
  }')"

OUT_PATH="$EVIDENCE_ROOT/transport-auth-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$RESULT" | jq . > "$OUT_TMP"
sync "$OUT_TMP" 2>/dev/null || true
mv -f "$OUT_TMP" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2

echo "$RESULT"
if [[ "$(jq -r '.passed' <<<"$RESULT")" == "true" ]]; then
  exit 0
fi
exit 1
