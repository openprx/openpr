#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
ARTIFACT_DIR="/opt/worker/report/openpr/artifacts/universal-forms-ui-2026-05-31"
REVIEW_PATH="${1:-$REPORT_DIR/openpr-universal-form-next-signoff-review-2026-05-31.md}"
SIGNOFF_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json"
SIGNOFF_STATUS_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.md"
MANUAL_EVIDENCE_MAP_PATH="$REPORT_DIR/openpr-universal-form-manual-evidence-map-2026-05-31.md"
USER_ACCEPTANCE_PACKET_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-packet-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
UI_REVIEW_GALLERY_PATH="$REPORT_DIR/openpr-universal-form-ui-review-gallery-2026-05-31.html"
DELIVERY_MANIFEST_PATH="$REPORT_DIR/openpr-universal-form-delivery-manifest-2026-05-31.md"

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-next-signoff-review.sh [REVIEW_PATH]

Verifies the one-page next manual signoff review Markdown against the current
signoff status JSON and delivery evidence. This is read-only and never marks a
manual row accepted.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

failures=0

pass() {
  printf 'PASS: %s\n' "$1"
}

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  failures=$((failures + 1))
}

equals() {
  local description="$1"
  local actual="$2"
  local expected="$3"
  if [[ "$actual" == "$expected" ]]; then
    pass "$description"
  else
    fail "$description"
    printf '  expected: %s\n  actual: %s\n' "$expected" "${actual:-<empty>}" >&2
  fi
}

require_file() {
  local path="$1"
  if [[ -f "$path" ]]; then
    pass "file exists: $path"
  else
    fail "file missing: $path"
  fi
}

contains() {
  local description="$1"
  local path="$2"
  local needle="$3"
  if rg -q --fixed-strings -- "$needle" "$path"; then
    pass "$description"
  else
    fail "$description"
    printf '  missing in %s: %s\n' "$path" "$needle" >&2
  fi
}

printf 'Universal forms next signoff review verification\n'
printf '  review: %s\n' "$REVIEW_PATH"
printf '\n'

if command -v jq >/dev/null 2>&1; then
  pass "jq is available"
else
  fail "jq is available"
fi

if command -v rg >/dev/null 2>&1; then
  pass "ripgrep is available"
else
  fail "ripgrep is available"
fi

for path in \
  "$REVIEW_PATH" \
  "$SIGNOFF_STATUS_JSON_PATH" \
  "$SIGNOFF_STATUS_PATH" \
  "$MANUAL_EVIDENCE_MAP_PATH" \
  "$USER_ACCEPTANCE_PACKET_PATH" \
  "$RUNBOOK_PATH" \
  "$EVIDENCE_PATH" \
  "$UI_REVIEW_GALLERY_PATH" \
  "$DELIVERY_MANIFEST_PATH"; do
  require_file "$path"
done

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms next signoff review verification failed before consistency checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

"$ROOT_DIR/scripts/verify-universal-forms-signoff-status-json.sh" "$SIGNOFF_STATUS_JSON_PATH" >/dev/null
"$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh" "$RUNBOOK_PATH" "$EVIDENCE_PATH" >/dev/null

equals "review starts with Markdown heading" "$(head -n 1 "$REVIEW_PATH")" "# OpenPR Universal Forms Next Signoff Review"
contains "review points to repository" "$REVIEW_PATH" "Repository: \`$ROOT_DIR\`"
contains "review points to signoff status JSON" "$REVIEW_PATH" "$SIGNOFF_STATUS_JSON_PATH"
contains "review points to signoff status report" "$REVIEW_PATH" "$SIGNOFF_STATUS_PATH"
contains "review points to manual evidence map" "$REVIEW_PATH" "$MANUAL_EVIDENCE_MAP_PATH"
contains "review points to user acceptance packet" "$REVIEW_PATH" "$USER_ACCEPTANCE_PACKET_PATH"
contains "review points to manual runbook" "$REVIEW_PATH" "$RUNBOOK_PATH"
contains "review points to automated evidence" "$REVIEW_PATH" "$EVIDENCE_PATH"
contains "review points to UI review gallery" "$REVIEW_PATH" "$UI_REVIEW_GALLERY_PATH"
contains "review points to delivery manifest" "$REVIEW_PATH" "$DELIVERY_MANIFEST_PATH"

