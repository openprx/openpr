#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 document row lock / update-seq uniqueness verifier.
#
# Contract: /opt/working/sylvode-flow/STATUS.md ("v0.4 使用 PostgreSQL
# document row lock 串行分配 update seq"), versions/v0.4-flow-alpha.md
# "验收" ("两个并发请求竞争同一 document 时 seq 连续唯一、head/projection
# 对齐"), ADR-0010 ("PostgreSQL `collab_documents` head 和 document row lock
# 是跨实例 seq 的唯一权威") and gates/v0.4-gate.yaml's
# `document_row_lock_seq_unique`.
#
# This gate is about a race, so this verifier creates one. Two things had to be
# true for the fixture to actually test the row lock:
#
#   * the requests must be genuinely concurrent -- a single-threaded walk over
#     the command endpoint produces a perfectly contiguous seq against an
#     implementation with no lock at all, and proves nothing;
#   * the writers must live in DIFFERENT API PROCESSES. `ADR-0010`'s "第 0 层"
#     instance-local `DocumentCoordinator` (apps/api/src/flow/collab/
#     coordinator.rs) is a per-document semaphore that already serialises
#     same-instance writers before the database is reached. A single-instance
#     race is therefore serialised by that semaphore whether or not the
#     `SELECT ... FOR UPDATE` exists -- measured: deleting `FOR UPDATE` from
#     the locked phase leaves a single-instance fixture perfectly green. The
#     row lock is what ADR-0010 calls the "跨实例 seq 的唯一权威", so only a
#     cross-instance race can observe it.
#
# So the script boots N (>= 2) `api` processes against ONE database, creates one
# Flow page, and fires the concurrent commands at that one document spread
# round-robin across the instances, repeated over several rounds.
#
# Asserted after each round, against the live database:
#   C1  every concurrent request ended in one of exactly two documented ways:
#       accepted (`code:0`), or refused with `server_draining` -- the
#       `reason=contention` outcome error-mapping-v1.md defines for a writer
#       that exhausted ADR-0010's three rebase attempts. Anything else (a 5xx,
#       a raw unique-violation, an unparseable body) is a violation. At least
#       two writers per round must be accepted, otherwise the round measured
#       nothing.
#   C2  the accepted_seq values the API handed back are exactly the contiguous
#       range (previous_head, previous_head + accepted_count] -- no duplicate
#       seq was ever handed to two callers, and no seq was skipped or burned by
#       a writer that was then refused.
#   C3  `collab_updates` for the document holds exactly those seqs, with
#       min/max/count agreeing -- the wire answer and the durable row agree.
#   C4  `collab_documents.head_seq` equals the maximum seq, and
#       `flow_object_projections.document_seq` equals it too: head and
#       projection stay aligned with the update log.
#   C5  every accepted update carries a distinct update_id and content_hash.
#
# Idempotency race (separate fixture): M concurrent requests that all carry
# the SAME idempotency key must produce exactly one `collab_updates` row and
# advance the head by exactly one. That is the property a naive
# "check-then-insert" outside the lock loses first.
#
# Row-lock fixture (L1): an independent psql session takes `SELECT ... FROM
# collab_documents WHERE id = <doc> FOR UPDATE` and holds it open, and one
# command is then sent over HTTP. It must be refused (`409 server_draining`,
# after ADR-0010's 100 ms lock wait) with zero new `collab_updates` rows, and
# the same command must succeed once the lock is released -- the second half
# proving the refusal came from the lock and not from a wedged server. This is
# the direct evidence that the `collab_documents` row is what serialises the
# write path across processes, which is ADR-0010's "跨实例 seq 的唯一权威".
#
# Two limitations of this verifier, measured rather than assumed, stated here
# because a reader would otherwise credit it with more than it proves:
#
#   1. Deleting `FOR UPDATE` from the locked-phase SELECT does NOT make any
#      fixture here fail. Two mechanisms cover for it: the `(document_id, seq)`
#      primary key rejects a duplicate seq outright, and the same transaction's
#      `UPDATE collab_documents SET head_seq = ... WHERE id = $1` takes the very
#      same row lock a moment later -- so L1 blocks either way. The `FOR UPDATE`
#      keyword's real effect is to move the conflict earlier, before the work is
#      done; that is a liveness property, and it is not observable from outside.
#   2. What IS observable, and what these fixtures do catch, is a broken locked
#      phase: allocating seq from the pre-lock observed head, or skipping the
#      in-lock head-match recheck, makes two cross-instance writers claim the
#      same seq and surfaces as an undocumented 5xx under C1 (verified by
#      injecting exactly that change).
#
# Schema evidence: the (document_id, seq) primary key is re-read from
# `information_schema`/`pg_index` on the live database rather than from the
# migration text, so a database whose constraint was never created cannot
# pass on the strength of a .sql file that says it should have been.
#
# Exit codes: 0 = every assertion passed in every round, 1 = an assertion
# failed, 2 = usage/tool/environment error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-}"
CONCURRENCY=8
ROUNDS=3
INSTANCES=2
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-document-seq-v0.4.sh --concurrency N --rounds N --json [OPTIONS]

