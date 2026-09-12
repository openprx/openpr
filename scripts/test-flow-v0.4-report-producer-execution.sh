#!/usr/bin/env bash
set -euo pipefail

# Fast wiring and mutation checks for the v0.4 report producer ledger. This
# does not run product commands; the formal report run supplies the dynamic
# evidence after this structural counterexample passes.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT="$ROOT_DIR/scripts/report-flow-v0.4-json.sh"
GATE_YAML="/opt/working/sylvode-flow/gates/v0.4-gate.yaml"
SCHEMA="$ROOT_DIR/docs/schemas/sylvode-flow-gate-v0.4.schema.json"
TMP_DIR="$(mktemp -d /tmp/openpr-flow-v04-producers.XXXXXX)"
trap 'rm -rf "$TMP_DIR"' EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

producer_keys() {
  awk '
    $0 == "required_commands:" { inside=1; next }
    inside && /^[^[:space:]#]/ { exit }
    inside && /^  [A-Za-z0-9_]+:/ {
      line=substr($0,3); sub(/:.*/, "", line)
      if (line != "report" && line != "verify" && line != "gate" && line != "manual_signoff") print line
    }
  ' "$GATE_YAML"
}

audit_report_source() {
  local source="$1" failed=0 key count
  while IFS= read -r key; do
    count="$(grep -cE "^[[:space:]]*run_step required\\.${key}([[:space:]]|$)" "$source" || true)"
    if [[ "$count" -ne 1 ]]; then
      echo "producer $key has $count report invocations; expected 1" >&2
      failed=1
    fi
  done < <(producer_keys)
  [[ "$failed" -eq 0 ]]
}

audit_report_source "$REPORT" || fail "current report does not invoke every declared producer exactly once"
echo "PASS: current report invokes every declared v0.4 producer exactly once"

MUTANT="$TMP_DIR/report-missing-cross-workspace.sh"
awk '!/^[[:space:]]*run_step required\.cross_workspace_verify([[:space:]]|$)/' "$REPORT" > "$MUTANT"
if audit_report_source "$MUTANT" >"$TMP_DIR/mutation.out" 2>&1; then
  fail "deleting the cross_workspace producer invocation did not turn the wiring audit red"
fi
grep -Fq "producer cross_workspace_verify has 0 report invocations; expected 1" "$TMP_DIR/mutation.out" \
  || fail "mutation failed for an unexpected reason"
echo "PASS: deleting an actual producer invocation is detected (mutation red)"

jq -e '
  (.["$defs"].check_result.required | index("executed_count") != null)
  and (.["$defs"].required_command_result.required | index("executed_count") != null)
' "$SCHEMA" >/dev/null || fail "schema does not require executed_count on checks and required commands"
echo "PASS: the v0.4 receipt schema requires executed_count on both execution ledgers"
