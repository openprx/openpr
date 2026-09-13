#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
: "${OPENPR_TEST_DATABASE_URL:?OPENPR_TEST_DATABASE_URL is required}"
CACHE_ROOT=/opt/worker/.cache/openpr-v08-rollback-mutations
WORKTREE="$CACHE_ROOT/worktree"
TARGET_DIR=/opt/worker/.cache/openpr-v08-shared-target
LOG_DIR="$CACHE_ROOT/logs"
cleanup() { git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true; }
trap cleanup EXIT
mkdir -p "$CACHE_ROOT" "$TARGET_DIR" "$LOG_DIR"
cleanup
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE" HEAD >/dev/null

run_case() {
  local label=$1 expected=$2 package=$3 test_name=$4
  local log="$LOG_DIR/$label.log"
  set +e
  local -a command=(cargo test --manifest-path "$WORKTREE/Cargo.toml" -p "$package")
  [[ $package == worker ]] && command+=(--bin worker) || command+=(--lib)
  command+=("$test_name" -- --exact --nocapture)
  env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" \
    CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
    "${command[@]}" >"$log" 2>&1
  local status=$?
  set -e
  if [[ $expected == green && $status -ne 0 ]]; then tail -80 "$log" >&2; echo "FAIL: green $label exited $status" >&2; exit 1; fi
  if [[ $expected == red && $status -eq 0 ]]; then tail -80 "$log" >&2; echo "FAIL: mutation $label was not detected" >&2; exit 1; fi
  grep -Eq '^test result: (ok|FAILED)\.' "$log" || { tail -80 "$log" >&2; echo "FAIL: $label did not execute libtest" >&2; exit 1; }
  printf '%s status=%s expected=%s log=%s\n' "$label" "$status" "$expected" "$log"
}

COMPACTION=flow::compaction::tests::flow_compaction_worker_preserves_exact_document_fingerprint
RETENTION=flow::retention::tests::worker_deletes_only_expired_full_access_archives_not_edit_tier_soft_archives
IMPORT=flow::package_import::tests::flow_package_import_preview_writes_no_canonical_state_and_commit_remaps_exact_document_heads
run_case compaction_pause_green green worker "$COMPACTION"
run_case retention_pause_green green worker "$RETENTION"
run_case import_pause_green green api "$IMPORT"

COMPACTION_SOURCE="$WORKTREE/apps/worker/src/flow/compaction.rs"
perl -0pi -e 's/if api::flow::rollback::control\(db\)\.await\?\.compaction_paused \{/if false {/' "$COMPACTION_SOURCE"
grep -Fq 'if false {' "$COMPACTION_SOURCE"
run_case compaction_pause_bypassed red worker "$COMPACTION"
git -C "$WORKTREE" restore apps/worker/src/flow/compaction.rs

RETENTION_SOURCE="$WORKTREE/apps/worker/src/flow/retention.rs"
perl -0pi -e 's/if api::flow::rollback::control\(db\)\.await\?\.retention_paused \{/if false {/' "$RETENTION_SOURCE"
grep -Fq 'if false {' "$RETENTION_SOURCE"
run_case retention_pause_bypassed red worker "$RETENTION"
git -C "$WORKTREE" restore apps/worker/src/flow/retention.rs

IMPORT_SOURCE="$WORKTREE/apps/api/src/flow/package_import.rs"
perl -0pi -e 's/if super::rollback::control\(db\)\.await\?\.import_promotion_paused \{/if false {/' "$IMPORT_SOURCE"
grep -Fq 'if false {' "$IMPORT_SOURCE"
run_case import_promotion_pause_bypassed red api "$IMPORT"

echo 'PASS: 3 green controls passed and 3/3 production-source rollback mutations were detected'
