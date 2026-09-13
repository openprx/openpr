#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
: "${OPENPR_TEST_DATABASE_URL:?OPENPR_TEST_DATABASE_URL is required}"

CACHE_ROOT=/opt/worker/.cache/openpr-v08-fanout-mutations
WORKTREE="$CACHE_ROOT/worktree"
TARGET_DIR=/opt/worker/.cache/openpr-v08-shared-target
LOG_DIR="$CACHE_ROOT/logs"
ATOMIC_TEST=flow::collab::write::database_tests::fanout_notice_failure_after_commit_keeps_the_canonical_update_and_head
NOTICE_TEST=flow::collab::write::database_tests::accept_update_commits_and_advances_head_seq_against_a_real_database
CURSOR_TEST=flow::collab::fanout::tests::transient_reconstruction_failure_retains_the_durable_cursor

cleanup() {
  git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir -p "$CACHE_ROOT" "$TARGET_DIR" "$LOG_DIR"
cleanup
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE" HEAD >/dev/null

# The real write path executes this production subprocess. Build it into the isolated target before
# running either control so a missing helper cannot masquerade as detection of a fanout mutant.
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
  cargo build --manifest-path "$WORKTREE/Cargo.toml" -p collab-core \
    --bin collab-isolated-apply-worker >"$LOG_DIR/isolated-worker-build.log" 2>&1

run_case() {
  local label=$1
  local expected=$2
  local test_name=$3
  local log="$LOG_DIR/$label.log"
  set +e
  env -u RUST_TEST_THREADS \
    OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" \
    CARGO_BUILD_JOBS=4 \
    CARGO_TARGET_DIR="$TARGET_DIR" \
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

run_case atomic_green_control green "$ATOMIC_TEST"
run_case notice_green_control green "$NOTICE_TEST"
run_case cursor_green_control green "$CURSOR_TEST"

WRITE_SOURCE="$WORKTREE/apps/api/src/flow/collab/write.rs"
perl -0pi -e 's/if let Err\(error\) = super::fanout::publish_document_update\((.*?)\)\n                \.await\n                \{.*?\n                \}/super::fanout::publish_document_update($1).await?;/s' "$WRITE_SOURCE"
grep -Fq ').await?;' "$WRITE_SOURCE"
run_case post_commit_fanout_failure_misreported red "$ATOMIC_TEST"
git -C "$WORKTREE" restore apps/api/src/flow/collab/write.rs

perl -0pi -e 's/if let Err\(error\) = super::fanout::publish_document_update\((.*?)\)\n                \.await\n                \{.*?\n                \}//s' "$WRITE_SOURCE"
if grep -Fq 'publish_document_update(' "$WRITE_SOURCE"; then
  echo 'FAIL: fanout omission mutation did not apply' >&2
  exit 1
fi
run_case committed_update_omits_fanout_notice red "$NOTICE_TEST"

FANOUT_SOURCE="$WORKTREE/apps/api/src/flow/collab/fanout.rs"
perl -0pi -e 's/("flow fanout reconstruction failed; retaining cursor for retry"\);\n)            break;/$1            cursor = notice.id;\n            break;/' "$FANOUT_SOURCE"
grep -Fq 'cursor = notice.id;' "$FANOUT_SOURCE"
run_case transient_failure_advances_cursor red "$CURSOR_TEST"

printf 'PASS: 3 green controls passed and 3/3 production-source mutations were detected\n'
