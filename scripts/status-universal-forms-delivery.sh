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
OUTPUT_JSON=0

usage() {
  cat <<'EOF'
Usage: scripts/status-universal-forms-delivery.sh [--json]

Prints the current universal forms delivery state from the machine-readable
handoff reports. This is read-only: it verifies JSON contracts, checks the
next manual signoff command smoke, checks row-by-row manual signoff progression,
checks every manual signoff command on temporary copies, runs the release gate
in pre-signoff mode, and never records acceptance.

Options:
  --json  Print a compact machine-readable status summary.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --json)
      OUTPUT_JSON=1
      shift
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

require_file() {
  local path="$1"
  if [[ ! -f "$path" ]]; then
    echo "Missing required file: $path" >&2
    exit 1
  fi
}

run_quiet() {
  local output_path
  output_path="$(mktemp /tmp/openpr-uf-delivery-status-check.XXXXXX)"
  if "$@" >"$output_path" 2>&1; then
    rm -f "$output_path"
  else
    local status=$?
    printf 'Delivery status prerequisite failed: %s\n' "$*" >&2
    cat "$output_path" >&2
    rm -f "$output_path"
    exit "$status"
  fi
}

if ! command -v jq >/dev/null 2>&1; then
  echo "Missing required command: jq" >&2
  exit 2
fi

for path in \
  "$READINESS_JSON_PATH" \
  "$TRACKER_PATH" \
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
  "$UI_REVIEW_GALLERY_PATH" \
  "$SCHEMA_PATH"; do
  require_file "$path"
done

run_quiet "$ROOT_DIR/scripts/verify-universal-forms-readiness-json.sh" "$READINESS_JSON_PATH"
run_quiet "$ROOT_DIR/scripts/verify-universal-forms-development-status-json.sh" "$DEVELOPMENT_STATUS_JSON_PATH"
run_quiet "$ROOT_DIR/scripts/verify-universal-forms-signoff-status-json.sh" "$SIGNOFF_STATUS_JSON_PATH"
run_quiet "$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest-json.sh" "$DELIVERY_MANIFEST_JSON_PATH"
run_quiet "$ROOT_DIR/scripts/smoke-universal-forms-next-signoff-command.sh" "$SIGNOFF_STATUS_JSON_PATH"
run_quiet "$ROOT_DIR/scripts/smoke-universal-forms-manual-signoff-progression.sh"
run_quiet "$ROOT_DIR/scripts/smoke-universal-forms-manual-signoff-commands.sh" "$SIGNOFF_STATUS_JSON_PATH"

release_gate_json="$("$ROOT_DIR/scripts/gate-universal-forms-release.sh" --allow-pending --json)"

stage="$(jq -r '.stage' "$READINESS_JSON_PATH")"
final_acceptance_complete="$(jq -r '.final_acceptance_complete' "$READINESS_JSON_PATH")"
automated_checks="$(jq -r '.gates.automated_checks' "$READINESS_JSON_PATH")"
failed_automated_checks="$(jq -r '.gates.failed_automated_checks' "$READINESS_JSON_PATH")"
non_manual_unresolved="$(jq -r '.gates.tracker_non_manual_unresolved_items' "$READINESS_JSON_PATH")"
manual_pending="$(jq -r '.manual_signoff.pending_rows' "$READINESS_JSON_PATH")"
manual_total="$(jq -r '.manual_signoff.total_rows' "$SIGNOFF_STATUS_JSON_PATH")"
manual_accepted="$(jq -r '.manual_signoff.accepted_rows' "$SIGNOFF_STATUS_JSON_PATH")"
manual_blocked="$(jq -r '.manual_signoff.blocked_rows' "$SIGNOFF_STATUS_JSON_PATH")"
manual_signoff_queue="$(jq '.manual_signoff.pending_queue' "$SIGNOFF_STATUS_JSON_PATH")"
next_key="$(jq -r '.manual_signoff.next_row.key // ""' "$READINESS_JSON_PATH")"
next_item="$(jq -r '.manual_signoff.next_row.item // ""' "$READINESS_JSON_PATH")"
next_command="$(jq -r '.manual_signoff.next_row.recorder_command // ""' "$READINESS_JSON_PATH")"
next_status="$(jq -r '.manual_signoff.next_row.status // "pending"' "$SIGNOFF_STATUS_JSON_PATH")"
next_actionable="$(jq -r '.manual_signoff.next_row.actionable // false' "$SIGNOFF_STATUS_JSON_PATH")"
next_automated_evidence="$(jq -r '.manual_signoff.next_row.automated_evidence // ""' "$SIGNOFF_STATUS_JSON_PATH")"
next_reviewer_check="$(jq -r '.manual_signoff.next_row.reviewer_check // ""' "$SIGNOFF_STATUS_JSON_PATH")"
next_suggested_evidence_note="$(jq -r '.manual_signoff.next_row.suggested_evidence_note // ""' "$SIGNOFF_STATUS_JSON_PATH")"
status_command="scripts/status-universal-forms-delivery.sh"
next_review_command="scripts/verify-universal-forms-next-signoff-review.sh"
signoff_verify_command="scripts/verify-universal-forms-acceptance-signoff.sh $AUTOMATED_EVIDENCE_PATH"
finalize_command="scripts/finalize-universal-forms-acceptance.sh"
development_final_release_allowed="$(jq -r '.status_summary.final_release_allowed' "$DEVELOPMENT_STATUS_JSON_PATH")"
signoff_final_allowed="$(jq -r '.manual_signoff.final_signoff_allowed' "$SIGNOFF_STATUS_JSON_PATH")"
manifest_file_count="$(jq -r '.file_count' "$DELIVERY_MANIFEST_JSON_PATH")"
release_mode="$(jq -r '.mode' <<<"$release_gate_json")"
release_allowed="$(jq -r '.release_allowed' <<<"$release_gate_json")"
release_reason="$(jq -r '.reason' <<<"$release_gate_json")"
generated_at="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
engineering_completed=$((automated_checks - failed_automated_checks))
overall_items_completed=$((engineering_completed + manual_accepted))
overall_items_total=$((automated_checks + manual_total))
overall_items_remaining=$((overall_items_total - overall_items_completed))
overall_completion_percent="$(jq -n \
  --argjson completed "$overall_items_completed" \
  --argjson total "$overall_items_total" \
  'if $total == 0 then 0 else (($completed * 10000 / $total | floor) / 100) end')"
