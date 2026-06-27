#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
SIGNOFF_STATUS_JSON_PATH="${1:-$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json}"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
MARKDOWN_STATUS_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.md"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-next-signoff-command.sh [SIGNOFF_STATUS_JSON]

Verifies that the machine-readable next manual signoff row is actionable:
the JSON next-row command matches the Markdown status report, and the recorder
can dry-run that row against temporary runbook/evidence copies without touching
the official handoff files.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

failures=0

pass() {
  printf 'PASS: %s\n' "$1"
}

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  failures=$((failures + 1))
}

require_file() {
  local path="$1"
  if [[ -f "$path" ]]; then
    pass "file exists: $path"
  else
    fail "file missing: $path"
  fi
}

contains_file() {
  local description="$1"
  local path="$2"
  local needle="$3"
  if rg -q --fixed-strings -- "$needle" "$path"; then
    pass "$description"
  else
    fail "$description"
    printf '  missing in %s: %s\n' "$path" "$needle" >&2
  fi
}

contains_text() {
  local description="$1"
  local haystack="$2"
  local needle="$3"
  if [[ "$haystack" == *"$needle"* ]]; then
    pass "$description"
  else
    fail "$description"
    printf '  missing text: %s\n' "$needle" >&2
  fi
}

printf 'Universal forms next signoff command smoke\n'
printf '  signoff JSON: %s\n' "$SIGNOFF_STATUS_JSON_PATH"
printf '\n'

if command -v jq >/dev/null 2>&1; then
  pass "jq is available"
else
  fail "jq is available"
fi

for path in "$SIGNOFF_STATUS_JSON_PATH" "$EVIDENCE_PATH" "$RUNBOOK_PATH" "$MARKDOWN_STATUS_PATH"; do
  require_file "$path"
done

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms next signoff command smoke failed before JSON checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

pending_rows="$(jq -r '.manual_signoff.pending_rows' "$SIGNOFF_STATUS_JSON_PATH")"
final_allowed="$(jq -r '.manual_signoff.final_signoff_allowed' "$SIGNOFF_STATUS_JSON_PATH")"

if [[ "$pending_rows" == "0" ]]; then
  if [[ "$final_allowed" == "true" ]]; then
    pass "no pending rows leaves final signoff allowed"
  else
    fail "no pending rows leaves final signoff allowed"
  fi
  if [[ "$failures" -eq 0 ]]; then
    printf '\nUniversal forms next signoff command smoke passed.\n'
    exit 0
  fi
  printf '\nUniversal forms next signoff command smoke failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

next_key="$(jq -r '.manual_signoff.next_row.key // empty' "$SIGNOFF_STATUS_JSON_PATH")"
next_item="$(jq -r '.manual_signoff.next_row.item // empty' "$SIGNOFF_STATUS_JSON_PATH")"
next_actionable="$(jq -r '.manual_signoff.next_row.actionable // false' "$SIGNOFF_STATUS_JSON_PATH")"
suggested_note="$(jq -r '.manual_signoff.next_row.suggested_evidence_note // empty' "$SIGNOFF_STATUS_JSON_PATH")"
recorder_command="$(jq -r '.manual_signoff.next_row.recorder_command // empty' "$SIGNOFF_STATUS_JSON_PATH")"

if [[ -n "$next_key" ]]; then
  pass "next manual signoff key is present"
else
  fail "next manual signoff key is present"
fi
if [[ -n "$next_item" ]]; then
  pass "next manual signoff item is present"
else
  fail "next manual signoff item is present"
fi
if [[ "$next_actionable" == "true" ]]; then
  pass "next manual signoff row is actionable"
else
  fail "next manual signoff row is actionable"
fi
if [[ -n "$suggested_note" ]]; then
  pass "next manual signoff suggested evidence note is present"
else
  fail "next manual signoff suggested evidence note is present"
fi
contains_text "next recorder command targets next key" "$recorder_command" "--item $next_key"
contains_text "next recorder command records accepted status" "$recorder_command" "--status accepted"
contains_text "next recorder command includes evidence flag" "$recorder_command" "--evidence"
contains_file "Markdown signoff status mirrors next key" "$MARKDOWN_STATUS_PATH" "Next row: \`$next_key\`"

tmp_dir="$(mktemp -d /tmp/openpr-uf-next-signoff.XXXXXX)"
trap 'rm -rf "$tmp_dir"' EXIT
tmp_runbook="$tmp_dir/runbook.md"
tmp_evidence="$tmp_dir/evidence.md"
cp "$RUNBOOK_PATH" "$tmp_runbook"
cp "$EVIDENCE_PATH" "$tmp_evidence"

if "$ROOT_DIR/scripts/record-universal-forms-manual-signoff.sh" \
  --runbook "$tmp_runbook" \
  --report "$tmp_evidence" \
  --item "$next_key" \
  --status accepted \
  --reviewer "Next Signoff Smoke" \
  --evidence "$suggested_note" \
  --dry-run >/dev/null; then
  pass "next recorder command dry-runs against temporary copies"
else
  fail "next recorder command dry-runs against temporary copies"
fi

if cmp -s "$RUNBOOK_PATH" "$tmp_runbook"; then
  pass "dry-run leaves temporary runbook input unchanged"
else
  fail "dry-run leaves temporary runbook input unchanged"
fi
if cmp -s "$EVIDENCE_PATH" "$tmp_evidence"; then
  pass "dry-run leaves temporary evidence input unchanged"
else
  fail "dry-run leaves temporary evidence input unchanged"
fi
if cmp -s "$RUNBOOK_PATH" "$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"; then
  pass "official runbook path remains unchanged"
else
  fail "official runbook path remains unchanged"
fi
if cmp -s "$EVIDENCE_PATH" "$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"; then
  pass "official evidence path remains unchanged"
else
  fail "official evidence path remains unchanged"
fi

if [[ "$failures" -eq 0 ]]; then
  printf '\nUniversal forms next signoff command smoke passed.\n'
else
  printf '\nUniversal forms next signoff command smoke failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi
