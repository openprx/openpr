#!/usr/bin/env bash
set -euo pipefail

# Sole supported writer for the two v0.5 manual-signoff rows. The shared jq
# program recomputes all derived receipt state after an atomic row update.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.5"
GATE_RESULT_PATH=""
KEY=""
STATUS_VALUE=""
REVIEWER=""
EVIDENCE_NOTE=""
FORCE=0
DRY_RUN=0
STATE_LIBRARY="$ROOT_DIR/scripts/lib/flow_gate_v0_5_receipt_state.jq"
# ADR-0017: multi_user and offline_recovery moved to gates/vF-frontend-gate.yaml.
VALID_KEYS="audit_causation permission_revocation"
VALID_STATUSES="pending passed failed needs_rework"

usage() {
  cat <<'EOF'
Usage: scripts/record-flow-v0.5-manual-signoff.sh --key KEY --status STATUS --reviewer NAME --evidence NOTE [OPTIONS]

Keys: audit_causation, permission_revocation
      (multi_user and offline_recovery moved to the frontend track, ADR-0017)
Statuses: pending, passed, failed, needs_rework

Options:
  --gate-result PATH    Default: <evidence-root>/gate-result.json
  --evidence-root DIR   Default: /opt/working/sylvode-flow/evidence/v0.5
  --force               Permit replacing a passed/failed signed row.
  --dry-run             Validate and show the update without writing.
  --list-keys           Print valid keys.
  -h, --help            Show help.

Exit codes: 0 recorded, 1 rejected, 2 usage/tool/malformed.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --key) KEY="${2:?--key requires VALUE}"; shift 2 ;;
    --status) STATUS_VALUE="${2:?--status requires VALUE}"; shift 2 ;;
    --reviewer) REVIEWER="${2:?--reviewer requires VALUE}"; shift 2 ;;
    --evidence) EVIDENCE_NOTE="${2:?--evidence requires VALUE}"; shift 2 ;;
    --gate-result) GATE_RESULT_PATH="${2:?--gate-result requires PATH}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires DIR}"; shift 2 ;;
    --force) FORCE=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --list-keys) tr ' ' '\n' <<<"$VALID_KEYS"; exit 0 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

command -v jq >/dev/null 2>&1 || { echo "FAIL: missing required command: jq" >&2; exit 2; }
[[ -f "$STATE_LIBRARY" ]] || { echo "FAIL: shared state library missing: $STATE_LIBRARY" >&2; exit 2; }
[[ -n "$GATE_RESULT_PATH" ]] || GATE_RESULT_PATH="$EVIDENCE_ROOT/gate-result.json"
[[ -n "$KEY" && -n "$STATUS_VALUE" ]] || { echo "FAIL: --key and --status are required" >&2; exit 2; }
[[ -n "$REVIEWER" ]] || { echo "FAIL: --reviewer must be non-empty" >&2; exit 1; }
[[ -n "$EVIDENCE_NOTE" ]] || { echo "FAIL: --evidence must be non-empty" >&2; exit 1; }
[[ " $VALID_KEYS " == *" $KEY "* ]] || { echo "FAIL: unknown key: $KEY" >&2; exit 1; }
[[ " $VALID_STATUSES " == *" $STATUS_VALUE "* ]] || { echo "FAIL: invalid status: $STATUS_VALUE" >&2; exit 1; }
if [[ ! -f "$GATE_RESULT_PATH" ]] || ! jq empty "$GATE_RESULT_PATH" >/dev/null 2>&1 || \
   [[ "$(jq -r type "$GATE_RESULT_PATH")" != object ]]; then
  echo "FAIL: gate-result is missing or malformed: $GATE_RESULT_PATH" >&2
  exit 2
fi
if [[ "$(jq -r '.schema_version // empty' "$GATE_RESULT_PATH")" != sylvode.flow.gate-result.v1 || \
      "$(jq -r '.schema_path // empty' "$GATE_RESULT_PATH")" != gates/v0.5-gate.yaml || \
      "$(jq -r '.release // empty' "$GATE_RESULT_PATH")" != 0.5.0 ]]; then
  echo "FAIL: gate-result is not a v0.5 receipt" >&2
  exit 2
fi
if ! jq -e '
  (.checks | type) == "array" and all(.checks[]; type == "object") and
  (.required_commands | type) == "object" and all(.required_commands[]; type == "object") and
  (.artifact_states | type) == "object" and all(.artifact_states[]; type == "object") and
  (.hard_gates | type) == "object" and
  (.predecessor | type) == "object" and
  (.budgets | type) == "object" and
  (.source | type) == "object" and
  (.manual_signoffs | type) == "object" and all(.manual_signoffs[]; type == "object")
' "$GATE_RESULT_PATH" >/dev/null 2>&1; then
  echo "FAIL: gate-result has malformed receipt-state collections" >&2
  exit 2
fi
EXPECTED_KEYS='["audit_causation","permission_revocation"]'
if [[ "$(jq -c '.manual_signoffs|keys' "$GATE_RESULT_PATH")" != "$EXPECTED_KEYS" ]]; then
  echo "FAIL: manual_signoffs keys do not exactly match the v0.5 contract" >&2
  exit 2
fi
CURRENT_STATUS="$(jq -r --arg k "$KEY" '.manual_signoffs[$k].status' "$GATE_RESULT_PATH")"
if [[ "$CURRENT_STATUS" == passed || "$CURRENT_STATUS" == failed ]] && [[ $FORCE -ne 1 ]]; then
  echo "FAIL: refusing to overwrite signed '$CURRENT_STATUS' row without --force" >&2
  exit 1
fi
if [[ $DRY_RUN -eq 1 ]]; then
  jq -cn --arg key "$KEY" --arg status "$STATUS_VALUE" --arg reviewer "$REVIEWER" \
    --arg evidence "$EVIDENCE_NOTE" --arg previous "$CURRENT_STATUS" \
    '{dry_run:true,key:$key,previous:$previous,new:{status:$status,reviewer:$reviewer,evidence:$evidence}}'
  exit 0
fi

TMP_PATH="$GATE_RESULT_PATH.tmp"
jq --arg k "$KEY" --arg status "$STATUS_VALUE" --arg reviewer "$REVIEWER" --arg evidence "$EVIDENCE_NOTE" \
  '.manual_signoffs[$k]={status:$status,reviewer:$reviewer,evidence:$evidence}' "$GATE_RESULT_PATH" \
  | jq -L "$ROOT_DIR/scripts/lib" 'include "flow_gate_v0_5_receipt_state"; flow_derive_receipt' > "$TMP_PATH"
mv -f "$TMP_PATH" "$GATE_RESULT_PATH"
jq -c --arg key "$KEY" --arg previous "$CURRENT_STATUS" \
  '{recorded:true,key:$key,previous:$previous,current:.manual_signoffs[$key],mode,gate_passed,counts,blockers}' "$GATE_RESULT_PATH"
