#!/usr/bin/env bash
set -euo pipefail

# Final v0.6 gate: independent receipt verification plus the one manual row.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT=""
GATE_YAML=""
GATE_RESULT=""
ALLOW_PENDING=0
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/gate-flow-v0.6.sh --json [OPTIONS]

Options:
  --allow-pending       Accept only a candidate-ready receipt whose sole manual row is pending
  --gate-result PATH    Default: <evidence-root>/gate-result.json
  --evidence-root DIR   Default: <contracts-root>/evidence/v0.6
  --contracts-root DIR  Default: /opt/working/sylvode-flow
  --repo-root DIR       Default: this checkout
  --gate-yaml PATH      Default: <contracts-root>/gates/v0.6-gate.yaml
  --json                Required
  -h, --help            Show help

Exit: 0 accepted, 1 blocked/pending, 2 usage/tool/malformed.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --allow-pending) ALLOW_PENDING=1; shift ;;
    --gate-result) GATE_RESULT="${2:?--gate-result requires a path}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a directory}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a directory}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a directory}"; shift 2 ;;
    --gate-yaml) GATE_YAML="${2:?--gate-yaml requires a path}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ $JSON_MODE -eq 1 ]] || { echo "FAIL: --json is required" >&2; exit 2; }
command -v jq >/dev/null 2>&1 || { echo "FAIL: jq is required" >&2; exit 2; }
[[ -n "$EVIDENCE_ROOT" ]] || EVIDENCE_ROOT="$CONTRACTS_ROOT/evidence/v0.6"
[[ -n "$GATE_YAML" ]] || GATE_YAML="$CONTRACTS_ROOT/gates/v0.6-gate.yaml"
[[ -n "$GATE_RESULT" ]] || GATE_RESULT="$EVIDENCE_ROOT/gate-result.json"
[[ -f "$GATE_RESULT" ]] || { echo "FAIL: gate result missing: $GATE_RESULT" >&2; exit 2; }

set +e
VERIFY_OUTPUT="$("$ROOT_DIR/scripts/verify-flow-v0.6-json.sh" "$GATE_RESULT" \
  --evidence-root "$EVIDENCE_ROOT" --contracts-root "$CONTRACTS_ROOT" \
  --repo-root "$REPO_ROOT" --gate-yaml "$GATE_YAML" --json 2>&1)"
VERIFY_EXIT=$?
set -e
if [[ $VERIFY_EXIT -ge 2 ]] || ! jq -e 'type=="object" and has("receipt_consistent")' >/dev/null 2>&1 <<<"$VERIFY_OUTPUT"; then
  printf '%s\n' "$VERIFY_OUTPUT" >&2
  echo "FAIL: v0.6 verifier returned malformed/tool error" >&2
  exit 2
fi

CONSISTENT="$(jq -r '.receipt_consistent' <<<"$VERIFY_OUTPUT")"
CANDIDATE="$(jq -r '.candidate_ready' <<<"$VERIFY_OUTPUT")"
MANUAL="$(jq -r '.manual_signoffs.field_secrecy_denial.status // "missing"' "$GATE_RESULT")"
PASSED=false
REASON=""
if [[ "$CONSISTENT" != true ]]; then
  REASON="receipt drift"
elif [[ "$CANDIDATE" != true ]]; then
  REASON="candidate prerequisites or automated gates are blocked"
elif [[ "$MANUAL" == passed ]]; then
  PASSED=true
  REASON="candidate ready and field_secrecy_denial signed passed"
elif [[ $ALLOW_PENDING -eq 1 && "$MANUAL" == pending ]]; then
  PASSED=true
  REASON="candidate ready; sole manual row pending under --allow-pending"
elif [[ "$MANUAL" == pending ]]; then
  REASON="field_secrecy_denial manual signoff is pending"
elif [[ "$MANUAL" == failed || "$MANUAL" == needs_rework ]]; then
  REASON="field_secrecy_denial manual signoff blocks acceptance"
else
  echo "FAIL: invalid manual signoff status: $MANUAL" >&2
  exit 2
fi

jq -cn --argjson verifier "$VERIFY_OUTPUT" --argjson verifier_exit "$VERIFY_EXIT" \
  --arg manual "$MANUAL" --argjson allow_pending "$([[ $ALLOW_PENDING -eq 1 ]] && echo true || echo false)" \
  --argjson gate_passed "$PASSED" --arg reason "$REASON" \
  '{schema_version:"sylvode.flow.gate.v1",verifier_exit_code:$verifier_exit,verification:$verifier,manual_signoff:$manual,allow_pending:$allow_pending,gate_passed:$gate_passed,reason:$reason}'
[[ "$PASSED" == true ]]
