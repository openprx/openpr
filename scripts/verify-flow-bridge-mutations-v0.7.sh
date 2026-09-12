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

mkdir -p "$EVIDENCE_ROOT"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT
export CARGO_BUILD_JOBS=4

run_case() {
  local name="$1" test_name="$2" mutation="${3:-}"
  local log="$TMP_DIR/$name.log"
  set +e
  if [[ -n "$mutation" ]]; then
    (cd "$REPO_ROOT" && OPENPR_FLOW_TEST_BRIDGE_MUTATION="$mutation" \
      cargo test -p api "$test_name" -- --exact --nocapture) >"$log" 2>&1
  else
    (cd "$REPO_ROOT" && env -u OPENPR_FLOW_TEST_BRIDGE_MUTATION \
      cargo test -p api "$test_name" -- --exact --nocapture) >"$log" 2>&1
  fi
  local exit_code=$?
  set -e
  cp "$log" "$EVIDENCE_ROOT/$name.log"
  printf '%s' "$exit_code"
}

BR4_TEST="flow::bridge::tests::unconfigured_forms_are_read_only_and_honestly_labelled"
BR4_GREEN="$(run_case br4-green "$BR4_TEST")"
BR4_RED="$(run_case br4-default-allow-red "$BR4_TEST" br4_default_allow)"

passed=true
[[ "$BR4_GREEN" == 0 ]] || passed=false
[[ "$BR4_RED" != 0 ]] || passed=false
grep -q 'test result: FAILED' "$EVIDENCE_ROOT/br4-default-allow-red.log" || passed=false

jq -n \
  --arg schema_version 'sylvode.flow.bridge-mutation-result.v1' \
  --arg source_head "$(git -C "$REPO_ROOT" rev-parse HEAD)" \
  --argjson passed "$passed" \
  --argjson br4_green "$BR4_GREEN" --argjson br4_red "$BR4_RED" \
  '{schema_version:$schema_version,source_head:$source_head,executed_count:2,passed:$passed,
    cases:[
      {id:"br4_production_green",expected:"green",exit_code:$br4_green},
      {id:"br4_forms_default_allow_mutation",expected:"red",exit_code:$br4_red}
    ]}' | tee "$EVIDENCE_ROOT/bridge-mutation-result.json"

[[ "$passed" == true ]]
