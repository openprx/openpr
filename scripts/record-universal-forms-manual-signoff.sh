#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_RUNBOOK_PATH="/opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
DEFAULT_EVIDENCE_PATH="/opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="$DEFAULT_RUNBOOK_PATH"
EVIDENCE_PATH="$DEFAULT_EVIDENCE_PATH"
ITEM_KEY=""
STATUS_VALUE=""
REVIEWER=""
EVIDENCE_NOTE=""
DRY_RUN=0

usage() {
  cat <<'EOF'
Usage: scripts/record-universal-forms-manual-signoff.sh --item KEY --status STATUS --reviewer NAME --evidence NOTE [options]

Records one user-side manual acceptance row in both the Chinese runbook and the
formal acceptance evidence report, then verifies both files are synchronized.
This script does not run final acceptance; finalization still requires every row
to be accepted and the finalizer gates to pass.
The `overall` row can only be accepted after the six concrete review rows are
already accepted.

Items:
  restaurant_template   Restaurant template can create a project directly
  frontend_usability    Universal forms frontend is usable by a non-technical operator
  amounts               Amount, quantity, and subtotal behavior is acceptable
  workflow              Order, order line, table change, print, and report workflow is acceptable
  hub_consistency       MCP/API/Webhook/Connector consistency is acceptable
  docs                  README/docs are sufficient for a new user to reproduce
  overall               Overall acceptance

Statuses:
  pending, accepted, approved, passed, failed, rework

Options:
  --runbook PATH        User acceptance runbook path.
  --report PATH         Acceptance evidence report path.
  --dry-run             Show the edits without replacing files.
  --list-items          Print accepted item keys.

For the default handoff paths, successful writes also refresh the generated
completion audit, completion audit JSON, manual evidence map, UI review gallery,
signoff status report, signoff status JSON, next signoff review, next signoff
review contract smoke, signoff dashboard, signoff dashboard render smoke, signoff dashboard progression smoke,
signoff status output smoke, user acceptance
packet, readiness summary, readiness JSON, development status JSON, scenario catalog JSON,
implementation map JSON, report
output boundary smoke, next signoff command smoke, manual signoff progression smoke, all manual signoff command smoke,
release gate output smoke, delivery status output smoke, delivery manifest, delivery manifest JSON, and their focused
verifiers/contract smokes so pending counts and checksums stay synchronized
with the signed row.
EOF
}

list_items() {
  cat <<'EOF'
restaurant_template
frontend_usability
amounts
workflow
hub_consistency
docs
overall
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --item)
      ITEM_KEY="${2:-}"
      shift 2
      ;;
    --status)
      STATUS_VALUE="${2:-}"
      shift 2
      ;;
    --reviewer)
      REVIEWER="${2:-}"
      shift 2
      ;;
    --evidence)
      EVIDENCE_NOTE="${2:-}"
      shift 2
      ;;
    --runbook)
      RUNBOOK_PATH="${2:-}"
      shift 2
      ;;
    --report)
      EVIDENCE_PATH="${2:-}"
      shift 2
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --list-items)
      list_items
      exit 0
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

trim() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  printf '%s' "$value"
}

require_clean_cell() {
  local label="$1"
  local value="$2"
  if [[ "$value" == *"|"* || "$value" == *$'\n'* || "$value" == *$'\r'* ]]; then
    echo "$label must not contain markdown table pipes or newlines" >&2
    exit 2
  fi
}

