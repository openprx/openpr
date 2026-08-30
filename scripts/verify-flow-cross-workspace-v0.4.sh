#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 cross-workspace / policy-bypass negative verifier.
#
# Contract: /opt/working/sylvode-flow/security/threat-model.md rows
# "跨 workspace object/relation 访问" (first gate: v0.4) and "伪造 actor/origin",
# its "必须失败关闭" list (workspace / 授权 / feature flag 不一致), and
# /opt/working/sylvode-flow/gates/v0.4-gate.yaml's hard gate
# `cross_workspace_and_policy_bypass_negative`.
#
# Backs exactly one hard gate: cross_workspace_and_policy_bypass_negative.
#
# THIS IS A NEGATIVE GATE. Every check drives a real request against a real
# `api` binary and a real PostgreSQL database, asserting that the request is
# REFUSED (business `code` in the JSON envelope -- this API answers every
# error with HTTP 200, so HTTP status carries no verdict) and that nothing
# was written before the refusal. `get`, `update` and `link` are covered
# across the workspace boundary, plus the policy-bypass negatives: a bot
# token used outside its own workspace, a read-only bot attempting a write,
# a bot on the two ADR-0007 user-only surfaces, a disabled `flow_enabled`
# flag, an absent credential and a forged bot token.
#
# Each negative is paired with the identical request inside the caller's own
# workspace, which must SUCCEED. Without that control a broken fixture (bad
# ids, dead server, wrong payload) would make every "rejected" assertion
# true for the wrong reason.
#
# Exit codes: 0 = gate passed, 1 = gate failed, 2 = usage/tool/environment
# error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
THREAT_MODEL=""
DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-}"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-cross-workspace-v0.4.sh --threat-model PATH --json [OPTIONS]

Drives cross-workspace get/update/link and policy-bypass negatives against a
real api binary and database, asserting each is refused with zero writes and
that the same request inside the caller's own workspace succeeds.

Writes evidence/v0.4/cross-workspace-negative-result.json.

Options:
  --threat-model PATH   Path to security/threat-model.md. Required.
  --database-url URL    Postgres DSN. Default: $OPENPR_TEST_DATABASE_URL
  --repo-root DIR       Repository containing apps/api. Default: this checkout.
  --evidence-root DIR   Where cross-workspace-negative-result.json is written.
                        Default: /opt/working/sylvode-flow/evidence/v0.4
  --json                Required for CLI-contract compatibility.
  -h, --help            Show this help and exit 0.

Exit codes: 0 gate passed, 1 gate failed, 2 usage/tool error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --threat-model) THREAT_MODEL="${2:?--threat-model requires a PATH argument}"; shift 2 ;;
    --database-url) DATABASE_URL="${2:?--database-url requires a value}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$THREAT_MODEL" ]]; then echo "FAIL: --threat-model is required" >&2; usage >&2; exit 2; fi
if [[ ! -f "$THREAT_MODEL" ]]; then echo "FAIL: --threat-model file not found: $THREAT_MODEL" >&2; exit 2; fi
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

CHECKS_JSON="[]"
record() {
  local id="$1" ok="$2" expectation="$3" observed="$4"
  CHECKS_JSON="$(jq -n --argjson acc "$CHECKS_JSON" --arg id "$id" --argjson passed "$ok" \
    --arg expectation "$expectation" --arg observed "$observed" \
    '$acc + [{id:$id, passed:$passed, expectation:$expectation, observed:$observed}]')"
  if [[ "$ok" == "true" ]]; then echo "  PASS  $id" >&2; else
    echo "  FAIL  $id -- expected: $expectation | observed: $observed" >&2
  fi
}

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

uuid() { python3 -c 'import uuid; print(uuid.uuid4())'; }
RUN_ID="$(python3 -c 'import uuid; print(uuid.uuid4().hex[:8])')"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/openpr-cross-workspace-verify.XXXXXX")"
WS_A="$(uuid)"; WS_B="$(uuid)"; WS_C="$(uuid)"
USER_A="$(uuid)"; USER_B="$(uuid)"
BOT_RW="$(uuid)"; BOT_RO="$(uuid)"
BOT_RW_TOKEN="opr_xws_rw_${RUN_ID}"
BOT_RO_TOKEN="opr_xws_ro_${RUN_ID}"
EMAIL_A="xws-a-$RUN_ID@example.local"
EMAIL_B="xws-b-$RUN_ID@example.local"
PASSWORD="CrossWorkspace!${RUN_ID}"
ORIGIN_A="https://flow-xws.test"
API_PORT=$((22000 + RANDOM % 20000))
API_LOG="$TMP_DIR/api.log"
API_PID=""

