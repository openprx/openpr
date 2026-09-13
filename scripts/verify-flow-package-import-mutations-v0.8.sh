#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CACHE_ROOT=/opt/worker/.cache/openpr-v08-package-import-mutations
WORKTREE="$CACHE_ROOT/worktree"
TARGET_DIR="$CACHE_ROOT/target"
LOG_DIR="$CACHE_ROOT/logs"
IMPORT_REL=apps/api/src/flow/package_import.rs
EXPORT_REL=apps/api/src/flow/export.rs

cleanup() {
  git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true
}
trap cleanup EXIT
mkdir -p "$CACHE_ROOT" "$TARGET_DIR" "$LOG_DIR"
cleanup
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE" HEAD >/dev/null
IMPORT_SOURCE="$WORKTREE/$IMPORT_REL"
EXPORT_SOURCE="$WORKTREE/$EXPORT_REL"

env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
  cargo build --manifest-path "$WORKTREE/Cargo.toml" -p collab-core \
    --bin collab-isolated-apply-worker >"$LOG_DIR/isolated-worker-build.log" 2>&1

restore_sources() {
  git -C "$WORKTREE" restore "$IMPORT_REL" "$EXPORT_REL"
}

run_case() {
  local label=$1
  local expected=$2
  local log="$LOG_DIR/$label.log"
  local status=0
  : >"$log"
  local test_name
  for test_name in \
    flow::package_import::tests::preview_writes_no_canonical_state_and_commit_remaps_exact_document_heads \
    flow::package_import::tests::promotion_fault_rolls_back_every_canonical_row_and_completion_event
  do
    set +e
    env -u RUST_TEST_THREADS \
      OPENPR_TEST_DATABASE_URL=postgresql://flowtest:flowtest@127.0.0.1:25433/postgres \
      CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
      cargo test --manifest-path "$WORKTREE/Cargo.toml" -p api --lib \
        "$test_name" -- --exact --nocapture >>"$log" 2>&1
    local test_status=$?
    set -e
    if [[ $test_status -ne 0 ]]; then
      status=$test_status
    fi
  done
  if [[ $expected == green && $status -ne 0 ]]; then
    tail -120 "$log" >&2
    echo "FAIL: green control exited $status" >&2
    exit 1
  fi
  if [[ $expected == red && $status -eq 0 ]]; then
    tail -120 "$log" >&2
    echo "FAIL: mutation $label was not detected" >&2
    exit 1
  fi
  printf '%s status=%s expected=%s log=%s\n' "$label" "$status" "$expected" "$log"
}

run_case package_import_green_control green

perl -0pi -e 's/ AND NOT flow_is_system_navigator_root\(fo\.object_type,fo\.parent_id,fo\.governance_metadata\)//g' "$EXPORT_SOURCE"
if grep -Fq 'AND NOT flow_is_system_navigator_root' "$EXPORT_SOURCE"; then
  echo 'FAIL: system navigator mutation did not apply' >&2
  exit 1
fi
run_case system_navigator_copied_into_package red
restore_sources

perl -0pi -e 's/engine\n            \.import_update\(&member_bytes\(package, path\)\?\)\n            \.map_err\(\|_\| ApiError::checksum_mismatch\("package update cannot be applied"\)\)\?;/let _ = member_bytes(package, path)?;/' "$IMPORT_SOURCE"
grep -Fq 'let _ = member_bytes(package, path)?;' "$IMPORT_SOURCE"
run_case accepted_tail_not_applied red
restore_sources

perl -0pi -e 's/(== Some\(&request\.preview_id\)\n            \{\n)                return Err\(ApiError::Internal\);/${1}                return Ok((created, reused));/' "$IMPORT_SOURCE"
grep -Fq 'return Ok((created, reused));' "$IMPORT_SOURCE"
run_case promotion_fault_swallowed red
restore_sources

perl -0pi -e 's/"flow\.import\.completed"/"flow.import.previewed"/' "$IMPORT_SOURCE"
run_case completion_event_misclassified red
restore_sources

perl -0pi -e 's/prior_targets\n                        \.get\(&key\)\n                        \.copied\(\)\n                        \.unwrap_or_else\(Uuid::new_v4\)/prior_targets.get(\&key).copied().unwrap_or(row.source_object_id)/' "$IMPORT_SOURCE"
grep -Fq 'unwrap_or(row.source_object_id)' "$IMPORT_SOURCE"
run_case source_object_id_reused red
restore_sources

perl -0pi -e 's/    enforce_admin\(principal\)\?;\n//' "$IMPORT_SOURCE"
run_case artifact_upload_admin_check_removed red

printf 'PASS: green control passed and 6/6 production-source import mutations were detected\n'
