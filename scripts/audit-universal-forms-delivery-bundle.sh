#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
ARTIFACT_DIR="/opt/worker/report/openpr/artifacts/universal-forms-ui-2026-05-31"
TRACKER_PATH="$REPORT_DIR/openpr-universal-form-development-execution-tracker-2026-05-31.md"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
COMPLETION_AUDIT_PATH="$REPORT_DIR/openpr-universal-form-completion-audit-2026-05-31.md"
COMPLETION_AUDIT_JSON_PATH="$REPORT_DIR/openpr-universal-form-completion-audit-2026-05-31.json"
USER_ACCEPTANCE_PACKET_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-packet-2026-05-31.md"
NEXT_SIGNOFF_REVIEW_PATH="$REPORT_DIR/openpr-universal-form-next-signoff-review-2026-05-31.md"
READINESS_SUMMARY_PATH="$REPORT_DIR/openpr-universal-form-readiness-summary-2026-05-31.md"
READINESS_JSON_PATH="$REPORT_DIR/openpr-universal-form-readiness-2026-05-31.json"
READINESS_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-readiness.schema.json"
SIGNOFF_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json"
SIGNOFF_STATUS_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-signoff-status.schema.json"
DEVELOPMENT_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-development-status-2026-05-31.json"
COMPLETION_AUDIT_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-completion-audit.schema.json"
DEVELOPMENT_STATUS_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-development-status.schema.json"
SCENARIO_CATALOG_JSON_PATH="$REPORT_DIR/openpr-universal-form-scenario-catalog-2026-05-31.json"
SCENARIO_CATALOG_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-scenario-catalog.schema.json"
IMPLEMENTATION_MAP_JSON_PATH="$REPORT_DIR/openpr-universal-form-implementation-map-2026-05-31.json"
IMPLEMENTATION_MAP_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-implementation-map.schema.json"
DELIVERY_MANIFEST_JSON_PATH="$REPORT_DIR/openpr-universal-form-delivery-manifest-2026-05-31.json"
DELIVERY_STATUS_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-delivery-status.schema.json"
DELIVERY_MANIFEST_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-delivery-manifest.schema.json"
RELEASE_GATE_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-release-gate.schema.json"
MANUAL_EVIDENCE_MAP_PATH="$REPORT_DIR/openpr-universal-form-manual-evidence-map-2026-05-31.md"
SIGNOFF_STATUS_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.md"
UI_ARTIFACT_MANIFEST_PATH="$REPORT_DIR/openpr-universal-form-ui-artifacts-2026-05-31.md"
UI_REVIEW_GALLERY_PATH="$REPORT_DIR/openpr-universal-form-ui-review-gallery-2026-05-31.html"
SIGNOFF_DASHBOARD_PATH="$REPORT_DIR/openpr-universal-form-signoff-dashboard-2026-05-31.html"
DELIVERY_MANIFEST_PATH="$REPORT_DIR/openpr-universal-form-delivery-manifest-2026-05-31.md"
REPORT_INDEX_PATH="$REPORT_DIR/README.md"

usage() {
  cat <<'EOF'
Usage: scripts/audit-universal-forms-delivery-bundle.sh

Audits the generated universal forms delivery bundle as a single handoff unit:
tracker, formal evidence, completion audit, user acceptance packet, manual
evidence map, runbook, UI artifacts, signoff verifier, and finalizer drill.
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

contains() {
  local description="$1"
  local path="$2"
  local needle="$3"
  if rg -q --fixed-strings -- "$needle" "$path"; then
    pass "$description"
  else
    fail "$description"
    printf '  missing in %s: %s\n' "$path" "$needle" >&2
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

first_line_equals() {
  local description="$1"
  local path="$2"
  local expected="$3"
  local actual
  actual="$(head -n 1 "$path" || true)"
  equals "$description" "$actual" "$expected"
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

status_for_tracker_row() {
  local label="$1"
  table_value "$TRACKER_PATH" "$label"
}

manual_status_rows() {
  local path="$1"
  local heading="$2"
  awk -F'|' -v heading="$heading" '
    $0 == heading { in_section = 1; next }
    /^## / && in_section { exit }
    in_section && NF >= 5 {
      item = $2
      status = $3
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", item)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", status)
      if (item != "" && item != "Item" && item != "项目" && item !~ /^-+$/) {
        print item "|" status
      }
    }
  ' "$path"
}

printf 'Universal forms delivery bundle audit\n'
printf '  repo: %s\n' "$ROOT_DIR"
printf '  report dir: %s\n' "$REPORT_DIR"
printf '\n'

if ! command -v jq >/dev/null 2>&1; then
  fail "jq is available for readiness JSON checks"
fi

for path in \
  "$TRACKER_PATH" \
  "$REPORT_INDEX_PATH" \
  "$EVIDENCE_PATH" \
  "$RUNBOOK_PATH" \
  "$COMPLETION_AUDIT_PATH" \
  "$COMPLETION_AUDIT_JSON_PATH" \
  "$COMPLETION_AUDIT_JSON_SCHEMA_PATH" \
  "$USER_ACCEPTANCE_PACKET_PATH" \
  "$NEXT_SIGNOFF_REVIEW_PATH" \
  "$READINESS_SUMMARY_PATH" \
  "$READINESS_JSON_PATH" \
  "$READINESS_JSON_SCHEMA_PATH" \
  "$SIGNOFF_STATUS_JSON_PATH" \
  "$SIGNOFF_STATUS_JSON_SCHEMA_PATH" \
  "$DEVELOPMENT_STATUS_JSON_PATH" \
  "$DEVELOPMENT_STATUS_JSON_SCHEMA_PATH" \
  "$SCENARIO_CATALOG_JSON_PATH" \
  "$SCENARIO_CATALOG_JSON_SCHEMA_PATH" \
  "$IMPLEMENTATION_MAP_JSON_PATH" \
  "$IMPLEMENTATION_MAP_JSON_SCHEMA_PATH" \
  "$DELIVERY_MANIFEST_JSON_PATH" \
  "$DELIVERY_STATUS_JSON_SCHEMA_PATH" \
  "$DELIVERY_MANIFEST_JSON_SCHEMA_PATH" \
  "$RELEASE_GATE_JSON_SCHEMA_PATH" \
  "$MANUAL_EVIDENCE_MAP_PATH" \
  "$SIGNOFF_STATUS_PATH" \
  "$UI_ARTIFACT_MANIFEST_PATH" \
  "$UI_REVIEW_GALLERY_PATH" \
  "$SIGNOFF_DASHBOARD_PATH" \
  "$DELIVERY_MANIFEST_PATH" \
  "$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh" \
  "$ROOT_DIR/scripts/verify-universal-forms-acceptance-signoff.sh" \
  "$ROOT_DIR/scripts/bootstrap-restaurant-demo.sh" \
  "$ROOT_DIR/scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh" \
  "$ROOT_DIR/scripts/audit-universal-forms-security-scope.sh" \
  "$ROOT_DIR/scripts/report-universal-forms-completion-audit-json.sh" \
  "$ROOT_DIR/scripts/verify-universal-forms-completion-audit-json.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-completion-audit-json-contract.sh" \
  "$ROOT_DIR/scripts/finalize-universal-forms-acceptance.sh" \
  "$ROOT_DIR/scripts/status-universal-forms-delivery.sh" \
  "$ROOT_DIR/scripts/verify-universal-forms-delivery-status-json.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-delivery-status-json-contract.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-delivery-status-output.sh" \
  "$ROOT_DIR/scripts/gate-universal-forms-release.sh" \
  "$ROOT_DIR/scripts/verify-universal-forms-release-gate-json.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-release-gate-json-contract.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-release-gate.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-release-gate-output.sh" \
  "$ROOT_DIR/scripts/audit-universal-forms-delivery-state.sh" \
  "$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest.sh" \
  "$ROOT_DIR/scripts/report-universal-forms-delivery-manifest-json.sh" \
  "$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest-json.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-delivery-manifest-json-contract.sh" \
  "$ROOT_DIR/scripts/record-universal-forms-manual-signoff.sh" \
  "$ROOT_DIR/scripts/report-universal-forms-signoff-status.sh" \
  "$ROOT_DIR/scripts/report-universal-forms-signoff-status-json.sh" \
  "$ROOT_DIR/scripts/verify-universal-forms-signoff-status-json.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-signoff-status-json-contract.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-signoff-status-output.sh" \
  "$ROOT_DIR/scripts/verify-universal-forms-next-signoff-review.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-next-signoff-review-contract.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-next-signoff-command.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-manual-signoff-progression.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-manual-signoff-commands.sh" \
  "$ROOT_DIR/scripts/report-universal-forms-readiness-summary.sh" \
  "$ROOT_DIR/scripts/report-universal-forms-readiness-json.sh" \
  "$ROOT_DIR/scripts/verify-universal-forms-readiness-json.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-readiness-json-contract.sh" \
  "$ROOT_DIR/scripts/report-universal-forms-development-status-json.sh" \
  "$ROOT_DIR/scripts/verify-universal-forms-development-status-json.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-development-status-json-contract.sh" \
  "$ROOT_DIR/scripts/report-universal-forms-scenario-catalog-json.sh" \
  "$ROOT_DIR/scripts/verify-universal-forms-scenario-catalog-json.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-scenario-catalog-json-contract.sh" \
  "$ROOT_DIR/scripts/report-universal-forms-implementation-map-json.sh" \
  "$ROOT_DIR/scripts/verify-universal-forms-implementation-map-json.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-implementation-map-json-contract.sh" \
  "$ROOT_DIR/scripts/verify-universal-forms-ui-artifacts.sh" \
  "$ROOT_DIR/scripts/prepare-universal-forms-ui-review-gallery.sh" \
  "$ROOT_DIR/scripts/verify-universal-forms-ui-review-gallery.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-ui-review-gallery-render.sh" \
  "$ROOT_DIR/scripts/smoke-universal-forms-signoff-dashboard-progression.sh"; do
  require_file "$path"
done

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms delivery bundle audit failed before consistency checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nGenerated evidence consistency:\n'
first_line_equals "next signoff review starts with Markdown heading" "$NEXT_SIGNOFF_REVIEW_PATH" "# OpenPR Universal Forms Next Signoff Review"
contains "next signoff review links signoff status JSON" "$NEXT_SIGNOFF_REVIEW_PATH" "$SIGNOFF_STATUS_JSON_PATH"
contains "next signoff review links manual evidence map" "$NEXT_SIGNOFF_REVIEW_PATH" "$MANUAL_EVIDENCE_MAP_PATH"
contains "next signoff review links user acceptance packet" "$NEXT_SIGNOFF_REVIEW_PATH" "$USER_ACCEPTANCE_PACKET_PATH"
contains "next signoff review links delivery manifest" "$NEXT_SIGNOFF_REVIEW_PATH" "$DELIVERY_MANIFEST_PATH"
contains "next signoff review lists current key" "$NEXT_SIGNOFF_REVIEW_PATH" '| Key | `restaurant_template` |'
contains "next signoff review lists current item" "$NEXT_SIGNOFF_REVIEW_PATH" "Restaurant template can create a project directly"
contains "next signoff review lists pending manual rows" "$NEXT_SIGNOFF_REVIEW_PATH" "| Pending manual rows | 7 | 0 |"
contains "next signoff review emits recorder command" "$NEXT_SIGNOFF_REVIEW_PATH" "scripts/record-universal-forms-manual-signoff.sh --item restaurant_template"
contains "next signoff review links UI review gallery" "$NEXT_SIGNOFF_REVIEW_PATH" "$UI_REVIEW_GALLERY_PATH"
contains "next signoff review links project template desktop screenshot" "$NEXT_SIGNOFF_REVIEW_PATH" "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-desktop.png"
contains "next signoff review links project template mobile screenshot" "$NEXT_SIGNOFF_REVIEW_PATH" "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-mobile.png"
evidence_total="$(sed -n 's/^- Total automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
evidence_failed="$(sed -n 's/^- Failed automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
evidence_pass_count="$(rg -c '^\*\*Status:\*\* PASS$' "$EVIDENCE_PATH" || true)"
evidence_index_count="$(awk '
  /^## Automated Check Index$/ { in_section = 1; next }
  /^## / && in_section { exit }
  in_section && /^\| [0-9]+ \|/ { count++ }
  END { print count + 0 }
' "$EVIDENCE_PATH")"
evidence_index_failed_count="$(awk -F'|' '
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
evidence_pending_count="$(rg -c '^\| .* \| Pending \|' "$EVIDENCE_PATH" || true)"
completion_total="$(table_value "$COMPLETION_AUDIT_PATH" "Total automated checks")"
completion_failed="$(table_value "$COMPLETION_AUDIT_PATH" "Failed automated checks")"
completion_pass_count="$(table_value "$COMPLETION_AUDIT_PATH" "PASS status lines")"
completion_unresolved="$(table_value "$COMPLETION_AUDIT_PATH" "Non-manual unresolved tracker rows")"
completion_ui_artifacts="$(table_value "$COMPLETION_AUDIT_PATH" "UI artifact verification")"
completion_ui_gallery="$(table_value "$COMPLETION_AUDIT_PATH" "UI review gallery verification")"
completion_ui_gallery_render="$(table_value "$COMPLETION_AUDIT_PATH" "UI review gallery browser render")"
completion_audit_json_schema_path="$(jq -r '.schema_path' "$COMPLETION_AUDIT_JSON_PATH")"
completion_audit_json_total="$(jq -r '.gates.automated_checks' "$COMPLETION_AUDIT_JSON_PATH")"
completion_audit_json_pass_count="$(jq -r '.gates.pass_status_lines' "$COMPLETION_AUDIT_JSON_PATH")"
completion_audit_json_failed="$(jq -r '.gates.failed_automated_checks' "$COMPLETION_AUDIT_JSON_PATH")"
completion_audit_json_unresolved="$(jq -r '.gates.non_manual_unresolved_items' "$COMPLETION_AUDIT_JSON_PATH")"
completion_audit_json_pending="$(jq -r '.manual_signoff.pending_rows' "$COMPLETION_AUDIT_JSON_PATH")"
completion_audit_json_complete="$(jq -r '.gates.automated_delivery_complete' "$COMPLETION_AUDIT_JSON_PATH")"
completion_audit_json_conclusion="$(jq -r '.conclusion' "$COMPLETION_AUDIT_JSON_PATH")"
completion_audit_schema_version="$(jq -r '.properties.schema_version.const' "$COMPLETION_AUDIT_JSON_SCHEMA_PATH")"
packet_pending="$(table_value "$USER_ACCEPTANCE_PACKET_PATH" "Manual signoff rows pending")"
packet_unresolved="$(table_value "$USER_ACCEPTANCE_PACKET_PATH" "Tracker non-manual unresolved items")"
readiness_stage="$(table_value "$READINESS_SUMMARY_PATH" "Readiness stage")"
readiness_total="$(table_value "$READINESS_SUMMARY_PATH" "Automated checks")"
readiness_pending="$(table_value "$READINESS_SUMMARY_PATH" "Manual signoff rows pending")"
readiness_manual_status="$(table_value "$READINESS_SUMMARY_PATH" "User-side manual acceptance")"
readiness_json_stage="$(jq -r '.stage' "$READINESS_JSON_PATH")"
readiness_json_total="$(jq -r '.gates.automated_checks' "$READINESS_JSON_PATH")"
readiness_json_failed="$(jq -r '.gates.failed_automated_checks' "$READINESS_JSON_PATH")"
readiness_json_pending="$(jq -r '.manual_signoff.pending_rows' "$READINESS_JSON_PATH")"
readiness_json_manual_status="$(jq -r '.tracker_status.user_side_manual_acceptance' "$READINESS_JSON_PATH")"
readiness_json_schema_path="$(jq -r '.schema_path' "$READINESS_JSON_PATH")"
readiness_json_signoff_status_json_path="$(jq -r '.reports.signoff_status_json' "$READINESS_JSON_PATH")"
readiness_schema_version="$(jq -r '.properties.schema_version.const' "$READINESS_JSON_SCHEMA_PATH")"
signoff_status_json_total="$(jq -r '.manual_signoff.total_rows' "$SIGNOFF_STATUS_JSON_PATH")"
signoff_status_json_pending="$(jq -r '.manual_signoff.pending_rows' "$SIGNOFF_STATUS_JSON_PATH")"
signoff_status_json_accepted="$(jq -r '.manual_signoff.accepted_rows' "$SIGNOFF_STATUS_JSON_PATH")"
signoff_status_json_blocked="$(jq -r '.manual_signoff.blocked_rows' "$SIGNOFF_STATUS_JSON_PATH")"
signoff_status_json_failed="$(jq -r '.gate_summary.failed_automated_checks' "$SIGNOFF_STATUS_JSON_PATH")"
signoff_status_json_final_allowed="$(jq -r '.manual_signoff.final_signoff_allowed' "$SIGNOFF_STATUS_JSON_PATH")"
signoff_status_json_schema_path="$(jq -r '.schema_path' "$SIGNOFF_STATUS_JSON_PATH")"
signoff_status_schema_version="$(jq -r '.properties.schema_version.const' "$SIGNOFF_STATUS_JSON_SCHEMA_PATH")"
development_status_total="$(jq -r '.status_summary.total_rows' "$DEVELOPMENT_STATUS_JSON_PATH")"
development_status_tested_or_accepted="$(jq -r '.status_summary.tested_or_accepted_rows' "$DEVELOPMENT_STATUS_JSON_PATH")"
development_status_pending="$(jq -r '.status_summary.pending_rows' "$DEVELOPMENT_STATUS_JSON_PATH")"
development_status_non_manual_unresolved="$(jq -r '.status_summary.non_manual_unresolved_rows' "$DEVELOPMENT_STATUS_JSON_PATH")"
development_status_manual_pending="$(jq -r '.status_summary.manual_signoff_pending_rows' "$DEVELOPMENT_STATUS_JSON_PATH")"
development_status_failed="$(jq -r '.status_summary.failed_automated_checks' "$DEVELOPMENT_STATUS_JSON_PATH")"
development_status_final_release="$(jq -r '.status_summary.final_release_allowed' "$DEVELOPMENT_STATUS_JSON_PATH")"
development_status_schema_path="$(jq -r '.schema_path' "$DEVELOPMENT_STATUS_JSON_PATH")"
development_status_schema_version="$(jq -r '.properties.schema_version.const' "$DEVELOPMENT_STATUS_JSON_SCHEMA_PATH")"
development_status_phase_count="$(jq -r '[.rows[].phase] | length' "$DEVELOPMENT_STATUS_JSON_PATH")"
development_status_nonempty_text_fields="$(jq -r '[.rows[] | (.phase, .engineering_requirement, .completion_rule, .evidence) | type == "string" and length > 0] | all(. == true)' "$DEVELOPMENT_STATUS_JSON_PATH")"
development_status_schema_phase_min_length="$(jq -r '.["$defs"].status_row.properties.phase.minLength' "$DEVELOPMENT_STATUS_JSON_SCHEMA_PATH")"
development_status_schema_completion_min_length="$(jq -r '.["$defs"].status_row.properties.completion_rule.minLength' "$DEVELOPMENT_STATUS_JSON_SCHEMA_PATH")"
scenario_catalog_count="$(jq -r '.template_count' "$SCENARIO_CATALOG_JSON_PATH")"
scenario_catalog_schema_path="$(jq -r '.schema_path' "$SCENARIO_CATALOG_JSON_PATH")"
scenario_catalog_schema_version="$(jq -r '.properties.schema_version.const' "$SCENARIO_CATALOG_JSON_SCHEMA_PATH")"
implementation_map_module_count="$(jq -r '.module_count' "$IMPLEMENTATION_MAP_JSON_PATH")"
implementation_map_schema_path="$(jq -r '.schema_path' "$IMPLEMENTATION_MAP_JSON_PATH")"
implementation_map_schema_version="$(jq -r '.properties.schema_version.const' "$IMPLEMENTATION_MAP_JSON_SCHEMA_PATH")"
implementation_map_manual_marker="$(jq -r '.modules[] | select(.key == "user_side_manual_acceptance") | .current_marker' "$IMPLEMENTATION_MAP_JSON_PATH")"
delivery_manifest_json_file_count="$(jq -r '.file_count' "$DELIVERY_MANIFEST_JSON_PATH")"
delivery_manifest_json_total="$(jq -r '.gate_summary.automated_checks' "$DELIVERY_MANIFEST_JSON_PATH")"
delivery_manifest_json_failed="$(jq -r '.gate_summary.failed_automated_checks' "$DELIVERY_MANIFEST_JSON_PATH")"
delivery_manifest_json_pending="$(jq -r '.gate_summary.manual_signoff_rows_pending' "$DELIVERY_MANIFEST_JSON_PATH")"
delivery_manifest_json_schema_path="$(jq -r '.schema_path' "$DELIVERY_MANIFEST_JSON_PATH")"
delivery_manifest_json_schema_version="$(jq -r '.properties.schema_version.const' "$DELIVERY_MANIFEST_JSON_SCHEMA_PATH")"

equals "evidence total matches PASS status lines" "$evidence_pass_count" "$evidence_total"
equals "evidence failed checks is 0" "$evidence_failed" "0"
equals "evidence automated check index row count matches total" "$evidence_index_count" "$evidence_total"
equals "evidence automated check index has no failed rows" "$evidence_index_failed_count" "0"
equals "completion audit total matches evidence" "$completion_total" "$evidence_total"
equals "completion audit PASS count matches evidence" "$completion_pass_count" "$evidence_pass_count"
equals "completion audit failed count matches evidence" "$completion_failed" "$evidence_failed"
equals "completion audit has no non-manual unresolved rows" "$completion_unresolved" "0"
equals "completion audit UI artifact gate passed" "$completion_ui_artifacts" "passed"
equals "completion audit UI review gallery gate passed" "$completion_ui_gallery" "passed"
equals "completion audit UI review gallery render gate passed" "$completion_ui_gallery_render" "passed"
equals "completion audit JSON total matches evidence" "$completion_audit_json_total" "$evidence_total"
equals "completion audit JSON PASS count matches evidence" "$completion_audit_json_pass_count" "$evidence_pass_count"
equals "completion audit JSON failed count matches evidence" "$completion_audit_json_failed" "$evidence_failed"
equals "completion audit JSON unresolved count matches tracker" "$completion_audit_json_unresolved" "$completion_unresolved"
equals "completion audit JSON pending rows match evidence" "$completion_audit_json_pending" "$evidence_pending_count"
equals "completion audit JSON automated completion flag is true" "$completion_audit_json_complete" "true"
equals "completion audit JSON conclusion remains pre-signoff" "$completion_audit_json_conclusion" "pre_signoff_ready"
equals "completion audit JSON schema path matches repository schema" "$completion_audit_json_schema_path" "$COMPLETION_AUDIT_JSON_SCHEMA_PATH"
equals "completion audit JSON schema pins v1" "$completion_audit_schema_version" "openpr.universal_forms.completion_audit.v1"
equals "user acceptance packet unresolved count matches completion audit" "$packet_unresolved" "$completion_unresolved"
equals "user acceptance packet pending rows match evidence" "$packet_pending" "$evidence_pending_count"
equals "readiness summary total matches evidence" "$readiness_total" "$evidence_total PASS / $evidence_failed failed"
equals "readiness summary pending rows match evidence" "$readiness_pending" "$evidence_pending_count"
equals "readiness summary manual status matches tracker" "$readiness_manual_status" "$(status_for_tracker_row "用户侧人工验收")"
equals "readiness JSON stage matches summary" "$readiness_json_stage" "$readiness_stage"
equals "readiness JSON total matches evidence" "$readiness_json_total" "$evidence_total"
equals "readiness JSON failed count matches evidence" "$readiness_json_failed" "$evidence_failed"
equals "readiness JSON pending rows match evidence" "$readiness_json_pending" "$evidence_pending_count"
equals "readiness JSON manual status matches tracker" "$readiness_json_manual_status" "$(status_for_tracker_row "用户侧人工验收")"
equals "readiness JSON schema path matches repository schema" "$readiness_json_schema_path" "$READINESS_JSON_SCHEMA_PATH"
equals "readiness JSON signoff status JSON path matches repository report" "$readiness_json_signoff_status_json_path" "$SIGNOFF_STATUS_JSON_PATH"
equals "readiness JSON schema pins v1" "$readiness_schema_version" "openpr.universal_forms.readiness.v1"
equals "manual signoff status JSON total rows" "$signoff_status_json_total" "7"
equals "manual signoff status JSON pending rows match evidence" "$signoff_status_json_pending" "$evidence_pending_count"
equals "manual signoff status JSON accepted rows match evidence" "$signoff_status_json_accepted" "$(jq -r '[.manual_signoff.rows[] | select(.status == "accepted")] | length' "$SIGNOFF_STATUS_JSON_PATH")"
equals "manual signoff status JSON has no blocked rows" "$signoff_status_json_blocked" "0"
equals "manual signoff status JSON failed checks match evidence" "$signoff_status_json_failed" "$evidence_failed"
expected_signoff_final_allowed="false"
if [[ "$evidence_failed" == "0" && "$evidence_pending_count" == "0" && "$signoff_status_json_blocked" == "0" ]]; then
  expected_signoff_final_allowed="true"
fi
equals "manual signoff status JSON final flag matches state" "$signoff_status_json_final_allowed" "$expected_signoff_final_allowed"
equals "manual signoff status JSON schema path matches repository schema" "$signoff_status_json_schema_path" "$SIGNOFF_STATUS_JSON_SCHEMA_PATH"
equals "manual signoff status JSON schema pins v1" "$signoff_status_schema_version" "openpr.universal_forms.signoff_status.v1"
equals "development status JSON total rows" "$development_status_total" "10"
equals "development status JSON tested rows match rows" "$development_status_tested_or_accepted" "$(jq -r '[.rows[] | select(.status == "已测试" or .status == "已验收")] | length' "$DEVELOPMENT_STATUS_JSON_PATH")"
equals "development status JSON pending rows match rows" "$development_status_pending" "$(jq -r '[.rows[] | select(.status == "待处理")] | length' "$DEVELOPMENT_STATUS_JSON_PATH")"
equals "development status JSON non-manual unresolved rows" "$development_status_non_manual_unresolved" "$completion_unresolved"
equals "development status JSON manual pending rows match evidence" "$development_status_manual_pending" "$evidence_pending_count"
equals "development status JSON failed checks match evidence" "$development_status_failed" "$evidence_failed"
equals "development status JSON phase count matches rows" "$development_status_phase_count" "$development_status_total"
equals "development status JSON row text fields are non-empty" "$development_status_nonempty_text_fields" "true"
expected_development_final_release="false"
if [[ "$evidence_failed" == "0" && "$completion_unresolved" == "0" && "$evidence_pending_count" == "0" && "$(status_for_tracker_row "用户侧人工验收")" == "已验收" ]]; then
  expected_development_final_release="true"
fi
equals "development status JSON final release flag matches state" "$development_status_final_release" "$expected_development_final_release"
equals "development status JSON schema path matches repository schema" "$development_status_schema_path" "$DEVELOPMENT_STATUS_JSON_SCHEMA_PATH"
equals "development status JSON schema pins v1" "$development_status_schema_version" "openpr.universal_forms.development_status.v1"
equals "development status JSON schema requires non-empty phase" "$development_status_schema_phase_min_length" "1"
equals "development status JSON schema requires non-empty completion rule" "$development_status_schema_completion_min_length" "1"
equals "scenario catalog JSON template count" "$scenario_catalog_count" "6"
equals "scenario catalog JSON schema path matches repository schema" "$scenario_catalog_schema_path" "$SCENARIO_CATALOG_JSON_SCHEMA_PATH"
equals "scenario catalog JSON schema pins v1" "$scenario_catalog_schema_version" "openpr.universal_forms.scenario_catalog.v1"
equals "implementation map JSON module count" "$implementation_map_module_count" "12"
equals "implementation map JSON schema path matches repository schema" "$implementation_map_schema_path" "$IMPLEMENTATION_MAP_JSON_SCHEMA_PATH"
equals "implementation map JSON schema pins v1" "$implementation_map_schema_version" "openpr.universal_forms.implementation_map.v1"
equals "implementation map JSON manual row remains pending" "$implementation_map_manual_marker" "待处理"
if [[ "$evidence_pending_count" == "0" ]]; then
  equals "readiness summary reaches accepted stage after signoff" "$readiness_stage" "Accepted"
else
  equals "readiness summary stays pre-signoff while manual rows are pending" "$readiness_stage" "Ready for user-side manual signoff"
fi

printf '\nDelivery manifest consistency:\n'
manifest_total="$(table_value "$DELIVERY_MANIFEST_PATH" "Automated checks")"
manifest_pass_count="$(table_value "$DELIVERY_MANIFEST_PATH" "PASS status lines")"
manifest_failed="$(table_value "$DELIVERY_MANIFEST_PATH" "Failed automated checks")"
manifest_pending="$(table_value "$DELIVERY_MANIFEST_PATH" "Manual signoff rows pending")"
equals "delivery manifest total matches evidence" "$manifest_total" "$evidence_total"
equals "delivery manifest PASS count matches evidence" "$manifest_pass_count" "$evidence_pass_count"
equals "delivery manifest failed count matches evidence" "$manifest_failed" "$evidence_failed"
equals "delivery manifest pending count matches evidence" "$manifest_pending" "$evidence_pending_count"
equals "delivery manifest JSON total matches evidence" "$delivery_manifest_json_total" "$evidence_total"
equals "delivery manifest JSON failed count matches evidence" "$delivery_manifest_json_failed" "$evidence_failed"
equals "delivery manifest JSON pending count matches evidence" "$delivery_manifest_json_pending" "$evidence_pending_count"
equals "delivery manifest JSON schema path matches repository schema" "$delivery_manifest_json_schema_path" "$DELIVERY_MANIFEST_JSON_SCHEMA_PATH"
equals "delivery manifest JSON schema pins v1" "$delivery_manifest_json_schema_version" "openpr.universal_forms.delivery_manifest.v1"

verify_manifest_checksum() {
  local label="$1"
  local path="$2"
  local expected_size expected_sha actual_size actual_sha
  expected_size="$(awk -F'|' -v label="$label" '
    NF >= 5 {
      key = $2
      size = $4
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", key)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", size)
      if (key == label) {
        print size
        exit
      }
    }
  ' "$DELIVERY_MANIFEST_PATH")"
  expected_sha="$(awk -F'|' -v label="$label" '
    NF >= 5 {
      key = $2
      sha = $5
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", key)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", sha)
      if (key == label) {
        print sha
        exit
      }
    }
  ' "$DELIVERY_MANIFEST_PATH")"
  if [[ -z "$expected_size" || -z "$expected_sha" ]]; then
    fail "delivery manifest has row: $label"
    return
  fi
  if [[ ! -f "$path" ]]; then
    fail "delivery manifest target exists: $label"
    return
  fi
  actual_size="$(stat -c '%s' "$path")"
  actual_sha="$(sha256sum "$path" | awk '{print $1}')"
  equals "delivery manifest size matches: $label" "$actual_size" "$expected_size"
  equals "delivery manifest sha256 matches: $label" "$actual_sha" "$expected_sha"
}

