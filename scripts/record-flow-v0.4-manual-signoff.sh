#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 manual signoff recorder.
#
# Contract: gate-commands.md ("record" role) -- "唯一可写人工签署入口；拒绝
# 空 reviewer/evidence、未知 key、非法状态和覆盖已签记录." The five v0.4 keys
# are the manual_signoffs required by sylvode-flow-gate-v1.schema.json:
# page_editor, navigator_a11y, restart_recovery, feature_flag, forms_regression.
#
# This is the ONLY script allowed to write gate-result.json's
# manual_signoffs block. It also recomputes the receipt's derived acceptance
# state (mode, gate_passed, counts, blockers) from checks, hard_gates and all
# five manual rows. It edits gate-result.json in place with an atomic write;
# it never changes checks/hard_gates/artifacts.
#
# Exit codes: 0 = recorded, 1 = rejected (unknown key/status, empty
# reviewer/evidence, overwrite of an already-signed row without --force),
# 2 = usage/tool/evidence malformed.

EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
GATE_RESULT_PATH=""
KEY=""
STATUS_VALUE=""
REVIEWER=""
EVIDENCE_NOTE=""
FORCE=0
DRY_RUN=0
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RECEIPT_STATE_FILTER="$ROOT_DIR/scripts/lib/flow_gate_v0_4_receipt_state.jq"

VALID_KEYS="page_editor navigator_a11y restart_recovery feature_flag forms_regression"
VALID_STATUSES="pending passed failed needs_rework"

usage() {
  local key_count
  key_count="$(wc -w <<<"$VALID_KEYS" | tr -d ' ')"
  cat <<EOF
Usage: scripts/record-flow-v0.4-manual-signoff.sh --key KEY --status STATUS --reviewer NAME --evidence NOTE [OPTIONS]

Records one manual signoff row in evidence/v0.4/gate-result.json's
manual_signoffs block. This is the only script allowed to write that block.

Keys (sylvode-flow-gate-v0.4.schema.json manual_signoffs):
  page_editor            Page editor manual review (v0.4 content commands
                          exercised through the real editor UI)
  navigator_a11y          Navigator keyboard/a11y equivalence review
  restart_recovery         Server restart / snapshot-tail recovery review
  feature_flag             Flow feature-flag navigation + direct-URL review
  forms_regression          Forms UI regression review (no degradation from
                          Flow landing alongside existing Forms)

Statuses: pending, passed, failed, needs_rework

Required options:
  --key KEY           One of the ${key_count} keys above.
  --status STATUS      One of the four statuses above.
  --reviewer NAME       Non-empty reviewer identity. Required whenever
                        --status is not "pending" (a "pending" row records
                        who initiated pending state and is also required to
                        be non-empty here -- this script does not accept an
                        anonymous row at any status).
  --evidence NOTE       Non-empty pointer to the manual evidence (a path, a
                        video index, an issue link, etc). Same non-empty
                        requirement as --reviewer.

Options:
  --gate-result PATH    Path to gate-result.json to edit.
                        Default: <evidence-root>/gate-result.json
  --evidence-root DIR   Used only to compute the default --gate-result path.
                        Default: /opt/working/sylvode-flow/evidence/v0.4
  --force               Allow overwriting a row that is already "passed" or
                        "failed" (normally rejected -- a signed record is
                        not silently replaced).
  --dry-run             Print what would change without writing the file.
  --list-keys            Print the valid keys and exit 0.
  -h, --help              Show this help and exit 0.

Exit codes: 0 recorded, 1 rejected, 2 usage/tool/malformed.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --key) KEY="${2:?--key requires a value}"; shift 2 ;;
    --status) STATUS_VALUE="${2:?--status requires a value}"; shift 2 ;;
    --reviewer) REVIEWER="${2:?--reviewer requires a value}"; shift 2 ;;
    --evidence) EVIDENCE_NOTE="${2:?--evidence requires a value}"; shift 2 ;;
    --gate-result) GATE_RESULT_PATH="${2:?--gate-result requires a PATH argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --force) FORCE=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --list-keys) echo "$VALID_KEYS" | tr ' ' '\n'; exit 0 ;;
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
if [[ ! -f "$RECEIPT_STATE_FILTER" ]]; then
  echo "FAIL: receipt-state filter not found: $RECEIPT_STATE_FILTER" >&2
  exit 2
fi

if [[ -z "$KEY" ]]; then
  echo "FAIL: --key is required" >&2
  usage >&2
  exit 2
fi
if [[ -z "$STATUS_VALUE" ]]; then
  echo "FAIL: --status is required" >&2
  usage >&2
  exit 2
fi
if [[ -z "$REVIEWER" ]]; then
  echo "FAIL: --reviewer is required and must be non-empty" >&2
  exit 1