# shellcheck disable=SC2317  # invoked only via `trap ... EXIT` below.
cleanup() {
  local ec=$?
  if [[ -n "$API_PID" ]] && kill -0 "$API_PID" 2>/dev/null; then
    kill "$API_PID" 2>/dev/null || true
    wait "$API_PID" 2>/dev/null || true
  fi
  psql "$DATABASE_URL" -q >/dev/null 2>&1 <<SQL || true
DELETE FROM collab_updates WHERE document_id IN (
  SELECT id FROM collab_documents WHERE object_id IN (
    SELECT id FROM flow_objects WHERE workspace_id IN ('$WS_A','$WS_B','$WS_C')));
DELETE FROM event_dispatch WHERE event_id IN (
  SELECT id FROM business_events WHERE workspace_id IN ('$WS_A','$WS_B','$WS_C'));
DELETE FROM business_events WHERE workspace_id IN ('$WS_A','$WS_B','$WS_C');
DELETE FROM collab_tickets WHERE workspace_id IN ('$WS_A','$WS_B','$WS_C');
DELETE FROM flow_integrity_records WHERE workspace_id IN ('$WS_A','$WS_B','$WS_C');
DELETE FROM flow_object_projections WHERE object_id IN (
  SELECT id FROM flow_objects WHERE workspace_id IN ('$WS_A','$WS_B','$WS_C'));
DELETE FROM collab_documents WHERE object_id IN (
  SELECT id FROM flow_objects WHERE workspace_id IN ('$WS_A','$WS_B','$WS_C'));
DELETE FROM flow_objects WHERE workspace_id IN ('$WS_A','$WS_B','$WS_C');
DELETE FROM flow_workspace_settings WHERE workspace_id IN ('$WS_A','$WS_B','$WS_C');
DELETE FROM workspace_bots WHERE id IN ('$BOT_RW','$BOT_RO');
DELETE FROM workspace_members WHERE workspace_id IN ('$WS_A','$WS_B','$WS_C');
DELETE FROM workspaces WHERE id IN ('$WS_A','$WS_B','$WS_C');
DELETE FROM users WHERE id IN ('$USER_A','$USER_B','$BOT_RW','$BOT_RO');
SQL
  rm -rf "$TMP_DIR"
  exit "$ec"
}
trap cleanup EXIT

export PW="$PASSWORD"
PW_HASH="$(python3 -c "import bcrypt,os; print(bcrypt.hashpw(os.environ['PW'].encode(), bcrypt.gensalt(rounds=6)).decode())")"
unset PW
hash_token() { printf '%s' "$1" | sha256sum | awk '{print $1}'; }

psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES ('$USER_A', '$EMAIL_A', '$PW_HASH', 'XWS User A', 'user', true, 'human', NULL, now(), now()),
       ('$USER_B', '$EMAIL_B', '$PW_HASH', 'XWS User B', 'user', true, 'human', NULL, now(), now()),
       ('$BOT_RW', 'xws-bot-rw-$RUN_ID@bot.openpr.local', '!', 'XWS Bot RW', 'user', true, 'bot_mcp', 'mcp', now(), now()),
       ('$BOT_RO', 'xws-bot-ro-$RUN_ID@bot.openpr.local', '!', 'XWS Bot RO', 'user', true, 'bot_mcp', 'mcp', now(), now());
INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WS_A', 'xws-a-$RUN_ID', 'XWS A', '$USER_A', now(), now()),
       ('$WS_B', 'xws-b-$RUN_ID', 'XWS B', '$USER_B', now(), now()),
       ('$WS_C', 'xws-c-$RUN_ID', 'XWS C', '$USER_A', now(), now());
INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES ('$WS_A', '$USER_A', 'owner', now()),
       ('$WS_A', '$BOT_RW', 'member', now()),
       ('$WS_A', '$BOT_RO', 'member', now()),
       ('$WS_B', '$USER_B', 'owner', now()),
       ('$WS_C', '$USER_A', 'owner', now());
INSERT INTO flow_workspace_settings (workspace_id, flow_enabled, default_member_level, authz_epoch, updated_at)
VALUES ('$WS_A', true, 'edit', 0, now()),
       ('$WS_B', true, 'edit', 0, now()),
       ('$WS_C', true, 'edit', 0, now());
INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by, is_active, created_at, updated_at)
VALUES ('$BOT_RW', '$WS_A', 'XWS Bot RW', '$(hash_token "$BOT_RW_TOKEN")', '${BOT_RW_TOKEN:0:8}', '["read","write"]'::jsonb, '$USER_A', true, now(), now()),
       ('$BOT_RO', '$WS_A', 'XWS Bot RO', '$(hash_token "$BOT_RO_TOKEN")', '${BOT_RO_TOKEN:0:8}', '["read"]'::jsonb, '$USER_A', true, now(), now());
SQL

cat > "$TMP_DIR/api.toml" <<EOF
[server]
app_name = "api"
bind_addr = "127.0.0.1:$API_PORT"

[database]
url = "$DATABASE_URL"

[auth]
jwt_secret = "cross-workspace-verify-not-a-real-secret"

[flow]
collab_allowed_origins = ["$ORIGIN_A"]

[logging]
filter = "api=info,openpr=info"
format = "text"
EOF

"$API_BIN" --config "$TMP_DIR/api.toml" > "$API_LOG" 2>&1 &
API_PID=$!
HEALTHY=0
for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1; then HEALTHY=1; break; fi
  sleep 0.5
done
if [[ $HEALTHY -ne 1 ]]; then
  echo "FAIL: api did not become healthy within 30s; log follows" >&2
  cat "$API_LOG" >&2
  exit 2
fi
BASE="http://127.0.0.1:$API_PORT"

login() {
  curl -sS -X POST "$BASE/api/v1/auth/login" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg e "$1" --arg p "$PASSWORD" '{email:$e, password:$p}')" \
    | jq -r '.data.tokens.access_token // empty'
}
TOKEN_A="$(login "$EMAIL_A")"
TOKEN_B="$(login "$EMAIL_B")"
if [[ -z "$TOKEN_A" || -z "$TOKEN_B" ]]; then
  echo "FAIL: could not log the fixture users in" >&2
  cat "$API_LOG" >&2
  exit 2
fi

sql1() { psql "$DATABASE_URL" -Atc "$1"; }
envelope_code() { jq -r '.code // "missing"' <<<"$1"; }

create_object() {
  # $1 = workspace, $2 = auth header value, $3 = title, $4 = parent json ("null" or "\"uuid\"")
  curl -sS -X POST "$BASE/api/v1/workspaces/$1/flow/objects" \
    -H "Authorization: $2" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg t "$3" --arg k "$(uuid)" --argjson parent "$4" \
      '{object_type:"page", title:$t, idempotency_key:$k}
       + (if $parent == null then {} else {parent_object_id:$parent} end)')"
}

send_command() {
  # $1 = object_id, $2 = auth header value ("" = send no Authorization header)
  local args=(-sS -X POST "$BASE/api/v1/flow/objects/$1/commands" -H 'Content-Type: application/json'
              -d "$(jq -n --arg k "$(uuid)" '{command:{type:"set_title", payload:{title:"cross-workspace probe"}}, idempotency_key:$k}')")
  if [[ -n "$2" ]]; then args+=(-H "Authorization: $2"); fi
  curl "${args[@]}"
}

doc_write_counts() {
  psql "$DATABASE_URL" -Atc "
    SELECT (SELECT count(*) FROM collab_updates WHERE document_id='$1')
        || '/' || (SELECT count(*) FROM business_events WHERE aggregate_id='$1')
        || '/' || (SELECT head_seq::text FROM collab_documents WHERE id='$1')"
}
ws_object_count() { sql1 "SELECT count(*) FROM flow_objects WHERE workspace_id='$1'"; }

# ---------------------------------------------------------------- fixtures
A_RESP="$(create_object "$WS_A" "Bearer $TOKEN_A" "xws A page" null)"
B_RESP="$(create_object "$WS_B" "Bearer $TOKEN_B" "xws B SECRET page" null)"
C_RESP="$(create_object "$WS_C" "Bearer $TOKEN_A" "xws C page" null)"
OBJ_A="$(jq -r '.data.object.id // empty' <<<"$A_RESP")"; DOC_A="$(jq -r '.data.object.document_id // empty' <<<"$A_RESP")"
OBJ_B="$(jq -r '.data.object.id // empty' <<<"$B_RESP")"; DOC_B="$(jq -r '.data.object.document_id // empty' <<<"$B_RESP")"
OBJ_C="$(jq -r '.data.object.id // empty' <<<"$C_RESP")"
for v in "$OBJ_A" "$DOC_A" "$OBJ_B" "$DOC_B" "$OBJ_C"; do
  if [[ -z "$v" ]]; then
    echo "FAIL: fixture object creation failed. A=$A_RESP B=$B_RESP C=$C_RESP" >&2
    exit 2
  fi
done

# ======================================================== POSITIVE CONTROLS
GET_A="$(curl -sS "$BASE/api/v1/flow/objects/$OBJ_A" -H "Authorization: Bearer $TOKEN_A")"
GET_A_CODE="$(envelope_code "$GET_A")"
CTRL_BEFORE="$(doc_write_counts "$DOC_A")"
CMD_A="$(send_command "$OBJ_A" "Bearer $TOKEN_A")"
CMD_A_CODE="$(envelope_code "$CMD_A")"
CTRL_AFTER="$(doc_write_counts "$DOC_A")"
if [[ "$GET_A_CODE" == "0" && "$CMD_A_CODE" == "0" && "$CTRL_BEFORE" == "0/0/0" && "$CTRL_AFTER" == "1/1/1" ]]; then
  record "same_workspace_get_and_update_control" true \
    "the identical get + set_title inside the caller's own workspace succeed and write exactly one update/event" \
    "get_code=$GET_A_CODE command_code=$CMD_A_CODE counts updates/events/head_seq $CTRL_BEFORE -> $CTRL_AFTER"
else
  record "same_workspace_get_and_update_control" false \
    "the identical get + set_title inside the caller's own workspace succeed and write exactly one update/event" \
    "get_code=$GET_A_CODE command_code=$CMD_A_CODE counts $CTRL_BEFORE -> $CTRL_AFTER"
fi

# ====================================================== CROSS-WORKSPACE GET
XGET="$(curl -sS "$BASE/api/v1/flow/objects/$OBJ_B" -H "Authorization: Bearer $TOKEN_A")"
XGET_CODE="$(envelope_code "$XGET")"
XGET_LEAKS="no"; if grep -q "SECRET" <<<"$XGET"; then XGET_LEAKS="yes"; fi
if [[ "$XGET_CODE" == "404" && "$XGET_LEAKS" == "no" ]]; then
  record "cross_workspace_object_get_rejected_not_found_safe" true \
    "GET /flow/objects/{object in another workspace} => envelope code 404 (not-found-safe) and no title leak" \
    "code=$XGET_CODE title_leaked=$XGET_LEAKS"
else
  record "cross_workspace_object_get_rejected_not_found_safe" false \
    "GET /flow/objects/{object in another workspace} => envelope code 404 (not-found-safe) and no title leak" \
    "code=$XGET_CODE title_leaked=$XGET_LEAKS body=$(head -c 200 <<<"$XGET")"
fi

XLIST="$(curl -sS "$BASE/api/v1/workspaces/$WS_B/flow/objects" -H "Authorization: Bearer $TOKEN_A")"
XLIST_CODE="$(envelope_code "$XLIST")"
XLIST_ITEMS="$(jq -r '(.data.items // []) | length' <<<"$XLIST" 2>/dev/null || echo "n/a")"
if [[ "$XLIST_CODE" == "404" && "$XLIST_ITEMS" == "0" ]]; then
  record "cross_workspace_object_list_rejected" true \
    "GET /workspaces/{other}/flow/objects => envelope code 404 and zero items" \
    "code=$XLIST_CODE items=$XLIST_ITEMS"
else
  record "cross_workspace_object_list_rejected" false \
    "GET /workspaces/{other}/flow/objects => envelope code 404 and zero items" \
    "code=$XLIST_CODE items=$XLIST_ITEMS body=$(head -c 200 <<<"$XLIST")"
fi

XBOOT="$(curl -sS "$BASE/api/v1/flow/objects/$OBJ_B/bootstrap" -H "Authorization: Bearer $TOKEN_A")"
XBOOT_CODE="$(envelope_code "$XBOOT")"
XBOOT_BYTES="$(jq -r 'if (.data.snapshot // .data.bootstrap.snapshot // null) == null then "none" else "present" end' <<<"$XBOOT" 2>/dev/null || echo "unparsable")"
if [[ "$XBOOT_CODE" == "404" && "$XBOOT_BYTES" == "none" ]]; then
  record "cross_workspace_bootstrap_rejected_no_crdt_bytes" true \
    "GET /flow/objects/{other workspace}/bootstrap => envelope code 404 and no snapshot bytes in the body" \
    "code=$XBOOT_CODE snapshot=$XBOOT_BYTES"
