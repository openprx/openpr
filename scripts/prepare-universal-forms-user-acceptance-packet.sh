#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
TRACKER_PATH="$REPORT_DIR/openpr-universal-form-development-execution-tracker-2026-05-31.md"
IMPLEMENTATION_MAP_PATH="$ROOT_DIR/docs/universal-forms-implementation-map.md"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
COMPLETION_AUDIT_PATH="$REPORT_DIR/openpr-universal-form-completion-audit-2026-05-31.md"
UI_ARTIFACT_MANIFEST_PATH="$REPORT_DIR/openpr-universal-form-ui-artifacts-2026-05-31.md"
UI_REVIEW_GALLERY_PATH="$REPORT_DIR/openpr-universal-form-ui-review-gallery-2026-05-31.html"
MANUAL_EVIDENCE_MAP_PATH="$REPORT_DIR/openpr-universal-form-manual-evidence-map-2026-05-31.md"
SIGNOFF_STATUS_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.md"
SIGNOFF_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json"
NEXT_SIGNOFF_REVIEW_PATH="$REPORT_DIR/openpr-universal-form-next-signoff-review-2026-05-31.md"
DELIVERY_MANIFEST_PATH="$REPORT_DIR/openpr-universal-form-delivery-manifest-2026-05-31.md"
OUTPUT_PATH="${OPENPR_USER_ACCEPTANCE_PACKET:-$REPORT_DIR/openpr-universal-form-user-acceptance-packet-2026-05-31.md}"

usage() {
  cat <<'EOF'
Usage: scripts/prepare-universal-forms-user-acceptance-packet.sh [--output PATH]

Generates a user acceptance packet for reviewers. The packet does not mark
acceptance as passed; it summarizes the automated evidence, runbook, pending
manual signoff rows, and finalization commands needed after signoff.

Environment:
  OPENPR_USER_ACCEPTANCE_PACKET  Optional output path.
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

require_file "$TRACKER_PATH"
require_file "$IMPLEMENTATION_MAP_PATH"
require_file "$EVIDENCE_PATH"
require_file "$RUNBOOK_PATH"
require_file "$COMPLETION_AUDIT_PATH"
require_file "$UI_ARTIFACT_MANIFEST_PATH"
require_file "$UI_REVIEW_GALLERY_PATH"
require_file "$MANUAL_EVIDENCE_MAP_PATH"
require_file "$SIGNOFF_STATUS_PATH"
require_file "$SIGNOFF_STATUS_JSON_PATH"
require_file "$NEXT_SIGNOFF_REVIEW_PATH"

mkdir -p "$(dirname "$OUTPUT_PATH")"
output_tmp="$(mktemp "$(dirname "$OUTPUT_PATH")/.user-acceptance-packet.XXXXXX")"
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
manual_pending_count="$(rg -c '^\| .* \| Pending \|' "$EVIDENCE_PATH" || true)"
manual_consistency_status="failed"
if "$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh" "$RUNBOOK_PATH" "$EVIDENCE_PATH" >/dev/null; then
  manual_consistency_status="passed"
fi
ui_artifact_status="failed"
if "$ROOT_DIR/scripts/verify-universal-forms-ui-artifacts.sh" >/dev/null; then
  ui_artifact_status="passed"
fi
ui_gallery_status="failed"
if "$ROOT_DIR/scripts/verify-universal-forms-ui-review-gallery.sh" >/dev/null; then
  ui_gallery_status="passed"
fi
ui_gallery_render_status="failed"
if "$ROOT_DIR/scripts/smoke-universal-forms-ui-review-gallery-render.sh" >/dev/null; then
  ui_gallery_render_status="passed"
fi
non_manual_unresolved="$(non_manual_unresolved_count)"
e2e_status="$(status_for "端到端验收")"
manual_status="$(status_for "用户侧人工验收")"

{
  printf '# OpenPR Universal Forms User Acceptance Packet\n\n'
  printf '%s\n' "- Generated at: $(date -Is)"
  printf '%s\n' "- Repository: \`$ROOT_DIR\`"
  printf '%s\n' "- Tracker: \`$TRACKER_PATH\`"
  printf '%s\n' "- Implementation map: \`$IMPLEMENTATION_MAP_PATH\`"
  printf '%s\n' "- Automated evidence: \`$EVIDENCE_PATH\`"
  printf '%s\n' "- Completion audit: \`$COMPLETION_AUDIT_PATH\`"
  printf '%s\n' "- UI artifacts: \`$UI_ARTIFACT_MANIFEST_PATH\`"
  printf '%s\n' "- UI review gallery: \`$UI_REVIEW_GALLERY_PATH\`"
  printf '%s\n' "- Manual evidence map: \`$MANUAL_EVIDENCE_MAP_PATH\`"
  printf '%s\n' "- Manual signoff status: \`$SIGNOFF_STATUS_PATH\`"
  printf '%s\n' "- Manual signoff status JSON: \`$SIGNOFF_STATUS_JSON_PATH\`"
  printf '%s\n' "- Next signoff review: \`$NEXT_SIGNOFF_REVIEW_PATH\`"
  printf '%s\n' "- Delivery manifest: \`$DELIVERY_MANIFEST_PATH\`"
  printf '%s\n' "- Manual runbook: \`$RUNBOOK_PATH\`"
  printf '\n'

  printf '## Current Gate\n\n'
  printf '| Gate | Current value | Required for final acceptance |\n'
  printf '| --- | --- | --- |\n'
  printf '| Automated checks | %s PASS / %s failed | 0 failed and all checks PASS |\n' "${summary_total:-missing}" "${summary_failed:-missing}"
  printf '| PASS status lines | %s | Must equal automated check count |\n' "$pass_count"
  printf '| Automated check index | %s rows / %s failed | %s rows and 0 failed |\n' "$check_index_rows" "$check_index_failed_rows" "${summary_total:-missing}"
  printf '| Tracker non-manual unresolved items | %s | 0 |\n' "${non_manual_unresolved:-missing}"
  printf '| Manual signoff consistency | %s | passed |\n' "$manual_consistency_status"
  printf '| UI artifact verification | %s | passed |\n' "$ui_artifact_status"
  printf '| UI review gallery verification | %s | passed |\n' "$ui_gallery_status"
  printf '| UI review gallery browser render | %s | passed |\n' "$ui_gallery_render_status"
  printf '| Manual evidence map | generated | reviewer evidence is mapped to every manual row |\n'
  printf '| Implementation map | `%s` | reviewer can trace modules to source paths and verification commands |\n' "$IMPLEMENTATION_MAP_PATH"
  printf '| Report docs index | `/opt/worker/report/openpr/docs/README.md` | current bundle wins over historical reports |\n'
  printf '| End-to-end acceptance | %s | 已验收 after manual signoff |\n' "${e2e_status:-missing}"
  printf '| User-side manual acceptance | %s | 已验收 after manual signoff |\n' "${manual_status:-missing}"
  printf '| Manual signoff rows pending | %s | 0 |\n' "$manual_pending_count"
  printf '\n'

  printf '## Reviewer Scope\n\n'
  printf 'Use the runbook to validate the actual user experience, not just command output. The acceptance target is:\n\n'
  printf '```text\n'
  printf 'restaurant_ordering_default project\n'
  printf '  -> universal forms grid/detail workflow\n'
  printf '  -> restaurant_calc WASM formula plugin\n'
  printf '  -> order, order_line, table change, print_job, business_report\n'
  printf '  -> API/MCP/Webhook/Connector consistency\n'
  printf '```\n\n'

  printf '## Manual Signoff Rows\n\n'
  printf '| Item | Status | Reviewer | Evidence |\n'
  printf '| --- | --- | --- | --- |\n'
	  manual_rows
	  printf '\n'

	  printf '## Manual Signoff Key Map\n\n'
	  printf 'Use these keys with `scripts/record-universal-forms-manual-signoff.sh --item <key>` after the matching reviewer checks are complete.\n\n'
	  printf '| Key | Manual row | Runbook steps | Reviewer pass rule |\n'
	  printf '| --- | --- | --- | --- |\n'
	  printf '| `restaurant_template` | Restaurant template can create a project directly | 1-3 | Project is created from `restaurant_ordering_default`, has 7 forms, and `restaurant_calc` is active. |\n'
	  printf '| `frontend_usability` | Universal forms frontend is usable by a non-technical operator | 2, 4-6 | Grid/detail/create/edit/link workflows are understandable on desktop and mobile screenshots. |\n'
	  printf '| `amounts` | Amount, quantity, and subtotal behavior is acceptable | 4-5, 9 | Decimal amount input/display and `line_total` calculation are correct without float drift. |\n'
	  printf '| `workflow` | Order, order line, table change, print, and report workflow is acceptable | 5-9 | Restaurant order lifecycle, table change, print jobs, receipts, and business report are acceptable. |\n'
	  printf '| `hub_consistency` | MCP/API/Webhook/Connector consistency is acceptable | 9-10 | REST, MCP, webhook, connector, outbox/inbox, and receipt evidence describe the same business flow. |\n'
	  printf '| `docs` | README/docs are sufficient for a new user to reproduce | Preconditions, automated evidence, acceptance docs | A new user can follow README/docs/runbook and reproduce the delivery path. |\n'
	  printf '| `overall` | Overall acceptance | All steps after six rows pass | All automated gates are green, the delivery manifest matches, six prerequisite rows are accepted, and finalizer gates pass. |\n'
	  printf '\n'

	  printf '## Suggested Review Order\n\n'
  printf '1. Open the runbook and complete its 10 acceptance steps. Open `%s` when you need to trace a reviewed behavior back to source modules, public surfaces, verification commands, and allowed status markers.\n' "$IMPLEMENTATION_MAP_PATH"
  printf '2. Record each accepted row with `scripts/record-universal-forms-manual-signoff.sh --item <key> --status accepted --reviewer <name> --evidence <note>` so the runbook and evidence report stay synchronized. For the default handoff paths, the recorder also refreshes the derived acceptance handoff: completion audit, completion audit JSON, manual evidence map, UI review gallery, signoff status report, signoff status JSON, signoff status output smoke, next signoff review, user acceptance packet, readiness summary, readiness JSON, development status JSON, scenario catalog JSON, implementation map JSON, report output boundary smoke, next signoff command smoke, manual signoff progression smoke, release gate output smoke, delivery manifest, delivery manifest JSON, and their focused verifiers/contract smokes.\n'
  printf '3. Run `scripts/report-universal-forms-signoff-status.sh --reviewer <name>` whenever you need the next actionable item key, suggested evidence note, and recorder command template. The script also accepts a copied evidence report path as an optional positional argument.\n'
  printf '4. Open the next signoff review report for the one-page reviewer checklist, evidence files, screenshots, and recorder command for the current row.\n'
  printf '5. Open the manual signoff status report to see the persistent completed/pending row state in the delivery bundle.\n'
  printf '6. Open the automated evidence and inspect `## Automated Check Index`; it must list every automated check as `PASS` before manual signoff continues.\n'
  printf '7. Open the UI review gallery for grouped desktop/mobile screenshots and reviewer row mapping.\n'
  printf '8. Open the UI artifact manifest and inspect raw screenshot/log paths for project template wizard, template work items, Forms UI, and restaurant ordering.\n'
  printf '9. Open the delivery manifest and confirm the report, script, doc, and screenshot checksums match the reviewed bundle.\n'
  printf '10. Open the manual evidence map and use its suggested evidence notes when signing each row.\n'
  printf '11. Run `scripts/verify-universal-forms-ui-artifacts.sh`, `scripts/verify-universal-forms-ui-review-gallery.sh`, and `scripts/smoke-universal-forms-ui-review-gallery-render.sh` if screenshots are used as reviewer evidence.\n'
  printf '12. Run `scripts/record-universal-forms-manual-signoff.sh --list-items` if you need the item keys.\n'
  printf '13. Record `overall` only after the first six rows are accepted; the signoff recorder rejects early overall acceptance.\n'
  printf '14. If `overall` is already accepted, reopen `overall` before downgrading any prerequisite row; the recorder rejects prerequisite downgrade while overall acceptance is passed.\n'
  printf '15. Confirm every accepted row has reviewer and evidence notes in both generated tables.\n'
  printf '16. Run `scripts/verify-universal-forms-manual-signoff-consistency.sh` to confirm both manual tables are synchronized.\n'
  printf '17. Run the finalization gate below.\n\n'

  printf '## Finalization Gate\n\n'
printf '```bash\n'
  printf 'scripts/report-universal-forms-signoff-status.sh --reviewer "<name>"\n'
  printf 'scripts/report-universal-forms-signoff-status.sh --output \\\n'
  printf '  /opt/worker/report/openpr/docs/openpr-universal-form-signoff-status-2026-05-31.md\n'
  printf 'scripts/report-universal-forms-signoff-status.sh \\\n'
  printf '  /opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md \\\n'
  printf '  --reviewer "<name>"\n'
  printf 'scripts/report-universal-forms-signoff-status-json.sh\n'
  printf 'scripts/verify-universal-forms-signoff-status-json.sh\n'
  printf 'scripts/smoke-universal-forms-signoff-status-output.sh\n'
  printf 'scripts/smoke-universal-forms-next-signoff-command.sh\n'
  printf 'scripts/smoke-universal-forms-manual-signoff-progression.sh\n'
  printf 'scripts/smoke-universal-forms-manual-signoff-commands.sh\n'
  printf 'scripts/verify-universal-forms-readiness-json.sh\n'
  printf 'scripts/verify-universal-forms-development-status-json.sh\n'
  printf 'scripts/verify-universal-forms-scenario-catalog-json.sh\n'
  printf 'scripts/verify-universal-forms-delivery-manifest-json.sh\n'
  printf 'scripts/status-universal-forms-delivery.sh\n'
  printf 'scripts/verify-universal-forms-delivery-status-json.sh\n'
  printf 'scripts/smoke-universal-forms-delivery-status-json-contract.sh\n'
  printf 'scripts/smoke-universal-forms-delivery-status-output.sh\n'
  printf 'scripts/verify-universal-forms-manual-signoff-consistency.sh \\\n'
  printf '  /opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md \\\n'
  printf '  /opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md\n'
  printf 'scripts/verify-universal-forms-acceptance-signoff.sh \\\n'
  printf '  /opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md \\\n'
  printf '  --runbook /opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md\n'
  printf 'scripts/finalize-universal-forms-acceptance.sh \\\n'
  printf '  --runbook /opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md\n'
  printf 'scripts/audit-universal-forms-delivery-state.sh --strict\n'
  printf 'scripts/audit-universal-forms-delivery-bundle.sh\n'
  printf 'scripts/report-universal-forms-completion-audit-json.sh\n'
  printf 'scripts/verify-universal-forms-completion-audit-json.sh\n'
  printf 'scripts/smoke-universal-forms-completion-audit-json-contract.sh\n'
  printf 'scripts/gate-universal-forms-release.sh\n'
  printf 'scripts/verify-universal-forms-release-gate-json.sh\n'
  printf 'scripts/smoke-universal-forms-release-gate-json-contract.sh\n'
  printf 'scripts/smoke-universal-forms-release-gate-output.sh\n'
  printf '```\n\n'

  printf '## Stop Conditions\n\n'
  printf '%s\n' '- Do not finalize if any signoff row is still `Pending`, `失败`, or `需整改`.'
  printf '%s\n' '- Do not finalize if runbook and evidence manual signoff rows are not synchronized.'
  printf '%s\n' '- Do not finalize if automated failed checks is not `0`.'
  printf '%s\n' '- Do not finalize if tracker non-manual unresolved items is not `0`.'
  printf '%s\n' '- If the reviewer finds a UX or business-flow issue, record the issue in the evidence row and keep user-side manual acceptance as `待处理`.'
} >"$output_tmp"

if [[ -f "$OUTPUT_PATH" ]]; then
  chmod --reference="$OUTPUT_PATH" "$output_tmp"
fi
mv -f "$output_tmp" "$OUTPUT_PATH"
output_tmp=""

echo "user acceptance packet: $OUTPUT_PATH" >&2

if [[ "${summary_failed:-missing}" != "0" || "$pass_count" != "${summary_total:-}" || "$check_index_rows" != "${summary_total:-}" || "$check_index_failed_rows" != "0" || "${non_manual_unresolved:-missing}" != "0" || "$manual_consistency_status" != "passed" || "$ui_artifact_status" != "passed" || "$ui_gallery_status" != "passed" || "$ui_gallery_render_status" != "passed" ]]; then
  exit 1
fi
