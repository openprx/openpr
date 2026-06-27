#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
STATUS_JSON_PATH="${1:-$REPORT_DIR/openpr-universal-form-development-status-2026-05-31.json}"
TRACKER_PATH="$REPORT_DIR/openpr-universal-form-development-execution-tracker-2026-05-31.md"
READINESS_JSON_PATH="$REPORT_DIR/openpr-universal-form-readiness-2026-05-31.json"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-development-status.schema.json"

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-development-status-json.sh [JSON_PATH]

Verifies the machine-readable development matrix status JSON against the
tracker, readiness JSON, and repository schema.
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

json_value() {
  jq -r "$1" "$STATUS_JSON_PATH"
}

matrix_tsv() {
  awk -F'|' '
    /^## 开发阶段准入与交付判定矩阵$/ { seen_heading = 1; next }
    seen_heading && /^\| 环节 \|/ { in_table = 1; next }
    in_table && /^\| --- / { next }
    in_table && /^$/ { exit }
    in_table && NF >= 6 {
      phase = $2
      requirement = $3
      completion = $4
      evidence = $5
      status = $6
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", phase)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", requirement)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", completion)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", evidence)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", status)
      if (phase != "") {
        print phase "\t" requirement "\t" completion "\t" evidence "\t" status
      }
    }
  ' "$TRACKER_PATH"
}

phase_key() {
  case "$1" in
    "需求/协议") printf 'requirements_protocol' ;;
    "数据库/模型") printf 'database_model' ;;
    "后端 API") printf 'backend_api' ;;
    "MCP") printf 'mcp' ;;
    "Webhook/Connector") printf 'webhook_connector' ;;
    "WASM 插件") printf 'wasm_plugin' ;;
    "前端 UI") printf 'frontend_ui' ;;
    "场景模板") printf 'scenario_template' ;;
    "交付包") printf 'delivery_bundle' ;;
    "用户侧人工验收") printf 'user_manual_acceptance' ;;
    *) printf 'unknown' ;;
  esac
}

printf 'Universal forms development status JSON verification\n'
printf '  JSON: %s\n' "$STATUS_JSON_PATH"
printf '\n'

if command -v jq >/dev/null 2>&1; then
  pass "jq is available"
else
  fail "jq is available"
fi

for path in "$STATUS_JSON_PATH" "$TRACKER_PATH" "$READINESS_JSON_PATH" "$SCHEMA_PATH"; do
  require_file "$path"
done

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms development status JSON verification failed before consistency checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

if jq empty "$STATUS_JSON_PATH" >/dev/null; then
  pass "development status JSON is valid JSON"
else
  fail "development status JSON is valid JSON"
fi
if jq empty "$SCHEMA_PATH" >/dev/null; then
  pass "development status JSON schema is valid JSON"
else
  fail "development status JSON schema is valid JSON"
fi

equals "schema version is v1" "$(json_value '.schema_version')" "openpr.universal_forms.development_status.v1"
equals "JSON schema path matches repository schema" "$(json_value '.schema_path')" "$SCHEMA_PATH"
equals "schema file pins development status JSON v1" "$(jq -r '.properties.schema_version.const' "$SCHEMA_PATH")" "openpr.universal_forms.development_status.v1"
equals "schema file disallows top-level additional properties" "$(jq -r '.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file enumerates ten development row keys" "$(jq -r '.["$defs"].status_row.properties.key.enum | length' "$SCHEMA_PATH")" "10"
equals "schema file pins development row order length" "$(jq -r '.properties.rows.prefixItems | length' "$SCHEMA_PATH")" "10"
equals "schema file pins development row order" "$(jq -r '[.properties.rows.prefixItems[].allOf[1].properties.key.const] | join(",")' "$SCHEMA_PATH")" "requirements_protocol,database_model,backend_api,mcp,webhook_connector,wasm_plugin,frontend_ui,scenario_template,delivery_bundle,user_manual_acceptance"
equals "schema file disallows extra development rows" "$(jq -r '.properties.rows.items' "$SCHEMA_PATH")" "false"
equals "schema file requires final release flag" "$(jq -r '.properties.status_summary.required | index("final_release_allowed") != null' "$SCHEMA_PATH")" "true"
equals "schema file requires row phase" "$(jq -r '.["$defs"].status_row.required | index("phase") != null' "$SCHEMA_PATH")" "true"
equals "schema file requires non-empty row phase" "$(jq -r '.["$defs"].status_row.properties.phase.minLength' "$SCHEMA_PATH")" "1"
equals "schema file requires non-empty engineering requirement" "$(jq -r '.["$defs"].status_row.properties.engineering_requirement.minLength' "$SCHEMA_PATH")" "1"
equals "schema file requires non-empty completion rule" "$(jq -r '.["$defs"].status_row.properties.completion_rule.minLength' "$SCHEMA_PATH")" "1"
equals "JSON has every schema top-level required key" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].required - (keys_unsorted)) | join(",")' "$STATUS_JSON_PATH")" ""
equals "JSON has no extra top-level keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(keys_unsorted - ($schema[0].properties | keys_unsorted)) | join(",")' "$STATUS_JSON_PATH")" ""
equals "JSON status summary matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.status_summary.required - (.status_summary | keys_unsorted)) | join(",")' "$STATUS_JSON_PATH")" ""
equals "JSON status summary has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.status_summary | keys_unsorted) - ($schema[0].properties.status_summary.properties | keys_unsorted) | join(",")' "$STATUS_JSON_PATH")" ""
equals "JSON rows have every schema row required key" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.rows[] | ($schema[0].["$defs"].status_row.required - (keys_unsorted)) | join(",")] | map(select(. != "")) | join(",")' "$STATUS_JSON_PATH")" ""
equals "JSON rows have no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.rows[]; ((keys_unsorted - ($schema[0].["$defs"].status_row.properties | keys_unsorted)) | length) == 0)' "$STATUS_JSON_PATH")" "true"
equals "JSON row keys are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '([.rows[].key] - $schema[0].["$defs"].status_row.properties.key.enum) | join(",")' "$STATUS_JSON_PATH")" ""
equals "JSON row keys exactly match schema enum" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '([.rows[].key] | sort) == ($schema[0].["$defs"].status_row.properties.key.enum | sort)' "$STATUS_JSON_PATH")" "true"
equals "JSON row keys match schema order" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.rows[].key] == [$schema[0].properties.rows.prefixItems[].allOf[1].properties.key.const]' "$STATUS_JSON_PATH")" "true"
equals "JSON row statuses are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.rows[].status] as $statuses | ($schema[0].["$defs"].status_row.properties.status.enum) as $allowed | all($statuses[]; $allowed | index(.) != null)' "$STATUS_JSON_PATH")" "true"
equals "JSON row text fields are non-empty strings" "$(jq -r '[.rows[] | (.phase, .engineering_requirement, .completion_rule, .evidence) | type == "string" and length > 0] | all(. == true)' "$STATUS_JSON_PATH")" "true"
equals "JSON status summary counters are integers" "$(jq -r '[.status_summary.total_rows, .status_summary.tested_or_accepted_rows, .status_summary.accepted_rows, .status_summary.pending_rows, .status_summary.non_manual_unresolved_rows, .status_summary.manual_signoff_pending_rows, .status_summary.failed_automated_checks] | all(type == "number" and floor == .)' "$STATUS_JSON_PATH")" "true"
equals "JSON status summary final flag is boolean" "$(jq -r '.status_summary.final_release_allowed | type' "$STATUS_JSON_PATH")" "boolean"

