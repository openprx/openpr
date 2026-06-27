#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
JSON_PATH="${1:-$REPORT_DIR/openpr-universal-form-scenario-catalog-2026-05-31.json}"
CATALOG_PATH="$ROOT_DIR/docs/scenario-templates.md"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-scenario-catalog.schema.json"
SMOKE_PATH="$ROOT_DIR/scripts/smoke-scenario-template-forms.sh"

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-scenario-catalog-json.sh [JSON_PATH]

Verifies the machine-readable scenario catalog JSON against the Markdown
catalog, repository schema, and scenario smoke coverage.
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

require_file() {
  local path="$1"
  if [[ -f "$path" ]]; then
    pass "file exists: $path"
  else
    fail "file missing: $path"
  fi
}

markdown_rows_tsv() {
  awk -F'|' '
    /^## Built-In Templates$/ { in_section = 1; next }
    /^## / && in_section { exit }
    in_section && NF >= 6 {
      key = $2
      project_type = $3
      best_fit = $4
      forms = $5
      integrations = $6
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", key)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", project_type)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", best_fit)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", forms)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", integrations)
      gsub(/^`|`$/, "", key)
      gsub(/^`|`$/, "", project_type)
      if (key != "" && key != "Template key" && key !~ /^-+$/) {
        print key "\t" project_type "\t" best_fit "\t" forms "\t" integrations
      }
    }
  ' "$CATALOG_PATH"
}

printf 'Universal forms scenario catalog JSON verification\n'
printf '  JSON: %s\n' "$JSON_PATH"
printf '\n'

if command -v jq >/dev/null 2>&1; then
  pass "jq is available"
else
  fail "jq is available"
fi

for path in "$JSON_PATH" "$CATALOG_PATH" "$SCHEMA_PATH" "$SMOKE_PATH"; do
  require_file "$path"
done

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms scenario catalog JSON verification failed before consistency checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

if jq empty "$JSON_PATH" >/dev/null; then
  pass "scenario catalog JSON is valid JSON"
else
  fail "scenario catalog JSON is valid JSON"
fi
if jq empty "$SCHEMA_PATH" >/dev/null; then
  pass "scenario catalog JSON schema is valid JSON"
else
  fail "scenario catalog JSON schema is valid JSON"
fi

equals "schema version is v1" "$(jq -r '.schema_version' "$JSON_PATH")" "openpr.universal_forms.scenario_catalog.v1"
equals "JSON schema path matches repository schema" "$(jq -r '.schema_path' "$JSON_PATH")" "$SCHEMA_PATH"
equals "schema file pins scenario catalog JSON v1" "$(jq -r '.properties.schema_version.const' "$SCHEMA_PATH")" "openpr.universal_forms.scenario_catalog.v1"
equals "schema file disallows top-level additional properties" "$(jq -r '.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file pins operator entrypoint order length" "$(jq -r '.properties.operator_entrypoints.prefixItems | length' "$SCHEMA_PATH")" "3"
equals "schema file pins operator entrypoint order" "$(jq -r '[.properties.operator_entrypoints.prefixItems[].allOf[1].properties.key.const] | join(",")' "$SCHEMA_PATH")" "frontend_project_wizard,rest_projects_create,mcp_projects_create"
equals "schema file enumerates six scenario template keys" "$(jq -r '.["$defs"].template.properties.key.enum | length' "$SCHEMA_PATH")" "6"
equals "schema file pins scenario template order length" "$(jq -r '.properties.templates.prefixItems | length' "$SCHEMA_PATH")" "6"
equals "schema file pins scenario template order" "$(jq -r '[.properties.templates.prefixItems[].allOf[1].properties.key.const] | join(",")' "$SCHEMA_PATH")" "code_delivery_default,contract_review_default,equipment_maintenance_default,quality_corrective_action_default,customer_delivery_default,restaurant_ordering_default"
equals "schema file disallows extra scenario templates" "$(jq -r '.properties.templates.items' "$SCHEMA_PATH")" "false"
equals "JSON has every schema top-level required key" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].required - (keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON has no extra top-level keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(keys_unsorted - ($schema[0].properties | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON operator entrypoints have every schema required key" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.operator_entrypoints[] | ($schema[0].["$defs"].operator_entrypoint.required - (keys_unsorted)) | join(",")] | map(select(. != "")) | join(",")' "$JSON_PATH")" ""
equals "JSON operator entrypoints have no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.operator_entrypoints[]; ((keys_unsorted - ($schema[0].["$defs"].operator_entrypoint.properties | keys_unsorted)) | length) == 0)' "$JSON_PATH")" "true"
equals "JSON operator entrypoint keys match schema order" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.operator_entrypoints[].key] == [$schema[0].properties.operator_entrypoints.prefixItems[].allOf[1].properties.key.const]' "$JSON_PATH")" "true"
equals "JSON operator entrypoints cover human, integration, and AI consumers" "$(jq -r '[.operator_entrypoints[].consumer] | sort | join(",")' "$JSON_PATH")" "ai_or_automation_client,human_operator,integration_service"
equals "JSON templates have every schema required key" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.templates[] | ($schema[0].["$defs"].template.required - (keys_unsorted)) | join(",")] | map(select(. != "")) | join(",")' "$JSON_PATH")" ""
equals "JSON templates have no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.templates[]; ((keys_unsorted - ($schema[0].["$defs"].template.properties | keys_unsorted)) | length) == 0)' "$JSON_PATH")" "true"
equals "JSON template keys are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '([.templates[].key] - $schema[0].["$defs"].template.properties.key.enum) | join(",")' "$JSON_PATH")" ""
equals "JSON template keys exactly match schema enum" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '([.templates[].key] | sort) == ($schema[0].["$defs"].template.properties.key.enum | sort)' "$JSON_PATH")" "true"
equals "JSON template keys match schema order" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.templates[].key] == [$schema[0].properties.templates.prefixItems[].allOf[1].properties.key.const]' "$JSON_PATH")" "true"
equals "JSON template_count is integer" "$(jq -r '.template_count | type == "number" and floor == .' "$JSON_PATH")" "true"
equals "JSON template arrays are non-empty" "$(jq -r '[.templates[] | (.forms | length > 0) and (.integrations | length > 0)] | all' "$JSON_PATH")" "true"
equals "JSON template operator steps are usable" "$(jq -r '[.templates[] | .operator_steps | length >= 3] | all' "$JSON_PATH")" "true"
equals "JSON template primary MCP tools include project creation" "$(jq -r '[.templates[] | .primary_mcp_tools | index("projects.create") != null] | all' "$JSON_PATH")" "true"
equals "JSON template connector kinds include MCP" "$(jq -r '[.templates[] | .connector_kinds | index("mcp") != null] | all' "$JSON_PATH")" "true"
equals "JSON template acceptance focus is present" "$(jq -r '[.templates[] | .acceptance_focus | length > 0] | all' "$JSON_PATH")" "true"