else
  record "cross_workspace_bootstrap_rejected_no_crdt_bytes" false \
    "GET /flow/objects/{other workspace}/bootstrap => envelope code 404 and no snapshot bytes in the body" \
    "code=$XBOOT_CODE snapshot=$XBOOT_BYTES body=$(head -c 200 <<<"$XBOOT")"
fi

XDIAG="$(curl -sS "$BASE/api/v1/flow/objects/$OBJ_B/collab" -H "Authorization: Bearer $TOKEN_A")"
XDIAG_CODE="$(envelope_code "$XDIAG")"
if [[ "$XDIAG_CODE" == "404" ]]; then
  record "cross_workspace_collab_diagnostics_rejected" true \
    "GET /flow/objects/{other workspace}/collab => envelope code 404" "code=$XDIAG_CODE"
else
  record "cross_workspace_collab_diagnostics_rejected" false \
    "GET /flow/objects/{other workspace}/collab => envelope code 404" \
    "code=$XDIAG_CODE body=$(head -c 200 <<<"$XDIAG")"
fi

# =================================================== CROSS-WORKSPACE UPDATE
XCMD_BEFORE="$(doc_write_counts "$DOC_B")"
XCMD="$(send_command "$OBJ_B" "Bearer $TOKEN_A")"
XCMD_CODE="$(envelope_code "$XCMD")"
XCMD_AFTER="$(doc_write_counts "$DOC_B")"
if [[ "$XCMD_CODE" == "404" && "$XCMD_AFTER" == "$XCMD_BEFORE" ]]; then
  record "cross_workspace_command_rejected_zero_write" true \
    "POST /flow/objects/{other workspace}/commands => envelope code 404 and the target document's updates/events/head_seq unchanged" \
    "code=$XCMD_CODE counts $XCMD_BEFORE -> $XCMD_AFTER"
else
  record "cross_workspace_command_rejected_zero_write" false \
    "POST /flow/objects/{other workspace}/commands => envelope code 404 and the target document's updates/events/head_seq unchanged" \
    "code=$XCMD_CODE counts $XCMD_BEFORE -> $XCMD_AFTER body=$(head -c 200 <<<"$XCMD")"
fi

# ===================================================== CROSS-WORKSPACE LINK
# v0.4 has no `flow_relations` table yet, so the one caller-supplied
# object-to-object link this version accepts is `create_object`'s
# `parent_object_id` -- which is exactly the "跨 workspace relation" the
# threat model names, and which `flow::command` must fail closed on.
XLINK_A_BEFORE="$(ws_object_count "$WS_A")"
XLINK="$(create_object "$WS_A" "Bearer $TOKEN_A" "xws link probe" "\"$OBJ_B\"")"
XLINK_CODE="$(envelope_code "$XLINK")"
XLINK_A_AFTER="$(ws_object_count "$WS_A")"
XLINK_CHILD="$(sql1 "SELECT count(*) FROM flow_objects WHERE parent_id='$OBJ_B'")"
if [[ "$XLINK_CODE" != "0" && "$XLINK_A_AFTER" == "$XLINK_A_BEFORE" && "$XLINK_CHILD" == "0" ]]; then
  record "cross_workspace_link_rejected_zero_write" true \
    "create_object with parent_object_id in another workspace => non-success envelope code, no new flow_objects row, no child attached to the foreign parent" \
    "code=$XLINK_CODE workspace_a_objects $XLINK_A_BEFORE -> $XLINK_A_AFTER children_of_foreign_parent=$XLINK_CHILD"
else
  record "cross_workspace_link_rejected_zero_write" false \
    "create_object with parent_object_id in another workspace => non-success envelope code, no new flow_objects row, no child attached to the foreign parent" \
    "code=$XLINK_CODE workspace_a_objects $XLINK_A_BEFORE -> $XLINK_A_AFTER children_of_foreign_parent=$XLINK_CHILD body=$(head -c 200 <<<"$XLINK")"
fi

# ------- same link inside one workspace must still work (control) ---------
LINK_OK="$(create_object "$WS_A" "Bearer $TOKEN_A" "xws link control" "\"$OBJ_A\"")"
LINK_OK_CODE="$(envelope_code "$LINK_OK")"
if [[ "$LINK_OK_CODE" == "0" ]]; then
  record "same_workspace_link_control" true \
    "the identical create_object with a parent in the caller's OWN workspace succeeds" "code=$LINK_OK_CODE"
