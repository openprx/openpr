#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
: "${OPENPR_TEST_DATABASE_URL:?OPENPR_TEST_DATABASE_URL is required}"
CACHE_ROOT=/opt/worker/.cache/openpr-v08-import-limit-mutations
WORKTREE="$CACHE_ROOT/worktree"
TARGET_DIR=/opt/worker/.cache/openpr-v08-shared-target
LOG_DIR="$CACHE_ROOT/logs"
IMPORT_REL=apps/api/src/flow/import.rs
PACKAGE_REL=apps/api/src/flow/package.rs
TEST_NAME=routes::flow::flow_database_tests::flow_package_import_wire_limits_accept_exact_boundary_and_reject_plus_one_with_zero_writes

cleanup() {
  git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true
}
trap cleanup EXIT
mkdir -p "$CACHE_ROOT" "$TARGET_DIR" "$LOG_DIR"
cleanup
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE" HEAD >/dev/null
IMPORT_SOURCE="$WORKTREE/$IMPORT_REL"
PACKAGE_SOURCE="$WORKTREE/$PACKAGE_REL"

run_case() {
  local label=$1 expected=$2 log="$LOG_DIR/$1.log"
  set +e
  env -u RUST_TEST_THREADS \
    OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
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

restore_sources() {
  git -C "$WORKTREE" restore "$IMPORT_REL" "$PACKAGE_REL"
}

run_case import_limits_green_control green

perl -0pi -e 's/effective_import_limits\(\)\.archive_bytes/u64::MAX/' "$IMPORT_SOURCE"
if grep -Fq 'effective_import_limits().archive_bytes' "$IMPORT_SOURCE"; then
  echo 'FAIL: archive ceiling mutation did not apply' >&2
  exit 1
fi
run_case archive_byte_ceiling_bypassed red
restore_sources

perl -0pi -e 's/effective_import_limits\(\)\.expanded_bytes/u64::MAX/' "$IMPORT_SOURCE"
if grep -Fq 'effective_import_limits().expanded_bytes' "$IMPORT_SOURCE"; then
  echo 'FAIL: expanded ceiling mutation did not apply' >&2
  exit 1
fi
run_case expanded_byte_ceiling_bypassed red
restore_sources

perl -0pi -e 's/let entry_count_max = effective_import_limits\(\)\.entry_count;/let entry_count_max = u64::MAX;/' "$PACKAGE_SOURCE"
grep -Fq 'let entry_count_max = u64::MAX;' "$PACKAGE_SOURCE"
perl -0pi -e 's/effective_import_limits\(\)\.entry_count/u64::MAX/' "$IMPORT_SOURCE"
if grep -Fq 'effective_import_limits().entry_count' "$IMPORT_SOURCE"; then
  echo 'FAIL: streaming entry-count mutation did not apply' >&2
  exit 1
fi
run_case entry_count_preflight_bypassed red
restore_sources

perl -0pi -e 's/let limit = effective_import_limits\(\)\.compression_ratio;/let limit = u64::MAX;/' "$IMPORT_SOURCE"
grep -Fq 'let limit = u64::MAX;' "$IMPORT_SOURCE"
run_case compression_ratio_ceiling_bypassed red

printf 'PASS: green control passed and 4/4 wire limit production-source mutations were detected\n'