markdown_count="$(markdown_rows_tsv | wc -l | tr -d ' ')"
equals "JSON template_count matches Markdown table" "$(jq -r '.template_count' "$JSON_PATH")" "$markdown_count"
equals "JSON templates length matches template_count" "$(jq -r '.templates | length' "$JSON_PATH")" "$(jq -r '.template_count' "$JSON_PATH")"

while IFS=$'\t' read -r key project_type best_fit forms integrations; do
  equals "JSON project type mirrors Markdown: $key" "$(jq -r --arg key "$key" '.templates[] | select(.key == $key) | .project_type' "$JSON_PATH")" "$project_type"
  equals "JSON best fit mirrors Markdown: $key" "$(jq -r --arg key "$key" '.templates[] | select(.key == $key) | .best_fit' "$JSON_PATH")" "$best_fit"
  IFS=',' read -ra form_items <<<"$forms"
  for raw_form in "${form_items[@]}"; do
    form="$(printf '%s' "$raw_form" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//; s/^`//; s/`$//')"
    [[ -z "$form" ]] && continue
    equals "JSON includes Markdown form $key/$form" "$(jq -r --arg key "$key" --arg form "$form" '.templates[] | select(.key == $key) | .forms | index($form) != null' "$JSON_PATH")" "true"
  done
  equals "scenario smoke covers template: $key" "$(rg -q --fixed-strings "$key" "$SMOKE_PATH" && printf true || printf false)" "true"
done < <(markdown_rows_tsv)

equals "restaurant template includes seven forms" "$(jq -r '.templates[] | select(.key == "restaurant_ordering_default") | .forms | length' "$JSON_PATH")" "7"
equals "restaurant template includes business_report" "$(jq -r '.templates[] | select(.key == "restaurant_ordering_default") | .forms | index("business_report") != null' "$JSON_PATH")" "true"
equals "restaurant template includes restaurant_calc integration" "$(jq -r '.templates[] | select(.key == "restaurant_ordering_default") | .integrations | map(contains("restaurant_calc")) | any' "$JSON_PATH")" "true"
equals "restaurant template includes print connector kind" "$(jq -r '.templates[] | select(.key == "restaurant_ordering_default") | .connector_kinds | index("print") != null' "$JSON_PATH")" "true"
equals "restaurant template includes restaurant_calc plugin key" "$(jq -r '.templates[] | select(.key == "restaurant_ordering_default") | .plugin_keys | index("restaurant_calc") != null' "$JSON_PATH")" "true"
equals "restaurant template includes aggregate MCP tool" "$(jq -r '.templates[] | select(.key == "restaurant_ordering_default") | .primary_mcp_tools | index("form_records.aggregate") != null' "$JSON_PATH")" "true"
equals "all templates have at least three forms" "$(jq -r '[.templates[].forms | length >= 3] | all' "$JSON_PATH")" "true"

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms scenario catalog JSON verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms scenario catalog JSON verification passed.\n'
