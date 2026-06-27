#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
MODE="full"

usage() {
  cat <<'EOF'
Usage: scripts/refresh-universal-forms-delivery-bundle.sh [--quick|--full]

Regenerates the formal universal forms delivery bundle in the required order:
acceptance evidence, completion audit, manual evidence map, UI review gallery,
signoff status, signoff dashboard, signoff dashboard render smoke, signoff dashboard progression smoke,
signoff status output smoke, next signoff review, next signoff review verifier, user
signoff review contract smoke, user acceptance packet, readiness summary, readiness JSON, development status JSON,
completion audit JSON, manual signoff status JSON, next signoff command smoke,
manual signoff progression smoke,
report output boundary smoke,
delivery manifest, delivery manifest JSON, release gate smoke, release gate output smoke,
delivery status command, delivery status output smoke, docs/protocol audit, delivery-state audit, and
delivery-bundle audit.

The script does not finalize acceptance. If manual signoff rows are still
pending, it verifies that the final signoff gate rejects the unsigned evidence.

Modes:
  --quick   Run focused smoke acceptance checks against a temporary report only.
            This is a preflight and does not refresh the formal delivery bundle.
  --full    Refresh the formal bundle with CI-grade checks plus focused smoke
            checks. Default.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --quick)
      MODE="quick"
      shift
      ;;
    --full)
      MODE="full"
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

cd "$ROOT_DIR"

printf 'Refreshing universal forms delivery bundle\n'
printf '  mode: %s\n' "$MODE"
printf '  evidence: %s\n' "$EVIDENCE_PATH"
printf '\n'

if [[ "$MODE" == "quick" ]]; then
  quick_report="$(mktemp /tmp/openpr-universal-forms-quick-preflight.XXXXXX.md)"
  "$ROOT_DIR/scripts/acceptance-universal-forms.sh" --quick --output "$quick_report"
  printf 'Quick preflight evidence: %s\n' "$quick_report"
  printf 'Quick mode does not overwrite the formal delivery bundle. Run this script with --full to refresh formal evidence and handoff reports.\n'
  exit 0
fi

"$ROOT_DIR/scripts/acceptance-universal-forms.sh" "--$MODE" --output "$EVIDENCE_PATH"
"$ROOT_DIR/scripts/report-universal-forms-completion-audit.sh"
"$ROOT_DIR/scripts/prepare-universal-forms-manual-evidence-map.sh"
"$ROOT_DIR/scripts/prepare-universal-forms-ui-review-gallery.sh"
"$ROOT_DIR/scripts/verify-universal-forms-ui-review-gallery.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-ui-review-gallery-render.sh"
"$ROOT_DIR/scripts/report-universal-forms-signoff-status.sh" \
  --output "$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.md"
"$ROOT_DIR/scripts/report-universal-forms-signoff-status-json.sh"
"$ROOT_DIR/scripts/verify-universal-forms-signoff-status-json.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-signoff-status-json-contract.sh"
"$ROOT_DIR/scripts/prepare-universal-forms-signoff-dashboard.sh"
"$ROOT_DIR/scripts/verify-universal-forms-signoff-dashboard.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-signoff-dashboard-render.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-signoff-dashboard-progression.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-signoff-status-output.sh"
"$ROOT_DIR/scripts/prepare-universal-forms-next-signoff-review.sh"
"$ROOT_DIR/scripts/verify-universal-forms-next-signoff-review.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-next-signoff-review-contract.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-next-signoff-command.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-manual-signoff-progression.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-manual-signoff-commands.sh"
"$ROOT_DIR/scripts/prepare-universal-forms-user-acceptance-packet.sh"
"$ROOT_DIR/scripts/report-universal-forms-readiness-summary.sh"
"$ROOT_DIR/scripts/report-universal-forms-readiness-json.sh"
"$ROOT_DIR/scripts/verify-universal-forms-readiness-json.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-readiness-json-contract.sh"
"$ROOT_DIR/scripts/report-universal-forms-development-status-json.sh"
"$ROOT_DIR/scripts/verify-universal-forms-development-status-json.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-development-status-json-contract.sh"
"$ROOT_DIR/scripts/report-universal-forms-completion-audit-json.sh"
"$ROOT_DIR/scripts/verify-universal-forms-completion-audit-json.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-completion-audit-json-contract.sh"
"$ROOT_DIR/scripts/report-universal-forms-scenario-catalog-json.sh"
"$ROOT_DIR/scripts/verify-universal-forms-scenario-catalog-json.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-scenario-catalog-json-contract.sh"
"$ROOT_DIR/scripts/report-universal-forms-implementation-map-json.sh"
"$ROOT_DIR/scripts/verify-universal-forms-implementation-map-json.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-implementation-map-json-contract.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-report-output-boundaries.sh"
"$ROOT_DIR/scripts/prepare-universal-forms-delivery-manifest.sh"
"$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest.sh"
"$ROOT_DIR/scripts/report-universal-forms-delivery-manifest-json.sh"
"$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest-json.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-delivery-manifest-json-contract.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-release-gate.sh"
"$ROOT_DIR/scripts/verify-universal-forms-release-gate-json.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-release-gate-json-contract.sh"
"$ROOT_DIR/scripts/smoke-universal-forms-release-gate-output.sh"
"$ROOT_DIR/scripts/status-universal-forms-delivery.sh" >/dev/null
"$ROOT_DIR/scripts/verify-universal-forms-delivery-status-json.sh" >/dev/null
"$ROOT_DIR/scripts/smoke-universal-forms-delivery-status-json-contract.sh" >/dev/null
"$ROOT_DIR/scripts/smoke-universal-forms-delivery-status-output.sh" >/dev/null
"$ROOT_DIR/scripts/audit-universal-forms-docs.sh"
"$ROOT_DIR/scripts/audit-universal-forms-delivery-state.sh"
"$ROOT_DIR/scripts/audit-universal-forms-delivery-bundle.sh"

pending_rows="$(rg -c '^\| .* \| Pending \|' "$EVIDENCE_PATH" || true)"
if [[ "$pending_rows" == "0" ]]; then
  "$ROOT_DIR/scripts/verify-universal-forms-acceptance-signoff.sh" "$EVIDENCE_PATH"
  printf 'Manual signoff is complete. Run scripts/finalize-universal-forms-acceptance.sh to finalize the tracker.\n'
else
  if "$ROOT_DIR/scripts/verify-universal-forms-acceptance-signoff.sh" "$EVIDENCE_PATH" >/tmp/openpr-universal-forms-signoff-check.log 2>&1; then
    cat /tmp/openpr-universal-forms-signoff-check.log
    echo "Unsigned evidence unexpectedly passed final signoff verification" >&2
    exit 1
  fi
  printf 'Manual signoff is still pending: %s row(s). Final signoff gate rejected unsigned evidence as expected.\n' "$pending_rows"
fi

printf '\nUniversal forms delivery bundle refreshed.\n'
