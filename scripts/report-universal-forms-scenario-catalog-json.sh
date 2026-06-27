#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
CATALOG_PATH="$ROOT_DIR/docs/scenario-templates.md"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-scenario-catalog.schema.json"
OUTPUT_PATH="${OPENPR_SCENARIO_CATALOG_JSON_REPORT:-$REPORT_DIR/openpr-universal-form-scenario-catalog-2026-05-31.json}"

usage() {
  cat <<'EOF'
Usage: scripts/report-universal-forms-scenario-catalog-json.sh [--output PATH]

Generates a machine-readable JSON view of the built-in scenario template
catalog. The report mirrors docs/scenario-templates.md and is read-only.

Environment:
  OPENPR_SCENARIO_CATALOG_JSON_REPORT  Optional output path.
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
  echo "jq is required to generate scenario catalog JSON" >&2
  exit 2
fi

for path in "$CATALOG_PATH" "$SCHEMA_PATH"; do
  if [[ ! -f "$path" ]]; then
    echo "Missing required file: $path" >&2
    exit 1
  fi
done

rows_tsv="$(mktemp)"
output_tmp=""
trap 'rm -f "$rows_tsv" "$output_tmp"' EXIT

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
' "$CATALOG_PATH" >"$rows_tsv"

template_count="$(wc -l <"$rows_tsv" | tr -d ' ')"
if [[ "$template_count" == "0" ]]; then
  echo "No scenario template rows found in $CATALOG_PATH" >&2
  exit 1
fi

templates_json="$(jq -R -s '
  def split_list:
    split(",")
    | map(gsub("^\\s+|\\s+$"; ""))
    | map(gsub("`"; ""))
    | map(select(length > 0));
  def usage_for($key):
    if $key == "code_delivery_default" then {
      operator_steps: [
        "Create the project from the code delivery template",
        "Register repository and working-directory resources",
        "Create code tasks and change records",
        "Record CI and release evidence before handoff"
      ],
      primary_mcp_tools: ["scenario_templates.get", "projects.create", "forms.list", "form_records.create", "events.tail"],
      connector_kinds: ["mcp", "webhook"],
      plugin_keys: [],
      acceptance_focus: ["Repository resource is visible", "Release check records hold verification evidence"]
    }
    elif $key == "contract_review_default" then {
      operator_steps: [
        "Create the project from the contract review template",
        "Upload or link the contract document resource",
        "Create contract and risk-clause records",
        "Route approval records through MCP or webhook consumers"
      ],
      primary_mcp_tools: ["scenario_templates.get", "projects.create", "forms.list", "form_records.create", "form_records.link", "events.tail"],
      connector_kinds: ["mcp", "webhook"],
      plugin_keys: [],
      acceptance_focus: ["Contract amount stays decimal-safe", "Risk clauses are linked to the contract"]
    }
    elif $key == "equipment_maintenance_default" then {
      operator_steps: [
        "Create the project from the equipment maintenance template",
        "Register equipment and site resources",
        "Create repair orders from fault reports",
        "Close the workflow with inspection records"
      ],
      primary_mcp_tools: ["scenario_templates.get", "projects.create", "forms.list", "form_records.create", "form_records.update", "events.tail"],
      connector_kinds: ["webhook", "mcp"],
      plugin_keys: [],
      acceptance_focus: ["Repair orders reference equipment", "Inspection acceptance is captured as a record"]
    }
    elif $key == "quality_corrective_action_default" then {
      operator_steps: [
        "Create the project from the quality corrective action template",
        "Create defect records by batch or order",
        "Link root-cause analysis to corrective actions",
        "Record recheck results and audit evidence"
      ],
      primary_mcp_tools: ["scenario_templates.get", "projects.create", "forms.list", "form_records.create", "form_records.link", "events.tail"],
      connector_kinds: ["mcp", "webhook"],
      plugin_keys: [],
      acceptance_focus: ["Defect, root-cause, and corrective-action records stay linked", "Recheck status is visible to audit consumers"]
    }
    elif $key == "customer_delivery_default" then {
      operator_steps: [
        "Create the project from the customer delivery template",
        "Register customer and acceptance-material resources",
        "Create milestone and change-request records",
        "Send updates through MCP, webhook, or REST connectors"
      ],
      primary_mcp_tools: ["scenario_templates.get", "projects.create", "forms.list", "form_records.create", "form_records.update", "events.tail"],
      connector_kinds: ["mcp", "webhook", "rest"],
      plugin_keys: [],
      acceptance_focus: ["Milestones expose acceptance status", "Change requests keep commercial risk evidence"]
    }
    elif $key == "restaurant_ordering_default" then {
      operator_steps: [
        "Create the project from the restaurant ordering template",
        "Create menu categories, SKU, and tables",
        "Create orders with order-line child records",
        "Generate print jobs, receipts, and business reports"
      ],
      primary_mcp_tools: ["scenario_templates.get", "projects.create", "forms.list", "form_records.create", "form_records.link", "form_records.aggregate", "events.tail", "plugins.invoke"],
      connector_kinds: ["mcp", "print", "webhook"],
      plugin_keys: ["restaurant_calc"],
      acceptance_focus: ["Order-line totals are calculated by the plugin", "Print connector receipts update delivery state", "Revenue aggregate returns decimal strings"]
    }
    else error("unknown template key: " + $key)
    end;
  split("\n")
  | map(select(length > 0))
  | map(split("\t") | {
      key: .[0],
      project_type: .[1],
      best_fit: .[2],
      forms: (.[3] | split_list),
      integrations: (.[4] | split_list)
    } + usage_for(.[0]))
' "$rows_tsv")"

operator_entrypoints_json="$(jq -n '[
  {
    key: "frontend_project_wizard",
    label: "Frontend project wizard",
    consumer: "human_operator",
    command_hint: "Open the workspace project list and pick a scenario template card"
  },
  {
    key: "rest_projects_create",
    label: "REST project creation API",
    consumer: "integration_service",
    command_hint: "POST /api/v1/workspaces/{workspace_id}/projects with scenario_template_key"
  },
  {
    key: "mcp_projects_create",
    label: "MCP projects.create",
    consumer: "ai_or_automation_client",
    command_hint: "projects.create with scenario_template_key, then forms.list"
  }
]')"

mkdir -p "$(dirname "$OUTPUT_PATH")"
output_tmp="$(mktemp "$(dirname "$OUTPUT_PATH")/.scenario-catalog-json.XXXXXX")"

jq -n \
  --arg schema_version "openpr.universal_forms.scenario_catalog.v1" \
  --arg schema_path "$SCHEMA_PATH" \
  --arg generated_at "$(date -Is)" \
  --arg markdown_catalog "$CATALOG_PATH" \
  --argjson template_count "$template_count" \
  --argjson operator_entrypoints "$operator_entrypoints_json" \
  --argjson templates "$templates_json" \
  '{
    schema_version: $schema_version,
    schema_path: $schema_path,
    generated_at: $generated_at,
    markdown_catalog: $markdown_catalog,
    template_count: $template_count,
    operator_entrypoints: $operator_entrypoints,
    templates: $templates
  }' >"$output_tmp"

if [[ -f "$OUTPUT_PATH" ]]; then
  chmod --reference="$OUTPUT_PATH" "$output_tmp"
fi
mv -f "$output_tmp" "$OUTPUT_PATH"
output_tmp=""

echo "scenario catalog JSON: $OUTPUT_PATH" >&2
