#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
READINESS_JSON_PATH="$REPORT_DIR/openpr-universal-form-readiness-2026-05-31.json"
DEVELOPMENT_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-development-status-2026-05-31.json"
SIGNOFF_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-release-gate.schema.json"
ALLOW_PENDING=0
OUTPUT_JSON=0

usage() {
  cat <<'EOF'
Usage: scripts/gate-universal-forms-release.sh [--allow-pending] [--json]

Runs the read-only universal forms release gate. Strict mode exits 0 only when
the automated gates are green, all seven manual signoff rows are accepted, and
the tracker has been finalized. With --allow-pending, the command exits 0 for
the current pre-signoff handoff state when automation is green and the only
remaining blocker is user-side manual signoff.

Options:
  --allow-pending  Treat "ready for user-side manual signoff" as a successful
                   pre-release handoff state.
  --json           Print the final gate summary as JSON instead of text.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --allow-pending)
      ALLOW_PENDING=1
      shift
      ;;
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

for path in \
  "$READINESS_JSON_PATH" \
  "$DEVELOPMENT_STATUS_JSON_PATH" \
  "$SIGNOFF_STATUS_JSON_PATH" \
  "$EVIDENCE_PATH" \
  "$RUNBOOK_PATH" \
  "$SCHEMA_PATH"; do
  require_file "$path"
done

if ! command -v jq >/dev/null 2>&1; then
  echo "Missing required command: jq" >&2
  exit 2
fi

"$ROOT_DIR/scripts/verify-universal-forms-readiness-json.sh" "$READINESS_JSON_PATH" >/dev/null
"$ROOT_DIR/scripts/verify-universal-forms-development-status-json.sh" "$DEVELOPMENT_STATUS_JSON_PATH" >/dev/null
"$ROOT_DIR/scripts/verify-universal-forms-signoff-status-json.sh" "$SIGNOFF_STATUS_JSON_PATH" >/dev/null
"$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest-json.sh" >/dev/null

stage="$(jq -r '.stage' "$READINESS_JSON_PATH")"
final_acceptance_complete="$(jq -r '.final_acceptance_complete' "$READINESS_JSON_PATH")"
automated_failed="$(jq -r '.gates.failed_automated_checks' "$READINESS_JSON_PATH")"
automated_total="$(jq -r '.gates.automated_checks' "$READINESS_JSON_PATH")"
non_manual_unresolved="$(jq -r '.gates.tracker_non_manual_unresolved_items' "$READINESS_JSON_PATH")"
manual_pending="$(jq -r '.manual_signoff.pending_rows' "$READINESS_JSON_PATH")"
manual_next_key="$(jq -r '.manual_signoff.next_row.key // ""' "$READINESS_JSON_PATH")"
manual_final_allowed="$(jq -r '.manual_signoff.final_signoff_allowed // false' "$SIGNOFF_STATUS_JSON_PATH")"
development_final_allowed="$(jq -r '.status_summary.final_release_allowed' "$DEVELOPMENT_STATUS_JSON_PATH")"
tracker_e2e="$(jq -r '.tracker_status.end_to_end_acceptance' "$READINESS_JSON_PATH")"
tracker_manual="$(jq -r '.tracker_status.user_side_manual_acceptance' "$READINESS_JSON_PATH")"

if [[ "$final_acceptance_complete" == "true" || "$development_final_allowed" == "true" || "$manual_final_allowed" == "true" ]]; then
  "$ROOT_DIR/scripts/verify-universal-forms-acceptance-signoff.sh" \
    "$EVIDENCE_PATH" \
    --runbook "$RUNBOOK_PATH" >/dev/null
  "$ROOT_DIR/scripts/audit-universal-forms-delivery-state.sh" --strict >/dev/null
else
  "$ROOT_DIR/scripts/audit-universal-forms-delivery-state.sh" >/dev/null
fi

release_allowed=false
mode="blocked"
exit_code=1
reason="release gate is blocked"

if [[ "$automated_failed" == "0" && "$non_manual_unresolved" == "0" && "$manual_pending" == "0" && "$final_acceptance_complete" == "true" && "$manual_final_allowed" == "true" && "$development_final_allowed" == "true" && "$tracker_e2e" == "已验收" && "$tracker_manual" == "已验收" ]]; then
  release_allowed=true
  mode="release"
  exit_code=0
  reason="final release gate passed"