verify_manifest_checksum "tracker" "$TRACKER_PATH"
verify_manifest_checksum "report docs index" "$REPORT_INDEX_PATH"
verify_manifest_checksum "acceptance evidence" "$EVIDENCE_PATH"
verify_manifest_checksum "completion audit" "$COMPLETION_AUDIT_PATH"
verify_manifest_checksum "completion audit JSON" "$COMPLETION_AUDIT_JSON_PATH"
verify_manifest_checksum "user acceptance packet" "$USER_ACCEPTANCE_PACKET_PATH"
verify_manifest_checksum "next signoff review" "$NEXT_SIGNOFF_REVIEW_PATH"
verify_manifest_checksum "readiness summary" "$READINESS_SUMMARY_PATH"
verify_manifest_checksum "readiness JSON" "$READINESS_JSON_PATH"
verify_manifest_checksum "readiness JSON schema" "$READINESS_JSON_SCHEMA_PATH"
verify_manifest_checksum "manual signoff status JSON" "$SIGNOFF_STATUS_JSON_PATH"
verify_manifest_checksum "manual signoff status JSON schema" "$SIGNOFF_STATUS_JSON_SCHEMA_PATH"
verify_manifest_checksum "development status JSON" "$DEVELOPMENT_STATUS_JSON_PATH"
verify_manifest_checksum "development status JSON schema" "$DEVELOPMENT_STATUS_JSON_SCHEMA_PATH"
verify_manifest_checksum "scenario catalog JSON" "$SCENARIO_CATALOG_JSON_PATH"
verify_manifest_checksum "scenario catalog JSON schema" "$SCENARIO_CATALOG_JSON_SCHEMA_PATH"
verify_manifest_checksum "implementation map JSON" "$IMPLEMENTATION_MAP_JSON_PATH"
verify_manifest_checksum "implementation map JSON schema" "$IMPLEMENTATION_MAP_JSON_SCHEMA_PATH"
verify_manifest_checksum "manual evidence map" "$MANUAL_EVIDENCE_MAP_PATH"
verify_manifest_checksum "manual signoff status report" "$SIGNOFF_STATUS_PATH"
verify_manifest_checksum "manual runbook" "$RUNBOOK_PATH"
verify_manifest_checksum "UI artifact manifest" "$UI_ARTIFACT_MANIFEST_PATH"
verify_manifest_checksum "UI review gallery" "$UI_REVIEW_GALLERY_PATH"
verify_manifest_checksum "signoff dashboard" "$SIGNOFF_DASHBOARD_PATH"
verify_manifest_checksum "signoff dashboard desktop render" "$ARTIFACT_DIR/signoff-dashboard/signoff-dashboard-desktop.png"
verify_manifest_checksum "signoff dashboard mobile render" "$ARTIFACT_DIR/signoff-dashboard/signoff-dashboard-mobile.png"
verify_manifest_checksum "UI review gallery desktop render" "$ARTIFACT_DIR/ui-review-gallery/ui-review-gallery-desktop.png"
verify_manifest_checksum "UI review gallery mobile render" "$ARTIFACT_DIR/ui-review-gallery/ui-review-gallery-mobile.png"
verify_manifest_checksum "refresh delivery bundle" "$ROOT_DIR/scripts/refresh-universal-forms-delivery-bundle.sh"
verify_manifest_checksum "restaurant demo bootstrap" "$ROOT_DIR/scripts/bootstrap-restaurant-demo.sh"
verify_manifest_checksum "restaurant demo MCP HTTP smoke" "$ROOT_DIR/scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh"
verify_manifest_checksum "security scope audit" "$ROOT_DIR/scripts/audit-universal-forms-security-scope.sh"
verify_manifest_checksum "completion audit JSON reporter" "$ROOT_DIR/scripts/report-universal-forms-completion-audit-json.sh"
verify_manifest_checksum "completion audit JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-completion-audit-json.sh"
verify_manifest_checksum "completion audit JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-completion-audit-json-contract.sh"
verify_manifest_checksum "delivery-bundle audit" "$ROOT_DIR/scripts/audit-universal-forms-delivery-bundle.sh"
verify_manifest_checksum "delivery status command" "$ROOT_DIR/scripts/status-universal-forms-delivery.sh"
verify_manifest_checksum "delivery status JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-delivery-status-json.sh"
verify_manifest_checksum "delivery status JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-delivery-status-json-contract.sh"
verify_manifest_checksum "delivery status output smoke" "$ROOT_DIR/scripts/smoke-universal-forms-delivery-status-output.sh"
verify_manifest_checksum "release gate" "$ROOT_DIR/scripts/gate-universal-forms-release.sh"
verify_manifest_checksum "release gate JSON schema" "$RELEASE_GATE_JSON_SCHEMA_PATH"
verify_manifest_checksum "release gate JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-release-gate-json.sh"
verify_manifest_checksum "release gate JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-release-gate-json-contract.sh"
verify_manifest_checksum "release gate smoke" "$ROOT_DIR/scripts/smoke-universal-forms-release-gate.sh"
verify_manifest_checksum "release gate output smoke" "$ROOT_DIR/scripts/smoke-universal-forms-release-gate-output.sh"
verify_manifest_checksum "delivery manifest verifier" "$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest.sh"
verify_manifest_checksum "delivery manifest JSON reporter" "$ROOT_DIR/scripts/report-universal-forms-delivery-manifest-json.sh"
verify_manifest_checksum "delivery manifest JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest-json.sh"
verify_manifest_checksum "delivery manifest JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-delivery-manifest-json-contract.sh"
verify_manifest_checksum "implementation map verifier" "$ROOT_DIR/scripts/verify-universal-forms-implementation-map.sh"
verify_manifest_checksum "implementation map contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-implementation-map-contract.sh"
verify_manifest_checksum "MCP skill validator" "$ROOT_DIR/skills/openpr-mcp/scripts/validate-mcp.sh"
verify_manifest_checksum "MCP skill regression" "$ROOT_DIR/skills/openpr-mcp/scripts/mcp-regression.py"
verify_manifest_checksum "UI review gallery generator" "$ROOT_DIR/scripts/prepare-universal-forms-ui-review-gallery.sh"
verify_manifest_checksum "UI review gallery verifier" "$ROOT_DIR/scripts/verify-universal-forms-ui-review-gallery.sh"
verify_manifest_checksum "UI review gallery browser render smoke" "$ROOT_DIR/scripts/smoke-universal-forms-ui-review-gallery-render.sh"
verify_manifest_checksum "signoff dashboard generator" "$ROOT_DIR/scripts/prepare-universal-forms-signoff-dashboard.sh"
verify_manifest_checksum "signoff dashboard verifier" "$ROOT_DIR/scripts/verify-universal-forms-signoff-dashboard.sh"
verify_manifest_checksum "signoff dashboard browser render smoke" "$ROOT_DIR/scripts/smoke-universal-forms-signoff-dashboard-render.sh"
verify_manifest_checksum "signoff dashboard progression smoke" "$ROOT_DIR/scripts/smoke-universal-forms-signoff-dashboard-progression.sh"
verify_manifest_checksum "report output boundary smoke" "$ROOT_DIR/scripts/smoke-universal-forms-report-output-boundaries.sh"
verify_manifest_checksum "manual signoff recorder" "$ROOT_DIR/scripts/record-universal-forms-manual-signoff.sh"
verify_manifest_checksum "manual signoff status reporter" "$ROOT_DIR/scripts/report-universal-forms-signoff-status.sh"
verify_manifest_checksum "manual signoff status JSON reporter" "$ROOT_DIR/scripts/report-universal-forms-signoff-status-json.sh"
verify_manifest_checksum "manual signoff status JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-signoff-status-json.sh"
verify_manifest_checksum "manual signoff status JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-signoff-status-json-contract.sh"
verify_manifest_checksum "manual signoff status output smoke" "$ROOT_DIR/scripts/smoke-universal-forms-signoff-status-output.sh"
verify_manifest_checksum "next signoff review verifier" "$ROOT_DIR/scripts/verify-universal-forms-next-signoff-review.sh"
verify_manifest_checksum "next signoff review contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-next-signoff-review-contract.sh"
verify_manifest_checksum "next signoff command smoke" "$ROOT_DIR/scripts/smoke-universal-forms-next-signoff-command.sh"
verify_manifest_checksum "manual signoff progression smoke" "$ROOT_DIR/scripts/smoke-universal-forms-manual-signoff-progression.sh"
verify_manifest_checksum "manual signoff commands smoke" "$ROOT_DIR/scripts/smoke-universal-forms-manual-signoff-commands.sh"
verify_manifest_checksum "readiness summary reporter" "$ROOT_DIR/scripts/report-universal-forms-readiness-summary.sh"
verify_manifest_checksum "readiness JSON reporter" "$ROOT_DIR/scripts/report-universal-forms-readiness-json.sh"
verify_manifest_checksum "readiness JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-readiness-json.sh"
verify_manifest_checksum "readiness JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-readiness-json-contract.sh"
verify_manifest_checksum "development status JSON reporter" "$ROOT_DIR/scripts/report-universal-forms-development-status-json.sh"
verify_manifest_checksum "development status JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-development-status-json.sh"
verify_manifest_checksum "development status JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-development-status-json-contract.sh"
verify_manifest_checksum "scenario catalog JSON reporter" "$ROOT_DIR/scripts/report-universal-forms-scenario-catalog-json.sh"
verify_manifest_checksum "scenario catalog JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-scenario-catalog-json.sh"
verify_manifest_checksum "scenario catalog JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-scenario-catalog-json-contract.sh"
verify_manifest_checksum "implementation map JSON reporter" "$ROOT_DIR/scripts/report-universal-forms-implementation-map-json.sh"
verify_manifest_checksum "implementation map JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-implementation-map-json.sh"
verify_manifest_checksum "implementation map JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-implementation-map-json-contract.sh"
verify_manifest_checksum "completion audit JSON schema" "$COMPLETION_AUDIT_JSON_SCHEMA_PATH"
verify_manifest_checksum "delivery status JSON schema" "$DELIVERY_STATUS_JSON_SCHEMA_PATH"
verify_manifest_checksum "delivery manifest JSON schema" "$DELIVERY_MANIFEST_JSON_SCHEMA_PATH"
verify_manifest_checksum "MCP app README" "$ROOT_DIR/apps/mcp-server/README.md"
verify_manifest_checksum "MCP app AGENTS guide" "$ROOT_DIR/apps/mcp-server/AGENTS.md"
verify_manifest_checksum "MCP skill guide" "$ROOT_DIR/skills/openpr-mcp/SKILL.md"
verify_manifest_checksum "scenario template catalog" "$ROOT_DIR/docs/scenario-templates.md"
verify_manifest_checksum "implementation map" "$ROOT_DIR/docs/universal-forms-implementation-map.md"
verify_manifest_checksum "contributing guide" "$ROOT_DIR/CONTRIBUTING.md"
verify_manifest_checksum "CI workflow" "$ROOT_DIR/.github/workflows/ci.yml"
verify_manifest_checksum "frontend README" "$ROOT_DIR/frontend/README.md"
verify_manifest_checksum "frontend quickstart" "$ROOT_DIR/frontend/QUICKSTART.md"
verify_manifest_checksum "compose stack" "$ROOT_DIR/docker-compose.yml"
verify_manifest_checksum "source Dockerfile" "$ROOT_DIR/Dockerfile"
verify_manifest_checksum "prebuilt runtime Dockerfile" "$ROOT_DIR/Dockerfile.prebuilt"
verify_manifest_checksum "frontend Dockerfile" "$ROOT_DIR/frontend/Dockerfile"
verify_manifest_checksum "frontend nginx config" "$ROOT_DIR/frontend/nginx.conf"
verify_manifest_checksum "env example" "$ROOT_DIR/.env.example"
verify_manifest_checksum "webhook example config" "$ROOT_DIR/config/openpr-webhook.example.toml"
verify_manifest_checksum "start script" "$ROOT_DIR/scripts/start.sh"
verify_manifest_checksum "verify script" "$ROOT_DIR/scripts/verify.sh"
verify_manifest_checksum "e2e test script" "$ROOT_DIR/scripts/e2e-test.sh"
verify_manifest_checksum "API test script" "$ROOT_DIR/scripts/test-api.sh"
verify_manifest_checksum "MCP test script" "$ROOT_DIR/scripts/test-mcp.sh"
verify_manifest_checksum "benchmark script" "$ROOT_DIR/scripts/benchmark.sh"
verify_manifest_checksum "dev database script" "$ROOT_DIR/scripts/dev-up.sh"
verify_manifest_checksum "dev check script" "$ROOT_DIR/scripts/dev-check.sh"
verify_manifest_checksum "database init script" "$ROOT_DIR/scripts/init-db.sh"
verify_manifest_checksum "backup script" "$ROOT_DIR/scripts/backup-db.sh"
verify_manifest_checksum "restore script" "$ROOT_DIR/scripts/restore-db.sh"
verify_manifest_checksum "stop script" "$ROOT_DIR/scripts/stop.sh"
verify_manifest_checksum "clean script" "$ROOT_DIR/scripts/clean.sh"

