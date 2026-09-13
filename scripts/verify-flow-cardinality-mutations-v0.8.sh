#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CACHE_ROOT=/opt/worker/.cache/openpr-v08-cardinality-mutations
WORKTREE="$CACHE_ROOT/worktree"
TARGET_DIR=/opt/worker/.cache/openpr-v08-shared-target
LOG_DIR="$CACHE_ROOT/logs"
TEST_NAME=flow::command::cardinality_gate_tests::v0_8_hardening_registry_declares_every_new_command_cardinality

cleanup() { git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true; }
trap cleanup EXIT
mkdir -p "$CACHE_ROOT" "$TARGET_DIR" "$LOG_DIR"
cleanup
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE" HEAD >/dev/null

run_case() {
  local label=$1 expected=$2 log="$LOG_DIR/$1.log"
  set +e
  env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
    cargo test --manifest-path "$WORKTREE/Cargo.toml" -p api --lib "$TEST_NAME" -- --exact --nocapture \
      >"$log" 2>&1
  local status=$?
  set -e
  [[ $expected == green && $status -eq 0 ]] || [[ $expected == red && $status -ne 0 ]] || {
    tail -100 "$log" >&2
    echo "FAIL: $label status=$status expected=$expected" >&2
    exit 1
  }
  printf '%s status=%s expected=%s log=%s\n' "$label" "$status" "$expected" "$log"
}

run_case cardinality_registry_green_control green
SOURCE="$WORKTREE/apps/api/src/flow/command.rs"
perl -0pi -e 's/^        \("objects\.import_commit", Zero\),\n//m' "$SOURCE"
if grep -Fq '("objects.import_commit", Zero)' "$SOURCE"; then
  echo 'FAIL: cardinality declaration mutation did not apply' >&2
  exit 1
fi
run_case import_commit_declaration_removed red
printf 'PASS: green control passed and the missing production declaration was detected\n'
