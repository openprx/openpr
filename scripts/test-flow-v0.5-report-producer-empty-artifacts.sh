#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT="$ROOT_DIR/scripts/report-flow-v0.5-json.sh"
JQ_LIB="$ROOT_DIR/scripts/lib"

declare -A expected_gate_counts=(
  [relation_policy_result]=2
  [search_contract_result]=3
  [mcp_cli_equivalence_result]=3
)

for command_key in relation_policy_verify search_contract_verify mcp_cli_equivalence_verify; do
  count="$(grep -cE "^[[:space:]]{4}${command_key}\)" "$REPORT")"
  [[ "$count" -eq 1 ]] || {
    echo "FAIL: report invocation mapping for $command_key is not unique" >&2
    exit 1
  }
done

for artifact in relation_policy_result search_contract_result mcp_cli_equivalence_result; do
  input="$(jq -cn --arg path "evidence/v0.5/${artifact//_/-}.json" '{
    path:$path,
    exists:false,
    valid_json:false,
    sha256:null,
    document:{},
    producer_status:"available",
    producer_command:"mutation: producer exited without writing its artifact",
    producer_executed_count:1,
    producer_execution_status:"passed"
  }')"
  state="$(jq -cn -L "$JQ_LIB" --arg artifact "$artifact" --argjson input "$input" '
    include "flow_gate_v0_5_receipt_state";
    flow_artifact_state($artifact; $input; "mutation-head")
  ')"

  jq -e --argjson expected "${expected_gate_counts[$artifact]}" '
    .status == "artifact_missing_after_execution"
    and .producer_executed_count == 1
    and (.gate_verdicts | length) == $expected
    and all(.gate_verdicts[]; . == "failed")
    and (any(.gate_verdicts[]; . == "not_covered") | not)
  ' >/dev/null <<<"$state" || {
    echo "FAIL: $artifact did not fail closed after an executed producer returned no artifact" >&2
    jq . <<<"$state" >&2
    exit 1
  }

  echo "PASS: $artifact empty-after-execution => all ${expected_gate_counts[$artifact]} owned gates failed"
done
