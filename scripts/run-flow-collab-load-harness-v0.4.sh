#!/usr/bin/env bash
set -euo pipefail

# Qualified runner for the v0.4 release load harness. The Rust harness is a
# measurement instrument, not an environment classifier: this wrapper refuses
# to start it on a known shared or currently contended PostgreSQL instance.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
EVIDENCE_OUT=""
DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-}"
DEDICATED_PG_CONTAINER="${OPENPR_FLOW_DEDICATED_PG_CONTAINER:-}"
PG_LOG_CONTAINER="${OPENPR_FLOW_PG_LOG_CONTAINER:-}"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/run-flow-collab-load-harness-v0.4.sh --evidence-out PATH --json [OPTIONS]

Runs flow_collab_load_harness in release mode only after proving that the
declared PostgreSQL log container is an explicitly dedicated, reachable and
currently uncontended instance. An unsatisfied environment writes a durable
environment result, exits 1, and does not start the harness.

Options:
  --evidence-out PATH             Required load-harness evidence path.
  --repo-root DIR                 Repository to build. Default: this checkout.
  --database-url URL              Defaults to OPENPR_TEST_DATABASE_URL.
  --dedicated-pg-container NAME   Defaults to OPENPR_FLOW_DEDICATED_PG_CONTAINER.
  --pg-log-container NAME         Defaults to OPENPR_FLOW_PG_LOG_CONTAINER.
  --json                          Required.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --evidence-out) EVIDENCE_OUT="${2:?--evidence-out requires PATH}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires DIR}"; shift 2 ;;
    --database-url) DATABASE_URL="${2:?--database-url requires URL}"; shift 2 ;;
    --dedicated-pg-container) DEDICATED_PG_CONTAINER="${2:?--dedicated-pg-container requires NAME}"; shift 2 ;;
    --pg-log-container) PG_LOG_CONTAINER="${2:?--pg-log-container requires NAME}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ $JSON_MODE -eq 1 ]] || { echo "FAIL: --json is required" >&2; exit 2; }
[[ -n "$EVIDENCE_OUT" ]] || { echo "FAIL: --evidence-out is required" >&2; exit 2; }
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi
for tool in cargo docker git jq psql; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required command: $tool" >&2; exit 2; }
done

mkdir -p "$(dirname "$EVIDENCE_OUT")"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

write_environment_failure() {
  local reason_code="$1" detail="$2" active_clients="${3:-null}" tmp="$EVIDENCE_OUT.tmp"
  jq -n \
    --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" \
    --arg reason_code "$reason_code" --arg detail "$detail" \
    --arg declared "$DEDICATED_PG_CONTAINER" --arg log_container "$PG_LOG_CONTAINER" \
    --argjson active_clients "$active_clients" \
    '{
      schema_version:"sylvode.flow.collab-load-harness-environment.v1",
      source_head:$head,
      generated_at:$generated_at,
      environment_gate:{
        status:"not_satisfied",
        reason_code:$reason_code,
        detail:$detail,
        declared_dedicated_pg_container:(if $declared=="" then null else $declared end),
        pg_log_container:(if $log_container=="" then null else $log_container end),
        active_other_clients:$active_clients,
        known_shared_instances_rejected:["flow-test-pg"]
      },
      execution:{status:"not_run_environment_not_satisfied",harness_started:false},
      violations:[$detail],
      passed:false
    }' > "$tmp"
  mv -f "$tmp" "$EVIDENCE_OUT"
  jq . "$EVIDENCE_OUT"
  echo "ENVIRONMENT NOT SATISFIED [$reason_code]: $detail" >&2
  exit 1
}

[[ -n "$DATABASE_URL" ]] || write_environment_failure \
  "database_url_missing" "OPENPR_TEST_DATABASE_URL/--database-url is required; the harness was not started"
[[ -n "$DEDICATED_PG_CONTAINER" ]] || write_environment_failure \
  "dedicated_container_not_declared" "an explicit dedicated PostgreSQL container is required; the harness was not started"
[[ -n "$PG_LOG_CONTAINER" ]] || write_environment_failure \
  "pg_log_container_not_declared" "the PostgreSQL server-log container must be declared; the harness was not started"
