#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
: "${OPENPR_TEST_DATABASE_URL:?OPENPR_TEST_DATABASE_URL is required}"

CACHE_ROOT=/opt/worker/.cache/openpr-v08-replay-mutations
WORKTREE="$CACHE_ROOT/worktree"
TARGET_DIR=/opt/worker/.cache/openpr-v08-shared-target
LOG_DIR="$CACHE_ROOT/logs"
RETENTION_TEST=events::dispatcher::dispatcher_database_tests::replay_is_windowed_deduplicated_and_crosses_delivery_retention_without_duplication
ANCHOR_TEST=events::dispatcher::dispatcher_database_tests::requeue_failed_filters_terminated_time_and_preserves_delivery_id
ROUTE_TEST=routes::flow::flow_database_tests::delivery_replay_route_requires_admin_and_replays_identical_idempotency_key
BACKOFF_TEST=events::dispatcher::dispatcher_database_tests::flow_delivery_retry_backoff_matches_every_frozen_attempt

cleanup() {
  git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir -p "$CACHE_ROOT" "$TARGET_DIR" "$LOG_DIR"
cleanup
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE" HEAD >/dev/null

run_case() {
  local label=$1
  local expected=$2
  local test_name=$3
  local log="$LOG_DIR/$label.log"
  set +e
  env -u RUST_TEST_THREADS \
    OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" \
    CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
    cargo test --manifest-path "$WORKTREE/Cargo.toml" -p api --lib "$test_name" -- --exact --nocapture \
      >"$log" 2>&1
  local status=$?
  set -e
  if [[ $expected == green && $status -ne 0 ]]; then
    tail -80 "$log" >&2
    echo "FAIL: green control exited $status" >&2
    exit 1
  fi
  if [[ $expected == red && $status -eq 0 ]]; then
    tail -80 "$log" >&2
    echo "FAIL: mutation $label was not detected" >&2
    exit 1
  fi
  printf '%s status=%s expected=%s log=%s\n' "$label" "$status" "$expected" "$log"
}

run_case retention_green_control green "$RETENTION_TEST"
run_case terminated_anchor_green_control green "$ANCHOR_TEST"
run_case admin_idempotency_green_control green "$ROUTE_TEST"
run_case delivery_backoff_green_control green "$BACKOFF_TEST"

DISPATCHER="$WORKTREE/apps/api/src/events/dispatcher.rs"
ROUTES="$WORKTREE/apps/api/src/routes/flow.rs"

perl -0pi -e 's/const DELIVERY_SOURCE_RETENTION_DAYS: i64 = 90;/const DELIVERY_SOURCE_RETENTION_DAYS: i64 = 30;/' "$DISPATCHER"
grep -Fq 'const DELIVERY_SOURCE_RETENTION_DAYS: i64 = 30;' "$DISPATCHER"
run_case source_tombstone_expires_with_delivery red "$RETENTION_TEST"
git -C "$WORKTREE" restore apps/api/src/events/dispatcher.rs

perl -0pi -e 's/d\.terminated_at >= \$2 AND d\.terminated_at < \$3/be.created_at >= \$2 AND be.created_at < \$3/' "$DISPATCHER"
grep -Fq "be.created_at >= \$2 AND be.created_at < \$3" "$DISPATCHER"
run_case requeue_filters_source_event_time red "$ANCHOR_TEST"
git -C "$WORKTREE" restore apps/api/src/events/dispatcher.rs

perl -0pi -e 's/if request\.from <= oldest \{/if request.from < oldest {/' "$DISPATCHER"
grep -Fq 'if request.from < oldest {' "$DISPATCHER"
run_case replay_exact_oldest_boundary_allowed red "$RETENTION_TEST"
git -C "$WORKTREE" restore apps/api/src/events/dispatcher.rs

perl -0pi -e 's/const DELIVERY_BACKOFF_STEP_MS: i64 = 30_000;/const DELIVERY_BACKOFF_STEP_MS: i64 = 31_000;/' "$DISPATCHER"
grep -Fq 'const DELIVERY_BACKOFF_STEP_MS: i64 = 31_000;' "$DISPATCHER"
run_case delivery_backoff_step_drift red "$BACKOFF_TEST"

perl -0pi -e 's/(pub async fn post_flow_delivery_replay.*?policy::)require_flow_workspace_admin_access/$1require_flow_workspace_access/s' "$ROUTES"
grep -A30 -F 'pub async fn post_flow_delivery_replay' "$ROUTES" | grep -Fq 'require_flow_workspace_access'
run_case replay_accepts_non_admin_member red "$ROUTE_TEST"

printf 'PASS: 4 green controls passed and 5/5 production-source mutations were detected\n'
