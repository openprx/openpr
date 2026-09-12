#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
: "${OPENPR_TEST_DATABASE_URL:?OPENPR_TEST_DATABASE_URL is required}"

CACHE_ROOT=/opt/worker/.cache/openpr-v08-search-rebuild-mutations
WORKTREE="$CACHE_ROOT/worktree"
TARGET_DIR="$CACHE_ROOT/target"
LOG_DIR="$CACHE_ROOT/logs"
TEST_NAME=flow::maintenance::database_tests::dry_run_changes_nothing_and_execute_restores_the_exact_canonical_projection

cleanup() {
  git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir -p "$CACHE_ROOT" "$TARGET_DIR" "$LOG_DIR"
cleanup
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE" HEAD >/dev/null

run_case() {
  local label=$1
  local expected=$2
  local log="$LOG_DIR/$label.log"
  set +e
  env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" \
    CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
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
SOURCE="$WORKTREE/apps/api/src/flow/maintenance.rs"

perl -0pi -e 's/(pub async fn rebuild_search.*?)(if execute && changed \{)/$1if false {/s' "$SOURCE"
grep -A90 -F 'pub async fn rebuild_search' "$SOURCE" | grep -Fq 'if false {'
run_case search_execute_becomes_noop red
perl -0pi -e 's/(pub async fn rebuild_search.*?)(if false \{)/$1if execute \&\& changed {/s' "$SOURCE"

perl -0pi -e 's/(pub async fn rebuild_search.*?)(expected != source\.document_seq)/$1false/s' "$SOURCE"
grep -A30 -F 'pub async fn rebuild_search' "$SOURCE" | grep -Fq '&& false'
run_case stale_projection_head_accepted red
perl -0pi -e 's/(pub async fn rebuild_search.*?)(false)/$1expected != source.document_seq/s' "$SOURCE"

perl -0pi -e 's/(pub async fn rebuild_search.*?)(title: source\.title\.clone\(\),)/$1title: "stale search".to_string(),/s' "$SOURCE"
grep -A50 -F 'pub async fn rebuild_search' "$SOURCE" | grep -Fq 'title: "stale search".to_string(),'
run_case accepted_projection_title_ignored red

printf 'PASS: green control passed and 3/3 production-source mutations were detected\n'