else
  record "same_workspace_link_control" false \
    "the identical create_object with a parent in the caller's OWN workspace succeeds" \
    "code=$LINK_OK_CODE body=$(head -c 200 <<<"$LINK_OK")"
fi

# =========================================== CROSS-WORKSPACE COLLAB TICKET
# A ticket is a direct grant onto one collab document. Asking for one with a
# `workspace_id` the caller belongs to but a `document_id` that belongs to a
# different workspace must fail closed -- otherwise the WebSocket session
# built from it streams another workspace's snapshot.
XTICKET="$(curl -sS -X POST "$BASE/api/v1/collab/tickets" \
  -H "Authorization: Bearer $TOKEN_A" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg w "$WS_A" --arg d "$DOC_B" --arg c "xws-client-$RUN_ID" --arg o "$ORIGIN_A" \
    '{workspace_id:$w, document_id:$d, client_id:$c, origin:$o}')")"
XTICKET_CODE="$(envelope_code "$XTICKET")"
XTICKET_ROWS="$(sql1 "SELECT count(*) FROM collab_tickets WHERE document_id='$DOC_B'")"
if [[ "$XTICKET_CODE" != "0" && "$XTICKET_ROWS" == "0" ]]; then
  record "cross_workspace_collab_ticket_rejected_zero_write" true \
    "POST /collab/tickets with the caller's own workspace_id but another workspace's document_id => non-success envelope code and 0 collab_tickets rows" \
    "code=$XTICKET_CODE rows=$XTICKET_ROWS"
else
  record "cross_workspace_collab_ticket_rejected_zero_write" false \
    "POST /collab/tickets with the caller's own workspace_id but another workspace's document_id => non-success envelope code and 0 collab_tickets rows" \
    "code=$XTICKET_CODE rows=$XTICKET_ROWS body=$(head -c 200 <<<"$XTICKET")"
fi

# ============================================= POLICY BYPASS: BOT SCOPING
BOT_LIST_B="$(curl -sS "$BASE/api/v1/workspaces/$WS_B/flow/objects" -H "Authorization: Bearer $BOT_RW_TOKEN")"
BOT_LIST_B_CODE="$(envelope_code "$BOT_LIST_B")"
BOT_GET_B="$(curl -sS "$BASE/api/v1/flow/objects/$OBJ_B" -H "Authorization: Bearer $BOT_RW_TOKEN")"
BOT_GET_B_CODE="$(envelope_code "$BOT_GET_B")"
BOT_LIST_A="$(curl -sS "$BASE/api/v1/workspaces/$WS_A/flow/objects" -H "Authorization: Bearer $BOT_RW_TOKEN")"
BOT_LIST_A_CODE="$(envelope_code "$BOT_LIST_A")"
if [[ "$BOT_LIST_B_CODE" == "403" && "$BOT_GET_B_CODE" == "403" && "$BOT_LIST_A_CODE" == "0" ]]; then
  record "bot_token_confined_to_its_own_workspace" true \
    "a workspace-A bot token => 403 on workspace B's list AND on an object owned by B, while the same token succeeds inside A" \
    "list_b=$BOT_LIST_B_CODE get_b=$BOT_GET_B_CODE list_a=$BOT_LIST_A_CODE"
else
  record "bot_token_confined_to_its_own_workspace" false \
    "a workspace-A bot token => 403 on workspace B's list AND on an object owned by B, while the same token succeeds inside A" \
    "list_b=$BOT_LIST_B_CODE get_b=$BOT_GET_B_CODE list_a=$BOT_LIST_A_CODE"
fi

RO_BEFORE="$(doc_write_counts "$DOC_A")"
RO_CMD="$(send_command "$OBJ_A" "Bearer $BOT_RO_TOKEN")"
RO_CMD_CODE="$(envelope_code "$RO_CMD")"
RO_AFTER="$(doc_write_counts "$DOC_A")"
RO_GET="$(curl -sS "$BASE/api/v1/flow/objects/$OBJ_A" -H "Authorization: Bearer $BOT_RO_TOKEN")"
RO_GET_CODE="$(envelope_code "$RO_GET")"
if [[ "$RO_CMD_CODE" == "403" && "$RO_AFTER" == "$RO_BEFORE" && "$RO_GET_CODE" == "0" ]]; then
  record "read_only_bot_cannot_write_zero_write" true \
    "a read-only bot token => 403 on POST commands with the document unchanged, while its GET of the same object still succeeds" \
    "command=$RO_CMD_CODE counts $RO_BEFORE -> $RO_AFTER get=$RO_GET_CODE"
