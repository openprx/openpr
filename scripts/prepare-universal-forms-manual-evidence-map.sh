#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
UI_ARTIFACT_MANIFEST_PATH="$REPORT_DIR/openpr-universal-form-ui-artifacts-2026-05-31.md"
COMPLETION_AUDIT_PATH="$REPORT_DIR/openpr-universal-form-completion-audit-2026-05-31.md"
DELIVERY_MANIFEST_PATH="$REPORT_DIR/openpr-universal-form-delivery-manifest-2026-05-31.md"
OUTPUT_PATH="${OPENPR_MANUAL_EVIDENCE_MAP:-$REPORT_DIR/openpr-universal-form-manual-evidence-map-2026-05-31.md}"

usage() {
  cat <<'EOF'
Usage: scripts/prepare-universal-forms-manual-evidence-map.sh [--output PATH]

Generates a reviewer-facing evidence map for the seven manual acceptance rows.
It does not sign acceptance. It links each pending row to automated checks,
UI artifacts, and reviewer commands so human signoff can be done without
searching multiple reports.

Environment:
  OPENPR_MANUAL_EVIDENCE_MAP  Optional output path.
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

require_file "$EVIDENCE_PATH"
require_file "$RUNBOOK_PATH"
require_file "$UI_ARTIFACT_MANIFEST_PATH"
require_file "$COMPLETION_AUDIT_PATH"

summary_total="$(sed -n 's/^- Total automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
summary_failed="$(sed -n 's/^- Failed automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
pass_count="$(rg -c '^\*\*Status:\*\* PASS$' "$EVIDENCE_PATH" || true)"
manual_pending_count="$(rg -c '^\| .* \| Pending \|' "$EVIDENCE_PATH" || true)"

"$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh" "$RUNBOOK_PATH" "$EVIDENCE_PATH" >/dev/null
"$ROOT_DIR/scripts/verify-universal-forms-ui-artifacts.sh" >/dev/null

mkdir -p "$(dirname "$OUTPUT_PATH")"
output_tmp="$(mktemp "$(dirname "$OUTPUT_PATH")/.manual-evidence-map.XXXXXX")"
trap 'rm -f "$output_tmp"' EXIT

{
  printf '# OpenPR Universal Forms Manual Evidence Map\n\n'
  printf '%s\n' "- Generated at: $(date -Is)"
  printf '%s\n' "- Repository: \`$ROOT_DIR\`"
  printf '%s\n' "- Automated evidence: \`$EVIDENCE_PATH\`"
  printf '%s\n' "- Manual runbook: \`$RUNBOOK_PATH\`"
  printf '%s\n' "- UI artifacts: \`$UI_ARTIFACT_MANIFEST_PATH\`"
  printf '%s\n' "- Completion audit: \`$COMPLETION_AUDIT_PATH\`"
  printf '%s\n' "- Delivery manifest: \`$DELIVERY_MANIFEST_PATH\`"
  printf '\n'

  printf '## Pre-Signoff State\n\n'
  printf '| Gate | Current value | Required before finalizer |\n'
  printf '| --- | --- | --- |\n'
  printf '| Automated checks | %s PASS / %s failed | 0 failed and all PASS |\n' "${summary_total:-missing}" "${summary_failed:-missing}"
  printf '| PASS status lines | %s | Must equal automated check count |\n' "$pass_count"
  printf '| Manual signoff rows pending | %s | 0 after reviewer approval |\n' "$manual_pending_count"
  printf '| Runbook/evidence consistency | passed | Must stay passed |\n'
  printf '| UI artifact verification | passed | Must stay passed |\n'
  printf '\n'

  printf '## Manual Rows\n\n'
  printf '| Item | Status | Reviewer | Evidence |\n'
  printf '| --- | --- | --- | --- |\n'
  manual_rows
  printf '\n'

  printf '## Evidence Map\n\n'
  printf '| Manual row | Automated evidence | UI artifact / reviewer check | Suggested evidence note |\n'
  printf '| --- | --- | --- | --- |\n'
  printf '| Restaurant template can create a project directly | `Scenario template forms and plugin smoke`; `Frontend project template wizard smoke`; source coverage checks `restaurant_ordering_default` and `restaurant_calc`; docs/protocol audit | Inspect project template wizard screenshots and confirm project creation from runbook steps 1-3 | `scenario template smoke PASS; project template wizard screenshots inspected; restaurant_ordering_default creates 7 forms and active restaurant_calc plugin` |\n'
  printf '| Universal forms frontend is usable by a non-technical operator | `Frontend project template wizard smoke`; `Frontend template work items smoke`; `Frontend forms browser smoke`; `Frontend restaurant browser smoke`; UI artifact verifier | Inspect project template wizard, template work-item, and Forms UI desktop/mobile screenshots and complete runbook steps 2, 4, 5 | `template wizard, template work item, and Forms UI desktop/mobile screenshots inspected; grid/detail/create/edit/link workflow acceptable` |\n'
  printf '| Amount, quantity, and subtotal behavior is acceptable | `Restaurant ordering backend smoke`; `Frontend forms browser smoke`; `WASM plugin runtime smoke`; source coverage checks decimal and formula hook | Confirm decimal string and currency display in Forms/restaurant screenshots | `decimal amount and subtotal behavior verified; JSON number rejected; line_total calculated by restaurant_calc` |\n'
  printf '| Order, order line, table change, print, and report workflow is acceptable | `Restaurant ordering backend smoke`; `Frontend restaurant browser smoke`; UI artifact verifier | Inspect restaurant desktop/mobile screenshots and runbook steps 4-9 | `order/order_line/table change/print_job/business_report workflow verified from backend and browser smoke` |\n'
  printf '| MCP/API/Webhook/Connector consistency is acceptable | `Universal forms MCP and generic CLI smoke`; `Restaurant demo bootstrap MCP HTTP smoke`; `Generic webhook consumer smoke`; source coverage checks MCP tools, worker outbox, connector receipt | Confirm API/MCP/Webhook/Connector all use same form/business event flow and the demo MCP HTTP onboarding path | `MCP forms aggregate/events, generic CLI, demo bootstrap MCP HTTP projects.list, webhook consumer, print/device connector and receipt path verified` |\n'
  printf '| README/docs are sufficient for a new user to reproduce | `Universal forms docs and protocol audit`; source coverage audit; acceptance guide | Review README, docs index, universal forms guide, acceptance guide and runbook | `README/docs/runbook reviewed; commands and expected delivery surface are reproducible` |\n'
  printf '| Overall acceptance | Completion audit; delivery-state audit; delivery-bundle audit; delivery manifest; all %s automated checks; the six rows above | Verify every previous row is passed in both runbook and evidence | `all automated gates PASS, delivery manifest reviewed, six manual rows accepted, finalizer dry path verified` |\n' "${summary_total:-generated}"
  printf '\n'

  printf '## Suggested Recorder Commands\n\n'
  printf 'Use these only after the reviewer has actually completed the matching runbook checks. The `overall` row must remain pending until the first six rows are accepted.\n\n'
  printf '```bash\n'
  printf 'scripts/record-universal-forms-manual-signoff.sh --item restaurant_template --status accepted --reviewer "<name>" --evidence "scenario template smoke PASS; restaurant_ordering_default creates 7 forms and active restaurant_calc plugin"\n'
  printf 'scripts/record-universal-forms-manual-signoff.sh --item frontend_usability --status accepted --reviewer "<name>" --evidence "template wizard, template work item, and Forms UI desktop/mobile screenshots inspected; grid/detail/create/edit/link workflow acceptable"\n'
  printf 'scripts/record-universal-forms-manual-signoff.sh --item amounts --status accepted --reviewer "<name>" --evidence "decimal amount and subtotal behavior verified; JSON number rejected; line_total calculated by restaurant_calc"\n'
  printf 'scripts/record-universal-forms-manual-signoff.sh --item workflow --status accepted --reviewer "<name>" --evidence "order/order_line/table change/print_job/business_report workflow verified from backend and browser smoke"\n'
  printf 'scripts/record-universal-forms-manual-signoff.sh --item hub_consistency --status accepted --reviewer "<name>" --evidence "MCP forms aggregate/events, generic CLI, demo bootstrap MCP HTTP projects.list, webhook consumer, print/device connector and receipt path verified"\n'
  printf 'scripts/record-universal-forms-manual-signoff.sh --item docs --status accepted --reviewer "<name>" --evidence "README/docs/runbook reviewed; commands and expected delivery surface are reproducible"\n'
  printf 'scripts/record-universal-forms-manual-signoff.sh --item overall --status accepted --reviewer "<name>" --evidence "all automated gates PASS, delivery manifest reviewed, six manual rows accepted, finalizer dry path verified"\n'
  printf '```\n\n'

  printf '## Reviewer Commands\n\n'
  printf '```bash\n'
  printf 'scripts/report-universal-forms-signoff-status.sh --reviewer "<name>"\n'
  printf 'scripts/report-universal-forms-signoff-status.sh /opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md --reviewer "<name>"\n'
  printf 'scripts/record-universal-forms-manual-signoff.sh --list-items\n'
  printf 'scripts/record-universal-forms-manual-signoff.sh --item restaurant_template --status accepted --reviewer "<name>" --evidence "<note>"\n'
  printf 'scripts/verify-universal-forms-ui-artifacts.sh\n'
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

echo "manual evidence map: $OUTPUT_PATH" >&2

if [[ "${summary_failed:-missing}" != "0" || "$pass_count" != "${summary_total:-}" ]]; then
  exit 1
fi
