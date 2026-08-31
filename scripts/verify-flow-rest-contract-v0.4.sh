#!/usr/bin/env bash
set -euo pipefail

# Live Sylvode Flow v0.4 success-path REST contract verifier.
#
# One user/API write fixture is read back from the same live api process by
# direct REST, the shipped MCP server, and the shipped CLI.  The verifier also
# drives every non-WebSocket v0.4 Flow REST route and exercises real pagination
# with more than 100 rows.  It never substitutes OpenAPI/markdown parsing or a
# prior verifier artifact for runtime observations.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
EVIDENCE_ROOT=""
DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-}"
RELEASE="0.4"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-rest-contract-v0.4.sh --release 0.4 --json [OPTIONS]

Options:
  --release VER          Must be 0.4.
  --database-url URL     PostgreSQL DSN on which a scratch database may be created.
                         Default: $OPENPR_TEST_DATABASE_URL.
  --repo-root DIR        Product checkout. Default: this checkout.
  --evidence-root DIR    Required output directory.
  --json                 Required.
  -h, --help             Show help.

Exit codes: 0 verified, 1 a contract assertion failed, 2 usage/environment error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release) RELEASE="${2:?--release requires a value}"; shift 2 ;;
    --database-url) DATABASE_URL="${2:?--database-url requires a value}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a directory}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a directory}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ "$RELEASE" == "0.4" ]] || { echo "FAIL: --release must be 0.4" >&2; exit 2; }
[[ $JSON_MODE -eq 1 ]] || { echo "FAIL: --json is required" >&2; exit 2; }
[[ -n "$EVIDENCE_ROOT" ]] || { echo "FAIL: --evidence-root is required" >&2; exit 2; }
[[ -n "$DATABASE_URL" ]] || { echo "FAIL: set --database-url or OPENPR_TEST_DATABASE_URL" >&2; exit 2; }
for tool in cargo curl git jq psql python3 sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required command: $tool" >&2; exit 2; }
done
git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1 || {
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2; exit 2; }
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atc 'SELECT 1' >/dev/null 2>&1 || {
  echo "FAIL: database is not reachable" >&2; exit 2; }

mkdir -p "$EVIDENCE_ROOT"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
RUN_ID="$(python3 -c 'import uuid; print(uuid.uuid4().hex[:10])')"
SCRATCH_DB="openpr_flow_rest_contract_$RUN_ID"
DB_PREFIX="${DATABASE_URL%/*}"
SCRATCH_URL="$DB_PREFIX/$SCRATCH_DB"
TMP_DIR="$(mktemp -d /tmp/openpr-flow-rest-contract.XXXXXX)"
API_PORT=$((20000 + RANDOM % 18000))
JWT_SECRET="flow-rest-contract-not-a-real-secret"
ORIGIN="http://127.0.0.1:4173"
API_PID=""

cleanup() {
  local ec=$?
  if [[ -n "$API_PID" ]] && kill -0 "$API_PID" 2>/dev/null; then
    kill "$API_PID" 2>/dev/null || true
    wait "$API_PID" 2>/dev/null || true
  fi
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q \
    -c "DROP DATABASE IF EXISTS \"$SCRATCH_DB\" WITH (FORCE)" >/dev/null 2>&1 || true
  rm -rf "$TMP_DIR"
  exit "$ec"
}
trap cleanup EXIT

psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q \
  -c "DROP DATABASE IF EXISTS \"$SCRATCH_DB\" WITH (FORCE)" >/dev/null 2>&1 || true
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "CREATE DATABASE \"$SCRATCH_DB\"" >/dev/null

TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
echo "=== building live api, MCP server and CLI ===" >&2
(cd "$REPO_ROOT" && cargo build -q -p api --bin api -p mcp-server --bin mcp-server --bin sylvode) || {
  echo "FAIL: runtime binary build failed" >&2; exit 2; }
API_BIN="$TARGET_DIR/debug/api"
MCP_BIN="$TARGET_DIR/debug/mcp-server"
CLI_BIN="$TARGET_DIR/debug/sylvode"
for binary in "$API_BIN" "$MCP_BIN" "$CLI_BIN"; do
  [[ -x "$binary" ]] || { echo "FAIL: missing runtime binary: $binary" >&2; exit 2; }
done

