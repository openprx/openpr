#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 gate aggregator.
#
# Contract: gate-commands.md ("gate" role) -- "聚合 verified report + manual
# signoff；strict 仅全部通过 exit 0，--allow-pending 只允许"自动全绿、仅人工
# 待签"exit 0."
#
# Design: this script does NOT trust gate-result.json's own self-reported
# hard_gates/gate_passed fields. Instead it calls
# scripts/verify-flow-v0.4-json.sh -- which independently recomputes every
# hard gate from the referenced evidence artifacts -- and treats verify's
# exit code as the sole authority for "is automation green". manual_signoffs
# are read directly from gate-result.json (that block is
# scripts/record-flow-v0.4-manual-signoff.sh's exclusive write surface, not
# something this script recomputes).
#
# Exit codes: 0 = contract satisfied (strict: everything passed;
# --allow-pending: automation passed and only manual signoffs are pending),
# 1 = gate failed, 2 = usage/tool/evidence malformed.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
REPO_ROOT="$ROOT_DIR"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/sylvode-flow-gate-v0.4.schema.json"
GATE_RESULT_PATH=""
ALLOW_PENDING=0
JSON_MODE=0

MANUAL_KEYS="page_editor navigator_a11y restart_recovery feature_flag forms_regression"

usage() {
  cat <<EOF
Usage: scripts/gate-flow-v0.4.sh --json [--allow-pending] [OPTIONS]

Aggregates the verified v0.4 report with the recorded manual signoffs.
Strict mode (default) exits 0 only when every automated hard gate
(verified independently via scripts/verify-flow-v0.4-json.sh, not trusted
from gate-result.json's self-report) AND all five manual signoffs
($MANUAL_KEYS) are "passed". --allow-pending additionally accepts the
pre-signoff handoff state: automation fully green, manual signoffs still
"pending" (never "failed" or "needs_rework").

Options:
  --allow-pending         Accept "automation green, manual signoffs still
                          pending" as success.
  --json                  Print the gate summary as JSON. Required for
                          contract compatibility (the frozen
                          required_commands.gate entry is
                          "scripts/gate-flow-v0.4.sh --json").
  --gate-result PATH      Path to gate-result.json. Default:
                          <evidence-root>/gate-result.json
  --evidence-root DIR    Root passed through to verify-flow-v0.4-json.sh.
                          Default: /opt/working/sylvode-flow/evidence/v0.4
  --repo-root DIR         Path passed through to verify-flow-v0.4-json.sh.
                          Default: this checkout.
  --schema PATH           Path passed through to verify-flow-v0.4-json.sh.
                          Default: docs/schemas/sylvode-flow-gate-v0.4.schema.json
  -h, --help              Show this help and exit 0.

Exit codes: 0 contract satisfied, 1 gate failed, 2 usage/tool/malformed.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --allow-pending) ALLOW_PENDING=1; shift ;;
    --json) JSON_MODE=1; shift ;;
    --gate-result) GATE_RESULT_PATH="${2:?--gate-result requires a PATH argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --schema) SCHEMA_PATH="${2:?--schema requires a PATH argument}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$GATE_RESULT_PATH" ]]; then
  GATE_RESULT_PATH="$EVIDENCE_ROOT/gate-result.json"
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "FAIL: missing required command: jq" >&2
  echo "Fix: sudo apt-get install -y jq" >&2
  exit 2
fi
if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required (frozen required_commands entry uses --json)" >&2
  usage >&2
  exit 2
fi
if [[ ! -f "$GATE_RESULT_PATH" ]]; then
  echo "FAIL: gate-result.json not found: $GATE_RESULT_PATH" >&2
  echo "Fix: run scripts/report-flow-v0.4-json.sh first (it writes this file when every required artifact exists, preserving failed checks with report exit 1)." >&2
  exit 2
fi

VERIFY_SCRIPT="$ROOT_DIR/scripts/verify-flow-v0.4-json.sh"
if [[ ! -x "$VERIFY_SCRIPT" ]]; then
  echo "FAIL: verify script not found or not executable: $VERIFY_SCRIPT" >&2
  exit 2
fi

echo "=== Running scripts/verify-flow-v0.4-json.sh (authoritative automated result) ===" >&2
set +e
VERIFY_OUTPUT="$("$VERIFY_SCRIPT" "$GATE_RESULT_PATH" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --schema "$SCHEMA_PATH" --json 2>&1)"
VERIFY_EXIT=$?
set -e
while IFS= read -r line; do
  echo "  [verify] $line" >&2
done <<<"$VERIFY_OUTPUT"
AUTOMATION_PASSED=$([[ $VERIFY_EXIT -eq 0 ]] && echo true || echo false)

if [[ $VERIFY_EXIT -ge 2 ]]; then
  echo "FAIL: verify-flow-v0.4-json.sh reported a usage/tool/evidence error (exit=$VERIFY_EXIT), not a drift finding; gate cannot proceed" >&2
  exit 2
fi

MANUAL_ALL_PASSED=true
MANUAL_ANY_BLOCKING=false
MANUAL_SUMMARY="{}"
for key in $MANUAL_KEYS; do
  status="$(jq -r --arg k "$key" '.manual_signoffs[$k].status // empty' "$GATE_RESULT_PATH")"
  if [[ -z "$status" ]]; then
    echo "FAIL: gate-result.json manual_signoffs.$key is missing" >&2
    exit 2
  fi
  MANUAL_SUMMARY="$(jq -c --arg k "$key" --arg s "$status" '. + {($k): $s}' <<<"$MANUAL_SUMMARY")"
  if [[ "$status" != "passed" ]]; then
    MANUAL_ALL_PASSED=false
  fi
  if [[ "$status" == "failed" || "$status" == "needs_rework" ]]; then
    MANUAL_ANY_BLOCKING=true
  fi
done

GATE_PASSED=false
REASON=""
if [[ "$AUTOMATION_PASSED" != "true" ]]; then
  REASON="automation not green (scripts/verify-flow-v0.4-json.sh exited $VERIFY_EXIT)"
elif [[ "$MANUAL_ANY_BLOCKING" == "true" ]]; then
  REASON="one or more manual signoffs are failed/needs_rework"
elif [[ "$MANUAL_ALL_PASSED" == "true" ]]; then
  GATE_PASSED=true
  REASON="automation green and all manual signoffs passed"
elif [[ $ALLOW_PENDING -eq 1 ]]; then
  GATE_PASSED=true
  REASON="automation green; manual signoffs pending, accepted under --allow-pending"
else
  REASON="automation green but manual signoffs are not all passed (rerun with --allow-pending to accept the pre-signoff handoff state, or record signoffs with scripts/record-flow-v0.4-manual-signoff.sh)"
fi

SUMMARY="$(jq -n \
  --arg gate_result "$GATE_RESULT_PATH" \
  --argjson automation_passed "$AUTOMATION_PASSED" \
  --argjson verify_exit_code "$VERIFY_EXIT" \
  --argjson manual "$MANUAL_SUMMARY" \
  --argjson allow_pending "$([[ $ALLOW_PENDING -eq 1 ]] && echo true || echo false)" \
  --argjson gate_passed "$GATE_PASSED" \
  --arg reason "$REASON" \
  '{
    gate_result: $gate_result,
    automation_passed: $automation_passed,
    verify_exit_code: $verify_exit_code,
    manual_signoffs: $manual,
    allow_pending: $allow_pending,
    gate_passed: $gate_passed,
    reason: $reason
  }')"

echo "$SUMMARY" | jq .

if [[ "$GATE_PASSED" == "true" ]]; then
  exit 0
fi
exit 1
