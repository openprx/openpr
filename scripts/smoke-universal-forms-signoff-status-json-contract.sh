#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
JSON_PATH="${1:-$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json}"
VERIFY="$ROOT_DIR/scripts/verify-universal-forms-signoff-status-json.sh"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-signoff-status-json-contract.sh [JSON_PATH]

Runs negative contract checks for the manual signoff status JSON verifier. The
canonical JSON must pass; malformed copies must fail.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for signoff status JSON contract smoke" >&2
  exit 2
fi

if [[ ! -f "$JSON_PATH" ]]; then
  echo "Signoff status JSON not found: $JSON_PATH" >&2
  exit 2
fi

pass() {
  printf 'PASS: %s\n' "$1"
}

expect_reject() {
  local description="$1"
  local filter="$2"
  local tmp
  tmp="$(mktemp /tmp/openpr-uf-signoff-status.XXXXXX.json)"
  jq "$filter" "$JSON_PATH" >"$tmp"
  if "$VERIFY" "$tmp" >/dev/null 2>&1; then
    echo "FAIL: $description was accepted" >&2
    rm -f "$tmp"
    exit 1
  fi
  rm -f "$tmp"
  pass "$description is rejected by verifier"
}

printf 'Universal forms signoff status JSON contract smoke\n'
printf '  JSON: %s\n' "$JSON_PATH"
printf '\n'

"$VERIFY" "$JSON_PATH" >/dev/null
pass "canonical signoff status JSON passes verifier"

expect_reject "missing schema path" 'del(.schema_path)'
expect_reject "extra top-level property" '.unexpected = true'
expect_reject "extra reports property" '.reports.unexpected = true'
expect_reject "extra gate summary property" '.gate_summary.unexpected = true'
expect_reject "extra manual signoff property" '.manual_signoff.unexpected = true'
expect_reject "extra pending queue property" '.manual_signoff.pending_queue[0].unexpected = true'
expect_reject "extra next row property" '.manual_signoff.next_row.unexpected = true'
expect_reject "extra manual row property" '.manual_signoff.rows[0].unexpected = true'
expect_reject "extra release requirement property" '.release_requirement.unexpected = true'
expect_reject "string pending row counter" '.manual_signoff.pending_rows = "7"'
expect_reject "unknown manual row key" '.manual_signoff.rows[0].key = "unknown"'
expect_reject "manual row order drift" '(.manual_signoff.rows[0]) as $first | (.manual_signoff.rows[1]) as $second | .manual_signoff.rows[0] = $second | .manual_signoff.rows[1] = $first'
expect_reject "unknown next manual row key" '.manual_signoff.next_row.key = "unknown"'
expect_reject "invalid manual row status" '.manual_signoff.rows[0].status = "waiting"'
expect_reject "pending count drift" '.manual_signoff.pending_rows += 1'
expect_reject "missing pending queue" 'del(.manual_signoff.pending_queue)'
expect_reject "pending queue deletion" 'del(.manual_signoff.pending_queue[0])'
expect_reject "pending queue order drift" '(.manual_signoff.pending_queue[0]) as $first | (.manual_signoff.pending_queue[1]) as $second | .manual_signoff.pending_queue[0] = $second | .manual_signoff.pending_queue[1] = $first'
expect_reject "pending queue next marker drift" '.manual_signoff.pending_queue[0].is_next = false'
expect_reject "pending queue actionable drift" '.manual_signoff.pending_queue[0].actionable = false'
expect_reject "pending queue reviewer check drift" '.manual_signoff.pending_queue[0].reviewer_check = "wrong reviewer check"'
expect_reject "final signoff flag drift" '.manual_signoff.final_signoff_allowed = true'
expect_reject "missing per-row automated evidence" 'del(.manual_signoff.rows[0].automated_evidence)'
expect_reject "per-row automated evidence drift" '.manual_signoff.rows[0].automated_evidence = "wrong evidence"'
expect_reject "missing per-row reviewer check" 'del(.manual_signoff.rows[0].reviewer_check)'
expect_reject "per-row reviewer check drift" '.manual_signoff.rows[0].reviewer_check = "wrong reviewer check"'
expect_reject "missing next-row automated evidence" 'del(.manual_signoff.next_row.automated_evidence)'
expect_reject "next-row automated evidence drift" '.manual_signoff.next_row.automated_evidence = "wrong evidence"'
expect_reject "missing next-row reviewer check" 'del(.manual_signoff.next_row.reviewer_check)'
expect_reject "next-row reviewer check drift" '.manual_signoff.next_row.reviewer_check = "wrong reviewer check"'
expect_reject "missing per-row recorder command" 'del(.manual_signoff.rows[0].recorder_command)'
expect_reject "per-row recorder command drift" '.manual_signoff.rows[0].recorder_command = "scripts/record-universal-forms-manual-signoff.sh --item wrong --status accepted"'
expect_reject "row deletion" 'del(.manual_signoff.rows[0])'

printf '\nUniversal forms signoff status JSON contract smoke passed.\n'
