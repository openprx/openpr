#!/usr/bin/env bash
set -euo pipefail

# Live Sylvode Flow v0.4 canonical-document integrity verifier.
#
# The fixture is created and mutated through a real api process backed by a
# scratch PostgreSQL database.  The verifier then reads the authoritative
# snapshot/tail/projection rows directly from PostgreSQL and hands their bytes
# to a separate collab-core replay process.  Finally it terminates the api,
# observes that PID dead, starts a new api process against the same database,
# bootstraps the same document again, and compares semantic hashes.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
EVIDENCE_ROOT=""
DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-}"
RELEASE="0.4"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-document-integrity-v0.4.sh --release 0.4 --json [OPTIONS]

Options:
  --release VER          Must be 0.4.
  --database-url URL     PostgreSQL DSN on which a scratch database may be created.
                         Default: $OPENPR_TEST_DATABASE_URL.
  --repo-root DIR        Product checkout. Default: this checkout.
  --evidence-root DIR    Required output directory.
  --json                 Required.
  -h, --help             Show help.

Exit codes: 0 verified, 1 an integrity assertion failed, 2 usage/environment error.
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
SCRATCH_DB="openpr_flow_integrity_$RUN_ID"
DB_PREFIX="${DATABASE_URL%/*}"
SCRATCH_URL="$DB_PREFIX/$SCRATCH_DB"
TMP_DIR="$(mktemp -d /tmp/openpr-flow-integrity.XXXXXX)"
API_PORT=$((20000 + RANDOM % 18000))
JWT_SECRET="flow-document-integrity-not-a-real-secret"
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
echo "=== building api and independent replay helper ===" >&2
(cd "$REPO_ROOT" && cargo build -q -p api --bin api) || { echo "FAIL: api build failed" >&2; exit 2; }
API_BIN="$TARGET_DIR/debug/api"
[[ -x "$API_BIN" ]] || { echo "FAIL: missing api binary: $API_BIN" >&2; exit 2; }

REPLAY_CRATE="$TMP_DIR/replay-crate"
mkdir -p "$REPLAY_CRATE/src"
cp "$ROOT_DIR/scripts/lib/flow_document_replay.rs" "$REPLAY_CRATE/src/main.rs"
cat > "$REPLAY_CRATE/Cargo.toml" <<EOF
[workspace]

[package]
name = "flow-document-replay-probe"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
base64 = "0.22"
collab-core = { path = "$REPO_ROOT/crates/collab-core" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
EOF
CARGO_TARGET_DIR="$TARGET_DIR" cargo build -q --manifest-path "$REPLAY_CRATE/Cargo.toml" || {
  echo "FAIL: independent replay helper build failed" >&2; exit 2; }
REPLAY_BIN="$TARGET_DIR/debug/flow-document-replay-probe"
[[ -x "$REPLAY_BIN" ]] || { echo "FAIL: missing replay helper: $REPLAY_BIN" >&2; exit 2; }

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

wait_healthy() {
  local pid="$1"
  for _ in $(seq 1 120); do
    if curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1; then return 0; fi
    if ! kill -0 "$pid" 2>/dev/null; then return 1; fi
    sleep 0.25
  done
  return 1
}

start_api() {
  local label="$1"
  "$API_BIN" --config "$API_CONFIG" > "$TMP_DIR/api-$label.log" 2>&1 &
  API_PID=$!
  if ! wait_healthy "$API_PID"; then
    echo "FAIL: api $label did not become healthy" >&2
    tail -40 "$TMP_DIR/api-$label.log" >&2 || true
    exit 2
  fi
}

start_api first
FIRST_PID="$API_PID"

WORKSPACE_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
OWNER_USER="$(python3 -c 'import uuid; print(uuid.uuid4())')"
OWNER_EMAIL="integrity-$RUN_ID@example.local"
psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, created_at, updated_at)
VALUES ('$OWNER_USER', '$OWNER_EMAIL', '', 'Integrity Verify Owner', 'user', true, 'human', now(), now());
INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE_ID', 'integrity-$RUN_ID', 'Integrity Verify', '$OWNER_USER', now(), now());
INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES ('$WORKSPACE_ID', '$OWNER_USER', 'owner', now());
INSERT INTO flow_workspace_settings (workspace_id, flow_enabled, default_member_level, authz_epoch, updated_at)
VALUES ('$WORKSPACE_ID', true, 'edit', 0, now());
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
AUTH=(-H "Authorization: Bearer $USER_JWT" -H 'Content-Type: application/json')

CREATE_RESPONSE="$(curl -sS -X POST "$BASE/api/v1/workspaces/$WORKSPACE_ID/flow/objects" "${AUTH[@]}" \
  -d "$(jq -n --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" \
    '{object_type:"page",title:"Integrity initial",idempotency_key:$key}')")"
OBJECT_ID="$(jq -r '.data.object.id // empty' <<<"$CREATE_RESPONSE")"
[[ -n "$OBJECT_ID" ]] || { echo "FAIL: create fixture failed: $CREATE_RESPONSE" >&2; exit 2; }
DOCUMENT_ID="$(psql "$SCRATCH_URL" -Atc "SELECT id FROM collab_documents WHERE object_id='$OBJECT_ID'")"
[[ -n "$DOCUMENT_ID" ]] || { echo "FAIL: fixture has no collab document" >&2; exit 2; }

command_request() {
  local command_type="$1" payload="$2"
  curl -sS -X POST "$BASE/api/v1/flow/objects/$OBJECT_ID/commands" "${AUTH[@]}" \
    -d "$(jq -n --arg type "$command_type" --argjson payload "$payload" \
      --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" \
      '{command:{type:$type,payload:$payload},idempotency_key:$key}')"
}

INSERT_RESPONSE="$(command_request insert_block '{"block_id":"integrity-block","index":0,"text":"durable block text"}')"
TITLE_RESPONSE="$(command_request set_title '{"title":"Integrity final title"}')"
[[ "$(jq -r '.code // -1' <<<"$INSERT_RESPONSE")" == "0" ]] || {
  echo "FAIL: insert_block failed: $INSERT_RESPONSE" >&2; exit 2; }
[[ "$(jq -r '.code // -1' <<<"$TITLE_RESPONSE")" == "0" ]] || {
  echo "FAIL: set_title failed: $TITLE_RESPONSE" >&2; exit 2; }

# Test-only mutation used by the required anti-proof.  It can only make the
# verifier red, and its presence is recorded in the artifact.
TEST_MUTATION="${FLOW_DOCUMENT_INTEGRITY_TEST_MUTATION:-none}"
case "$TEST_MUTATION" in
  none) ;;
  corrupt_projection_title)
    psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -q \
      -c "UPDATE flow_object_projections SET title='anti-proof corrupted title' WHERE object_id='$OBJECT_ID'" >/dev/null
    ;;
  *) echo "FAIL: unknown FLOW_DOCUMENT_INTEGRITY_TEST_MUTATION=$TEST_MUTATION" >&2; exit 2 ;;
esac

VIOLATIONS='[]'
add_violation() { VIOLATIONS="$(jq -c --arg value "$1" '. + [$value]' <<<"$VIOLATIONS")"; }

