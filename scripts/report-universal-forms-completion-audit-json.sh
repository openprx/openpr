#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
TRACKER_PATH="$REPORT_DIR/openpr-universal-form-development-execution-tracker-2026-05-31.md"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
COMPLETION_AUDIT_PATH="$REPORT_DIR/openpr-universal-form-completion-audit-2026-05-31.md"
READINESS_JSON_PATH="$REPORT_DIR/openpr-universal-form-readiness-2026-05-31.json"
DEVELOPMENT_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-development-status-2026-05-31.json"
SIGNOFF_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-completion-audit.schema.json"
OUTPUT_PATH="${OPENPR_COMPLETION_AUDIT_JSON_REPORT:-$REPORT_DIR/openpr-universal-form-completion-audit-2026-05-31.json}"

usage() {
  cat <<'EOF'
Usage: scripts/report-universal-forms-completion-audit-json.sh [--output PATH] [--completion-audit PATH]

Generates the machine-readable completion audit JSON for CI, release
automation, MCP tools, and webhook consumers. The JSON mirrors the same
handoff sources as the Markdown completion audit and never records acceptance.

Options:
  --output PATH             Write JSON to PATH.
  --completion-audit PATH   Markdown completion audit path to reference.
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
    --completion-audit)
      COMPLETION_AUDIT_PATH="${2:-}"
      if [[ -z "$COMPLETION_AUDIT_PATH" ]]; then
        echo "--completion-audit requires a path" >&2
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

require_command() {
  local name="$1"
  if ! command -v "$name" >/dev/null 2>&1; then
    echo "Missing required command: $name" >&2
    exit 2
  fi
}

require_file() {
  local path="$1"
  if [[ ! -f "$path" ]]; then
    echo "Missing required file: $path" >&2
    exit 1
  fi
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

manual_status_count() {
  local selector="$1"
  jq -r --arg selector "$selector" '
    if $selector == "accepted" then
      [.manual_signoff.rows[] | select(.status == "accepted")] | length
    elif $selector == "pending" then
      .manual_signoff.pending_rows
    elif $selector == "blocked" then
      .manual_signoff.blocked_rows
    else
      .manual_signoff.total_rows
    end
  ' "$SIGNOFF_STATUS_JSON_PATH"
}

gate_status() {
  local command="$1"
  local passed="passed"
  if ! eval "$command" >/dev/null 2>&1; then
    passed="failed"
  fi
  printf '%s' "$passed"
}

require_command jq
for path in \
  "$TRACKER_PATH" \
  "$EVIDENCE_PATH" \
  "$RUNBOOK_PATH" \
  "$COMPLETION_AUDIT_PATH" \
  "$READINESS_JSON_PATH" \
  "$DEVELOPMENT_STATUS_JSON_PATH" \
  "$SIGNOFF_STATUS_JSON_PATH" \
  "$SCHEMA_PATH"; do
require_file "$path"
done

mkdir -p "$(dirname "$OUTPUT_PATH")"
output_tmp="$(mktemp "$(dirname "$OUTPUT_PATH")/.completion-audit-json.XXXXXX")"
trap 'rm -f "$output_tmp"' EXIT

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
manual_consistency_status="$(gate_status "\"$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh\" \"$RUNBOOK_PATH\" \"$EVIDENCE_PATH\"")"
ui_artifact_status="$(gate_status "\"$ROOT_DIR/scripts/verify-universal-forms-ui-artifacts.sh\"")"
ui_gallery_status="$(gate_status "\"$ROOT_DIR/scripts/verify-universal-forms-ui-review-gallery.sh\"")"
ui_gallery_render_status="$(gate_status "\"$ROOT_DIR/scripts/smoke-universal-forms-ui-review-gallery-render.sh\"")"
finalizer_safety_status="not-proven"
if rg -q --fixed-strings -- '--tracker "$tmp_file"' "$ROOT_DIR/scripts/finalize-universal-forms-acceptance.sh"; then
  finalizer_safety_status="candidate-strict-before-replace"
fi

e2e_status="$(status_for "端到端验收")"
manual_tracker_status="$(status_for "用户侧人工验收")"
manual_total_rows="$(manual_status_count total)"
manual_accepted_rows="$(manual_status_count accepted)"
manual_pending_rows="$(manual_status_count pending)"
manual_blocked_rows="$(manual_status_count blocked)"
manual_final_allowed="$(jq -r '.manual_signoff.final_signoff_allowed' "$SIGNOFF_STATUS_JSON_PATH")"
development_release_allowed="$(jq -r '.status_summary.final_release_allowed' "$DEVELOPMENT_STATUS_JSON_PATH")"
readiness_final_acceptance="$(jq -r '.final_acceptance_complete' "$READINESS_JSON_PATH")"

automated_delivery_complete=false
if [[ "$summary_failed" == "0" && "$pass_count" == "$summary_total" && "$check_index_rows" == "$summary_total" && "$check_index_failed_rows" == "0" && "$non_manual_unresolved" == "0" && "$manual_consistency_status" == "passed" && "$ui_artifact_status" == "passed" && "$ui_gallery_status" == "passed" && "$ui_gallery_render_status" == "passed" && "$finalizer_safety_status" == "candidate-strict-before-replace" ]]; then
  automated_delivery_complete=true
