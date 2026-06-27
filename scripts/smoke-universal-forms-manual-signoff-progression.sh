#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
MANUAL_EVIDENCE_MAP_PATH="$REPORT_DIR/openpr-universal-form-manual-evidence-map-2026-05-31.md"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-manual-signoff-progression.sh

Verifies the manual signoff journey one row at a time on temporary
runbook/evidence copies. After each accepted row, the smoke regenerates the
signoff status JSON and checks accepted/pending counts, next row key,
actionable flag, and final signoff flag. Official handoff files are never
modified.
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

run_quiet() {
  local label="$1"
  shift

  local stdout_path="$tmp_dir/${label//[^A-Za-z0-9_]/_}.stdout"
  local stderr_path="$tmp_dir/${label//[^A-Za-z0-9_]/_}.stderr"

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

equals() {
  local description="$1"
  local actual="$2"
  local expected="$3"
  if [[ "$actual" == "$expected" ]]; then
    pass "$description"
  else
    fail "$description"
    printf '  expected: %s\n  actual: %s\n' "$expected" "${actual:-<empty>}" >&2
  fi
}

json_value() {
  local path="$1"
  local expression="$2"
  jq -r "$expression" "$path"
}

generate_status_json() {
  local tmp_runbook="$1"
  local tmp_evidence="$2"
  local tmp_markdown="$3"
  local tmp_json="$4"

  run_quiet "generate signoff status JSON" \
    "$ROOT_DIR/scripts/report-universal-forms-signoff-status-json.sh" \
    --runbook "$tmp_runbook" \
    --report "$tmp_evidence" \
    --manual-map "$MANUAL_EVIDENCE_MAP_PATH" \
    --markdown "$tmp_markdown" \
    --reviewer "Progression Smoke" \
    --output "$tmp_json"
}

verify_state() {
  local tmp_json="$1"
  local accepted="$2"
  local expected_next="$3"
  local label="$4"
  local pending=$((7 - accepted))
  local final_allowed="false"

  if [[ "$accepted" -eq 7 ]]; then
    final_allowed="true"
  fi

  equals "$label accepted row count" "$(json_value "$tmp_json" '.manual_signoff.accepted_rows')" "$accepted"
  equals "$label pending row count" "$(json_value "$tmp_json" '.manual_signoff.pending_rows')" "$pending"
  equals "$label blocked row count" "$(json_value "$tmp_json" '.manual_signoff.blocked_rows')" "0"
  equals "$label final signoff flag" "$(json_value "$tmp_json" '.manual_signoff.final_signoff_allowed')" "$final_allowed"
  equals "$label next row key" "$(json_value "$tmp_json" '.manual_signoff.next_row.key // ""')" "$expected_next"

  if [[ "$pending" -gt 0 ]]; then
    equals "$label next row actionable" "$(json_value "$tmp_json" '.manual_signoff.next_row.actionable')" "true"
    if [[ -n "$(json_value "$tmp_json" '.manual_signoff.next_row.recorder_command // ""')" ]]; then
      pass "$label next row recorder command is present"
    else
      fail "$label next row recorder command is present"
    fi
  else
    equals "$label next row item is empty after completion" "$(json_value "$tmp_json" '.manual_signoff.next_row.item // ""')" ""
    equals "$label next row recorder command is empty after completion" "$(json_value "$tmp_json" '.manual_signoff.next_row.recorder_command // ""')" ""
  fi
}

printf 'Universal forms manual signoff progression smoke\n'
printf '  runbook: %s\n' "$RUNBOOK_PATH"
printf '  evidence: %s\n' "$EVIDENCE_PATH"
printf '\n'

if command -v jq >/dev/null 2>&1; then
  pass "jq is available"
else
  fail "jq is available"
fi

for path in "$RUNBOOK_PATH" "$EVIDENCE_PATH" "$MANUAL_EVIDENCE_MAP_PATH"; do
  require_file "$path"
done

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms manual signoff progression smoke failed before progression checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

runbook_before="$(sha256sum "$RUNBOOK_PATH" | awk '{print $1}')"
evidence_before="$(sha256sum "$EVIDENCE_PATH" | awk '{print $1}')"

tmp_dir="$(mktemp -d /tmp/openpr-uf-signoff-progression.XXXXXX)"
trap 'rm -rf "$tmp_dir"' EXIT
tmp_runbook="$tmp_dir/runbook.md"
tmp_evidence="$tmp_dir/evidence.md"
tmp_markdown="$tmp_dir/signoff-status.md"
tmp_json="$tmp_dir/signoff-status.json"
cp "$RUNBOOK_PATH" "$tmp_runbook"
cp "$EVIDENCE_PATH" "$tmp_evidence"

keys=(
  restaurant_template
  frontend_usability
  amounts
  workflow
  hub_consistency
  docs
  overall
)

next_keys=(
  restaurant_template
  frontend_usability
  amounts
  workflow
  hub_consistency
  docs
  overall
  ""
)

generate_status_json "$tmp_runbook" "$tmp_evidence" "$tmp_markdown" "$tmp_json"
verify_state "$tmp_json" 0 "${next_keys[0]}" "initial state"

for index in "${!keys[@]}"; do
  key="${keys[$index]}"
  note="$(json_value "$tmp_json" ".manual_signoff.rows[] | select(.key == \"$key\") | .suggested_evidence_note")"
  if [[ -z "$note" || "$note" == "null" ]]; then
    fail "suggested evidence note is present before recording $key"
    note="progression smoke evidence"
  else
    pass "suggested evidence note is present before recording $key"
  fi

  if run_quiet "record manual signoff $key" \
    "$ROOT_DIR/scripts/record-universal-forms-manual-signoff.sh" \
    --runbook "$tmp_runbook" \
    --report "$tmp_evidence" \
    --item "$key" \
    --status accepted \
    --reviewer "Progression Smoke" \
    --evidence "$note" >/dev/null; then
    pass "manual signoff recorder advances temporary row: $key"
  else
    fail "manual signoff recorder advances temporary row: $key"
  fi

  generate_status_json "$tmp_runbook" "$tmp_evidence" "$tmp_markdown" "$tmp_json"
  verify_state "$tmp_json" $((index + 1)) "${next_keys[index + 1]}" "after $key"
done

if run_quiet "verify temporary final signoff" \
  "$ROOT_DIR/scripts/verify-universal-forms-acceptance-signoff.sh" \
  "$tmp_evidence" \
  --runbook "$tmp_runbook"; then
  pass "temporary fully progressed signoff passes final verifier"
else
  fail "temporary fully progressed signoff passes final verifier"
fi

runbook_after="$(sha256sum "$RUNBOOK_PATH" | awk '{print $1}')"
evidence_after="$(sha256sum "$EVIDENCE_PATH" | awk '{print $1}')"
equals "official runbook remains unchanged" "$runbook_after" "$runbook_before"
equals "official evidence remains unchanged" "$evidence_after" "$evidence_before"

if [[ "$failures" -eq 0 ]]; then
  printf '\nUniversal forms manual signoff progression smoke passed.\n'
else
  printf '\nUniversal forms manual signoff progression smoke failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi
