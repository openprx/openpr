#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
SIGNOFF_STATUS_JSON_PATH="${1:-$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json}"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-manual-signoff-commands.sh [SIGNOFF_STATUS_JSON]

Verifies every machine-readable manual signoff row can be recorded on temporary
runbook/evidence copies. The smoke records the six prerequisite rows first,
then records overall acceptance and runs the final signoff verifier against the
temporary fully signed copies. Official handoff files are never modified.
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

printf 'Universal forms manual signoff command smoke\n'
printf '  signoff JSON: %s\n' "$SIGNOFF_STATUS_JSON_PATH"
printf '\n'

if command -v jq >/dev/null 2>&1; then
  pass "jq is available"
else
  fail "jq is available"
fi

for path in "$SIGNOFF_STATUS_JSON_PATH" "$RUNBOOK_PATH" "$EVIDENCE_PATH"; do
  require_file "$path"
done

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms manual signoff command smoke failed before JSON checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

total_rows="$(jq -r '.manual_signoff.total_rows' "$SIGNOFF_STATUS_JSON_PATH")"
row_keys="$(jq -r '.manual_signoff.rows[].key' "$SIGNOFF_STATUS_JSON_PATH")"

if [[ "$total_rows" == "7" ]]; then
  pass "manual signoff JSON has seven rows"
else
  fail "manual signoff JSON has seven rows"
fi

expected_order=$'restaurant_template\nfrontend_usability\namounts\nworkflow\nhub_consistency\ndocs\noverall'
if [[ "$row_keys" == "$expected_order" ]]; then
  pass "manual signoff row order preserves overall prerequisite rule"
else
  fail "manual signoff row order preserves overall prerequisite rule"
  printf '  actual order:\n%s\n' "$row_keys" >&2
fi

tmp_dir="$(mktemp -d /tmp/openpr-uf-all-signoff.XXXXXX)"
trap 'rm -rf "$tmp_dir"' EXIT
tmp_runbook="$tmp_dir/runbook.md"
tmp_evidence="$tmp_dir/evidence.md"
cp "$RUNBOOK_PATH" "$tmp_runbook"
cp "$EVIDENCE_PATH" "$tmp_evidence"

while IFS=$'\t' read -r key note command; do
  [[ -z "$key" ]] && continue
  if [[ -z "$note" || "$note" == "null" ]]; then
    fail "manual signoff row has suggested evidence note: $key"
    continue
  fi
  contains_text "recorder command targets row: $key" "$command" "--item $key"
  contains_text "recorder command records accepted status: $key" "$command" "--status accepted"

  if "$ROOT_DIR/scripts/record-universal-forms-manual-signoff.sh" \
    --runbook "$tmp_runbook" \
    --report "$tmp_evidence" \
    --item "$key" \
    --status accepted \
    --reviewer "All Signoff Smoke" \
    --evidence "$note" >/dev/null; then
    pass "manual signoff recorder writes temporary row: $key"
  else
    fail "manual signoff recorder writes temporary row: $key"
  fi
done < <(jq -r '.manual_signoff.rows[] | [.key, .suggested_evidence_note, .recorder_command] | @tsv' "$SIGNOFF_STATUS_JSON_PATH")

if "$ROOT_DIR/scripts/verify-universal-forms-acceptance-signoff.sh" \
  "$tmp_evidence" \
  --runbook "$tmp_runbook" >/dev/null; then
  pass "temporary fully signed runbook/evidence pass final signoff verifier"
else
  fail "temporary fully signed runbook/evidence pass final signoff verifier"
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
  printf '\nUniversal forms manual signoff command smoke passed.\n'
else
  printf '\nUniversal forms manual signoff command smoke failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi
