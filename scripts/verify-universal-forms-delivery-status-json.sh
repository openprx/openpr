#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
TRACKER_PATH="$REPORT_DIR/openpr-universal-form-development-execution-tracker-2026-05-31.md"
READINESS_JSON_PATH="$REPORT_DIR/openpr-universal-form-readiness-2026-05-31.json"
DEVELOPMENT_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-development-status-2026-05-31.json"
SIGNOFF_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json"
DELIVERY_MANIFEST_JSON_PATH="$REPORT_DIR/openpr-universal-form-delivery-manifest-2026-05-31.json"
MANUAL_RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
AUTOMATED_EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
MANUAL_EVIDENCE_MAP_PATH="$REPORT_DIR/openpr-universal-form-manual-evidence-map-2026-05-31.md"
USER_ACCEPTANCE_PACKET_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-packet-2026-05-31.md"
SIGNOFF_STATUS_REPORT_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.md"
NEXT_SIGNOFF_REVIEW_PATH="$REPORT_DIR/openpr-universal-form-next-signoff-review-2026-05-31.md"
SIGNOFF_DASHBOARD_PATH="$REPORT_DIR/openpr-universal-form-signoff-dashboard-2026-05-31.html"
UI_REVIEW_GALLERY_PATH="$REPORT_DIR/openpr-universal-form-ui-review-gallery-2026-05-31.html"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-delivery-status.schema.json"
RELEASE_GATE_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-release-gate.schema.json"
STATUS_COMMAND="scripts/status-universal-forms-delivery.sh"
NEXT_REVIEW_COMMAND="scripts/verify-universal-forms-next-signoff-review.sh"
SIGNOFF_VERIFY_COMMAND="scripts/verify-universal-forms-acceptance-signoff.sh $AUTOMATED_EVIDENCE_PATH"
FINALIZE_COMMAND="scripts/finalize-universal-forms-acceptance.sh"
JSON_PATH="${1:-}"
GENERATED_TMP=""

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-delivery-status-json.sh [JSON_PATH]

Verifies the compact delivery status JSON emitted by
scripts/status-universal-forms-delivery.sh --json. When JSON_PATH is omitted,
the verifier generates a temporary status JSON from the current handoff bundle.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

cleanup() {
  if [[ -n "$GENERATED_TMP" ]]; then
    rm -f "$GENERATED_TMP"
  fi
}
trap cleanup EXIT

if [[ -z "$JSON_PATH" ]]; then
  GENERATED_TMP="$(mktemp /tmp/openpr-uf-delivery-status.XXXXXX.json)"
  "$ROOT_DIR/scripts/status-universal-forms-delivery.sh" --json >"$GENERATED_TMP"
  JSON_PATH="$GENERATED_TMP"
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

printf 'Universal forms delivery status JSON verification\n'
printf '  JSON: %s\n' "$JSON_PATH"
printf '\n'

if command -v jq >/dev/null 2>&1; then
  pass "jq is available"
else
  fail "jq is available"
fi

for path in \
  "$JSON_PATH" \
  "$SCHEMA_PATH" \
  "$RELEASE_GATE_SCHEMA_PATH" \
  "$TRACKER_PATH" \
  "$READINESS_JSON_PATH" \
  "$DEVELOPMENT_STATUS_JSON_PATH" \
  "$SIGNOFF_STATUS_JSON_PATH" \
  "$DELIVERY_MANIFEST_JSON_PATH" \
  "$MANUAL_RUNBOOK_PATH" \
  "$AUTOMATED_EVIDENCE_PATH" \
  "$MANUAL_EVIDENCE_MAP_PATH" \
  "$USER_ACCEPTANCE_PACKET_PATH" \
  "$SIGNOFF_STATUS_REPORT_PATH" \
  "$NEXT_SIGNOFF_REVIEW_PATH" \
  "$SIGNOFF_DASHBOARD_PATH" \
  "$UI_REVIEW_GALLERY_PATH"; do
  require_file "$path"
done

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms delivery status JSON verification failed before consistency checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

if jq empty "$JSON_PATH" >/dev/null; then
  pass "delivery status JSON is valid JSON"
else
  fail "delivery status JSON is valid JSON"
fi
if jq empty "$SCHEMA_PATH" >/dev/null; then
  pass "delivery status JSON schema is valid JSON"
else
  fail "delivery status JSON schema is valid JSON"
fi

release_gate_json="$("$ROOT_DIR/scripts/gate-universal-forms-release.sh" --allow-pending --json)"

