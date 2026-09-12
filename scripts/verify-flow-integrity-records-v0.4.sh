#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 integrity-record fail-closed verifier.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md, "Integrity-
# record verifier" paragraph, and ADR-0013 §4.
#
# This is a LIVE end-to-end check, not a static/schema check: it builds
# and boots the real `api` binary against a Postgres database, seeds two
# isolated workspaces directly via SQL, and sends one real HTTP request
# designed to trip the "cross workspace relation" invariant a database
# constraint alone cannot catch (apps/api/src/flow/command.rs
# `record_cross_workspace_relation_and_fail_closed`, read-only reference
# -- this script never edits apps/**): a `create_object` call whose
# `parent_object_id` names a real `flow_objects` row that belongs to a
# DIFFERENT workspace than the request's own `workspace_id`.
#
# It asserts every invariant gate-commands.md names for this gate:
#   - the request is rejected wire-visibly as `invalid_update`
#     (error-mapping-v1.md; HTTP 200 + envelope code=400 in this repo's
#     REST convention, per rest-api-v1.md "统一 wire contract")
#   - zero rows are added to flow_objects / collab_documents /
#     business_events / event_dispatch for the requesting workspace
#     (canonical state and event_dispatch are untouched -- true fail
#     closed, not "rejected after partially writing")
#   - exactly one `flow_integrity_records` row is inserted, with
#     status=open, kind=cross_workspace_relation,
#     detected_by=flow.command.create_object, subject_kind=flow_object,
#     subject_id=<the referenced object id>
#   - `details_redacted` carries only the two workspace ids (no title,
#     no body_md/semantic content, no CRDT bytes)
#
# It does NOT cover the second invariant class gate-commands.md names in
# the same sentence ("悬空 navigator 排序条目" / dangling navigator
# ordering entries): that is a v0.5 feature (`ADR-0013` §1's "带 parent
# 的 create 只锁 navigator" case) that does not exist in the v0.4 code
# this repository ships, so there is nothing to exercise yet. This is
# recorded explicitly in the output as `navigator_ordering_check:
# not_applicable_v0.4`, never silently omitted.
#
# Exit codes: 0 = every assertion above passed, 1 = one or more
# assertions failed (specific violations are listed), 2 = usage/tool/
# environment error (binary would not build/boot, database unreachable).

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Shared --adr/--contract/--limits path resolution (absolute -> as-is;
# relative-to-CWD -> as-is; otherwise resolved against --contracts-root;
# unresolvable -> FAIL naming both attempted paths).
# shellcheck source=scripts/lib/flow_contract_path.sh
source "$ROOT_DIR/scripts/lib/flow_contract_path.sh"
REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
ADR_PATH=""
DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-}"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-integrity-records-v0.4.sh --adr PATH --json [OPTIONS]

Boots the real `api` binary against a Postgres database, seeds two
isolated throwaway workspaces, sends one real HTTP request that trips the
"cross workspace relation" fail-closed path, and asserts the invariants
listed in the file header against the live database afterwards. Writes
evidence/v0.4/integrity-records-result.json.

Options:
  --adr PATH              Path to ADR-0013. Required (recorded in output).
                          A relative path is resolved against the current
                          directory first, then against --contracts-root.
  --contracts-root DIR     Root containing decisions/. Default:
                          /opt/working/sylvode-flow
  --database-url URL       Postgres DSN the api binary and this script's
                          own assertions connect to. Default:
                          $OPENPR_TEST_DATABASE_URL
  --repo-root DIR         Repository containing apps/api. Default: this
                          checkout.
  --evidence-root DIR     Where integrity-records-result.json is written.
                          Default: /opt/working/sylvode-flow/evidence/v0.4
  --json                  Required for CLI-contract compatibility.
  -h, --help              Show this help and exit 0.

Exit codes: 0 all assertions passed, 1 an assertion failed,
2 usage/tool/environment error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --adr) ADR_PATH="${2:?--adr requires a PATH argument}"; shift 2 ;;
    --database-url) DATABASE_URL="${2:?--database-url requires a value}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$ADR_PATH" ]]; then
  echo "FAIL: --adr is required" >&2
  usage >&2
  exit 2
fi
if ! ADR_PATH="$(flow_resolve_contract_path --adr "$ADR_PATH" "$CONTRACTS_ROOT")"; then
  exit 2
