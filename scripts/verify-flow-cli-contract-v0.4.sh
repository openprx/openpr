#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 CLI JSON-envelope and exit-code contract verifier.
#
# Contract: /opt/working/sylvode-flow/contracts/cli-surface-v1.md ("命名与格式":
# the `sylvode.cli.v1` success/failure envelope, "JSON 成功/失败均只写 stdout，
# 诊断日志只写 stderr", "全局 --format json|table 只决定 CLI envelope/显示") and
# contracts/error-mapping-v1.md "CLI 退出码全集" (the 0/2/3/4/5/6/7/8/9/10
# table), for gates/v0.4-gate.yaml's `cli_json_and_exit_code_contract`.
#
# Two halves, neither of which trusts a self-report:
#
# STATIC -- the exit-code table is parsed out of error-mapping-v1.md and
# compared against the `exit` constants the shipped binary actually compiles
# (`apps/mcp-server/src/cli_app/error.rs`, read-only). A constant that drifted
# from the contract is a violation even if every live fixture happens to pass,
# because the untested codes would then be wrong too.
#
# LIVE -- the real `api` binary is booted against a scratch database, and the
# real `sylvode` binary is run against it once per fixture. For every fixture
# the process exit status, the exact stdout bytes and the exact stderr bytes
# are captured and asserted:
#
#   F0  a successful read exits 0 and prints exactly one `sylvode.cli.v1`
#       object with ok=true, the right `command`, a `data` member and a UUID
#       `request_id`.
#   F2  a malformed argument exits 2 AND never reaches the network -- proven
#       by pointing --api-url at a closed port: a request that was actually
#       sent would come back as exit 9 instead.
#   F3  a bad bot token exits 3.
#   F4  a workspace with the Flow flag off exits 4.
#   F5  an unknown object id exits 5.
#   F9  a valid command against an unreachable API exits 9.
#   F10 `collab verify --expected-head <wrong>` exits 10: the call itself
#       succeeded but reported an inconsistency.
#   FJ  every failing fixture writes its `sylvode.cli.v1` failure envelope to
#       stdout and nothing resembling an envelope to stderr.
#   FT  `--format table` changes only the rendering: the same fixture keeps
#       the same exit code, and stdout is no longer the JSON envelope.
#
# NOT covered, and recorded rather than quietly dropped: exits 6 (stale /
# resync), 7 (protocol / invalid update) and 8 (limit) have no v0.4 CLI command
# that can provoke them from outside -- the v0.4 `sylvode` surface is
# read-plus-feature-flag, and `error-mapping-v1.md` maps those three to
# collaboration-write and package paths that arrive in later releases. They are
# listed in `exit_codes_not_exercised` with that reason; the static half still
# pins their constants.
#
# Exit codes: 0 = every assertion passed, 1 = an assertion failed,
# 2 = usage/tool/environment error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/flow_contract_path.sh
source "$ROOT_DIR/scripts/lib/flow_contract_path.sh"
REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT=""
CONTRACT_PATH=""
RELEASE="0.4"
DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-}"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-cli-contract-v0.4.sh --contract PATH --release 0.4 --json [OPTIONS]

Compares the shipped `sylvode` binary's compiled exit-code constants against
error-mapping-v1.md's table, then boots the real api binary and runs the real
CLI once per fixture, asserting the exit status and the exact stdout/stderr
split for success, usage, unauthenticated, forbidden, not-found, network and
integrity-mismatch outcomes. Writes evidence/v0.4/cli-contract-result.json.

Options:
  --contract PATH        error-mapping-v1.md. Default:
                         <contracts-root>/contracts/error-mapping-v1.md
  --release X.Y          Recorded in the output. Default: 0.4
  --database-url URL     Postgres DSN this script may CREATE DATABASE on.
                         Default: $OPENPR_TEST_DATABASE_URL
  --repo-root DIR        Default: this checkout.
  --contracts-root DIR   Default: /opt/working/sylvode-flow
  --evidence-root DIR    Required. Evidence output directory.
  --json                 Required for CLI-contract compatibility.
  -h, --help             Show this help and exit 0.

Exit codes: 0 all assertions passed, 1 an assertion failed,
2 usage/tool/environment error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --contract) CONTRACT_PATH="${2:?--contract requires a PATH argument}"; shift 2 ;;
    --release) RELEASE="${2:?--release requires a value}"; shift 2 ;;
    --database-url) DATABASE_URL="${2:?--database-url requires a value}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ -n "$CONTRACT_PATH" ]] || CONTRACT_PATH="$CONTRACTS_ROOT/contracts/error-mapping-v1.md"