if "$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest.sh" "$DELIVERY_MANIFEST_PATH" >/dev/null; then
  pass "delivery manifest verifier passes"
else
  fail "delivery manifest verifier passes"
fi
if "$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest-json.sh" "$DELIVERY_MANIFEST_JSON_PATH" >/dev/null; then
  pass "delivery manifest JSON verifier passes"
else
  fail "delivery manifest JSON verifier passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-delivery-manifest-json-contract.sh" "$DELIVERY_MANIFEST_JSON_PATH" >/dev/null; then
  pass "delivery manifest JSON contract smoke passes"
else
  fail "delivery manifest JSON contract smoke passes"
fi
if "$ROOT_DIR/scripts/verify-universal-forms-implementation-map.sh" >/dev/null; then
  pass "implementation map verifier passes"
else
  fail "implementation map verifier passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-implementation-map-contract.sh" >/dev/null; then
  pass "implementation map contract smoke passes"
else
  fail "implementation map contract smoke passes"
fi
if "$ROOT_DIR/scripts/verify-universal-forms-readiness-json.sh" "$READINESS_JSON_PATH" >/dev/null; then
  pass "readiness JSON verifier passes"
else
  fail "readiness JSON verifier passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-readiness-json-contract.sh" "$READINESS_JSON_PATH" >/dev/null; then
  pass "readiness JSON contract smoke passes"
