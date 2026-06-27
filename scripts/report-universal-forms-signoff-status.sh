#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
MANUAL_EVIDENCE_MAP_PATH="$REPORT_DIR/openpr-universal-form-manual-evidence-map-2026-05-31.md"
REVIEWER_PLACEHOLDER="<name>"
OUTPUT_PATH=""

usage() {
  cat <<'EOF'
Usage: scripts/report-universal-forms-signoff-status.sh [options]
       scripts/report-universal-forms-signoff-status.sh [options] [REPORT_PATH]

Prints the current user-side manual signoff state, the next actionable row,
the suggested evidence note, and the exact recorder command template. This is a
read-only reviewer aid; it never marks a row accepted.

Options:
  --report PATH        Acceptance evidence report path.
  --runbook PATH       User acceptance runbook path.
  --manual-map PATH    Manual evidence map path.
  --reviewer NAME      Reviewer name to place in command templates.
  --output PATH        Write the Markdown status report to PATH.

Optional positional REPORT_PATH is accepted as a convenience alias for
--report PATH when reviewing a copied evidence report.
EOF
}

positional_report_seen=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --report)
      EVIDENCE_PATH="${2:-}"
      shift 2
      ;;
    --runbook)
      RUNBOOK_PATH="${2:-}"
      shift 2
      ;;
    --manual-map)
      MANUAL_EVIDENCE_MAP_PATH="${2:-}"
      shift 2
      ;;
    --reviewer)
      REVIEWER_PLACEHOLDER="${2:-}"
      shift 2
      ;;
    --output)
      OUTPUT_PATH="${2:-}"
      if [[ -z "$OUTPUT_PATH" ]]; then
        echo "--output requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      if [[ "$1" == -* ]]; then
        echo "Unknown argument: $1" >&2
        usage >&2
        exit 2
      fi
      if [[ "$positional_report_seen" -eq 1 ]]; then
        echo "Only one positional REPORT_PATH is supported: $1" >&2
        usage >&2
        exit 2
      fi
      EVIDENCE_PATH="$1"
      positional_report_seen=1
      shift
      ;;
  esac
done

require_file() {
  local path="$1"
  if [[ ! -f "$path" ]]; then
    echo "Missing required file: $path" >&2
    exit 1
  fi
}

trim() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  printf '%s' "$value"
}

normalize_status() {
  local raw
  raw="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
  raw="$(trim "$raw")"
  case "$raw" in
    accepted|approved|passed|pass|通过|已通过|已验收)
      printf 'passed'
      ;;
    pending|待验收|'')
      printf 'pending'
      ;;
    failed|fail|失败)
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

manual_row_cell() {
  local item="$1"
  local column="$2"
  awk -F'|' -v expected="$item" -v column="$column" '
    /^## Manual Acceptance Signoff$/ { in_section = 1; next }
    /^## / && in_section { exit }
    in_section && NF >= 5 {
      current = $2
      value = $(column)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", current)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
      if (current == expected) {
        print value
        exit
      }
    }
  ' "$EVIDENCE_PATH"
}

suggested_note_for() {
  local item="$1"
  awk -F'|' -v expected="$item" '
    /^## Evidence Map$/ { in_section = 1; next }
    /^## / && in_section { exit }
    in_section && NF >= 6 {
      current = $2
      note = $5
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", current)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", note)
      if (current == expected) {
        gsub(/^`|`$/, "", note)
        print note
        exit
      }
    }
  ' "$MANUAL_EVIDENCE_MAP_PATH"
}

