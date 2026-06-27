#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
JSON_PATH="${1:-$REPORT_DIR/openpr-universal-form-completion-audit-2026-05-31.json}"
VERIFY="$ROOT_DIR/scripts/verify-universal-forms-completion-audit-json.sh"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-completion-audit-json-contract.sh [JSON_PATH]

Runs negative contract checks for the completion audit JSON verifier. The
canonical JSON must pass; malformed copies must fail.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for completion audit JSON contract smoke" >&2
  exit 2
fi

if [[ ! -f "$JSON_PATH" ]]; then
  echo "Completion audit JSON not found: $JSON_PATH" >&2
  exit 2
fi

pass() {
  printf 'PASS: %s\n' "$1"
}

expect_reject() {
  local description="$1"
  local filter="$2"
  local tmp
  tmp="$(mktemp /tmp/openpr-uf-completion-audit.XXXXXX.json)"
  jq "$filter" "$JSON_PATH" >"$tmp"
  if "$VERIFY" "$tmp" >/dev/null 2>&1; then
    echo "FAIL: $description was accepted" >&2
    rm -f "$tmp"
    exit 1
  fi
  rm -f "$tmp"
  pass "$description is rejected by verifier"
}

printf 'Universal forms completion audit JSON contract smoke\n'
printf '  JSON: %s\n' "$JSON_PATH"
printf '\n'

"$VERIFY" "$JSON_PATH" >/dev/null
pass "canonical completion audit JSON passes verifier"

expect_reject "missing schema path" 'del(.schema_path)'
expect_reject "extra top-level property" '.unexpected = true'
expect_reject "extra reports property" '.reports.unexpected = true'
expect_reject "extra gates property" '.gates.unexpected = true'
expect_reject "extra tracker status property" '.tracker_status.unexpected = true'
expect_reject "extra manual signoff property" '.manual_signoff.unexpected = true'
expect_reject "extra finalization property" '.finalization.unexpected = true'
expect_reject "string automated check counter" '.gates.automated_checks = "27"'
expect_reject "failed check count drift" '.gates.failed_automated_checks += 1'
expect_reject "PASS status line drift" '.gates.pass_status_lines -= 1'
expect_reject "manual pending count drift" '.manual_signoff.pending_rows += 1'
expect_reject "manual final flag drift" '.manual_signoff.final_signoff_allowed = true'
expect_reject "development release flag drift" '.finalization.development_release_allowed = true'
expect_reject "tracker manual status drift" '.tracker_status.user_side_manual_acceptance = "已验收"'
expect_reject "conclusion drift" '.conclusion = "finalized"'

printf '\nUniversal forms completion audit JSON contract smoke passed.\n'
