#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
MAP_PATH="$ROOT_DIR/docs/universal-forms-implementation-map.md"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-implementation-map.schema.json"
OUTPUT_PATH="${OPENPR_IMPLEMENTATION_MAP_JSON_REPORT:-$REPORT_DIR/openpr-universal-form-implementation-map-2026-05-31.json}"

usage() {
  cat <<'EOF'
Usage: scripts/report-universal-forms-implementation-map-json.sh [--output PATH]

Generates a machine-readable JSON view of docs/universal-forms-implementation-map.md.
The report is read-only and is intended for CI, release automation, MCP tools,
and webhook consumers that need module-to-source verification metadata without
parsing Markdown.

Environment:
  OPENPR_IMPLEMENTATION_MAP_JSON_REPORT  Optional output path.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --output)
      OUTPUT_PATH="${2:-}"
      if [[ -z "$OUTPUT_PATH" ]]; then
        echo "--output requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to generate implementation map JSON" >&2
  exit 2
fi

for path in "$MAP_PATH" "$SCHEMA_PATH"; do
  if [[ ! -f "$path" ]]; then
    echo "Missing required file: $path" >&2
    exit 1
  fi
done

markers_tsv="$(mktemp)"
modules_tsv="$(mktemp)"
output_tmp=""
trap 'rm -f "$markers_tsv" "$modules_tsv" "$output_tmp"' EXIT

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
' "$MAP_PATH" >"$markers_tsv"

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
' "$MAP_PATH" >"$modules_tsv"

marker_count="$(wc -l <"$markers_tsv" | tr -d ' ')"
module_count="$(wc -l <"$modules_tsv" | tr -d ' ')"
if [[ "$marker_count" != "5" ]]; then
  echo "Expected 5 implementation map status markers, found $marker_count" >&2
  exit 1
fi
if [[ "$module_count" != "12" ]]; then
  echo "Expected 12 implementation map module rows, found $module_count" >&2
  exit 1
fi

markers_json="$(jq -R -s '
  split("\n")
  | map(select(length > 0))
  | map(split("\t") | {
      marker: .[0],
      meaning: .[1],
      allowed_transition: .[2]
    })
' "$markers_tsv")"

modules_json="$(jq -R -s '
  def code_refs: [scan("`([^`]+)`") | .[0]];
  def key_for_area:
    {
      "Project types and scenario templates": "project_types_scenario_templates",
      "Universal form definitions and records": "universal_form_records",
      "Decimal-safe amount fields": "decimal_amount_fields",
      "Subforms and record links": "subforms_record_links",
      "Business events and delivery ledger": "business_events_delivery_ledger",
      "Connectors, webhooks, and print": "connectors_webhooks_print",
      "WASM plugin runtime": "wasm_plugin_runtime",
      "MCP business surface": "mcp_business_surface",
      "Frontend operator workflow": "frontend_operator_workflow",
      "Restaurant reference scenario": "restaurant_reference_scenario",
      "Delivery evidence and release gates": "delivery_evidence_release_gates",
      "User-side manual acceptance": "user_side_manual_acceptance"
    }[.] // "unknown";
  split("\n")
  | map(select(length > 0))
  | map(split("\t") | {
      key: (.[0] | key_for_area),
      delivery_area: .[0],
      implementation_paths: (.[1] | code_refs),
      public_surface: .[2],
      primary_verification: (.[3] | code_refs),
      current_marker: .[4],
      manual_gate: (.[0] == "User-side manual acceptance")
    })
' "$modules_tsv")"

mkdir -p "$(dirname "$OUTPUT_PATH")"
output_tmp="$(mktemp "$(dirname "$OUTPUT_PATH")/.implementation-map-json.XXXXXX")"

jq -n \
  --arg schema_version "openpr.universal_forms.implementation_map.v1" \
  --arg schema_path "$SCHEMA_PATH" \
  --arg generated_at "$(date -Is)" \
  --arg implementation_map_path "$MAP_PATH" \
  --argjson status_markers "$markers_json" \
  --argjson module_count "$module_count" \
  --argjson modules "$modules_json" \
  '{
    schema_version: $schema_version,
    schema_path: $schema_path,
    generated_at: $generated_at,
    implementation_map_path: $implementation_map_path,
    status_markers: $status_markers,
    module_count: $module_count,
    modules: $modules,
    release_boundary: {
      status_command: "scripts/status-universal-forms-delivery.sh --json",
      delivery_bundle_audit: "scripts/audit-universal-forms-delivery-bundle.sh",
      strict_release_gate: "scripts/gate-universal-forms-release.sh --json",
      manual_signoff_required: true
    }
  }' >"$output_tmp"

if [[ -f "$OUTPUT_PATH" ]]; then
  chmod --reference="$OUTPUT_PATH" "$output_tmp"
fi
mv -f "$output_tmp" "$OUTPUT_PATH"
output_tmp=""

echo "implementation map JSON: $OUTPUT_PATH" >&2
