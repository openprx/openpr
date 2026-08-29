#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 authz-baseline verifier.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md, "Authz-
# baseline verifier" paragraph, and ADR-0012 §2-3.
#
# This script covers `flow_parent_authority_in_postgres` with a mix of
# static schema/source introspection and one live end-to-end request
# against the real `api` binary. It does NOT cover
# `member_baseline_no_behaviour_regression` (the default_member_level
# before/after comparison across REST, all three MCP transports and CLI)
# -- that needs a much larger fixture (MCP HTTP/SSE/stdio + CLI binary
# equivalence testing) this round did not build, and this is reported
# explicitly rather than silently skipped.
#
# flow_parent_authority_in_postgres assertions:
#   1. STATIC: `flow_object_projections` (information_schema.columns) has
#      no parent/parent_id-shaped column at all -- so `nodes[].parent_id`
#      / `parent_id` filtering CANNOT be sourced from the projection
#      table; there is nothing there to read even if the code wanted to.
#   2. STATIC (read-only source grep, never edits apps/**): the read
#      queries in apps/api/src/flow/repository.rs select `fo.parent_id`
#      (the `flow_objects` alias), not any projection-table alias.
#   3. STATIC: v0.4's registered command wire names
#      (apps/api/src/flow/command.rs) contain no cross-parent "move"
#      command -- a v0.4 navigator drag cannot invoke anything that
#      changes `parent_id` between different parents; only v0.4's content
#      commands (which never touch `parent_id`) and the parent-less
#      `create_object`/lifecycle commands exist.
#   4. LIVE: create a parent object and a child object with
#      `parent_object_id` set through the real REST create endpoint (so
#      `flow_objects`, `collab_documents` and `flow_object_projections`
#      are all populated exactly as production would), then GET
#      `/workspaces/{id}/flow/objects?parent_id=<parent>` and assert the
#      child appears with the SAME parent_id value a direct SQL read of
#      `flow_objects.parent_id` shows -- confirming the live read path
#      actually works end-to-end, not just in theory.
#
# Exit codes: 0 = flow_parent_authority_in_postgres assertions all
# passed AND member_baseline is explicitly reported not_covered (so
# overall `passed` is still false -- see script tail), 1 = an assertion
# failed, 2 = usage/tool/environment error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
ADR_PATH=""
DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-}"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-authz-baseline-v0.4.sh --adr PATH --json [OPTIONS]

Verifies flow_parent_authority_in_postgres via static schema/source
introspection plus one live request against the real api binary.
Explicitly reports member_baseline_no_behaviour_regression as
not_covered (needs MCP 3-transport + CLI equivalence testing not built
this round). Writes evidence/v0.4/authz-baseline-result.json.

Options:
  --adr PATH              Path to ADR-0012. Required.
  --database-url URL       Postgres DSN. Default: $OPENPR_TEST_DATABASE_URL
  --repo-root DIR         Repository containing apps/api. Default: this
                          checkout.
  --evidence-root DIR     Where authz-baseline-result.json is written.
                          Default: /opt/working/sylvode-flow/evidence/v0.4
  --json                  Required for CLI-contract compatibility.
  -h, --help              Show this help and exit 0.

Exit codes: 0 never today (member_baseline gap always present, see
header), 1 an assertion failed, 2 usage/tool/environment error.
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

if [[ -z "$ADR_PATH" ]]; then
  echo "FAIL: --adr is required" >&2
  usage >&2
  exit 2
fi
if [[ ! -f "$ADR_PATH" ]]; then
  echo "FAIL: --adr file not found: $ADR_PATH" >&2
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
COMMAND_RS="$REPO_ROOT/apps/api/src/flow/command.rs"
REPOSITORY_RS="$REPO_ROOT/apps/api/src/flow/repository.rs"
for f in "$COMMAND_RS" "$REPOSITORY_RS"; do
  if [[ ! -f "$f" ]]; then
    echo "FAIL: source file not found (nothing to statically verify): $f" >&2
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

VIOLATIONS=()

# ---- static 1: flow_object_projections has no parent-shaped column ----
PROJ_COLUMNS="$(psql "$DATABASE_URL" -Atc "SELECT string_agg(column_name, ',') FROM information_schema.columns WHERE table_schema='public' AND table_name='flow_object_projections'")"
if grep -qi "parent" <<<"$PROJ_COLUMNS"; then
  VIOLATIONS+=("flow_object_projections has a parent-shaped column ($PROJ_COLUMNS) -- parent authority may leak into the projection table")
fi
echo "static check 1: flow_object_projections columns = $PROJ_COLUMNS" >&2

# ---- static 2: read queries select fo.parent_id, not a projection alias ----
if ! grep -q "fo\.parent_id" "$REPOSITORY_RS"; then
  VIOLATIONS+=("$REPOSITORY_RS: no 'fo.parent_id' select found -- cannot confirm parent_id is read from the flow_objects alias")
fi
if grep -qE "p\.parent_id|projection[a-z_]*\.parent_id" "$REPOSITORY_RS"; then
  VIOLATIONS+=("$REPOSITORY_RS: found a parent_id reference on a projection-table alias")
fi
echo "static check 2: fo.parent_id present=$(grep -c 'fo\.parent_id' "$REPOSITORY_RS") projection-alias parent_id refs=$(grep -cE 'p\.parent_id|projection[a-z_]*\.parent_id' "$REPOSITORY_RS")" >&2

# ---- static 3: no cross-parent "move" command registered in v0.4 ----
WIRE_NAMES="$(grep -oE '"[a-z_]+" =>' "$COMMAND_RS" | sed -E 's/"([a-z_]+)" =>/\1/' | sort -u | tr '\n' ',' )"
if grep -qE "move_object|change_parent|reparent" <<<"$WIRE_NAMES"; then
  VIOLATIONS+=("a cross-parent command wire name was found in v0.4's registry: $WIRE_NAMES")
fi
echo "static check 3: v0.4 command wire names = $WIRE_NAMES" >&2

# ---- live end-to-end check ----
echo "=== building api binary (cargo build -p api --bin api) ===" >&2
( cd "$REPO_ROOT" && cargo build -q -p api --bin api ) || {
  echo "FAIL: api binary failed to build" >&2
  exit 2
}
API_BIN="$REPO_ROOT/target/debug/api"

RUN_ID="$(python3 -c 'import uuid; print(uuid.uuid4().hex[:8])')"
TMP_DIR="$(mktemp -d "/tmp/openpr-authz-baseline-verify.XXXXXX")"
API_PORT=$((22000 + RANDOM % 20000))
API_LOG="$TMP_DIR/api.log"
API_PID=""
WORKSPACE_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
OWNER_USER="$(python3 -c 'import uuid; print(uuid.uuid4())')"
BOT_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
BOT_TOKEN="opr_authz_verify_${RUN_ID}"

# shellcheck disable=SC2317  # invoked only via `trap ... EXIT` below.
cleanup() {
  local ec=$?
  if [[ -n "$API_PID" ]] && kill -0 "$API_PID" 2>/dev/null; then
    kill "$API_PID" 2>/dev/null || true
    wait "$API_PID" 2>/dev/null || true
  fi
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q >/dev/null 2>&1 <<SQL || true
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

APP_CONFIG="$TMP_DIR/openpr.toml"
cat > "$APP_CONFIG" <<EOF
[server]
app_name = "api"
bind_addr = "127.0.0.1:$API_PORT"

[database]
url = "$DATABASE_URL"

[auth]
jwt_secret = "authz-baseline-verify-not-a-real-secret"

[logging]
filter = "api=info,openpr=info"
format = "text"
EOF

BOT_TOKEN_HASH="$(printf '%s' "$BOT_TOKEN" | sha256sum | awk '{print $1}')"
BOT_TOKEN_PREFIX="${BOT_TOKEN:0:8}"
# A bot's actor identity is a *mirrored* `users` row with the same id as its
# `workspace_bots` row (apps/api/src/routes/bot.rs's real bot-creation path,
# read-only reference) -- `flow_objects.created_by` has an FK to `users(id)`,
# so a bot without this mirror row cannot author a flow object at all.
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES ('$OWNER_USER', 'authz-verify-$RUN_ID@example.local', '', 'Authz Verify Owner', 'user', true, 'human', NULL, now(), now());
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES ('$BOT_ID', 'authz-verify-bot-$RUN_ID@bot.openpr.local', '!', 'Authz Verify Bot', 'user', true, 'bot_mcp', 'mcp', now(), now());
INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE_ID', 'authz-verify-$RUN_ID', 'Authz Verify', '$OWNER_USER', now(), now());
INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES
  ('$WORKSPACE_ID', '$OWNER_USER', 'owner', now()),
  ('$WORKSPACE_ID', '$BOT_ID', 'member', now());
INSERT INTO flow_workspace_settings (workspace_id, flow_enabled, default_member_level, authz_epoch, updated_at)
VALUES ('$WORKSPACE_ID', true, 'edit', 0, now());
INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by, is_active, created_at, updated_at)
VALUES ('$BOT_ID', '$WORKSPACE_ID', 'Authz Verify Bot', '$BOT_TOKEN_HASH', '$BOT_TOKEN_PREFIX', '["read","write"]'::jsonb, '$OWNER_USER', true, now(), now());
SQL

