#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERIFY="$ROOT_DIR/scripts/verify-universal-forms-delivery-status-json.sh"
JSON_PATH="${1:-}"
GENERATED_TMP=""

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-delivery-status-json-contract.sh [JSON_PATH]

Runs negative contract checks for the compact delivery status JSON verifier.
When JSON_PATH is omitted, the canonical JSON is generated from
scripts/status-universal-forms-delivery.sh --json.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

cleanup() {
  if [[ -n "$GENERATED_TMP" ]]; then
    rm -f "$GENERATED_TMP"
  fi
}
trap cleanup EXIT

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for delivery status JSON contract smoke" >&2
  exit 2
fi

if [[ -z "$JSON_PATH" ]]; then
  GENERATED_TMP="$(mktemp /tmp/openpr-uf-delivery-status.XXXXXX.json)"
  "$ROOT_DIR/scripts/status-universal-forms-delivery.sh" --json >"$GENERATED_TMP"
  JSON_PATH="$GENERATED_TMP"
fi

if [[ ! -f "$JSON_PATH" ]]; then
  echo "Delivery status JSON not found: $JSON_PATH" >&2
  exit 2
fi

pass() {
  printf 'PASS: %s\n' "$1"
}

expect_reject() {
  local description="$1"
  local filter="$2"
  local tmp
  tmp="$(mktemp /tmp/openpr-uf-delivery-status-bad.XXXXXX.json)"
  jq "$filter" "$JSON_PATH" >"$tmp"
  if "$VERIFY" "$tmp" >/dev/null 2>&1; then
    echo "FAIL: $description was accepted" >&2
    rm -f "$tmp"
    exit 1
  fi
  rm -f "$tmp"
  pass "$description is rejected by verifier"
}

printf 'Universal forms delivery status JSON contract smoke\n'
printf '  JSON: %s\n' "$JSON_PATH"
printf '\n'

"$VERIFY" "$JSON_PATH" >/dev/null
pass "canonical delivery status JSON passes verifier"

