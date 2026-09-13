#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
: "${OPENPR_TEST_DATABASE_URL:?OPENPR_TEST_DATABASE_URL is required}"

CACHE_ROOT=/opt/worker/.cache/openpr-v08-worker-compaction-mutations
WORKTREE="$CACHE_ROOT/worktree"
TARGET_DIR="$CACHE_ROOT/target"
LOG_DIR="$CACHE_ROOT/logs"
TEST_NAME=flow::compaction::tests::flow_compaction_worker_preserves_exact_document_fingerprint

cleanup() {
  git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir -p "$CACHE_ROOT" "$TARGET_DIR" "$LOG_DIR"
cleanup
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE" HEAD >/dev/null

# The accepted command path executes this production helper. Build it in the same isolated target
# so a missing executable cannot masquerade as a compaction verifier failure.
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
    cargo test --manifest-path "$WORKTREE/Cargo.toml" -p worker "$TEST_NAME" -- --exact --nocapture \
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

run_case worker_compaction_green_control green

SOURCE="$WORKTREE/apps/worker/src/flow/compaction.rs"
perl -0pi -e 's/cd\.head_seq > cd\.compaction_boundary_seq/cd.head_seq < cd.compaction_boundary_seq/' "$SOURCE"
grep -Fq 'cd.head_seq < cd.compaction_boundary_seq' "$SOURCE"
run_case eligible_document_silently_ignored red

printf 'PASS: green control passed and the production worker scheduling mutation was detected\n'