automated_checks="$(jq -r '.gate_summary.automated_checks' "$SIGNOFF_STATUS_JSON_PATH")"
failed_checks="$(jq -r '.gate_summary.failed_automated_checks' "$SIGNOFF_STATUS_JSON_PATH")"
accepted_rows="$(jq -r '.manual_signoff.accepted_rows' "$SIGNOFF_STATUS_JSON_PATH")"
pending_rows="$(jq -r '.manual_signoff.pending_rows' "$SIGNOFF_STATUS_JSON_PATH")"
blocked_rows="$(jq -r '.manual_signoff.blocked_rows' "$SIGNOFF_STATUS_JSON_PATH")"
final_allowed="$(jq -r '.manual_signoff.final_signoff_allowed' "$SIGNOFF_STATUS_JSON_PATH")"
next_key="$(jq -r '.manual_signoff.next_row.key // ""' "$SIGNOFF_STATUS_JSON_PATH")"
next_item="$(jq -r '.manual_signoff.next_row.item // ""' "$SIGNOFF_STATUS_JSON_PATH")"
next_actionable="$(jq -r '.manual_signoff.next_row.actionable // false' "$SIGNOFF_STATUS_JSON_PATH")"
suggested_note="$(jq -r '.manual_signoff.next_row.suggested_evidence_note // ""' "$SIGNOFF_STATUS_JSON_PATH")"
recorder_command="$(jq -r '.manual_signoff.next_row.recorder_command // ""' "$SIGNOFF_STATUS_JSON_PATH")"

contains "review mirrors automated check count" "$REVIEW_PATH" "| Automated checks | $automated_checks | all PASS |"
contains "review mirrors failed check count" "$REVIEW_PATH" "| Failed automated checks | $failed_checks | 0 |"
contains "review mirrors accepted manual row count" "$REVIEW_PATH" "| Accepted manual rows | $accepted_rows | 7 |"
contains "review mirrors pending manual row count" "$REVIEW_PATH" "| Pending manual rows | $pending_rows | 0 |"
contains "review mirrors blocked manual row count" "$REVIEW_PATH" "| Blocked manual rows | $blocked_rows | 0 |"
contains "review mirrors final signoff flag" "$REVIEW_PATH" "| Final signoff allowed | $final_allowed | true |"

if [[ "$pending_rows" == "0" ]]; then
  contains "review switches to finalization guidance after signoff" "$REVIEW_PATH" "All manual rows are accepted. Run finalization instead of recording another row."
  contains "review includes finalizer command after signoff" "$REVIEW_PATH" "scripts/finalize-universal-forms-acceptance.sh"
else
  if [[ -z "$next_key" ]]; then
    fail "pending manual rows expose a next signoff key"
  else
    pass "pending manual rows expose a next signoff key"
  fi
  contains "review mirrors next signoff key" "$REVIEW_PATH" "| Key | \`$next_key\` |"
  contains "review mirrors next signoff item" "$REVIEW_PATH" "| Item | $next_item |"
  contains "review mirrors next signoff actionable flag" "$REVIEW_PATH" "| Actionable | $next_actionable |"
  contains "review mirrors suggested evidence note" "$REVIEW_PATH" "| Suggested evidence note | \`$suggested_note\` |"
  contains "review mirrors recorder command" "$REVIEW_PATH" "$recorder_command"
  contains "review includes row review scope" "$REVIEW_PATH" "## Review Scope"
  contains "review includes evidence file table" "$REVIEW_PATH" "## Evidence Files To Open"
  contains "review includes verification commands" "$REVIEW_PATH" "## Verification Commands"

  for artifact in \
    "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-desktop.png" \
    "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-mobile.png" \
    "$ARTIFACT_DIR/forms-ui/forms-ui-desktop.png" \
    "$ARTIFACT_DIR/forms-ui/forms-ui-mobile.png" \
    "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-desktop.png" \
    "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-mobile.png"; do
    require_file "$artifact"
    contains "review links artifact: $artifact" "$REVIEW_PATH" "$artifact"
  done

  contains "review verifies signoff status JSON command" "$REVIEW_PATH" "scripts/verify-universal-forms-signoff-status-json.sh"
  contains "review verifies manual signoff consistency command" "$REVIEW_PATH" "scripts/verify-universal-forms-manual-signoff-consistency.sh"
  contains "review verifies UI artifacts command" "$REVIEW_PATH" "scripts/verify-universal-forms-ui-artifacts.sh"
  contains "review verifies UI gallery command" "$REVIEW_PATH" "scripts/verify-universal-forms-ui-review-gallery.sh"
  contains "review verifies UI gallery render command" "$REVIEW_PATH" "scripts/smoke-universal-forms-ui-review-gallery-render.sh"
fi

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms next signoff review verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms next signoff review verification passed.\n'