RUNBOOK_ITEM=""
EVIDENCE_ITEM=""
case "$ITEM_KEY" in
  restaurant_template)
    RUNBOOK_ITEM="餐厅模板可直接创建项目"
    EVIDENCE_ITEM="Restaurant template can create a project directly"
    ;;
  frontend_usability)
    RUNBOOK_ITEM="万能表单前端可被非技术用户操作"
    EVIDENCE_ITEM="Universal forms frontend is usable by a non-technical operator"
    ;;
  amounts)
    RUNBOOK_ITEM="金额/数量/小计展示符合业务预期"
    EVIDENCE_ITEM="Amount, quantity, and subtotal behavior is acceptable"
    ;;
  workflow)
    RUNBOOK_ITEM="订单、明细、换桌、打印、报表链路可用"
    EVIDENCE_ITEM="Order, order line, table change, print, and report workflow is acceptable"
    ;;
  hub_consistency)
    RUNBOOK_ITEM="MCP/API/Webhook/Connector 业务枢纽一致性可接受"
    EVIDENCE_ITEM="MCP/API/Webhook/Connector consistency is acceptable"
    ;;
  docs)
    RUNBOOK_ITEM="README/docs 足够让新用户理解和复现"
    EVIDENCE_ITEM="README/docs are sufficient for a new user to reproduce"
    ;;
  overall)
    RUNBOOK_ITEM="总体验收"
    EVIDENCE_ITEM="Overall acceptance"
    ;;
  "")
    echo "--item is required" >&2
    usage >&2
    exit 2
    ;;
  *)
    echo "Unknown --item: $ITEM_KEY" >&2
    list_items >&2
    exit 2
    ;;
esac

status_normalized="$(printf '%s' "$STATUS_VALUE" | tr '[:upper:]' '[:lower:]')"
status_normalized="$(trim "$status_normalized")"
RUNBOOK_STATUS=""
EVIDENCE_STATUS=""
case "$status_normalized" in
  pending|待验收)
    RUNBOOK_STATUS="待验收"
    EVIDENCE_STATUS="Pending"
    REVIEWER=""
    EVIDENCE_NOTE=""
    ;;
  accepted|approved|passed|pass|通过|已通过|已验收)
    RUNBOOK_STATUS="通过"
    EVIDENCE_STATUS="Accepted"
    ;;
  failed|fail|失败)
    RUNBOOK_STATUS="失败"
    EVIDENCE_STATUS="Failed"
    ;;
  rework|'needs rework'|需整改)
    RUNBOOK_STATUS="需整改"
    EVIDENCE_STATUS="Needs rework"
    ;;
  "")
    echo "--status is required" >&2
    usage >&2
    exit 2
    ;;
  *)
    echo "Unsupported --status: $STATUS_VALUE" >&2
    usage >&2
    exit 2
    ;;
esac

if [[ ! -f "$RUNBOOK_PATH" ]]; then
  echo "Runbook not found: $RUNBOOK_PATH" >&2
  exit 2
fi
if [[ ! -f "$EVIDENCE_PATH" ]]; then
  echo "Acceptance evidence report not found: $EVIDENCE_PATH" >&2
  exit 2
fi

if [[ "$RUNBOOK_STATUS" != "待验收" ]]; then
  if [[ -z "$(trim "$REVIEWER")" ]]; then
    echo "--reviewer is required for non-pending signoff statuses" >&2
    exit 2
  fi
  if [[ -z "$(trim "$EVIDENCE_NOTE")" ]]; then
    echo "--evidence is required for non-pending signoff statuses" >&2
    exit 2
  fi
fi

require_clean_cell "--reviewer" "$REVIEWER"
require_clean_cell "--evidence" "$EVIDENCE_NOTE"

normalize_signoff_status() {
  local raw
  raw="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
  raw="$(trim "$raw")"
  case "$raw" in
    pass|passed|approved|accepted|通过|已通过|已验收)
      printf 'passed'
      ;;
    pending|待验收|'')
      printf 'pending'
      ;;
    fail|failed|失败)
      printf 'failed'
      ;;
    rework|'needs rework'|需整改)
      printf 'rework'
      ;;
    *)
      printf 'unknown'
      ;;
  esac
}

