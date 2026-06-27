#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
TRACKER_PATH="$REPORT_DIR/openpr-universal-form-development-execution-tracker-2026-05-31.md"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
COMPLETION_AUDIT_PATH="$REPORT_DIR/openpr-universal-form-completion-audit-2026-05-31.md"
USER_ACCEPTANCE_PACKET_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-packet-2026-05-31.md"
READINESS_SUMMARY_PATH="$REPORT_DIR/openpr-universal-form-readiness-summary-2026-05-31.md"
SIGNOFF_STATUS_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.md"
SIGNOFF_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json"
DELIVERY_MANIFEST_PATH="$REPORT_DIR/openpr-universal-form-delivery-manifest-2026-05-31.md"
UI_REVIEW_GALLERY_PATH="$REPORT_DIR/openpr-universal-form-ui-review-gallery-2026-05-31.html"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-readiness.schema.json"
OUTPUT_PATH="${OPENPR_READINESS_JSON_REPORT:-$REPORT_DIR/openpr-universal-form-readiness-2026-05-31.json}"

usage() {
  cat <<'EOF'
Usage: scripts/report-universal-forms-readiness-json.sh [--output PATH]

Generates a machine-readable JSON readiness report for CI, release automation,
MCP tools, webhook consumers, and deployment scripts. The report is read-only:
it never marks manual acceptance as passed.

Environment:
  OPENPR_READINESS_JSON_REPORT  Optional output path.
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

require_file() {
  local path="$1"
  if [[ ! -f "$path" ]]; then
    echo "Missing required file: $path" >&2
    exit 1
  fi
}

require_command() {
  local name="$1"
  if ! command -v "$name" >/dev/null 2>&1; then
    echo "Missing required command: $name" >&2
    exit 2
  fi
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

tracker_status() {
  table_value "$TRACKER_PATH" "$1"
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

number_or_zero() {
  local value="${1:-}"
  if [[ "$value" =~ ^[0-9]+$ ]]; then
    printf '%s' "$value"
  else
    printf '0'
  fi
}

for path in \
  "$TRACKER_PATH" \
  "$EVIDENCE_PATH" \
  "$RUNBOOK_PATH" \
  "$COMPLETION_AUDIT_PATH" \
  "$USER_ACCEPTANCE_PACKET_PATH" \
  "$READINESS_SUMMARY_PATH" \
  "$SIGNOFF_STATUS_PATH" \
  "$SIGNOFF_STATUS_JSON_PATH" \
  "$SCHEMA_PATH" \
  "$UI_REVIEW_GALLERY_PATH"; do
  require_file "$path"
done
require_command jq

mkdir -p "$(dirname "$OUTPUT_PATH")"
output_tmp="$(mktemp "$(dirname "$OUTPUT_PATH")/.readiness-json.XXXXXX")"
trap 'rm -f "$output_tmp"' EXIT

summary_total="$(number_or_zero "$(sed -n 's/^- Total automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)")"
summary_failed="$(number_or_zero "$(sed -n 's/^- Failed automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)")"
pass_count="$(number_or_zero "$(rg -c '^\*\*Status:\*\* PASS$' "$EVIDENCE_PATH" || true)")"
check_index_rows="$(number_or_zero "$(table_value "$COMPLETION_AUDIT_PATH" "Automated check index rows")")"
check_index_failed_rows="$(number_or_zero "$(table_value "$COMPLETION_AUDIT_PATH" "Automated check index failed rows")")"
manual_pending_count="$(number_or_zero "$(rg -c '^\| .* \| Pending \|' "$EVIDENCE_PATH" || true)")"
non_manual_unresolved="$(number_or_zero "$(non_manual_unresolved_count)")"
e2e_status="$(tracker_status "端到端验收")"
manual_status="$(tracker_status "用户侧人工验收")"

manual_consistency_status="failed"
if "$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh" "$RUNBOOK_PATH" "$EVIDENCE_PATH" >/dev/null; then
  manual_consistency_status="passed"
fi

ui_gallery_status="failed"
if "$ROOT_DIR/scripts/verify-universal-forms-ui-review-gallery.sh" >/dev/null 2>&1; then
  ui_gallery_status="passed"
fi

ui_gallery_render_status="failed"
if "$ROOT_DIR/scripts/smoke-universal-forms-ui-review-gallery-render.sh" >/dev/null 2>&1; then
  ui_gallery_render_status="passed"
fi

manifest_exists=false
if [[ -f "$DELIVERY_MANIFEST_PATH" ]]; then
  manifest_exists=true
fi

readiness_stage="Not ready"
next_action="Resolve automated or consistency failures before user-side acceptance."
if [[ "$summary_failed" == "0" && "$pass_count" == "$summary_total" && "$check_index_rows" == "$summary_total" && "$check_index_failed_rows" == "0" && "$non_manual_unresolved" == "0" && "$manual_consistency_status" == "passed" && "$ui_gallery_status" == "passed" && "$ui_gallery_render_status" == "passed" ]]; then
  if [[ "$manual_pending_count" == "0" && "$e2e_status" == "已验收" && "$manual_status" == "已验收" ]]; then
    readiness_stage="Accepted"
    next_action="The tracker is finalized. Keep running strict delivery-state and delivery-bundle audits before release cuts."
  elif [[ "$manual_pending_count" == "0" ]]; then
    readiness_stage="Signed, not finalized"
    next_action="Run final signoff verifier, finalizer, strict delivery-state audit, and delivery-bundle audit."
  else
    readiness_stage="Ready for user-side manual signoff"
    next_action="Review the runbook, inspect the UI review gallery and automated check index, then record the seven manual signoff rows."
  fi
fi

manual_rows_json="$(
  awk -F'|' '
    /^## Manual Rows$/ { in_section = 1; next }
    /^## / && in_section { exit }
    in_section && NF >= 6 {
      key = $2
      item = $3
      status = $4
      reviewer = $5
      evidence = $6
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", key)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", item)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", status)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", reviewer)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", evidence)
      gsub(/^`|`$/, "", key)
      if (key != "Key" && key !~ /^-+$/ && key != "") {
        print key "\t" item "\t" status "\t" reviewer "\t" evidence
      }
    }
  ' "$SIGNOFF_STATUS_PATH" | jq -R -s '
    split("\n")
    | map(select(length > 0) | split("\t") | {
        key: .[0],
        item: .[1],
        status: .[2],
        reviewer: .[3],
        evidence: .[4]
      })
  '
)"

next_key="$(sed -n 's/^Next row: `\([^`]*\)` -.*/\1/p' "$SIGNOFF_STATUS_PATH" | head -n 1)"
next_item="$(sed -n 's/^Next row: `[^`]*` - //p' "$SIGNOFF_STATUS_PATH" | head -n 1)"
next_evidence_note="$(
  awk '
    /^Suggested evidence note:$/ { want_fence = 1; next }
    want_fence && /^```/ { in_block = 1; want_fence = 0; next }
    in_block && /^```/ { exit }
    in_block { print }
  ' "$SIGNOFF_STATUS_PATH"
)"
next_recorder_command="$(
  awk '
    /^Recorder command after reviewer approval:$/ { want_fence = 1; next }
    want_fence && /^```/ { in_block = 1; want_fence = 0; next }
    in_block && /^```/ { exit }
    in_block { print }
  ' "$SIGNOFF_STATUS_PATH"
)"

jq -n \
  --arg generated_at "$(date -Is)" \
  --arg root_dir "$ROOT_DIR" \
  --arg schema_path "$SCHEMA_PATH" \
  --arg tracker "$TRACKER_PATH" \
  --arg evidence "$EVIDENCE_PATH" \
  --arg completion_audit "$COMPLETION_AUDIT_PATH" \
  --arg user_acceptance_packet "$USER_ACCEPTANCE_PACKET_PATH" \
  --arg readiness_summary "$READINESS_SUMMARY_PATH" \
  --arg signoff_status "$SIGNOFF_STATUS_PATH" \
  --arg signoff_status_json "$SIGNOFF_STATUS_JSON_PATH" \
  --arg delivery_manifest "$DELIVERY_MANIFEST_PATH" \
  --arg ui_review_gallery "$UI_REVIEW_GALLERY_PATH" \
  --arg stage "$readiness_stage" \
  --arg next_action "$next_action" \
  --arg e2e_status "${e2e_status:-missing}" \
  --arg manual_status "${manual_status:-missing}" \
  --arg manual_consistency_status "$manual_consistency_status" \
  --arg ui_gallery_status "$ui_gallery_status" \
  --arg ui_gallery_render_status "$ui_gallery_render_status" \
  --arg next_key "$next_key" \
  --arg next_item "$next_item" \
  --arg next_evidence_note "$next_evidence_note" \
  --arg next_recorder_command "$next_recorder_command" \
  --argjson total_checks "$summary_total" \
  --argjson failed_checks "$summary_failed" \
  --argjson pass_count "$pass_count" \
  --argjson check_index_rows "$check_index_rows" \
  --argjson check_index_failed_rows "$check_index_failed_rows" \
  --argjson manual_pending_count "$manual_pending_count" \
  --argjson non_manual_unresolved "$non_manual_unresolved" \
  --argjson manifest_exists "$manifest_exists" \
  --argjson manual_rows "$manual_rows_json" \
  '{
    schema_version: "openpr.universal_forms.readiness.v1",
    schema_path: $schema_path,
    generated_at: $generated_at,
    repository: $root_dir,
    reports: {
      tracker: $tracker,
      evidence: $evidence,
      completion_audit: $completion_audit,
      user_acceptance_packet: $user_acceptance_packet,
      readiness_summary: $readiness_summary,
      signoff_status: $signoff_status,
      signoff_status_json: $signoff_status_json,
      delivery_manifest: $delivery_manifest,
      ui_review_gallery: $ui_review_gallery
    },
    stage: $stage,
    final_acceptance_complete: ($stage == "Accepted"),
    next_action: $next_action,
    gates: {
      automated_checks: $total_checks,
      pass_status_lines: $pass_count,
      failed_automated_checks: $failed_checks,
      automated_check_index_rows: $check_index_rows,
      automated_check_index_failed_rows: $check_index_failed_rows,
      tracker_non_manual_unresolved_items: $non_manual_unresolved,
      manual_signoff_consistency: $manual_consistency_status,
      ui_review_gallery_verification: $ui_gallery_status,
      ui_review_gallery_browser_render: $ui_gallery_render_status,
      delivery_manifest_exists: $manifest_exists
    },
    tracker_status: {
      end_to_end_acceptance: $e2e_status,
      user_side_manual_acceptance: $manual_status
    },
    manual_signoff: {
      pending_rows: $manual_pending_count,
      rows: $manual_rows,
      next_row: {
        key: $next_key,
        item: $next_item,
        suggested_evidence_note: $next_evidence_note,
        recorder_command: $next_recorder_command
      }
    },
    release_requirement: {
      stage: "Accepted",
      failed_automated_checks: 0,
      tracker_non_manual_unresolved_items: 0,
      manual_signoff_pending_rows: 0,
      end_to_end_acceptance: "已验收",
      user_side_manual_acceptance: "已验收"
    }
  }' >"$output_tmp"

if [[ -f "$OUTPUT_PATH" ]]; then
  chmod --reference="$OUTPUT_PATH" "$output_tmp"
fi
mv -f "$output_tmp" "$OUTPUT_PATH"
output_tmp=""

echo "readiness JSON report: $OUTPUT_PATH" >&2