else
  fail "readiness JSON contract smoke passes"
fi
if "$ROOT_DIR/scripts/verify-universal-forms-development-status-json.sh" "$DEVELOPMENT_STATUS_JSON_PATH" >/dev/null; then
  pass "development status JSON verifier passes"
else
  fail "development status JSON verifier passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-development-status-json-contract.sh" "$DEVELOPMENT_STATUS_JSON_PATH" >/dev/null; then
  pass "development status JSON contract smoke passes"
else
  fail "development status JSON contract smoke passes"
fi
if "$ROOT_DIR/scripts/verify-universal-forms-completion-audit-json.sh" "$COMPLETION_AUDIT_JSON_PATH" >/dev/null; then
  pass "completion audit JSON verifier passes"
else
  fail "completion audit JSON verifier passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-completion-audit-json-contract.sh" "$COMPLETION_AUDIT_JSON_PATH" >/dev/null; then
  pass "completion audit JSON contract smoke passes"
else
  fail "completion audit JSON contract smoke passes"
fi
if "$ROOT_DIR/scripts/verify-universal-forms-scenario-catalog-json.sh" "$SCENARIO_CATALOG_JSON_PATH" >/dev/null; then
  pass "scenario catalog JSON verifier passes"
else
  fail "scenario catalog JSON verifier passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-scenario-catalog-json-contract.sh" "$SCENARIO_CATALOG_JSON_PATH" >/dev/null; then
  pass "scenario catalog JSON contract smoke passes"