if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
if [[ -z "$EVIDENCE_ROOT" ]]; then
  echo "FAIL: --evidence-root is required; evidence must never default into the contract repository" >&2
  exit 2
fi
if ! CONTRACT_PATH="$(flow_resolve_contract_path --contract "$CONTRACT_PATH" "$CONTRACTS_ROOT")"; then
  exit 2
fi
if [[ -z "$DATABASE_URL" ]]; then
  echo "FAIL: no database URL configured (set --database-url or OPENPR_TEST_DATABASE_URL)" >&2
  exit 2
fi
for tool in jq git python3 psql curl cargo sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required command: $tool" >&2; exit 2; }
done
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
CONTRACT_SHA="$(sha256sum "$CONTRACT_PATH" | awk '{print $1}')"

VIOLATIONS=()

# ================= static: contract table vs compiled constants =================
echo "=== static: error-mapping-v1.md exit-code table vs cli_app::error::exit ===" >&2
# shellcheck disable=SC2016
STATIC_JSON="$(python3 -c '
import json, re, sys

contract_path, error_rs = sys.argv[1], sys.argv[2]
text = open(contract_path, encoding="utf-8").read()
violations = []

section = re.search(r"## CLI 退出码全集(.*?)(?=\n## |\Z)", text, re.S)
if not section:
    print(json.dumps({"error": "error-mapping-v1.md has no \"CLI 退出码全集\" section"}))
    sys.exit(0)
contract_codes = {}
for row in re.finditer(r"(?m)^\|\s*(\d+)\s*\|\s*([^|]+?)\s*\|", section.group(1)):
    contract_codes[int(row.group(1))] = row.group(2).strip()

src = open(error_rs, encoding="utf-8").read()
mod = re.search(r"pub mod exit \{(.*?)\n\}", src, re.S)
if not mod:
    print(json.dumps({"error": "cli_app::error has no `pub mod exit` block"}))
    sys.exit(0)
compiled = {m.group(1): int(m.group(2)) for m in re.finditer(r"pub const (\w+): i32 = (\d+);", mod.group(1))}

compiled_values = sorted(compiled.values())
contract_values = sorted(contract_codes)
if compiled_values != contract_values:
    violations.append(
        f"the compiled exit constants {compiled_values} do not match the error-mapping-v1.md table {contract_values} "
        f"(compiled-only: {sorted(set(compiled_values) - set(contract_values))}, "
        f"contract-only: {sorted(set(contract_values) - set(compiled_values))})"
    )
if len(set(compiled.values())) != len(compiled):
    violations.append(f"two exit constants share a value: {compiled}")

print(json.dumps({
    "contract_table": {str(k): v for k, v in sorted(contract_codes.items())},
    "compiled_constants": compiled,
    "violations": violations,
}))
' "$CONTRACT_PATH" "$REPO_ROOT/apps/mcp-server/src/cli_app/error.rs")"

jq -e . >/dev/null 2>&1 <<<"$STATIC_JSON" || { echo "FAIL: static parser produced no JSON: $STATIC_JSON" >&2; exit 2; }
if jq -e 'has("error")' >/dev/null 2>&1 <<<"$STATIC_JSON"; then
  echo "FAIL: $(jq -r '.error' <<<"$STATIC_JSON")" >&2
  exit 2
fi
while IFS= read -r v; do
  [[ -n "$v" ]] && VIOLATIONS+=("static: $v")
done < <(jq -r '.violations[]' <<<"$STATIC_JSON")

SCHEMA_CONST="$(grep -oP 'pub const SCHEMA_VERSION: &str = "\K[^"]+' "$REPO_ROOT/apps/mcp-server/src/cli_app/render.rs" || printf '')"
if [[ "$SCHEMA_CONST" != "sylvode.cli.v1" ]]; then
  VIOLATIONS+=("static: cli_app::render::SCHEMA_VERSION is '$SCHEMA_CONST', cli-surface-v1.md fixes it at 'sylvode.cli.v1'")
fi

# ================= live half =================
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
echo "=== building api and sylvode ===" >&2
( cd "$REPO_ROOT" && cargo build -q -p api --bin api -p mcp-server --bin sylvode ) || {
  echo "FAIL: binaries failed to build" >&2; exit 2; }
API_BIN="$TARGET_DIR/debug/api"
CLI_BIN="$TARGET_DIR/debug/sylvode"
[[ -x "$API_BIN" ]] || { echo "FAIL: api binary not found: $API_BIN" >&2; exit 2; }
[[ -x "$CLI_BIN" ]] || { echo "FAIL: sylvode binary not found: $CLI_BIN" >&2; exit 2; }

RUN_ID="$(python3 -c 'import uuid; print(uuid.uuid4().hex[:8])')"
SCRATCH_DB="openpr_flow_cli_verify_$RUN_ID"
DB_PREFIX="${DATABASE_URL%/*}"
SCRATCH_URL="$DB_PREFIX/$SCRATCH_DB"
TMP_DIR="$(mktemp -d "/tmp/openpr-flow-cli-verify.XXXXXX")"
API_PORT=$((20000 + RANDOM % 20000))
# A port nothing listens on: the fixture that must NOT reach the network and the
# fixture that must fail to connect both point here.
DEAD_PORT=$((1024 + RANDOM % 200))
API_PID=""

# shellcheck disable=SC2317  # invoked only via `trap ... EXIT`
cleanup() {
  local ec=$?
  if [[ -n "$API_PID" ]] && kill -0 "$API_PID" 2>/dev/null; then
    kill "$API_PID" 2>/dev/null || true
    wait "$API_PID" 2>/dev/null || true
  fi
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "DROP DATABASE IF EXISTS \"$SCRATCH_DB\" WITH (FORCE)" >/dev/null 2>&1 || true
  rm -rf "$TMP_DIR"
  exit "$ec"
}
trap cleanup EXIT

psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "DROP DATABASE IF EXISTS \"$SCRATCH_DB\" WITH (FORCE)" >/dev/null 2>&1 || true
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "CREATE DATABASE \"$SCRATCH_DB\"" >/dev/null

JWT_SECRET="flow-cli-contract-verify-not-a-real-secret"
API_CONFIG="$TMP_DIR/openpr-api.toml"
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

echo "=== starting api on 127.0.0.1:$API_PORT ===" >&2
"$API_BIN" --config "$API_CONFIG" > "$TMP_DIR/api.log" 2>&1 &
API_PID=$!
HEALTHY=0
for _ in $(seq 1 120); do
  if curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1; then HEALTHY=1; break; fi
  if ! kill -0 "$API_PID" 2>/dev/null; then break; fi
  sleep 0.5
done
[[ $HEALTHY -eq 1 ]] || { echo "FAIL: api did not become healthy" >&2; tail -40 "$TMP_DIR/api.log" >&2; exit 2; }

WORKSPACE="$(python3 -c 'import uuid; print(uuid.uuid4())')"
DISABLED_WORKSPACE="$(python3 -c 'import uuid; print(uuid.uuid4())')"
OWNER_USER="$(python3 -c 'import uuid; print(uuid.uuid4())')"
BOT_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
DISABLED_BOT_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
BOT_TOKEN="opr_cli_verify_${RUN_ID}"
DISABLED_BOT_TOKEN="opr_cli_verify_off_${RUN_ID}"
BOT_TOKEN_HASH="$(printf '%s' "$BOT_TOKEN" | sha256sum | awk '{print $1}')"
DISABLED_BOT_TOKEN_HASH="$(printf '%s' "$DISABLED_BOT_TOKEN" | sha256sum | awk '{print $1}')"

psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, created_at, updated_at)
VALUES ('$OWNER_USER', 'cli-verify-$RUN_ID@example.local', '', 'CLI Verify Owner', 'user', true, 'human', now(), now());
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES ('$BOT_ID', '$BOT_ID@bot.openpr.local', '!', 'CLI Verify Bot', 'user', true, 'bot_mcp', 'mcp', now(), now()),
       ('$DISABLED_BOT_ID', '$DISABLED_BOT_ID@bot.openpr.local', '!', 'CLI Verify Bot Off', 'user', true, 'bot_mcp', 'mcp', now(), now());
INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE', 'cli-verify-$RUN_ID', 'CLI Verify', '$OWNER_USER', now(), now()),
       ('$DISABLED_WORKSPACE', 'cli-verify-off-$RUN_ID', 'CLI Verify Off', '$OWNER_USER', now(), now());
INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES ('$WORKSPACE', '$OWNER_USER', 'owner', now()),
       ('$DISABLED_WORKSPACE', '$OWNER_USER', 'owner', now()),
       ('$WORKSPACE', '$BOT_ID', 'admin', now()),
       ('$DISABLED_WORKSPACE', '$DISABLED_BOT_ID', 'admin', now());
