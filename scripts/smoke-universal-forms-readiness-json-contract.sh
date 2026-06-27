#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
READINESS_JSON_PATH="${1:-$REPORT_DIR/openpr-universal-form-readiness-2026-05-31.json}"
VERIFIER_PATH="$ROOT_DIR/scripts/verify-universal-forms-readiness-json.sh"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-readiness-json-contract.sh [JSON_PATH]

Runs a negative contract smoke for the machine-readable universal forms
readiness JSON. The canonical JSON must pass verification; selected malformed
temporary copies must be rejected by the verifier.
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

if [[ ! -f "$READINESS_JSON_PATH" ]]; then
  echo "Readiness JSON not found: $READINESS_JSON_PATH" >&2
  exit 2
fi
if [[ ! -x "$VERIFIER_PATH" ]]; then
  echo "Readiness JSON verifier is not executable: $VERIFIER_PATH" >&2
  exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "Missing required command: jq" >&2
  exit 2
fi

printf 'Universal forms readiness JSON contract smoke\n'
printf '  JSON: %s\n' "$READINESS_JSON_PATH"
printf '\n'

if "$VERIFIER_PATH" "$READINESS_JSON_PATH" >/dev/null; then
  pass "canonical readiness JSON passes verifier"
else
  fail "canonical readiness JSON passes verifier"
fi

tmp_dir="$(mktemp -d /tmp/openpr-uf-readiness-json-contract.XXXXXX)"
trap 'rm -rf "$tmp_dir"' EXIT

expect_reject() {
  local label="$1"
  local filter="$2"
  local candidate="$tmp_dir/${label//[^A-Za-z0-9_]/_}.json"

  if ! jq "$filter" "$READINESS_JSON_PATH" >"$candidate"; then
    fail "$label mutation can be generated"
    return
  fi

  if "$VERIFIER_PATH" "$candidate" >/dev/null 2>&1; then
    fail "$label is rejected by verifier"
  else
    pass "$label is rejected by verifier"
  fi
}

expect_reject "missing next-row recorder command" 'del(.manual_signoff.next_row.recorder_command)'
expect_reject "missing signoff status JSON report path" 'del(.reports.signoff_status_json)'
expect_reject "extra top-level property" '.unexpected_contract_field = true'
expect_reject "extra reports property" '.reports.unexpected = true'
expect_reject "extra gates property" '.gates.unexpected = true'
expect_reject "extra tracker status property" '.tracker_status.unexpected = true'
expect_reject "extra manual signoff property" '.manual_signoff.unexpected = true'
expect_reject "extra next row property" '.manual_signoff.next_row.unexpected = true'
expect_reject "extra manual row property" '.manual_signoff.rows[0].unexpected = true'
expect_reject "extra release requirement property" '.release_requirement.unexpected = true'
expect_reject "string automated check counter" '.gates.automated_checks = "26"'
expect_reject "unknown manual signoff key" '.manual_signoff.rows[0].key = "unknown"'
expect_reject "manual row order drift" '(.manual_signoff.rows[0]) as $first | (.manual_signoff.rows[1]) as $second | .manual_signoff.rows[0] = $second | .manual_signoff.rows[1] = $first'
expect_reject "unknown next manual row key" '.manual_signoff.next_row.key = "unknown"'
expect_reject "invalid stage enum" '.stage = "Somewhere else"'
expect_reject "release requirement constant drift" '.release_requirement.stage = "Ready for user-side manual signoff"'

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms readiness JSON contract smoke failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms readiness JSON contract smoke passed.\n'