row_status() {
  local path="$1"
  local heading="$2"
  local item="$3"
  awk -F'|' -v heading="$heading" -v expected="$item" '
    $0 == heading { in_section = 1; next }
    /^## / && in_section { exit }
    in_section && NF >= 5 {
      current = $2
      status = $3
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", current)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", status)
      if (current == expected) {
        print status
        found = 1
        exit
      }
    }
    END {
      if (!found) {
        exit 1
      }
    }
  ' "$path"
}

prerequisite_items=(
  "Restaurant template can create a project directly"
  "Universal forms frontend is usable by a non-technical operator"
  "Amount, quantity, and subtotal behavior is acceptable"
  "Order, order line, table change, print, and report workflow is acceptable"
  "MCP/API/Webhook/Connector consistency is acceptable"
  "README/docs are sufficient for a new user to reproduce"
)

is_overall_prerequisite() {
  local candidate="$1"
  local prerequisite
  for prerequisite in "${prerequisite_items[@]}"; do
    if [[ "$candidate" == "$prerequisite" ]]; then
      return 0
    fi
  done
  return 1
}

update_row() {
  local path="$1"
  local heading="$2"
  local item="$3"
  local status="$4"
  local reviewer="$5"
  local evidence="$6"
  local output="$7"

  awk -v heading="$heading" \
    -v expected="$item" \
    -v replacement="| $item | $status | $reviewer | $evidence |" '
    BEGIN { in_section = 0; found = 0 }
    $0 == heading { in_section = 1; print; next }
    /^## / && in_section { in_section = 0 }
    in_section && $0 ~ /^\|/ {
      split($0, cells, "|")
      current = cells[2]
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", current)
      if (current == expected) {
        print replacement
        found = 1
        next
      }
    }
    { print }
    END {
      if (!found) {
        exit 4
      }
    }
  ' "$path" >"$output"
}

runbook_candidate="$(mktemp "$(dirname "$RUNBOOK_PATH")/.signoff-runbook.XXXXXX")"
evidence_candidate="$(mktemp "$(dirname "$EVIDENCE_PATH")/.signoff-evidence.XXXXXX")"
trap 'rm -f "$runbook_candidate" "$evidence_candidate"' EXIT

if ! update_row "$RUNBOOK_PATH" "## 验收结论记录" "$RUNBOOK_ITEM" "$RUNBOOK_STATUS" "$REVIEWER" "$EVIDENCE_NOTE" "$runbook_candidate"; then
  echo "Runbook signoff row not found: $RUNBOOK_ITEM" >&2
  exit 1
fi
if ! update_row "$EVIDENCE_PATH" "## Manual Acceptance Signoff" "$EVIDENCE_ITEM" "$EVIDENCE_STATUS" "$REVIEWER" "$EVIDENCE_NOTE" "$evidence_candidate"; then
  echo "Evidence signoff row not found: $EVIDENCE_ITEM" >&2
  exit 1
fi

if [[ "$ITEM_KEY" == "overall" && "$(normalize_signoff_status "$EVIDENCE_STATUS")" == "passed" ]]; then
  missing=0
  for prerequisite in "${prerequisite_items[@]}"; do
    prerequisite_status="$(row_status "$evidence_candidate" "## Manual Acceptance Signoff" "$prerequisite" || true)"
    if [[ "$(normalize_signoff_status "$prerequisite_status")" != "passed" ]]; then
      echo "Overall acceptance cannot be passed before prerequisite row is passed: $prerequisite => ${prerequisite_status:-missing}" >&2
      missing=$((missing + 1))
    fi
  done
  if [[ "$missing" -ne 0 ]]; then
    echo "Overall acceptance signoff is blocked: $missing prerequisite row(s) are not passed" >&2
    exit 1
  fi
fi