else
  fail "scenario catalog JSON contract smoke passes"
fi
if "$ROOT_DIR/scripts/verify-universal-forms-implementation-map-json.sh" "$IMPLEMENTATION_MAP_JSON_PATH" >/dev/null; then
  pass "implementation map JSON verifier passes"
else
  fail "implementation map JSON verifier passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-implementation-map-json-contract.sh" "$IMPLEMENTATION_MAP_JSON_PATH" >/dev/null; then
  pass "implementation map JSON contract smoke passes"
else
  fail "implementation map JSON contract smoke passes"
fi

printf '\nManual signoff consistency:\n'
evidence_manual_rows="$(manual_status_rows "$EVIDENCE_PATH" "## Manual Acceptance Signoff")"
completion_manual_rows="$(manual_status_rows "$COMPLETION_AUDIT_PATH" "## Manual Acceptance Signoff")"
packet_manual_rows="$(manual_status_rows "$USER_ACCEPTANCE_PACKET_PATH" "## Manual Signoff Rows")"

if [[ "$evidence_manual_rows" == "$completion_manual_rows" ]]; then
  pass "completion audit manual rows mirror evidence"
else
  fail "completion audit manual rows mirror evidence"
fi

if [[ "$evidence_manual_rows" == "$packet_manual_rows" ]]; then
  pass "user acceptance packet manual rows mirror evidence"