fi
if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
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
CARGO_OUTPUT_DIR="${CARGO_TARGET_DIR:-target}"
if [[ "$CARGO_OUTPUT_DIR" != /* ]]; then
  CARGO_OUTPUT_DIR="$REPO_ROOT/$CARGO_OUTPUT_DIR"
fi

echo "=== building api binary (cargo build -p api --bin api) ===" >&2
( cd "$REPO_ROOT" && cargo build -q -p api --bin api ) || {
  echo "FAIL: api binary failed to build" >&2
  exit 2
}
API_BIN="$CARGO_OUTPUT_DIR/debug/api"
if [[ ! -x "$API_BIN" ]]; then
  echo "FAIL: api binary not found after build: $API_BIN" >&2
  exit 2
fi

RUN_ID="$(python3 -c 'import uuid; print(uuid.uuid4().hex[:8])')"
TMP_DIR="$(mktemp -d "/tmp/openpr-integrity-records-verify.XXXXXX")"
API_PORT=$((20000 + RANDOM % 20000))
API_LOG="$TMP_DIR/api.log"
API_PID=""

WORKSPACE_A="$(python3 -c 'import uuid; print(uuid.uuid4())')"
WORKSPACE_B="$(python3 -c 'import uuid; print(uuid.uuid4())')"
OWNER_USER="$(python3 -c 'import uuid; print(uuid.uuid4())')"
BOT_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
BOT_TOKEN="opr_integrity_verify_${RUN_ID}"
PARENT_OBJECT_B=""

# shellcheck disable=SC2317  # invoked only via `trap ... EXIT` below; shellcheck's
# reachability analysis does not follow that call path.
cleanup() {
  local ec=$?
  if [[ -n "$API_PID" ]] && kill -0 "$API_PID" 2>/dev/null; then
    kill "$API_PID" 2>/dev/null || true
    wait "$API_PID" 2>/dev/null || true
  fi
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q >/dev/null 2>&1 <<SQL || true
DELETE FROM flow_integrity_records WHERE workspace_id IN ('$WORKSPACE_A','$WORKSPACE_B');
DELETE FROM flow_objects WHERE workspace_id IN ('$WORKSPACE_A','$WORKSPACE_B');
DELETE FROM flow_workspace_settings WHERE workspace_id IN ('$WORKSPACE_A','$WORKSPACE_B');
DELETE FROM workspace_bots WHERE id = '$BOT_ID';
DELETE FROM workspace_members WHERE workspace_id IN ('$WORKSPACE_A','$WORKSPACE_B');
DELETE FROM workspaces WHERE id IN ('$WORKSPACE_A','$WORKSPACE_B');
DELETE FROM users WHERE id = '$OWNER_USER';
SQL
  rm -rf "$TMP_DIR"
  exit "$ec"
}
trap cleanup EXIT

APP_CONFIG="$TMP_DIR/openpr.toml"
cat > "$APP_CONFIG" <<EOF
[server]
app_name = "api"
bind_addr = "127.0.0.1:$API_PORT"

[database]
url = "$DATABASE_URL"

[auth]
jwt_secret = "integrity-records-verify-not-a-real-secret"

[logging]
filter = "api=info,openpr=info"
format = "text"
EOF

echo "=== seeding isolated test workspaces ($WORKSPACE_A requester, $WORKSPACE_B target) ===" >&2
BOT_TOKEN_HASH="$(printf '%s' "$BOT_TOKEN" | sha256sum | awk '{print $1}')"
BOT_TOKEN_PREFIX="${BOT_TOKEN:0:8}"
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, created_at, updated_at)
VALUES ('$OWNER_USER', 'integrity-verify-$RUN_ID@example.local', '', 'Integrity Verify Owner', 'user', true, 'human', now(), now());

INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES
  ('$WORKSPACE_A', 'integrity-verify-a-$RUN_ID', 'Integrity Verify A', '$OWNER_USER', now(), now()),
  ('$WORKSPACE_B', 'integrity-verify-b-$RUN_ID', 'Integrity Verify B', '$OWNER_USER', now(), now());

INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES ('$WORKSPACE_A', '$OWNER_USER', 'owner', now());

INSERT INTO flow_workspace_settings (workspace_id, flow_enabled, default_member_level, authz_epoch, updated_at)
VALUES ('$WORKSPACE_A', true, 'edit', 0, now());

INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by, is_active, created_at, updated_at)
VALUES ('$BOT_ID', '$WORKSPACE_A', 'Integrity Verify Bot', '$BOT_TOKEN_HASH', '$BOT_TOKEN_PREFIX', '["read","write"]'::jsonb, '$OWNER_USER', true, now(), now());
SQL

# Migration 0059 creates one system navigator root transactionally with each
# workspace. Reuse that canonical foreign parent; inserting a second root is a
# stale-fixture unique-key failure and never reaches the integrity behavior.
PARENT_OBJECT_B="$(psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atc \
  "SELECT id FROM flow_objects WHERE workspace_id='$WORKSPACE_B' AND parent_id IS NULL AND governance_metadata->>'system_role'='workspace_navigator_root'")"
if [[ -z "$PARENT_OBJECT_B" ]]; then
  echo "FAIL: workspace B has no canonical navigator root after creation" >&2
  exit 2
fi

echo "=== starting api on 127.0.0.1:$API_PORT ===" >&2
"$API_BIN" --config "$APP_CONFIG" > "$API_LOG" 2>&1 &
API_PID=$!
HEALTHY=0
for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1; then
    HEALTHY=1
    break
  fi
  sleep 0.5
done
if [[ $HEALTHY -ne 1 ]]; then
  echo "FAIL: api did not become healthy within 30s; log follows" >&2
  cat "$API_LOG" >&2
  exit 2
fi

VIOLATIONS=()
IDEMPOTENCY_KEY="$(python3 -c 'import uuid; print(uuid.uuid4())')"

# Migration 0059 gives every workspace a canonical navigator root and its
# paired collab document. Capture the real baseline so the rejection assertion
# detects request-caused writes without misclassifying those required rows.
FLOW_OBJECTS_A_BEFORE="$(psql "$DATABASE_URL" -Atc "SELECT count(*) FROM flow_objects WHERE workspace_id='$WORKSPACE_A'")"
BUSINESS_EVENTS_A_BEFORE="$(psql "$DATABASE_URL" -Atc "SELECT count(*) FROM business_events WHERE workspace_id='$WORKSPACE_A'")"
EVENT_DISPATCH_A_BEFORE="$(psql "$DATABASE_URL" -Atc "SELECT count(*) FROM event_dispatch WHERE workspace_id='$WORKSPACE_A'")"
COLLAB_DOCS_A_BEFORE="$(psql "$DATABASE_URL" -Atc "SELECT count(*) FROM collab_documents cd JOIN flow_objects fo ON fo.id = cd.object_id WHERE fo.workspace_id='$WORKSPACE_A'")"

echo "=== sending cross-workspace parent_object_id request ===" >&2
RESPONSE="$(curl -sS -X POST "http://127.0.0.1:$API_PORT/api/v1/workspaces/$WORKSPACE_A/flow/objects" \
  -H "Authorization: Bearer $BOT_TOKEN" -H "Content-Type: application/json" \
  -d "$(jq -n --arg parent "$PARENT_OBJECT_B" --arg key "$IDEMPOTENCY_KEY" \
    '{object_type:"page", parent_object_id:$parent, title:"Cross-workspace integrity fixture", idempotency_key:$key}')")"

RESP_CODE="$(jq -r '.code // empty' <<<"$RESPONSE" 2>/dev/null || echo "")"
RESP_MESSAGE="$(jq -r '.message // empty' <<<"$RESPONSE" 2>/dev/null || echo "")"
if [[ "$RESP_CODE" != "400" ]]; then
  VIOLATIONS+=("expected envelope code=400, got code='$RESP_CODE' (full response: $RESPONSE)")
fi
if [[ "$RESP_MESSAGE" != "invalid_update" ]]; then
  VIOLATIONS+=("expected envelope message='invalid_update', got '$RESP_MESSAGE'")
fi

# ---- zero canonical-state side effects for workspace A ----
FLOW_OBJECTS_A="$(psql "$DATABASE_URL" -Atc "SELECT count(*) FROM flow_objects WHERE workspace_id='$WORKSPACE_A'")"
if [[ "$FLOW_OBJECTS_A" != "$FLOW_OBJECTS_A_BEFORE" ]]; then
  VIOLATIONS+=("flow_objects count changed $FLOW_OBJECTS_A_BEFORE -> $FLOW_OBJECTS_A in workspace A -- request did not fail closed before writing canonical state")
fi
BUSINESS_EVENTS_A="$(psql "$DATABASE_URL" -Atc "SELECT count(*) FROM business_events WHERE workspace_id='$WORKSPACE_A'")"
if [[ "$BUSINESS_EVENTS_A" != "$BUSINESS_EVENTS_A_BEFORE" ]]; then
  VIOLATIONS+=("business_events count changed $BUSINESS_EVENTS_A_BEFORE -> $BUSINESS_EVENTS_A in workspace A")
fi
EVENT_DISPATCH_A="$(psql "$DATABASE_URL" -Atc "SELECT count(*) FROM event_dispatch WHERE workspace_id='$WORKSPACE_A'")"
if [[ "$EVENT_DISPATCH_A" != "$EVENT_DISPATCH_A_BEFORE" ]]; then
  VIOLATIONS+=("event_dispatch count changed $EVENT_DISPATCH_A_BEFORE -> $EVENT_DISPATCH_A in workspace A")
fi
COLLAB_DOCS_A="$(psql "$DATABASE_URL" -Atc "SELECT count(*) FROM collab_documents cd JOIN flow_objects fo ON fo.id = cd.object_id WHERE fo.workspace_id='$WORKSPACE_A'")"
if [[ "$COLLAB_DOCS_A" != "$COLLAB_DOCS_A_BEFORE" ]]; then
  VIOLATIONS+=("collab_documents count changed $COLLAB_DOCS_A_BEFORE -> $COLLAB_DOCS_A for objects in workspace A")
fi

# ---- exactly one flow_integrity_records row, with the right shape ----
IR_ROWS="$(psql "$DATABASE_URL" -Atc "SELECT count(*) FROM flow_integrity_records WHERE workspace_id='$WORKSPACE_A'")"
if [[ "$IR_ROWS" != "1" ]]; then
  VIOLATIONS+=("expected exactly 1 flow_integrity_records row for workspace A, found $IR_ROWS")
else
  IR_ROW_JSON="$(psql "$DATABASE_URL" -Atc "SELECT row_to_json(r) FROM (SELECT kind, subject_kind, subject_id, detected_by, status, details_redacted FROM flow_integrity_records WHERE workspace_id='$WORKSPACE_A') r")"
  ir_kind="$(jq -r '.kind' <<<"$IR_ROW_JSON")"
  ir_subject_kind="$(jq -r '.subject_kind' <<<"$IR_ROW_JSON")"
  ir_subject_id="$(jq -r '.subject_id' <<<"$IR_ROW_JSON")"
  ir_detected_by="$(jq -r '.detected_by' <<<"$IR_ROW_JSON")"
  ir_status="$(jq -r '.status' <<<"$IR_ROW_JSON")"
  [[ "$ir_kind" == "cross_workspace_relation" ]] || VIOLATIONS+=("flow_integrity_records.kind='$ir_kind' (expected cross_workspace_relation)")
  [[ "$ir_subject_kind" == "flow_object" ]] || VIOLATIONS+=("flow_integrity_records.subject_kind='$ir_subject_kind' (expected flow_object)")
  [[ "$ir_subject_id" == "$PARENT_OBJECT_B" ]] || VIOLATIONS+=("flow_integrity_records.subject_id='$ir_subject_id' (expected $PARENT_OBJECT_B)")
  [[ "$ir_detected_by" == "flow.command.create_object" ]] || VIOLATIONS+=("flow_integrity_records.detected_by='$ir_detected_by' (expected flow.command.create_object)")
  [[ "$ir_status" == "open" ]] || VIOLATIONS+=("flow_integrity_records.status='$ir_status' (expected open)")

  DETAILS_JSON="$(jq -c '.details_redacted' <<<"$IR_ROW_JSON")"
  DETAILS_KEYS="$(jq -r 'keys | sort | join(",")' <<<"$DETAILS_JSON")"
  if [[ "$DETAILS_KEYS" != "referenced_workspace_id,requesting_workspace_id" ]]; then
    VIOLATIONS+=("details_redacted has unexpected key set '$DETAILS_KEYS' (expected exactly referenced_workspace_id,requesting_workspace_id -- any other key risks leaking content)")
  fi
  for forbidden in title body_md content bytes update snapshot tail; do
    if jq -e "has(\"$forbidden\")" <<<"$DETAILS_JSON" >/dev/null 2>&1; then
      VIOLATIONS+=("details_redacted contains forbidden key '$forbidden'")
    fi
  done
fi

PASSED=$([[ ${#VIOLATIONS[@]} -eq 0 ]] && echo true || echo false)
VIOLATIONS_JSON="$(printf '%s\n' "${VIOLATIONS[@]:-}" | jq -R 'select(length>0)' | jq -s '.')"

RESULT="$(jq -n \
  --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" --arg adr "$ADR_PATH" \
  --arg response "$RESPONSE" \
  --argjson violations "$VIOLATIONS_JSON" --argjson passed "$PASSED" \
  '{
    schema_version: "sylvode.flow.integrity-records-result.v1",
    source_head: $head,
    generated_at: $generated_at,
    adr: $adr,
    fixture: "cross_workspace_relation (parent_object_id references a flow_objects row in a different workspace)",
    navigator_ordering_check: "not_applicable_v0.4 (dangling navigator ordering is a v0.5 feature; nothing to exercise in this repository yet)",
    http_response: $response,
    violations: $violations,
    passed: $passed
  }')"

OUT_PATH="$EVIDENCE_ROOT/integrity-records-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$RESULT" | jq . > "$OUT_TMP"
sync "$OUT_TMP" 2>/dev/null || true
mv -f "$OUT_TMP" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2

echo "$RESULT"
if [[ "$PASSED" == "true" ]]; then
  exit 0
fi
exit 1