engineering_items_remaining=$((automated_checks - engineering_completed))
engineering_completion_percent="$(jq -n \
  --argjson completed "$engineering_completed" \
  --argjson total "$automated_checks" \
  'if $total == 0 then 0 else (($completed * 10000 / $total | floor) / 100) end')"
manual_signoff_completion_percent="$(jq -n \
  --argjson completed "$manual_accepted" \
  --argjson total "$manual_total" \
  'if $total == 0 then 0 else (($completed * 10000 / $total | floor) / 100) end')"

if [[ "$OUTPUT_JSON" -eq 1 ]]; then
  jq -n \
    --arg schema_version "openpr.universal_forms.delivery_status.v1" \
    --arg schema_path "$SCHEMA_PATH" \
    --arg generated_at "$generated_at" \
    --arg repository "$ROOT_DIR" \
    --arg readiness_json "$READINESS_JSON_PATH" \
    --arg development_status_json "$DEVELOPMENT_STATUS_JSON_PATH" \
    --arg signoff_status_json "$SIGNOFF_STATUS_JSON_PATH" \
    --arg delivery_manifest_json "$DELIVERY_MANIFEST_JSON_PATH" \
    --arg manual_runbook "$MANUAL_RUNBOOK_PATH" \
    --arg automated_evidence "$AUTOMATED_EVIDENCE_PATH" \
    --arg manual_evidence_map "$MANUAL_EVIDENCE_MAP_PATH" \
    --arg user_acceptance_packet "$USER_ACCEPTANCE_PACKET_PATH" \
    --arg signoff_status_report "$SIGNOFF_STATUS_REPORT_PATH" \
    --arg next_signoff_review "$NEXT_SIGNOFF_REVIEW_PATH" \
    --arg signoff_dashboard "$SIGNOFF_DASHBOARD_PATH" \
    --arg ui_review_gallery "$UI_REVIEW_GALLERY_PATH" \
    --arg stage "$stage" \
    --arg next_key "$next_key" \
    --arg next_item "$next_item" \
    --arg next_command "$next_command" \
    --arg next_status "$next_status" \
    --arg next_automated_evidence "$next_automated_evidence" \
    --arg next_reviewer_check "$next_reviewer_check" \
    --arg next_suggested_evidence_note "$next_suggested_evidence_note" \
    --arg status_command "$status_command" \
    --arg next_review_command "$next_review_command" \
    --arg signoff_verify_command "$signoff_verify_command" \
    --arg finalize_command "$finalize_command" \
    --arg tracker_path "$TRACKER_PATH" \
    --arg release_mode "$release_mode" \
    --arg release_reason "$release_reason" \
    --argjson final_acceptance_complete "$final_acceptance_complete" \
    --argjson automated_checks "$automated_checks" \
    --argjson failed_automated_checks "$failed_automated_checks" \
    --argjson non_manual_unresolved "$non_manual_unresolved" \
    --argjson manual_pending "$manual_pending" \
    --argjson manual_total "$manual_total" \
    --argjson manual_accepted "$manual_accepted" \
    --argjson manual_blocked "$manual_blocked" \
    --argjson manual_signoff_queue "$manual_signoff_queue" \
    --argjson development_final_release_allowed "$development_final_release_allowed" \
    --argjson signoff_final_allowed "$signoff_final_allowed" \
    --argjson manifest_file_count "$manifest_file_count" \
    --argjson release_allowed "$release_allowed" \
    --argjson next_actionable "$next_actionable" \
    --argjson overall_items_completed "$overall_items_completed" \
    --argjson overall_items_total "$overall_items_total" \
    --argjson overall_items_remaining "$overall_items_remaining" \
    --argjson overall_completion_percent "$overall_completion_percent" \
    --argjson engineering_items_remaining "$engineering_items_remaining" \
    --argjson engineering_completion_percent "$engineering_completion_percent" \
    --argjson manual_signoff_completion_percent "$manual_signoff_completion_percent" \
    '{
      schema_version: $schema_version,
      schema_path: $schema_path,
      generated_at: $generated_at,
      repository: $repository,
      verified_prerequisites: [
        "readiness_json",
        "development_status_json",
        "signoff_status_json",
        "delivery_manifest_json",
        "next_signoff_command_smoke",
        "manual_signoff_progression_smoke",
        "manual_signoff_commands_smoke",
        "release_gate_pre_signoff_json"
      ],
      reports: {
        readiness_json: $readiness_json,
        development_status_json: $development_status_json,
        signoff_status_json: $signoff_status_json,
        delivery_manifest_json: $delivery_manifest_json
      },
      review_surfaces: {
        manual_runbook: $manual_runbook,
        automated_evidence: $automated_evidence,
        manual_evidence_map: $manual_evidence_map,
        user_acceptance_packet: $user_acceptance_packet,
        signoff_status_report: $signoff_status_report,
        next_signoff_review: $next_signoff_review,
        signoff_dashboard: $signoff_dashboard,
        ui_review_gallery: $ui_review_gallery
      },
      stage: $stage,
      final_acceptance_complete: $final_acceptance_complete,
      automated_checks: $automated_checks,
      failed_automated_checks: $failed_automated_checks,
      non_manual_unresolved_items: $non_manual_unresolved,
      manual_signoff_pending_rows: $manual_pending,
      completion_summary: {
        engineering_checks_completed: ($automated_checks - $failed_automated_checks),
        engineering_checks_total: $automated_checks,
        engineering_complete: ($failed_automated_checks == 0 and $non_manual_unresolved == 0),
        manual_signoff_accepted_rows: $manual_accepted,
        manual_signoff_total_rows: $manual_total,
        manual_signoff_remaining_rows: $manual_pending,
        manual_signoff_blocked_rows: $manual_blocked,
        overall_items_completed: $overall_items_completed,
        overall_items_total: $overall_items_total,
        overall_items_remaining: $overall_items_remaining,
        overall_completion_percent: $overall_completion_percent,
        release_blocked_by_manual_signoff: (($release_allowed | not) and $manual_pending > 0),
        delivery_state: (
          if $release_allowed then
            "ready_for_release"
          elif ($failed_automated_checks == 0 and $non_manual_unresolved == 0 and $manual_pending > 0) then
            "engineering_complete_pending_manual_signoff"
          else
            "blocked_or_in_progress"
          end
        )
      },
      completion_breakdown: {
        engineering_checks: {
          label: "Engineering automated checks",
          status_marker: (
            if ($failed_automated_checks == 0 and $non_manual_unresolved == 0) then
              "已测试"
            else
              "开发中"
            end
          ),
          completed: ($automated_checks - $failed_automated_checks),
          total: $automated_checks,
          remaining: $engineering_items_remaining,
          completion_percent: $engineering_completion_percent
        },
        manual_signoff: {
          label: "User-side manual signoff",
          status_marker: (
            if $manual_pending == 0 and $manual_blocked == 0 then
              "已验收"
            elif $manual_blocked > 0 then
              "阻塞"
            else
              "待处理"
            end
          ),
          completed: $manual_accepted,
          total: $manual_total,
          remaining: $manual_pending,
          completion_percent: $manual_signoff_completion_percent
        },
        overall_handoff: {
          label: "Overall delivery handoff",
          status_marker: (
            if $release_allowed then
              "已验收"
            elif ($failed_automated_checks == 0 and $non_manual_unresolved == 0 and $manual_pending > 0) then
              "待处理"
            else
              "开发中"
            end
          ),
          completed: $overall_items_completed,
          total: $overall_items_total,
          remaining: $overall_items_remaining,
          completion_percent: $overall_completion_percent
        }
      },
      release_blockers: [
        {
          key: "automated_checks",
          label: "Automated checks",
          status: (if $failed_automated_checks == 0 then "clear" else "blocking" end),
          blocking: ($failed_automated_checks > 0),
          count: $failed_automated_checks,
          required_action: "Keep automated evidence at 0 failed checks",
          evidence: $automated_evidence
        },
        {
          key: "non_manual_unresolved",
          label: "Non-manual tracker items",
          status: (if $non_manual_unresolved == 0 then "clear" else "blocking" end),
          blocking: ($non_manual_unresolved > 0),
          count: $non_manual_unresolved,
          required_action: "Resolve non-manual tracker rows before final release",
          evidence: $development_status_json
        },
        {
          key: "manual_signoff",
          label: "User-side manual signoff",
          status: (if $manual_pending == 0 and $manual_blocked == 0 then "clear" else "blocking" end),
          blocking: ($manual_pending > 0 or $manual_blocked > 0),
          count: $manual_pending,
          required_action: "Complete every pending Manual Acceptance Signoff row",
          evidence: $manual_runbook
        }
      ],
      next_manual_signoff: {
        key: $next_key,
        item: $next_item,
        status: $next_status,
        actionable: $next_actionable,
        automated_evidence: $next_automated_evidence,
        reviewer_check: $next_reviewer_check,
        suggested_evidence_note: $next_suggested_evidence_note,
        recorder_command: $next_command
      },
      next_actions: [
        {
          key: "review_status",
          label: "Review current delivery status",
          enabled: true,
          blocked_by: [],
          command: $status_command,
          evidence: $user_acceptance_packet
        },
        {
          key: "review_next_signoff",
          label: "Review the next manual signoff row",
          enabled: ($manual_pending > 0),
          blocked_by: [],
          command: $next_review_command,
          evidence: $next_signoff_review
        },
        {
          key: "record_next_signoff",
          label: "Record the next manual signoff after reviewer approval",
          enabled: ($manual_pending > 0 and $next_command != ""),
          blocked_by: [],
          command: $next_command,
          evidence: $manual_runbook
        },
        {
          key: "verify_manual_signoff",
          label: "Verify all manual signoff rows",
          enabled: ($failed_automated_checks == 0 and $non_manual_unresolved == 0 and $manual_pending == 0 and $manual_blocked == 0),
          blocked_by: [
            (if $failed_automated_checks > 0 then "automated_checks" else empty end),
            (if $non_manual_unresolved > 0 then "non_manual_unresolved" else empty end),
            (if ($manual_pending > 0 or $manual_blocked > 0) then "manual_signoff" else empty end)
          ],
          command: $signoff_verify_command,
          evidence: $automated_evidence
        },
        {
          key: "finalize_acceptance",
          label: "Finalize accepted delivery tracker",
          enabled: $signoff_final_allowed,
          blocked_by: [
            (if ($manual_pending > 0 or $manual_blocked > 0) then "manual_signoff" else empty end)
          ],
          command: $finalize_command,
          evidence: $tracker_path
        }
      ],
      manual_signoff_queue: $manual_signoff_queue,
      manifest_file_count: $manifest_file_count,
      final_flags: {
        signoff_allowed: $signoff_final_allowed,
        development_release_allowed: $development_final_release_allowed
      },
      release_gate: {
        mode: $release_mode,
        release_allowed: $release_allowed,
        reason: $release_reason
      }
    }'
  exit 0
