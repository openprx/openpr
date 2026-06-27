#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
ARTIFACT_DIR="/opt/worker/report/openpr/artifacts/universal-forms-ui-2026-05-31"
SIGNOFF_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json"
SIGNOFF_STATUS_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.md"
MANUAL_EVIDENCE_MAP_PATH="$REPORT_DIR/openpr-universal-form-manual-evidence-map-2026-05-31.md"
USER_ACCEPTANCE_PACKET_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-packet-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
UI_REVIEW_GALLERY_PATH="$REPORT_DIR/openpr-universal-form-ui-review-gallery-2026-05-31.html"
DELIVERY_MANIFEST_PATH="$REPORT_DIR/openpr-universal-form-delivery-manifest-2026-05-31.md"
OUTPUT_PATH="${OPENPR_NEXT_SIGNOFF_REVIEW:-$REPORT_DIR/openpr-universal-form-next-signoff-review-2026-05-31.md}"
REVIEWER_PLACEHOLDER="<name>"

usage() {
  cat <<'EOF'
Usage: scripts/prepare-universal-forms-next-signoff-review.sh [options]

Generates a one-page reviewer aid for the next manual signoff row. It is
read-only and never marks a row accepted.

Options:
  --signoff-json PATH  Signoff status JSON path.
  --reviewer NAME      Reviewer name to place in the recorder command.
  --output PATH        Write the Markdown report to PATH.

Environment:
  OPENPR_NEXT_SIGNOFF_REVIEW  Optional output path.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --signoff-json)
      SIGNOFF_STATUS_JSON_PATH="${2:-}"
      shift 2
      ;;
    --reviewer)
      REVIEWER_PLACEHOLDER="${2:-}"
      shift 2
      ;;
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

shell_single_quote() {
  printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

append_review_scope() {
  local key="$1"
  case "$key" in
    restaurant_template)
      cat <<'EOF'
| Review target | Confirm `restaurant_ordering_default` can create a project directly. |
| Automated evidence | Scenario template forms and plugin smoke; project template wizard smoke; source coverage for `restaurant_ordering_default` and `restaurant_calc`. |
| UI evidence | Project template wizard desktop/mobile screenshots; UI review gallery. |
| Manual check | Runbook steps 1-3. Confirm 7 forms exist and `restaurant_calc` is active. |
EOF
      ;;
    frontend_usability)
      cat <<'EOF'
| Review target | Confirm a non-technical operator can use the universal forms grid/detail/create/edit/link flow. |
| Automated evidence | Project template wizard smoke; template work items smoke; Forms UI smoke; restaurant browser smoke. |
| UI evidence | Project template, template work items, Forms UI, and restaurant desktop/mobile screenshots. |
| Manual check | Runbook steps 2, 4, and 5. Confirm grid and detail interaction are understandable. |
EOF
      ;;
    amounts)
      cat <<'EOF'
| Review target | Confirm amount, quantity, subtotal, and decimal behavior are acceptable. |
| Automated evidence | Restaurant ordering backend smoke; WASM plugin runtime smoke; decimal and formula source coverage. |
| UI evidence | Forms UI and restaurant screenshots showing amount/quantity/subtotal surfaces. |
| Manual check | Confirm decimal values are string-safe and `line_total` is calculated by `restaurant_calc`. |
EOF
      ;;
    workflow)
      cat <<'EOF'
| Review target | Confirm order, order line, table change, print job, and report workflow. |
| Automated evidence | Restaurant ordering backend smoke; restaurant browser smoke; worker/connector source coverage. |
| UI evidence | Restaurant ordering desktop/mobile screenshots and UI review gallery. |
| Manual check | Runbook steps 4-9. Confirm operational flow is coherent for restaurant staff. |
EOF
      ;;
    hub_consistency)
      cat <<'EOF'
| Review target | Confirm API, MCP, webhook, connector, and business event flow are consistent. |
| Automated evidence | Universal forms MCP/CLI smoke; restaurant demo MCP HTTP smoke; generic webhook consumer smoke; worker outbox and connector receipt coverage. |
| UI evidence | Use UI only as scenario context; this row is primarily integration behavior. |
| Manual check | Confirm each ingress/egress path maps to the same universal forms and business event model. |
EOF
      ;;
    docs)
      cat <<'EOF'
| Review target | Confirm README, docs, runbook, and production guide are enough for a new user to reproduce. |
| Automated evidence | Docs/protocol audit; source coverage audit; production readiness audit. |
| UI evidence | UI review gallery as visual handoff reference. |
| Manual check | Follow the acceptance and production docs without relying on hidden local knowledge. |
EOF
      ;;
    overall)
      cat <<'EOF'