API_CONFIG="$TMP_DIR/api.toml"
cat > "$API_CONFIG" <<EOF
[server]
app_name = "api"
bind_addr = "127.0.0.1:$API_PORT"

[database]
url = "$SCRATCH_URL"

[auth]
jwt_secret = "$JWT_SECRET"

[flow]
collab_allowed_origins = ["$ORIGIN"]

[logging]
filter = "api=warn,openpr=warn"
format = "text"
EOF

"$API_BIN" --config "$API_CONFIG" > "$TMP_DIR/api.log" 2>&1 &
API_PID=$!
HEALTHY=0
for _ in $(seq 1 120); do
  if curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1; then HEALTHY=1; break; fi
  if ! kill -0 "$API_PID" 2>/dev/null; then break; fi
  sleep 0.25
done
if [[ $HEALTHY -ne 1 ]]; then
  echo "FAIL: api did not become healthy" >&2
  tail -40 "$TMP_DIR/api.log" >&2 || true
  exit 2
fi

WORKSPACE_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
OWNER_USER="$(python3 -c 'import uuid; print(uuid.uuid4())')"
BOT_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
OWNER_EMAIL="rest-contract-$RUN_ID@example.local"
BOT_TOKEN="opr_rest_contract_${RUN_ID}"
BOT_TOKEN_HASH="$(printf '%s' "$BOT_TOKEN" | sha256sum | awk '{print $1}')"
BOT_TOKEN_PREFIX="${BOT_TOKEN:0:8}"
psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, created_at, updated_at)
VALUES ('$OWNER_USER', '$OWNER_EMAIL', '', 'REST Contract Owner', 'user', true, 'human', now(), now());
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES ('$BOT_ID', '$BOT_ID@bot.openpr.local', '!', 'REST Contract Bot', 'user', true, 'bot_mcp', 'mcp', now(), now());
INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE_ID', 'rest-contract-$RUN_ID', 'REST Contract', '$OWNER_USER', now(), now());
INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES ('$WORKSPACE_ID', '$OWNER_USER', 'owner', now()), ('$WORKSPACE_ID', '$BOT_ID', 'admin', now());
INSERT INTO flow_workspace_settings (workspace_id, flow_enabled, default_member_level, authz_epoch, updated_at)
VALUES ('$WORKSPACE_ID', true, 'edit', 0, now());
INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by, is_active, created_at, updated_at)
VALUES ('$BOT_ID', '$WORKSPACE_ID', 'REST Contract Bot', '$BOT_TOKEN_HASH', '$BOT_TOKEN_PREFIX',
        '["read","write","admin"]'::jsonb, '$OWNER_USER', true, now(), now());
SQL

USER_JWT="$(python3 -c '
import base64, hashlib, hmac, json, sys, time
def enc(value):
    return base64.urlsafe_b64encode(json.dumps(value, separators=(",", ":")).encode()).rstrip(b"=").decode()
secret, subject, email = sys.argv[1:4]
now = int(time.time())
header = enc({"alg":"HS256","typ":"JWT"})
body = enc({"sub":subject,"email":email,"token_type":"access","iat":now,"exp":now+3600})
signature = base64.urlsafe_b64encode(hmac.new(secret.encode(), f"{header}.{body}".encode(), hashlib.sha256).digest()).rstrip(b"=").decode()
print(f"{header}.{body}.{signature}")
' "$JWT_SECRET" "$OWNER_USER" "$OWNER_EMAIL")"
BASE="http://127.0.0.1:$API_PORT"
USER_AUTH=(-H "Authorization: Bearer $USER_JWT" -H 'Content-Type: application/json')

