#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
: "${OPENPR_TEST_DATABASE_URL:?OPENPR_TEST_DATABASE_URL is required}"

CACHE_ROOT=/opt/worker/.cache/openpr-v08-authz-broadcast-mutations
WORKTREE="$CACHE_ROOT/worktree"
TARGET_DIR="$CACHE_ROOT/target"
LOG_DIR="$CACHE_ROOT/logs"
TEST_NAME=flow::grants::database_tests::multi_instance_revocation_closes_subtree_sessions_without_skipping_log_epochs

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
  local log="$LOG_DIR/$label.log"
  set +e
  env -u RUST_TEST_THREADS \
    OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" \
    CARGO_BUILD_JOBS=4 \
    CARGO_TARGET_DIR="$TARGET_DIR" \
    cargo test --manifest-path "$WORKTREE/Cargo.toml" -p api --lib "$TEST_NAME" -- --exact --nocapture \
      >"$log" 2>&1
  local status=$?
  set -e
  if [[ $expected == green && $status -ne 0 ]]; then
    tail -100 "$log" >&2
    echo "FAIL: green control exited $status" >&2
    exit 1
  fi
  if [[ $expected == red && $status -eq 0 ]]; then
    tail -100 "$log" >&2
    echo "FAIL: mutation $label was not detected" >&2
    exit 1
  fi
  printf '%s status=%s expected=%s log=%s\n' "$label" "$status" "$expected" "$log"
}

run_case durable_broadcast_green_control green

FANOUT_SOURCE="$WORKTREE/apps/api/src/flow/collab/fanout.rs"
perl -0pi -e 's/ORDER BY authz_epoch ASC LIMIT \$3/ORDER BY authz_epoch DESC LIMIT \$3/' "$FANOUT_SOURCE"
grep -Fq "ORDER BY authz_epoch DESC LIMIT \$3" "$FANOUT_SOURCE"
run_case later_epoch_hides_earlier_revocation red

git -C "$WORKTREE" checkout -- apps/api/src/flow/collab/fanout.rs
perl -0pi -e 's/authz_epoch > \$2/authz_epoch < \$2/' "$FANOUT_SOURCE"
grep -Fq "authz_epoch < \$2" "$FANOUT_SOURCE"
run_case lost_notify_becomes_permanent_miss red

printf 'PASS: green control passed and 2/2 production-source authorization broadcast mutations were detected\n'
