#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
: "${OPENPR_TEST_DATABASE_URL:?OPENPR_TEST_DATABASE_URL is required}"

CACHE_ROOT=/opt/worker/.cache/openpr-v08-compaction-mutations
WORKTREE="$CACHE_ROOT/worktree"
TARGET_DIR="$CACHE_ROOT/target"
LOG_DIR="$CACHE_ROOT/logs"
TEST_NAME=flow::collab::snapshot::database_tests::compaction_preserves_head_frontier_and_hash_and_never_swallows_a_lagging_ack

cleanup() {
  git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir -p "$CACHE_ROOT" "$TARGET_DIR" "$LOG_DIR"
cleanup
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE" HEAD >/dev/null

# `accept_update` executes this production subprocess. A separate target directory starts empty;
# omitting the build makes the green control fail `Internal` before compaction is reached.
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
  cargo build --manifest-path "$WORKTREE/Cargo.toml" -p collab-core \
    --bin collab-isolated-apply-worker >"$LOG_DIR/isolated-worker-build.log" 2>&1

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

run_case green_control green

SOURCE="$WORKTREE/apps/api/src/flow/collab/compaction.rs"
perl -0pi -e 's/} else if force_resync \{/} else if true {/' "$SOURCE"
grep -Fq '} else if true {' "$SOURCE"
run_case force_flag_ignored red
perl -0pi -e 's/} else if true \{/} else if force_resync {/' "$SOURCE"

perl -0pi -e 's/if expected\.is_none_or\(\|row\| row\.frontier != frontier\) \{/if false {/' "$SOURCE"
grep -Fq 'if false {' "$SOURCE"
run_case forged_frontier_accepted red

printf 'PASS: green control passed and 2/2 production-source mutations were detected\n'