equals "schema version is v1" "$(json_value '.schema_version')" "openpr.universal_forms.delivery_status.v1"
equals "JSON schema path matches repository schema" "$(json_value '.schema_path')" "$SCHEMA_PATH"
equals "schema file pins delivery status JSON v1" "$(jq -r '.properties.schema_version.const' "$SCHEMA_PATH")" "openpr.universal_forms.delivery_status.v1"
equals "schema file disallows top-level additional properties" "$(jq -r '.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file disallows reports additional properties" "$(jq -r '.properties.reports.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file disallows review surfaces additional properties" "$(jq -r '.properties.review_surfaces.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file disallows completion summary additional properties" "$(jq -r '.properties.completion_summary.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file disallows completion breakdown additional properties" "$(jq -r '.properties.completion_breakdown.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file defines completion breakdown row contract" "$(jq -r '.properties.completion_breakdown.properties.engineering_checks["$ref"]' "$SCHEMA_PATH")" '#/$defs/completion_breakdown_row'
equals "schema file disallows completion breakdown row additional properties" "$(jq -r '.["$defs"].completion_breakdown_row.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file enumerates six status markers" "$(jq -r '.["$defs"].status_marker.enum | length' "$SCHEMA_PATH")" "6"
equals "schema file pins three release blockers" "$(jq -r '.properties.release_blockers.prefixItems | length' "$SCHEMA_PATH")" "3"
equals "schema file forbids extra release blockers" "$(jq -r '.properties.release_blockers.items' "$SCHEMA_PATH")" "false"
equals "schema file disallows release blocker row additional properties" "$(jq -r '.["$defs"].release_blocker.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file enumerates release blocker statuses" "$(jq -r '.["$defs"].release_blocker_status.enum | join(",")' "$SCHEMA_PATH")" "clear,blocking"
equals "schema file pins five next actions" "$(jq -r '.properties.next_actions.prefixItems | length' "$SCHEMA_PATH")" "5"
equals "schema file forbids extra next actions" "$(jq -r '.properties.next_actions.items' "$SCHEMA_PATH")" "false"
equals "schema file disallows next action row additional properties" "$(jq -r '.["$defs"].next_action.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file enumerates five next action keys" "$(jq -r '.["$defs"].next_action_key.enum | length' "$SCHEMA_PATH")" "5"
equals "schema file disallows next manual signoff additional properties" "$(jq -r '.properties.next_manual_signoff.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file defines manual signoff queue" "$(jq -r '.properties.manual_signoff_queue.items["$ref"]' "$SCHEMA_PATH")" '#/$defs/manual_queue_row'
equals "schema file disallows manual queue row additional properties" "$(jq -r '.["$defs"].manual_queue_row.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file constrains manual queue key to manual key enum" "$(jq -r '.["$defs"].manual_queue_row.properties.key["$ref"]' "$SCHEMA_PATH")" '#/$defs/manual_key'
equals "schema file disallows final flags additional properties" "$(jq -r '.properties.final_flags.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file disallows release gate additional properties" "$(jq -r '.properties.release_gate.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file pins eight verified prerequisites" "$(jq -r '.properties.verified_prerequisites.prefixItems | length' "$SCHEMA_PATH")" "8"
equals "schema file forbids extra verified prerequisites" "$(jq -r '.properties.verified_prerequisites.items' "$SCHEMA_PATH")" "false"
equals "schema file aligns release gate modes with release gate schema" "$(jq -r --slurpfile release_schema "$RELEASE_GATE_SCHEMA_PATH" '(.properties.release_gate.properties.mode.enum | sort) == ($release_schema[0].properties.mode.enum | sort)' "$SCHEMA_PATH")" "true"
equals "schema file enumerates seven manual signoff keys" "$(jq -r '.["$defs"].manual_key.enum | length' "$SCHEMA_PATH")" "7"
equals "schema file constrains next manual key to manual key enum" "$(jq -r '.properties.next_manual_signoff.properties.key.anyOf[]? | select(.["$ref"] == "#/$defs/manual_key") | .["$ref"]' "$SCHEMA_PATH")" '#/$defs/manual_key'
equals "schema file allows empty final next manual key" "$(jq -r '.properties.next_manual_signoff.properties.key.anyOf[]? | select(.const == "") | .const' "$SCHEMA_PATH")" ""
equals "JSON has every schema top-level required key" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].required - (keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON has no extra top-level keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(keys_unsorted - ($schema[0].properties | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON verified prerequisites match schema order" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '.verified_prerequisites == ($schema[0].properties.verified_prerequisites.prefixItems | map(.const))' "$JSON_PATH")" "true"
equals "JSON verified prerequisites list is exact" "$(jq -r '.verified_prerequisites | join(",")' "$JSON_PATH")" "readiness_json,development_status_json,signoff_status_json,delivery_manifest_json,next_signoff_command_smoke,manual_signoff_progression_smoke,manual_signoff_commands_smoke,release_gate_pre_signoff_json"
equals "JSON reports object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.reports.required - (.reports | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON reports object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.reports | keys_unsorted) - ($schema[0].properties.reports.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON review surfaces object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.review_surfaces.required - (.review_surfaces | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON review surfaces object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.review_surfaces | keys_unsorted) - ($schema[0].properties.review_surfaces.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON completion summary object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.completion_summary.required - (.completion_summary | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON completion summary object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.completion_summary | keys_unsorted) - ($schema[0].properties.completion_summary.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON completion breakdown object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.completion_breakdown.required - (.completion_breakdown | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON completion breakdown object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.completion_breakdown | keys_unsorted) - ($schema[0].properties.completion_breakdown.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON completion breakdown rows have schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.completion_breakdown[]; (($schema[0].["$defs"].completion_breakdown_row.required - (keys_unsorted)) | length) == 0)' "$JSON_PATH")" "true"
equals "JSON completion breakdown rows have no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.completion_breakdown[]; ((keys_unsorted - ($schema[0].["$defs"].completion_breakdown_row.properties | keys_unsorted)) | length) == 0)' "$JSON_PATH")" "true"
equals "JSON release blocker order is exact" "$(jq -r '.release_blockers | map(.key) | join(",")' "$JSON_PATH")" "automated_checks,non_manual_unresolved,manual_signoff"
equals "JSON release blockers have schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.release_blockers[]; (($schema[0].["$defs"].release_blocker.required - (keys_unsorted)) | length) == 0)' "$JSON_PATH")" "true"
equals "JSON release blockers have no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.release_blockers[]; ((keys_unsorted - ($schema[0].["$defs"].release_blocker.properties | keys_unsorted)) | length) == 0)' "$JSON_PATH")" "true"
equals "JSON next action order is exact" "$(jq -r '.next_actions | map(.key) | join(",")' "$JSON_PATH")" "review_status,review_next_signoff,record_next_signoff,verify_manual_signoff,finalize_acceptance"
equals "JSON next actions have schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.next_actions[]; (($schema[0].["$defs"].next_action.required - (keys_unsorted)) | length) == 0)' "$JSON_PATH")" "true"
equals "JSON next actions have no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.next_actions[]; ((keys_unsorted - ($schema[0].["$defs"].next_action.properties | keys_unsorted)) | length) == 0)' "$JSON_PATH")" "true"
equals "JSON next manual signoff object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.next_manual_signoff.required - (.next_manual_signoff | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON next manual signoff object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.next_manual_signoff | keys_unsorted) - ($schema[0].properties.next_manual_signoff.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON manual signoff queue rows have schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.manual_signoff_queue[]; (($schema[0].["$defs"].manual_queue_row.required - (keys_unsorted)) | length) == 0)' "$JSON_PATH")" "true"
equals "JSON manual signoff queue rows have no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.manual_signoff_queue[]; ((keys_unsorted - ($schema[0].["$defs"].manual_queue_row.properties | keys_unsorted)) | length) == 0)' "$JSON_PATH")" "true"
equals "JSON final flags match schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.final_flags.required - (.final_flags | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON final flags object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.final_flags | keys_unsorted) - ($schema[0].properties.final_flags.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON release gate matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.release_gate.required - (.release_gate | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON release gate object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.release_gate | keys_unsorted) - ($schema[0].properties.release_gate.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON release gate mode is allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '.release_gate.mode as $mode | ($schema[0].properties.release_gate.properties.mode.enum | index($mode) != null)' "$JSON_PATH")" "true"
equals "JSON delivery state is allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '.completion_summary.delivery_state as $state | ($schema[0].properties.completion_summary.properties.delivery_state.enum | index($state) != null)' "$JSON_PATH")" "true"
equals "JSON completion breakdown status markers are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.completion_breakdown[].status_marker] as $markers | ($schema[0].["$defs"].status_marker.enum) as $allowed | all($markers[]; $allowed | index(.) != null)' "$JSON_PATH")" "true"
equals "JSON release blocker statuses are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.release_blockers[].status] as $statuses | ($schema[0].["$defs"].release_blocker_status.enum) as $allowed | all($statuses[]; $allowed | index(.) != null)' "$JSON_PATH")" "true"
equals "JSON next action keys are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.next_actions[].key] as $keys | ($schema[0].["$defs"].next_action_key.enum) as $allowed | all($keys[]; $allowed | index(.) != null)' "$JSON_PATH")" "true"
equals "JSON next action blocked_by keys are release blockers" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.next_actions[].blocked_by[]?] as $keys | ($schema[0].["$defs"].release_blocker.properties.key.enum) as $allowed | all($keys[]; $allowed | index(.) != null)' "$JSON_PATH")" "true"
equals "JSON next manual key is allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '.next_manual_signoff.key as $key | ($key == "" or ($schema[0].["$defs"].manual_key.enum | index($key) != null))' "$JSON_PATH")" "true"
equals "JSON next manual status is allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '.next_manual_signoff.status as $status | ($schema[0].["$defs"].manual_status.enum | index($status) != null)' "$JSON_PATH")" "true"
equals "JSON manual signoff queue keys are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '([.manual_signoff_queue[].key] - $schema[0].["$defs"].manual_key.enum) | join(",")' "$JSON_PATH")" ""
equals "JSON manual signoff queue statuses are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.manual_signoff_queue[].status] as $statuses | ($schema[0].["$defs"].manual_status.enum) as $allowed | all($statuses[]; $allowed | index(.) != null)' "$JSON_PATH")" "true"

equals "repository path matches current repository" "$(json_value '.repository')" "$ROOT_DIR"
equals "readiness JSON path matches current report" "$(json_value '.reports.readiness_json')" "$READINESS_JSON_PATH"
equals "development status JSON path matches current report" "$(json_value '.reports.development_status_json')" "$DEVELOPMENT_STATUS_JSON_PATH"
equals "signoff status JSON path matches current report" "$(json_value '.reports.signoff_status_json')" "$SIGNOFF_STATUS_JSON_PATH"
equals "delivery manifest JSON path matches current report" "$(json_value '.reports.delivery_manifest_json')" "$DELIVERY_MANIFEST_JSON_PATH"
equals "manual runbook path matches current report" "$(json_value '.review_surfaces.manual_runbook')" "$MANUAL_RUNBOOK_PATH"
equals "automated evidence path matches current report" "$(json_value '.review_surfaces.automated_evidence')" "$AUTOMATED_EVIDENCE_PATH"
equals "manual evidence map path matches current report" "$(json_value '.review_surfaces.manual_evidence_map')" "$MANUAL_EVIDENCE_MAP_PATH"
equals "user acceptance packet path matches current report" "$(json_value '.review_surfaces.user_acceptance_packet')" "$USER_ACCEPTANCE_PACKET_PATH"
equals "signoff status report path matches current report" "$(json_value '.review_surfaces.signoff_status_report')" "$SIGNOFF_STATUS_REPORT_PATH"
equals "next signoff review path matches current report" "$(json_value '.review_surfaces.next_signoff_review')" "$NEXT_SIGNOFF_REVIEW_PATH"
equals "signoff dashboard path matches current report" "$(json_value '.review_surfaces.signoff_dashboard')" "$SIGNOFF_DASHBOARD_PATH"
equals "UI review gallery path matches current report" "$(json_value '.review_surfaces.ui_review_gallery')" "$UI_REVIEW_GALLERY_PATH"

equals "stage matches readiness JSON" "$(json_value '.stage')" "$(jq -r '.stage' "$READINESS_JSON_PATH")"
equals "final acceptance flag matches readiness JSON" "$(json_value '.final_acceptance_complete')" "$(jq -r '.final_acceptance_complete' "$READINESS_JSON_PATH")"
equals "automated check count matches readiness JSON" "$(json_value '.automated_checks')" "$(jq -r '.gates.automated_checks' "$READINESS_JSON_PATH")"
equals "failed automated check count matches readiness JSON" "$(json_value '.failed_automated_checks')" "$(jq -r '.gates.failed_automated_checks' "$READINESS_JSON_PATH")"
equals "non-manual unresolved count matches readiness JSON" "$(json_value '.non_manual_unresolved_items')" "$(jq -r '.gates.tracker_non_manual_unresolved_items' "$READINESS_JSON_PATH")"
equals "manual pending rows match readiness JSON" "$(json_value '.manual_signoff_pending_rows')" "$(jq -r '.manual_signoff.pending_rows' "$READINESS_JSON_PATH")"
equals "completion summary engineering total matches automated checks" "$(json_value '.completion_summary.engineering_checks_total')" "$(json_value '.automated_checks')"
equals "completion summary engineering completed subtracts failures" "$(json_value '.completion_summary.engineering_checks_completed')" "$(jq -r '.automated_checks - .failed_automated_checks' "$JSON_PATH")"
equals "completion summary engineering complete flag matches counters" "$(json_value '.completion_summary.engineering_complete')" "$(jq -r '.failed_automated_checks == 0 and .non_manual_unresolved_items == 0' "$JSON_PATH")"
equals "completion summary manual accepted rows match signoff status JSON" "$(json_value '.completion_summary.manual_signoff_accepted_rows')" "$(jq -r '.manual_signoff.accepted_rows' "$SIGNOFF_STATUS_JSON_PATH")"
equals "completion summary manual total rows match signoff status JSON" "$(json_value '.completion_summary.manual_signoff_total_rows')" "$(jq -r '.manual_signoff.total_rows' "$SIGNOFF_STATUS_JSON_PATH")"
equals "completion summary manual remaining rows match pending rows" "$(json_value '.completion_summary.manual_signoff_remaining_rows')" "$(json_value '.manual_signoff_pending_rows')"
equals "completion summary manual blocked rows match signoff status JSON" "$(json_value '.completion_summary.manual_signoff_blocked_rows')" "$(jq -r '.manual_signoff.blocked_rows' "$SIGNOFF_STATUS_JSON_PATH")"
equals "completion summary overall completed adds engineering and manual accepted rows" "$(json_value '.completion_summary.overall_items_completed')" "$(jq -r '.completion_summary.engineering_checks_completed + .completion_summary.manual_signoff_accepted_rows' "$JSON_PATH")"
equals "completion summary overall total adds engineering and manual total rows" "$(json_value '.completion_summary.overall_items_total')" "$(jq -r '.completion_summary.engineering_checks_total + .completion_summary.manual_signoff_total_rows' "$JSON_PATH")"
equals "completion summary overall remaining subtracts completed from total" "$(json_value '.completion_summary.overall_items_remaining')" "$(jq -r '.completion_summary.overall_items_total - .completion_summary.overall_items_completed' "$JSON_PATH")"
equals "completion summary overall percent matches overall counters" "$(jq -r 'if .completion_summary.overall_items_total == 0 then .completion_summary.overall_completion_percent == 0 else .completion_summary.overall_completion_percent == ((.completion_summary.overall_items_completed * 10000 / .completion_summary.overall_items_total | floor) / 100) end' "$JSON_PATH")" "true"
equals "completion summary release manual block flag matches status" "$(json_value '.completion_summary.release_blocked_by_manual_signoff')" "$(jq -r '(.release_gate.release_allowed | not) and .manual_signoff_pending_rows > 0' "$JSON_PATH")"
equals "completion summary delivery state matches counters" "$(json_value '.completion_summary.delivery_state')" "$(jq -r 'if .release_gate.release_allowed then "ready_for_release" elif (.failed_automated_checks == 0 and .non_manual_unresolved_items == 0 and .manual_signoff_pending_rows > 0) then "engineering_complete_pending_manual_signoff" else "blocked_or_in_progress" end' "$JSON_PATH")"
equals "completion breakdown engineering completed mirrors summary" "$(json_value '.completion_breakdown.engineering_checks.completed')" "$(json_value '.completion_summary.engineering_checks_completed')"
equals "completion breakdown engineering total mirrors summary" "$(json_value '.completion_breakdown.engineering_checks.total')" "$(json_value '.completion_summary.engineering_checks_total')"
equals "completion breakdown engineering remaining mirrors failures" "$(json_value '.completion_breakdown.engineering_checks.remaining')" "$(json_value '.failed_automated_checks')"
equals "completion breakdown engineering percent matches counters" "$(jq -r 'if .completion_breakdown.engineering_checks.total == 0 then .completion_breakdown.engineering_checks.completion_percent == 0 else .completion_breakdown.engineering_checks.completion_percent == ((.completion_breakdown.engineering_checks.completed * 10000 / .completion_breakdown.engineering_checks.total | floor) / 100) end' "$JSON_PATH")" "true"
equals "completion breakdown engineering status matches counters" "$(json_value '.completion_breakdown.engineering_checks.status_marker')" "$(jq -r 'if (.failed_automated_checks == 0 and .non_manual_unresolved_items == 0) then "已测试" else "开发中" end' "$JSON_PATH")"
equals "completion breakdown manual completed mirrors summary" "$(json_value '.completion_breakdown.manual_signoff.completed')" "$(json_value '.completion_summary.manual_signoff_accepted_rows')"
equals "completion breakdown manual total mirrors summary" "$(json_value '.completion_breakdown.manual_signoff.total')" "$(json_value '.completion_summary.manual_signoff_total_rows')"
equals "completion breakdown manual remaining mirrors summary" "$(json_value '.completion_breakdown.manual_signoff.remaining')" "$(json_value '.completion_summary.manual_signoff_remaining_rows')"
equals "completion breakdown manual percent matches counters" "$(jq -r 'if .completion_breakdown.manual_signoff.total == 0 then .completion_breakdown.manual_signoff.completion_percent == 0 else .completion_breakdown.manual_signoff.completion_percent == ((.completion_breakdown.manual_signoff.completed * 10000 / .completion_breakdown.manual_signoff.total | floor) / 100) end' "$JSON_PATH")" "true"
equals "completion breakdown manual status matches counters" "$(json_value '.completion_breakdown.manual_signoff.status_marker')" "$(jq -r 'if (.completion_summary.manual_signoff_remaining_rows == 0 and .completion_summary.manual_signoff_blocked_rows == 0) then "已验收" elif .completion_summary.manual_signoff_blocked_rows > 0 then "阻塞" else "待处理" end' "$JSON_PATH")"
equals "completion breakdown overall completed mirrors summary" "$(json_value '.completion_breakdown.overall_handoff.completed')" "$(json_value '.completion_summary.overall_items_completed')"
equals "completion breakdown overall total mirrors summary" "$(json_value '.completion_breakdown.overall_handoff.total')" "$(json_value '.completion_summary.overall_items_total')"
equals "completion breakdown overall remaining mirrors summary" "$(json_value '.completion_breakdown.overall_handoff.remaining')" "$(json_value '.completion_summary.overall_items_remaining')"
equals "completion breakdown overall percent mirrors summary" "$(json_value '.completion_breakdown.overall_handoff.completion_percent')" "$(json_value '.completion_summary.overall_completion_percent')"
equals "completion breakdown overall status matches release state" "$(json_value '.completion_breakdown.overall_handoff.status_marker')" "$(jq -r 'if .release_gate.release_allowed then "已验收" elif (.failed_automated_checks == 0 and .non_manual_unresolved_items == 0 and .manual_signoff_pending_rows > 0) then "待处理" else "开发中" end' "$JSON_PATH")"
equals "release blocker automated count matches failed checks" "$(jq -r '.release_blockers[0].count' "$JSON_PATH")" "$(json_value '.failed_automated_checks')"
equals "release blocker automated status matches count" "$(jq -r '.release_blockers[0].status' "$JSON_PATH")" "$(jq -r 'if .failed_automated_checks == 0 then "clear" else "blocking" end' "$JSON_PATH")"
equals "release blocker automated flag matches status" "$(jq -r '.release_blockers[0].blocking' "$JSON_PATH")" "$(jq -r '.failed_automated_checks > 0' "$JSON_PATH")"
equals "release blocker automated evidence points to automated evidence" "$(jq -r '.release_blockers[0].evidence' "$JSON_PATH")" "$AUTOMATED_EVIDENCE_PATH"
equals "release blocker non-manual count matches unresolved items" "$(jq -r '.release_blockers[1].count' "$JSON_PATH")" "$(json_value '.non_manual_unresolved_items')"
equals "release blocker non-manual status matches count" "$(jq -r '.release_blockers[1].status' "$JSON_PATH")" "$(jq -r 'if .non_manual_unresolved_items == 0 then "clear" else "blocking" end' "$JSON_PATH")"
equals "release blocker non-manual flag matches status" "$(jq -r '.release_blockers[1].blocking' "$JSON_PATH")" "$(jq -r '.non_manual_unresolved_items > 0' "$JSON_PATH")"
equals "release blocker non-manual evidence points to development status" "$(jq -r '.release_blockers[1].evidence' "$JSON_PATH")" "$DEVELOPMENT_STATUS_JSON_PATH"
equals "release blocker manual count matches pending rows" "$(jq -r '.release_blockers[2].count' "$JSON_PATH")" "$(json_value '.manual_signoff_pending_rows')"
equals "release blocker manual status matches pending or blocked rows" "$(jq -r '.release_blockers[2].status' "$JSON_PATH")" "$(jq -r 'if (.manual_signoff_pending_rows == 0 and .completion_summary.manual_signoff_blocked_rows == 0) then "clear" else "blocking" end' "$JSON_PATH")"
equals "release blocker manual flag matches pending or blocked rows" "$(jq -r '.release_blockers[2].blocking' "$JSON_PATH")" "$(jq -r '.manual_signoff_pending_rows > 0 or .completion_summary.manual_signoff_blocked_rows > 0' "$JSON_PATH")"
equals "release blocker manual evidence points to manual runbook" "$(jq -r '.release_blockers[2].evidence' "$JSON_PATH")" "$MANUAL_RUNBOOK_PATH"
equals "release blockers explain release allowed false" "$(jq -r 'if .release_gate.release_allowed then ([.release_blockers[] | select(.blocking == true)] | length) == 0 else ([.release_blockers[] | select(.blocking == true)] | length) > 0 end' "$JSON_PATH")" "true"
equals "next action review status is always enabled" "$(jq -r '.next_actions[0].enabled' "$JSON_PATH")" "true"
equals "next action review status command matches CLI" "$(jq -r '.next_actions[0].command' "$JSON_PATH")" "$STATUS_COMMAND"
equals "next action review status evidence points to packet" "$(jq -r '.next_actions[0].evidence' "$JSON_PATH")" "$USER_ACCEPTANCE_PACKET_PATH"
equals "next action review next signoff enabled matches pending rows" "$(jq -r '.next_actions[1].enabled' "$JSON_PATH")" "$(jq -r '.manual_signoff_pending_rows > 0' "$JSON_PATH")"
equals "next action review next signoff command matches verifier" "$(jq -r '.next_actions[1].command' "$JSON_PATH")" "$NEXT_REVIEW_COMMAND"
equals "next action review next signoff evidence points to next review" "$(jq -r '.next_actions[1].evidence' "$JSON_PATH")" "$NEXT_SIGNOFF_REVIEW_PATH"
equals "next action record next signoff enabled matches pending command" "$(jq -r '.next_actions[2].enabled' "$JSON_PATH")" "$(jq -r '.manual_signoff_pending_rows > 0 and .next_manual_signoff.recorder_command != ""' "$JSON_PATH")"
equals "next action record next signoff command mirrors recorder" "$(jq -r '.next_actions[2].command' "$JSON_PATH")" "$(jq -r '.next_manual_signoff.recorder_command' "$JSON_PATH")"
equals "next action record next signoff evidence points to runbook" "$(jq -r '.next_actions[2].evidence' "$JSON_PATH")" "$MANUAL_RUNBOOK_PATH"
equals "next action verify manual signoff enabled matches blockers" "$(jq -r '.next_actions[3].enabled' "$JSON_PATH")" "$(jq -r '.failed_automated_checks == 0 and .non_manual_unresolved_items == 0 and .manual_signoff_pending_rows == 0 and .completion_summary.manual_signoff_blocked_rows == 0' "$JSON_PATH")"
equals "next action verify manual signoff blocked_by matches blockers" "$(jq -r '.next_actions[3].blocked_by | join(",")' "$JSON_PATH")" "$(jq -r '[if .failed_automated_checks > 0 then "automated_checks" else empty end, if .non_manual_unresolved_items > 0 then "non_manual_unresolved" else empty end, if (.manual_signoff_pending_rows > 0 or .completion_summary.manual_signoff_blocked_rows > 0) then "manual_signoff" else empty end] | join(",")' "$JSON_PATH")"
equals "next action verify manual signoff command matches verifier" "$(jq -r '.next_actions[3].command' "$JSON_PATH")" "$SIGNOFF_VERIFY_COMMAND"
equals "next action verify manual signoff evidence points to evidence report" "$(jq -r '.next_actions[3].evidence' "$JSON_PATH")" "$AUTOMATED_EVIDENCE_PATH"
equals "next action finalize acceptance enabled matches signoff flag" "$(jq -r '.next_actions[4].enabled' "$JSON_PATH")" "$(jq -r '.final_flags.signoff_allowed' "$JSON_PATH")"
equals "next action finalize acceptance blocked_by matches manual blocker" "$(jq -r '.next_actions[4].blocked_by | join(",")' "$JSON_PATH")" "$(jq -r '[if (.manual_signoff_pending_rows > 0 or .completion_summary.manual_signoff_blocked_rows > 0) then "manual_signoff" else empty end] | join(",")' "$JSON_PATH")"
equals "next action finalize acceptance command matches finalizer" "$(jq -r '.next_actions[4].command' "$JSON_PATH")" "$FINALIZE_COMMAND"
equals "next action finalize acceptance evidence points to tracker" "$(jq -r '.next_actions[4].evidence' "$JSON_PATH")" "$TRACKER_PATH"
equals "next manual key matches readiness JSON" "$(json_value '.next_manual_signoff.key')" "$(jq -r '.manual_signoff.next_row.key // ""' "$READINESS_JSON_PATH")"
equals "next manual item matches readiness JSON" "$(json_value '.next_manual_signoff.item')" "$(jq -r '.manual_signoff.next_row.item // ""' "$READINESS_JSON_PATH")"
equals "next recorder command matches readiness JSON" "$(json_value '.next_manual_signoff.recorder_command')" "$(jq -r '.manual_signoff.next_row.recorder_command // ""' "$READINESS_JSON_PATH")"
equals "next manual key matches signoff status JSON" "$(json_value '.next_manual_signoff.key')" "$(jq -r '.manual_signoff.next_row.key // ""' "$SIGNOFF_STATUS_JSON_PATH")"
equals "next manual status matches signoff status JSON" "$(json_value '.next_manual_signoff.status')" "$(jq -r '.manual_signoff.next_row.status // "pending"' "$SIGNOFF_STATUS_JSON_PATH")"
equals "next manual actionable matches signoff status JSON" "$(json_value '.next_manual_signoff.actionable')" "$(jq -r '.manual_signoff.next_row.actionable // false' "$SIGNOFF_STATUS_JSON_PATH")"
equals "next manual automated evidence matches signoff status JSON" "$(json_value '.next_manual_signoff.automated_evidence')" "$(jq -r '.manual_signoff.next_row.automated_evidence // ""' "$SIGNOFF_STATUS_JSON_PATH")"
equals "next manual reviewer check matches signoff status JSON" "$(json_value '.next_manual_signoff.reviewer_check')" "$(jq -r '.manual_signoff.next_row.reviewer_check // ""' "$SIGNOFF_STATUS_JSON_PATH")"
equals "next manual suggested evidence matches signoff status JSON" "$(json_value '.next_manual_signoff.suggested_evidence_note')" "$(jq -r '.manual_signoff.next_row.suggested_evidence_note // ""' "$SIGNOFF_STATUS_JSON_PATH")"
equals "manual signoff queue mirrors signoff status JSON" "$(jq -r --slurpfile signoff "$SIGNOFF_STATUS_JSON_PATH" '.manual_signoff_queue == $signoff[0].manual_signoff.pending_queue' "$JSON_PATH")" "true"
equals "manual signoff queue length matches pending rows" "$(json_value '.manual_signoff_queue | length')" "$(json_value '.manual_signoff_pending_rows')"
equals "manual signoff queue first row mirrors next manual key" "$(jq -r 'if (.manual_signoff_queue | length) == 0 then .next_manual_signoff.key == "" else .manual_signoff_queue[0].key == .next_manual_signoff.key end' "$JSON_PATH")" "true"
equals "manual signoff queue next marker mirrors next manual key" "$(jq -r 'if (.manual_signoff_queue | length) == 0 then true else ([.manual_signoff_queue[] | select(.is_next == true) | .key] == [.next_manual_signoff.key]) end' "$JSON_PATH")" "true"
equals "manual signoff queue has at most one actionable row" "$(jq -r '[.manual_signoff_queue[] | select(.actionable == true)] | length <= 1' "$JSON_PATH")" "true"
equals "manifest file count matches delivery manifest JSON" "$(json_value '.manifest_file_count')" "$(jq -r '.file_count' "$DELIVERY_MANIFEST_JSON_PATH")"
equals "signoff final flag matches signoff status JSON" "$(json_value '.final_flags.signoff_allowed')" "$(jq -r '.manual_signoff.final_signoff_allowed' "$SIGNOFF_STATUS_JSON_PATH")"
equals "development release flag matches development status JSON" "$(json_value '.final_flags.development_release_allowed')" "$(jq -r '.status_summary.final_release_allowed' "$DEVELOPMENT_STATUS_JSON_PATH")"
equals "release gate mode matches release gate JSON" "$(json_value '.release_gate.mode')" "$(jq -r '.mode' <<<"$release_gate_json")"
equals "release gate allowed flag matches release gate JSON" "$(json_value '.release_gate.release_allowed')" "$(jq -r '.release_allowed' <<<"$release_gate_json")"
equals "release gate reason matches release gate JSON" "$(json_value '.release_gate.reason')" "$(jq -r '.reason' <<<"$release_gate_json")"

equals "integer counters are typed as numbers" "$(jq -r '[
  (.automated_checks | type),
  (.failed_automated_checks | type),
  (.non_manual_unresolved_items | type),
  (.manual_signoff_pending_rows | type),
  (.completion_summary.engineering_checks_completed | type),
  (.completion_summary.engineering_checks_total | type),
  (.completion_summary.manual_signoff_accepted_rows | type),
  (.completion_summary.manual_signoff_total_rows | type),
  (.completion_summary.manual_signoff_remaining_rows | type),
  (.completion_summary.manual_signoff_blocked_rows | type),
  (.completion_summary.overall_items_completed | type),
  (.completion_summary.overall_items_total | type),
  (.completion_summary.overall_items_remaining | type),
  (.completion_breakdown.engineering_checks.completed | type),
  (.completion_breakdown.engineering_checks.total | type),
  (.completion_breakdown.engineering_checks.remaining | type),
  (.completion_breakdown.manual_signoff.completed | type),
  (.completion_breakdown.manual_signoff.total | type),
  (.completion_breakdown.manual_signoff.remaining | type),
  (.completion_breakdown.overall_handoff.completed | type),
  (.completion_breakdown.overall_handoff.total | type),
  (.completion_breakdown.overall_handoff.remaining | type),
  (.release_blockers[0].count | type),
  (.release_blockers[1].count | type),
  (.release_blockers[2].count | type),
  (.manifest_file_count | type)
] | unique | join(",")' "$JSON_PATH")" "number"
equals "completion percents are typed as numbers" "$(jq -r '[
  (.completion_summary.overall_completion_percent | type),
  (.completion_breakdown.engineering_checks.completion_percent | type),
  (.completion_breakdown.manual_signoff.completion_percent | type),
  (.completion_breakdown.overall_handoff.completion_percent | type)
] | unique | join(",")' "$JSON_PATH")" "number"
equals "boolean flags are typed as booleans" "$(jq -r '[
  (.final_acceptance_complete | type),
  (.completion_summary.engineering_complete | type),
  (.completion_summary.release_blocked_by_manual_signoff | type),
  (.release_blockers[0].blocking | type),
  (.release_blockers[1].blocking | type),
  (.release_blockers[2].blocking | type),
  (.next_manual_signoff.actionable | type),
  (.next_actions[0].enabled | type),
  (.next_actions[1].enabled | type),
  (.next_actions[2].enabled | type),
  (.next_actions[3].enabled | type),
  (.next_actions[4].enabled | type),
  (.final_flags.signoff_allowed | type),
  (.final_flags.development_release_allowed | type),
  (.release_gate.release_allowed | type)
] | unique | join(",")' "$JSON_PATH")" "boolean"

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms delivery status JSON verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms delivery status JSON verification passed.\n'