[[ "$DEDICATED_PG_CONTAINER" == "$PG_LOG_CONTAINER" ]] || write_environment_failure \
  "container_declaration_mismatch" "declared dedicated container '$DEDICATED_PG_CONTAINER' does not match log container '$PG_LOG_CONTAINER'"

case "${DEDICATED_PG_CONTAINER,,}" in
  flow-test-pg|*shared*)
    write_environment_failure "known_shared_postgresql_instance" \
      "PostgreSQL container '$DEDICATED_PG_CONTAINER' is a shared instance; load distributions require a dedicated instance"
    ;;
esac

RUNNING="$(docker inspect -f '{{.State.Running}}' "$DEDICATED_PG_CONTAINER" 2>/dev/null || true)"
[[ "$RUNNING" == "true" ]] || write_environment_failure \
  "dedicated_container_unreachable" "declared dedicated PostgreSQL container '$DEDICATED_PG_CONTAINER' is not running or cannot be inspected"

psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atc 'SELECT 1' >/dev/null 2>&1 || write_environment_failure \
  "database_unreachable" "the declared dedicated PostgreSQL database is unreachable; the harness was not started"

ACTIVE_OTHER_CLIENTS="$(psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atc \
  "SELECT count(*) FROM pg_stat_activity WHERE pid <> pg_backend_pid() AND backend_type = 'client backend' AND state <> 'idle'" 2>/dev/null || true)"
[[ "$ACTIVE_OTHER_CLIENTS" =~ ^[0-9]+$ ]] || write_environment_failure \
  "database_activity_probe_failed" "could not determine whether the dedicated PostgreSQL instance is uncontended"
[[ "$ACTIVE_OTHER_CLIENTS" -eq 0 ]] || write_environment_failure \
  "dedicated_instance_contended" \
  "declared dedicated PostgreSQL instance has $ACTIVE_OTHER_CLIENTS other active client(s); the harness was not started" \
  "$ACTIVE_OTHER_CLIENTS"

TMP_DIR="$(mktemp -d /opt/worker/.cache/flow-load-runner.XXXXXX)"
cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

set +e
(cd "$REPO_ROOT" && \
  OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
  OPENPR_FLOW_DEDICATED_PG_CONTAINER="$DEDICATED_PG_CONTAINER" \
  OPENPR_FLOW_PG_LOG_CONTAINER="$PG_LOG_CONTAINER" \
  OPENPR_FLOW_LOAD_HARNESS_OUT="$EVIDENCE_OUT" \
  cargo test --release -p api --test flow_collab_load_harness \
    ten_client_load_harness_round_trip_p95_and_lock_hold_p95 -- --exact --nocapture) \
  >"$TMP_DIR/harness.log" 2>&1
HARNESS_EXIT=$?
set -e

if [[ ! -f "$EVIDENCE_OUT" ]] || ! jq -e '.schema_version == "sylvode.flow.collab-load-harness.v1"' "$EVIDENCE_OUT" >/dev/null 2>&1; then
  echo "FAIL: load harness exit=$HARNESS_EXIT did not produce a valid load-harness artifact" >&2
  tail -80 "$TMP_DIR/harness.log" >&2
  exit 2
fi

jq \
  --arg declared "$DEDICATED_PG_CONTAINER" \
  --argjson active_clients "$ACTIVE_OTHER_CLIENTS" \
  '.environment += {
    qualification_status:"satisfied",
    declared_dedicated_pg_container:$declared,
    active_other_clients_at_preflight:$active_clients,
    shared_database_measurements_accepted:false
  }' "$EVIDENCE_OUT" > "$EVIDENCE_OUT.tmp"
mv -f "$EVIDENCE_OUT.tmp" "$EVIDENCE_OUT"
jq . "$EVIDENCE_OUT"

if [[ $HARNESS_EXIT -ne 0 ]] || [[ "$(jq -r '.passed' "$EVIDENCE_OUT")" != "true" ]]; then
  echo "FAIL: qualified load harness ran but its measurements/assertions failed (exit=$HARNESS_EXIT)" >&2
  exit 1
fi
echo "PASS: qualified dedicated load harness completed" >&2
