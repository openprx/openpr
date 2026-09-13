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
BACKOFF_TEST=events::dispatcher::dispatcher_database_tests::delivery_attempts_one_through_ten_write_the_frozen_database_backoff_and_never_attempt_eleven
RECOVERY_TEST=events::dispatcher::dispatcher_database_tests::flow_delivery_failure_then_fresh_dispatcher_delivers_once_with_the_same_delivery_id
COALESCED_CONSUMER_TEST=events::dispatcher::dispatcher_database_tests::golden_wire_fixture_coalesced_delivery_body_matches_the_frozen_shape
CONCURRENT_REPLAY_TEST=events::dispatcher::dispatcher_database_tests::flow_delivery_v08_concurrent_replay_reserves_before_building_and_never_merges
BLOCK_UNION_TEST=events::dispatcher::dispatcher_database_tests::flow_delivery_v08_changed_block_union_exact_limit_and_plus_one_truncation
LEASE_PAIR_TEST=events::dispatcher::dispatcher_database_tests::flow_delivery_v08_dispatch_and_delivery_lease_pairs_reject_both_unreachable_halves
CURRENT_TARGET_TEST=events::dispatcher::dispatcher_database_tests::flow_delivery_v08_reads_the_current_endpoint_and_rotated_secret_only_at_send_time
BACKLOG_CANCEL_TEST=events::dispatcher::dispatcher_database_tests::flow_delivery_v08_deleting_a_subscriber_cancels_its_entire_backlog_without_dead_letter_or_audit_loss

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
run_case delivery_recovery_green_control green "$RECOVERY_TEST"
run_case consumer_dedupe_green_control green "$COALESCED_CONSUMER_TEST"
run_case concurrent_replay_green_control green "$CONCURRENT_REPLAY_TEST"
run_case block_union_green_control green "$BLOCK_UNION_TEST"
run_case lease_pair_green_control green "$LEASE_PAIR_TEST"
run_case current_target_green_control green "$CURRENT_TARGET_TEST"
run_case backlog_cancel_green_control green "$BACKLOG_CANCEL_TEST"

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
git -C "$WORKTREE" restore apps/api/src/events/dispatcher.rs

perl -0pi -e "s/(async fn mark_delivered.*?SET status = )'dispatched'/\$1'failed'/s" "$DISPATCHER"
sed -n '/async fn mark_delivered/,/async fn cancel_delivery/p' "$DISPATCHER" | grep -Fq "SET status = 'failed'"
run_case successful_delivery_never_terminalizes red "$RECOVERY_TEST"
git -C "$WORKTREE" restore apps/api/src/events/dispatcher.rs

perl -0pi -e "s/(async fn reclaim_expired_delivery_leases.*?WHERE )status = 'leased' AND lease_expires_at < now\(\)/\${1}false AND status = 'leased' AND lease_expires_at < now()/s" "$DISPATCHER"
grep -A35 -F 'async fn reclaim_expired_delivery_leases' "$DISPATCHER" | grep -Fq "WHERE false AND status = 'leased'"
run_case delivery_crash_lease_not_reclaimed red "$RECOVERY_TEST"
git -C "$WORKTREE" restore apps/api/src/events/dispatcher.rs

perl -0pi -e 's/"source_event_ids": source_ids,/"source_event_ids": vec![first_event.id],/' "$DISPATCHER"
grep -Fq '"source_event_ids": vec![first_event.id],' "$DISPATCHER"
run_case coalesced_consumer_uses_event_id red "$COALESCED_CONSUMER_TEST"
git -C "$WORKTREE" restore apps/api/src/events/dispatcher.rs

perl -0pi -e 's/(pub async fn replay_deliveries.*?ON CONFLICT \(subscriber_kind, subscriber_id, source_event_id\) )DO NOTHING/${1}DO UPDATE SET created_at = event_delivery_sources.created_at/s' "$DISPATCHER"
grep -Fq 'DO UPDATE SET created_at = event_delivery_sources.created_at' "$DISPATCHER"
run_case replay_check_then_build_race red "$CONCURRENT_REPLAY_TEST"
git -C "$WORKTREE" restore apps/api/src/events/dispatcher.rs

perl -0pi -e 's/const CHANGED_BLOCK_IDS_PER_DELIVERY_MAX: usize = 200;/const CHANGED_BLOCK_IDS_PER_DELIVERY_MAX: usize = 201;/' "$DISPATCHER"
grep -Fq 'const CHANGED_BLOCK_IDS_PER_DELIVERY_MAX: usize = 201;' "$DISPATCHER"
run_case changed_block_union_ceiling_drift red "$BLOCK_UNION_TEST"
git -C "$WORKTREE" restore apps/api/src/events/dispatcher.rs

MIGRATION="$WORKTREE/migrations/0054_flow_data_layer.sql"
perl -0pi -e 's/CHECK \(\(lease_token IS NULL\) = \(lease_expires_at IS NULL\)\)/CHECK (true)/g' "$MIGRATION"
[[ $(grep -Fc 'CHECK (true)' "$MIGRATION") -eq 2 ]]
run_case lease_pair_constraint_removed red "$LEASE_PAIR_TEST"
git -C "$WORKTREE" restore migrations/0054_flow_data_layer.sql

perl -0pi -e "s/SELECT url, secret FROM webhooks/SELECT 'http:\/\/old.example.invalid\/hook' AS url, secret FROM webhooks/" "$DISPATCHER"
grep -Fq "SELECT 'http://old.example.invalid/hook' AS url" "$DISPATCHER"
run_case delivery_uses_stale_endpoint_snapshot red "$CURRENT_TARGET_TEST"
git -C "$WORKTREE" restore apps/api/src/events/dispatcher.rs

perl -0pi -e "s/SET status = 'cancelled'/SET status = 'failed'/" "$DISPATCHER"
grep -A18 -F 'async fn cancel_delivery' "$DISPATCHER" | grep -Fq "SET status = 'failed'"
run_case subscriber_gone_inflates_dead_letter red "$BACKLOG_CANCEL_TEST"
git -C "$WORKTREE" restore apps/api/src/events/dispatcher.rs

perl -0pi -e 's/(pub async fn post_flow_delivery_replay.*?policy::)require_flow_workspace_admin_access/$1require_flow_workspace_access/s' "$ROUTES"
grep -A30 -F 'pub async fn post_flow_delivery_replay' "$ROUTES" | grep -Fq 'require_flow_workspace_access'
run_case replay_accepts_non_admin_member red "$ROUTE_TEST"

printf 'PASS: 11 green controls passed and 13/13 production-source mutations were detected\n'