if is_overall_prerequisite "$EVIDENCE_ITEM" && [[ "$(normalize_signoff_status "$EVIDENCE_STATUS")" != "passed" ]]; then
  overall_status="$(row_status "$evidence_candidate" "## Manual Acceptance Signoff" "Overall acceptance" || true)"
  if [[ "$(normalize_signoff_status "$overall_status")" == "passed" ]]; then
    echo "Prerequisite signoff cannot be downgraded while overall acceptance is passed: $EVIDENCE_ITEM => $EVIDENCE_STATUS" >&2
    echo "Reopen the overall acceptance row first, then update the prerequisite row." >&2
    exit 1
  fi
fi

"$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh" \
  "$runbook_candidate" \
  "$evidence_candidate" >/dev/null

if [[ "$DRY_RUN" -eq 1 ]]; then
  echo "Dry run: would update manual signoff row"
  echo "  item: $ITEM_KEY"
  echo "  runbook status: $RUNBOOK_STATUS"
  echo "  evidence status: $EVIDENCE_STATUS"
  echo
  diff -u "$RUNBOOK_PATH" "$runbook_candidate" || true
  diff -u "$EVIDENCE_PATH" "$evidence_candidate" || true
  exit 0
fi

chmod --reference="$RUNBOOK_PATH" "$runbook_candidate"
chmod --reference="$EVIDENCE_PATH" "$evidence_candidate"
mv -f "$runbook_candidate" "$RUNBOOK_PATH"
mv -f "$evidence_candidate" "$EVIDENCE_PATH"
runbook_candidate=""
evidence_candidate=""
trap - EXIT

"$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh" \
  "$RUNBOOK_PATH" \
  "$EVIDENCE_PATH"

if [[ "$RUNBOOK_PATH" == "$DEFAULT_RUNBOOK_PATH" && "$EVIDENCE_PATH" == "$DEFAULT_EVIDENCE_PATH" ]]; then
  "$ROOT_DIR/scripts/report-universal-forms-completion-audit.sh" >/dev/null
  "$ROOT_DIR/scripts/prepare-universal-forms-manual-evidence-map.sh" >/dev/null
  "$ROOT_DIR/scripts/prepare-universal-forms-ui-review-gallery.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-ui-review-gallery.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-ui-review-gallery-render.sh" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-signoff-status.sh" \
    --output "/opt/worker/report/openpr/docs/openpr-universal-form-signoff-status-2026-05-31.md" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-signoff-status-json.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-signoff-status-json.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-signoff-status-json-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/prepare-universal-forms-signoff-dashboard.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-signoff-dashboard.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-signoff-dashboard-render.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-signoff-dashboard-progression.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-signoff-status-output.sh" >/dev/null
  "$ROOT_DIR/scripts/prepare-universal-forms-next-signoff-review.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-next-signoff-review.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-next-signoff-review-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-next-signoff-command.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-manual-signoff-progression.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-manual-signoff-commands.sh" >/dev/null
  "$ROOT_DIR/scripts/prepare-universal-forms-user-acceptance-packet.sh" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-readiness-summary.sh" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-readiness-json.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-readiness-json.sh" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-development-status-json.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-development-status-json.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-development-status-json-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-completion-audit-json.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-completion-audit-json.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-completion-audit-json-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-scenario-catalog-json.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-scenario-catalog-json.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-scenario-catalog-json-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-implementation-map-json.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-implementation-map-json.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-implementation-map-json-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-report-output-boundaries.sh" >/dev/null
  "$ROOT_DIR/scripts/prepare-universal-forms-delivery-manifest.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest.sh" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-delivery-manifest-json.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest-json.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-delivery-manifest-json-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-release-gate.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-release-gate-json.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-release-gate-json-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-release-gate-output.sh" >/dev/null
  "$ROOT_DIR/scripts/status-universal-forms-delivery.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-delivery-status-json.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-delivery-status-json-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-delivery-status-output.sh" >/dev/null
  echo "derived acceptance handoff refreshed"
else
  echo "custom signoff paths used; derived default handoff was not refreshed"
fi

echo "manual acceptance signoff recorded: $ITEM_KEY -> $EVIDENCE_STATUS"
