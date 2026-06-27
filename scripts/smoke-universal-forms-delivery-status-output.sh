#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d /tmp/openpr-uf-delivery-status-output.XXXXXX)"
trap 'rm -rf "$TMP_DIR"' EXIT

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-delivery-status-output.sh

Verifies the human-readable delivery status output mirrors the compact JSON
status for stage, gate counts, release gate state, manifest count, and next
manual signoff command. This is read-only and does not record acceptance.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if [[ $# -gt 0 ]]; then
  echo "Unknown argument: $1" >&2
  usage >&2
  exit 2
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for delivery status output smoke" >&2
  exit 2
fi

failures=0

pass() {
  printf 'PASS: %s\n' "$1"
}

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  failures=$((failures + 1))
}

contains() {
  local description="$1"
  local needle="$2"
  if rg -q --fixed-strings -- "$needle" "$TEXT_OUTPUT"; then
    pass "$description"
  else
    fail "$description"
    printf '  missing: %s\n' "$needle" >&2
  fi
}

not_contains() {
  local description="$1"
  local needle="$2"
  if rg -q --fixed-strings -- "$needle" "$TEXT_OUTPUT"; then
    fail "$description"
    printf '  forbidden: %s\n' "$needle" >&2
  else
    pass "$description"
  fi
}

empty_file() {
  local description="$1"
  local path="$2"
  if [[ ! -s "$path" ]]; then
    pass "$description"
  else
    fail "$description"
    sed -n '1,20p' "$path" >&2
  fi
}

printf 'Universal forms delivery status output smoke\n'
printf '  repo: %s\n' "$ROOT_DIR"
printf '\n'

JSON_OUTPUT="$TMP_DIR/status.json"
JSON_STDERR="$TMP_DIR/status-json.stderr"
TEXT_OUTPUT="$TMP_DIR/status.txt"
TEXT_STDERR="$TMP_DIR/status-text.stderr"

"$ROOT_DIR/scripts/status-universal-forms-delivery.sh" --json >"$JSON_OUTPUT" 2>"$JSON_STDERR"
empty_file "delivery status JSON output leaves stderr empty on success" "$JSON_STDERR"
"$ROOT_DIR/scripts/verify-universal-forms-delivery-status-json.sh" "$JSON_OUTPUT" >/dev/null
pass "delivery status JSON verifier accepts generated JSON"

"$ROOT_DIR/scripts/status-universal-forms-delivery.sh" >"$TEXT_OUTPUT" 2>"$TEXT_STDERR"
empty_file "delivery status text output leaves stderr empty on success" "$TEXT_STDERR"
pass "delivery status text output is generated"

stage="$(jq -r '.stage' "$JSON_OUTPUT")"
automated_checks="$(jq -r '.automated_checks' "$JSON_OUTPUT")"
failed_checks="$(jq -r '.failed_automated_checks' "$JSON_OUTPUT")"
non_manual_unresolved="$(jq -r '.non_manual_unresolved_items' "$JSON_OUTPUT")"
pending_rows="$(jq -r '.manual_signoff_pending_rows' "$JSON_OUTPUT")"
accepted_rows="$(jq -r '.completion_summary.manual_signoff_accepted_rows' "$JSON_OUTPUT")"
total_manual_rows="$(jq -r '.completion_summary.manual_signoff_total_rows' "$JSON_OUTPUT")"
overall_completed="$(jq -r '.completion_summary.overall_items_completed' "$JSON_OUTPUT")"
overall_total="$(jq -r '.completion_summary.overall_items_total' "$JSON_OUTPUT")"
overall_remaining="$(jq -r '.completion_summary.overall_items_remaining' "$JSON_OUTPUT")"
overall_percent="$(jq -r '.completion_summary.overall_completion_percent' "$JSON_OUTPUT")"
engineering_completed="$(jq -r '.completion_breakdown.engineering_checks.completed' "$JSON_OUTPUT")"
engineering_total="$(jq -r '.completion_breakdown.engineering_checks.total' "$JSON_OUTPUT")"
engineering_remaining="$(jq -r '.completion_breakdown.engineering_checks.remaining' "$JSON_OUTPUT")"
engineering_percent="$(jq -r '.completion_breakdown.engineering_checks.completion_percent' "$JSON_OUTPUT")"
engineering_status="$(jq -r '.completion_breakdown.engineering_checks.status_marker' "$JSON_OUTPUT")"
manual_completed="$(jq -r '.completion_breakdown.manual_signoff.completed' "$JSON_OUTPUT")"
manual_total="$(jq -r '.completion_breakdown.manual_signoff.total' "$JSON_OUTPUT")"
manual_remaining="$(jq -r '.completion_breakdown.manual_signoff.remaining' "$JSON_OUTPUT")"
manual_percent="$(jq -r '.completion_breakdown.manual_signoff.completion_percent' "$JSON_OUTPUT")"
manual_status="$(jq -r '.completion_breakdown.manual_signoff.status_marker' "$JSON_OUTPUT")"
handoff_completed="$(jq -r '.completion_breakdown.overall_handoff.completed' "$JSON_OUTPUT")"
handoff_total="$(jq -r '.completion_breakdown.overall_handoff.total' "$JSON_OUTPUT")"
handoff_remaining="$(jq -r '.completion_breakdown.overall_handoff.remaining' "$JSON_OUTPUT")"
handoff_percent="$(jq -r '.completion_breakdown.overall_handoff.completion_percent' "$JSON_OUTPUT")"
handoff_status="$(jq -r '.completion_breakdown.overall_handoff.status_marker' "$JSON_OUTPUT")"
automated_blocker_status="$(jq -r '.release_blockers[0].status' "$JSON_OUTPUT")"
automated_blocker_count="$(jq -r '.release_blockers[0].count' "$JSON_OUTPUT")"
non_manual_blocker_status="$(jq -r '.release_blockers[1].status' "$JSON_OUTPUT")"
non_manual_blocker_count="$(jq -r '.release_blockers[1].count' "$JSON_OUTPUT")"
manual_blocker_status="$(jq -r '.release_blockers[2].status' "$JSON_OUTPUT")"
manual_blocker_count="$(jq -r '.release_blockers[2].count' "$JSON_OUTPUT")"
final_acceptance_complete="$(jq -r '.final_acceptance_complete' "$JSON_OUTPUT")"
signoff_allowed="$(jq -r '.final_flags.signoff_allowed' "$JSON_OUTPUT")"
development_release_allowed="$(jq -r '.final_flags.development_release_allowed' "$JSON_OUTPUT")"
manifest_file_count="$(jq -r '.manifest_file_count' "$JSON_OUTPUT")"
release_mode="$(jq -r '.release_gate.mode' "$JSON_OUTPUT")"
release_allowed="$(jq -r '.release_gate.release_allowed' "$JSON_OUTPUT")"
release_reason="$(jq -r '.release_gate.reason' "$JSON_OUTPUT")"
manual_runbook="$(jq -r '.review_surfaces.manual_runbook' "$JSON_OUTPUT")"
signoff_dashboard="$(jq -r '.review_surfaces.signoff_dashboard' "$JSON_OUTPUT")"
user_acceptance_packet="$(jq -r '.review_surfaces.user_acceptance_packet' "$JSON_OUTPUT")"
next_key="$(jq -r '.next_manual_signoff.key' "$JSON_OUTPUT")"
next_item="$(jq -r '.next_manual_signoff.item' "$JSON_OUTPUT")"
next_status="$(jq -r '.next_manual_signoff.status' "$JSON_OUTPUT")"
next_actionable="$(jq -r '.next_manual_signoff.actionable' "$JSON_OUTPUT")"
next_automated_evidence="$(jq -r '.next_manual_signoff.automated_evidence' "$JSON_OUTPUT")"
next_reviewer_check="$(jq -r '.next_manual_signoff.reviewer_check' "$JSON_OUTPUT")"
next_suggested_evidence_note="$(jq -r '.next_manual_signoff.suggested_evidence_note' "$JSON_OUTPUT")"
next_command="$(jq -r '.next_manual_signoff.recorder_command' "$JSON_OUTPUT")"
action_review_status_enabled="$(jq -r '.next_actions[0].enabled | if . then "enabled" else "disabled" end' "$JSON_OUTPUT")"
action_review_status_command="$(jq -r '.next_actions[0].command' "$JSON_OUTPUT")"
action_review_next_enabled="$(jq -r '.next_actions[1].enabled | if . then "enabled" else "disabled" end' "$JSON_OUTPUT")"
action_review_next_command="$(jq -r '.next_actions[1].command' "$JSON_OUTPUT")"
action_record_next_enabled="$(jq -r '.next_actions[2].enabled | if . then "enabled" else "disabled" end' "$JSON_OUTPUT")"
action_record_next_command="$(jq -r '.next_actions[2].command' "$JSON_OUTPUT")"
action_verify_signoff_enabled="$(jq -r '.next_actions[3].enabled | if . then "enabled" else "disabled" end' "$JSON_OUTPUT")"
action_verify_signoff_command="$(jq -r '.next_actions[3].command' "$JSON_OUTPUT")"
action_finalize_enabled="$(jq -r '.next_actions[4].enabled | if . then "enabled" else "disabled" end' "$JSON_OUTPUT")"
action_finalize_command="$(jq -r '.next_actions[4].command' "$JSON_OUTPUT")"

contains "text output has title" "OpenPR universal forms delivery status"
contains "text output mirrors stage" "  stage: $stage"
contains "text output mirrors automated check counts" "  automated checks: $automated_checks total, $failed_checks failed"
contains "text output mirrors non-manual unresolved count" "  non-manual unresolved items: $non_manual_unresolved"
contains "text output mirrors manual pending rows" "  manual signoff pending rows: $pending_rows"
contains "text output mirrors manual accepted rows" "  manual signoff accepted rows: $accepted_rows of $total_manual_rows"
contains "text output mirrors overall completed items" "  overall completed items: $overall_completed of $overall_total"
contains "text output mirrors overall remaining items" "  overall remaining items: $overall_remaining"
contains "text output mirrors overall completion percent" "  overall completion percent: $overall_percent%"
contains "text output mirrors engineering progress breakdown" "  engineering progress: $engineering_completed of $engineering_total, remaining $engineering_remaining, $engineering_percent%, status $engineering_status"
contains "text output mirrors manual signoff progress breakdown" "  manual signoff progress: $manual_completed of $manual_total, remaining $manual_remaining, $manual_percent%, status $manual_status"
contains "text output mirrors overall handoff progress breakdown" "  overall handoff progress: $handoff_completed of $handoff_total, remaining $handoff_remaining, $handoff_percent%, status $handoff_status"
contains "text output mirrors automated release blocker" "  release blocker automated checks: $automated_blocker_status, count $automated_blocker_count"
contains "text output mirrors non-manual release blocker" "  release blocker non-manual unresolved: $non_manual_blocker_status, count $non_manual_blocker_count"
contains "text output mirrors manual signoff release blocker" "  release blocker manual signoff: $manual_blocker_status, count $manual_blocker_count"
contains "text output mirrors final acceptance flag" "  final acceptance complete: $final_acceptance_complete"
contains "text output mirrors signoff final flag" "  signoff final allowed: $signoff_allowed"
contains "text output mirrors development release flag" "  development final release allowed: $development_release_allowed"
contains "text output mirrors manifest file count" "  delivery manifest files: $manifest_file_count"
contains "text output mirrors release gate mode" "  release gate mode: $release_mode"
contains "text output mirrors release allowed flag" "  release allowed: $release_allowed"
contains "text output mirrors release reason" "  release reason: $release_reason"
contains "text output mirrors manual runbook path" "  manual runbook: $manual_runbook"
contains "text output mirrors signoff dashboard path" "  signoff dashboard: $signoff_dashboard"
contains "text output mirrors user acceptance packet path" "  user acceptance packet: $user_acceptance_packet"
contains "text output mirrors review status next action" "  next action review status: $action_review_status_enabled, command $action_review_status_command"
contains "text output mirrors review next signoff action" "  next action review next signoff: $action_review_next_enabled, command $action_review_next_command"
contains "text output mirrors record next signoff action" "  next action record next signoff: $action_record_next_enabled, command $action_record_next_command"
contains "text output mirrors verify manual signoff action" "  next action verify manual signoff: $action_verify_signoff_enabled, command $action_verify_signoff_command"
contains "text output mirrors finalize acceptance action" "  next action finalize acceptance: $action_finalize_enabled, command $action_finalize_command"
not_contains "text output does not leak JSON nulls" "null"

if [[ "$pending_rows" -gt 0 ]]; then
  contains "text output mirrors next manual key" "  next manual signoff key: $next_key"
  contains "text output mirrors next manual item" "  next manual signoff item: $next_item"
  contains "text output mirrors next manual status" "  next manual signoff status: $next_status"
  contains "text output mirrors next manual actionable flag" "  next manual signoff actionable: $next_actionable"
  contains "text output mirrors next manual automated evidence" "  next manual signoff automated evidence: $next_automated_evidence"
  contains "text output mirrors next manual reviewer check" "  next manual signoff reviewer check: $next_reviewer_check"
  contains "text output mirrors next manual suggested evidence" "  next manual signoff suggested evidence: $next_suggested_evidence_note"
  contains "text output includes recorder command heading" "Next recorder command after reviewer approval:"
  contains "text output mirrors recorder command" "$next_command"
else
  not_contains "finalized text output omits next manual key" "next manual signoff key:"
  not_contains "finalized text output omits recorder command heading" "Next recorder command after reviewer approval:"
fi

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms delivery status output smoke failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms delivery status output smoke passed.\n'