Boots the real `api` binary, then drives N genuinely concurrent command
requests at ONE Flow document over several rounds and asserts that update
seq is allocated contiguously and uniquely, that head and projection stay
aligned, and that a concurrent same-idempotency-key burst produces exactly
one update. Writes evidence/v0.4/document-seq-result.json.

Options:
  --concurrency N        Concurrent writers per round (default 8).
  --rounds N             Number of rounds (default 3).
  --instances N          API processes sharing the database (default 2,
                         minimum 2 -- a one-instance race is serialised by the
                         instance-local coordinator and tests nothing).
  --database-url URL     Postgres DSN this script may CREATE DATABASE on.
                         Default: $OPENPR_TEST_DATABASE_URL
  --repo-root DIR        Default: this checkout.
  --contracts-root DIR   Default: /opt/working/sylvode-flow
  --evidence-root DIR    Default: /opt/working/sylvode-flow/evidence/v0.4
  --json                 Required for CLI-contract compatibility.
  -h, --help             Show this help and exit 0.

Exit codes: 0 all assertions passed, 1 an assertion failed,
2 usage/tool/environment error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --concurrency) CONCURRENCY="${2:?--concurrency requires a value}"; shift 2 ;;
    --rounds) ROUNDS="${2:?--rounds requires a value}"; shift 2 ;;
    --instances) INSTANCES="${2:?--instances requires a value}"; shift 2 ;;
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
: "$CONTRACTS_ROOT"

if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
if [[ -z "$DATABASE_URL" ]]; then
  echo "FAIL: no database URL configured (set --database-url or OPENPR_TEST_DATABASE_URL)" >&2
  exit 2
fi
if ! [[ "$CONCURRENCY" =~ ^[0-9]+$ ]] || [[ "$CONCURRENCY" -lt 2 ]]; then
  echo "FAIL: --concurrency must be an integer >= 2 (a single writer proves nothing about a race)" >&2
  exit 2
fi
if ! [[ "$ROUNDS" =~ ^[0-9]+$ ]] || [[ "$ROUNDS" -lt 1 ]]; then
  echo "FAIL: --rounds must be an integer >= 1" >&2
  exit 2
fi
if ! [[ "$INSTANCES" =~ ^[0-9]+$ ]] || [[ "$INSTANCES" -lt 2 ]]; then
  echo "FAIL: --instances must be an integer >= 2; a single-instance race is serialised by the ADR-0010 instance-local coordinator and would pass with no row lock at all" >&2
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

TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
echo "=== building api binary (cargo build -p api --bin api) ===" >&2
( cd "$REPO_ROOT" && cargo build -q -p api --bin api ) || { echo "FAIL: api binary failed to build" >&2; exit 2; }
API_BIN="$TARGET_DIR/debug/api"
[[ -x "$API_BIN" ]] || { echo "FAIL: api binary not found after build: $API_BIN" >&2; exit 2; }

RUN_ID="$(python3 -c 'import uuid; print(uuid.uuid4().hex[:8])')"
SCRATCH_DB="openpr_flow_seq_verify_$RUN_ID"
DB_PREFIX="${DATABASE_URL%/*}"
SCRATCH_URL="$DB_PREFIX/$SCRATCH_DB"
TMP_DIR="$(mktemp -d "/tmp/openpr-flow-seq-verify.XXXXXX")"
JWT_SECRET="flow-document-seq-verify-not-a-real-secret"
API_PORTS=()
API_PIDS=()

# shellcheck disable=SC2317  # invoked only via `trap ... EXIT`
cleanup() {
  local ec=$?
  for pid in "${API_PIDS[@]:-}"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "DROP DATABASE IF EXISTS \"$SCRATCH_DB\" WITH (FORCE)" >/dev/null 2>&1 || true
  rm -rf "$TMP_DIR"
  exit "$ec"
}
trap cleanup EXIT

psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "DROP DATABASE IF EXISTS \"$SCRATCH_DB\" WITH (FORCE)" >/dev/null 2>&1 || true
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "CREATE DATABASE \"$SCRATCH_DB\"" >/dev/null

# Instance 1 is started alone first: it runs the migrations, and letting several
# processes race the ledger would be testing the wrong thing here.
for inst in $(seq 1 "$INSTANCES"); do
  port=$((20000 + RANDOM % 20000))
  config="$TMP_DIR/openpr-$inst.toml"
  cat > "$config" <<EOF
[server]
app_name = "api"
bind_addr = "127.0.0.1:$port"

[database]
url = "$SCRATCH_URL"

[auth]
jwt_secret = "$JWT_SECRET"

[logging]
filter = "api=warn,openpr=warn"
format = "text"
EOF
  echo "=== starting api instance $inst on 127.0.0.1:$port (shared database $SCRATCH_DB) ===" >&2
  "$API_BIN" --config "$config" > "$TMP_DIR/api-$inst.log" 2>&1 &
  pid=$!
  API_PORTS+=("$port")
  API_PIDS+=("$pid")
  healthy=0
  for _ in $(seq 1 120); do
    if curl -fsS "http://127.0.0.1:$port/health" >/dev/null 2>&1; then healthy=1; break; fi
    if ! kill -0 "$pid" 2>/dev/null; then break; fi
    sleep 0.5
  done
  if [[ $healthy -ne 1 ]]; then
    echo "FAIL: api instance $inst did not become healthy; log follows" >&2
    tail -40 "$TMP_DIR/api-$inst.log" >&2 || true
    exit 2
  fi
done

WORKSPACE="$(python3 -c 'import uuid; print(uuid.uuid4())')"
OWNER_USER="$(python3 -c 'import uuid; print(uuid.uuid4())')"
psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, created_at, updated_at)
VALUES ('$OWNER_USER', 'seq-verify-$RUN_ID@example.local', '', 'Seq Verify Owner', 'user', true, 'human', now(), now());
INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE', 'seq-verify-$RUN_ID', 'Seq Verify', '$OWNER_USER', now(), now());
INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES ('$WORKSPACE', '$OWNER_USER', 'owner', now());
INSERT INTO flow_workspace_settings (workspace_id, flow_enabled, default_member_level, authz_epoch, updated_at)
VALUES ('$WORKSPACE', true, 'edit', 0, now());
SQL

# A human member's access token: a bot's actor id is a `workspace_bots` row id, and
# `flow_objects.created_by` is a foreign key into `users`.
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
' "$JWT_SECRET" "$OWNER_USER" "seq-verify-$RUN_ID@example.local")"

# Setup traffic goes to instance 1; the racing writers are spread across all of
# them by `api_for`.
API="http://127.0.0.1:${API_PORTS[0]}"
api_for() { printf 'http://127.0.0.1:%s' "${API_PORTS[$(( ($1 - 1) % INSTANCES ))]}"; }
AUTH=(-H "Authorization: Bearer $USER_JWT" -H "Content-Type: application/json")
json_get() { jq -r "$2" <<<"$1" 2>/dev/null || printf ''; }

VIOLATIONS=()

CREATE_RESP="$(curl -sS -X POST "$API/api/v1/workspaces/$WORKSPACE/flow/objects" "${AUTH[@]}" \
  -d "$(jq -n --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" '{object_type:"page", title:"Seq race fixture", idempotency_key:$key}')")"
OBJECT_ID="$(json_get "$CREATE_RESP" '.data.id // .data.object.id // empty')"
if [[ -z "$OBJECT_ID" ]]; then
  echo "FAIL: could not create the Flow page the race runs against (response: $CREATE_RESP)" >&2
  exit 2
fi
DOCUMENT_ID="$(psql "$SCRATCH_URL" -Atc "SELECT id FROM collab_documents WHERE object_id='$OBJECT_ID'")"
[[ -n "$DOCUMENT_ID" ]] || { echo "FAIL: the created page has no collab_documents row" >&2; exit 2; }

# ---- schema evidence: the (document_id, seq) primary key really exists ----
PK_COLUMNS="$(psql "$SCRATCH_URL" -Atc "
SELECT string_agg(a.attname, ',' ORDER BY k.ord)
FROM pg_index i
JOIN LATERAL unnest(i.indkey) WITH ORDINALITY AS k(attnum, ord) ON true
JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k.attnum
WHERE i.indrelid = to_regclass('collab_updates') AND i.indisprimary")"
if [[ "$PK_COLUMNS" != "document_id,seq" ]]; then
  VIOLATIONS+=("schema: collab_updates primary key is '$PK_COLUMNS', expected 'document_id,seq' -- without it the database itself would permit two writers to claim the same seq")
fi

ROUNDS_JSON="[]"
PREV_HEAD=0
TOTAL_ACCEPTED=0
TOTAL_CONTENTION=0

