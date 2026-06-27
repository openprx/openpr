#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
JSON_PATH="${1:-$REPORT_DIR/openpr-universal-form-implementation-map-2026-05-31.json}"
MAP_PATH="$ROOT_DIR/docs/universal-forms-implementation-map.md"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-implementation-map.schema.json"
MARKDOWN_VERIFIER="$ROOT_DIR/scripts/verify-universal-forms-implementation-map.sh"

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-implementation-map-json.sh [JSON_PATH]

Verifies the machine-readable implementation map JSON against the Markdown
implementation map, repository schema, local source paths, and verification
command paths.
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

require_path() {
  local description="$1"
  local path="$2"
  local resolved
  if [[ "$path" == /* ]]; then
    resolved="$path"
  else
    resolved="$ROOT_DIR/$path"
  fi

  if [[ "$path" == */ ]]; then
    if [[ -d "$resolved" ]]; then
      pass "$description"
    else
      fail "$description"
      printf '  missing directory: %s\n' "$resolved" >&2
    fi
  else
    if [[ -e "$resolved" ]]; then
      pass "$description"
    else
      fail "$description"
      printf '  missing path: %s\n' "$resolved" >&2
    fi
  fi
}

json_value() {
  jq -r "$1" "$JSON_PATH"
}

status_markers_tsv() {
  awk -F'|' '
    /^## Status Markers$/ { in_section = 1; next }
    /^## / && in_section { exit }
    in_section && NF >= 4 {
      marker = $2
      meaning = $3
      transition = $4
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", marker)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", meaning)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", transition)
      gsub(/^`|`$/, "", marker)
      if (marker != "" && marker != "Marker" && marker !~ /^-+$/) {
        print marker "\t" meaning "\t" transition
      }
    }
  ' "$MAP_PATH"
}

module_rows_tsv() {
  awk -F'|' '
    /^## Module Map$/ { in_section = 1; next }
    /^## / && in_section { exit }
    in_section && NF >= 6 {
      area = $2
      paths = $3
      surface = $4
      verification = $5
      marker = $6
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", area)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", paths)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", surface)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", verification)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", marker)
      gsub(/^`|`$/, "", marker)
      if (area != "" && area != "Delivery area" && area !~ /^-+$/) {
        print area "\t" paths "\t" surface "\t" verification "\t" marker
      }
    }
  ' "$MAP_PATH"
}

area_key() {
  case "$1" in
    "Project types and scenario templates") printf 'project_types_scenario_templates' ;;
    "Universal form definitions and records") printf 'universal_form_records' ;;
    "Decimal-safe amount fields") printf 'decimal_amount_fields' ;;
    "Subforms and record links") printf 'subforms_record_links' ;;
    "Business events and delivery ledger") printf 'business_events_delivery_ledger' ;;
    "Connectors, webhooks, and print") printf 'connectors_webhooks_print' ;;
    "WASM plugin runtime") printf 'wasm_plugin_runtime' ;;
    "MCP business surface") printf 'mcp_business_surface' ;;
    "Frontend operator workflow") printf 'frontend_operator_workflow' ;;
    "Restaurant reference scenario") printf 'restaurant_reference_scenario' ;;
    "Delivery evidence and release gates") printf 'delivery_evidence_release_gates' ;;
    "User-side manual acceptance") printf 'user_side_manual_acceptance' ;;
    *) printf 'unknown' ;;
  esac
}

code_refs_joined() {
  printf '%s' "$1" | grep -oE '`[^`]+`' | sed 's/^`//; s/`$//' | paste -sd '|' - || true
}

printf 'Universal forms implementation map JSON verification\n'
printf '  JSON: %s\n' "$JSON_PATH"
printf '\n'

if command -v jq >/dev/null 2>&1; then
  pass "jq is available"
else
  fail "jq is available"
fi

for path in "$JSON_PATH" "$MAP_PATH" "$SCHEMA_PATH" "$MARKDOWN_VERIFIER"; do
  require_file "$path"
done

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms implementation map JSON verification failed before consistency checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

if jq empty "$JSON_PATH" >/dev/null; then
  pass "implementation map JSON is valid JSON"
else
  fail "implementation map JSON is valid JSON"
fi
if jq empty "$SCHEMA_PATH" >/dev/null; then
  pass "implementation map JSON schema is valid JSON"
else
  fail "implementation map JSON schema is valid JSON"
fi

if "$MARKDOWN_VERIFIER" "$MAP_PATH" >/dev/null; then
  pass "Markdown implementation map verifier passes"
else
  fail "Markdown implementation map verifier passes"
fi

