#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
JSON_PATH="${1:-$REPORT_DIR/openpr-universal-form-delivery-manifest-2026-05-31.json}"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-delivery-manifest.schema.json"

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-delivery-manifest-json.sh [JSON_PATH]

Verifies the machine-readable universal forms delivery manifest JSON:
  - JSON and schema are valid
  - top-level, gate summary, and file row keys are pinned
  - Markdown manifest rows are mirrored exactly
  - every file path, byte size, and SHA256 matches the live file
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

markdown_gate_value() {
  local manifest_path="$1"
  local label="$2"
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
  ' "$manifest_path"
}

manifest_rows_tsv() {
  local manifest_path="$1"
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
  ' "$manifest_path"
}

printf 'Universal forms delivery manifest JSON verification\n'
printf '  json: %s\n' "$JSON_PATH"
printf '\n'

if ! command -v jq >/dev/null 2>&1; then
  fail "jq is available"
fi

if [[ -f "$JSON_PATH" ]]; then
  pass "delivery manifest JSON exists"
else
  fail "delivery manifest JSON exists"
fi

if [[ -f "$SCHEMA_PATH" ]]; then
  pass "delivery manifest JSON schema exists"
else
  fail "delivery manifest JSON schema exists"
fi

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms delivery manifest JSON verification failed before JSON checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

if jq empty "$JSON_PATH" >/dev/null 2>&1; then
  pass "delivery manifest JSON is valid JSON"
else
  fail "delivery manifest JSON is valid JSON"
fi

if jq empty "$SCHEMA_PATH" >/dev/null 2>&1; then
  pass "delivery manifest JSON schema is valid JSON"
else
  fail "delivery manifest JSON schema is valid JSON"
fi

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms delivery manifest JSON verification failed before consistency checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

schema_version="$(jq -r '.schema_version' "$JSON_PATH")"
schema_const="$(jq -r '.properties.schema_version.const' "$SCHEMA_PATH")"
markdown_manifest="$(jq -r '.markdown_manifest' "$JSON_PATH")"
json_schema_path="$(jq -r '.schema_path' "$JSON_PATH")"

equals "schema version matches schema const" "$schema_version" "$schema_const"
equals "schema file pins delivery manifest JSON v1" "$schema_const" "openpr.universal_forms.delivery_manifest.v1"
equals "schema file disallows top-level additional properties" "$(jq -r '.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file disallows gate summary additional properties" "$(jq -r '.properties.gate_summary.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file disallows file row additional properties" "$(jq -r '.properties.files.items.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file requires file row sha256" "$(jq -r '.properties.files.items.required | index("sha256") != null' "$SCHEMA_PATH")" "true"
equals "schema path matches canonical schema" "$json_schema_path" "$SCHEMA_PATH"

if [[ -f "$markdown_manifest" ]]; then
  pass "Markdown delivery manifest exists"
else
  fail "Markdown delivery manifest exists"
fi

top_keys="$(jq -r 'keys | sort | join(",")' "$JSON_PATH")"
equals "top-level keys are pinned" "$top_keys" "artifact_directory,file_count,files,gate_summary,generated_at,markdown_manifest,report_directory,repository,schema_path,schema_version,source_generated_at"

gate_keys="$(jq -r '.gate_summary | keys | sort | join(",")' "$JSON_PATH")"
equals "gate summary keys are pinned" "$gate_keys" "automated_checks,end_to_end_acceptance,failed_automated_checks,manual_signoff_rows_pending,pass_status_lines,user_side_manual_acceptance"

file_row_key_failures="$(jq '[.files[] | keys | sort | join(",") | select(. != "bytes,label,path,sha256")] | length' "$JSON_PATH")"
equals "file row keys are pinned" "$file_row_key_failures" "0"

equals "file_count equals files length" "$(jq -r '.file_count' "$JSON_PATH")" "$(jq -r '.files | length' "$JSON_PATH")"
equals "automated check count type is number" "$(jq -r '.gate_summary.automated_checks | type' "$JSON_PATH")" "number"
equals "manual pending count type is number" "$(jq -r '.gate_summary.manual_signoff_rows_pending | type' "$JSON_PATH")" "number"

if [[ -f "$markdown_manifest" ]]; then
  equals "gate automated checks matches Markdown" "$(jq -r '.gate_summary.automated_checks' "$JSON_PATH")" "$(markdown_gate_value "$markdown_manifest" "Automated checks")"
  equals "gate PASS status lines matches Markdown" "$(jq -r '.gate_summary.pass_status_lines' "$JSON_PATH")" "$(markdown_gate_value "$markdown_manifest" "PASS status lines")"
  equals "gate failed checks matches Markdown" "$(jq -r '.gate_summary.failed_automated_checks' "$JSON_PATH")" "$(markdown_gate_value "$markdown_manifest" "Failed automated checks")"
  equals "gate manual pending rows matches Markdown" "$(jq -r '.gate_summary.manual_signoff_rows_pending' "$JSON_PATH")" "$(markdown_gate_value "$markdown_manifest" "Manual signoff rows pending")"
  equals "gate end-to-end acceptance matches Markdown" "$(jq -r '.gate_summary.end_to_end_acceptance' "$JSON_PATH")" "$(markdown_gate_value "$markdown_manifest" "End-to-end acceptance")"
  equals "gate user manual acceptance matches Markdown" "$(jq -r '.gate_summary.user_side_manual_acceptance' "$JSON_PATH")" "$(markdown_gate_value "$markdown_manifest" "User-side manual acceptance")"

  markdown_rows="$(mktemp)"
  json_rows="$(mktemp)"
  trap 'rm -f "$markdown_rows" "$json_rows"' EXIT

  manifest_rows_tsv "$markdown_manifest" >"$markdown_rows"
  jq -r '.files[] | [.label, .path, (.bytes | tostring), .sha256] | @tsv' "$JSON_PATH" >"$json_rows"

  equals "JSON file row count matches Markdown" "$(wc -l <"$json_rows" | tr -d ' ')" "$(wc -l <"$markdown_rows" | tr -d ' ')"
  if cmp -s "$markdown_rows" "$json_rows"; then
    pass "JSON file rows mirror Markdown manifest exactly"
  else
    fail "JSON file rows mirror Markdown manifest exactly"
  fi
fi

while IFS=$'\t' read -r label path expected_size expected_sha; do
  if [[ -z "$label" ]]; then
    continue
  fi
  if [[ "$path" != /* ]]; then
    fail "$label path is absolute"
    continue
  fi
  if [[ ! "$expected_size" =~ ^[0-9]+$ ]]; then
    fail "$label byte size is numeric"
    continue
  fi
  if [[ ! "$expected_sha" =~ ^[0-9a-f]{64}$ ]]; then
    fail "$label sha256 is valid hex"
    continue
  fi
  if [[ ! -f "$path" ]]; then
    fail "$label file exists: $path"
    continue
  fi
  actual_size="$(stat -c '%s' "$path")"
  actual_sha="$(sha256sum "$path" | awk '{print $1}')"
  equals "$label byte size matches live file" "$actual_size" "$expected_size"
  equals "$label sha256 matches live file" "$actual_sha" "$expected_sha"
done < <(jq -r '.files[] | [.label, .path, (.bytes | tostring), .sha256] | @tsv' "$JSON_PATH")

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms delivery manifest JSON verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms delivery manifest JSON verification passed.\n'