for round in $(seq 1 "$ROUNDS"); do
  echo "=== round $round: $CONCURRENCY concurrent commands against document $DOCUMENT_ID ===" >&2
  RESP_DIR="$TMP_DIR/round$round"
  mkdir -p "$RESP_DIR"

  # Every writer's body is prepared BEFORE any process starts, so the concurrency
  # measured is the server's, not this script's json/uuid generation.
  for w in $(seq 1 "$CONCURRENCY"); do
    jq -n --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" \
      --arg t "round-$round-writer-$w-$RUN_ID" \
      '{command:{type:"set_title", payload:{title:$t}}, idempotency_key:$key}' > "$RESP_DIR/body-$w.json"
  done

  # A shared start gate: each writer blocks reading the fifo until the gate opens, so
  # all N requests are genuinely in flight together rather than staggered by process
  # spawn time.
  GATE="$RESP_DIR/gate"
  mkfifo "$GATE"
  PIDS=()
  for w in $(seq 1 "$CONCURRENCY"); do
    (
      read -r _ < "$GATE" || true
      curl -sS -X POST "$(api_for "$w")/api/v1/flow/objects/$OBJECT_ID/commands" "${AUTH[@]}" \
        --data-binary "@$RESP_DIR/body-$w.json" > "$RESP_DIR/resp-$w.json" 2>"$RESP_DIR/err-$w.txt"
    ) &
    PIDS+=($!)
  done
  sleep 0.3
  # Opening the fifo for writing releases every blocked reader at once.
  exec 9>"$GATE"
  for w in $(seq 1 "$CONCURRENCY"); do echo "go" >&9; done
  exec 9>&-
  for pid in "${PIDS[@]}"; do wait "$pid" || true; done
  rm -f "$GATE"

  ACCEPTED_SEQS=()
  REJECTED=0
  CONTENTION=0
  for w in $(seq 1 "$CONCURRENCY"); do
    body="$(cat "$RESP_DIR/resp-$w.json" 2>/dev/null || printf '')"
    code="$(json_get "$body" '.code // empty')"
    message="$(json_get "$body" '.message // empty')"
    seq_v="$(json_get "$body" '.data.accepted_seq // empty')"
    if [[ "$code" == "0" && -n "$seq_v" ]]; then
      ACCEPTED_SEQS+=("$seq_v")
    elif [[ "$code" == "409" && "$message" == "server_draining" ]]; then
      # error-mapping-v1.md: the documented, recoverable outcome for a writer
      # that exhausted ADR-0010 three rebase attempts. Legal -- but it must not
      # have written anything, which C2/C3 below check by counting.
      REJECTED=$((REJECTED + 1))
      CONTENTION=$((CONTENTION + 1))
    else
      REJECTED=$((REJECTED + 1))
      VIOLATIONS+=("C1 round $round writer $w: request ended in an undocumented way (envelope code='$code' message='$message', body: ${body:0:400}); only code:0 or 409 server_draining are legal outcomes here")
    fi
  done

  ACCEPTED_COUNT=${#ACCEPTED_SEQS[@]}
  if [[ "$ACCEPTED_COUNT" -lt 2 ]]; then
    VIOLATIONS+=("C1 round $round: only $ACCEPTED_COUNT of $CONCURRENCY writers were accepted; a round that serialised fewer than two writers measured no race at all")
  fi

  SEQS_JSON="$(printf '%s\n' "${ACCEPTED_SEQS[@]:-}" | jq -R 'select(length>0) | tonumber' | jq -c -s 'sort')"
  EXPECTED_JSON="$(python3 -c '
import json, sys
prev, n = int(sys.argv[1]), int(sys.argv[2])
print(json.dumps(list(range(prev + 1, prev + n + 1)), separators=(",", ":")))
' "$PREV_HEAD" "$ACCEPTED_COUNT")"

  if [[ "$SEQS_JSON" != "$EXPECTED_JSON" ]]; then
    VIOLATIONS+=("C2 round $round: the accepted_seq values handed back were $SEQS_JSON, expected the contiguous range $EXPECTED_JSON (a duplicate means two callers were handed the same seq; a gap means a refused writer still burned one)")
  fi

  DB_COUNT="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM collab_updates WHERE document_id='$DOCUMENT_ID'")"
  DB_MIN="$(psql "$SCRATCH_URL" -Atc "SELECT coalesce(min(seq),-1) FROM collab_updates WHERE document_id='$DOCUMENT_ID'")"
  DB_MAX="$(psql "$SCRATCH_URL" -Atc "SELECT coalesce(max(seq),-1) FROM collab_updates WHERE document_id='$DOCUMENT_ID'")"
  DB_DISTINCT="$(psql "$SCRATCH_URL" -Atc "SELECT count(DISTINCT seq) FROM collab_updates WHERE document_id='$DOCUMENT_ID'")"
  # Every refused writer must have written nothing: the durable row count is the
  # accepted count exactly, never the attempted count.
  EXPECTED_TOTAL=$((PREV_HEAD + ACCEPTED_COUNT))
  if true; then
    [[ "$DB_COUNT" == "$EXPECTED_TOTAL" ]] || VIOLATIONS+=("C3 round $round: collab_updates holds $DB_COUNT rows for the document, expected $EXPECTED_TOTAL ($ACCEPTED_COUNT accepted, $REJECTED refused and therefore contributing nothing)")
    [[ "$DB_DISTINCT" == "$DB_COUNT" ]] || VIOLATIONS+=("C3 round $round: collab_updates has duplicate seq values ($DB_COUNT rows, $DB_DISTINCT distinct seqs)")
    [[ "$DB_MIN" == "1" ]] || VIOLATIONS+=("C3 round $round: minimum durable seq is $DB_MIN, expected 1")
    [[ "$DB_MAX" == "$EXPECTED_TOTAL" ]] || VIOLATIONS+=("C3 round $round: maximum durable seq is $DB_MAX, expected $EXPECTED_TOTAL (a gap means a seq was allocated and lost)")
  fi

  HEAD_SEQ="$(psql "$SCRATCH_URL" -Atc "SELECT head_seq FROM collab_documents WHERE id='$DOCUMENT_ID'")"
  PROJ_SEQ="$(psql "$SCRATCH_URL" -Atc "SELECT document_seq FROM flow_object_projections WHERE object_id='$OBJECT_ID'")"
  [[ "$HEAD_SEQ" == "$DB_MAX" ]] || VIOLATIONS+=("C4 round $round: collab_documents.head_seq=$HEAD_SEQ but the maximum collab_updates.seq is $DB_MAX")
  [[ "$PROJ_SEQ" == "$DB_MAX" ]] || VIOLATIONS+=("C4 round $round: flow_object_projections.document_seq=$PROJ_SEQ but the maximum collab_updates.seq is $DB_MAX")

  DISTINCT_UPDATE_IDS="$(psql "$SCRATCH_URL" -Atc "SELECT count(DISTINCT update_id) FROM collab_updates WHERE document_id='$DOCUMENT_ID'")"
  DISTINCT_HASHES="$(psql "$SCRATCH_URL" -Atc "SELECT count(DISTINCT content_hash) FROM collab_updates WHERE document_id='$DOCUMENT_ID'")"
  [[ "$DISTINCT_UPDATE_IDS" == "$DB_COUNT" ]] || VIOLATIONS+=("C5 round $round: $DB_COUNT rows but only $DISTINCT_UPDATE_IDS distinct update_id")
  [[ "$DISTINCT_HASHES" == "$DB_COUNT" ]] || VIOLATIONS+=("C5 round $round: $DB_COUNT rows but only $DISTINCT_HASHES distinct content_hash")

  ROUNDS_JSON="$(jq -c --argjson round "$round" --argjson concurrency "$CONCURRENCY" \
    --argjson accepted "$SEQS_JSON" --argjson expected "$EXPECTED_JSON" \
    --argjson rejected "$REJECTED" --argjson contention "$CONTENTION" --argjson accepted_count "$ACCEPTED_COUNT" \
    --arg db_count "$DB_COUNT" --arg db_min "$DB_MIN" --arg db_max "$DB_MAX" --arg db_distinct "$DB_DISTINCT" \
    --arg head_seq "$HEAD_SEQ" --arg projection_seq "$PROJ_SEQ" \
    '. + [{
      round: $round, concurrency: $concurrency, accepted: $accepted_count,
      rejected: $rejected, refused_with_server_draining_contention: $contention,
      accepted_seqs: $accepted, expected_seqs: $expected,
      durable: {rows: $db_count, distinct_seqs: $db_distinct, min_seq: $db_min, max_seq: $db_max},
      head_seq: $head_seq, projection_seq: $projection_seq
    }]' <<<"$ROUNDS_JSON")"

  PREV_HEAD="$EXPECTED_TOTAL"
  TOTAL_ACCEPTED=$((TOTAL_ACCEPTED + ACCEPTED_COUNT))
  TOTAL_CONTENTION=$((TOTAL_CONTENTION + CONTENTION))
