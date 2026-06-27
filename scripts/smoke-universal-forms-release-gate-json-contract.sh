#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
JSON_PATH="${1:-}"
VERIFY="$ROOT_DIR/scripts/verify-universal-forms-release-gate-json.sh"
GENERATED_TMP=""

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-release-gate-json-contract.sh [JSON_PATH]

Runs negative contract checks for the release gate JSON verifier. When
JSON_PATH is omitted, the smoke generates the current pre-signoff release gate
JSON with scripts/gate-universal-forms-release.sh --allow-pending --json.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for release gate JSON contract smoke" >&2
  exit 2
fi

cleanup() {
  if [[ -n "$GENERATED_TMP" ]]; then
    rm -f "$GENERATED_TMP"
  fi
}
trap cleanup EXIT

if [[ -z "$JSON_PATH" ]]; then
  GENERATED_TMP="$(mktemp /tmp/openpr-uf-release-gate.XXXXXX.json)"
  "$ROOT_DIR/scripts/gate-universal-forms-release.sh" --allow-pending --json >"$GENERATED_TMP"
  JSON_PATH="$GENERATED_TMP"
fi

if [[ ! -f "$JSON_PATH" ]]; then
  echo "Release gate JSON not found: $JSON_PATH" >&2
  exit 2
fi

pass() {
  printf 'PASS: %s\n' "$1"
}

expect_reject() {
  local description="$1"
  local filter="$2"
  local tmp
  tmp="$(mktemp /tmp/openpr-uf-release-gate-bad.XXXXXX.json)"
  jq "$filter" "$JSON_PATH" >"$tmp"
  if "$VERIFY" "$tmp" >/dev/null 2>&1; then
    echo "FAIL: $description was accepted" >&2
    rm -f "$tmp"
    exit 1
  fi
  rm -f "$tmp"
  pass "$description is rejected by verifier"
}

printf 'Universal forms release gate JSON contract smoke\n'
printf '  JSON: %s\n' "$JSON_PATH"
printf '\n'

"$VERIFY" "$JSON_PATH" >/dev/null
pass "canonical release gate JSON passes verifier"

expect_reject "missing schema path" 'del(.schema_path)'
expect_reject "extra top-level property" '.unexpected = true'
expect_reject "extra reports property" '.reports.unexpected = true'
expect_reject "extra tracker status property" '.tracker_status.unexpected = true'
expect_reject "missing readiness report path" 'del(.reports.readiness_json)'
expect_reject "missing tracker status required key" 'del(.tracker_status.user_side_manual_acceptance)'
expect_reject "unknown mode" '.mode = "ready"'
expect_reject "string failed check counter" '.failed_automated_checks = "0"'
expect_reject "release allowed drift" '.release_allowed = true'
expect_reject "manual pending count drift" '.manual_signoff_pending_rows += 1'
expect_reject "next manual key drift" '.next_manual_signoff_key = "overall"'
expect_reject "unknown next manual key" '.next_manual_signoff_key = "unknown"'
expect_reject "manual final flag drift" '.manual_final_signoff_allowed = true'
expect_reject "development final flag drift" '.development_final_release_allowed = true'
expect_reject "tracker manual status drift" '.tracker_status.user_side_manual_acceptance = "已验收"'

printf '\nUniversal forms release gate JSON contract smoke passed.\n'
