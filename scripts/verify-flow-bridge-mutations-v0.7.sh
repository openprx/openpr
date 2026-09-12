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
GUEST_TEST="flow::bridge::tests::guests_and_flow_denied_principals_get_no_reference_shape"
BR4_GREEN="$(run_case br4-green "$BR4_TEST")"
BR4_RED="$(run_case br4-default-allow-red "$BR4_TEST" br4_default_allow)"
GUEST_GREEN="$(run_case guest-green "$GUEST_TEST")"
GUEST_RED="$(run_case guest-as-member-red "$GUEST_TEST" guest_as_member)"

passed=true
[[ "$BR4_GREEN" == 0 && "$GUEST_GREEN" == 0 ]] || passed=false
[[ "$BR4_RED" != 0 && "$GUEST_RED" != 0 ]] || passed=false
grep -q 'test result: FAILED' "$EVIDENCE_ROOT/br4-default-allow-red.log" || passed=false
grep -q 'test result: FAILED' "$EVIDENCE_ROOT/guest-as-member-red.log" || passed=false

jq -n \
  --arg schema_version 'sylvode.flow.bridge-mutation-result.v1' \
  --arg source_head "$(git -C "$REPO_ROOT" rev-parse HEAD)" \
  --argjson passed "$passed" \
  --argjson br4_green "$BR4_GREEN" --argjson br4_red "$BR4_RED" \
  --argjson guest_green "$GUEST_GREEN" --argjson guest_red "$GUEST_RED" \
  '{schema_version:$schema_version,source_head:$source_head,executed_count:4,passed:$passed,
    cases:[
      {id:"br4_production_green",expected:"green",exit_code:$br4_green},
      {id:"br4_forms_default_allow_mutation",expected:"red",exit_code:$br4_red},
      {id:"guest_production_green",expected:"green",exit_code:$guest_green},
      {id:"guest_member_branch_mutation",expected:"red",exit_code:$guest_red}
    ]}' | tee "$EVIDENCE_ROOT/bridge-mutation-result.json"

[[ "$passed" == true ]]
