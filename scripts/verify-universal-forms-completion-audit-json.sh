#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
JSON_PATH="${1:-$REPORT_DIR/openpr-universal-form-completion-audit-2026-05-31.json}"
TRACKER_PATH="$REPORT_DIR/openpr-universal-form-development-execution-tracker-2026-05-31.md"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
COMPLETION_AUDIT_PATH="$REPORT_DIR/openpr-universal-form-completion-audit-2026-05-31.md"
READINESS_JSON_PATH="$REPORT_DIR/openpr-universal-form-readiness-2026-05-31.json"
DEVELOPMENT_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-development-status-2026-05-31.json"
SIGNOFF_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-completion-audit.schema.json"

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-completion-audit-json.sh [JSON_PATH]

Verifies the machine-readable completion audit JSON against the tracker,
evidence, Markdown completion audit, readiness JSON, development status JSON,
signoff status JSON, and repository schema.
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
  jq -r "$1" "$JSON_PATH"
}

status_for() {
  local label="$1"
  awk -F'|' -v label="$label" '
    NF >= 4 {
      item = $2
      status = $3
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", item)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", status)
      if (item == label) {
        print status
        exit
      }
    }
  ' "$TRACKER_PATH"
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

printf 'Universal forms completion audit JSON verification\n'
printf '  JSON: %s\n' "$JSON_PATH"
printf '\n'

if command -v jq >/dev/null 2>&1; then
  pass "jq is available"
else
  fail "jq is available"
fi

for path in "$JSON_PATH" "$TRACKER_PATH" "$EVIDENCE_PATH" "$RUNBOOK_PATH" "$COMPLETION_AUDIT_PATH" "$READINESS_JSON_PATH" "$DEVELOPMENT_STATUS_JSON_PATH" "$SIGNOFF_STATUS_JSON_PATH" "$SCHEMA_PATH"; do
  require_file "$path"
done

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms completion audit JSON verification failed before consistency checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

if jq empty "$JSON_PATH" >/dev/null; then
  pass "completion audit JSON is valid JSON"
else
  fail "completion audit JSON is valid JSON"
fi
if jq empty "$SCHEMA_PATH" >/dev/null; then
  pass "completion audit JSON schema is valid JSON"
else
  fail "completion audit JSON schema is valid JSON"
fi

summary_total="$(sed -n 's/^- Total automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
summary_failed="$(sed -n 's/^- Failed automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
pass_count="$(rg -c '^\*\*Status:\*\* PASS$' "$EVIDENCE_PATH" || true)"
check_index_rows="$(awk '
  /^## Automated Check Index$/ { in_section = 1; next }
  /^## / && in_section { exit }
  in_section && /^\| [0-9]+ \|/ { count++ }
  END { print count + 0 }
' "$EVIDENCE_PATH")"
check_index_failed_rows="$(awk -F'|' '
  /^## Automated Check Index$/ { in_section = 1; next }
  /^## / && in_section { exit }
  in_section && NF >= 4 {
    status = $4
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", status)
    if (status ~ /^FAIL/) {
      count++
    }
  }
  END { print count + 0 }
' "$EVIDENCE_PATH")"
non_manual_unresolved="$(non_manual_unresolved_count)"

