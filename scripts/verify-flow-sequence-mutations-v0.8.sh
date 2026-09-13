#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
: "${OPENPR_TEST_DATABASE_URL:?OPENPR_TEST_DATABASE_URL is required}"

CACHE_ROOT=/opt/worker/.cache/openpr-v08-sequence-mutations
WORKTREE="$CACHE_ROOT/worktree"
TARGET_DIR=/opt/worker/.cache/openpr-v08-shared-target
LOG_DIR="$CACHE_ROOT/logs"
ALLOC_TEST=flow::collab::write::database_tests::independent_api_instances_allocate_distinct_contiguous_document_sequences
EGRESS_TEST=flow::collab::egress::tests::a_seq_ahead_of_expectation_is_a_gap_with_the_exact_missing_range

cleanup() {
  git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir -p "$CACHE_ROOT" "$TARGET_DIR" "$LOG_DIR"
cleanup
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE" HEAD >/dev/null

# The canonical write path executes this helper. Build it in the isolated target first so a
# missing executable cannot masquerade as detection of the allocation mutation.
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

run_case allocation_green_control green "$ALLOC_TEST"
run_case egress_gap_green_control green "$EGRESS_TEST"

WRITE_SOURCE="$WORKTREE/apps/api/src/flow/collab/write.rs"
perl -0pi -e 's/if locked\.head_seq != prepared\.observed\.head_seq \{/if false {/' "$WRITE_SOURCE"
perl -0pi -e 's/let new_head_seq = locked\.head_seq \+ 1;/let new_head_seq = prepared.observed.head_seq + 1;/' "$WRITE_SOURCE"
grep -Fq 'if false {' "$WRITE_SOURCE"
grep -Fq 'let new_head_seq = prepared.observed.head_seq + 1;' "$WRITE_SOURCE"
run_case stale_prepared_head_allocates_duplicate_sequence red "$ALLOC_TEST"

EGRESS_SOURCE="$WORKTREE/apps/api/src/flow/collab/egress.rs"
perl -0pi -e 's/Ordering::Greater => SeqDecision::Gap \{\n                missing_from: self\.next_expected_seq,\n                missing_to: seq - 1,\n            \},/Ordering::Greater => SeqDecision::InOrder,/' "$EGRESS_SOURCE"
grep -Fq 'Ordering::Greater => SeqDecision::InOrder' "$EGRESS_SOURCE"
run_case forward_gap_is_silently_forwarded red "$EGRESS_TEST"

printf 'PASS: 2 green controls passed and 2/2 production-source mutations were detected\n'