VIOLATIONS='[]'
ENDPOINT_CHECKS='[]'
add_violation() { VIOLATIONS="$(jq -c --arg value "$1" '. + [$value]' <<<"$VIOLATIONS")"; }
record_endpoint() {
  local id="$1" method="$2" path="$3" status="$4" body="$5"
  local code passed
  code="$(jq -r '.code // -1' <<<"$body" 2>/dev/null || printf '%s' -1)"
  passed=false
  if [[ "$status" =~ ^2[0-9][0-9]$ && "$code" == "0" ]]; then passed=true; fi
  ENDPOINT_CHECKS="$(jq -c --arg id "$id" --arg method "$method" --arg path "$path" \
    --argjson status "$status" --argjson code "$code" --argjson passed "$passed" \
    '. + [{id:$id,method:$method,path:$path,http_status:$status,code:$code,passed:$passed}]' \
    <<<"$ENDPOINT_CHECKS")"
  [[ "$passed" == "true" ]] || add_violation "$method $path did not return a 2xx ApiResponse with code=0"
}
http_request() {
  local method="$1" path="$2" body="${3:-}" auth="${4:-user}"
  local -a headers
  if [[ "$auth" == "bot" ]]; then
    headers=(-H "Authorization: Bearer $BOT_TOKEN" -H 'Content-Type: application/json')
  else
    headers=("${USER_AUTH[@]}")
  fi
  if [[ -n "$body" ]]; then
    HTTP_STATUS="$(curl -sS -o "$TMP_DIR/http.body" -w '%{http_code}' -X "$method" "$BASE$path" "${headers[@]}" -d "$body")"
  else
    HTTP_STATUS="$(curl -sS -o "$TMP_DIR/http.body" -w '%{http_code}' -X "$method" "$BASE$path" "${headers[@]}")"
  fi
  HTTP_BODY="$(<"$TMP_DIR/http.body")"
}

CREATE_PATH="/api/v1/workspaces/$WORKSPACE_ID/flow/objects"
http_request POST "$CREATE_PATH" "$(jq -n --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" \
  '{object_type:"page",title:"Parity initial",idempotency_key:$key}')"
CREATE_RESPONSE="$HTTP_BODY"
record_endpoint create_object POST "$CREATE_PATH" "$HTTP_STATUS" "$HTTP_BODY"
OBJECT_ID="$(jq -r '.data.object.id // empty' <<<"$CREATE_RESPONSE")"
[[ -n "$OBJECT_ID" ]] || { echo "FAIL: create response has no object id" >&2; exit 2; }
DOCUMENT_ID="$(psql "$SCRATCH_URL" -Atc "SELECT id FROM collab_documents WHERE object_id='$OBJECT_ID'")"
[[ -n "$DOCUMENT_ID" ]] || { echo "FAIL: created object has no collab document" >&2; exit 2; }

command_body() {
  local type="$1" payload="$2"
  jq -n --arg type "$type" --argjson payload "$payload" \
    --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" \
    '{command:{type:$type,payload:$payload},idempotency_key:$key}'
}
COMMAND_PATH="/api/v1/flow/objects/$OBJECT_ID/commands"
http_request POST "$COMMAND_PATH" "$(command_body insert_block '{"block_id":"parity-block","index":0,"text":"one shared block projection"}')"
[[ "$(jq -r '.code // -1' <<<"$HTTP_BODY")" == "0" ]] || {
  echo "FAIL: insert_block fixture failed: $HTTP_BODY" >&2; exit 2; }

echo "=== creating real >100-row list/history fixtures through the api ===" >&2
for index in $(seq 1 100); do
  http_request POST "$COMMAND_PATH" "$(command_body set_title "$(jq -n --arg title "Parity final $index" '{title:$title}')")"
  if [[ "$(jq -r '.code // -1' <<<"$HTTP_BODY")" != "0" ]]; then
    echo "FAIL: set_title history fixture $index failed: $HTTP_BODY" >&2
    exit 2
  fi
  if [[ $index -eq 100 ]]; then FINAL_WRITE_RESPONSE="$HTTP_BODY"; FINAL_WRITE_STATUS="$HTTP_STATUS"; fi
done
record_endpoint command_object POST "$COMMAND_PATH" "$FINAL_WRITE_STATUS" "$FINAL_WRITE_RESPONSE"

for index in $(seq 1 100); do
  http_request POST "$CREATE_PATH" "$(jq -n --arg title "Page fixture $index" \
    --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" \
    '{object_type:"page",title:$title,idempotency_key:$key}')"
  if [[ "$(jq -r '.code // -1' <<<"$HTTP_BODY")" != "0" ]]; then
    echo "FAIL: list fixture create $index failed: $HTTP_BODY" >&2
    exit 2
  fi
done

WRITE_EVENT_ID="$(jq -r '.data.event_id // empty' <<<"$FINAL_WRITE_RESPONSE")"
WRITE_SEQ="$(jq -r '.data.accepted_seq // -1' <<<"$FINAL_WRITE_RESPONSE")"
[[ -n "$WRITE_EVENT_ID" && "$WRITE_SEQ" -gt 0 ]] || {
  echo "FAIL: final Web command carries no event_id/accepted_seq" >&2; exit 2; }

# Every non-WebSocket v0.4 Flow REST endpoint gets one successful live request.
LIST_PATH="/api/v1/workspaces/$WORKSPACE_ID/flow/objects"
http_request GET "$LIST_PATH"
LIST_DEFAULT="$HTTP_BODY"
record_endpoint list_objects GET "$LIST_PATH" "$HTTP_STATUS" "$HTTP_BODY"

GET_PATH="/api/v1/flow/objects/$OBJECT_ID"
http_request GET "$GET_PATH"
REST_OBJECT="$HTTP_BODY"
record_endpoint get_object GET "$GET_PATH" "$HTTP_STATUS" "$HTTP_BODY"

BOOTSTRAP_PATH="/api/v1/flow/objects/$OBJECT_ID/bootstrap"
http_request GET "$BOOTSTRAP_PATH"
BOOTSTRAP_RESPONSE="$HTTP_BODY"
record_endpoint bootstrap GET "$BOOTSTRAP_PATH" "$HTTP_STATUS" "$HTTP_BODY"

HISTORY_PATH="/api/v1/flow/objects/$OBJECT_ID/history"
http_request GET "$HISTORY_PATH"
HISTORY_DEFAULT="$HTTP_BODY"
record_endpoint history GET "$HISTORY_PATH" "$HTTP_STATUS" "$HTTP_BODY"

FEATURE_PATH="/api/v1/workspaces/$WORKSPACE_ID/features/flow"
http_request GET "$FEATURE_PATH"
record_endpoint feature_get GET "$FEATURE_PATH" "$HTTP_STATUS" "$HTTP_BODY"
http_request PUT "$FEATURE_PATH" "$(jq -n --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" \
  '{enabled:true,default_member_level:"edit",idempotency_key:$key}')"
record_endpoint feature_put PUT "$FEATURE_PATH" "$HTTP_STATUS" "$HTTP_BODY"

TICKET_PATH="/api/v1/collab/tickets"
http_request POST "$TICKET_PATH" "$(jq -n --arg workspace "$WORKSPACE_ID" --arg document "$DOCUMENT_ID" \
  --arg client "rest-contract-$RUN_ID" --arg origin "$ORIGIN" \
  '{workspace_id:$workspace,document_id:$document,client_id:$client,origin:$origin}')"
record_endpoint collab_ticket POST "$TICKET_PATH" "$HTTP_STATUS" "$HTTP_BODY"

DIAGNOSTICS_PATH="/api/v1/flow/objects/$OBJECT_ID/collab?include_sizes=true"
http_request GET "$DIAGNOSTICS_PATH"
record_endpoint collab_diagnostics GET "$DIAGNOSTICS_PATH" "$HTTP_STATUS" "$HTTP_BODY"

VERIFY_PATH="/api/v1/flow/objects/$OBJECT_ID/collab/verify"
http_request POST "$VERIFY_PATH" "$(jq -n --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" \
  --argjson head "$WRITE_SEQ" '{expected_head_seq:$head,deep:false,idempotency_key:$key}')"
record_endpoint collab_verify POST "$VERIFY_PATH" "$HTTP_STATUS" "$HTTP_BODY"

ENDPOINT_COUNT="$(jq 'length' <<<"$ENDPOINT_CHECKS")"
[[ "$ENDPOINT_COUNT" -eq 11 ]] || add_violation "endpoint census recorded $ENDPOINT_COUNT non-WebSocket routes, expected 11"
[[ "$(jq '[.[].passed] | all' <<<"$ENDPOINT_CHECKS")" == "true" ]] || \
  add_violation "one or more v0.4 REST endpoints did not return a successful ApiResponse envelope"