equals "schema version is v1" "$(json_value '.schema_version')" "openpr.universal_forms.implementation_map.v1"
equals "JSON schema path matches repository schema" "$(json_value '.schema_path')" "$SCHEMA_PATH"
equals "JSON implementation map path matches repository doc" "$(json_value '.implementation_map_path')" "$MAP_PATH"
equals "schema file pins implementation map JSON v1" "$(jq -r '.properties.schema_version.const' "$SCHEMA_PATH")" "openpr.universal_forms.implementation_map.v1"
equals "schema file disallows top-level additional properties" "$(jq -r '.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file enumerates five status markers" "$(jq -r '.["$defs"].status_marker.properties.marker.enum | length' "$SCHEMA_PATH")" "5"
equals "schema file enumerates twelve module keys" "$(jq -r '.["$defs"].module.properties.key.enum | length' "$SCHEMA_PATH")" "12"
equals "schema file pins status marker order length" "$(jq -r '.properties.status_markers.prefixItems | length' "$SCHEMA_PATH")" "5"
equals "schema file pins status marker order" "$(jq -r '[.properties.status_markers.prefixItems[].allOf[1].properties.marker.const] | join(",")' "$SCHEMA_PATH")" "待处理,开发中,已完成,已测试,已验收"
equals "schema file disallows extra status markers" "$(jq -r '.properties.status_markers.items' "$SCHEMA_PATH")" "false"
equals "schema file pins module order length" "$(jq -r '.properties.modules.prefixItems | length' "$SCHEMA_PATH")" "12"
equals "schema file pins module order" "$(jq -r '[.properties.modules.prefixItems[].allOf[1].properties.key.const] | join(",")' "$SCHEMA_PATH")" "project_types_scenario_templates,universal_form_records,decimal_amount_fields,subforms_record_links,business_events_delivery_ledger,connectors_webhooks_print,wasm_plugin_runtime,mcp_business_surface,frontend_operator_workflow,restaurant_reference_scenario,delivery_evidence_release_gates,user_side_manual_acceptance"
equals "schema file disallows extra modules" "$(jq -r '.properties.modules.items' "$SCHEMA_PATH")" "false"
equals "JSON has every schema top-level required key" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].required - (keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON has no extra top-level keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(keys_unsorted - ($schema[0].properties | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON status markers have every schema required key" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.status_markers[] | ($schema[0].["$defs"].status_marker.required - (keys_unsorted)) | join(",")] | map(select(. != "")) | join(",")' "$JSON_PATH")" ""
equals "JSON status markers have no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.status_markers[]; ((keys_unsorted - ($schema[0].["$defs"].status_marker.properties | keys_unsorted)) | length) == 0)' "$JSON_PATH")" "true"
equals "JSON modules have every schema required key" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.modules[] | ($schema[0].["$defs"].module.required - (keys_unsorted)) | join(",")] | map(select(. != "")) | join(",")' "$JSON_PATH")" ""
equals "JSON modules have no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.modules[]; ((keys_unsorted - ($schema[0].["$defs"].module.properties | keys_unsorted)) | length) == 0)' "$JSON_PATH")" "true"
equals "JSON release boundary matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.release_boundary.required - (.release_boundary | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON release boundary has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.release_boundary | keys_unsorted) - ($schema[0].properties.release_boundary.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON marker values are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '([.status_markers[].marker] - $schema[0].["$defs"].status_marker.properties.marker.enum) | join(",")' "$JSON_PATH")" ""
equals "JSON marker values exactly match schema enum" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '([.status_markers[].marker] | sort) == ($schema[0].["$defs"].status_marker.properties.marker.enum | sort)' "$JSON_PATH")" "true"
equals "JSON marker values match schema order" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.status_markers[].marker] == [$schema[0].properties.status_markers.prefixItems[].allOf[1].properties.marker.const]' "$JSON_PATH")" "true"
equals "JSON module keys are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '([.modules[].key] - $schema[0].["$defs"].module.properties.key.enum) | join(",")' "$JSON_PATH")" ""
equals "JSON module keys exactly match schema enum" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '([.modules[].key] | sort) == ($schema[0].["$defs"].module.properties.key.enum | sort)' "$JSON_PATH")" "true"
equals "JSON module keys match schema order" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.modules[].key] == [$schema[0].properties.modules.prefixItems[].allOf[1].properties.key.const]' "$JSON_PATH")" "true"
equals "JSON module_count is integer" "$(jq -r '.module_count | type == "number" and floor == .' "$JSON_PATH")" "true"
equals "JSON module_count is 12" "$(json_value '.module_count')" "12"
equals "JSON modules length matches module_count" "$(jq -r '.modules | length' "$JSON_PATH")" "$(json_value '.module_count')"
equals "JSON status marker length is 5" "$(jq -r '.status_markers | length' "$JSON_PATH")" "5"
equals "JSON release boundary requires manual signoff" "$(json_value '.release_boundary.manual_signoff_required')" "true"
equals "JSON release boundary status command matches docs" "$(json_value '.release_boundary.status_command')" "scripts/status-universal-forms-delivery.sh --json"
equals "JSON release boundary delivery audit matches docs" "$(json_value '.release_boundary.delivery_bundle_audit')" "scripts/audit-universal-forms-delivery-bundle.sh"
equals "JSON release boundary strict gate matches docs" "$(json_value '.release_boundary.strict_release_gate')" "scripts/gate-universal-forms-release.sh --json"

