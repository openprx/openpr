#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
MANUAL_EVIDENCE_MAP_PATH="$REPORT_DIR/openpr-universal-form-manual-evidence-map-2026-05-31.md"
SIGNOFF_STATUS_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.md"
SIGNOFF_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json"
SIGNOFF_DASHBOARD_PATH="$REPORT_DIR/openpr-universal-form-signoff-dashboard-2026-05-31.html"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-signoff-dashboard-progression.sh

Verifies the manual signoff dashboard across the reviewer journey on temporary
runbook/evidence copies only. The smoke renders and verifies the dashboard for
0/7 accepted, after restaurant_template is accepted, and after all seven manual
rows are accepted. Official handoff files are never modified.
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

require_file() {
  local path="$1"
  if [[ -f "$path" ]]; then
    pass "file exists: $path"
  else
    fail "file missing: $path"
  fi
}

require_command() {
  local name="$1"
  if command -v "$name" >/dev/null 2>&1; then
    pass "$name is available"
  else
    fail "$name is available"
  fi
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

json_value() {
  local path="$1"
  local expression="$2"
  jq -r "$expression" "$path"
}

run_quiet() {
  local label="$1"
  shift
  local stdout_path stderr_path
  stdout_path="$(mktemp "$tmp_dir/${label//[^A-Za-z0-9_]/_}.stdout.XXXXXX")"
  stderr_path="$(mktemp "$tmp_dir/${label//[^A-Za-z0-9_]/_}.stderr.XXXXXX")"
  if "$@" >"$stdout_path" 2>"$stderr_path"; then
    rm -f "$stdout_path" "$stderr_path"
    return 0
  fi
  fail "$label failed"
  if [[ -s "$stdout_path" ]]; then
    printf '  stdout:\n' >&2
    sed 's/^/    /' "$stdout_path" >&2
  fi
  if [[ -s "$stderr_path" ]]; then
    printf '  stderr:\n' >&2
    sed 's/^/    /' "$stderr_path" >&2
  fi
  rm -f "$stdout_path" "$stderr_path"
  return 1
}

record_status() {
  local tmp_runbook="$1"
  local tmp_evidence="$2"
  local key="$3"
  local status="$4"
  local evidence_note="${5:-temporary dashboard progression smoke accepted $key}"

  if [[ "$status" == "pending" ]]; then
    run_quiet "record $key pending" \
      "$ROOT_DIR/scripts/record-universal-forms-manual-signoff.sh" \
      --runbook "$tmp_runbook" \
      --report "$tmp_evidence" \
      --item "$key" \
      --status pending
  else
    run_quiet "record $key accepted" \
      "$ROOT_DIR/scripts/record-universal-forms-manual-signoff.sh" \
      --runbook "$tmp_runbook" \
      --report "$tmp_evidence" \
      --item "$key" \
      --status accepted \
      --reviewer "Dashboard Progression Smoke" \
      --evidence "$evidence_note"
  fi
}

generate_status_and_dashboard() {
  local tmp_runbook="$1"
  local tmp_evidence="$2"
  local tmp_markdown="$3"
  local tmp_json="$4"
  local tmp_dashboard="$5"
  local render_dir="$6"

  run_quiet "generate signoff markdown" \
    "$ROOT_DIR/scripts/report-universal-forms-signoff-status.sh" \
    --runbook "$tmp_runbook" \
    --report "$tmp_evidence" \
    --manual-map "$MANUAL_EVIDENCE_MAP_PATH" \
    --reviewer "Dashboard Progression Smoke" \
    --output "$tmp_markdown"

  run_quiet "generate signoff JSON" \
    "$ROOT_DIR/scripts/report-universal-forms-signoff-status-json.sh" \
    --runbook "$tmp_runbook" \
    --report "$tmp_evidence" \
    --manual-map "$MANUAL_EVIDENCE_MAP_PATH" \
    --markdown "$tmp_markdown" \
    --reviewer "Dashboard Progression Smoke" \
    --output "$tmp_json"

  run_quiet "generate signoff dashboard" \
    "$ROOT_DIR/scripts/prepare-universal-forms-signoff-dashboard.sh" \
    --status-json "$tmp_json" \
    --output "$tmp_dashboard"

  run_quiet "verify signoff dashboard" \
    env OPENPR_SIGNOFF_DASHBOARD_RENDER_DIR="$render_dir" \
    "$ROOT_DIR/scripts/verify-universal-forms-signoff-dashboard.sh" \
      --status-json "$tmp_json" \
      --dashboard "$tmp_dashboard"

  run_quiet "render signoff dashboard" \
    "$ROOT_DIR/scripts/smoke-universal-forms-signoff-dashboard-render.sh" \
    --status-json "$tmp_json" \
    --dashboard "$tmp_dashboard" \
    --render-dir "$render_dir"
}

verify_dashboard_state() {
  local label="$1"
  local tmp_json="$2"
  local tmp_dashboard="$3"
  local expected_accepted="$4"
  local expected_pending="$5"
  local expected_next="$6"
  local expected_final_allowed="$7"

  equals "$label accepted rows" "$(json_value "$tmp_json" '.manual_signoff.accepted_rows')" "$expected_accepted"
  equals "$label pending rows" "$(json_value "$tmp_json" '.manual_signoff.pending_rows')" "$expected_pending"
  equals "$label queue count" "$(json_value "$tmp_json" '.manual_signoff.pending_queue | length')" "$expected_pending"
  equals "$label next key" "$(json_value "$tmp_json" '.manual_signoff.next_row.key // ""')" "$expected_next"
  equals "$label final signoff allowed" "$(json_value "$tmp_json" '.manual_signoff.final_signoff_allowed')" "$expected_final_allowed"
  contains "$label dashboard exposes Start Here" "$tmp_dashboard" "Start Here"
  contains "$label dashboard pins next/start key" "$tmp_dashboard" "data-signoff-start-key=\"$expected_next\""

  if [[ "$expected_pending" == "0" ]]; then
    contains "$label dashboard exposes finalization action" "$tmp_dashboard" "Finalization Action"
    contains "$label dashboard exposes finalizer command" "$tmp_dashboard" "scripts/finalize-universal-forms-acceptance.sh"
    contains "$label dashboard exposes strict delivery audit command" "$tmp_dashboard" "scripts/audit-universal-forms-delivery-state.sh --strict"
    contains "$label dashboard exposes delivery bundle audit command" "$tmp_dashboard" "scripts/audit-universal-forms-delivery-bundle.sh"
  else
    contains "$label dashboard exposes recorder command" "$tmp_dashboard" "scripts/record-universal-forms-manual-signoff.sh --item $expected_next"
  fi
}

printf 'Universal forms signoff dashboard progression smoke\n'
printf '  runbook: %s\n' "$RUNBOOK_PATH"
printf '  evidence: %s\n' "$EVIDENCE_PATH"
printf '  dashboard: %s\n' "$SIGNOFF_DASHBOARD_PATH"
printf '\n'

require_command jq
require_command sha256sum
require_command rg
for path in \
  "$RUNBOOK_PATH" \
  "$EVIDENCE_PATH" \
  "$MANUAL_EVIDENCE_MAP_PATH" \
  "$SIGNOFF_STATUS_PATH" \
  "$SIGNOFF_STATUS_JSON_PATH" \
  "$SIGNOFF_DASHBOARD_PATH"; do
  require_file "$path"
done

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms signoff dashboard progression smoke failed before dashboard checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

official_paths=(
  "$RUNBOOK_PATH"
  "$EVIDENCE_PATH"
  "$SIGNOFF_STATUS_PATH"
  "$SIGNOFF_STATUS_JSON_PATH"
  "$SIGNOFF_DASHBOARD_PATH"
)
declare -A official_before
for path in "${official_paths[@]}"; do
  official_before["$path"]="$(sha256sum "$path" | awk '{print $1}')"
done

tmp_dir="$(mktemp -d /tmp/openpr-uf-dashboard-progression.XXXXXX)"
trap 'rm -rf "$tmp_dir"' EXIT
tmp_runbook="$tmp_dir/runbook.md"
tmp_evidence="$tmp_dir/evidence.md"
cp "$RUNBOOK_PATH" "$tmp_runbook"
cp "$EVIDENCE_PATH" "$tmp_evidence"

keys=(
  restaurant_template
  frontend_usability
  amounts
  workflow
  hub_consistency
  docs
  overall
)

reset_keys=(
  overall
  docs
  hub_consistency
  workflow
  amounts
  frontend_usability
  restaurant_template
)

for key in "${reset_keys[@]}"; do
  record_status "$tmp_runbook" "$tmp_evidence" "$key" pending
done

initial_markdown="$tmp_dir/signoff-status-initial.md"
initial_json="$tmp_dir/signoff-status-initial.json"
initial_dashboard="$tmp_dir/signoff-dashboard-initial.html"
generate_status_and_dashboard \
  "$tmp_runbook" \
  "$tmp_evidence" \
  "$initial_markdown" \
  "$initial_json" \
  "$initial_dashboard" \
  "$tmp_dir/render-initial"
verify_dashboard_state "initial state" "$initial_json" "$initial_dashboard" "0" "7" "restaurant_template" "false"

first_note="$(json_value "$initial_json" '.manual_signoff.rows[] | select(.key == "restaurant_template") | .suggested_evidence_note')"
record_status "$tmp_runbook" "$tmp_evidence" restaurant_template accepted "$first_note"

after_first_markdown="$tmp_dir/signoff-status-after-first.md"
after_first_json="$tmp_dir/signoff-status-after-first.json"
after_first_dashboard="$tmp_dir/signoff-dashboard-after-first.html"
generate_status_and_dashboard \
  "$tmp_runbook" \
  "$tmp_evidence" \
  "$after_first_markdown" \
  "$after_first_json" \
  "$after_first_dashboard" \
  "$tmp_dir/render-after-first"
verify_dashboard_state "after restaurant_template" "$after_first_json" "$after_first_dashboard" "1" "6" "frontend_usability" "false"

for key in frontend_usability amounts workflow hub_consistency docs overall; do
  note="$(json_value "$after_first_json" ".manual_signoff.rows[] | select(.key == \"$key\") | .suggested_evidence_note")"
  record_status "$tmp_runbook" "$tmp_evidence" "$key" accepted "$note"
  run_quiet "generate progressive signoff JSON" \
    "$ROOT_DIR/scripts/report-universal-forms-signoff-status-json.sh" \
    --runbook "$tmp_runbook" \
    --report "$tmp_evidence" \
    --manual-map "$MANUAL_EVIDENCE_MAP_PATH" \
    --markdown "$tmp_dir/signoff-status-progressive.md" \
    --reviewer "Dashboard Progression Smoke" \
    --output "$tmp_dir/signoff-status-progressive.json"
  after_first_json="$tmp_dir/signoff-status-progressive.json"
done

final_markdown="$tmp_dir/signoff-status-final.md"
final_json="$tmp_dir/signoff-status-final.json"
final_dashboard="$tmp_dir/signoff-dashboard-final.html"
generate_status_and_dashboard \
  "$tmp_runbook" \
  "$tmp_evidence" \
  "$final_markdown" \
  "$final_json" \
  "$final_dashboard" \
  "$tmp_dir/render-final"
verify_dashboard_state "final state" "$final_json" "$final_dashboard" "7" "0" "" "true"

if run_quiet "verify temporary final signoff" \
  "$ROOT_DIR/scripts/verify-universal-forms-acceptance-signoff.sh" \
    "$tmp_evidence" \
    --runbook "$tmp_runbook"; then
  pass "temporary fully accepted signoff passes final verifier"
else
  fail "temporary fully accepted signoff passes final verifier"
fi

for path in "${official_paths[@]}"; do
  official_after="$(sha256sum "$path" | awk '{print $1}')"
  equals "official file remains unchanged: $path" "$official_after" "${official_before[$path]}"
done

if [[ "$failures" -eq 0 ]]; then
  printf '\nUniversal forms signoff dashboard progression smoke passed.\n'
else
  printf '\nUniversal forms signoff dashboard progression smoke failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi
