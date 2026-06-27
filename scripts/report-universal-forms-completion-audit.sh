#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
TRACKER_PATH="$REPORT_DIR/openpr-universal-form-development-execution-tracker-2026-05-31.md"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
UI_REVIEW_GALLERY_PATH="$REPORT_DIR/openpr-universal-form-ui-review-gallery-2026-05-31.html"
OUTPUT_PATH="${OPENPR_COMPLETION_AUDIT_REPORT:-$REPORT_DIR/openpr-universal-form-completion-audit-2026-05-31.md}"

usage() {
  cat <<'EOF'
Usage: scripts/report-universal-forms-completion-audit.sh [--output PATH]

Generates a completion audit report from the development tracker and acceptance
evidence. The report separates automated completion from the remaining
user-side manual acceptance signoff.

Environment:
  OPENPR_COMPLETION_AUDIT_REPORT  Optional output path.
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

if [[ ! -f "$TRACKER_PATH" ]]; then
  echo "Tracker not found: $TRACKER_PATH" >&2
  exit 2
fi

if [[ ! -f "$EVIDENCE_PATH" ]]; then
  echo "Evidence report not found: $EVIDENCE_PATH" >&2
  exit 2
fi

if [[ ! -f "$RUNBOOK_PATH" ]]; then
  echo "Runbook not found: $RUNBOOK_PATH" >&2
  exit 2
fi

if [[ ! -f "$UI_REVIEW_GALLERY_PATH" ]]; then
  echo "UI review gallery not found: $UI_REVIEW_GALLERY_PATH" >&2
  exit 2
fi

mkdir -p "$(dirname "$OUTPUT_PATH")"
output_tmp="$(mktemp "$(dirname "$OUTPUT_PATH")/.completion-audit.XXXXXX")"
trap 'rm -f "$output_tmp"' EXIT

table_rows() {
  local start_heading="$1"
  local stop_heading="$2"
  awk -v start="$start_heading" -v stop="$stop_heading" '
    $0 == start { in_section = 1; next }
    $0 == stop && in_section { exit }
    in_section && /^\|/ { print }
  ' "$TRACKER_PATH"
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

manual_pending_count="$(rg -c '^\| .* \| Pending \|' "$EVIDENCE_PATH" || true)"
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
manual_status="$(status_for "用户侧人工验收")"
e2e_status="$(status_for "端到端验收")"

manual_consistency_status="missing"
if "$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh" "$RUNBOOK_PATH" "$EVIDENCE_PATH" >/dev/null; then
  manual_consistency_status="passed"
else
  manual_consistency_status="failed"
fi

ui_artifact_status="missing"
if "$ROOT_DIR/scripts/verify-universal-forms-ui-artifacts.sh" >/dev/null; then
  ui_artifact_status="passed"
else
  ui_artifact_status="failed"
fi

ui_gallery_status="missing"
if "$ROOT_DIR/scripts/verify-universal-forms-ui-review-gallery.sh" >/dev/null; then
  ui_gallery_status="passed"
else
  ui_gallery_status="failed"
fi

ui_gallery_render_status="missing"
if "$ROOT_DIR/scripts/smoke-universal-forms-ui-review-gallery-render.sh" >/dev/null; then
  ui_gallery_render_status="passed"
else
  ui_gallery_render_status="failed"
fi

finalizer_atomic_status="missing"
if rg -q --fixed-strings -- '--tracker "$tmp_file"' "$ROOT_DIR/scripts/finalize-universal-forms-acceptance.sh"; then
  finalizer_atomic_status="candidate-strict-before-replace"
else
  finalizer_atomic_status="not-proven"
fi

{
  printf '# OpenPR Universal Forms Completion Audit\n\n'
  printf '%s\n' "- Generated at: $(date -Is)"
  printf '%s\n' "- Repository: \`$ROOT_DIR\`"
  printf '%s\n' "- Tracker: \`$TRACKER_PATH\`"
  printf '%s\n' "- Evidence: \`$EVIDENCE_PATH\`"
  printf '\n'

  printf '## Executive Status\n\n'
  printf '| Area | Status | Evidence |\n'
  printf '| --- | --- | --- |\n'
  printf '| Automated checks | %s PASS / %s failed | `%s` |\n' "${summary_total:-missing}" "${summary_failed:-missing}" "$EVIDENCE_PATH"
  printf '| Automated check index | %s rows / %s failed | `## Automated Check Index` in `%s` |\n' "$check_index_rows" "$check_index_failed_rows" "$EVIDENCE_PATH"
  printf '| Tracker non-manual unresolved items | %s | `已测试` or `已验收` required for all non-manual rows |\n' "$non_manual_unresolved"
  printf '| Manual signoff consistency | %s | runbook conclusion table and evidence signoff table must match |\n' "$manual_consistency_status"
  printf '| UI artifact verification | %s | screenshots, dimensions, and browser smoke logs must verify |\n' "$ui_artifact_status"
  printf '| UI review gallery verification | %s | HTML gallery must reference all reviewer screenshots and logs |\n' "$ui_gallery_status"
  printf '| UI review gallery browser render | %s | Chromium must render the gallery and load all eight images |\n' "$ui_gallery_render_status"
  printf '| Finalizer safety | %s | candidate tracker must pass strict audit before replacement |\n' "$finalizer_atomic_status"
  printf '| End-to-end acceptance | %s | Pre-signoff state remains valid only while manual signoff is pending |\n' "${e2e_status:-missing}"
  printf '| User-side manual acceptance | %s | Manual signoff rows pending: %s |\n' "${manual_status:-missing}" "$manual_pending_count"
  printf '\n'

  if [[ "$summary_failed" == "0" && "$pass_count" == "$summary_total" && "$check_index_rows" == "$summary_total" && "$check_index_failed_rows" == "0" && "$non_manual_unresolved" == "0" && "$manual_pending_count" != "0" ]]; then
    printf 'Conclusion: automated delivery is complete and internally consistent. Final acceptance is not complete because user-side manual signoff is still pending.\n\n'
  elif [[ "$summary_failed" == "0" && "$pass_count" == "$summary_total" && "$check_index_rows" == "$summary_total" && "$check_index_failed_rows" == "0" && "$non_manual_unresolved" == "0" && "$manual_pending_count" == "0" && "$e2e_status" == "已验收" && "$manual_status" == "已验收" ]]; then
    printf 'Conclusion: automated delivery, manual signoff, and tracker finalization are complete. The derived handoff is synchronized with the finalized tracker.\n\n'
  elif [[ "$summary_failed" == "0" && "$pass_count" == "$summary_total" && "$check_index_rows" == "$summary_total" && "$check_index_failed_rows" == "0" && "$non_manual_unresolved" == "0" && "$manual_pending_count" == "0" ]]; then
    printf 'Conclusion: automated delivery and manual signoff are complete. Run `scripts/finalize-universal-forms-acceptance.sh`, then strict delivery-state and delivery-bundle audits.\n\n'
  else
    printf 'Conclusion: completion is not proven. Resolve the failing counts above before final acceptance.\n\n'
  fi

  printf '## Current Overall Tracker Status\n\n'
  table_rows '当前总体状态：' '当前工程边界：'
  printf '\n'

  printf '## Delivery Checklist\n\n'
  table_rows '最终交付必须全部满足：' '## 最新测试证据'
  printf '\n'

  printf '## Automated Evidence Summary\n\n'
  printf '| Metric | Value |\n'
  printf '| --- | --- |\n'
  printf '| Total automated checks | %s |\n' "${summary_total:-missing}"
  printf '| PASS status lines | %s |\n' "$pass_count"
  printf '| Failed automated checks | %s |\n' "${summary_failed:-missing}"
  printf '| Automated check index rows | %s |\n' "$check_index_rows"
  printf '| Automated check index failed rows | %s |\n' "$check_index_failed_rows"
  printf '| Non-manual unresolved tracker rows | %s |\n' "$non_manual_unresolved"
  printf '| Manual signoff consistency | %s |\n' "$manual_consistency_status"
  printf '| UI artifact verification | %s |\n' "$ui_artifact_status"
  printf '| UI review gallery verification | %s |\n' "$ui_gallery_status"
  printf '| UI review gallery browser render | %s |\n' "$ui_gallery_render_status"
  printf '| Finalizer safety | %s |\n' "$finalizer_atomic_status"
  printf '\n'

  printf '## Manual Acceptance Signoff\n\n'
  printf '| Item | Status | Reviewer | Evidence |\n'
  printf '| --- | --- | --- | --- |\n'
  manual_rows
  printf '\n'

  printf '## Finalization Gate\n\n'
  printf 'Final acceptance must run these commands after every manual signoff row is marked passed or accepted:\n\n'
  printf '```bash\n'
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

echo "completion audit report: $OUTPUT_PATH" >&2

if [[ "$summary_failed" != "0" || "$pass_count" != "$summary_total" || "$check_index_rows" != "$summary_total" || "$check_index_failed_rows" != "0" || "$non_manual_unresolved" != "0" || "$manual_consistency_status" != "passed" || "$ui_artifact_status" != "passed" || "$ui_gallery_status" != "passed" || "$ui_gallery_render_status" != "passed" || "$finalizer_atomic_status" != "candidate-strict-before-replace" ]]; then
  exit 1
fi
