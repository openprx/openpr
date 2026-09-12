#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_ROOT="${REPO_ROOT}/evidence/v0.7"
while (($#)); do
  case "$1" in
    --repo-root) REPO_ROOT="${2:?--repo-root requires a value}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a value}"; shift 2 ;;
    --json) shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

[[ -n "${OPENPR_TEST_DATABASE_URL:-}" ]] || {
  echo "OPENPR_TEST_DATABASE_URL is required" >&2
  exit 2
}

mkdir -p "$EVIDENCE_ROOT"
CACHE_ROOT="/opt/worker/.cache"
mkdir -p "$CACHE_ROOT"
TMP_DIR="$(mktemp -d "$CACHE_ROOT/v07-migration-replay.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT
export CARGO_BUILD_JOBS=4

TEST_NAME="migration_runner_database_tests::flow_forms_bridge_migration_is_replay_safe"
GREEN_LOG="$EVIDENCE_ROOT/migration-replay-green.log"
RED_LOG="$EVIDENCE_ROOT/migration-replay-no-if-not-exists-red.log"

set +e
(cd "$REPO_ROOT" && env -u OPENPR_TEST_FLOW_BRIDGE_MIGRATION_PATH \
  cargo test -p api --bin api "$TEST_NAME" -- --exact --nocapture) >"$GREEN_LOG" 2>&1
GREEN_EXIT=$?
set -e

MUTANT="$TMP_DIR/0062_flow_forms_bridge.sql"
sed '0,/ADD COLUMN IF NOT EXISTS/{s/ADD COLUMN IF NOT EXISTS/ADD COLUMN/}' \
  "$REPO_ROOT/migrations/0062_flow_forms_bridge.sql" >"$MUTANT"
grep -q 'ADD COLUMN transport_surface' "$MUTANT"

set +e
(cd "$REPO_ROOT" && OPENPR_TEST_FLOW_BRIDGE_MIGRATION_PATH="$MUTANT" \
  cargo test -p api --bin api "$TEST_NAME" -- --exact --nocapture) >"$RED_LOG" 2>&1
RED_EXIT=$?
set -e

passed=true
[[ "$GREEN_EXIT" == 0 && "$RED_EXIT" != 0 ]] || passed=false
grep -q '^test result: ok\.' "$GREEN_LOG" || passed=false
grep -q '^test result: FAILED\.' "$RED_LOG" || passed=false
grep -q 'code: "42701".*transport_surface.*already exists' "$RED_LOG" || passed=false

jq -n \
  --arg schema_version 'sylvode.flow.migration-replay-result.v1' \
  --arg source_head "$(git -C "$REPO_ROOT" rev-parse HEAD)" \
  --argjson passed "$passed" \
  --argjson green_exit "$GREEN_EXIT" \
  --argjson red_exit "$RED_EXIT" \
  '{schema_version:$schema_version,source_head:$source_head,executed_count:2,passed:$passed,
    cases:[
      {id:"flow_forms_bridge_migration_double_run",expected:"green",exit_code:$green_exit},
      {id:"flow_forms_bridge_migration_without_if_not_exists",expected:"red",exit_code:$red_exit}
    ]}' | tee "$EVIDENCE_ROOT/migration-replay-result.json"

[[ "$passed" == true ]]