else
  fail "user acceptance packet manual rows mirror evidence"
fi

if "$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh" "$RUNBOOK_PATH" "$EVIDENCE_PATH" >/dev/null; then
  pass "runbook/evidence manual signoff consistency verifier passes"
else
  fail "runbook/evidence manual signoff consistency verifier passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-next-signoff-command.sh" "$SIGNOFF_STATUS_JSON_PATH" >/dev/null; then
  pass "next signoff command smoke passes"
else
  fail "next signoff command smoke passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-manual-signoff-progression.sh" >/dev/null; then
  pass "manual signoff progression smoke passes"
else
  fail "manual signoff progression smoke passes"
fi
if "$ROOT_DIR/scripts/verify-universal-forms-next-signoff-review.sh" "$NEXT_SIGNOFF_REVIEW_PATH" >/dev/null; then
  pass "next signoff review verifier passes"
else
  fail "next signoff review verifier passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-next-signoff-review-contract.sh" "$NEXT_SIGNOFF_REVIEW_PATH" >/dev/null; then
  pass "next signoff review contract smoke passes"
else
  fail "next signoff review contract smoke passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-signoff-status-output.sh" >/dev/null; then
  pass "manual signoff status output smoke passes"
else
  fail "manual signoff status output smoke passes"
fi
if "$ROOT_DIR/scripts/verify-universal-forms-signoff-dashboard.sh" >/dev/null; then
  pass "signoff dashboard verifier passes"
else
  fail "signoff dashboard verifier passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-signoff-dashboard-render.sh" >/dev/null; then
  pass "signoff dashboard browser render smoke passes"
else
  fail "signoff dashboard browser render smoke passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-signoff-dashboard-progression.sh" >/dev/null; then
  pass "signoff dashboard progression smoke passes"
else
  fail "signoff dashboard progression smoke passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-manual-signoff-commands.sh" "$SIGNOFF_STATUS_JSON_PATH" >/dev/null; then
  pass "manual signoff commands smoke passes"
else
  fail "manual signoff commands smoke passes"
fi
if "$ROOT_DIR/scripts/status-universal-forms-delivery.sh" >/dev/null; then
  pass "delivery status command passes"
else
  fail "delivery status command passes"
fi
if "$ROOT_DIR/scripts/status-universal-forms-delivery.sh" --json >/dev/null; then
  pass "delivery status command JSON passes"
else
  fail "delivery status command JSON passes"
fi
if "$ROOT_DIR/scripts/verify-universal-forms-delivery-status-json.sh" >/dev/null; then
  pass "delivery status JSON verifier passes"
else
  fail "delivery status JSON verifier passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-delivery-status-json-contract.sh" >/dev/null; then
  pass "delivery status JSON contract smoke passes"
else
  fail "delivery status JSON contract smoke passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-delivery-status-output.sh" >/dev/null; then
  pass "delivery status output smoke passes"
else
  fail "delivery status output smoke passes"
fi

while IFS='|' read -r item _status; do
  [[ -z "$item" ]] && continue
  contains "manual evidence map covers: $item" "$MANUAL_EVIDENCE_MAP_PATH" "| $item |"
done <<<"$evidence_manual_rows"

printf '\nManual signoff recorder drill:\n'
recorder_drill_dir="$(mktemp -d /tmp/openpr-uf-recorder.XXXXXX)"
cp "$RUNBOOK_PATH" "$recorder_drill_dir/runbook.md"
cp "$EVIDENCE_PATH" "$recorder_drill_dir/evidence.md"
official_runbook_sha_before="$(sha256sum "$RUNBOOK_PATH" | awk '{print $1}')"
official_evidence_sha_before="$(sha256sum "$EVIDENCE_PATH" | awk '{print $1}')"

if "$ROOT_DIR/scripts/record-universal-forms-manual-signoff.sh" \
  --runbook "$recorder_drill_dir/runbook.md" \
  --report "$recorder_drill_dir/evidence.md" \
  --item restaurant_template \
  --status accepted \
  --reviewer "Delivery Bundle Drill" \
  --evidence "temporary recorder dry-run" \
  --dry-run >/dev/null; then
  pass "manual signoff recorder accepted-row dry-run succeeds"
else
  fail "manual signoff recorder accepted-row dry-run succeeds"
fi

if "$ROOT_DIR/scripts/record-universal-forms-manual-signoff.sh" \
  --runbook "$recorder_drill_dir/runbook.md" \
  --report "$recorder_drill_dir/evidence.md" \
  --item overall \
  --status accepted \
  --reviewer "Delivery Bundle Drill" \
  --evidence "temporary early overall dry-run" \
  --dry-run >/dev/null 2>&1; then
  fail "manual signoff recorder blocks early overall dry-run"
else
  pass "manual signoff recorder blocks early overall dry-run"
fi

if cmp -s "$RUNBOOK_PATH" "$recorder_drill_dir/runbook.md" && cmp -s "$EVIDENCE_PATH" "$recorder_drill_dir/evidence.md"; then
  pass "manual signoff recorder dry-run leaves copied files unchanged"
else
  fail "manual signoff recorder dry-run leaves copied files unchanged"
fi

if "$ROOT_DIR/scripts/record-universal-forms-manual-signoff.sh" \
  --runbook "$recorder_drill_dir/runbook.md" \
  --report "$recorder_drill_dir/evidence.md" \
  --item restaurant_template \
  --status accepted \
  --reviewer "Delivery Bundle Drill" \
  --evidence "temporary recorder actual write" >/dev/null; then
  pass "manual signoff recorder custom-path accepted-row write succeeds"
else
  fail "manual signoff recorder custom-path accepted-row write succeeds"
fi

contains "manual signoff recorder writes accepted evidence row to custom report" \
  "$recorder_drill_dir/evidence.md" \
  "| Restaurant template can create a project directly | Accepted | Delivery Bundle Drill | temporary recorder actual write |"
contains "manual signoff recorder writes accepted runbook row to custom runbook" \
  "$recorder_drill_dir/runbook.md" \
  "| 餐厅模板可直接创建项目 | 通过 | Delivery Bundle Drill | temporary recorder actual write |"

if "$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh" \
  "$recorder_drill_dir/runbook.md" \
  "$recorder_drill_dir/evidence.md" >/dev/null; then
  pass "manual signoff recorder custom-path write stays consistent"
else
  fail "manual signoff recorder custom-path write stays consistent"
fi

official_runbook_sha_after="$(sha256sum "$RUNBOOK_PATH" | awk '{print $1}')"
official_evidence_sha_after="$(sha256sum "$EVIDENCE_PATH" | awk '{print $1}')"
equals "manual signoff recorder custom-path write leaves official runbook unchanged" "$official_runbook_sha_after" "$official_runbook_sha_before"
equals "manual signoff recorder custom-path write leaves official evidence unchanged" "$official_evidence_sha_after" "$official_evidence_sha_before"
rm -rf "$recorder_drill_dir"

printf '\nPre-signoff state consistency:\n'
tracker_e2e_status="$(status_for_tracker_row "端到端验收")"
tracker_manual_status="$(status_for_tracker_row "用户侧人工验收")"

if [[ "$evidence_pending_count" == "0" ]]; then
  equals "signed evidence requires accepted tracker end-to-end status" "$tracker_e2e_status" "已验收"
  equals "signed evidence requires accepted tracker manual status" "$tracker_manual_status" "已验收"
  if "$ROOT_DIR/scripts/verify-universal-forms-acceptance-signoff.sh" "$EVIDENCE_PATH" >/dev/null; then
    pass "signed evidence passes signoff verifier"
  else
    fail "signed evidence passes signoff verifier"
  fi
else
  equals "pending evidence keeps tracker end-to-end status pre-signoff" "$tracker_e2e_status" "已测试"
  equals "pending evidence keeps tracker manual status pending" "$tracker_manual_status" "待处理"
  if "$ROOT_DIR/scripts/verify-universal-forms-acceptance-signoff.sh" "$EVIDENCE_PATH" >/dev/null 2>&1; then
    fail "unsigned evidence is rejected by signoff verifier"
  else
    pass "unsigned evidence is rejected by signoff verifier"
  fi
