#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
READINESS_JSON_PATH="${1:-$REPORT_DIR/openpr-universal-form-readiness-2026-05-31.json}"
TRACKER_PATH="$REPORT_DIR/openpr-universal-form-development-execution-tracker-2026-05-31.md"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
COMPLETION_AUDIT_PATH="$REPORT_DIR/openpr-universal-form-completion-audit-2026-05-31.md"
READINESS_SUMMARY_PATH="$REPORT_DIR/openpr-universal-form-readiness-summary-2026-05-31.md"
SIGNOFF_STATUS_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.md"
SIGNOFF_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-readiness.schema.json"

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-readiness-json.sh [JSON_PATH]

Verifies the machine-readable universal forms readiness JSON against the
authoritative tracker, acceptance evidence, completion audit, readiness
summary, and manual signoff status report.
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

require_file() {
  local path="$1"
  if [[ -f "$path" ]]; then
    pass "file exists: $path"
  else
    fail "file missing: $path"
  fi
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

json_value() {
  local expression="$1"
  jq -r "$expression" "$READINESS_JSON_PATH"
}

schema_value() {
  local expression="$1"
  jq -r "$expression" "$SCHEMA_PATH"
}

table_value() {
  local path="$1"
  local label="$2"
  awk -F'|' -v label="$label" '
    NF >= 3 {
      key = $2
      value = $3
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", key)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
      if (key == label) {
        print value
        exit
      }
    }
  ' "$path"
}

non_manual_unresolved_count() {
  awk -F'|' '
    /^\|/ && NF >= 4 {
      item = $2
      status = $3
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", item)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", status)
      if (item == "" || item == "---" || item == "模块" || item == "任务" || item == "检查项" || item == "状态") {
        next
      }
      if (status != "待处理" && status != "开发中" && status != "已完成" && status != "已测试" && status != "已验收" && status != "阻塞") {
        next
      }
      if (item == "用户侧人工验收") {
        next
      }
      if (status != "已测试" && status != "已验收") {
        count += 1
      }
    }
    END { print count + 0 }
  ' "$TRACKER_PATH"
}

printf 'Universal forms readiness JSON verification\n'
printf '  JSON: %s\n' "$READINESS_JSON_PATH"
printf '\n'

if ! command -v jq >/dev/null 2>&1; then
  fail "jq is available"
else
  pass "jq is available"
fi

for path in \
  "$READINESS_JSON_PATH" \
  "$TRACKER_PATH" \
  "$EVIDENCE_PATH" \
  "$COMPLETION_AUDIT_PATH" \
  "$READINESS_SUMMARY_PATH" \
  "$SIGNOFF_STATUS_PATH" \
  "$SIGNOFF_STATUS_JSON_PATH" \
  "$SCHEMA_PATH"; do
  require_file "$path"
done

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms readiness JSON verification failed before consistency checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

if jq empty "$READINESS_JSON_PATH" >/dev/null; then
  pass "readiness JSON is valid JSON"
else
  fail "readiness JSON is valid JSON"
fi
if jq empty "$SCHEMA_PATH" >/dev/null; then
  pass "readiness JSON schema is valid JSON"
else
  fail "readiness JSON schema is valid JSON"
fi

summary_total="$(sed -n 's/^- Total automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
summary_failed="$(sed -n 's/^- Failed automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
pass_count="$(rg -c '^\*\*Status:\*\* PASS$' "$EVIDENCE_PATH" || true)"
check_index_rows="$(table_value "$COMPLETION_AUDIT_PATH" "Automated check index rows")"
check_index_failed_rows="$(table_value "$COMPLETION_AUDIT_PATH" "Automated check index failed rows")"
manual_pending_count="$(rg -c '^\| .* \| Pending \|' "$EVIDENCE_PATH" || true)"
manual_row_count="$(awk -F'|' '
  /^## Manual Acceptance Signoff$/ { in_section = 1; next }
  /^## / && in_section { exit }
  in_section && NF >= 5 {
    item = $2
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", item)
    if (item != "" && item != "Item" && item !~ /^-+$/) {
      count += 1
    }
  }
  END { print count + 0 }
