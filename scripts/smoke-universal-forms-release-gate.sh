#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
READINESS_JSON_PATH="$REPORT_DIR/openpr-universal-form-readiness-2026-05-31.json"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-release-gate.sh

Exercises the read-only universal forms release gate in the current handoff
state. The smoke confirms that --allow-pending succeeds for the pre-signoff
handoff, strict mode rejects pending manual signoff, and JSON output exposes
the fields release automation needs.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "Missing required command: jq" >&2
  exit 2
fi

if [[ ! -f "$READINESS_JSON_PATH" ]]; then
  echo "Missing readiness JSON: $READINESS_JSON_PATH" >&2
  exit 1
fi

failures=0

pass() {
  printf 'PASS: %s\n' "$1"
}

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  failures=$((failures + 1))
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

printf 'Universal forms release gate smoke\n'
printf '  repo: %s\n' "$ROOT_DIR"
printf '\n'

pending_rows="$(jq -r '.manual_signoff.pending_rows' "$READINESS_JSON_PATH")"
stage="$(jq -r '.stage' "$READINESS_JSON_PATH")"

allow_output="$("$ROOT_DIR/scripts/gate-universal-forms-release.sh" --allow-pending)"
if [[ "$pending_rows" -gt 0 && "$stage" == "Ready for user-side manual signoff" ]]; then
  if rg -q --fixed-strings -- "mode: pre_signoff" <<<"$allow_output"; then
    pass "allow-pending release gate reports pre-signoff mode"
  else
    fail "allow-pending release gate reports pre-signoff mode"
  fi
  if rg -q --fixed-strings -- "release_allowed: false" <<<"$allow_output"; then
    pass "allow-pending release gate keeps final release disabled"
  else
    fail "allow-pending release gate keeps final release disabled"
  fi
else
  if rg -q --fixed-strings -- "mode: release" <<<"$allow_output"; then
    pass "allow-pending release gate passes through finalized release mode"
  else
    fail "allow-pending release gate passes through finalized release mode"
  fi
fi

json_output="$("$ROOT_DIR/scripts/gate-universal-forms-release.sh" --allow-pending --json)"
if jq empty <<<"$json_output" >/dev/null 2>&1; then
  pass "release gate JSON output is valid JSON"
else
  fail "release gate JSON output is valid JSON"
fi

equals "release gate JSON failed checks matches readiness" \
  "$(jq -r '.failed_automated_checks' <<<"$json_output")" \
  "$(jq -r '.gates.failed_automated_checks' "$READINESS_JSON_PATH")"
equals "release gate JSON pending rows matches readiness" \
  "$(jq -r '.manual_signoff_pending_rows' <<<"$json_output")" \
  "$pending_rows"
equals "release gate JSON next key matches readiness" \
  "$(jq -r '.next_manual_signoff_key' <<<"$json_output")" \
  "$(jq -r '.manual_signoff.next_row.key // ""' "$READINESS_JSON_PATH")"
equals "release gate JSON final flag matches readiness" \
  "$(jq -r '.final_acceptance_complete' <<<"$json_output")" \
  "$(jq -r '.final_acceptance_complete' "$READINESS_JSON_PATH")"
if "$ROOT_DIR/scripts/verify-universal-forms-release-gate-json.sh" >/dev/null; then
  pass "release gate JSON verifier passes"
else
  fail "release gate JSON verifier passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-release-gate-json-contract.sh" >/dev/null; then
  pass "release gate JSON contract smoke passes"
else
  fail "release gate JSON contract smoke passes"
fi

strict_output="$(mktemp /tmp/openpr-uf-release-gate-strict.XXXXXX)"
trap 'rm -f "$strict_output"' EXIT
if "$ROOT_DIR/scripts/gate-universal-forms-release.sh" >"$strict_output" 2>&1; then
  if [[ "$pending_rows" == "0" ]]; then
    pass "strict release gate passes finalized handoff"
  else
    fail "strict release gate rejects pending manual signoff"
  fi
else
  if [[ "$pending_rows" -gt 0 ]]; then
    pass "strict release gate rejects pending manual signoff"
    if rg -q --fixed-strings -- "user-side manual signoff is incomplete" "$strict_output"; then
      pass "strict release gate explains pending manual signoff"
    else
      fail "strict release gate explains pending manual signoff"
    fi
  else
    fail "strict release gate passes finalized handoff"
  fi
fi

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms release gate smoke failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms release gate smoke passed.\n'