DOC_ROW="$(psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -Atc "
SELECT json_build_object(
  'snapshot_base64', replace(encode(cd.snapshot,'base64'), E'\\n', ''),
  'snapshot_frontier_base64', replace(encode(cd.snapshot_frontier,'base64'), E'\\n', ''),
  'snapshot_seq', cd.snapshot_seq,
  'head_frontier_base64', replace(encode(cd.head_frontier,'base64'), E'\\n', ''),
  'head_seq', cd.head_seq,
  'projection', json_build_object(
    'title', p.title,
    'state', p.state,
    'plain_text', p.plain_text,
    'document_seq', p.document_seq,
    'document_frontier_base64', replace(encode(p.document_frontier,'base64'), E'\\n', '')
  )
)
FROM collab_documents cd
JOIN flow_object_projections p ON p.object_id=cd.object_id
WHERE cd.id='$DOCUMENT_ID'")"
TAIL_ROWS="$(psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -Atc "
SELECT coalesce(json_agg(json_build_object(
  'seq', seq,
  'bytes_base64', replace(encode(bytes,'base64'), E'\\n', ''),
  'before_frontier_base64', replace(encode(before_frontier,'base64'), E'\\n', ''),
  'after_frontier_base64', replace(encode(after_frontier,'base64'), E'\\n', ''),
  'content_hash', content_hash
) ORDER BY seq), '[]'::json)
FROM collab_updates
WHERE document_id='$DOCUMENT_ID' AND seq > (SELECT snapshot_seq FROM collab_documents WHERE id='$DOCUMENT_ID')
  AND seq <= (SELECT head_seq FROM collab_documents WHERE id='$DOCUMENT_ID')")"

SNAPSHOT_SEQ="$(jq -r '.snapshot_seq' <<<"$DOC_ROW")"
HEAD_SEQ="$(jq -r '.head_seq' <<<"$DOC_ROW")"
TAIL_COUNT="$(jq 'length' <<<"$TAIL_ROWS")"
EXPECTED_TAIL_COUNT=$((HEAD_SEQ - SNAPSHOT_SEQ))
SEQ_CONTIGUOUS="$(jq -n --argjson rows "$TAIL_ROWS" --argjson start "$SNAPSHOT_SEQ" --argjson head "$HEAD_SEQ" \
  '$rows | map(.seq) == ([$start + 1 | range(.; $head + 1)])')"
[[ "$SNAPSHOT_SEQ" -le "$HEAD_SEQ" ]] || add_violation "snapshot_seq $SNAPSHOT_SEQ exceeds head_seq $HEAD_SEQ"
[[ "$TAIL_COUNT" -eq "$EXPECTED_TAIL_COUNT" ]] || add_violation "tail row count $TAIL_COUNT does not equal head_seq-snapshot_seq $EXPECTED_TAIL_COUNT"
[[ "$SEQ_CONTIGUOUS" == "true" ]] || add_violation "collab_updates seq values are not the complete contiguous snapshot_seq+1..head_seq range"

DB_REPLAY_INPUT="$(jq -n --argjson doc "$DOC_ROW" --argjson tail "$TAIL_ROWS" \
  '{snapshot_base64:$doc.snapshot_base64,
    tail_updates_base64:($tail|map(.bytes_base64)),
    expected_head_frontier_base64:$doc.head_frontier_base64,
    projection:$doc.projection}')"
set +e
DB_REPLAY="$(printf '%s\n' "$DB_REPLAY_INPUT" | "$REPLAY_BIN" 2>"$TMP_DIR/db-replay.err")"
DB_REPLAY_EXIT=$?
set -e
if [[ $DB_REPLAY_EXIT -ne 0 ]] || ! jq -e . >/dev/null 2>&1 <<<"$DB_REPLAY"; then
  add_violation "independent database replay failed: $(tr '\n' ' ' < "$TMP_DIR/db-replay.err")"
  DB_REPLAY='{}'
fi
[[ "$(jq -r '.frontier_matches_head_byte_for_byte // false' <<<"$DB_REPLAY")" == "true" ]] || \
  add_violation "snapshot plus every tail update did not reproduce collab_documents.head_frontier byte-for-byte"
[[ "$(jq -r '.projection_title_matches // false' <<<"$DB_REPLAY")" == "true" ]] || \
  add_violation "flow_object_projections.title differs from canonical replay"