INSERT INTO flow_workspace_settings (workspace_id, flow_enabled, default_member_level, authz_epoch, updated_at)
VALUES ('$WORKSPACE', true, 'edit', 0, now()),
       ('$DISABLED_WORKSPACE', false, 'edit', 0, now());
INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by, is_active, created_at, updated_at)
VALUES ('$BOT_ID', '$WORKSPACE', 'CLI Verify Bot', '$BOT_TOKEN_HASH', '${BOT_TOKEN:0:8}', '["read","write","admin"]'::jsonb, '$OWNER_USER', true, now(), now()),
       ('$DISABLED_BOT_ID', '$DISABLED_WORKSPACE', 'CLI Verify Bot Off', '$DISABLED_BOT_TOKEN_HASH', '${DISABLED_BOT_TOKEN:0:8}', '["read","write","admin"]'::jsonb, '$OWNER_USER', true, now(), now());
SQL

USER_JWT="$(python3 -c '
import base64, hashlib, hmac, json, sys, time
def b64(o):
    return base64.urlsafe_b64encode(json.dumps(o, separators=(",", ":")).encode()).rstrip(b"=").decode()
secret, sub, email = sys.argv[1], sys.argv[2], sys.argv[3]
now = int(time.time())
head = b64({"alg": "HS256", "typ": "JWT"})
body = b64({"sub": sub, "email": email, "token_type": "access", "iat": now, "exp": now + 3600})
sig = base64.urlsafe_b64encode(hmac.new(secret.encode(), f"{head}.{body}".encode(), hashlib.sha256).digest()).rstrip(b"=").decode()
print(f"{head}.{body}.{sig}")
' "$JWT_SECRET" "$OWNER_USER" "cli-verify-$RUN_ID@example.local")"

API="http://127.0.0.1:$API_PORT"
DEAD_API="http://127.0.0.1:$DEAD_PORT"
json_get() { jq -r "$2" <<<"$1" 2>/dev/null || printf ''; }

CREATE_RESP="$(curl -sS -X POST "$API/api/v1/workspaces/$WORKSPACE/flow/objects" \
  -H "Authorization: Bearer $USER_JWT" -H "Content-Type: application/json" \
  -d "$(jq -n --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" '{object_type:"page", title:"CLI contract fixture", idempotency_key:$key}')")"
OBJECT_ID="$(json_get "$CREATE_RESP" '.data.id // .data.object.id // empty')"
[[ -n "$OBJECT_ID" ]] || { echo "FAIL: could not create the Flow page the CLI fixtures read (response: $CREATE_RESP)" >&2; exit 2; }

UNKNOWN_OBJECT_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"

# The CLI is configured by file only (`OpenPrConfig::load`), so even a run that
# overrides api-url/bot-token on the command line needs a valid file to exist.
# Its `[mcp]` values deliberately point nowhere useful: every fixture supplies
# the values it actually needs on the command line, so a broken override would
# show up as a failed fixture instead of silently passing.
CLI_CONFIG="$TMP_DIR/openpr-cli.toml"
cat > "$CLI_CONFIG" <<EOF
[database]
url = "postgres://unused:unused@127.0.0.1:5432/unused"

[auth]
jwt_secret = "unused-by-the-sylvode-cli"

[logging]
filter = "error"
format = "text"

[mcp]
api_url = "$DEAD_API"
bot_token = "opr_unused_config_token_${RUN_ID}"
workspace_id = "$WORKSPACE"
EOF

FIXTURES_JSON="[]"

# Runs one CLI fixture and asserts its exit status plus the stdout/stderr split.
# $1 id, $2 expected exit, $3 expected `ok` (true|false|table), then the argv.
run_fixture() {
  local id="$1" expect_exit="$2" expect_ok="$3"; shift 3
  local out="$TMP_DIR/$id.out" err="$TMP_DIR/$id.err" rc=0
  set +e
  "$CLI_BIN" "$@" > "$out" 2> "$err"
  rc=$?
  set -e
  local stdout_body stderr_body
  stdout_body="$(cat "$out")"
  stderr_body="$(cat "$err")"

  if [[ "$rc" -ne "$expect_exit" ]]; then
    VIOLATIONS+=("$id: exit status $rc, expected $expect_exit (stdout: ${stdout_body:0:300} | stderr: ${stderr_body:0:300})")
  fi

  local envelope_ok="" schema="" command_field="" request_id="" has_data="" has_error=""
  if [[ "$expect_ok" != "table" ]]; then
    if ! jq -e . >/dev/null 2>&1 <<<"$stdout_body"; then
      VIOLATIONS+=("$id: stdout is not a single JSON value (got: ${stdout_body:0:300})")
    else
      schema="$(jq -r '.schema_version // empty' <<<"$stdout_body")"
      # `.ok // empty` would be wrong here: jq's alternative operator treats a
      # literal `false` as absent, so a failure envelope would read as "no ok field".
      envelope_ok="$(jq -r 'if has("ok") then (.ok | tostring) else "" end' <<<"$stdout_body")"
      command_field="$(jq -r '.command // empty' <<<"$stdout_body")"
      request_id="$(jq -r '.request_id // empty' <<<"$stdout_body")"
      has_data="$(jq -r 'has("data")' <<<"$stdout_body")"
      has_error="$(jq -r 'has("error")' <<<"$stdout_body")"
      [[ "$schema" == "sylvode.cli.v1" ]] || VIOLATIONS+=("$id: stdout schema_version='$schema', expected sylvode.cli.v1")
      [[ "$envelope_ok" == "$expect_ok" ]] || VIOLATIONS+=("$id: stdout ok=$envelope_ok, expected $expect_ok")
      [[ -n "$command_field" ]] || VIOLATIONS+=("$id: stdout carries no 'command' field")
      if ! [[ "$request_id" =~ ^[0-9a-fA-F-]{36}$ ]]; then
        VIOLATIONS+=("$id: stdout request_id='$request_id' is not a UUID")
      fi
      if [[ "$expect_ok" == "true" ]]; then
        [[ "$has_data" == "true" ]] || VIOLATIONS+=("$id: a success envelope carries no 'data' member")
      else
        [[ "$has_error" == "true" ]] || VIOLATIONS+=("$id: a failure envelope carries no 'error' member")
        local err_code recoverable
        err_code="$(jq -r '.error.code // empty' <<<"$stdout_body")"
        recoverable="$(jq -r '.error | has("recoverable")' <<<"$stdout_body")"
        [[ -n "$err_code" ]] || VIOLATIONS+=("$id: failure envelope carries no error.code")
        [[ "$recoverable" == "true" ]] || VIOLATIONS+=("$id: failure envelope carries no error.recoverable")
      fi
    fi
  else
    if jq -e '.schema_version == "sylvode.cli.v1"' >/dev/null 2>&1 <<<"$stdout_body"; then
      VIOLATIONS+=("$id: --format table still printed the JSON envelope on stdout")
    fi
  fi

  # cli-surface-v1.md: "JSON 成功/失败均只写 stdout，诊断日志只写 stderr".
  if jq -e '.schema_version == "sylvode.cli.v1"' >/dev/null 2>&1 <<<"$stderr_body"; then
    VIOLATIONS+=("$id: the sylvode.cli.v1 envelope was written to stderr, which must carry diagnostics only")
  fi

  FIXTURES_JSON="$(jq -c --arg id "$id" --argjson expected_exit "$expect_exit" --argjson actual_exit "$rc" \
    --arg expect_ok "$expect_ok" --arg schema "$schema" --arg ok "$envelope_ok" \
    --arg command "$command_field" --arg request_id "$request_id" \
    --arg error_code "$(jq -r '.error.code // empty' <<<"$stdout_body" 2>/dev/null || printf '')" \
    --arg stdout_head "${stdout_body:0:400}" --arg stderr_head "${stderr_body:0:400}" \
    '. + [{id:$id, expected_exit:$expected_exit, actual_exit:$actual_exit, expected_ok:$expect_ok,
           schema_version:$schema, ok:$ok, command:$command, request_id:$request_id,
           error_code:$error_code, stdout_head:$stdout_head, stderr_head:$stderr_head}]' <<<"$FIXTURES_JSON")"
  echo "[$id] exit=$rc (expected $expect_exit)" >&2
}

echo "=== live CLI fixtures ===" >&2

# F0 success
run_fixture F0_success 0 true \
  --config "$CLI_CONFIG" --api-url "$API" --bot-token "$BOT_TOKEN" --format json objects get "$OBJECT_ID"

# F2 usage: a malformed uuid must be rejected locally, WITHOUT reaching the network.
# --api-url deliberately points at a closed port: had the request been sent, the
# outcome would be exit 9, not exit 2.
run_fixture F2_usage_no_request 2 false \
  --config "$CLI_CONFIG" --api-url "$DEAD_API" --bot-token "$BOT_TOKEN" --format json objects get "not-a-uuid"

# F3 unauthenticated
run_fixture F3_unauthenticated 3 false \
  --config "$CLI_CONFIG" --api-url "$API" --bot-token "opr_wrong_token_${RUN_ID}" --format json objects get "$OBJECT_ID"

# F4 feature disabled / forbidden.
# `objects query` is the flag-gated read (`policy::require_flow_workspace_access`).
# `features flow get` deliberately is NOT gated on the flag -- it is how a caller
# discovers whether Flow is on at all -- so it would be the wrong fixture here.
run_fixture F4_forbidden 4 false \
  --config "$CLI_CONFIG" --api-url "$API" --bot-token "$DISABLED_BOT_TOKEN" --format json objects query --workspace "$DISABLED_WORKSPACE" --unprojected

# F5 not found
run_fixture F5_not_found 5 false \
  --config "$CLI_CONFIG" --api-url "$API" --bot-token "$BOT_TOKEN" --format json objects get "$UNKNOWN_OBJECT_ID"

# F9 network / temporary
run_fixture F9_network 9 false \
  --config "$CLI_CONFIG" --api-url "$DEAD_API" --bot-token "$BOT_TOKEN" --format json objects get "$OBJECT_ID"

# F10 verify succeeded but found an inconsistency
run_fixture F10_integrity_mismatch 10 false \
  --config "$CLI_CONFIG" --api-url "$API" --bot-token "$BOT_TOKEN" --format json collab verify "$OBJECT_ID" --expected-head 999999

# FT --format table: same exit code, different rendering
run_fixture FT_table_success 0 table \
  --config "$CLI_CONFIG" --api-url "$API" --bot-token "$BOT_TOKEN" --format table objects get "$OBJECT_ID"
run_fixture FT_table_not_found 5 table \
  --config "$CLI_CONFIG" --api-url "$API" --bot-token "$BOT_TOKEN" --format table objects get "$UNKNOWN_OBJECT_ID"


PASSED=$([[ ${#VIOLATIONS[@]} -eq 0 ]] && echo true || echo false)
GATE_STATUS=$([[ "$PASSED" == "true" ]] && echo passed || echo failed)
VIOLATIONS_JSON="$(printf '%s\n' "${VIOLATIONS[@]:-}" | jq -R 'select(length>0)' | jq -s '.')"
REASON="static exit-table/constant comparison plus $(jq 'length' <<<"$FIXTURES_JSON") live fixtures against the shipped sylvode binary (exits 0/2/3/4/5/9/10 exercised; 6/7/8 have no v0.4 CLI command that can provoke them); $(jq 'length' <<<"$VIOLATIONS_JSON") violation(s)"

RESULT="$(jq -n \
  --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" --arg release "$RELEASE" \
  --arg contract "$CONTRACT_PATH" --arg contract_sha "$CONTRACT_SHA" \
  --argjson static_check "$STATIC_JSON" --arg schema_const "$SCHEMA_CONST" \
  --argjson fixtures "$FIXTURES_JSON" \
  --argjson violations "$VIOLATIONS_JSON" --argjson passed "$PASSED" \
  --arg gate_status "$GATE_STATUS" --arg reason "$REASON" \
  '{
    schema_version: "sylvode.flow.cli-contract-result.v1",
    source_head: $head,
    generated_at: $generated_at,
    release: $release,
    contract: {path: $contract, sha256: $contract_sha},
    static_check: ($static_check + {render_schema_version_constant: $schema_const}),
    live_fixtures: $fixtures,
    exit_codes_not_exercised: {
      "6": "stale_frontier / resync_required: reachable only from a collaboration write path; the v0.4 sylvode surface is read plus the feature flag",
      "7": "invalid_update / unsupported_protocol / checksum_mismatch / unsupported_format: no v0.4 CLI command submits an update or a package",
      "8": "limit_exceeded: no v0.4 CLI command carries a caller-sized payload the server can reject on a limit"
    },
    violations: $violations,
    passed: $passed,
    gates: {
      cli_json_and_exit_code_contract: {status: $gate_status, reason: $reason}
    }
  }')"

OUT_PATH="$EVIDENCE_ROOT/cli-contract-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$RESULT" | jq . > "$OUT_TMP"
sync "$OUT_TMP" 2>/dev/null || true
mv -f "$OUT_TMP" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2

if [[ "$PASSED" != "true" ]]; then
  jq -r '.violations[] | "  VIOLATION: " + .' <<<"$RESULT" >&2
fi

echo "$RESULT"
[[ "$PASSED" == "true" ]] && exit 0
exit 1