expect_reject "missing schema path" 'del(.schema_path)'
expect_reject "missing verified prerequisites" 'del(.verified_prerequisites)'
expect_reject "missing reports required key" 'del(.reports.readiness_json)'
expect_reject "missing review surfaces required key" 'del(.review_surfaces.manual_runbook)'
expect_reject "missing completion summary required key" 'del(.completion_summary.delivery_state)'
expect_reject "missing completion breakdown required key" 'del(.completion_breakdown.manual_signoff)'
expect_reject "missing release blockers" 'del(.release_blockers)'
expect_reject "missing next manual signoff required key" 'del(.next_manual_signoff.recorder_command)'
expect_reject "missing next manual reviewer check" 'del(.next_manual_signoff.reviewer_check)'
expect_reject "missing next actions" 'del(.next_actions)'
expect_reject "missing next action required key" 'del(.next_actions[0].command)'
expect_reject "missing manual signoff queue" 'del(.manual_signoff_queue)'
expect_reject "missing final flags required key" 'del(.final_flags.signoff_allowed)'
expect_reject "missing release gate required key" 'del(.release_gate.reason)'
expect_reject "extra top-level property" '.unexpected = true'
expect_reject "extra verified prerequisite" '.verified_prerequisites += ["unexpected"]'
expect_reject "verified prerequisite order drift" '.verified_prerequisites[0] = "development_status_json"'
expect_reject "extra reports property" '.reports.unexpected = true'
expect_reject "extra review surfaces property" '.review_surfaces.unexpected = true'
expect_reject "extra completion summary property" '.completion_summary.unexpected = true'
expect_reject "extra completion breakdown property" '.completion_breakdown.unexpected = true'
expect_reject "extra completion breakdown row property" '.completion_breakdown.manual_signoff.unexpected = true'
expect_reject "extra release blocker property" '.release_blockers[0].unexpected = true'
expect_reject "extra release blocker row" '.release_blockers += [.release_blockers[0]]'
expect_reject "extra next manual signoff property" '.next_manual_signoff.unexpected = true'
expect_reject "extra next action property" '.next_actions[0].unexpected = true'
expect_reject "extra next action row" '.next_actions += [.next_actions[0]]'
expect_reject "extra manual signoff queue row property" '.manual_signoff_queue[0].unexpected = true'
expect_reject "extra final flags property" '.final_flags.unexpected = true'
expect_reject "extra release gate property" '.release_gate.unexpected = true'
expect_reject "string automated check counter" '.automated_checks = "27"'
expect_reject "completion summary engineering completed drift" '.completion_summary.engineering_checks_completed -= 1'
expect_reject "completion summary manual total drift" '.completion_summary.manual_signoff_total_rows += 1'
expect_reject "completion summary overall remaining drift" '.completion_summary.overall_items_remaining += 1'
expect_reject "completion summary overall percent drift" '.completion_summary.overall_completion_percent = 100'
expect_reject "completion breakdown engineering completed drift" '.completion_breakdown.engineering_checks.completed -= 1'
expect_reject "completion breakdown manual remaining drift" '.completion_breakdown.manual_signoff.remaining -= 1'
expect_reject "completion breakdown overall status drift" '.completion_breakdown.overall_handoff.status_marker = "已验收"'
expect_reject "completion breakdown manual percent drift" '.completion_breakdown.manual_signoff.completion_percent = 100'
expect_reject "unknown completion breakdown status" '.completion_breakdown.manual_signoff.status_marker = "unknown"'
expect_reject "release blocker count drift" '.release_blockers[2].count -= 1'
expect_reject "release blocker status drift" '.release_blockers[2].status = "clear"'
expect_reject "release blocker flag drift" '.release_blockers[2].blocking = false'
expect_reject "release blocker order drift" '(.release_blockers[0]) as $first | (.release_blockers[1]) as $second | .release_blockers[0] = $second | .release_blockers[1] = $first'
expect_reject "release blocker evidence drift" '.release_blockers[2].evidence = "/tmp/wrong-runbook.md"'
expect_reject "unknown release blocker status" '.release_blockers[0].status = "unknown"'
expect_reject "next action order drift" '(.next_actions[0]) as $first | (.next_actions[1]) as $second | .next_actions[0] = $second | .next_actions[1] = $first'
expect_reject "next action enabled drift" '.next_actions[1].enabled = false'
expect_reject "next action command drift" '.next_actions[2].command = "wrong command"'
expect_reject "next action blocked_by drift" '.next_actions[3].blocked_by = []'
expect_reject "unknown next action key" '.next_actions[0].key = "unknown"'
expect_reject "review surface path drift" '.review_surfaces.signoff_dashboard = "/tmp/wrong-dashboard.html"'
expect_reject "completion summary delivery state drift" '.completion_summary.delivery_state = "ready_for_release"'
expect_reject "unknown completion summary delivery state" '.completion_summary.delivery_state = "unknown"'
expect_reject "manual pending count drift" '.manual_signoff_pending_rows += 1'
expect_reject "manual signoff queue deletion" 'del(.manual_signoff_queue[0])'
expect_reject "manual signoff queue order drift" '(.manual_signoff_queue[0]) as $first | (.manual_signoff_queue[1]) as $second | .manual_signoff_queue[0] = $second | .manual_signoff_queue[1] = $first'
expect_reject "manual signoff queue next marker drift" '.manual_signoff_queue[0].is_next = false'
expect_reject "manual signoff queue actionable drift" '.manual_signoff_queue[0].actionable = false'
expect_reject "manual signoff queue reviewer check drift" '.manual_signoff_queue[0].reviewer_check = "wrong reviewer check"'
expect_reject "next manual signoff key drift" '.next_manual_signoff.key = "overall"'
expect_reject "next manual reviewer check drift" '.next_manual_signoff.reviewer_check = "wrong reviewer check"'
expect_reject "next manual automated evidence drift" '.next_manual_signoff.automated_evidence = "wrong evidence"'
expect_reject "next manual actionable drift" '.next_manual_signoff.actionable = false'
expect_reject "unknown next manual signoff key" '.next_manual_signoff.key = "unknown"'
expect_reject "unknown next manual status" '.next_manual_signoff.status = "waiting"'
expect_reject "manifest file count drift" '.manifest_file_count += 1'
expect_reject "signoff final flag drift" '.final_flags.signoff_allowed = true'
expect_reject "unknown release gate mode" '.release_gate.mode = "ready"'
expect_reject "release gate allowed flag drift" '.release_gate.release_allowed = true'

printf '\nUniversal forms delivery status JSON contract smoke passed.\n'