else
  record "read_only_bot_cannot_write_zero_write" false \
    "a read-only bot token => 403 on POST commands with the document unchanged, while its GET of the same object still succeeds" \
    "command=$RO_CMD_CODE counts $RO_BEFORE -> $RO_AFTER get=$RO_GET_CODE"
fi

BOT_TICKET="$(curl -sS -X POST "$BASE/api/v1/collab/tickets" \
  -H "Authorization: Bearer $BOT_RW_TOKEN" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg w "$WS_A" --arg d "$DOC_A" --arg c "bot-client-$RUN_ID" --arg o "$ORIGIN_A" \
    '{workspace_id:$w, document_id:$d, client_id:$c, origin:$o}')")"
BOT_TICKET_CODE="$(envelope_code "$BOT_TICKET")"
BOT_BOOT="$(curl -sS "$BASE/api/v1/flow/objects/$OBJ_A/bootstrap" -H "Authorization: Bearer $BOT_RW_TOKEN")"
BOT_BOOT_CODE="$(envelope_code "$BOT_BOOT")"
BOT_BOOT_BYTES="$(jq -r 'if (.data.snapshot // .data.bootstrap.snapshot // null) == null then "none" else "present" end' <<<"$BOT_BOOT" 2>/dev/null || echo "unparsable")"
if [[ "$BOT_TICKET_CODE" == "403" && "$BOT_BOOT_CODE" == "403" && "$BOT_BOOT_BYTES" == "none" ]]; then
  record "bot_excluded_from_user_only_crdt_surfaces" true \
    "a fully permitted workspace-A bot => 403 on /collab/tickets AND on bootstrap for an object it CAN otherwise read, with no snapshot bytes returned" \
    "ticket=$BOT_TICKET_CODE bootstrap=$BOT_BOOT_CODE snapshot=$BOT_BOOT_BYTES"
else
  record "bot_excluded_from_user_only_crdt_surfaces" false \
    "a fully permitted workspace-A bot => 403 on /collab/tickets AND on bootstrap for an object it CAN otherwise read, with no snapshot bytes returned" \
    "ticket=$BOT_TICKET_CODE bootstrap=$BOT_BOOT_CODE snapshot=$BOT_BOOT_BYTES"
fi

# ======================================= POLICY BYPASS: FEATURE FLAG CLOSED
FLAG_LIST_ON="$(curl -sS "$BASE/api/v1/workspaces/$WS_C/flow/objects" -H "Authorization: Bearer $TOKEN_A")"
FLAG_ON_CODE="$(envelope_code "$FLAG_LIST_ON")"
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c \
  "UPDATE flow_workspace_settings SET flow_enabled = false WHERE workspace_id='$WS_C'"
FLAG_LIST_OFF="$(curl -sS "$BASE/api/v1/workspaces/$WS_C/flow/objects" -H "Authorization: Bearer $TOKEN_A")"
FLAG_OFF_CODE="$(envelope_code "$FLAG_LIST_OFF")"
DOC_C="$(sql1 "SELECT id FROM collab_documents WHERE object_id='$OBJ_C'")"
FLAG_BEFORE="$(doc_write_counts "$DOC_C")"
FLAG_CMD="$(send_command "$OBJ_C" "Bearer $TOKEN_A")"
FLAG_CMD_CODE="$(envelope_code "$FLAG_CMD")"
FLAG_AFTER="$(doc_write_counts "$DOC_C")"
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c \
  "UPDATE flow_workspace_settings SET flow_enabled = true WHERE workspace_id='$WS_C'"
if [[ "$FLAG_ON_CODE" == "0" && "$FLAG_OFF_CODE" != "0" && "$FLAG_CMD_CODE" != "0" && "$FLAG_AFTER" == "$FLAG_BEFORE" ]]; then
  record "disabled_flow_flag_fails_closed_zero_write" true \
    "an owner of a workspace with flow_enabled=false => list and set_title both refused with the document unchanged, while the identical list with the flag on succeeds" \
    "list_on=$FLAG_ON_CODE list_off=$FLAG_OFF_CODE command=$FLAG_CMD_CODE counts $FLAG_BEFORE -> $FLAG_AFTER"
else
  record "disabled_flow_flag_fails_closed_zero_write" false \
    "an owner of a workspace with flow_enabled=false => list and set_title both refused with the document unchanged, while the identical list with the flag on succeeds" \
    "list_on=$FLAG_ON_CODE list_off=$FLAG_OFF_CODE command=$FLAG_CMD_CODE counts $FLAG_BEFORE -> $FLAG_AFTER"