fi

printf 'OpenPR universal forms delivery status\n'
printf '  stage: %s\n' "$stage"
printf '  automated checks: %s total, %s failed\n' "$automated_checks" "$failed_automated_checks"
printf '  non-manual unresolved items: %s\n' "$non_manual_unresolved"
printf '  manual signoff pending rows: %s\n' "$manual_pending"
printf '  manual signoff accepted rows: %s of %s\n' "$manual_accepted" "$manual_total"
printf '  overall completed items: %s of %s\n' "$overall_items_completed" "$overall_items_total"
printf '  overall remaining items: %s\n' "$overall_items_remaining"
printf '  overall completion percent: %s%%\n' "$overall_completion_percent"
printf '  engineering progress: %s of %s, remaining %s, %s%%, status %s\n' \
  "$engineering_completed" "$automated_checks" "$engineering_items_remaining" "$engineering_completion_percent" \
  "$(if [[ "$failed_automated_checks" -eq 0 && "$non_manual_unresolved" -eq 0 ]]; then printf '已测试'; else printf '开发中'; fi)"
printf '  manual signoff progress: %s of %s, remaining %s, %s%%, status %s\n' \
  "$manual_accepted" "$manual_total" "$manual_pending" "$manual_signoff_completion_percent" \
  "$(if [[ "$manual_pending" -eq 0 && "$manual_blocked" -eq 0 ]]; then printf '已验收'; elif [[ "$manual_blocked" -gt 0 ]]; then printf '阻塞'; else printf '待处理'; fi)"
printf '  overall handoff progress: %s of %s, remaining %s, %s%%, status %s\n' \
  "$overall_items_completed" "$overall_items_total" "$overall_items_remaining" "$overall_completion_percent" \
  "$(if [[ "$release_allowed" == "true" ]]; then printf '已验收'; elif [[ "$failed_automated_checks" -eq 0 && "$non_manual_unresolved" -eq 0 && "$manual_pending" -gt 0 ]]; then printf '待处理'; else printf '开发中'; fi)"
printf '  release blocker automated checks: %s, count %s\n' \
  "$(if [[ "$failed_automated_checks" -eq 0 ]]; then printf 'clear'; else printf 'blocking'; fi)" \
  "$failed_automated_checks"
printf '  release blocker non-manual unresolved: %s, count %s\n' \
  "$(if [[ "$non_manual_unresolved" -eq 0 ]]; then printf 'clear'; else printf 'blocking'; fi)" \
  "$non_manual_unresolved"
printf '  release blocker manual signoff: %s, count %s\n' \
  "$(if [[ "$manual_pending" -eq 0 && "$manual_blocked" -eq 0 ]]; then printf 'clear'; else printf 'blocking'; fi)" \
  "$manual_pending"
printf '  final acceptance complete: %s\n' "$final_acceptance_complete"
printf '  signoff final allowed: %s\n' "$signoff_final_allowed"
printf '  development final release allowed: %s\n' "$development_final_release_allowed"
printf '  delivery manifest files: %s\n' "$manifest_file_count"
printf '  release gate mode: %s\n' "$release_mode"
printf '  release allowed: %s\n' "$release_allowed"
printf '  release reason: %s\n' "$release_reason"
printf '  manual runbook: %s\n' "$MANUAL_RUNBOOK_PATH"
printf '  signoff dashboard: %s\n' "$SIGNOFF_DASHBOARD_PATH"
printf '  user acceptance packet: %s\n' "$USER_ACCEPTANCE_PACKET_PATH"
printf '  next action review status: enabled, command %s\n' "$status_command"
printf '  next action review next signoff: %s, command %s\n' \
  "$(if [[ "$manual_pending" -gt 0 ]]; then printf 'enabled'; else printf 'disabled'; fi)" \
  "$next_review_command"
printf '  next action record next signoff: %s, command %s\n' \
  "$(if [[ "$manual_pending" -gt 0 && -n "$next_command" ]]; then printf 'enabled'; else printf 'disabled'; fi)" \
  "$next_command"
printf '  next action verify manual signoff: %s, command %s\n' \
  "$(if [[ "$failed_automated_checks" -eq 0 && "$non_manual_unresolved" -eq 0 && "$manual_pending" -eq 0 && "$manual_blocked" -eq 0 ]]; then printf 'enabled'; else printf 'disabled'; fi)" \
  "$signoff_verify_command"
printf '  next action finalize acceptance: %s, command %s\n' \
  "$(if [[ "$signoff_final_allowed" == "true" ]]; then printf 'enabled'; else printf 'disabled'; fi)" \
  "$finalize_command"
if [[ -n "$next_key" && "$next_key" != "null" ]]; then
  printf '  next manual signoff key: %s\n' "$next_key"
  printf '  next manual signoff item: %s\n' "$next_item"
  printf '  next manual signoff status: %s\n' "$next_status"
  printf '  next manual signoff actionable: %s\n' "$next_actionable"
  printf '  next manual signoff automated evidence: %s\n' "$next_automated_evidence"
  printf '  next manual signoff reviewer check: %s\n' "$next_reviewer_check"
  printf '  next manual signoff suggested evidence: %s\n' "$next_suggested_evidence_note"
  printf '\nNext recorder command after reviewer approval:\n'
  printf '%s\n' "$next_command"
fi
