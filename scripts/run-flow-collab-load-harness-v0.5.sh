#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
EVIDENCE_ROOT=""
CLIENTS=""
DATABASE_URL="postgresql://flowtest:flowtest@127.0.0.1:25433/postgres"
PG_CONTAINER="flow-test-pg"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/run-flow-collab-load-harness-v0.5.sh --clients 10 --evidence-root DIR --json [OPTIONS]

Runs the v0.5 official collaboration workload in release mode against the
fixed real PostgreSQL test endpoint. The runner refuses to start while another
cargo test/build is active or another non-idle database client exists. It
produces fresh v0.5 concurrency, lock, cache, restart, and fan-out evidence;
no v0.4 receipt is accepted as an input.

Options:
  --clients N          Required; the hard-gate fixture is exactly 10.
  --evidence-root DIR  Required output directory.
  --repo-root DIR      Repository root. Default: this checkout.
  --database-url URL   Must equal the dispatch-fixed test URL.
  --pg-container NAME  PostgreSQL container serving the fixed URL.
  --json               Required.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --clients) CLIENTS="${2:?--clients requires N}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires DIR}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires DIR}"; shift 2 ;;
    --database-url) DATABASE_URL="${2:?--database-url requires URL}"; shift 2 ;;
    --pg-container) PG_CONTAINER="${2:?--pg-container requires NAME}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ $JSON_MODE -eq 1 ]] || { echo "FAIL: --json is required" >&2; exit 2; }
[[ "$CLIENTS" =~ ^[1-9][0-9]*$ ]] || { echo "FAIL: --clients must be a positive integer" >&2; exit 2; }
[[ -n "$EVIDENCE_ROOT" ]] || { echo "FAIL: --evidence-root is required" >&2; exit 2; }
for tool in cargo git jq psql podman sha256sum nproc; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required command: $tool" >&2; exit 2; }
done

FIXED_DATABASE_URL="postgresql://flowtest:flowtest@127.0.0.1:25433/postgres"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
SOURCE_DIRTY=false
[[ -z "$(git -C "$REPO_ROOT" status --porcelain)" ]] || SOURCE_DIRTY=true
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
mkdir -p "$EVIDENCE_ROOT/logs"
OUT="$EVIDENCE_ROOT/collab-load-result.json"

LOAD_AVERAGE_BEFORE="$(cut -d' ' -f1-3 /proc/loadavg)"
CORES="$(nproc)"
CPU="$(sed -n 's/^model name[[:space:]]*:[[:space:]]*//p' /proc/cpuinfo | head -1)"
RAM_BYTES="$(awk '/^MemTotal:/ {print $2 * 1024}' /proc/meminfo)"
POSTGRES_VERSION="$(psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atc 'select version()' 2>/dev/null || true)"
ACTIVE_OTHER_CLIENTS="$(psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atc \
  "select count(*) from pg_stat_activity where pid <> pg_backend_pid() and backend_type='client backend' and state <> 'idle'" \
  2>/dev/null || true)"
CARGO_PROCESSES="$(ps -eo pid=,args= | awk '/cargo (test|build|check|clippy)/ && $0 !~ /awk/ {print}' || true)"
CONTAINER_RUNNING="$(podman inspect -f '{{.State.Running}}' "$PG_CONTAINER" 2>/dev/null || true)"
ACTIVE_OTHER_CLIENTS_JSON=null
[[ "$ACTIVE_OTHER_CLIENTS" =~ ^[0-9]+$ ]] && ACTIVE_OTHER_CLIENTS_JSON="$ACTIVE_OTHER_CLIENTS"

PREFLIGHT_VIOLATIONS='[]'
add_preflight_violation() {
  PREFLIGHT_VIOLATIONS="$(jq -c --arg value "$1" '. + [$value]' <<<"$PREFLIGHT_VIOLATIONS")"
}
[[ "$CLIENTS" -eq 10 ]] || add_preflight_violation "the hard gate requires exactly 10 clients, got $CLIENTS"
[[ "$DATABASE_URL" == "$FIXED_DATABASE_URL" ]] || add_preflight_violation "database URL differs from the dispatch-fixed test database"
[[ "$SOURCE_DIRTY" == false ]] || add_preflight_violation "source worktree is dirty"
[[ "$CONTAINER_RUNNING" == true ]] || add_preflight_violation "PostgreSQL container $PG_CONTAINER is not running"
[[ -n "$POSTGRES_VERSION" ]] || add_preflight_violation "the fixed PostgreSQL endpoint is unreachable"
[[ "$ACTIVE_OTHER_CLIENTS" =~ ^[0-9]+$ ]] || add_preflight_violation "could not count active PostgreSQL clients"
[[ "$ACTIVE_OTHER_CLIENTS" == 0 ]] || add_preflight_violation "fixed PostgreSQL has $ACTIVE_OTHER_CLIENTS other active client(s)"
[[ -z "$CARGO_PROCESSES" ]] || add_preflight_violation "another cargo test/build/check/clippy process is active"

write_preflight_failure() {
  jq -n \
    --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" --argjson dirty "$SOURCE_DIRTY" \
    --arg cpu "$CPU" --argjson cores "$CORES" --argjson ram "$RAM_BYTES" \
    --arg load "$LOAD_AVERAGE_BEFORE" --arg pg "$POSTGRES_VERSION" --arg container "$PG_CONTAINER" \
    --argjson active "$ACTIVE_OTHER_CLIENTS_JSON" --arg cargo "$CARGO_PROCESSES" \
    --argjson clients "$CLIENTS" --argjson violations "$PREFLIGHT_VIOLATIONS" \
    '{schema_version:"sylvode.flow.collab-load-v0.5-result.v1",release:"0.5",
      source_head:$head,source_dirty:$dirty,generated_at:$generated_at,
      environment:{build_profile:"release",database_kind:"real_postgresql",postgres_version:$pg,
        pg_container:$container,cpu:$cpu,cores:$cores,ram_bytes:$ram,
        load_average_before:$load,active_other_clients_before:$active,cargo_processes_before:$cargo,
        qualification_status:"not_satisfied"},
      fixture:{clients:$clients,warmup_rounds:5,min_measurements:30,locked:true},
      execution:{status:"not_run_environment_not_satisfied"},violations:$violations,passed:false}' \
    >"$OUT.tmp"
  mv -f "$OUT.tmp" "$OUT"
  jq . "$OUT"
}

if [[ "$(jq 'length' <<<"$PREFLIGHT_VIOLATIONS")" -ne 0 ]]; then
  write_preflight_failure
  exit 1
fi

FIXTURE_SHA256="$({
  sha256sum \
    "$REPO_ROOT/apps/api/tests/flow_collab_load_harness.rs" \
    "$REPO_ROOT/apps/api/tests/flow_collab_v05_session_round_trip.rs" \
    "$REPO_ROOT/apps/api/tests/flow_collab_cache_evidence.rs" \
    "$REPO_ROOT/apps/api/src/flow/collab/egress.rs" \
    "$REPO_ROOT/apps/api/src/flow/collab/session.rs" \
    "$REPO_ROOT/Cargo.lock"
} | sha256sum | awk '{print $1}')"

BASELINE_OUT="$EVIDENCE_ROOT/logs/v0.5-lock-load-raw.json"
SESSION_OUT="$EVIDENCE_ROOT/logs/v0.5-session-load-raw.json"
CACHE_OUT="$EVIDENCE_ROOT/logs/v0.5-cache-raw.json"
started_ms="$(date +%s%3N)"

set +e
(cd "$REPO_ROOT" && cargo build --release -p collab-core --bin collab-isolated-apply-worker) \
  >"$EVIDENCE_ROOT/logs/v0.5-worker-build.log" 2>&1
WORKER_BUILD_EXIT=$?

if [[ $WORKER_BUILD_EXIT -eq 0 ]]; then
  (cd "$REPO_ROOT" && \
    OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
    OPENPR_FLOW_DEDICATED_PG_CONTAINER="$PG_CONTAINER" \
    OPENPR_FLOW_PG_LOG_CONTAINER="$PG_CONTAINER" \
    OPENPR_FLOW_PG_LOG_ENGINE=podman \
    OPENPR_FLOW_QUIET_PG_QUALIFIED=1 \
    OPENPR_FLOW_LOAD_HARNESS_OUT="$BASELINE_OUT" \
    cargo test --release -p api --test flow_collab_load_harness \
      ten_client_load_harness_round_trip_p95_and_lock_hold_p95 -- --exact --nocapture) \
    >"$EVIDENCE_ROOT/logs/v0.5-lock-load.log" 2>&1
  BASELINE_EXIT=$?
