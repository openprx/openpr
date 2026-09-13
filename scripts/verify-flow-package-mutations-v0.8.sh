#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CACHE_ROOT=/opt/worker/.cache/openpr-v08-package-mutations
WORKTREE="$CACHE_ROOT/worktree"
TARGET_DIR=/opt/worker/.cache/openpr-v08-shared-target
LOG_DIR="$CACHE_ROOT/logs"
SOURCE_REL=apps/api/src/flow/package.rs

cleanup() {
  git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir -p "$CACHE_ROOT" "$TARGET_DIR" "$LOG_DIR"
cleanup
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE" HEAD >/dev/null
SOURCE="$WORKTREE/$SOURCE_REL"

run_case() {
  local label=$1
  local expected=$2
  local test_name=$3
  local exact=$4
  local log="$LOG_DIR/$label.log"
  local -a test_args
  test_args=("$test_name")
  if [[ $exact == exact ]]; then
    test_args+=(-- --exact --nocapture)
  else
    test_args+=(-- --nocapture)
  fi
  set +e
  env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
    cargo test --manifest-path "$WORKTREE/Cargo.toml" -p api --lib "${test_args[@]}" >"$log" 2>&1
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

restore_source() {
  git -C "$WORKTREE" restore "$SOURCE_REL"
}

run_case package_green_control green flow::package::tests filter

HASH_TEST=flow::package::tests::package_hash_member_hash_and_checksum_file_are_independent_fail_closed_checks
RAW_TEST=flow::package::tests::raw_header_validator_itself_rejects_flag_disagreement_forbidden_descriptor_and_non_zip64
PATH_TEST=flow::package::tests::input_path_validator_rejects_casefold_collision_before_manifest_layout
FORMAT_TEST=flow::package::tests::noncanonical_manifest_and_unsupported_compatibility_matrix_are_rejected
COUNT_TEST=flow::package::tests::nfc_and_zip64_entry_count_are_checked_before_archive_parsing

perl -0pi -e 's/expected != package_sha256/false/' "$SOURCE"
grep -Fq 'expected_package_sha256.is_some_and(|expected| false)' "$SOURCE"
run_case archive_hash_check_bypassed red "$HASH_TEST" exact
restore_source

perl -0pi -e 's/\|\| \*sha256 != member\.sha256/|| false/' "$SOURCE"
grep -Fq 'if *bytes != member.bytes || false' "$SOURCE"
run_case member_hash_check_bypassed red "$HASH_TEST" exact
restore_source

perl -0pi -e 's/if checksum_bytes != expected_checksums/if false/' "$SOURCE"
grep -Fq 'if false {' "$SOURCE"
run_case checksum_file_check_bypassed red "$HASH_TEST" exact
restore_source

perl -0pi -e 's/if canonical != manifest_bytes/if false/' "$SOURCE"
grep -Fq 'if false {' "$SOURCE"
run_case canonical_manifest_check_bypassed red "$FORMAT_TEST" exact
restore_source

perl -0pi -e 's/ \|\| !folded\.insert\(input\.path\.case_fold\(\)\.collect::<String>\(\)\)//' "$SOURCE"
if grep -Fq '|| !folded.insert(input.path.case_fold().collect::<String>())' "$SOURCE"; then
  echo 'FAIL: input casefold mutation did not apply' >&2
  exit 1
fi
run_case unicode_casefold_collision_accepted red "$PATH_TEST" exact
restore_source

perl -0pi -e 's/local_flags != central_flags/local_flags == central_flags/' "$SOURCE"
grep -Fq 'if local_flags == central_flags' "$SOURCE"
run_case local_central_flag_disagreement_accepted red "$RAW_TEST" exact
restore_source

perl -0pi -e 's/local_flags & \(FLAG_ENCRYPTED \| FLAG_DATA_DESCRIPTOR\) != 0/false/' "$SOURCE"
grep -Fq '|| false' "$SOURCE"
run_case data_descriptor_flag_accepted red "$RAW_TEST" exact
restore_source

perl -0pi -e 's/if !contains_extra_field\(local_extra, ZIP64_EXTRA_ID\)\?/if false/' "$SOURCE"
grep -Fq 'if false {' "$SOURCE"
run_case non_zip64_entry_accepted red "$RAW_TEST" exact
restore_source

perl -0pi -e 's/if declared_entries > IMPORT_ENTRY_COUNT_MAX/if false/' "$SOURCE"
grep -Fq 'if false {' "$SOURCE"
run_case entry_count_preflight_bypassed red "$COUNT_TEST" exact
restore_source

perl -0pi -e 's/\|\| manifest\.engine\.wire_format_version != ENGINE_WIRE_FORMAT_VERSION/|| false/' "$SOURCE"
grep -Fq '|| false' "$SOURCE"
run_case untested_engine_wire_version_accepted red "$FORMAT_TEST" exact

printf 'PASS: green control passed and 10/10 production-source mutations were detected\n'