' "$EVIDENCE_PATH")"
non_manual_unresolved="$(non_manual_unresolved_count)"
readiness_stage="$(table_value "$READINESS_SUMMARY_PATH" "Readiness stage")"
tracker_e2e_status="$(table_value "$TRACKER_PATH" "端到端验收")"
tracker_manual_status="$(table_value "$TRACKER_PATH" "用户侧人工验收")"
signoff_next_key="$(sed -n 's/^Next row: `\([^`]*\)` -.*/\1/p' "$SIGNOFF_STATUS_PATH" | head -n 1)"
signoff_next_evidence_note="$(
  awk '
    /^Suggested evidence note:$/ { want_fence = 1; next }
    want_fence && /^```/ { in_block = 1; want_fence = 0; next }
    in_block && /^```/ { exit }
    in_block { print }
  ' "$SIGNOFF_STATUS_PATH"
)"
signoff_next_recorder_command="$(
  awk '
    /^Recorder command after reviewer approval:$/ { want_fence = 1; next }
    want_fence && /^```/ { in_block = 1; want_fence = 0; next }
    in_block && /^```/ { exit }
    in_block { print }
  ' "$SIGNOFF_STATUS_PATH"
)"
signoff_json_pending_rows="$(jq -r '.manual_signoff.pending_rows' "$SIGNOFF_STATUS_JSON_PATH")"
signoff_json_next_key="$(jq -r '.manual_signoff.next_row.key' "$SIGNOFF_STATUS_JSON_PATH")"
signoff_json_next_note="$(jq -r '.manual_signoff.next_row.suggested_evidence_note' "$SIGNOFF_STATUS_JSON_PATH")"
signoff_json_next_command="$(jq -r '.manual_signoff.next_row.recorder_command' "$SIGNOFF_STATUS_JSON_PATH")"

equals "schema version is v1" "$(json_value '.schema_version')" "openpr.universal_forms.readiness.v1"
equals "JSON schema path matches repository schema" "$(json_value '.schema_path')" "$SCHEMA_PATH"
equals "schema file pins schema version" "$(schema_value '.properties.schema_version.const')" "openpr.universal_forms.readiness.v1"
equals "schema file disallows top-level additional properties" "$(schema_value '.additionalProperties')" "false"
equals "schema file requires next row recorder command" "$(schema_value '.properties.manual_signoff.properties.next_row.required | index("recorder_command") != null')" "true"
equals "schema file requires signoff status JSON report" "$(schema_value '.properties.reports.required | index("signoff_status_json") != null')" "true"
equals "schema file enumerates seven manual signoff keys" "$(schema_value '.["$defs"].manual_key.enum | length')" "7"
equals "schema file reuses manual key enum for rows" "$(schema_value '.["$defs"].manual_row.properties.key["$ref"]')" '#/$defs/manual_key'
equals "schema file constrains next row key to manual key enum" "$(schema_value '.properties.manual_signoff.properties.next_row.properties.key.anyOf[]? | select(.["$ref"] == "#/$defs/manual_key") | .["$ref"]')" '#/$defs/manual_key'
equals "schema file allows empty final next row key" "$(schema_value '.properties.manual_signoff.properties.next_row.properties.key.anyOf[]? | select(.const == "") | .const')" ""
equals "schema file pins manual row order length" "$(schema_value '.properties.manual_signoff.properties.rows.prefixItems | length')" "7"
equals "schema file pins manual row order" "$(jq -r '[.properties.manual_signoff.properties.rows.prefixItems[].allOf[1].properties.key.const] | join(",")' "$SCHEMA_PATH")" "restaurant_template,frontend_usability,amounts,workflow,hub_consistency,docs,overall"
equals "schema file disallows extra manual rows" "$(schema_value '.properties.manual_signoff.properties.rows.items')" "false"
equals "JSON has every schema top-level required key" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].required - (keys_unsorted)) | join(",")' "$READINESS_JSON_PATH")" ""
equals "JSON has no extra top-level keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(keys_unsorted - ($schema[0].properties | keys_unsorted)) | join(",")' "$READINESS_JSON_PATH")" ""
equals "JSON reports object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.reports.required - (.reports | keys_unsorted)) | join(",")' "$READINESS_JSON_PATH")" ""
equals "JSON reports object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.reports | keys_unsorted) - ($schema[0].properties.reports.properties | keys_unsorted) | join(",")' "$READINESS_JSON_PATH")" ""
equals "JSON gates object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.gates.required - (.gates | keys_unsorted)) | join(",")' "$READINESS_JSON_PATH")" ""
equals "JSON gates object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.gates | keys_unsorted) - ($schema[0].properties.gates.properties | keys_unsorted) | join(",")' "$READINESS_JSON_PATH")" ""
equals "JSON tracker status object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.tracker_status | keys_unsorted) - ($schema[0].properties.tracker_status.properties | keys_unsorted) | join(",")' "$READINESS_JSON_PATH")" ""
equals "JSON manual_signoff object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.manual_signoff.required - (.manual_signoff | keys_unsorted)) | join(",")' "$READINESS_JSON_PATH")" ""
equals "JSON manual_signoff object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.manual_signoff | keys_unsorted) - ($schema[0].properties.manual_signoff.properties | keys_unsorted) | join(",")' "$READINESS_JSON_PATH")" ""
equals "JSON next_row object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.manual_signoff.properties.next_row.required - (.manual_signoff.next_row | keys_unsorted)) | join(",")' "$READINESS_JSON_PATH")" ""
equals "JSON next_row object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.manual_signoff.next_row | keys_unsorted) - ($schema[0].properties.manual_signoff.properties.next_row.properties | keys_unsorted) | join(",")' "$READINESS_JSON_PATH")" ""
equals "JSON release_requirement object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.release_requirement.required - (.release_requirement | keys_unsorted)) | join(",")' "$READINESS_JSON_PATH")" ""
equals "JSON release_requirement object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.release_requirement | keys_unsorted) - ($schema[0].properties.release_requirement.properties | keys_unsorted) | join(",")' "$READINESS_JSON_PATH")" ""
equals "JSON manual row objects have no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.manual_signoff.rows[]; ((keys_unsorted - ($schema[0].["$defs"].manual_row.properties | keys_unsorted)) | length) == 0)' "$READINESS_JSON_PATH")" "true"
equals "JSON manual row keys are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '([.manual_signoff.rows[].key] - $schema[0].["$defs"].manual_key.enum) | join(",")' "$READINESS_JSON_PATH")" ""
equals "JSON stage is allowed by schema enum" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.stage as $stage | $schema[0].properties.stage.enum | index($stage) != null)' "$READINESS_JSON_PATH")" "true"
equals "JSON final flag is boolean" "$(jq -r '.final_acceptance_complete | type' "$READINESS_JSON_PATH")" "boolean"
equals "JSON numeric gates are integers" "$(jq -r '[.gates.automated_checks, .gates.pass_status_lines, .gates.failed_automated_checks, .gates.automated_check_index_rows, .gates.automated_check_index_failed_rows, .gates.tracker_non_manual_unresolved_items, .manual_signoff.pending_rows] | all(type == "number" and floor == .)' "$READINESS_JSON_PATH")" "true"
equals "JSON non-negative counters are non-negative" "$(jq -r '[.gates.automated_checks, .gates.pass_status_lines, .gates.failed_automated_checks, .gates.automated_check_index_rows, .gates.automated_check_index_failed_rows, .gates.tracker_non_manual_unresolved_items, .manual_signoff.pending_rows] | all(. >= 0)' "$READINESS_JSON_PATH")" "true"
equals "JSON gate status values are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.gates.manual_signoff_consistency, .gates.ui_review_gallery_verification, .gates.ui_review_gallery_browser_render] as $statuses | ($schema[0].["$defs"].gate_status.enum) as $allowed | all($statuses[]; $allowed | index(.) != null)' "$READINESS_JSON_PATH")" "true"
equals "JSON delivery manifest exists flag is boolean" "$(jq -r '.gates.delivery_manifest_exists | type' "$READINESS_JSON_PATH")" "boolean"
equals "JSON tracker statuses are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.tracker_status.end_to_end_acceptance, .tracker_status.user_side_manual_acceptance] as $statuses | ($schema[0].["$defs"].tracker_status.enum) as $allowed | all($statuses[]; $allowed | index(.) != null)' "$READINESS_JSON_PATH")" "true"
equals "JSON manual row statuses are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.manual_signoff.rows[].status] as $statuses | ($schema[0].["$defs"].manual_row.properties.status.enum) as $allowed | all($statuses[]; $allowed | index(.) != null)' "$READINESS_JSON_PATH")" "true"
equals "JSON manual row keys exactly match schema enum" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '([.manual_signoff.rows[].key] | sort) == ($schema[0].["$defs"].manual_key.enum | sort)' "$READINESS_JSON_PATH")" "true"
equals "JSON manual row keys match schema order" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.manual_signoff.rows[].key] == [$schema[0].properties.manual_signoff.properties.rows.prefixItems[].allOf[1].properties.key.const]' "$READINESS_JSON_PATH")" "true"
equals "JSON next row key is allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '.manual_signoff.next_row.key as $key | ($key == "" or ($schema[0].["$defs"].manual_key.enum | index($key) != null))' "$READINESS_JSON_PATH")" "true"
equals "JSON release requirement constants match schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '.release_requirement as $release | $schema[0].properties.release_requirement.properties as $props | ($release.stage == $props.stage.const and $release.failed_automated_checks == $props.failed_automated_checks.const and $release.tracker_non_manual_unresolved_items == $props.tracker_non_manual_unresolved_items.const and $release.manual_signoff_pending_rows == $props.manual_signoff_pending_rows.const and $release.end_to_end_acceptance == $props.end_to_end_acceptance.const and $release.user_side_manual_acceptance == $props.user_side_manual_acceptance.const)' "$READINESS_JSON_PATH")" "true"
equals "JSON signoff status JSON path matches default report" "$(json_value '.reports.signoff_status_json')" "$SIGNOFF_STATUS_JSON_PATH"
equals "JSON stage matches readiness summary" "$(json_value '.stage')" "$readiness_stage"
equals "JSON final acceptance flag matches stage" "$(json_value '(.final_acceptance_complete == (.stage == "Accepted"))')" "true"
equals "JSON automated checks match evidence" "$(json_value '.gates.automated_checks')" "$summary_total"
equals "JSON PASS status lines match evidence" "$(json_value '.gates.pass_status_lines')" "$pass_count"
equals "JSON failed checks match evidence" "$(json_value '.gates.failed_automated_checks')" "$summary_failed"
equals "JSON check index rows match completion audit" "$(json_value '.gates.automated_check_index_rows')" "$check_index_rows"
equals "JSON check index failed rows match completion audit" "$(json_value '.gates.automated_check_index_failed_rows')" "$check_index_failed_rows"
equals "JSON non-manual unresolved count matches tracker" "$(json_value '.gates.tracker_non_manual_unresolved_items')" "$non_manual_unresolved"
equals "JSON e2e tracker status matches tracker" "$(json_value '.tracker_status.end_to_end_acceptance')" "$tracker_e2e_status"
equals "JSON manual tracker status matches tracker" "$(json_value '.tracker_status.user_side_manual_acceptance')" "$tracker_manual_status"
equals "JSON manual pending rows match evidence" "$(json_value '.manual_signoff.pending_rows')" "$manual_pending_count"
equals "JSON manual pending rows match signoff status JSON" "$(json_value '.manual_signoff.pending_rows')" "$signoff_json_pending_rows"
equals "JSON manual row count matches evidence" "$(json_value '.manual_signoff.rows | length')" "$manual_row_count"
if [[ "$manual_pending_count" == "0" ]]; then
  equals "JSON next manual row is empty after signoff" "$(json_value '.manual_signoff.next_row.key')" ""
  equals "JSON next manual evidence note is empty after signoff" "$(json_value '.manual_signoff.next_row.suggested_evidence_note')" ""
  equals "JSON next manual recorder command is empty after signoff" "$(json_value '.manual_signoff.next_row.recorder_command')" ""