fi

if "$ROOT_DIR/scripts/audit-universal-forms-delivery-state.sh" >/dev/null; then
  pass "default delivery-state audit passes"
else
  fail "default delivery-state audit passes"
fi

printf '\nUI artifact consistency:\n'
if "$ROOT_DIR/scripts/verify-universal-forms-ui-artifacts.sh" >/dev/null; then
  pass "UI artifact verifier passes"
else
  fail "UI artifact verifier passes"
fi
if "$ROOT_DIR/scripts/verify-universal-forms-ui-review-gallery.sh" >/dev/null; then
  pass "UI review gallery verifier passes"
else
  fail "UI review gallery verifier passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-ui-review-gallery-render.sh" >/dev/null; then
  pass "UI review gallery browser render smoke passes"
else
  fail "UI review gallery browser render smoke passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-report-output-boundaries.sh" >/dev/null; then
  pass "report output boundary smoke passes"
else
  fail "report output boundary smoke passes"
fi
contains "user acceptance packet links UI artifact manifest" "$USER_ACCEPTANCE_PACKET_PATH" "$UI_ARTIFACT_MANIFEST_PATH"
contains "user acceptance packet links UI review gallery" "$USER_ACCEPTANCE_PACKET_PATH" "$UI_REVIEW_GALLERY_PATH"
contains "manual evidence map links UI artifact manifest" "$MANUAL_EVIDENCE_MAP_PATH" "$UI_ARTIFACT_MANIFEST_PATH"
contains "user acceptance packet links delivery manifest" "$USER_ACCEPTANCE_PACKET_PATH" "$DELIVERY_MANIFEST_PATH"
contains "manual evidence map links delivery manifest" "$MANUAL_EVIDENCE_MAP_PATH" "$DELIVERY_MANIFEST_PATH"
contains "user acceptance packet links manual signoff status" "$USER_ACCEPTANCE_PACKET_PATH" "$SIGNOFF_STATUS_PATH"
contains "user acceptance packet links manual signoff status JSON" "$USER_ACCEPTANCE_PACKET_PATH" "$SIGNOFF_STATUS_JSON_PATH"
contains "user acceptance packet includes manual signoff key map" "$USER_ACCEPTANCE_PACKET_PATH" "## Manual Signoff Key Map"
contains "user acceptance packet maps docs signoff key" "$USER_ACCEPTANCE_PACKET_PATH" '| `docs` | README/docs are sufficient for a new user to reproduce |'
contains "user acceptance packet maps overall signoff key" "$USER_ACCEPTANCE_PACKET_PATH" '| `overall` | Overall acceptance | All steps after six rows pass |'
contains "readiness summary links manual signoff status" "$READINESS_SUMMARY_PATH" "$SIGNOFF_STATUS_PATH"
contains "readiness summary links manual signoff status JSON" "$READINESS_SUMMARY_PATH" "$SIGNOFF_STATUS_JSON_PATH"
contains "manual signoff status report shows next row" "$SIGNOFF_STATUS_PATH" "Next row:"
contains "readiness summary links user acceptance packet" "$READINESS_SUMMARY_PATH" "$USER_ACCEPTANCE_PACKET_PATH"
contains "readiness summary exposes reviewer commands" "$READINESS_SUMMARY_PATH" "scripts/report-universal-forms-signoff-status.sh"

printf '\nRelease gate:\n'
if "$ROOT_DIR/scripts/gate-universal-forms-release.sh" --allow-pending >/dev/null; then
  pass "release gate allows current pre-signoff handoff state"
else
  fail "release gate allows current pre-signoff handoff state"
fi

if [[ "$evidence_pending_count" == "0" ]]; then
  if "$ROOT_DIR/scripts/gate-universal-forms-release.sh" >/dev/null; then
    pass "strict release gate passes for finalized handoff"
  else
    fail "strict release gate passes for finalized handoff"
  fi
else
  if "$ROOT_DIR/scripts/gate-universal-forms-release.sh" >/dev/null 2>&1; then
    fail "strict release gate rejects pending manual signoff"
  else
    pass "strict release gate rejects pending manual signoff"
  fi
fi

if "$ROOT_DIR/scripts/smoke-universal-forms-release-gate.sh" >/dev/null; then
  pass "release gate smoke passes"
else
  fail "release gate smoke passes"
fi
if "$ROOT_DIR/scripts/verify-universal-forms-release-gate-json.sh" >/dev/null; then
  pass "release gate JSON verifier passes"
else
  fail "release gate JSON verifier passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-release-gate-json-contract.sh" >/dev/null; then
  pass "release gate JSON contract smoke passes"
else
  fail "release gate JSON contract smoke passes"
fi
if "$ROOT_DIR/scripts/smoke-universal-forms-release-gate-output.sh" >/dev/null; then
  pass "release gate output smoke passes"
else
  fail "release gate output smoke passes"
fi

printf '\nFinalizer drill:\n'
drill_dir="$(mktemp -d /tmp/openpr-uf-delivery-bundle.XXXXXX)"
trap 'rm -rf "$drill_dir"' EXIT
cp "$EVIDENCE_PATH" "$drill_dir/evidence.md"
cp "$RUNBOOK_PATH" "$drill_dir/runbook.md"
cp "$TRACKER_PATH" "$drill_dir/tracker.md"
perl -0pi -e 's/\| Pending \|  \|  \|/| Passed | Delivery Bundle Drill | Temporary signed-copy finalizer drill |/g' "$drill_dir/evidence.md"
perl -0pi -e 's/\| 待验收 \|  \|  \|/| 通过 | Delivery Bundle Drill | Temporary signed-copy finalizer drill |/g' "$drill_dir/runbook.md"

if "$ROOT_DIR/scripts/verify-universal-forms-acceptance-signoff.sh" \
  "$drill_dir/evidence.md" \
  --runbook "$drill_dir/missing-runbook.md" >/dev/null 2>&1; then
  fail "signed evidence without runbook is rejected by signoff verifier"
else
  pass "signed evidence without runbook is rejected by signoff verifier"
fi

if "$ROOT_DIR/scripts/finalize-universal-forms-acceptance.sh" \
  --report "$drill_dir/evidence.md" \
  --tracker "$drill_dir/tracker.md" \
  --runbook "$drill_dir/runbook.md" >/dev/null; then
  pass "temporary signed-copy finalizer succeeds"
else
  fail "temporary signed-copy finalizer succeeds"
fi

if "$ROOT_DIR/scripts/audit-universal-forms-delivery-state.sh" \
  --strict \
  --tracker "$drill_dir/tracker.md" \
  --report "$drill_dir/evidence.md" \
  --runbook "$drill_dir/runbook.md" >/dev/null; then
  pass "temporary signed-copy strict delivery audit passes"
else
  fail "temporary signed-copy strict delivery audit passes"
fi

if "$ROOT_DIR/scripts/finalize-universal-forms-acceptance.sh" \
  --report "$drill_dir/evidence.md" \
  --tracker "$drill_dir/tracker.md" \
  --runbook "$drill_dir/runbook.md" >/dev/null; then
  pass "temporary signed-copy finalizer repeat is idempotent"
else
  fail "temporary signed-copy finalizer repeat is idempotent"
fi

final_slot_count="$(rg -c "166\\. 用户侧人工验收完成" "$drill_dir/tracker.md" || true)"
equals "finalized temporary tracker records one final acceptance slot 166" "$final_slot_count" "1"

printf '\nFinal manifest consistency:\n'
if "$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest.sh" "$DELIVERY_MANIFEST_PATH" >/dev/null; then
  pass "final delivery manifest verifier passes after audit drills"
else
  fail "final delivery manifest verifier passes after audit drills"
fi
if "$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest-json.sh" "$DELIVERY_MANIFEST_JSON_PATH" >/dev/null; then
  pass "final delivery manifest JSON verifier passes after audit drills"
else
  fail "final delivery manifest JSON verifier passes after audit drills"
fi

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms delivery bundle audit failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms delivery bundle audit passed.\n'