"$API_BIN" --config "$APP_CONFIG" > "$API_LOG" 2>&1 &
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

create_object() {
  local parent_json="$1" key
  key="$(python3 -c 'import uuid; print(uuid.uuid4())')"
  curl -sS -X POST "http://127.0.0.1:$API_PORT/api/v1/workspaces/$WORKSPACE_ID/flow/objects" \
    -H "Authorization: Bearer $BOT_TOKEN" -H "Content-Type: application/json" \
    -d "$(jq -n --arg key "$key" --argjson parent "$parent_json" \
      '{object_type:"page", title:"Authz baseline fixture", idempotency_key:$key} + (if $parent == null then {} else {parent_object_id:$parent} end)')"
}

PARENT_RESP="$(create_object null)"
PARENT_ID="$(jq -r '.data.object.id // empty' <<<"$PARENT_RESP")"
if [[ -z "$PARENT_ID" ]]; then
  echo "FAIL: could not create parent object via live REST call: $PARENT_RESP" >&2
  cat "$API_LOG" >&2
  exit 2
fi
CHILD_RESP="$(create_object "\"$PARENT_ID\"")"
CHILD_ID="$(jq -r '.data.object.id // empty' <<<"$CHILD_RESP")"
if [[ -z "$CHILD_ID" ]]; then
  echo "FAIL: could not create child object via live REST call: $CHILD_RESP" >&2
  cat "$API_LOG" >&2
  exit 2
fi

LIST_RESP="$(curl -sS "http://127.0.0.1:$API_PORT/api/v1/workspaces/$WORKSPACE_ID/flow/objects?parent_id=$PARENT_ID" -H "Authorization: Bearer $BOT_TOKEN")"
LIST_HAS_CHILD="$(jq --arg id "$CHILD_ID" '[.data.items[]? | select(.id==$id)] | length' <<<"$LIST_RESP")"
if [[ "$LIST_HAS_CHILD" != "1" ]]; then
  VIOLATIONS+=("GET .../flow/objects?parent_id=$PARENT_ID did not return the child object (response: $LIST_RESP)")
fi
SQL_PARENT_ID="$(psql "$DATABASE_URL" -Atc "SELECT parent_id FROM flow_objects WHERE id='$CHILD_ID'")"
if [[ "$SQL_PARENT_ID" != "$PARENT_ID" ]]; then
  VIOLATIONS+=("direct SQL read of flow_objects.parent_id ($SQL_PARENT_ID) does not match the parent used to create the child ($PARENT_ID)")
fi

PASSED_PARENT_AUTHORITY=$([[ ${#VIOLATIONS[@]} -eq 0 ]] && echo true || echo false)
VIOLATIONS_JSON="$(printf '%s\n' "${VIOLATIONS[@]:-}" | jq -R 'select(length>0)' | jq -s '.')"

RESULT="$(jq -n \
  --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" --arg adr "$ADR_PATH" \
  --arg proj_columns "$PROJ_COLUMNS" --arg wire_names "$WIRE_NAMES" \
  --argjson violations "$VIOLATIONS_JSON" --argjson parent_authority_passed "$PASSED_PARENT_AUTHORITY" \
  '{
    schema_version: "sylvode.flow.authz-baseline-result.v1",
    source_head: $head,
    generated_at: $generated_at,
    adr: $adr,
    flow_parent_authority_in_postgres: {
      flow_object_projections_columns: $proj_columns,
      v0_4_command_wire_names: $wire_names,
      violations: $violations,
      passed: $parent_authority_passed
    },
    member_baseline_no_behaviour_regression: {
      status: "not_covered",
      reason: "requires a default_member_level=edit before/after fixture compared across REST, all three MCP transports (HTTP/SSE/stdio) and CLI, including archive|restore -- not built this round"
    },
    passed: false
  }')"

OUT_PATH="$EVIDENCE_ROOT/authz-baseline-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$RESULT" | jq . > "$OUT_TMP"
sync "$OUT_TMP" 2>/dev/null || true
mv -f "$OUT_TMP" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2

echo "$RESULT"
exit 1
