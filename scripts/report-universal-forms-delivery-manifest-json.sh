#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
MANIFEST_PATH="${OPENPR_DELIVERY_MANIFEST:-$REPORT_DIR/openpr-universal-form-delivery-manifest-2026-05-31.md}"
OUTPUT_PATH="${OPENPR_DELIVERY_MANIFEST_JSON:-$REPORT_DIR/openpr-universal-form-delivery-manifest-2026-05-31.json}"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-delivery-manifest.schema.json"

usage() {
  cat <<'EOF'
Usage: scripts/report-universal-forms-delivery-manifest-json.sh [--manifest PATH] [--output PATH]

Generates a machine-readable JSON view of the universal forms delivery
manifest. The JSON mirrors the Markdown manifest's gate summary and file rows
without adding itself to the checksum manifest, avoiding recursive checksums.

Environment:
  OPENPR_DELIVERY_MANIFEST       Optional Markdown manifest path.
  OPENPR_DELIVERY_MANIFEST_JSON  Optional JSON output path.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --manifest)
      MANIFEST_PATH="${2:-}"
      if [[ -z "$MANIFEST_PATH" ]]; then
        echo "--manifest requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
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
  echo "jq is required to generate delivery manifest JSON" >&2
  exit 2
fi

if [[ ! -f "$MANIFEST_PATH" ]]; then
  echo "Delivery manifest not found: $MANIFEST_PATH" >&2
  exit 2
fi

if [[ ! -f "$SCHEMA_PATH" ]]; then
  echo "Delivery manifest JSON schema not found: $SCHEMA_PATH" >&2
  exit 2
fi

metadata_value() {
  local label="$1"
  sed -n "s/^- $label: //p" "$MANIFEST_PATH" | tail -n 1 | sed 's/^`//; s/`$//'
}

gate_value() {
  local label="$1"
  awk -F'|' -v label="$label" '
    /^## Gate Summary$/ { in_section = 1; next }
    /^## / && in_section { exit }
    in_section && NF >= 3 {
      key = $2
      value = $3
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", key)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
      if (key == label) {
        print value
        exit
      }
    }
  ' "$MANIFEST_PATH"
}

rows_tsv="$(mktemp)"
output_tmp=""
trap 'rm -f "$rows_tsv" "$output_tmp"' EXIT

awk -F'|' '
  NF >= 5 {
    label = $2
    path = $3
    bytes = $4
    sha = $5
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", label)
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", path)
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", bytes)
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", sha)
    gsub(/^`|`$/, "", path)
    if (label != "" && path ~ /^\// && bytes ~ /^[0-9]+$/ && sha ~ /^[0-9a-f]{64}$/) {
      print label "\t" path "\t" bytes "\t" sha
    }
  }
' "$MANIFEST_PATH" >"$rows_tsv"

file_count="$(wc -l <"$rows_tsv" | tr -d ' ')"
if [[ "$file_count" == "0" ]]; then
  echo "No file rows found in delivery manifest: $MANIFEST_PATH" >&2
  exit 1
fi

files_json="$(jq -R -s '
  split("\n")
  | map(select(length > 0))
  | map(split("\t") | {
      label: .[0],
      path: .[1],
      bytes: (.[2] | tonumber),
      sha256: .[3]
    })
' "$rows_tsv")"

mkdir -p "$(dirname "$OUTPUT_PATH")"
output_tmp="$(mktemp "$(dirname "$OUTPUT_PATH")/.delivery-manifest-json.XXXXXX")"

jq -n \
  --arg schema_version "openpr.universal_forms.delivery_manifest.v1" \
  --arg generated_at "$(date -Is)" \
  --arg source_generated_at "$(metadata_value "Generated at")" \
  --arg repository "$(metadata_value "Repository")" \
  --arg report_directory "$(metadata_value "Report directory")" \
  --arg artifact_directory "$(metadata_value "Artifact directory")" \
  --arg markdown_manifest "$MANIFEST_PATH" \
  --arg schema_path "$SCHEMA_PATH" \
  --arg automated_checks "$(gate_value "Automated checks")" \
  --arg pass_status_lines "$(gate_value "PASS status lines")" \
  --arg failed_automated_checks "$(gate_value "Failed automated checks")" \
  --arg manual_pending "$(gate_value "Manual signoff rows pending")" \
  --arg e2e_status "$(gate_value "End-to-end acceptance")" \
  --arg manual_status "$(gate_value "User-side manual acceptance")" \
  --argjson file_count "$file_count" \
  --argjson files "$files_json" \
  '{
    schema_version: $schema_version,
    generated_at: $generated_at,
    source_generated_at: $source_generated_at,
    repository: $repository,
    report_directory: $report_directory,
    artifact_directory: $artifact_directory,
    markdown_manifest: $markdown_manifest,
    schema_path: $schema_path,
    gate_summary: {
      automated_checks: ($automated_checks | tonumber),
      pass_status_lines: ($pass_status_lines | tonumber),
      failed_automated_checks: ($failed_automated_checks | tonumber),
      manual_signoff_rows_pending: ($manual_pending | tonumber),
      end_to_end_acceptance: $e2e_status,
      user_side_manual_acceptance: $manual_status
    },
    file_count: $file_count,
    files: $files
  }' >"$output_tmp"

if [[ -f "$OUTPUT_PATH" ]]; then
  chmod --reference="$OUTPUT_PATH" "$output_tmp"
fi
mv -f "$output_tmp" "$OUTPUT_PATH"
output_tmp=""

echo "delivery manifest JSON: $OUTPUT_PATH" >&2
