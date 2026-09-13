#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CACHE_ROOT=/opt/worker/.cache/openpr-v08-fuzz-mutations
WORKTREE="$CACHE_ROOT/worktree"
TARGET_DIR=/opt/worker/.cache/openpr-v08-shared-target
LOG_DIR="$CACHE_ROOT/logs"
cleanup() { git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true; }
trap cleanup EXIT
mkdir -p "$CACHE_ROOT" "$TARGET_DIR" "$LOG_DIR"
cleanup
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE" HEAD >/dev/null

run_case() {
  local label=$1 expected=$2 target=$3 filter=$4
  local log="$LOG_DIR/$label.log"
  local -a command=(cargo test --manifest-path "$WORKTREE/Cargo.toml" -p collab-core)
  [[ $target == integration ]] && command+=(--test flow_v08_locked_corpus) || command+=(--lib "$filter" -- --exact)
  set +e
  env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" "${command[@]}" >"$log" 2>&1
  local status=$?
  set -e
  if [[ $expected == green && $status -ne 0 ]]; then tail -80 "$log" >&2; echo "FAIL: green $label exited $status" >&2; exit 1; fi
  if [[ $expected == red && $status -eq 0 ]]; then tail -80 "$log" >&2; echo "FAIL: mutation $label was not detected" >&2; exit 1; fi
  printf '%s status=%s expected=%s log=%s\n' "$label" "$status" "$expected" "$log"
}

LOCKED=locked_update_corpus_rejects_damage_and_converges_duplicates_and_reordering
CRC=isolation::host::tests::response_frame_rejects_a_corrupted_crc
run_case locked_corpus_green green integration "$LOCKED"
run_case response_crc_green green lib "$CRC"

ENGINE="$WORKTREE/crates/collab-core/src/engine.rs"
perl -0pi -e 's/changed: before != after/changed: true/' "$ENGINE"
grep -Fq 'changed: true' "$ENGINE"
run_case duplicate_replay_reports_changed red integration "$LOCKED"
git -C "$WORKTREE" restore crates/collab-core/src/engine.rs

HOST="$WORKTREE/crates/collab-core/src/isolation/host.rs"
perl -0pi -e 's/if computed_crc32 != declared_crc32 \{/if false {/' "$HOST"
grep -Fq 'if false {' "$HOST"
run_case response_crc_is_not_checked red lib "$CRC"

echo 'PASS: 2 green controls passed and 2/2 production-source mutations were detected'