equals "schema version is v1" "$(json_value '.schema_version')" "openpr.universal_forms.completion_audit.v1"
equals "JSON schema path matches repository schema" "$(json_value '.schema_path')" "$SCHEMA_PATH"
equals "schema file pins completion audit JSON v1" "$(jq -r '.properties.schema_version.const' "$SCHEMA_PATH")" "openpr.universal_forms.completion_audit.v1"
equals "schema file disallows top-level additional properties" "$(jq -r '.additionalProperties' "$SCHEMA_PATH")" "false"
equals "JSON has every schema top-level required key" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].required - (keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON has no extra top-level keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(keys_unsorted - ($schema[0].properties | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON reports object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.reports.required - (.reports | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON reports object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.reports | keys_unsorted) - ($schema[0].properties.reports.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON gates object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.gates.required - (.gates | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON gates object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.gates | keys_unsorted) - ($schema[0].properties.gates.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON tracker status object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.tracker_status.required - (.tracker_status | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON tracker status object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.tracker_status | keys_unsorted) - ($schema[0].properties.tracker_status.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON manual signoff object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.manual_signoff.required - (.manual_signoff | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON manual signoff object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.manual_signoff | keys_unsorted) - ($schema[0].properties.manual_signoff.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON finalization object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.finalization.required - (.finalization | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON finalization object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.finalization | keys_unsorted) - ($schema[0].properties.finalization.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""

equals "tracker path matches current report" "$(json_value '.reports.tracker')" "$TRACKER_PATH"
equals "evidence path matches current report" "$(json_value '.reports.evidence')" "$EVIDENCE_PATH"
equals "runbook path matches current report" "$(json_value '.reports.runbook')" "$RUNBOOK_PATH"
equals "completion audit path matches current report" "$(json_value '.reports.completion_audit')" "$COMPLETION_AUDIT_PATH"
equals "readiness JSON path matches current report" "$(json_value '.reports.readiness_json')" "$READINESS_JSON_PATH"
equals "development status JSON path matches current report" "$(json_value '.reports.development_status_json')" "$DEVELOPMENT_STATUS_JSON_PATH"
equals "signoff status JSON path matches current report" "$(json_value '.reports.signoff_status_json')" "$SIGNOFF_STATUS_JSON_PATH"

equals "automated check count matches evidence" "$(json_value '.gates.automated_checks')" "$summary_total"
equals "PASS status lines match evidence" "$(json_value '.gates.pass_status_lines')" "$pass_count"
equals "failed automated checks match evidence" "$(json_value '.gates.failed_automated_checks')" "$summary_failed"
equals "automated check index rows match evidence" "$(json_value '.gates.automated_check_index_rows')" "$check_index_rows"
equals "automated check index failed rows match evidence" "$(json_value '.gates.automated_check_index_failed_rows')" "$check_index_failed_rows"
equals "non-manual unresolved count matches tracker" "$(json_value '.gates.non_manual_unresolved_items')" "$non_manual_unresolved"
equals "manual consistency gate is passed" "$(json_value '.gates.manual_consistency_status')" "passed"
equals "UI artifact gate is passed" "$(json_value '.gates.ui_artifact_status')" "passed"
equals "UI review gallery gate is passed" "$(json_value '.gates.ui_review_gallery_status')" "passed"
equals "UI review gallery render gate is passed" "$(json_value '.gates.ui_review_gallery_render_status')" "passed"
equals "finalizer safety gate is candidate strict before replace" "$(json_value '.gates.finalizer_safety_status')" "candidate-strict-before-replace"

expected_automated_complete=false
if [[ "$summary_failed" == "0" && "$pass_count" == "$summary_total" && "$check_index_rows" == "$summary_total" && "$check_index_failed_rows" == "0" && "$non_manual_unresolved" == "0" ]]; then
  expected_automated_complete=true
fi
equals "automated delivery flag matches evidence and tracker" "$(json_value '.gates.automated_delivery_complete')" "$expected_automated_complete"

equals "tracker end-to-end status matches tracker" "$(json_value '.tracker_status.end_to_end_acceptance')" "$(status_for "端到端验收")"
equals "tracker manual status matches tracker" "$(json_value '.tracker_status.user_side_manual_acceptance')" "$(status_for "用户侧人工验收")"
equals "manual total rows match signoff status JSON" "$(json_value '.manual_signoff.total_rows')" "$(jq -r '.manual_signoff.total_rows' "$SIGNOFF_STATUS_JSON_PATH")"
equals "manual accepted rows match signoff status JSON" "$(json_value '.manual_signoff.accepted_rows')" "$(jq -r '[.manual_signoff.rows[] | select(.status == "accepted")] | length' "$SIGNOFF_STATUS_JSON_PATH")"
equals "manual pending rows match signoff status JSON" "$(json_value '.manual_signoff.pending_rows')" "$(jq -r '.manual_signoff.pending_rows' "$SIGNOFF_STATUS_JSON_PATH")"
equals "manual blocked rows match signoff status JSON" "$(json_value '.manual_signoff.blocked_rows')" "$(jq -r '.manual_signoff.blocked_rows' "$SIGNOFF_STATUS_JSON_PATH")"
equals "manual final signoff flag matches signoff status JSON" "$(json_value '.manual_signoff.final_signoff_allowed')" "$(jq -r '.manual_signoff.final_signoff_allowed' "$SIGNOFF_STATUS_JSON_PATH")"
equals "final acceptance flag matches readiness JSON" "$(json_value '.finalization.final_acceptance_complete')" "$(jq -r '.final_acceptance_complete' "$READINESS_JSON_PATH")"
equals "development release flag matches development status JSON" "$(json_value '.finalization.development_release_allowed')" "$(jq -r '.status_summary.final_release_allowed' "$DEVELOPMENT_STATUS_JSON_PATH")"
equals "strict release required flag is true" "$(json_value '.finalization.strict_release_required')" "true"
equals "strict delivery-state required flag is true" "$(json_value '.finalization.strict_delivery_state_required')" "true"
equals "delivery-bundle audit required flag is true" "$(json_value '.finalization.delivery_bundle_audit_required')" "true"

expected_conclusion="not_proven"
if [[ "$(json_value '.gates.automated_delivery_complete')" == "true" && "$(json_value '.manual_signoff.pending_rows')" != "0" ]]; then
  expected_conclusion="pre_signoff_ready"
elif [[ "$(json_value '.gates.automated_delivery_complete')" == "true" && "$(json_value '.manual_signoff.pending_rows')" == "0" && "$(json_value '.finalization.final_acceptance_complete')" == "true" ]]; then
  expected_conclusion="finalized"
elif [[ "$(json_value '.gates.automated_delivery_complete')" == "true" && "$(json_value '.manual_signoff.pending_rows')" == "0" ]]; then
  expected_conclusion="ready_for_finalizer"
fi
equals "conclusion matches gates" "$(json_value '.conclusion')" "$expected_conclusion"

case "$expected_conclusion" in
  pre_signoff_ready)
    expected_markdown_conclusion="Conclusion: automated delivery is complete and internally consistent. Final acceptance is not complete because user-side manual signoff is still pending."
    ;;
  ready_for_finalizer)
    expected_markdown_conclusion='Conclusion: automated delivery and manual signoff are complete. Run `scripts/finalize-universal-forms-acceptance.sh`, then strict delivery-state and delivery-bundle audits.'
    ;;
  finalized)
    expected_markdown_conclusion="Conclusion: automated delivery, manual signoff, and tracker finalization are complete. The derived handoff is synchronized with the finalized tracker."
    ;;
  *)
    expected_markdown_conclusion="Conclusion: completion is not proven. Resolve the failing counts above before final acceptance."
    ;;
esac
if rg -q --fixed-strings -- "$expected_markdown_conclusion" "$COMPLETION_AUDIT_PATH"; then
  pass "completion audit Markdown conclusion mirrors JSON conclusion"
else
  fail "completion audit Markdown conclusion mirrors JSON conclusion"
  printf '  expected Markdown line: %s\n' "$expected_markdown_conclusion" >&2
fi

equals "integer counters are typed as numbers" "$(jq -r '[
  .gates.automated_checks,
  .gates.pass_status_lines,
  .gates.failed_automated_checks,
  .gates.automated_check_index_rows,
  .gates.automated_check_index_failed_rows,
  .gates.non_manual_unresolved_items,
  .manual_signoff.total_rows,
  .manual_signoff.accepted_rows,
  .manual_signoff.pending_rows,
  .manual_signoff.blocked_rows
] | all(type == "number" and floor == .)' "$JSON_PATH")" "true"
equals "boolean flags are typed as booleans" "$(jq -r '[
  .gates.automated_delivery_complete,
  .manual_signoff.final_signoff_allowed,
  .finalization.final_acceptance_complete,
  .finalization.development_release_allowed,
  .finalization.strict_release_required,
  .finalization.strict_delivery_state_required,
  .finalization.delivery_bundle_audit_required
] | all(type == "boolean")' "$JSON_PATH")" "true"

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms completion audit JSON verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms completion audit JSON verification passed.\n'
