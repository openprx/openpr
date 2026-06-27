#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
JSON_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json"
STATUS_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.md"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-signoff-status-output.sh

Verifies that the reviewer-facing manual signoff Markdown status report mirrors
the machine-readable signoff status JSON. This is read-only and never records a
manual signoff row.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if [[ $# -gt 0 ]]; then
  echo "Unknown argument: $1" >&2
  usage >&2
  exit 2
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for signoff status output smoke" >&2
  exit 2
fi

pass() {
  printf 'PASS: %s\n' "$1"
}

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

run_quiet() {
  local label="$1"
  shift

  local stdout_path stderr_path
  stdout_path="$(mktemp /tmp/openpr-uf-signoff-status-output-${label//[^A-Za-z0-9_]/_}.stdout.XXXXXX)"
  stderr_path="$(mktemp /tmp/openpr-uf-signoff-status-output-${label//[^A-Za-z0-9_]/_}.stderr.XXXXXX)"

  if "$@" >"$stdout_path" 2>"$stderr_path"; then
    rm -f "$stdout_path" "$stderr_path"
    return 0
  fi

  printf 'Command failed during %s\n' "$label" >&2
  if [[ -s "$stdout_path" ]]; then
    printf '  stdout:\n' >&2
    sed 's/^/    /' "$stdout_path" >&2
  fi
  if [[ -s "$stderr_path" ]]; then
    printf '  stderr:\n' >&2
    sed 's/^/    /' "$stderr_path" >&2
  fi
  rm -f "$stdout_path" "$stderr_path"
  return 1
}

require_file() {
  local path="$1"
  if [[ -f "$path" ]]; then
    pass "file exists: $path"
  else
    fail "file missing: $path"
  fi
}

contains() {
  local description="$1"
  local path="$2"
  local needle="$3"
  if rg -Fq -- "$needle" "$path"; then
    pass "$description"
  else
    printf 'Expected to find in %s:\n%s\n' "$path" "$needle" >&2
    fail "$description"
  fi
}

not_contains() {
  local description="$1"
  local path="$2"
  local needle="$3"
  if rg -Fq -- "$needle" "$path"; then
    printf 'Unexpected text found in %s:\n%s\n' "$path" "$needle" >&2
    fail "$description"
  else
    pass "$description"
  fi
}

json_value() {
  jq -r "$1" "$JSON_PATH"
}

validate_status_report() {
  local path="$1"
  local label="$2"

  require_file "$path"
  contains "$label starts with Markdown heading" "$path" "# OpenPR Universal Forms Manual Signoff Status"
  contains "$label mirrors automated checks" "$path" "| Automated checks | $(json_value '.gate_summary.automated_checks') |"
  contains "$label mirrors PASS status lines" "$path" "| PASS status lines | $(json_value '.gate_summary.pass_status_lines') |"
  contains "$label mirrors failed checks" "$path" "| Failed automated checks | $(json_value '.gate_summary.failed_automated_checks') |"
  contains "$label mirrors consistency gate" "$path" "| Runbook/evidence consistency | $(json_value '.gate_summary.runbook_evidence_consistency') |"

  local total_rows
  total_rows="$(json_value '.manual_signoff.total_rows')"
  local row_count
  row_count="$(
    awk -F'|' '
      /^## Manual Rows$/ { in_section = 1; next }
      /^## / && in_section { exit }
      in_section && NF >= 6 {
        key = $2
        gsub(/^[[:space:]]+|[[:space:]]+$/, "", key)
        if (key ~ /^`/) {
          count += 1
        }
      }
      END { print count + 0 }
    ' "$path"
  )"
  if [[ "$row_count" == "$total_rows" ]]; then
    pass "$label row count mirrors JSON"
  else
    printf 'Expected %s manual rows in %s, found %s\n' "$total_rows" "$path" "$row_count" >&2
    fail "$label row count mirrors JSON"
  fi

  while IFS=$'\t' read -r key item raw_status reviewer evidence; do
    contains "$label mirrors row $key" "$path" "| \`$key\` | $item | $raw_status | $reviewer | $evidence |"
  done < <(
    jq -r '.manual_signoff.rows[] | [.key, .item, .raw_status, .reviewer, .evidence] | @tsv' "$JSON_PATH"
  )

  local pending_rows
  pending_rows="$(json_value '.manual_signoff.pending_rows')"
  local final_allowed
  final_allowed="$(json_value '.manual_signoff.final_signoff_allowed')"
  local next_key
  next_key="$(json_value '.manual_signoff.next_row.key')"

  if [[ "$pending_rows" -gt 0 ]]; then
    contains "$label mirrors next row key and item" "$path" "Next row: \`$next_key\` - $(json_value '.manual_signoff.next_row.item')"
    contains "$label mirrors suggested evidence note" "$path" "$(json_value '.manual_signoff.next_row.suggested_evidence_note')"
    contains "$label mirrors recorder command" "$path" "$(json_value '.manual_signoff.next_row.recorder_command')"
    contains "$label mirrors pending row count" "$path" "Remaining manual rows before finalization: $pending_rows"
    not_contains "$label does not expose finalizer before final signoff" "$path" "scripts/finalize-universal-forms-acceptance.sh"
  elif [[ "$final_allowed" == "true" ]]; then
    contains "$label exposes finalizer after all rows accepted" "$path" "scripts/finalize-universal-forms-acceptance.sh"
    contains "$label exposes strict delivery-state audit after all rows accepted" "$path" "scripts/audit-universal-forms-delivery-state.sh --strict"
    not_contains "$label omits next-row prompt after final signoff" "$path" "Next row:"
    not_contains "$label omits recorder command after final signoff" "$path" "Recorder command after reviewer approval:"
  else
    contains "$label reports blocked gate when not actionable" "$path" "Automated or consistency gates are not ready for manual signoff."
  fi

  not_contains "$label does not leak JSON nulls" "$path" "null"
}

require_file "$JSON_PATH"
require_file "$STATUS_PATH"

printf 'Universal forms signoff status output smoke\n'
printf '  JSON: %s\n' "$JSON_PATH"
printf '  Markdown: %s\n' "$STATUS_PATH"
printf '\n'

run_quiet "verify canonical signoff status JSON" \
  "$ROOT_DIR/scripts/verify-universal-forms-signoff-status-json.sh" "$JSON_PATH"
pass "canonical signoff status JSON passes verifier"

tmp_status="$(mktemp /tmp/openpr-uf-signoff-status-output.XXXXXX.md)"
trap 'rm -f "$tmp_status"' EXIT
run_quiet "render temporary signoff status Markdown" \
  "$ROOT_DIR/scripts/report-universal-forms-signoff-status.sh" --output "$tmp_status"
pass "signoff status reporter can render temporary Markdown"

validate_status_report "$STATUS_PATH" "official signoff status output"
validate_status_report "$tmp_status" "generated signoff status output"

printf '\nUniversal forms signoff status output smoke passed.\n'