tracker_rows="$(matrix_tsv | wc -l | tr -d ' ')"
json_rows="$(json_value '.rows | length')"
equals "JSON row count matches tracker development matrix" "$json_rows" "$tracker_rows"
equals "JSON summary total matches rows" "$(json_value '.status_summary.total_rows')" "$json_rows"
equals "JSON tested/accepted count matches rows" "$(jq -r '[.rows[] | select(.status == "已测试" or .status == "已验收")] | length' "$STATUS_JSON_PATH")" "$(json_value '.status_summary.tested_or_accepted_rows')"
equals "JSON accepted count matches rows" "$(jq -r '[.rows[] | select(.status == "已验收")] | length' "$STATUS_JSON_PATH")" "$(json_value '.status_summary.accepted_rows')"
equals "JSON pending count matches rows" "$(jq -r '[.rows[] | select(.status == "待处理")] | length' "$STATUS_JSON_PATH")" "$(json_value '.status_summary.pending_rows')"
equals "JSON non-manual unresolved count matches rows" "$(jq -r '[.rows[] | select(.key != "user_manual_acceptance" and (.status != "已测试" and .status != "已验收"))] | length' "$STATUS_JSON_PATH")" "$(json_value '.status_summary.non_manual_unresolved_rows')"
equals "JSON manual pending count matches readiness JSON" "$(json_value '.status_summary.manual_signoff_pending_rows')" "$(jq -r '.manual_signoff.pending_rows' "$READINESS_JSON_PATH")"
equals "JSON failed automated checks match readiness JSON" "$(json_value '.status_summary.failed_automated_checks')" "$(jq -r '.gates.failed_automated_checks' "$READINESS_JSON_PATH")"

while IFS=$'\t' read -r phase requirement completion evidence status; do
  key="$(phase_key "$phase")"
  equals "JSON phase mirrors tracker matrix: $phase" "$(jq -r --arg key "$key" '.rows[] | select(.key == $key) | .phase' "$STATUS_JSON_PATH")" "$phase"
  equals "JSON status mirrors tracker matrix: $phase" "$(jq -r --arg key "$key" '.rows[] | select(.key == $key) | .status' "$STATUS_JSON_PATH")" "$status"
  equals "JSON engineering requirement mirrors tracker matrix: $phase" "$(jq -r --arg key "$key" '.rows[] | select(.key == $key) | .engineering_requirement' "$STATUS_JSON_PATH")" "$requirement"
  equals "JSON completion rule mirrors tracker matrix: $phase" "$(jq -r --arg key "$key" '.rows[] | select(.key == $key) | .completion_rule' "$STATUS_JSON_PATH")" "$completion"
  equals "JSON evidence mirrors tracker matrix: $phase" "$(jq -r --arg key "$key" '.rows[] | select(.key == $key) | .evidence' "$STATUS_JSON_PATH")" "$evidence"
  equals "JSON manual gate flag is true only for user acceptance: $phase" "$(jq -r --arg key "$key" '.rows[] | select(.key == $key) | .manual_gate' "$STATUS_JSON_PATH")" "$([[ "$key" == "user_manual_acceptance" ]] && printf true || printf false)"
done < <(matrix_tsv)

expected_final_release=false
if [[ "$(json_value '.status_summary.failed_automated_checks')" == "0" && "$(json_value '.status_summary.non_manual_unresolved_rows')" == "0" && "$(json_value '.status_summary.manual_signoff_pending_rows')" == "0" && "$(jq -r '.tracker_status.user_side_manual_acceptance' "$READINESS_JSON_PATH")" == "已验收" ]]; then
  expected_final_release=true
fi
equals "JSON final release flag matches tracker/readiness state" "$(json_value '.status_summary.final_release_allowed')" "$expected_final_release"

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms development status JSON verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms development status JSON verification passed.\n'
