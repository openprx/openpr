#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
TRACKER_PATH="$REPORT_DIR/openpr-universal-form-development-execution-tracker-2026-05-31.md"
IMPLEMENTATION_MAP_PATH="$ROOT_DIR/docs/universal-forms-implementation-map.md"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
COMPLETION_AUDIT_PATH="$REPORT_DIR/openpr-universal-form-completion-audit-2026-05-31.md"
USER_ACCEPTANCE_PACKET_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-packet-2026-05-31.md"
DELIVERY_MANIFEST_PATH="$REPORT_DIR/openpr-universal-form-delivery-manifest-2026-05-31.md"
UI_REVIEW_GALLERY_PATH="$REPORT_DIR/openpr-universal-form-ui-review-gallery-2026-05-31.html"
SIGNOFF_STATUS_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.md"
SIGNOFF_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json"
READINESS_JSON_PATH="$REPORT_DIR/openpr-universal-form-readiness-2026-05-31.json"
OUTPUT_PATH="${OPENPR_READINESS_SUMMARY_REPORT:-$REPORT_DIR/openpr-universal-form-readiness-summary-2026-05-31.md}"

usage() {
  cat <<'EOF'
Usage: scripts/report-universal-forms-readiness-summary.sh [--output PATH]

Generates a short release-readiness summary for the universal forms handoff.
The summary is read-only: it reports every tracker module status, automated
gate state, manual signoff state, and the next required action. It does not
mark user acceptance as passed.

Environment:
  OPENPR_READINESS_SUMMARY_REPORT  Optional output path.
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

table_rows() {
  local start_heading="$1"
  local stop_heading="$2"
  awk -v start="$start_heading" -v stop="$stop_heading" '
    $0 == start { in_section = 1; next }
    $0 == stop && in_section { exit }
    in_section && /^\|/ { print }
  ' "$TRACKER_PATH"
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

manual_rows() {
  awk -F'|' '
    /^## Manual Acceptance Signoff$/ { in_section = 1; next }
    /^## / && in_section { exit }
    in_section && NF >= 5 {
      item = $2
      status = $3
      reviewer = $4
      evidence = $5
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", item)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", status)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", reviewer)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", evidence)
      if (item != "Item" && item !~ /^-+$/ && item != "") {
        print "| " item " | " status " | " reviewer " | " evidence " |"
      }
    }
  ' "$EVIDENCE_PATH"
}

manual_pending_keys() {
  awk -F'|' '
    /^## Manual Acceptance Signoff$/ { in_section = 1; next }
    /^## / && in_section { exit }
    in_section && NF >= 5 {
      item = $2
      status = $3
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", item)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", status)
      if (item != "Item" && item !~ /^-+$/ && item != "" && status == "Pending") {
        print "- " item
      }
    }
  ' "$EVIDENCE_PATH"
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

for path in \
  "$TRACKER_PATH" \
  "$IMPLEMENTATION_MAP_PATH" \
  "$EVIDENCE_PATH" \
  "$RUNBOOK_PATH" \
  "$COMPLETION_AUDIT_PATH" \
  "$USER_ACCEPTANCE_PACKET_PATH" \
  "$SIGNOFF_STATUS_PATH" \
  "$SIGNOFF_STATUS_JSON_PATH" \
  "$UI_REVIEW_GALLERY_PATH"; do
  require_file "$path"
done

mkdir -p "$(dirname "$OUTPUT_PATH")"
output_tmp="$(mktemp "$(dirname "$OUTPUT_PATH")/.readiness-summary.XXXXXX")"
trap 'rm -f "$output_tmp"' EXIT

summary_total="$(sed -n 's/^- Total automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
summary_failed="$(sed -n 's/^- Failed automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
pass_count="$(rg -c '^\*\*Status:\*\* PASS$' "$EVIDENCE_PATH" || true)"
manual_pending_count="$(rg -c '^\| .* \| Pending \|' "$EVIDENCE_PATH" || true)"
non_manual_unresolved="$(non_manual_unresolved_count)"
e2e_status="$(table_value "$TRACKER_PATH" "端到端验收")"
manual_status="$(table_value "$TRACKER_PATH" "用户侧人工验收")"
check_index_rows="$(table_value "$COMPLETION_AUDIT_PATH" "Automated check index rows")"
check_index_failed_rows="$(table_value "$COMPLETION_AUDIT_PATH" "Automated check index failed rows")"

manual_consistency_status="failed"
if "$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh" "$RUNBOOK_PATH" "$EVIDENCE_PATH" >/dev/null; then
  manual_consistency_status="passed"
fi

ui_gallery_status="failed"
if "$ROOT_DIR/scripts/verify-universal-forms-ui-review-gallery.sh" >/dev/null; then
  ui_gallery_status="passed"
fi

ui_gallery_render_status="failed"
if "$ROOT_DIR/scripts/smoke-universal-forms-ui-review-gallery-render.sh" >/dev/null; then
  ui_gallery_render_status="passed"
fi

manifest_status="deferred to delivery manifest verifier"
if [[ ! -f "$DELIVERY_MANIFEST_PATH" ]]; then
  manifest_status="manifest not generated yet"
fi

readiness_stage="Not ready"
next_action="Resolve automated or consistency failures before user-side acceptance."
if [[ "${summary_failed:-missing}" == "0" && "$pass_count" == "${summary_total:-}" && "$check_index_rows" == "${summary_total:-}" && "$check_index_failed_rows" == "0" && "$non_manual_unresolved" == "0" && "$manual_consistency_status" == "passed" && "$ui_gallery_status" == "passed" && "$ui_gallery_render_status" == "passed" ]]; then
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

{
  printf '# OpenPR Universal Forms Readiness Summary\n\n'
  printf '%s\n' "- Generated at: $(date -Is)"
  printf '%s\n' "- Repository: \`$ROOT_DIR\`"
  printf '%s\n' "- Tracker: \`$TRACKER_PATH\`"
  printf '%s\n' "- Implementation map: \`$IMPLEMENTATION_MAP_PATH\`"
  printf '%s\n' "- Evidence: \`$EVIDENCE_PATH\`"
  printf '%s\n' "- Completion audit: \`$COMPLETION_AUDIT_PATH\`"
  printf '%s\n' "- User acceptance packet: \`$USER_ACCEPTANCE_PACKET_PATH\`"
  printf '%s\n' "- Manual signoff status: \`$SIGNOFF_STATUS_PATH\`"
  printf '%s\n' "- Machine-readable signoff status JSON: \`$SIGNOFF_STATUS_JSON_PATH\`"
  printf '%s\n' "- Machine-readable readiness JSON: \`$READINESS_JSON_PATH\`"
  printf '%s\n' "- UI review gallery: \`$UI_REVIEW_GALLERY_PATH\`"
  printf '\n'

  printf '## Overall Readiness\n\n'
  printf '| Area | Current value | Release requirement |\n'
  printf '| --- | --- | --- |\n'
  printf '| Readiness stage | %s | Accepted |\n' "$readiness_stage"
  printf '| Automated checks | %s PASS / %s failed | 0 failed and all checks PASS |\n' "${summary_total:-missing}" "${summary_failed:-missing}"
  printf '| PASS status lines | %s | Must equal automated check count |\n' "$pass_count"
  printf '| Automated check index | %s rows / %s failed | %s rows and 0 failed |\n' "${check_index_rows:-missing}" "${check_index_failed_rows:-missing}" "${summary_total:-missing}"
  printf '| Tracker non-manual unresolved items | %s | 0 |\n' "$non_manual_unresolved"
  printf '| Manual signoff consistency | %s | passed |\n' "$manual_consistency_status"
  printf '| UI review gallery verification | %s | passed |\n' "$ui_gallery_status"
  printf '| UI review gallery browser render | %s | passed |\n' "$ui_gallery_render_status"
  printf '| Manual signoff rows pending | %s | 0 |\n' "$manual_pending_count"
  printf '| End-to-end acceptance | %s | 已验收 |\n' "${e2e_status:-missing}"
  printf '| User-side manual acceptance | %s | 已验收 |\n' "${manual_status:-missing}"
  printf '| Delivery manifest verification | %s | `scripts/verify-universal-forms-delivery-manifest.sh` passes after manifest generation |\n' "$manifest_status"
  printf '\n'
  printf 'Next action: %s\n\n' "$next_action"

  printf '## Implementation Map\n\n'
  printf 'Use `%s` to map each delivery area to implementation paths, public surfaces, primary verification commands, and allowed status markers. This is the developer-facing entrypoint for answering how the universal business-platform modules are implemented, tested, and accepted.\n\n' "$IMPLEMENTATION_MAP_PATH"

  printf '## Module Status\n\n'
  table_rows '当前总体状态：' '当前工程边界：'
  printf '\n'

  printf '## Delivery Checklist\n\n'
  table_rows '最终交付必须全部满足：' '## 最新测试证据'
  printf '\n'

  printf '## Manual Acceptance Signoff\n\n'
  printf '| Item | Status | Reviewer | Evidence |\n'
  printf '| --- | --- | --- | --- |\n'
  manual_rows
  printf '\n'

  printf '## Pending Manual Rows\n\n'
  if [[ "$manual_pending_count" == "0" ]]; then
    printf 'No pending manual rows.\n\n'
  else
    manual_pending_keys
    printf '\n'
  fi

  printf '## Reviewer Entrypoints\n\n'
  printf '```bash\n'
  printf 'scripts/report-universal-forms-signoff-status.sh --reviewer "<name>"\n'
  printf 'scripts/report-universal-forms-signoff-status.sh --output \\\n'
  printf '  /opt/worker/report/openpr/docs/openpr-universal-form-signoff-status-2026-05-31.md\n'
  printf 'scripts/report-universal-forms-readiness-json.sh\n'
  printf 'scripts/verify-universal-forms-ui-review-gallery.sh\n'
  printf 'scripts/smoke-universal-forms-ui-review-gallery-render.sh\n'
  printf 'scripts/record-universal-forms-manual-signoff.sh --list-items\n'
  printf 'scripts/verify-universal-forms-manual-signoff-consistency.sh \\\n'
  printf '  /opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md \\\n'
  printf '  /opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md\n'
  printf 'scripts/verify-universal-forms-acceptance-signoff.sh \\\n'
  printf '  /opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md\n'
  printf 'scripts/finalize-universal-forms-acceptance.sh\n'
  printf 'scripts/audit-universal-forms-delivery-state.sh --strict\n'
  printf 'scripts/audit-universal-forms-delivery-bundle.sh\n'
  printf '```\n'
} >"$output_tmp"

if [[ -f "$OUTPUT_PATH" ]]; then
  chmod --reference="$OUTPUT_PATH" "$output_tmp"
fi
mv -f "$output_tmp" "$OUTPUT_PATH"
output_tmp=""

echo "readiness summary report: $OUTPUT_PATH" >&2

if [[ "$readiness_stage" == "Not ready" ]]; then
  exit 1
fi