else
  BASELINE_EXIT=125
fi

if [[ $BASELINE_EXIT -eq 0 ]]; then
  (cd "$REPO_ROOT" && \
    OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
    OPENPR_FLOW_DEDICATED_PG_CONTAINER="$PG_CONTAINER" \
    OPENPR_FLOW_QUIET_PG_QUALIFIED=1 \
    OPENPR_FLOW_V05_SESSION_ROUND_TRIP_OUT="$SESSION_OUT" \
    cargo test --release -p api --test flow_collab_v05_session_round_trip \
      v05_multi_user_session_workload_round_trip_p95 -- --exact --nocapture) \
    >"$EVIDENCE_ROOT/logs/v0.5-session-load.log" 2>&1
  SESSION_EXIT=$?
else
  SESSION_EXIT=125
fi

if [[ $SESSION_EXIT -eq 0 ]]; then
  (cd "$REPO_ROOT" && \
    OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
    OPENPR_FLOW_PG_LOG_CONTAINER="$PG_CONTAINER" \
    OPENPR_FLOW_CACHE_EVIDENCE_OUT="$CACHE_OUT" \
    cargo test --release -p api --test flow_collab_cache_evidence \
      warm_cache_boundaries_rebuild_bypass_and_retry_exhaustion_evidence -- --exact --nocapture) \
    >"$EVIDENCE_ROOT/logs/v0.5-cache.log" 2>&1
  CACHE_EXIT=$?
else
  CACHE_EXIT=125
fi
set -e

duration_ms="$(( $(date +%s%3N) - started_ms ))"
LOAD_AVERAGE_AFTER="$(cut -d' ' -f1-3 /proc/loadavg)"
ACTIVE_OTHER_CLIENTS_AFTER="$(psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atc \
  "select count(*) from pg_stat_activity where pid <> pg_backend_pid() and backend_type='client backend' and state <> 'idle'" \
  2>/dev/null || true)"
ACTIVE_OTHER_CLIENTS_AFTER_JSON=null
[[ "$ACTIVE_OTHER_CLIENTS_AFTER" =~ ^[0-9]+$ ]] && ACTIVE_OTHER_CLIENTS_AFTER_JSON="$ACTIVE_OTHER_CLIENTS_AFTER"

baseline='{}'; session='{}'; cache='{}'
[[ -f "$BASELINE_OUT" ]] && baseline="$(jq -c . "$BASELINE_OUT")"
[[ -f "$SESSION_OUT" ]] && session="$(jq -c . "$SESSION_OUT")"
[[ -f "$CACHE_OUT" ]] && cache="$(jq -c . "$CACHE_OUT")"

VIOLATIONS='[]'
add_violation() { VIOLATIONS="$(jq -c --arg value "$1" '. + [$value]' <<<"$VIOLATIONS")"; }
[[ $WORKER_BUILD_EXIT -eq 0 ]] || add_violation "release isolated-apply worker build failed (exit=$WORKER_BUILD_EXIT)"
[[ $BASELINE_EXIT -eq 0 ]] || add_violation "release lock/load harness failed or was not run (exit=$BASELINE_EXIT)"
[[ $SESSION_EXIT -eq 0 ]] || add_violation "release v0.5 multi-user/offline harness failed or was not run (exit=$SESSION_EXIT)"
[[ $CACHE_EXIT -eq 0 ]] || add_violation "release cache/eviction/restart harness failed or was not run (exit=$CACHE_EXIT)"
if [[ $BASELINE_EXIT -eq 0 && $SESSION_EXIT -eq 0 && $CACHE_EXIT -eq 0 ]]; then
  [[ "$(jq -r '.passed' <<<"$baseline")" == true ]] || add_violation "lock/load component self-verdict is not passed"
  [[ "$(jq -r '.passed' <<<"$session")" == true ]] || add_violation "multi-user/offline component self-verdict is not passed"
  [[ "$(jq -r '.passed' <<<"$cache")" == true ]] || add_violation "cache component self-verdict is not passed"
  [[ "$(jq -r '.environment.build_profile' <<<"$baseline")" == release ]] || add_violation "lock/load component was not release-built"
  [[ "$(jq -r '.environment.build_profile' <<<"$session")" == release ]] || add_violation "session component was not release-built"
  [[ "$(jq -r '.environment.build_profile' <<<"$cache")" == release ]] || add_violation "cache component was not release-built"
  [[ "$(jq -r '."10_client".clients' <<<"$baseline")" -eq 10 ]] || add_violation "lock/load component did not run 10 clients"
  [[ "$(jq -r '.fixture.clients' <<<"$session")" -eq 10 ]] || add_violation "session component did not run 10 clients"
  [[ "$(jq -r '."10_client".warmup_rounds' <<<"$baseline")" -eq 5 ]] || add_violation "lock/load component did not run five warmups"
  [[ "$(jq -r '.fixture.warmup_rounds' <<<"$session")" -eq 5 ]] || add_violation "session component did not run five warmups"
  [[ "$(jq -r '.lock.lock_hold_p95.samples' <<<"$baseline")" -ge 30 ]] || add_violation "lock-hold samples are below 30"
  [[ "$(jq -r '.measured.round_trip.samples' <<<"$session")" -ge 30 ]] || add_violation "multi-user round-trip samples are below 30"
  [[ "$(jq -r '.measured.fanout_rounds_complete' <<<"$session")" -eq 150 ]] || add_violation "not all controlled fan-out rounds completed"
  [[ "$(jq -r '.measured.peer_accepted_observed' <<<"$session")" -eq 1350 ]] || add_violation "10-client fan-out did not deliver all 1350 peer accepted frames"
  [[ "$(jq -r '.measured.peer_updates_observed' <<<"$session")" -eq 1350 ]] || add_violation "10-client fan-out did not deliver all 1350 peer update frames"
fi

PASSED=false
[[ "$(jq 'length' <<<"$VIOLATIONS")" -eq 0 ]] && PASSED=true
jq -n \
  --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" --arg fixture_sha "$FIXTURE_SHA256" \
  --arg cpu "$CPU" --argjson cores "$CORES" --argjson ram "$RAM_BYTES" --arg pg "$POSTGRES_VERSION" \
  --arg container "$PG_CONTAINER" --arg load_before "$LOAD_AVERAGE_BEFORE" --arg load_after "$LOAD_AVERAGE_AFTER" \
  --argjson active_before "$ACTIVE_OTHER_CLIENTS_JSON" --argjson active_after "$ACTIVE_OTHER_CLIENTS_AFTER_JSON" \
  --argjson dirty "$SOURCE_DIRTY" --argjson clients "$CLIENTS" --argjson duration "$duration_ms" \
  --argjson worker_exit "$WORKER_BUILD_EXIT" --argjson baseline_exit "$BASELINE_EXIT" \
  --argjson session_exit "$SESSION_EXIT" --argjson cache_exit "$CACHE_EXIT" \
  --argjson baseline "$baseline" --argjson session "$session" --argjson cache "$cache" \
  --argjson violations "$VIOLATIONS" --argjson passed "$PASSED" \
  '{schema_version:"sylvode.flow.collab-load-v0.5-result.v1",release:"0.5",
    source_head:$head,source_dirty:$dirty,generated_at:$generated_at,
    environment:{build_profile:"release",database_kind:"real_postgresql",postgres_version:$pg,
      pg_container:$container,cpu:$cpu,cores:$cores,ram_bytes:$ram,
      load_average_before:$load_before,load_average_after:$load_after,
      active_other_clients_before:$active_before,active_other_clients_after:$active_after,
      concurrent_cargo_before:0,qualification_status:"satisfied",
      qualification_note:"dispatch-fixed PostgreSQL accepted only after zero-active-client and zero-cargo preflight"},
    fixture:{clients:$clients,distinct_users:true,warmup_rounds:5,min_measurements:30,locked:true,
      fixture_sha256:$fixture_sha,offline_resume_clients:3,
      injections:["presence_fanout","content_fanout_barrier","offline_resume"]},
    execution:{duration_ms:$duration,release_worker_build_exit:$worker_exit,
      lock_load_exit:$baseline_exit,session_load_exit:$session_exit,cache_exit:$cache_exit},
    concurrency:$session.measured,lock:$baseline.lock,cache:$cache,
    components:{lock_load:$baseline,multi_user_offline:$session,cache_eviction_restart:$cache},
    violations:$violations,passed:$passed}' >"$OUT.tmp"
mv -f "$OUT.tmp" "$OUT"
jq . "$OUT"
[[ "$PASSED" == true ]]
