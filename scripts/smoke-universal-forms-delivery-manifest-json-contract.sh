#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
DELIVERY_MANIFEST_JSON_PATH="${1:-$REPORT_DIR/openpr-universal-form-delivery-manifest-2026-05-31.json}"
VERIFIER_PATH="$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest-json.sh"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-delivery-manifest-json-contract.sh [JSON_PATH]

Runs a negative contract smoke for the machine-readable universal forms
delivery manifest JSON. The canonical JSON must pass verification; selected
malformed temporary copies must be rejected by the verifier.
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

if [[ ! -f "$DELIVERY_MANIFEST_JSON_PATH" ]]; then
  echo "Delivery manifest JSON not found: $DELIVERY_MANIFEST_JSON_PATH" >&2
  exit 2
fi
if [[ ! -x "$VERIFIER_PATH" ]]; then
  echo "Delivery manifest JSON verifier is not executable: $VERIFIER_PATH" >&2
  exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "Missing required command: jq" >&2
  exit 2
fi

printf 'Universal forms delivery manifest JSON contract smoke\n'
printf '  JSON: %s\n' "$DELIVERY_MANIFEST_JSON_PATH"
printf '\n'

if "$VERIFIER_PATH" "$DELIVERY_MANIFEST_JSON_PATH" >/dev/null; then
  pass "canonical delivery manifest JSON passes verifier"
else
  fail "canonical delivery manifest JSON passes verifier"
fi

tmp_dir="$(mktemp -d /tmp/openpr-uf-delivery-manifest-json-contract.XXXXXX)"
trap 'rm -rf "$tmp_dir"' EXIT

expect_reject() {
  local label="$1"
  local filter="$2"
  local candidate="$tmp_dir/${label//[^A-Za-z0-9_]/_}.json"

  if ! jq "$filter" "$DELIVERY_MANIFEST_JSON_PATH" >"$candidate"; then
    fail "$label mutation can be generated"
    return
  fi

  if "$VERIFIER_PATH" "$candidate" >/dev/null 2>&1; then
    fail "$label is rejected by verifier"
  else
    pass "$label is rejected by verifier"
  fi
}

expect_reject "missing schema path" 'del(.schema_path)'
expect_reject "extra top-level property" '.unexpected_contract_field = true'
expect_reject "extra gate summary property" '.gate_summary.unexpected_contract_field = true'
expect_reject "extra file row property" '.files[0].unexpected_contract_field = true'
expect_reject "string automated check counter" '.gate_summary.automated_checks = "26"'
expect_reject "file count drift" '.file_count = (.file_count + 1)'
expect_reject "missing file row sha256" 'del(.files[0].sha256)'
expect_reject "file row checksum drift" '.files[0].sha256 = "0000000000000000000000000000000000000000000000000000000000000000"'
expect_reject "file row order drift" '.files |= (if length > 1 then [.[1], .[0]] + .[2:] else . end)'
expect_reject "missing markdown manifest" '.markdown_manifest = "/tmp/openpr-missing-delivery-manifest.md"'

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms delivery manifest JSON contract smoke failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms delivery manifest JSON contract smoke passed.\n'