# Pagination uses real cardinalities greater than the default and maximum.
http_request GET "$LIST_PATH?limit=100"
LIST_MAX="$HTTP_BODY"
http_request GET "$LIST_PATH?limit=101"
LIST_OVER="$HTTP_BODY"
http_request GET "$HISTORY_PATH?limit=100"
HISTORY_MAX="$HTTP_BODY"
http_request GET "$HISTORY_PATH?limit=101"
HISTORY_OVER="$HTTP_BODY"
LIST_DEFAULT_COUNT="$(jq '.data.items | length' <<<"$LIST_DEFAULT")"
LIST_MAX_COUNT="$(jq '.data.items | length' <<<"$LIST_MAX")"
HISTORY_DEFAULT_COUNT="$(jq '.data.items | length' <<<"$HISTORY_DEFAULT")"
HISTORY_MAX_COUNT="$(jq '.data.items | length' <<<"$HISTORY_MAX")"
[[ "$LIST_DEFAULT_COUNT" -eq 50 ]] || add_violation "list default page contained $LIST_DEFAULT_COUNT items, expected 50"
[[ "$LIST_MAX_COUNT" -eq 100 ]] || add_violation "list limit=100 contained $LIST_MAX_COUNT items, expected 100"
[[ "$HISTORY_DEFAULT_COUNT" -eq 50 ]] || add_violation "history default page contained $HISTORY_DEFAULT_COUNT items, expected 50"
[[ "$HISTORY_MAX_COUNT" -eq 100 ]] || add_violation "history limit=100 contained $HISTORY_MAX_COUNT items, expected 100"
for pair in "list:$LIST_OVER" "history:$HISTORY_OVER"; do
  label="${pair%%:*}"; response="${pair#*:}"
  if [[ "$(jq -r '.code // 0' <<<"$response")" == "0" ]]; then
    add_violation "$label limit=101 was not rejected"
  fi
  [[ "$(jq -r '.error_code // empty' <<<"$response")" == "limit_exceeded" ]] || \
    add_violation "$label limit=101 did not use error_code=limit_exceeded"
  [[ "$(jq -r '.details.limit // -1' <<<"$response")" == "100" ]] || \
    add_violation "$label limit=101 did not report limit=100"
done
LIST_OVER_REJECTED="$(jq -n --argjson response "$LIST_OVER" \
  '$response.code != 0 and $response.error_code == "limit_exceeded" and $response.details.limit == 100')"
HISTORY_OVER_REJECTED="$(jq -n --argjson response "$HISTORY_OVER" \
  '$response.code != 0 and $response.error_code == "limit_exceeded" and $response.details.limit == 100')"

# The frozen gate text asks this v0.4 verifier for relation pagination, while
# rest-api-v1.md places `/relations` under the v0.5 Collaboration heading.  Do
# not silently turn that contradiction into a passing N/A: probe the live route,
# preserve the version boundary, and leave the gate red until the contract owner
# corrects the frozen criterion or explicitly changes the implementation scope.
http_request GET "/api/v1/flow/objects/$OBJECT_ID/relations"
RELATION_HTTP_STATUS="$HTTP_STATUS"
RELATION_VERSION_BOUNDARY_OK=false
if [[ "$RELATION_HTTP_STATUS" == "404" ]]; then RELATION_VERSION_BOUNDARY_OK=true; fi
add_violation "frozen v0.4 REST gate requires relation pagination, but rest-api-v1.md defines /relations only in v0.5 (live v0.4 probe HTTP $RELATION_HTTP_STATUS)"

# `Bootstrap.limits` must be the complete effective schema, not merely non-null.
EXPECTED_LIMIT_KEYS='["authorized_scan_rows_max","bootstrap_decoded_bytes_max","bootstrap_response_bytes_max","connections_per_document_max","connections_per_user_max","connections_per_workspace_max","container_count_max","decode_apply_cpu_ms_max","decode_apply_wall_ms_max","document_block_count_max","document_text_chars_max","frame_burst_max","frames_per_connection_per_second","import_archive_bytes_max","import_compression_ratio_max","import_entry_count_max","import_expanded_bytes_max","isolated_apply_memory_bytes_max","open_documents_per_connection_max","page_limit_default","page_limit_max","presence_entries_per_connection_max","presence_entries_per_document_max","presence_payload_bytes_max","presence_ttl_seconds_max","semantic_patch_json_bytes_max","semantic_patch_operations_max","slow_consumer_queue_bytes_max","slow_consumer_queue_frames_max","text_block_chars_max","tree_depth_max","update_burst_max","update_bytes_max","updates_per_connection_per_second","version","websocket_frame_bytes_max"]'
ACTUAL_LIMIT_KEYS="$(jq -c '.data.limits | keys' <<<"$BOOTSTRAP_RESPONSE")"
LIMITS_COMPLETE=false
if [[ "$ACTUAL_LIMIT_KEYS" == "$EXPECTED_LIMIT_KEYS" ]] \
  && [[ "$(jq -r '.data.limits.version // empty' <<<"$BOOTSTRAP_RESPONSE")" == "sylvode.flow.limits.v1" ]] \
  && [[ "$(jq -r '.data.limits.page_limit_default // -1' <<<"$BOOTSTRAP_RESPONSE")" == "50" ]] \
  && [[ "$(jq -r '.data.limits.page_limit_max // -1' <<<"$BOOTSTRAP_RESPONSE")" == "100" ]]; then
  LIMITS_COMPLETE=true