[[ "$(jq -r '.projection_state_matches // false' <<<"$DB_REPLAY")" == "true" ]] || \
  add_violation "flow_object_projections.state differs from canonical replay"
[[ "$(jq -r '.projection_plain_text_matches // false' <<<"$DB_REPLAY")" == "true" ]] || \
  add_violation "flow_object_projections.plain_text differs from canonical replay"
[[ "$(jq -r '.projection.document_seq' <<<"$DOC_ROW")" == "$HEAD_SEQ" ]] || \
  add_violation "projection document_seq does not equal canonical head_seq"
[[ "$(jq -r '.projection.document_frontier_base64' <<<"$DOC_ROW")" == "$(jq -r '.head_frontier_base64' <<<"$DOC_ROW")" ]] || \
  add_violation "projection document_frontier differs byte-for-byte from canonical head_frontier"

bootstrap_replay() {
  local response="$1" label="$2"
  local input
  if [[ "$(jq -r '.code // -1' <<<"$response")" != "0" ]]; then
    add_violation "$label bootstrap did not return code=0"
    printf '{}'
    return
  fi
  input="$(jq -n --argjson response "$response" --argjson projection "$(jq '.projection' <<<"$DOC_ROW")" \
    '{snapshot_base64:$response.data.snapshot_base64,
      tail_updates_base64:($response.data.tail_updates|map(.bytes)),
      expected_head_frontier_base64:$response.data.head_frontier,
      projection:$projection}')"
  set +e
  local output
  output="$(printf '%s\n' "$input" | "$REPLAY_BIN" 2>"$TMP_DIR/$label-replay.err")"
  local ec=$?
  set -e
  if [[ $ec -ne 0 ]] || ! jq -e . >/dev/null 2>&1 <<<"$output"; then
    add_violation "$label bootstrap replay failed: $(tr '\n' ' ' < "$TMP_DIR/$label-replay.err")"
    printf '{}'
  else
    printf '%s' "$output"
  fi
}

BOOTSTRAP_BEFORE="$(curl -sS "$BASE/api/v1/flow/objects/$OBJECT_ID/bootstrap" "${AUTH[@]}")"
REPLAY_BEFORE="$(bootstrap_replay "$BOOTSTRAP_BEFORE" before)"
HASH_BEFORE="$(jq -r '.semantic_hash // empty' <<<"$REPLAY_BEFORE")"
[[ -n "$HASH_BEFORE" ]] || add_violation "pre-restart bootstrap produced no semantic hash"

kill "$FIRST_PID"
for _ in $(seq 1 120); do
  if ! kill -0 "$FIRST_PID" 2>/dev/null; then break; fi
  sleep 0.1
done
FIRST_PROCESS_DEAD=false
if ! kill -0 "$FIRST_PID" 2>/dev/null; then FIRST_PROCESS_DEAD=true; fi
wait "$FIRST_PID" 2>/dev/null || true
API_PID=""
[[ "$FIRST_PROCESS_DEAD" == "true" ]] || add_violation "first api process was not observed dead before restart"

start_api second
SECOND_PID="$API_PID"
[[ "$SECOND_PID" != "$FIRST_PID" ]] || add_violation "api restart reused the same PID"
BOOTSTRAP_AFTER="$(curl -sS "$BASE/api/v1/flow/objects/$OBJECT_ID/bootstrap" "${AUTH[@]}")"
REPLAY_AFTER="$(bootstrap_replay "$BOOTSTRAP_AFTER" after)"
HASH_AFTER="$(jq -r '.semantic_hash // empty' <<<"$REPLAY_AFTER")"
[[ -n "$HASH_AFTER" ]] || add_violation "post-restart bootstrap produced no semantic hash"
[[ -n "$HASH_BEFORE" && "$HASH_BEFORE" == "$HASH_AFTER" ]] || \
  add_violation "semantic hash changed across a real api process restart"
