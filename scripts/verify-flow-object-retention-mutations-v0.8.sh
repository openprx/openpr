#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
: "${OPENPR_TEST_DATABASE_URL:?OPENPR_TEST_DATABASE_URL is required}"

CACHE_ROOT=/opt/worker/.cache/openpr-v08-object-retention-mutations
WORKTREE="$CACHE_ROOT/worktree"
TARGET_DIR="$CACHE_ROOT/target"
LOG_DIR="$CACHE_ROOT/logs"
TIER_TEST=flow::command::database_tests::flow_collection_container_archive_tier_enforces_collection_and_page_contrast
WORKER_TEST=flow::retention::tests::worker_deletes_only_expired_full_access_archives_not_edit_tier_soft_archives

cleanup() {
  git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir -p "$CACHE_ROOT" "$TARGET_DIR" "$LOG_DIR"
cleanup
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE" HEAD >/dev/null

env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
  cargo build --manifest-path "$WORKTREE/Cargo.toml" -p collab-core \
    --bin collab-isolated-apply-worker >"$LOG_DIR/isolated-worker-build.log" 2>&1

run_case() {
  local label=$1
  local expected=$2
  local package=$3
  local target_args=$4
  local test_name=$5
  local log="$LOG_DIR/$label.log"
  local -a cargo_target
  if [[ $target_args == --lib ]]; then
    cargo_target=(--lib)
  else
    cargo_target=(--bin worker)
  fi
  set +e
  env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" \
    CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
    cargo test --manifest-path "$WORKTREE/Cargo.toml" -p "$package" "${cargo_target[@]}" \
      "$test_name" -- --exact --nocapture >"$log" 2>&1
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

run_case tier_green_control green api --lib "$TIER_TEST"
run_case worker_retention_green_control green worker '--bin worker' "$WORKER_TEST"

COMMAND="$WORKTREE/apps/api/src/flow/command.rs"
RETENTION="$WORKTREE/apps/worker/src/flow/retention.rs"

perl -0pi -e 's/\|\| facts\.object_type == "collection"//' "$COMMAND"
if grep -Fq '|| facts.object_type == "collection"' "$COMMAND"; then
  echo 'FAIL: collection tier mutation did not apply' >&2
  exit 1
fi
run_case collection_archive_falls_to_edit red api --lib "$TIER_TEST"
git -C "$WORKTREE" restore apps/api/src/flow/command.rs

perl -0pi -e 's/ && required_tier == authz::PermissionLevel::FullAccess//' "$COMMAND"
if grep -Fq '&& required_tier == authz::PermissionLevel::FullAccess' "$COMMAND"; then
  echo 'FAIL: cleanup tier mutation did not apply' >&2
  exit 1
fi
run_case edit_tier_soft_archive_schedules_deletion red api --lib "$TIER_TEST"
git -C "$WORKTREE" restore apps/api/src/flow/command.rs

perl -0pi -e "s/ AND permanent_cleanup_after < \\\$2//g" "$RETENTION"
if grep -Fq "permanent_cleanup_after < \$2" "$RETENTION"; then
  echo 'FAIL: worker deadline mutation did not apply' >&2
  exit 1
fi
run_case worker_deletes_archives_without_eligibility_deadline red worker '--bin worker' "$WORKER_TEST"

perl -0pi -e 's/if target_status == "active" \{.*?\n    \}\n    let changed/let changed/s' "$COMMAND"
if grep -A8 -F 'if target_status == "active"' "$COMMAND" | grep -Fq permanent_cleanup_after; then
  echo 'FAIL: restore cleanup cancellation mutation did not apply' >&2
  exit 1
fi
run_case restore_leaves_irreversible_deadline red api --lib "$TIER_TEST"

printf 'PASS: 2 green controls passed and 4/4 production-source mutations were detected\n'