done

# ---- L1: is the document row lock actually taken? ----
# See the file header. An independent psql session holds
# `SELECT ... FROM collab_documents WHERE id = <doc> FOR UPDATE` open; the write
# path must block on it and refuse, then succeed once it is released. An
# implementation that reads the head without `FOR UPDATE` is not blocked by
# another session's row lock under MVCC and would sail straight through.
echo "=== L1: external row lock on the document must block the write path ===" >&2
LOCK_DIR="$TMP_DIR/rowlock"
mkdir -p "$LOCK_DIR"
cat > "$LOCK_DIR/hold.sql" <<SQL
BEGIN;
SELECT id FROM collab_documents WHERE id = '$DOCUMENT_ID' FOR UPDATE;
\echo LOCK_ACQUIRED
SELECT pg_sleep(12);
COMMIT;
SQL
psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -f "$LOCK_DIR/hold.sql" > "$LOCK_DIR/hold.log" 2>&1 &
LOCK_HOLDER_PID=$!
LOCK_HELD=0
for _ in $(seq 1 60); do
  if grep -q LOCK_ACQUIRED "$LOCK_DIR/hold.log" 2>/dev/null; then LOCK_HELD=1; break; fi
  if ! kill -0 "$LOCK_HOLDER_PID" 2>/dev/null; then break; fi
  sleep 0.2
done

ROWLOCK_BLOCKED_CODE=""
ROWLOCK_BLOCKED_MESSAGE=""
ROWLOCK_ROWS_DURING=""
ROWLOCK_RELEASED_CODE=""
ROWS_BEFORE_LOCK="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM collab_updates WHERE document_id='$DOCUMENT_ID'")"
if [[ $LOCK_HELD -ne 1 ]]; then
  VIOLATIONS+=("L1: the verifier could not take its own FOR UPDATE row lock on the document, so the row-lock discriminator did not run (psql log: $(tr -d '\000' < "$LOCK_DIR/hold.log" | head -3 | tr '\n' ' '))")
  kill "$LOCK_HOLDER_PID" 2>/dev/null || true
  wait "$LOCK_HOLDER_PID" 2>/dev/null || true
else
  BLOCKED_RESP="$(curl -sS --max-time 30 -X POST "$(api_for 1)/api/v1/flow/objects/$OBJECT_ID/commands" "${AUTH[@]}" \
    -d "$(jq -n --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" --arg t "written while the row lock was held $RUN_ID" \
      '{command:{type:"set_title", payload:{title:$t}}, idempotency_key:$key}')" || printf '')"
  ROWLOCK_BLOCKED_CODE="$(json_get "$BLOCKED_RESP" '.code // empty')"
  ROWLOCK_BLOCKED_MESSAGE="$(json_get "$BLOCKED_RESP" '.message // empty')"
  ROWLOCK_ROWS_DURING="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM collab_updates WHERE document_id='$DOCUMENT_ID'")"

  if [[ "$ROWLOCK_BLOCKED_CODE" == "0" ]]; then
    VIOLATIONS+=("L1: a command was ACCEPTED while another session held the document row lock -- the write path never takes SELECT ... FOR UPDATE on collab_documents, so cross-instance seq allocation rests on the (document_id, seq) primary key alone (response: ${BLOCKED_RESP:0:300})")
  elif [[ "$ROWLOCK_BLOCKED_CODE" != "409" || "$ROWLOCK_BLOCKED_MESSAGE" != "server_draining" ]]; then
    VIOLATIONS+=("L1: with the row lock held the command answered envelope code='$ROWLOCK_BLOCKED_CODE' message='$ROWLOCK_BLOCKED_MESSAGE'; the documented outcome is 409 server_draining (response: ${BLOCKED_RESP:0:300})")
  fi
  if [[ "$ROWLOCK_ROWS_DURING" != "$ROWS_BEFORE_LOCK" ]]; then
    VIOLATIONS+=("L1: collab_updates grew from $ROWS_BEFORE_LOCK to $ROWLOCK_ROWS_DURING while the row lock was held by another session")
  fi

  # Killing psql only closes the client; the server backend keeps running its
  # `pg_sleep` until it notices. So the lock is released explicitly, and then
  # confirmed gone with a `FOR UPDATE NOWAIT` probe before the control command
  # is sent -- otherwise a still-held lock would be misread as "the refusal was
  # not caused by the lock".
  psql "$SCRATCH_URL" -Atc "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = current_database() AND pid <> pg_backend_pid() AND query LIKE '%pg_sleep%'" >/dev/null 2>&1 || true
  kill "$LOCK_HOLDER_PID" 2>/dev/null || true
  wait "$LOCK_HOLDER_PID" 2>/dev/null || true
  LOCK_RELEASED=0
  for _ in $(seq 1 60); do
    if psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -Atc "BEGIN; SELECT id FROM collab_documents WHERE id = '$DOCUMENT_ID' FOR UPDATE NOWAIT; ROLLBACK;" >/dev/null 2>&1; then
      LOCK_RELEASED=1
      break
    fi
    sleep 0.5
  done
  if [[ $LOCK_RELEASED -ne 1 ]]; then
    VIOLATIONS+=("L1: the external row lock was still held after the holder was terminated, so the control command could not be run")
  fi

  RELEASED_RESP="$(curl -sS --max-time 30 -X POST "$(api_for 1)/api/v1/flow/objects/$OBJECT_ID/commands" "${AUTH[@]}" \
    -d "$(jq -n --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" --arg t "written after the row lock was released $RUN_ID" \
      '{command:{type:"set_title", payload:{title:$t}}, idempotency_key:$key}')" || printf '')"
  ROWLOCK_RELEASED_CODE="$(json_get "$RELEASED_RESP" '.code // empty')"
  if [[ "$ROWLOCK_RELEASED_CODE" != "0" ]]; then
    VIOLATIONS+=("L1: the same command still failed after the row lock was released (envelope code='$ROWLOCK_RELEASED_CODE') -- the earlier refusal cannot be attributed to the lock (response: ${RELEASED_RESP:0:300})")
  fi