| Review target | Confirm final acceptance after the six prerequisite rows are accepted. |
| Automated evidence | Completion audit; delivery-state audit; delivery-bundle audit; delivery manifest; release gate smoke. |
| UI evidence | All reviewer screenshots and generated gallery. |
| Manual check | Verify all six prerequisite rows are accepted in both runbook and evidence before finalizing. |
EOF
      ;;
    *)
      cat <<'EOF'
| Review target | Unknown next signoff key. Refresh signoff status JSON before review. |
| Automated evidence | Missing. |
| UI evidence | Missing. |
| Manual check | Do not sign until the signoff status JSON is valid. |
EOF
      ;;
  esac
}

require_file "$SIGNOFF_STATUS_JSON_PATH"
require_file "$SIGNOFF_STATUS_PATH"
require_file "$MANUAL_EVIDENCE_MAP_PATH"
require_file "$USER_ACCEPTANCE_PACKET_PATH"
require_file "$RUNBOOK_PATH"
require_file "$EVIDENCE_PATH"
require_file "$UI_REVIEW_GALLERY_PATH"
require_file "$DELIVERY_MANIFEST_PATH"

"$ROOT_DIR/scripts/verify-universal-forms-signoff-status-json.sh" "$SIGNOFF_STATUS_JSON_PATH" >/dev/null
"$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh" "$RUNBOOK_PATH" "$EVIDENCE_PATH" >/dev/null

schema_version="$(jq -r '.schema_version' "$SIGNOFF_STATUS_JSON_PATH")"
automated_checks="$(jq -r '.gate_summary.automated_checks' "$SIGNOFF_STATUS_JSON_PATH")"
failed_checks="$(jq -r '.gate_summary.failed_automated_checks' "$SIGNOFF_STATUS_JSON_PATH")"
pending_rows="$(jq -r '.manual_signoff.pending_rows' "$SIGNOFF_STATUS_JSON_PATH")"
accepted_rows="$(jq -r '.manual_signoff.accepted_rows' "$SIGNOFF_STATUS_JSON_PATH")"
blocked_rows="$(jq -r '.manual_signoff.blocked_rows' "$SIGNOFF_STATUS_JSON_PATH")"
final_allowed="$(jq -r '.manual_signoff.final_signoff_allowed' "$SIGNOFF_STATUS_JSON_PATH")"
next_key="$(jq -r '.manual_signoff.next_row.key // ""' "$SIGNOFF_STATUS_JSON_PATH")"
next_item="$(jq -r '.manual_signoff.next_row.item // ""' "$SIGNOFF_STATUS_JSON_PATH")"
next_actionable="$(jq -r '.manual_signoff.next_row.actionable // false' "$SIGNOFF_STATUS_JSON_PATH")"
suggested_note="$(jq -r '.manual_signoff.next_row.suggested_evidence_note // ""' "$SIGNOFF_STATUS_JSON_PATH")"
recorder_command="$(jq -r '.manual_signoff.next_row.recorder_command // ""' "$SIGNOFF_STATUS_JSON_PATH")"

if [[ "$next_key" == "" && "$pending_rows" != "0" ]]; then
  echo "Signoff status JSON has pending rows but no next row key" >&2
  exit 1
fi

if [[ "$REVIEWER_PLACEHOLDER" != "<name>" && -n "$recorder_command" ]]; then
  recorder_command="${recorder_command//\<name\>/$REVIEWER_PLACEHOLDER}"
fi

mkdir -p "$(dirname "$OUTPUT_PATH")"
output_tmp="$(mktemp "$(dirname "$OUTPUT_PATH")/.next-signoff-review.XXXXXX")"
trap 'rm -f "$output_tmp"' EXIT

