#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.3 gate aggregator.
#
# Contract: gate-commands.md ("gate" role) -- "聚合 verified report + manual
# signoff；strict 仅全部通过 exit 0，--allow-pending 只允许"自动全绿、仅人工
# 待签"exit 0."
#
# Design: this script does NOT trust gate-result.json's own self-reported
# hard_gates/gate_passed fields (a report script self-reporting "passed" is
# exactly the fake-green surface this v0.3 work exists to close). Instead it
# calls scripts/verify-flow-v0.3-json.sh -- which independently recomputes
# every hard gate from the referenced convergence-result.json/
# benchmark-result.json/etc -- and treats verify's exit code as the sole
# authority for "is automation green". manual_signoffs are read directly
# from gate-result.json (that block is scripts/record-flow-v0.3-manual-signoff.sh's
# exclusive write surface, not something this script recomputes).
#
# Exit codes: 0 = contract satisfied (strict: everything passed; --allow-pending:
# automation passed and only manual signoffs are pending), 1 = gate failed,
# 2 = usage/tool/evidence malformed.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.3"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
GATE_YAML="/opt/working/sylvode-flow/gates/v0.3-gate.yaml"
SCHEMA_DIR="$ROOT_DIR/docs/schemas"
REPO_ROOT="$ROOT_DIR"
GATE_RESULT_PATH=""
ALLOW_PENDING=0
JSON_MODE=0
CALIBRATION_PATH_OVERRIDE=""

usage() {
  cat <<'EOF'
Usage: scripts/gate-flow-v0.3.sh --json [--allow-pending] [OPTIONS]

Aggregates the verified v0.3 report with the recorded manual signoffs.
Strict mode (default) exits 0 only when every automated hard gate
(verified independently via scripts/verify-flow-v0.3-json.sh, not trusted
from gate-result.json's self-report) AND all four manual signoffs
(editor_ime, selection_cursor, dependency_license, engine_decision) are
"passed". --allow-pending additionally accepts the pre-signoff handoff
state: automation fully green, manual signoffs still "pending" (never
"failed" or "needs_rework").

Options:
  --allow-pending         Accept "automation green, manual signoffs still
                          pending" as success.
  --json                  Print the gate summary as JSON. Required for
                          contract compatibility (the frozen
                          required_commands.gate entry is
                          "scripts/gate-flow-v0.3.sh --json").
  --gate-result PATH      Path to gate-result.json. Default:
                          <evidence-root>/gate-result.json
  --evidence-root DIR    Root passed through to verify-flow-v0.3-json.sh.
                          Default: /opt/working/sylvode-flow/evidence/v0.3
  --contracts-root DIR   Root passed through to verify-flow-v0.3-json.sh.
                          Default: /opt/working/sylvode-flow
  --gate-yaml PATH        Path passed through to verify-flow-v0.3-json.sh.
                          Default: /opt/working/sylvode-flow/gates/v0.3-gate.yaml
  --schema-dir DIR        Path passed through to verify-flow-v0.3-json.sh.
                          Default: <repo>/docs/schemas
  --repo-root DIR         Path passed through to verify-flow-v0.3-json.sh.
                          Default: this checkout.
  --calibration PATH      Path passed through to verify-flow-v0.3-json.sh
                          (per-machine isolation-calibration evidence).
                          Default: same as verify's own default.
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
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
    --gate-yaml) GATE_YAML="${2:?--gate-yaml requires a PATH argument}"; shift 2 ;;
    --schema-dir) SCHEMA_DIR="${2:?--schema-dir requires a DIR argument}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --calibration) CALIBRATION_PATH_OVERRIDE="${2:?--calibration requires a PATH argument}"; shift 2 ;;
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
  exit 2
fi

VERIFY_SCRIPT="$ROOT_DIR/scripts/verify-flow-v0.3-json.sh"
if [[ ! -x "$VERIFY_SCRIPT" ]]; then
  echo "FAIL: verify script not found or not executable: $VERIFY_SCRIPT" >&2
  exit 2
fi

echo "=== Running scripts/verify-flow-v0.3-json.sh (authoritative automated result) ===" >&2
VERIFY_ARGS=(
  --evidence-root "$EVIDENCE_ROOT"
  --contracts-root "$CONTRACTS_ROOT"
  --gate-yaml "$GATE_YAML"
  --schema-dir "$SCHEMA_DIR"
  --repo-root "$REPO_ROOT"
)
if [[ -n "$CALIBRATION_PATH_OVERRIDE" ]]; then
  VERIFY_ARGS+=(--calibration "$CALIBRATION_PATH_OVERRIDE")
fi
set +e
VERIFY_OUTPUT="$("$VERIFY_SCRIPT" "${VERIFY_ARGS[@]}" "$GATE_RESULT_PATH" 2>&1)"
VERIFY_EXIT=$?
set -e
while IFS= read -r line; do
  echo "  [verify] $line" >&2
done <<<"$VERIFY_OUTPUT"
AUTOMATION_PASSED=$([[ $VERIFY_EXIT -eq 0 ]] && echo true || echo false)

if [[ $VERIFY_EXIT -ge 2 ]]; then
  echo "FAIL: verify-flow-v0.3-json.sh reported a usage/tool/evidence error (exit=$VERIFY_EXIT), not a drift finding; gate cannot proceed" >&2
  exit 2
fi

MANUAL_KEYS="editor_ime selection_cursor dependency_license engine_decision"
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
  REASON="automation not green (scripts/verify-flow-v0.3-json.sh exited $VERIFY_EXIT)"
elif [[ "$MANUAL_ANY_BLOCKING" == "true" ]]; then
  REASON="one or more manual signoffs are failed/needs_rework"
elif [[ "$MANUAL_ALL_PASSED" == "true" ]]; then
  GATE_PASSED=true
  REASON="automation green and all manual signoffs passed"
elif [[ $ALLOW_PENDING -eq 1 ]]; then
  GATE_PASSED=true
  REASON="automation green; manual signoffs pending, accepted under --allow-pending"
else
  REASON="automation green but manual signoffs are not all passed (rerun with --allow-pending to accept the pre-signoff handoff state, or record signoffs with scripts/record-flow-v0.3-manual-signoff.sh)"
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