shell_single_quote() {
  printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

require_file "$EVIDENCE_PATH"
require_file "$RUNBOOK_PATH"
require_file "$MANUAL_EVIDENCE_MAP_PATH"

output_tmp=""
restore_stdout=false
if [[ -n "$OUTPUT_PATH" ]]; then
  mkdir -p "$(dirname "$OUTPUT_PATH")"
  output_tmp="$(mktemp "$(dirname "$OUTPUT_PATH")/.signoff-status.XXXXXX")"
  trap 'rm -f "$output_tmp"' EXIT
  exec 3>&1
  exec >"$output_tmp"
  restore_stdout=true
fi

finish() {
  local code="$1"
  if [[ "$restore_stdout" == "true" ]]; then
    exec >&3
    exec 3>&-
    if [[ -f "$OUTPUT_PATH" ]]; then
      chmod --reference="$OUTPUT_PATH" "$output_tmp"
    fi
    mv -f "$output_tmp" "$OUTPUT_PATH"
    output_tmp=""
    echo "manual signoff status report: $OUTPUT_PATH" >&2
  fi
  exit "$code"
}

consistency_status="failed"
if "$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh" "$RUNBOOK_PATH" "$EVIDENCE_PATH" >/dev/null; then
  consistency_status="passed"
fi

summary_total="$(sed -n 's/^- Total automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
summary_failed="$(sed -n 's/^- Failed automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
pass_count="$(rg -c '^\*\*Status:\*\* PASS$' "$EVIDENCE_PATH" || true)"

keys=(
  restaurant_template
  frontend_usability
  amounts
  workflow
  hub_consistency
  docs
  overall
)
items=(
  "Restaurant template can create a project directly"
  "Universal forms frontend is usable by a non-technical operator"
  "Amount, quantity, and subtotal behavior is acceptable"
  "Order, order line, table change, print, and report workflow is acceptable"
  "MCP/API/Webhook/Connector consistency is acceptable"
  "README/docs are sufficient for a new user to reproduce"
  "Overall acceptance"
)

pending_count=0
blocked_count=0
next_index=-1
accepted_prerequisites=0

printf '# OpenPR Universal Forms Manual Signoff Status\n\n'
printf '%s\n' "- Evidence: \`$EVIDENCE_PATH\`"
printf '%s\n' "- Runbook: \`$RUNBOOK_PATH\`"
printf '%s\n' "- Manual evidence map: \`$MANUAL_EVIDENCE_MAP_PATH\`"
printf '\n'

printf '## Gate Summary\n\n'
printf '| Gate | Value |\n'
printf '| --- | --- |\n'
printf '| Automated checks | %s |\n' "${summary_total:-missing}"
printf '| PASS status lines | %s |\n' "$pass_count"
printf '| Failed automated checks | %s |\n' "${summary_failed:-missing}"
printf '| Runbook/evidence consistency | %s |\n' "$consistency_status"
printf '\n'

printf '## Manual Rows\n\n'
printf '| Key | Item | Status | Reviewer | Evidence |\n'
printf '| --- | --- | --- | --- | --- |\n'

for index in "${!keys[@]}"; do
  key="${keys[$index]}"
  item="${items[$index]}"
  status="$(manual_row_cell "$item" 3)"
  reviewer="$(manual_row_cell "$item" 4)"
  evidence="$(manual_row_cell "$item" 5)"
  normalized="$(normalize_status "$status")"

  if [[ "$normalized" == "passed" && "$index" -lt 6 ]]; then
    accepted_prerequisites=$((accepted_prerequisites + 1))
  fi
  if [[ "$normalized" != "passed" ]]; then
    pending_count=$((pending_count + 1))
    if [[ "$next_index" -lt 0 ]]; then
      next_index="$index"
    fi
    if [[ "$normalized" == "failed" || "$normalized" == "rework" || "$normalized" == "unknown" ]]; then
      blocked_count=$((blocked_count + 1))
    fi
  fi

  printf '| `%s` | %s | %s | %s | %s |\n' "$key" "$item" "${status:-missing}" "$reviewer" "$evidence"
done

printf '\n'
printf '## Next Action\n\n'

if [[ "${summary_failed:-missing}" != "0" || "$pass_count" != "${summary_total:-}" || "$consistency_status" != "passed" ]]; then
  printf 'Automated or consistency gates are not ready for manual signoff. Refresh the delivery bundle before recording reviewer acceptance.\n'
  finish 1
fi

if [[ "$pending_count" -eq 0 ]]; then
  printf 'All manual rows are accepted. Run the finalization gate:\n\n'
  printf '```bash\n'
  printf 'scripts/verify-universal-forms-acceptance-signoff.sh %s\n' "$(shell_single_quote "$EVIDENCE_PATH")"
  printf 'scripts/finalize-universal-forms-acceptance.sh\n'
  printf 'scripts/audit-universal-forms-delivery-state.sh --strict\n'
  printf 'scripts/audit-universal-forms-delivery-bundle.sh\n'
  printf '```\n'
  finish 0
fi

if [[ "$blocked_count" -gt 0 ]]; then
  printf 'There are %s failed/rework/unknown manual row(s). Resolve or update those rows before final acceptance.\n\n' "$blocked_count"
fi

next_key="${keys[$next_index]}"
next_item="${items[$next_index]}"
if [[ "$next_key" == "overall" && "$accepted_prerequisites" -lt 6 ]]; then
  printf 'Overall acceptance is not actionable until the first six rows are accepted.\n'
  finish 1
fi

suggested_note="$(suggested_note_for "$next_item")"
if [[ -z "$suggested_note" ]]; then
  suggested_note="<review note>"
fi

printf 'Next row: `%s` - %s\n\n' "$next_key" "$next_item"
printf 'Suggested evidence note:\n\n'
printf '```text\n%s\n```\n\n' "$suggested_note"
printf 'Recorder command after reviewer approval:\n\n'
printf '```bash\n'
printf 'scripts/record-universal-forms-manual-signoff.sh --item %s --status accepted --reviewer %s --evidence %s\n' \
  "$next_key" \
  "$(shell_single_quote "$REVIEWER_PLACEHOLDER")" \
  "$(shell_single_quote "$suggested_note")"
printf '```\n\n'
printf 'Remaining manual rows before finalization: %s\n' "$pending_count"

finish 0
