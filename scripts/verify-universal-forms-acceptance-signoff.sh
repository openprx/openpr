#!/usr/bin/env bash
set -euo pipefail

REPORT_PATH="/opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="${OPENPR_UNIVERSAL_FORMS_RUNBOOK:-/opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md}"

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-acceptance-signoff.sh [REPORT_PATH] [--runbook PATH]

Verifies that the generated universal forms acceptance evidence report is ready
for final tracker signoff:
  - automated checks total is present
  - automated checks failed count is 0
  - every automated check status line is PASS
  - Manual Acceptance Signoff section exists
  - every signoff row has a passing status
  - no signoff row is still pending, failed, or marked for rework
  - the runbook exists and its conclusion table matches the evidence signoff table

Environment:
  OPENPR_UNIVERSAL_FORMS_RUNBOOK  Runbook path override for consistency checks.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --runbook)
      RUNBOOK_PATH="${2:-}"
      if [[ -z "$RUNBOOK_PATH" ]]; then
        echo "--runbook requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    --*)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
    *)
      REPORT_PATH="$1"
      shift
      ;;
  esac
done

if [[ ! -f "$REPORT_PATH" ]]; then
  echo "Acceptance evidence report not found: $REPORT_PATH" >&2
  exit 2
fi
if [[ ! -f "$RUNBOOK_PATH" ]]; then
  echo "Runbook not found for final signoff consistency check: $RUNBOOK_PATH" >&2
  exit 2
fi

failed_checks="$(sed -n 's/^- Failed automated checks: //p' "$REPORT_PATH" | tail -n 1)"
total_checks="$(sed -n 's/^- Total automated checks: //p' "$REPORT_PATH" | tail -n 1)"
pass_count="$(rg -c '^\*\*Status:\*\* PASS$' "$REPORT_PATH" || true)"

if [[ -z "$total_checks" || ! "$total_checks" =~ ^[0-9]+$ ]]; then
  echo "Automated checks total is missing or invalid: ${total_checks:-missing}" >&2
  exit 1
fi

if [[ "$failed_checks" != "0" ]]; then
  echo "Automated checks are not clean: Failed automated checks = ${failed_checks:-missing}" >&2
  exit 1
fi

if [[ "$pass_count" != "$total_checks" ]]; then
  echo "Automated checks are not all PASS: Total automated checks = $total_checks, PASS status lines = $pass_count" >&2
  exit 1
fi

manual_section="$(awk '
  /^## Manual Acceptance Signoff$/ { in_section = 1; next }
  /^## / && in_section { exit }
  in_section { print }
' "$REPORT_PATH")"

if [[ -z "$manual_section" ]]; then
  echo "Manual Acceptance Signoff section is missing" >&2
  exit 1
fi

rows="$(printf '%s\n' "$manual_section" | awk -F'|' '
  NF >= 5 {
    item = $2
    status = $3
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", item)
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", status)
    if (item != "Item" && item !~ /^-+$/ && item != "") {
      print item "|" status
    }
  }
')"

if [[ -z "$rows" ]]; then
  echo "Manual Acceptance Signoff contains no rows" >&2
  exit 1
fi

bad_rows=0
while IFS='|' read -r item status; do
  normalized="$(printf '%s' "$status" | tr '[:upper:]' '[:lower:]')"
  case "$normalized" in
    pass|passed|approved|accepted|通过|已通过|已验收)
      ;;
    *)
      echo "Manual acceptance row is not passed: $item => ${status:-missing}" >&2
      bad_rows=$((bad_rows + 1))
      ;;
  esac
done <<<"$rows"

if [[ "$bad_rows" -ne 0 ]]; then
  echo "Manual acceptance signoff is incomplete: $bad_rows row(s) not passed" >&2
  exit 1
fi

"$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/scripts/verify-universal-forms-manual-signoff-consistency.sh" "$RUNBOOK_PATH" "$REPORT_PATH" >/dev/null

echo "universal forms manual acceptance signoff verified"