fi

# ===================================== POLICY BYPASS: MISSING/FORGED TOKENS
NOAUTH_BEFORE="$(doc_write_counts "$DOC_A")"
NOAUTH="$(send_command "$OBJ_A" "")"
NOAUTH_CODE="$(envelope_code "$NOAUTH")"
NOAUTH_AFTER="$(doc_write_counts "$DOC_A")"
FORGED="$(curl -sS "$BASE/api/v1/flow/objects/$OBJ_A" -H "Authorization: Bearer opr_forged_${RUN_ID}")"
FORGED_CODE="$(envelope_code "$FORGED")"
GARBAGE="$(curl -sS "$BASE/api/v1/flow/objects/$OBJ_A" -H "Authorization: Bearer not-a-real-jwt")"
GARBAGE_CODE="$(envelope_code "$GARBAGE")"
if [[ "$NOAUTH_CODE" == "401" && "$NOAUTH_AFTER" == "$NOAUTH_BEFORE" && "$FORGED_CODE" == "401" && "$GARBAGE_CODE" == "401" ]]; then
  record "absent_and_forged_credentials_rejected_zero_write" true \
    "no Authorization header, a forged opr_ bot token and a garbage bearer all => envelope code 401, with the document unchanged" \
    "no_auth=$NOAUTH_CODE forged_bot=$FORGED_CODE garbage=$GARBAGE_CODE counts $NOAUTH_BEFORE -> $NOAUTH_AFTER"
else
  record "absent_and_forged_credentials_rejected_zero_write" false \
    "no Authorization header, a forged opr_ bot token and a garbage bearer all => envelope code 401, with the document unchanged" \
    "no_auth=$NOAUTH_CODE forged_bot=$FORGED_CODE garbage=$GARBAGE_CODE counts $NOAUTH_BEFORE -> $NOAUTH_AFTER"
fi

# ============================================================ gate rollup
REQUIRED_IDS=(
  same_workspace_get_and_update_control
  cross_workspace_object_get_rejected_not_found_safe
  cross_workspace_object_list_rejected
  cross_workspace_bootstrap_rejected_no_crdt_bytes
  cross_workspace_collab_diagnostics_rejected
  cross_workspace_command_rejected_zero_write
  cross_workspace_link_rejected_zero_write
  same_workspace_link_control
  cross_workspace_collab_ticket_rejected_zero_write
  bot_token_confined_to_its_own_workspace
  read_only_bot_cannot_write_zero_write
  bot_excluded_from_user_only_crdt_surfaces
  disabled_flow_flag_fails_closed_zero_write
  absent_and_forged_credentials_rejected_zero_write
)
MISSING=(); FAILED=()
for id in "${REQUIRED_IDS[@]}"; do
  entry="$(jq -r --arg id "$id" '[.[] | select(.id==$id)] | if length==0 then "missing" else (.[0].passed|tostring) end' <<<"$CHECKS_JSON")"
  case "$entry" in
    missing) MISSING+=("$id") ;;
    false) FAILED+=("$id") ;;
    true) ;;
    *) FAILED+=("$id(unparsable)") ;;
  esac
done
if [[ ${#MISSING[@]} -eq 0 && ${#FAILED[@]} -eq 0 ]]; then
  VERDICT="$(jq -n --argjson n "${#REQUIRED_IDS[@]}" \
    '{status:"passed", reason:("all " + ($n|tostring) + " required negative/control checks passed")}')"
else
  VERDICT="$(jq -n --arg failed "${FAILED[*]-}" --arg missing "${MISSING[*]-}" \
    '{status:"failed", reason:("failed=[" + $failed + "] missing=[" + $missing + "]")}')"
fi

RESULT="$(jq -n \
  --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" --arg tm "$THREAT_MODEL" \
  --argjson checks "$CHECKS_JSON" --argjson verdict "$VERDICT" \
  '{
    schema_version: "sylvode.flow.cross-workspace-result.v1",
    source_head: $head,
    generated_at: $generated_at,
    threat_model: $tm,
    gates: { cross_workspace_and_policy_bypass_negative: $verdict },
    checks: $checks,
    counts: {
      checks_total: ($checks | length),
      checks_passed: ([$checks[] | select(.passed)] | length),
      checks_failed: ([$checks[] | select(.passed | not)] | length)
    },
    passed: ($verdict.status == "passed")
  }')"

OUT_PATH="$EVIDENCE_ROOT/cross-workspace-negative-result.json"
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