else
  equals "JSON next manual row matches signoff status" "$(json_value '.manual_signoff.next_row.key')" "$signoff_next_key"
  equals "JSON next manual evidence note matches signoff status" "$(json_value '.manual_signoff.next_row.suggested_evidence_note')" "$signoff_next_evidence_note"
  equals "JSON next manual recorder command matches signoff status" "$(json_value '.manual_signoff.next_row.recorder_command')" "$signoff_next_recorder_command"
  equals "JSON next manual row matches signoff status JSON" "$(json_value '.manual_signoff.next_row.key')" "$signoff_json_next_key"
  equals "JSON next manual evidence note matches signoff status JSON" "$(json_value '.manual_signoff.next_row.suggested_evidence_note')" "$signoff_json_next_note"
  equals "JSON next manual recorder command matches signoff status JSON" "$(json_value '.manual_signoff.next_row.recorder_command')" "$signoff_json_next_command"
fi

equals "release requirement stage is Accepted" "$(json_value '.release_requirement.stage')" "Accepted"
equals "release requirement failed checks is zero" "$(json_value '.release_requirement.failed_automated_checks')" "0"
equals "release requirement manual pending is zero" "$(json_value '.release_requirement.manual_signoff_pending_rows')" "0"
equals "release requirement e2e status is final" "$(json_value '.release_requirement.end_to_end_acceptance')" "已验收"
equals "release requirement manual status is final" "$(json_value '.release_requirement.user_side_manual_acceptance')" "已验收"

if [[ "$manual_pending_count" == "0" && "$tracker_e2e_status" == "已验收" && "$tracker_manual_status" == "已验收" ]]; then
  equals "accepted JSON stage after finalization" "$(json_value '.stage')" "Accepted"
  equals "accepted JSON final flag after finalization" "$(json_value '.final_acceptance_complete')" "true"
elif [[ "$manual_pending_count" == "0" ]]; then
  equals "signed JSON stage before finalization" "$(json_value '.stage')" "Signed, not finalized"
  equals "signed JSON final flag before finalization" "$(json_value '.final_acceptance_complete')" "false"
else
  equals "pre-signoff JSON stage while rows are pending" "$(json_value '.stage')" "Ready for user-side manual signoff"
  equals "pre-signoff JSON final flag while rows are pending" "$(json_value '.final_acceptance_complete')" "false"
fi

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms readiness JSON verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms readiness JSON verification passed.\n'
