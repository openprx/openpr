#!/usr/bin/env bash
set -euo pipefail

# v0.5 final gate. Automated truth comes only from the independent verifier.
# Strict mode requires all four manual rows passed. --allow-pending permits
# only the handoff state where automation is green and every manual row is
# either passed or pending; failed/needs_rework is never accepted.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT=""
REPO_ROOT="$ROOT_DIR"
GATE_YAML=""
GATE_RESULT_PATH=""
ALLOW_PENDING=0
JSON_MODE=0
MANUAL_KEYS="audit_causation multi_user offline_recovery permission_revocation"

usage() {
  cat <<'EOF'
Usage: scripts/gate-flow-v0.5.sh --json [OPTIONS]

Options:
  --allow-pending        Exit 0 when automation is green and manual rows are
                         only passed/pending. Never accepts failed/rework.
  --gate-result PATH     Default: <evidence-root>/gate-result.json
  --evidence-root DIR    Default: <contracts-root>/evidence/v0.5
  --contracts-root DIR   Default: /opt/working/sylvode-flow
  --repo-root DIR        Default: this checkout
  --gate-yaml PATH       Default: <contracts-root>/gates/v0.5-gate.yaml
  --json                 Required; emit JSON.
  -h, --help             Show help.

Exit codes: 0 satisfied, 1 failed/pending, 2 usage/tool/malformed.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --allow-pending) ALLOW_PENDING=1; shift ;;
    --gate-result) GATE_RESULT_PATH="${2:?--gate-result requires PATH}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires DIR}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires DIR}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires DIR}"; shift 2 ;;
    --gate-yaml) GATE_YAML="${2:?--gate-yaml requires PATH}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ $JSON_MODE -eq 1 ]] || { echo "FAIL: --json is required" >&2; exit 2; }
command -v jq >/dev/null 2>&1 || { echo "FAIL: missing required command: jq" >&2; exit 2; }
[[ -n "$EVIDENCE_ROOT" ]] || EVIDENCE_ROOT="$CONTRACTS_ROOT/evidence/v0.5"
[[ -n "$GATE_YAML" ]] || GATE_YAML="$CONTRACTS_ROOT/gates/v0.5-gate.yaml"
[[ -n "$GATE_RESULT_PATH" ]] || GATE_RESULT_PATH="$EVIDENCE_ROOT/gate-result.json"
VERIFY_SCRIPT="$ROOT_DIR/scripts/verify-flow-v0.5-json.sh"
if [[ ! -x "$VERIFY_SCRIPT" || ! -f "$GATE_RESULT_PATH" ]]; then
  echo "FAIL: verifier is not executable or gate-result is missing" >&2
  exit 2
fi

set +e
VERIFY_OUTPUT="$("$VERIFY_SCRIPT" "$GATE_RESULT_PATH" --evidence-root "$EVIDENCE_ROOT" \
  --contracts-root "$CONTRACTS_ROOT" --repo-root "$REPO_ROOT" --gate-yaml "$GATE_YAML" --json 2>&1)"
VERIFY_EXIT=$?
set -e
if [[ $VERIFY_EXIT -ge 2 ]]; then
  printf '%s\n' "$VERIFY_OUTPUT" >&2
  echo "FAIL: verifier returned malformed/tool error (exit=$VERIFY_EXIT)" >&2
  exit 2
fi
AUTOMATION_PASSED=$([[ $VERIFY_EXIT -eq 0 ]] && echo true || echo false)

MANUAL_SUMMARY='{}'
MANUAL_ALL_PASSED=true
MANUAL_ANY_PENDING=false
MANUAL_ANY_BLOCKING=false
for key in $MANUAL_KEYS; do
  status="$(jq -r --arg k "$key" '.manual_signoffs[$k].status // "missing"' "$GATE_RESULT_PATH")"
  case "$status" in
    passed) ;;
    pending) MANUAL_ALL_PASSED=false; MANUAL_ANY_PENDING=true ;;
    failed|needs_rework) MANUAL_ALL_PASSED=false; MANUAL_ANY_BLOCKING=true ;;
    *) echo "FAIL: invalid or missing manual_signoffs.$key.status: $status" >&2; exit 2 ;;
  esac
  MANUAL_SUMMARY="$(jq -c --arg k "$key" --arg v "$status" '.[$k]=$v' <<<"$MANUAL_SUMMARY")"
done

GATE_PASSED=false
REASON=""
if [[ "$AUTOMATION_PASSED" != true ]]; then
  REASON="automation not green: verifier exit $VERIFY_EXIT"
elif [[ "$MANUAL_ANY_BLOCKING" == true ]]; then
  REASON="manual signoff failed or needs_rework"
elif [[ "$MANUAL_ALL_PASSED" == true ]]; then
  GATE_PASSED=true; REASON="automation and all manual signoffs passed"
elif [[ $ALLOW_PENDING -eq 1 && "$MANUAL_ANY_PENDING" == true ]]; then
  GATE_PASSED=true; REASON="automation green; only manual signoffs pending under --allow-pending"
else
  REASON="automation green but manual signoffs remain pending"
fi

jq -cn --arg gate_result "$GATE_RESULT_PATH" --argjson verify_exit "$VERIFY_EXIT" \
  --argjson automation_passed "$AUTOMATION_PASSED" --argjson manual "$MANUAL_SUMMARY" \
  --argjson allow_pending "$([[ $ALLOW_PENDING -eq 1 ]] && echo true || echo false)" \
  --argjson passed "$GATE_PASSED" --arg reason "$REASON" \
  '{gate_result:$gate_result,verify_exit_code:$verify_exit,automation_passed:$automation_passed,manual_signoffs:$manual,allow_pending:$allow_pending,gate_passed:$passed,reason:$reason}'
[[ "$GATE_PASSED" == true ]] && exit 0
exit 1
