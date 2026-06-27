#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
JSON_PATH="${1:-$REPORT_DIR/openpr-universal-form-implementation-map-2026-05-31.json}"
VERIFY="$ROOT_DIR/scripts/verify-universal-forms-implementation-map-json.sh"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-implementation-map-json-contract.sh [JSON_PATH]

Runs negative contract checks for the implementation map JSON verifier. The
canonical JSON must pass; malformed temporary copies must fail.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if [[ ! -f "$JSON_PATH" ]]; then
  echo "Implementation map JSON not found: $JSON_PATH" >&2
  exit 2
fi
if [[ ! -x "$VERIFY" ]]; then
  echo "Implementation map JSON verifier is not executable: $VERIFY" >&2
  exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for implementation map JSON contract smoke" >&2
  exit 2
fi

failures=0

pass() {
  printf 'PASS: %s\n' "$1"
}

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  failures=$((failures + 1))
}

printf 'Universal forms implementation map JSON contract smoke\n'
printf '  JSON: %s\n' "$JSON_PATH"
printf '\n'

if "$VERIFY" "$JSON_PATH" >/dev/null; then
  pass "canonical implementation map JSON passes verifier"
else
  fail "canonical implementation map JSON passes verifier"
fi

tmp_dir="$(mktemp -d /tmp/openpr-uf-implementation-map-json-contract.XXXXXX)"
trap 'rm -rf "$tmp_dir"' EXIT

expect_reject() {
  local label="$1"
  local filter="$2"
  local candidate="$tmp_dir/${label//[^A-Za-z0-9_]/_}.json"

  if ! jq "$filter" "$JSON_PATH" >"$candidate"; then
    fail "$label mutation can be generated"
    return
  fi

  if "$VERIFY" "$candidate" >/dev/null 2>&1; then
    fail "$label is rejected by verifier"
  else
    pass "$label is rejected by verifier"
  fi
}

expect_reject "missing schema path" 'del(.schema_path)'
expect_reject "extra top-level property" '.unexpected_contract_field = true'
expect_reject "extra status marker property" '.status_markers[0].unexpected = true'
expect_reject "extra module property" '.modules[0].unexpected = true'
expect_reject "extra release boundary property" '.release_boundary.unexpected = true'
expect_reject "module count drift" '.module_count += 1'
expect_reject "unknown module key" '.modules[0].key = "unknown_module"'
expect_reject "status marker order drift" '(.status_markers[0]) as $first | (.status_markers[1]) as $second | .status_markers[0] = $second | .status_markers[1] = $first'
expect_reject "module order drift" '(.modules[0]) as $first | (.modules[1]) as $second | .modules[0] = $second | .modules[1] = $first'
expect_reject "missing module row" 'del(.modules[0])'
expect_reject "tested marker drift" '(.modules[] | select(.key == "project_types_scenario_templates") | .current_marker) = "已完成"'
expect_reject "manual marker drift" '(.modules[] | select(.key == "user_side_manual_acceptance") | .current_marker) = "已测试"'
expect_reject "missing implementation path" '(.modules[] | select(.key == "project_types_scenario_templates") | .implementation_paths[0]) = "apps/api/src/routes/project_missing.rs"'
expect_reject "missing verification command" '(.modules[] | select(.key == "project_types_scenario_templates") | .primary_verification[0]) = "scripts/smoke-scenario-template-forms-missing.sh"'
expect_reject "missing status marker" 'del(.status_markers[1])'

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms implementation map JSON contract smoke failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms implementation map JSON contract smoke passed.\n'
