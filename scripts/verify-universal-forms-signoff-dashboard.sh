#!/usr/bin/env bash
set -euo pipefail

REPORT_DIR="/opt/worker/report/openpr/docs"
ARTIFACT_DIR="${OPENPR_UI_ARTIFACT_DIR:-/opt/worker/report/openpr/artifacts/universal-forms-ui-2026-05-31}"
SIGNOFF_STATUS_JSON_PATH="${OPENPR_SIGNOFF_STATUS_JSON:-$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json}"
DASHBOARD_PATH="${OPENPR_SIGNOFF_DASHBOARD:-$REPORT_DIR/openpr-universal-form-signoff-dashboard-2026-05-31.html}"
RENDER_DIR="${OPENPR_SIGNOFF_DASHBOARD_RENDER_DIR:-$ARTIFACT_DIR/signoff-dashboard}"

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-signoff-dashboard.sh [--status-json PATH] [--dashboard PATH]

Verifies that the universal forms signoff dashboard mirrors the current
manual signoff queue and references the core reviewer evidence.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --status-json)
      SIGNOFF_STATUS_JSON_PATH="${2:-}"
      if [[ -z "$SIGNOFF_STATUS_JSON_PATH" ]]; then
        echo "--status-json requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --dashboard)
      DASHBOARD_PATH="${2:-}"
      if [[ -z "$DASHBOARD_PATH" ]]; then
        echo "--dashboard requires a path" >&2
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

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to verify the signoff dashboard" >&2
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

check() {
  local description="$1"
  shift
  if "$@"; then
    pass "$description"
  else
    fail "$description"
  fi
}

file_exists() {
  [[ -f "$1" ]]
}

non_empty_file() {
  [[ -s "$1" ]]
}

dashboard_contains() {
  local needle="$1"
  rg -q --fixed-strings -- "$needle" "$DASHBOARD_PATH"
}

html_escape() {
  jq -Rr @html <<<"$1"
}

printf 'Universal forms signoff dashboard verification\n'
printf '  dashboard: %s\n' "$DASHBOARD_PATH"
printf '  status JSON: %s\n' "$SIGNOFF_STATUS_JSON_PATH"
printf '\n'

check "dashboard exists" file_exists "$DASHBOARD_PATH"
check "dashboard is non-empty" non_empty_file "$DASHBOARD_PATH"
check "signoff status JSON exists" file_exists "$SIGNOFF_STATUS_JSON_PATH"
check "signoff status JSON is non-empty" non_empty_file "$SIGNOFF_STATUS_JSON_PATH"

queue_count="$(jq -r '.manual_signoff.pending_queue | length' "$SIGNOFF_STATUS_JSON_PATH")"
next_key="$(jq -r '.manual_signoff.next_row.key' "$SIGNOFF_STATUS_JSON_PATH")"
next_command="$(jq -r '.manual_signoff.next_row.recorder_command // ""' "$SIGNOFF_STATUS_JSON_PATH")"
pending_count="$(jq -r '.manual_signoff.pending_rows' "$SIGNOFF_STATUS_JSON_PATH")"
final_signoff_complete="$(jq -r '.manual_signoff.final_signoff_complete' "$SIGNOFF_STATUS_JSON_PATH")"

check "dashboard has title" dashboard_contains "OpenPR Universal Forms Signoff Dashboard"
check "dashboard pins queue count" dashboard_contains "data-signoff-dashboard-queue-count=\"$queue_count\""
check "dashboard pins next key" dashboard_contains "data-next-signoff-key=\"$next_key\""
check "dashboard pins final signoff flag" dashboard_contains "data-final-signoff-complete=\"$final_signoff_complete\""
check "dashboard references acceptance packet" dashboard_contains "openpr-universal-form-user-acceptance-packet-2026-05-31.md"
check "dashboard references manual evidence map" dashboard_contains "openpr-universal-form-manual-evidence-map-2026-05-31.md"
check "dashboard references UI review gallery" dashboard_contains "openpr-universal-form-ui-review-gallery-2026-05-31.html"
check "dashboard references manual runbook" dashboard_contains "openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
check "dashboard references delivery manifest" dashboard_contains "openpr-universal-form-delivery-manifest-2026-05-31.md"
check "dashboard exposes Start Here handoff section" dashboard_contains "Start Here"
check "dashboard pins Start Here next key" dashboard_contains "data-signoff-start-key=\"$next_key\""
if [[ "$pending_count" == "0" ]]; then
  check "dashboard exposes finalization action" dashboard_contains "Finalization Action"
  check "dashboard exposes finalizer command" dashboard_contains "scripts/finalize-universal-forms-acceptance.sh"
  check "dashboard exposes strict delivery audit command" dashboard_contains "scripts/audit-universal-forms-delivery-state.sh --strict"
  check "dashboard exposes delivery bundle audit command" dashboard_contains "scripts/audit-universal-forms-delivery-bundle.sh"
else
  check "dashboard exposes delivery status command" dashboard_contains "scripts/status-universal-forms-delivery.sh"
  check "dashboard exposes next recorder command in Start Here" dashboard_contains "$(html_escape "$next_command")"
fi
check "dashboard exposes browser image count marker" dashboard_contains "data-dashboard-image-count"
check "dashboard exposes browser loaded image marker" dashboard_contains "data-dashboard-loaded-images"

for path in \
  "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-desktop.png" \
  "$ARTIFACT_DIR/forms-ui/forms-ui-desktop.png" \
  "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-desktop.png"; do
  check "screenshot input exists: $path" file_exists "$path"
  check "dashboard references screenshot: $path" dashboard_contains "$path"
done

render_desktop="$RENDER_DIR/signoff-dashboard-desktop.png"
render_mobile="$RENDER_DIR/signoff-dashboard-mobile.png"
if [[ -f "$render_desktop" || -f "$render_mobile" ]]; then
  check "rendered desktop dashboard screenshot exists" file_exists "$render_desktop"
  check "rendered desktop dashboard screenshot is non-empty" non_empty_file "$render_desktop"
  check "rendered mobile dashboard screenshot exists" file_exists "$render_mobile"
  check "rendered mobile dashboard screenshot is non-empty" non_empty_file "$render_mobile"
fi

while IFS=$'\t' read -r key item command; do
  [[ -z "$key" ]] && continue
  check "dashboard includes queue key: $key" dashboard_contains "data-signoff-key=\"$key\""
  check "dashboard includes queue item: $item" dashboard_contains "$item"
  check "dashboard includes recorder command for: $key" dashboard_contains "$(html_escape "$command")"
done < <(
  jq -r '
    .manual_signoff.pending_queue[]
    | [.key, .item, .recorder_command]
    | @tsv
  ' "$SIGNOFF_STATUS_JSON_PATH"
)

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms signoff dashboard verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms signoff dashboard verification passed.\n'