[[ "$HASH_AFTER" == "$(jq -r '.semantic_hash // empty' <<<"$DB_REPLAY")" ]] || \
  add_violation "post-restart bootstrap hash differs from the direct database replay hash"

VIOLATION_COUNT="$(jq 'length' <<<"$VIOLATIONS")"
PASSED=$([[ "$VIOLATION_COUNT" -eq 0 ]] && echo true || echo false)
GATE_STATUS=$([[ "$PASSED" == "true" ]] && echo passed || echo failed)
REASON="real PostgreSQL snapshot+complete-tail replay, projection recomputation, and a killed/restarted api process produced $VIOLATION_COUNT violation(s)"

RESULT="$(jq -n \
  --arg release "$RELEASE" --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" \
  --arg workspace_id "$WORKSPACE_ID" --arg object_id "$OBJECT_ID" --arg document_id "$DOCUMENT_ID" \
  --arg mutation "$TEST_MUTATION" --argjson snapshot_seq "$SNAPSHOT_SEQ" --argjson head_seq "$HEAD_SEQ" \
  --argjson tail_count "$TAIL_COUNT" --argjson seq_contiguous "$SEQ_CONTIGUOUS" \
  --argjson db_replay "$DB_REPLAY" --argjson doc_row "$DOC_ROW" \
  --argjson first_pid "$FIRST_PID" --argjson second_pid "$SECOND_PID" \
  --argjson first_dead "$FIRST_PROCESS_DEAD" --arg before_hash "${HASH_BEFORE:-}" --arg after_hash "${HASH_AFTER:-}" \
  --argjson violations "$VIOLATIONS" --argjson passed "$PASSED" --arg status "$GATE_STATUS" --arg reason "$REASON" \
  '{
    schema_version:"sylvode.flow.document-integrity-result.v1",
    release:$release,
    source_head:$head,
    generated_at:$generated_at,
    fixture:{workspace_id:$workspace_id,object_id:$object_id,document_id:$document_id,test_mutation:$mutation},
    database_replay:{
      snapshot_seq:$snapshot_seq,
      head_seq:$head_seq,
      tail_update_count:$tail_count,
      seq_contiguous:$seq_contiguous,
      frontier_matches_head_byte_for_byte:($db_replay.frontier_matches_head_byte_for_byte // false),
      semantic_hash:($db_replay.semantic_hash // null),
      replay:$db_replay
    },
    projection_recompute:{
      title_matches:($db_replay.projection_title_matches // false),
      state_matches:($db_replay.projection_state_matches // false),
      plain_text_matches:($db_replay.projection_plain_text_matches // false),
      document_seq_matches_head:($doc_row.projection.document_seq == $head_seq),
      document_frontier_matches_head_byte_for_byte:($doc_row.projection.document_frontier_base64 == $doc_row.head_frontier_base64)
    },
    process_restart:{
      first_pid:$first_pid,
      first_process_observed_dead:$first_dead,
      second_pid:$second_pid,
      distinct_processes:($first_pid != $second_pid),
      before_semantic_hash:(if $before_hash=="" then null else $before_hash end),
      after_semantic_hash:(if $after_hash=="" then null else $after_hash end),
      semantic_hash_equal:($before_hash!="" and $before_hash==$after_hash)
    },
    violations:$violations,
    passed:$passed,
    gates:{snapshot_tail_restart_recovery:{status:$status,reason:$reason}}
  }')"

OUT_PATH="$EVIDENCE_ROOT/document-integrity-result.json"
printf '%s\n' "$RESULT" | jq . > "$OUT_PATH.tmp"
sync "$OUT_PATH.tmp" 2>/dev/null || true
mv -f "$OUT_PATH.tmp" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2
if [[ "$PASSED" != "true" ]]; then jq -r '.violations[] | "  VIOLATION: " + .' <<<"$RESULT" >&2; fi
echo "$RESULT"
[[ "$PASSED" == "true" ]]