else
  add_violation "Bootstrap.limits is absent, partial, or differs from the complete FlowLimitsV1 key set"
fi

# MCP stdio and CLI read the exact object written above.  Both are read-only;
# update-row counts before/after prove they did not create separate fixtures.
MCP_CONFIG="$TMP_DIR/mcp-stdio.toml"
cat > "$MCP_CONFIG" <<EOF
[database]
url = "postgres://unused:unused@127.0.0.1:5432/unused"

[auth]
jwt_secret = "unused-by-mcp-rest-contract-0001"

[logging]
filter = "error"
format = "text"

[mcp]
api_url = "$BASE"
bot_token = "$BOT_TOKEN"
workspace_id = "$WORKSPACE_ID"
transport = "stdio"
EOF
CLI_CONFIG="$TMP_DIR/cli.toml"
cp "$MCP_CONFIG" "$CLI_CONFIG"

READS_BEFORE_COUNT="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM collab_updates WHERE document_id='$DOCUMENT_ID'")"
READS_BEFORE_HEAD="$(psql "$SCRATCH_URL" -Atc "SELECT head_seq FROM collab_documents WHERE id='$DOCUMENT_ID'")"
MCP_REQUESTS="$(jq -c -n --arg object_id "$OBJECT_ID" '[
  {jsonrpc:"2.0",id:1,method:"initialize",params:{protocolVersion:"2024-11-05",capabilities:{},clientInfo:{name:"rest-contract-verifier",version:"1"}}},
  {jsonrpc:"2.0",id:2,method:"tools/call",params:{name:"objects.get",arguments:{object_id:$object_id}}}
]')"
set +e
MCP_PROBE="$(python3 "$ROOT_DIR/scripts/lib/mcp_transport_probe.py" --transport stdio \
  --requests "$MCP_REQUESTS" --timeout 45 --binary "$MCP_BIN" --config "$MCP_CONFIG" 2>"$TMP_DIR/mcp-probe.err")"
MCP_EXIT=$?
set -e
if [[ $MCP_EXIT -ne 0 ]] || ! jq -e . >/dev/null 2>&1 <<<"$MCP_PROBE"; then
  add_violation "MCP stdio probe failed: $(tr '\n' ' ' < "$TMP_DIR/mcp-probe.err")"
  MCP_OBJECT='{}'
else
  MCP_TEXT="$(jq -r '([.responses[] | select(.id==2)][0].result.content // []) | map(.text // empty) | join("\n")' <<<"$MCP_PROBE")"
  if ! MCP_OBJECT="$(jq -e -c '.data | select(type=="object")' <<<"$MCP_TEXT" 2>/dev/null)"; then
    add_violation "MCP objects.get did not return the REST data projection"
    MCP_OBJECT='{}'
  fi
fi

set +e
"$CLI_BIN" --config "$CLI_CONFIG" --api-url "$BASE" --bot-token "$BOT_TOKEN" --format json \
  objects get "$OBJECT_ID" >"$TMP_DIR/cli.stdout" 2>"$TMP_DIR/cli.stderr"
CLI_EXIT=$?
set -e
CLI_ENVELOPE="$(<"$TMP_DIR/cli.stdout")"
if [[ $CLI_EXIT -ne 0 ]] || ! jq -e '.ok == true and (.data|type=="object")' >/dev/null 2>&1 <<<"$CLI_ENVELOPE"; then
  add_violation "CLI objects get failed or returned no success data (exit=$CLI_EXIT)"
  CLI_OBJECT='{}'
else
  CLI_OBJECT="$(jq -c '.data' <<<"$CLI_ENVELOPE")"
fi
READS_AFTER_COUNT="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM collab_updates WHERE document_id='$DOCUMENT_ID'")"
READS_AFTER_HEAD="$(psql "$SCRATCH_URL" -Atc "SELECT head_seq FROM collab_documents WHERE id='$DOCUMENT_ID'")"

REST_DATA="$(jq -c '.data' <<<"$REST_OBJECT")"
REST_PROJECTION="$(jq -c '{id,title,semantic_content,seq:.document_seq}' <<<"$REST_DATA")"
MCP_PROJECTION="$(jq -c '{id,title,semantic_content,seq:.document_seq}' <<<"$MCP_OBJECT")"
CLI_PROJECTION="$(jq -c '{id,title,semantic_content,seq:.document_seq}' <<<"$CLI_OBJECT")"
CROSS_SURFACE_EQUAL=false
if [[ "$REST_PROJECTION" == "$MCP_PROJECTION" && "$REST_PROJECTION" == "$CLI_PROJECTION" ]]; then
  CROSS_SURFACE_EQUAL=true
else
  add_violation "REST, MCP and CLI disagree on object id/title/block projection/seq"
fi
[[ "$(jq -r '.id' <<<"$REST_PROJECTION")" == "$OBJECT_ID" ]] || add_violation "REST read returned a different object"
[[ "$(jq -r '.seq' <<<"$REST_PROJECTION")" == "$WRITE_SEQ" ]] || add_violation "surface seq differs from the final Web write accepted_seq"
[[ "$(jq -r '.title' <<<"$REST_PROJECTION")" == "Parity final 100" ]] || add_violation "surface title differs from final Web write"
[[ "$(jq -r '.semantic_content.nodes["parity-block"].text // empty' <<<"$REST_PROJECTION")" == "one shared block projection" ]] || \
  add_violation "surface block projection differs from the Web-written block"
[[ "$READS_BEFORE_COUNT" == "$READS_AFTER_COUNT" && "$READS_BEFORE_HEAD" == "$READS_AFTER_HEAD" ]] || \
  add_violation "MCP/CLI parity reads mutated the canonical document instead of reading the one Web write fixture"
[[ "$READS_AFTER_HEAD" == "$WRITE_SEQ" ]] || add_violation "database head_seq differs from the final Web write accepted_seq"
EVENT_MATCH="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM business_events WHERE id='$WRITE_EVENT_ID' AND aggregate_id='$DOCUMENT_ID' AND payload->>'object_id'='$OBJECT_ID'")"
[[ "$EVENT_MATCH" == "1" ]] || add_violation "final Web write event_id does not identify this same object/document"

VIOLATION_COUNT="$(jq 'length' <<<"$VIOLATIONS")"
PASSED=$([[ "$VIOLATION_COUNT" -eq 0 ]] && echo true || echo false)
ENVELOPE_OK="$(jq -n --argjson endpoints "$ENDPOINT_CHECKS" --argjson limits "$LIMITS_COMPLETE" \
  --argjson list_default "$LIST_DEFAULT_COUNT" --argjson list_max "$LIST_MAX_COUNT" \
  --argjson history_default "$HISTORY_DEFAULT_COUNT" --argjson history_max "$HISTORY_MAX_COUNT" \
  --argjson list_rejected "$LIST_OVER_REJECTED" --argjson history_rejected "$HISTORY_OVER_REJECTED" \
  '([$endpoints[].passed] | all) and $limits and $list_default==50 and $list_max==100 and
   $history_default==50 and $history_max==100 and $list_rejected and $history_rejected and false')"
PARITY_OK="$(jq -n --argjson equal "$CROSS_SURFACE_EQUAL" \
  --argjson before "$READS_BEFORE_COUNT" --argjson after "$READS_AFTER_COUNT" \
  --argjson head "$READS_AFTER_HEAD" --argjson write_seq "$WRITE_SEQ" --argjson event_match "$EVENT_MATCH" \
  '$equal and $before==$after and $head==$write_seq and $event_match==1')"
