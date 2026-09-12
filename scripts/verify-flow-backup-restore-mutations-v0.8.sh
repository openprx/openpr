#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
: "${OPENPR_TEST_DATABASE_URL:?OPENPR_TEST_DATABASE_URL is required}"
: "${OPENPR_BACKUP_SOURCE_DATABASE_URL:?OPENPR_BACKUP_SOURCE_DATABASE_URL is required}"
: "${OPENPR_BACKUP_RESTORE_ADMIN_URL:?OPENPR_BACKUP_RESTORE_ADMIN_URL is required}"

CACHE_ROOT=/opt/worker/.cache/openpr-v08-backup-restore-mutations
WORKTREE="$CACHE_ROOT/worktree"
LOG_DIR="$CACHE_ROOT/logs"
CHECKSUM_TEST=flow::collab::snapshot::database_tests::bootstrap_rejects_snapshot_bytes_that_no_longer_match_the_persisted_checksum

cleanup() {
  git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir -p "$CACHE_ROOT" "$LOG_DIR"
cleanup
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE" HEAD >/dev/null

run_test_case() {
  local label=$1
  local expected=$2
  local log="$LOG_DIR/$label.log"
  set +e
  env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" CARGO_BUILD_JOBS=4 \
    cargo test --manifest-path "$WORKTREE/Cargo.toml" -p api --lib "$CHECKSUM_TEST" -- --exact --nocapture \
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

run_restore_case() {
  local label=$1
  local expected=$2
  local log="$LOG_DIR/$label.log"
  set +e
  env -u RUST_TEST_THREADS \
    OPENPR_BACKUP_SOURCE_DATABASE_URL="$OPENPR_BACKUP_SOURCE_DATABASE_URL" \
    OPENPR_BACKUP_RESTORE_ADMIN_URL="$OPENPR_BACKUP_RESTORE_ADMIN_URL" \
    OPENPR_BACKUP_RESTORE_DATABASE_NAME="v08_restore_${label}" \
    CARGO_BUILD_JOBS=4 \
    "$WORKTREE/scripts/verify-flow-backup-restore-v0.8.sh" "$CACHE_ROOT/$label.json" \
      >"$log" 2>&1
  local status=$?
  set -e
  if [[ $expected == green && $status -ne 0 ]]; then
    tail -80 "$log" >&2
    echo "FAIL: restore green control exited $status" >&2
    exit 1
  fi
  if [[ $expected == red && $status -eq 0 ]]; then
    tail -80 "$log" >&2
    echo "FAIL: mutation $label was not detected" >&2
    exit 1
  fi
  printf '%s status=%s expected=%s log=%s\n' "$label" "$status" "$expected" "$log"
}

run_test_case checksum_green_control green

BOOTSTRAP_SOURCE="$WORKTREE/apps/api/src/flow/collab/bootstrap.rs"
perl -0pi -e 's/if content_hash\(&doc\.snapshot\) != doc\.snapshot_checksum \{/if false {/' "$BOOTSTRAP_SOURCE"
grep -Fq 'if false {' "$BOOTSTRAP_SOURCE"
run_test_case corrupted_snapshot_accepted red

# Restore the checksum check so the independent dump-completeness mutation reaches its own
# divergence rather than inheriting the first mutant.
perl -0pi -e 's/if false \{/if content_hash\(&doc.snapshot\) != doc.snapshot_checksum {/' "$BOOTSTRAP_SOURCE"

run_restore_case restore_green_control green

RESTORE_SOURCE="$WORKTREE/scripts/verify-flow-backup-restore-v0.8.sh"
perl -0pi -e 's/--format=plain --no-owner --no-acl \|/--format=plain --no-owner --no-acl --exclude-table-data=collab_updates |/' "$RESTORE_SOURCE"
grep -Fq -- '--exclude-table-data=collab_updates' "$RESTORE_SOURCE"
run_restore_case retained_updates_omitted_from_backup red

printf 'PASS: 2 green controls passed and 2/2 production-source mutations were detected\n'