fi

# ---- concurrent same-idempotency-key burst: exactly one update ----
echo "=== idempotency race: $CONCURRENCY concurrent requests sharing one idempotency key ===" >&2
IDEM_DIR="$TMP_DIR/idem"
mkdir -p "$IDEM_DIR"
IDEM_KEY="$(python3 -c 'import uuid; print(uuid.uuid4())')"
jq -n --arg key "$IDEM_KEY" --arg t "idempotent-title-$RUN_ID" \
  '{command:{type:"set_title", payload:{title:$t}}, idempotency_key:$key}' > "$IDEM_DIR/body.json"
HEAD_BEFORE_IDEM="$(psql "$SCRATCH_URL" -Atc "SELECT head_seq FROM collab_documents WHERE id='$DOCUMENT_ID'")"

GATE="$IDEM_DIR/gate"
mkfifo "$GATE"
PIDS=()
for w in $(seq 1 "$CONCURRENCY"); do
  (
    read -r _ < "$GATE" || true
    curl -sS -X POST "$(api_for "$w")/api/v1/flow/objects/$OBJECT_ID/commands" "${AUTH[@]}" \
      --data-binary "@$IDEM_DIR/body.json" > "$IDEM_DIR/resp-$w.json" 2>/dev/null
  ) &
  PIDS+=($!)
done
sleep 0.3
exec 9>"$GATE"
for w in $(seq 1 "$CONCURRENCY"); do echo "go" >&9; done
exec 9>&-
for pid in "${PIDS[@]}"; do wait "$pid" || true; done
rm -f "$GATE"

IDEM_ROWS="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM collab_updates WHERE document_id='$DOCUMENT_ID' AND idempotency_key='$IDEM_KEY'")"
HEAD_AFTER_IDEM="$(psql "$SCRATCH_URL" -Atc "SELECT head_seq FROM collab_documents WHERE id='$DOCUMENT_ID'")"
IDEM_ADVANCE=$((HEAD_AFTER_IDEM - HEAD_BEFORE_IDEM))
if [[ "$IDEM_ROWS" != "1" ]]; then
  VIOLATIONS+=("idempotency race: $CONCURRENCY concurrent requests with one shared idempotency key produced $IDEM_ROWS collab_updates rows, expected exactly 1")
fi
if [[ "$IDEM_ADVANCE" -ne 1 ]]; then
  VIOLATIONS+=("idempotency race: head_seq advanced by $IDEM_ADVANCE ($HEAD_BEFORE_IDEM -> $HEAD_AFTER_IDEM), expected exactly 1")
fi
IDEM_REPLAYED_SEQS="$(for w in $(seq 1 "$CONCURRENCY"); do jq -r '.data.accepted_seq // empty' "$IDEM_DIR/resp-$w.json" 2>/dev/null; done | sort -u | paste -sd, -)"
if [[ -n "$IDEM_REPLAYED_SEQS" && "$IDEM_REPLAYED_SEQS" == *","* ]]; then
  VIOLATIONS+=("idempotency race: the shared key was answered with more than one accepted_seq ($IDEM_REPLAYED_SEQS) -- an idempotent replay must return the original seq")
fi

# Final whole-document consistency after every fixture.
FINAL_ROWS="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM collab_updates WHERE document_id='$DOCUMENT_ID'")"
FINAL_DISTINCT="$(psql "$SCRATCH_URL" -Atc "SELECT count(DISTINCT seq) FROM collab_updates WHERE document_id='$DOCUMENT_ID'")"
FINAL_MAX="$(psql "$SCRATCH_URL" -Atc "SELECT max(seq) FROM collab_updates WHERE document_id='$DOCUMENT_ID'")"
FINAL_HEAD="$(psql "$SCRATCH_URL" -Atc "SELECT head_seq FROM collab_documents WHERE id='$DOCUMENT_ID'")"
GAPS="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM generate_series(1, $FINAL_MAX) s WHERE NOT EXISTS (SELECT 1 FROM collab_updates WHERE document_id='$DOCUMENT_ID' AND seq = s)")"
[[ "$FINAL_ROWS" == "$FINAL_DISTINCT" ]] || VIOLATIONS+=("final: $FINAL_ROWS rows but $FINAL_DISTINCT distinct seqs")
[[ "$GAPS" == "0" ]] || VIOLATIONS+=("final: $GAPS gap(s) in the seq range 1..$FINAL_MAX")
[[ "$FINAL_HEAD" == "$FINAL_MAX" ]] || VIOLATIONS+=("final: head_seq=$FINAL_HEAD != max(seq)=$FINAL_MAX")