elif [[ "$ALLOW_PENDING" -eq 1 && "$automated_failed" == "0" && "$non_manual_unresolved" == "0" && "$manual_pending" -gt 0 && "$stage" == "Ready for user-side manual signoff" ]]; then
  mode="pre_signoff"
  exit_code=0
  reason="pre-release handoff is ready; user-side manual signoff is still pending"
elif [[ "$automated_failed" != "0" || "$non_manual_unresolved" != "0" ]]; then
  reason="automated or non-manual development gates are not green"
elif [[ "$manual_pending" != "0" ]]; then
  reason="user-side manual signoff is incomplete"
elif [[ "$final_acceptance_complete" != "true" || "$manual_final_allowed" != "true" || "$development_final_allowed" != "true" ]]; then
  reason="manual signoff is complete but tracker finalization is incomplete"
fi

if [[ "$OUTPUT_JSON" -eq 1 ]]; then
  jq -n \
    --arg schema_version "openpr.universal_forms.release_gate.v1" \
    --arg schema_path "$SCHEMA_PATH" \
    --arg mode "$mode" \
    --arg reason "$reason" \
    --arg stage "$stage" \
    --arg next_key "$manual_next_key" \
    --arg tracker_e2e "$tracker_e2e" \
    --arg tracker_manual "$tracker_manual" \
    --arg readiness_json "$READINESS_JSON_PATH" \
    --arg development_status_json "$DEVELOPMENT_STATUS_JSON_PATH" \
    --arg signoff_status_json "$SIGNOFF_STATUS_JSON_PATH" \
    --arg evidence "$EVIDENCE_PATH" \
    --arg runbook "$RUNBOOK_PATH" \
    --argjson release_allowed "$release_allowed" \
    --argjson automated_total "$automated_total" \
    --argjson automated_failed "$automated_failed" \
    --argjson non_manual_unresolved "$non_manual_unresolved" \
    --argjson manual_pending "$manual_pending" \
    --argjson final_acceptance_complete "$final_acceptance_complete" \
    --argjson manual_final_allowed "$manual_final_allowed" \
    --argjson development_final_allowed "$development_final_allowed" \
    '{
      schema_version: $schema_version,
      schema_path: $schema_path,
      reports: {
        readiness_json: $readiness_json,
        development_status_json: $development_status_json,
        signoff_status_json: $signoff_status_json,
        evidence: $evidence,
        runbook: $runbook
      },
      mode: $mode,
      release_allowed: $release_allowed,
      reason: $reason,
      stage: $stage,
      automated_checks: $automated_total,
      failed_automated_checks: $automated_failed,
      non_manual_unresolved_items: $non_manual_unresolved,
      manual_signoff_pending_rows: $manual_pending,
      next_manual_signoff_key: $next_key,
      final_acceptance_complete: $final_acceptance_complete,
      manual_final_signoff_allowed: $manual_final_allowed,
      development_final_release_allowed: $development_final_allowed,
      tracker_status: {
        end_to_end_acceptance: $tracker_e2e,
        user_side_manual_acceptance: $tracker_manual
      }
    }'
else
  printf 'Universal forms release gate\n'
  printf '  mode: %s\n' "$mode"
  printf '  release_allowed: %s\n' "$release_allowed"
  printf '  reason: %s\n' "$reason"
  printf '  stage: %s\n' "$stage"
  printf '  automated checks: %s total, %s failed\n' "$automated_total" "$automated_failed"
  printf '  non-manual unresolved items: %s\n' "$non_manual_unresolved"
  printf '  manual signoff pending rows: %s\n' "$manual_pending"
  if [[ -n "$manual_next_key" && "$manual_next_key" != "null" ]]; then
    printf '  next manual signoff key: %s\n' "$manual_next_key"
  fi
  printf '  tracker end-to-end acceptance: %s\n' "$tracker_e2e"
  printf '  tracker user-side manual acceptance: %s\n' "$tracker_manual"
fi

exit "$exit_code"