{
  printf '# OpenPR Universal Forms Next Signoff Review\n\n'
  printf '%s\n' "- Generated at: $(date -Is)"
  printf '%s\n' "- Repository: \`$ROOT_DIR\`"
  printf '%s\n' "- Signoff JSON schema: \`$schema_version\`"
  printf '%s\n' "- Signoff status JSON: \`$SIGNOFF_STATUS_JSON_PATH\`"
  printf '%s\n' "- Signoff status report: \`$SIGNOFF_STATUS_PATH\`"
  printf '%s\n' "- Manual evidence map: \`$MANUAL_EVIDENCE_MAP_PATH\`"
  printf '%s\n' "- User acceptance packet: \`$USER_ACCEPTANCE_PACKET_PATH\`"
  printf '%s\n' "- Manual runbook: \`$RUNBOOK_PATH\`"
  printf '%s\n' "- Automated evidence: \`$EVIDENCE_PATH\`"
  printf '%s\n' "- UI review gallery: \`$UI_REVIEW_GALLERY_PATH\`"
  printf '%s\n' "- Delivery manifest: \`$DELIVERY_MANIFEST_PATH\`"
  printf '\n'

  printf '## Gate Snapshot\n\n'
  printf '| Gate | Current value | Required before final release |\n'
  printf '| --- | --- | --- |\n'
  printf '| Automated checks | %s | all PASS |\n' "$automated_checks"
  printf '| Failed automated checks | %s | 0 |\n' "$failed_checks"
  printf '| Accepted manual rows | %s | 7 |\n' "$accepted_rows"
  printf '| Pending manual rows | %s | 0 |\n' "$pending_rows"
  printf '| Blocked manual rows | %s | 0 |\n' "$blocked_rows"
  printf '| Final signoff allowed | %s | true |\n' "$final_allowed"
  printf '\n'

  printf '## Next Row\n\n'
  if [[ "$pending_rows" == "0" ]]; then
    printf 'All manual rows are accepted. Run finalization instead of recording another row.\n\n'
    printf '```bash\n'
    printf 'scripts/finalize-universal-forms-acceptance.sh\n'
    printf 'scripts/audit-universal-forms-delivery-state.sh --strict\n'
    printf 'scripts/audit-universal-forms-delivery-bundle.sh\n'
    printf '```\n'
  else
    printf '| Field | Value |\n'
    printf '| --- | --- |\n'
    printf '| Key | `%s` |\n' "$next_key"
    printf '| Item | %s |\n' "$next_item"
    printf '| Actionable | %s |\n' "$next_actionable"
    printf '| Suggested evidence note | `%s` |\n' "$suggested_note"
    printf '\n'

    printf '## Review Scope\n\n'
    printf '| Field | Reviewer check |\n'
    printf '| --- | --- |\n'
    append_review_scope "$next_key"
    printf '\n'

    printf '## Evidence Files To Open\n\n'
    printf '| Evidence | Path |\n'
    printf '| --- | --- |\n'
    printf '| Manual runbook | `%s` |\n' "$RUNBOOK_PATH"
    printf '| Automated evidence | `%s` |\n' "$EVIDENCE_PATH"
    printf '| Manual evidence map | `%s` |\n' "$MANUAL_EVIDENCE_MAP_PATH"
    printf '| Signoff status report | `%s` |\n' "$SIGNOFF_STATUS_PATH"
    printf '| UI review gallery | `%s` |\n' "$UI_REVIEW_GALLERY_PATH"
    printf '| Project template desktop screenshot | `%s/project-template-wizard/project-template-wizard-desktop.png` |\n' "$ARTIFACT_DIR"
    printf '| Project template mobile screenshot | `%s/project-template-wizard/project-template-wizard-mobile.png` |\n' "$ARTIFACT_DIR"
    printf '| Forms UI desktop screenshot | `%s/forms-ui/forms-ui-desktop.png` |\n' "$ARTIFACT_DIR"
    printf '| Forms UI mobile screenshot | `%s/forms-ui/forms-ui-mobile.png` |\n' "$ARTIFACT_DIR"
    printf '| Restaurant desktop screenshot | `%s/restaurant-ordering/restaurant-ordering-desktop.png` |\n' "$ARTIFACT_DIR"
    printf '| Restaurant mobile screenshot | `%s/restaurant-ordering/restaurant-ordering-mobile.png` |\n' "$ARTIFACT_DIR"
    printf '\n'

    printf '## Verification Commands\n\n'
    printf '```bash\n'
    printf 'scripts/verify-universal-forms-signoff-status-json.sh %s\n' "$(shell_single_quote "$SIGNOFF_STATUS_JSON_PATH")"
    printf 'scripts/verify-universal-forms-manual-signoff-consistency.sh %s %s\n' \
      "$(shell_single_quote "$RUNBOOK_PATH")" \
      "$(shell_single_quote "$EVIDENCE_PATH")"
    printf 'scripts/verify-universal-forms-ui-artifacts.sh\n'
    printf 'scripts/verify-universal-forms-ui-review-gallery.sh\n'
    printf 'scripts/smoke-universal-forms-ui-review-gallery-render.sh\n'
    printf '```\n\n'

    printf '## Recorder Command After Reviewer Approval\n\n'
    printf 'Use this only after the reviewer has completed the row-specific checks above.\n\n'
    printf '```bash\n'
    printf '%s\n' "$recorder_command"
    printf '```\n'
  fi
} >"$output_tmp"

if [[ -f "$OUTPUT_PATH" ]]; then
  chmod --reference="$OUTPUT_PATH" "$output_tmp"
fi
mv -f "$output_tmp" "$OUTPUT_PATH"
output_tmp=""

echo "next signoff review: $OUTPUT_PATH" >&2
