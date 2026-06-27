#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
JSON_PATH="${1:-$REPORT_DIR/openpr-universal-form-scenario-catalog-2026-05-31.json}"
VERIFY="$ROOT_DIR/scripts/verify-universal-forms-scenario-catalog-json.sh"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-scenario-catalog-json-contract.sh [JSON_PATH]

Runs negative contract checks for the scenario catalog JSON verifier. The
canonical JSON must pass; malformed copies must fail.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for scenario catalog JSON contract smoke" >&2
  exit 2
fi

if [[ ! -f "$JSON_PATH" ]]; then
  echo "Scenario catalog JSON not found: $JSON_PATH" >&2
  exit 2
fi

pass() {
  printf 'PASS: %s\n' "$1"
}

expect_reject() {
  local description="$1"
  local filter="$2"
  local tmp
  tmp="$(mktemp /tmp/openpr-uf-scenario-catalog.XXXXXX.json)"
  jq "$filter" "$JSON_PATH" >"$tmp"
  if "$VERIFY" "$tmp" >/dev/null 2>&1; then
    echo "FAIL: $description was accepted" >&2
    rm -f "$tmp"
    exit 1
  fi
  rm -f "$tmp"
  pass "$description is rejected by verifier"
}

printf 'Universal forms scenario catalog JSON contract smoke\n'
printf '  JSON: %s\n' "$JSON_PATH"
printf '\n'

"$VERIFY" "$JSON_PATH" >/dev/null
pass "canonical scenario catalog JSON passes verifier"

expect_reject "missing schema path" 'del(.schema_path)'
expect_reject "extra top-level property" '.unexpected = true'
expect_reject "missing operator entrypoint" 'del(.operator_entrypoints[0])'
expect_reject "operator entrypoint order drift" '(.operator_entrypoints[0]) as $first | (.operator_entrypoints[1]) as $second | .operator_entrypoints[0] = $second | .operator_entrypoints[1] = $first'
expect_reject "extra template property" '.templates[0].unexpected = true'
expect_reject "string template count" '.template_count = "6"'
expect_reject "unknown template key" '.templates[0].key = "unknown_default"'
expect_reject "template order drift" '(.templates[0]) as $first | (.templates[1]) as $second | .templates[0] = $second | .templates[1] = $first'
expect_reject "template count drift" '.template_count += 1'
expect_reject "missing operator steps" 'del(.templates[0].operator_steps)'
expect_reject "missing project creation MCP tool" '(.templates[] | select(.key == "contract_review_default") | .primary_mcp_tools) -= ["projects.create"]'
expect_reject "missing MCP connector kind" '(.templates[] | select(.key == "customer_delivery_default") | .connector_kinds) -= ["mcp"]'
expect_reject "missing restaurant form" '(.templates[] | select(.key == "restaurant_ordering_default") | .forms) -= ["business_report"]'
expect_reject "missing restaurant_calc integration" '(.templates[] | select(.key == "restaurant_ordering_default") | .integrations) = ["MCP kitchen assistant"]'
expect_reject "missing restaurant print connector kind" '(.templates[] | select(.key == "restaurant_ordering_default") | .connector_kinds) -= ["print"]'
expect_reject "missing restaurant_calc plugin key" '(.templates[] | select(.key == "restaurant_ordering_default") | .plugin_keys) = []'

printf '\nUniversal forms scenario catalog JSON contract smoke passed.\n'