ENVELOPE_STATUS=$([[ "$ENVELOPE_OK" == "true" ]] && echo passed || echo failed)
PARITY_STATUS=$([[ "$PARITY_OK" == "true" ]] && echo passed || echo failed)
PARITY_REASON="one Web REST command fixture was read without writes by REST, shipped MCP stdio and shipped CLI with identical title/block projection/seq"
ENVELOPE_REASON="all 11 non-WebSocket v0.4 Flow REST endpoints returned a 2xx ApiResponse with code=0, list/history pagination and Bootstrap.limits passed, but the frozen relation-pagination criterion has $VIOLATION_COUNT blocking contradiction(s)"

RESULT="$(jq -n \
  --arg release "$RELEASE" --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" \
  --arg workspace_id "$WORKSPACE_ID" --arg object_id "$OBJECT_ID" --arg document_id "$DOCUMENT_ID" \
  --arg event_id "$WRITE_EVENT_ID" --argjson write_seq "$WRITE_SEQ" \
  --argjson endpoints "$ENDPOINT_CHECKS" --argjson rest "$REST_PROJECTION" --argjson mcp "$MCP_PROJECTION" --argjson cli "$CLI_PROJECTION" \
  --argjson equal "$CROSS_SURFACE_EQUAL" --argjson before_count "$READS_BEFORE_COUNT" --argjson after_count "$READS_AFTER_COUNT" \
  --argjson list_default "$LIST_DEFAULT_COUNT" --argjson list_max "$LIST_MAX_COUNT" \
  --argjson history_default "$HISTORY_DEFAULT_COUNT" --argjson history_max "$HISTORY_MAX_COUNT" \
  --argjson list_rejected "$LIST_OVER_REJECTED" --argjson history_rejected "$HISTORY_OVER_REJECTED" \
  --argjson relation_status "$RELATION_HTTP_STATUS" --argjson relation_boundary "$RELATION_VERSION_BOUNDARY_OK" \
  --argjson expected_limit_keys "$EXPECTED_LIMIT_KEYS" --argjson actual_limit_keys "$ACTUAL_LIMIT_KEYS" --argjson limits_complete "$LIMITS_COMPLETE" \
  --argjson violations "$VIOLATIONS" --argjson passed "$PASSED" --arg parity_status "$PARITY_STATUS" --arg envelope_status "$ENVELOPE_STATUS" \
  --arg parity_reason "$PARITY_REASON" --arg envelope_reason "$ENVELOPE_REASON" \
  '{
    schema_version:"sylvode.flow.rest-contract-result.v1",
    release:$release,
    source_head:$head,
    generated_at:$generated_at,
    fixture:{workspace_id:$workspace_id,object_id:$object_id,document_id:$document_id,write_event_id:$event_id,write_seq:$write_seq,writer_surface:"web_rest_command"},
    endpoint_checks:$endpoints,
    cross_surface_projection:{rest:$rest,mcp:$mcp,cli:$cli,equal:$equal,collab_updates_before_reads:$before_count,collab_updates_after_reads:$after_count,read_surfaces_wrote_nothing:($before_count==$after_count)},
    pagination:{
      list:{default_count:$list_default,max_100_count:$list_max,limit_101_rejected:$list_rejected},
      history:{default_count:$history_default,max_100_count:$history_max,limit_101_rejected:$history_rejected},
      relations:{status:"blocked_by_frozen_version_contradiction",required_by_gate_release:"0.4",defined_by_rest_contract_release:"0.5",probed_http_status:$relation_status,version_boundary_preserved:$relation_boundary}
    },
    bootstrap_limits:{complete:$limits_complete,expected_keys:$expected_limit_keys,actual_keys:$actual_limit_keys},
    violations:$violations,
    passed:$passed,
    gates:{
      rest_mcp_cli_ui_surface_parity:{status:$parity_status,reason:$parity_reason},
      rest_envelope_and_error_contract:{status:$envelope_status,reason:$envelope_reason}
    }
  }')"

OUT_PATH="$EVIDENCE_ROOT/rest-contract-result.json"
printf '%s\n' "$RESULT" | jq . > "$OUT_PATH.tmp"
sync "$OUT_PATH.tmp" 2>/dev/null || true
mv -f "$OUT_PATH.tmp" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2
if [[ "$PASSED" != "true" ]]; then jq -r '.violations[] | "  VIOLATION: " + .' <<<"$RESULT" >&2; fi
echo "$RESULT"
[[ "$PASSED" == "true" ]]
