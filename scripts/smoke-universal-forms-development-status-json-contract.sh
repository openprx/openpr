#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
JSON_PATH="${1:-$REPORT_DIR/openpr-universal-form-development-status-2026-05-31.json}"
VERIFY="$ROOT_DIR/scripts/verify-universal-forms-development-status-json.sh"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-development-status-json-contract.sh [JSON_PATH]

Runs negative contract checks for the development status JSON verifier. The
canonical JSON must pass; malformed copies must fail.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for development status JSON contract smoke" >&2
  exit 2
fi

if [[ ! -f "$JSON_PATH" ]]; then
  echo "Development status JSON not found: $JSON_PATH" >&2
  exit 2
fi

pass() {
  printf 'PASS: %s\n' "$1"
}

expect_reject() {
  local description="$1"
  local filter="$2"
  local tmp
  tmp="$(mktemp /tmp/openpr-uf-development-status.XXXXXX.json)"
  jq "$filter" "$JSON_PATH" >"$tmp"
  if "$VERIFY" "$tmp" >/dev/null 2>&1; then
    echo "FAIL: $description was accepted" >&2
    rm -f "$tmp"
    exit 1
  fi
  rm -f "$tmp"
  pass "$description is rejected by verifier"
}

printf 'Universal forms development status JSON contract smoke\n'
printf '  JSON: %s\n' "$JSON_PATH"
printf '\n'

"$VERIFY" "$JSON_PATH" >/dev/null
pass "canonical development status JSON passes verifier"

expect_reject "missing schema path" 'del(.schema_path)'
expect_reject "extra top-level property" '.unexpected = true'
expect_reject "extra status summary property" '.status_summary.unexpected = true'
expect_reject "extra row property" '.rows[0].unexpected = true'
expect_reject "string total row counter" '.status_summary.total_rows = "10"'
expect_reject "unknown row key" '.rows[0].key = "unknown"'
expect_reject "row order drift" '(.rows[0]) as $first | (.rows[1]) as $second | .rows[0] = $second | .rows[1] = $first'
expect_reject "invalid row status" '.rows[0].status = "已发布"'
expect_reject "empty row phase" '.rows[0].phase = ""'
expect_reject "row phase drift" '.rows[0].phase = "错误阶段"'
expect_reject "engineering requirement drift" '.rows[0].engineering_requirement = "changed requirement"'
expect_reject "completion rule drift" '.rows[0].completion_rule = "changed completion rule"'
expect_reject "manual pending count drift" '.status_summary.manual_signoff_pending_rows += 1'
expect_reject "row count drift" 'del(.rows[0])'

printf '\nUniversal forms development status JSON contract smoke passed.\n'