markdown_marker_count="$(status_markers_tsv | wc -l | tr -d ' ')"
markdown_module_count="$(module_rows_tsv | wc -l | tr -d ' ')"
equals "JSON status markers count matches Markdown" "$(jq -r '.status_markers | length' "$JSON_PATH")" "$markdown_marker_count"
equals "JSON module count matches Markdown" "$(json_value '.module_count')" "$markdown_module_count"

while IFS=$'\t' read -r marker meaning transition; do
  equals "JSON marker meaning mirrors Markdown: $marker" "$(jq -r --arg marker "$marker" '.status_markers[] | select(.marker == $marker) | .meaning' "$JSON_PATH")" "$meaning"
  equals "JSON marker transition mirrors Markdown: $marker" "$(jq -r --arg marker "$marker" '.status_markers[] | select(.marker == $marker) | .allowed_transition' "$JSON_PATH")" "$transition"
done < <(status_markers_tsv)

while IFS=$'\t' read -r area paths surface verification marker; do
  key="$(area_key "$area")"
  expected_paths="$(code_refs_joined "$paths")"
  expected_verification="$(code_refs_joined "$verification")"
  equals "JSON delivery area mirrors Markdown: $area" "$(jq -r --arg key "$key" '.modules[] | select(.key == $key) | .delivery_area' "$JSON_PATH")" "$area"
  equals "JSON implementation paths mirror Markdown: $area" "$(jq -r --arg key "$key" '.modules[] | select(.key == $key) | .implementation_paths | join("|")' "$JSON_PATH")" "$expected_paths"
  equals "JSON public surface mirrors Markdown: $area" "$(jq -r --arg key "$key" '.modules[] | select(.key == $key) | .public_surface' "$JSON_PATH")" "$surface"
  equals "JSON verification commands mirror Markdown: $area" "$(jq -r --arg key "$key" '.modules[] | select(.key == $key) | .primary_verification | join("|")' "$JSON_PATH")" "$expected_verification"
  equals "JSON current marker mirrors Markdown: $area" "$(jq -r --arg key "$key" '.modules[] | select(.key == $key) | .current_marker' "$JSON_PATH")" "$marker"
  equals "JSON manual gate is true only for user acceptance: $area" "$(jq -r --arg key "$key" '.modules[] | select(.key == $key) | .manual_gate' "$JSON_PATH")" "$([[ "$key" == "user_side_manual_acceptance" ]] && printf true || printf false)"
done < <(module_rows_tsv)

while IFS=$'\t' read -r key path; do
  require_path "implementation path exists: $key/$path" "$path"
done < <(jq -r '.modules[] | .key as $key | .implementation_paths[] | [$key, .] | @tsv' "$JSON_PATH")

while IFS=$'\t' read -r key command_ref; do
  command_path="${command_ref%% *}"
  case "$command_path" in
    scripts/*|frontend/scripts/*|skills/*)
      require_path "verification command exists: $key/$command_path" "$command_path"
      ;;
    cargo|bun)
      pass "verification command is tool invocation: $key/$command_ref"
      ;;
    *)
      pass "verification command is descriptive: $key/$command_ref"
      ;;
  esac
done < <(jq -r '.modules[] | .key as $key | .primary_verification[] | [$key, .] | @tsv' "$JSON_PATH")

equals "non-manual modules remain tested" "$(jq -r '[.modules[] | select(.manual_gate == false) | .current_marker == "已测试"] | all' "$JSON_PATH")" "true"
equals "manual acceptance module remains pending" "$(jq -r '.modules[] | select(.key == "user_side_manual_acceptance") | .current_marker' "$JSON_PATH")" "待处理"

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms implementation map JSON verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms implementation map JSON verification passed.\n'