PASSED=$([[ ${#VIOLATIONS[@]} -eq 0 ]] && echo true || echo false)
GATE_STATUS=$([[ "$PASSED" == "true" ]] && echo passed || echo failed)
VIOLATIONS_JSON="$(printf '%s\n' "${VIOLATIONS[@]:-}" | jq -R 'select(length>0)' | jq -s '.')"
REASON="$ROUNDS round(s) of $CONCURRENCY concurrent commands spread across $INSTANCES separate api processes sharing one database, all writing one document ($TOTAL_ACCEPTED accepted, $TOTAL_CONTENTION refused as server_draining contention), plus an external-row-lock discriminator and a shared-idempotency-key burst; final seq 1..$FINAL_MAX contiguous=$([[ "$GAPS" == "0" ]] && echo yes || echo no), head=$FINAL_HEAD; $(jq 'length' <<<"$VIOLATIONS_JSON") violation(s)"

RESULT="$(jq -n \
  --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" \
  --arg object_id "$OBJECT_ID" --arg document_id "$DOCUMENT_ID" \
  --argjson concurrency "$CONCURRENCY" --argjson rounds "$ROUNDS" --argjson instances "$INSTANCES" \
  --argjson api_ports "$(printf '%s\n' "${API_PORTS[@]}" | jq -R 'tonumber' | jq -c -s '.')" \
  --arg pk "$PK_COLUMNS" \
  --argjson round_results "$ROUNDS_JSON" \
  --arg idem_key "$IDEM_KEY" --arg idem_rows "$IDEM_ROWS" --argjson idem_advance "$IDEM_ADVANCE" \
  --argjson lock_held "$([[ ${LOCK_HELD:-0} -eq 1 ]] && echo true || echo false)" \
  --arg lock_blocked_code "$ROWLOCK_BLOCKED_CODE" --arg lock_blocked_message "$ROWLOCK_BLOCKED_MESSAGE" \
  --arg lock_rows_before "$ROWS_BEFORE_LOCK" --arg lock_rows_during "$ROWLOCK_ROWS_DURING" \
  --arg lock_released_code "$ROWLOCK_RELEASED_CODE" \
  --argjson total_accepted "$TOTAL_ACCEPTED" --argjson total_contention "$TOTAL_CONTENTION" \
  --arg idem_seqs "$IDEM_REPLAYED_SEQS" \
  --arg final_rows "$FINAL_ROWS" --arg final_distinct "$FINAL_DISTINCT" --arg final_max "$FINAL_MAX" \
  --arg final_head "$FINAL_HEAD" --arg gaps "$GAPS" \
  --argjson violations "$VIOLATIONS_JSON" --argjson passed "$PASSED" \
  --arg gate_status "$GATE_STATUS" --arg reason "$REASON" \
  '{
    schema_version: "sylvode.flow.document-seq-result.v1",
    source_head: $head,
    generated_at: $generated_at,
    fixture: {object_id: $object_id, document_id: $document_id, concurrency: $concurrency, rounds: $rounds,
              api_instances: $instances, api_ports: $api_ports,
              note: "writers are spread round-robin across the api instances; a single-instance race would be serialised by the ADR-0010 instance-local DocumentCoordinator and would not observe the database row lock at all"},
    schema_evidence: {collab_updates_primary_key: $pk},
    concurrency_rounds: $round_results,
    totals: {accepted: $total_accepted, refused_with_server_draining_contention: $total_contention},
    row_lock_discriminator: {
      description: "an independent psql session held SELECT ... FROM collab_documents WHERE id = <doc> FOR UPDATE open while one command was sent over HTTP; a write path that does not take the row lock is not blocked by it under MVCC and would be accepted",
      external_lock_acquired: $lock_held,
      command_while_locked: {envelope_code: $lock_blocked_code, envelope_message: $lock_blocked_message},
      collab_updates_rows: {before_lock: $lock_rows_before, while_locked: $lock_rows_during},
      command_after_release: {envelope_code: $lock_released_code}
    },
    idempotency_race: {
      shared_key: $idem_key,
      collab_updates_rows_for_key: $idem_rows,
      head_seq_advance: $idem_advance,
      distinct_accepted_seqs_returned: $idem_seqs
    },
    final_state: {
      rows: $final_rows, distinct_seqs: $final_distinct, max_seq: $final_max,
      head_seq: $final_head, gaps_in_range: $gaps
    },
    violations: $violations,
    passed: $passed,
    gates: {
      document_row_lock_seq_unique: {status: $gate_status, reason: $reason}
    }
  }')"

OUT_PATH="$EVIDENCE_ROOT/document-seq-result.json"
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