fi

conclusion="not_proven"
next_action="Resolve failed automated or non-manual gates before user-side acceptance."
if [[ "$automated_delivery_complete" == "true" && "$manual_pending_rows" != "0" ]]; then
  conclusion="pre_signoff_ready"
  next_action="Run user-side manual acceptance, record all seven signoff rows, then finalize and run strict release gates."
elif [[ "$automated_delivery_complete" == "true" && "$manual_pending_rows" == "0" && "$e2e_status" == "已验收" && "$manual_tracker_status" == "已验收" && "$readiness_final_acceptance" == "true" ]]; then
  conclusion="finalized"
  next_action="Strict release gate may be used for production release."
elif [[ "$automated_delivery_complete" == "true" && "$manual_pending_rows" == "0" ]]; then
  conclusion="ready_for_finalizer"
  next_action="Run scripts/finalize-universal-forms-acceptance.sh, then strict delivery-state and delivery-bundle audits."
fi

jq -n \
  --arg schema_version "openpr.universal_forms.completion_audit.v1" \
  --arg schema_path "$SCHEMA_PATH" \
  --arg generated_at "$(date -Is)" \
  --arg tracker_path "$TRACKER_PATH" \
  --arg evidence_path "$EVIDENCE_PATH" \
  --arg runbook_path "$RUNBOOK_PATH" \
  --arg completion_audit_path "$COMPLETION_AUDIT_PATH" \
  --arg readiness_json_path "$READINESS_JSON_PATH" \
  --arg development_status_json_path "$DEVELOPMENT_STATUS_JSON_PATH" \
  --arg signoff_status_json_path "$SIGNOFF_STATUS_JSON_PATH" \
  --arg e2e_status "$e2e_status" \
  --arg manual_tracker_status "$manual_tracker_status" \
  --arg manual_consistency_status "$manual_consistency_status" \
  --arg ui_artifact_status "$ui_artifact_status" \
  --arg ui_gallery_status "$ui_gallery_status" \
  --arg ui_gallery_render_status "$ui_gallery_render_status" \
  --arg finalizer_safety_status "$finalizer_safety_status" \
  --arg conclusion "$conclusion" \
  --arg next_action "$next_action" \
  --argjson automated_checks "${summary_total:-0}" \
  --argjson pass_status_lines "$pass_count" \
  --argjson failed_automated_checks "${summary_failed:-0}" \
  --argjson automated_check_index_rows "$check_index_rows" \
  --argjson automated_check_index_failed_rows "$check_index_failed_rows" \
  --argjson non_manual_unresolved_items "$non_manual_unresolved" \
  --argjson automated_delivery_complete "$automated_delivery_complete" \
  --argjson manual_total_rows "$manual_total_rows" \
  --argjson manual_accepted_rows "$manual_accepted_rows" \
  --argjson manual_pending_rows "$manual_pending_rows" \
  --argjson manual_blocked_rows "$manual_blocked_rows" \
  --argjson manual_final_allowed "$manual_final_allowed" \
  --argjson readiness_final_acceptance "$readiness_final_acceptance" \
  --argjson development_release_allowed "$development_release_allowed" \
  '{
    schema_version: $schema_version,
    schema_path: $schema_path,
    generated_at: $generated_at,
    reports: {
      tracker: $tracker_path,
      evidence: $evidence_path,
      runbook: $runbook_path,
      completion_audit: $completion_audit_path,
      readiness_json: $readiness_json_path,
      development_status_json: $development_status_json_path,
      signoff_status_json: $signoff_status_json_path
    },
    gates: {
      automated_checks: $automated_checks,
      pass_status_lines: $pass_status_lines,
      failed_automated_checks: $failed_automated_checks,
      automated_check_index_rows: $automated_check_index_rows,
      automated_check_index_failed_rows: $automated_check_index_failed_rows,
      non_manual_unresolved_items: $non_manual_unresolved_items,
      manual_consistency_status: $manual_consistency_status,
      ui_artifact_status: $ui_artifact_status,
      ui_review_gallery_status: $ui_gallery_status,
      ui_review_gallery_render_status: $ui_gallery_render_status,
      finalizer_safety_status: $finalizer_safety_status,
      automated_delivery_complete: $automated_delivery_complete
    },
    tracker_status: {
      end_to_end_acceptance: $e2e_status,
      user_side_manual_acceptance: $manual_tracker_status
    },
    manual_signoff: {
      total_rows: $manual_total_rows,
      accepted_rows: $manual_accepted_rows,
      pending_rows: $manual_pending_rows,
      blocked_rows: $manual_blocked_rows,
      final_signoff_allowed: $manual_final_allowed
    },
    finalization: {
      final_acceptance_complete: $readiness_final_acceptance,
      development_release_allowed: $development_release_allowed,
      strict_release_required: true,
      strict_delivery_state_required: true,
      delivery_bundle_audit_required: true
    },
    conclusion: $conclusion,
    next_action: $next_action
  }' >"$output_tmp"

if [[ -f "$OUTPUT_PATH" ]]; then
  chmod --reference="$OUTPUT_PATH" "$output_tmp"
fi
mv -f "$output_tmp" "$OUTPUT_PATH"
output_tmp=""

echo "completion audit JSON: $OUTPUT_PATH" >&2