fi
if [[ -z "$EVIDENCE_NOTE" ]]; then
  echo "FAIL: --evidence is required and must be non-empty" >&2
  exit 1
fi

KEY_VALID=0
for k in $VALID_KEYS; do
  [[ "$KEY" == "$k" ]] && KEY_VALID=1
done
if [[ $KEY_VALID -ne 1 ]]; then
  echo "FAIL: unknown key: $KEY (valid: $VALID_KEYS)" >&2
  exit 1
fi

STATUS_VALID=0
for s in $VALID_STATUSES; do
  [[ "$STATUS_VALUE" == "$s" ]] && STATUS_VALID=1
done
if [[ $STATUS_VALID -ne 1 ]]; then
  echo "FAIL: invalid status: $STATUS_VALUE (valid: $VALID_STATUSES)" >&2
  exit 1
fi

if [[ ! -f "$GATE_RESULT_PATH" ]]; then
  echo "FAIL: gate-result.json not found: $GATE_RESULT_PATH" >&2
  exit 2
fi
if ! jq empty "$GATE_RESULT_PATH" >/dev/null 2>&1; then
  echo "FAIL: not valid JSON: $GATE_RESULT_PATH" >&2
  exit 2
fi
# Structural pre-flight, before any jq call that assumes an object shape (field
# access / key assignment on a non-object crashes jq and, for the plain `jq ...
# > $TMP_PATH` write below, would leak that crash's exit code straight out of
# this script under `set -e`). Any problem here is "evidence malformed" (exit 2).
TOP_LEVEL_TYPE="$(jq -r 'type' "$GATE_RESULT_PATH")"
if [[ "$TOP_LEVEL_TYPE" != "object" ]]; then
  echo "FAIL: $GATE_RESULT_PATH top level is not a JSON object (found $TOP_LEVEL_TYPE)" >&2
  exit 2
fi
MANUAL_SIGNOFFS_TYPE="$(jq -r 'if has("manual_signoffs") then (.manual_signoffs | type) else "missing" end' "$GATE_RESULT_PATH")"
if [[ "$MANUAL_SIGNOFFS_TYPE" != "missing" && "$MANUAL_SIGNOFFS_TYPE" != "object" ]]; then
  echo "FAIL: $GATE_RESULT_PATH .manual_signoffs must be a JSON object, found $MANUAL_SIGNOFFS_TYPE" >&2
  exit 2
fi
if [[ "$(jq -r '.schema_version // empty' "$GATE_RESULT_PATH")" != "sylvode.flow.gate-result.v1" ]]; then
  echo "FAIL: $GATE_RESULT_PATH is not a sylvode.flow.gate-result.v1 document" >&2
  exit 2
fi
if [[ "$(jq --arg k "$KEY" '.manual_signoffs | has($k) | not' "$GATE_RESULT_PATH")" == "true" ]]; then
  echo "FAIL: $GATE_RESULT_PATH manual_signoffs has no '$KEY' row (schema drift?)" >&2
  exit 2
fi

CURRENT_STATUS="$(jq -r --arg k "$KEY" '.manual_signoffs[$k].status // "pending"' "$GATE_RESULT_PATH")"
if [[ "$CURRENT_STATUS" == "passed" || "$CURRENT_STATUS" == "failed" ]] && [[ $FORCE -ne 1 ]]; then
  echo "FAIL: manual_signoffs.$KEY is already '$CURRENT_STATUS'; refusing to overwrite a signed record without --force" >&2
  exit 1
fi

if [[ $DRY_RUN -eq 1 ]]; then
  echo "DRY RUN: would set manual_signoffs.$KEY = {status: $STATUS_VALUE, reviewer: $REVIEWER, evidence: $EVIDENCE_NOTE} in $GATE_RESULT_PATH (current status: $CURRENT_STATUS)"
  exit 0
fi

TMP_PATH="$GATE_RESULT_PATH.tmp"
jq --arg k "$KEY" --arg status "$STATUS_VALUE" --arg reviewer "$REVIEWER" --arg evidence "$EVIDENCE_NOTE" \
  '.manual_signoffs[$k] = {status: $status, reviewer: $reviewer, evidence: $evidence}' \
  "$GATE_RESULT_PATH" | jq -f "$RECEIPT_STATE_FILTER" > "$TMP_PATH"
mv -f "$TMP_PATH" "$GATE_RESULT_PATH"

DERIVED_SUMMARY="$(jq -c '{mode,gate_passed,counts,blockers}' "$GATE_RESULT_PATH")"
echo "Recorded manual_signoffs.$KEY = $STATUS_VALUE (reviewer=$REVIEWER) in $GATE_RESULT_PATH (was: $CURRENT_STATUS)"
echo "Derived receipt state: $DERIVED_SUMMARY"
exit 0
