#!/usr/bin/env bash
set -euo pipefail

REPORT_DIR="/opt/worker/report/openpr/docs"
ARTIFACT_DIR="${OPENPR_UI_ARTIFACT_DIR:-/opt/worker/report/openpr/artifacts/universal-forms-ui-2026-05-31}"
SIGNOFF_STATUS_JSON_PATH="${OPENPR_SIGNOFF_STATUS_JSON:-$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json}"
DASHBOARD_PATH="${OPENPR_SIGNOFF_DASHBOARD:-$REPORT_DIR/openpr-universal-form-signoff-dashboard-2026-05-31.html}"
RENDER_DIR="${OPENPR_SIGNOFF_DASHBOARD_RENDER_DIR:-$ARTIFACT_DIR/signoff-dashboard}"
CHROMIUM_BIN="${CHROMIUM_BIN:-/usr/bin/chromium}"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-signoff-dashboard-render.sh [--status-json PATH] [--dashboard PATH] [--render-dir PATH]

Renders the universal forms manual signoff dashboard in headless Chromium,
verifies that the reviewer queue and primary screenshots load, and captures
desktop/mobile render screenshots for reviewer evidence.

Environment:
  OPENPR_SIGNOFF_STATUS_JSON           Optional signoff status JSON path.
  OPENPR_SIGNOFF_DASHBOARD             Optional HTML dashboard path.
  OPENPR_SIGNOFF_DASHBOARD_RENDER_DIR  Optional rendered screenshot directory.
  CHROMIUM_BIN                         Optional Chromium binary path.
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
    --render-dir)
      RENDER_DIR="${2:-}"
      if [[ -z "$RENDER_DIR" ]]; then
        echo "--render-dir requires a path" >&2
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

if [[ ! -x "$CHROMIUM_BIN" ]]; then
  echo "Chromium binary not found or not executable: $CHROMIUM_BIN" >&2
  exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to verify the rendered signoff dashboard" >&2
  exit 2
fi
if [[ ! -s "$SIGNOFF_STATUS_JSON_PATH" ]]; then
  echo "Signoff status JSON not found: $SIGNOFF_STATUS_JSON_PATH" >&2
  exit 1
fi
if [[ ! -f "$DASHBOARD_PATH" ]]; then
  echo "Signoff dashboard not found: $DASHBOARD_PATH" >&2
  exit 1
fi

mkdir -p "$RENDER_DIR"

dashboard_url="file://$DASHBOARD_PATH"
desktop_png="$RENDER_DIR/signoff-dashboard-desktop.png"
mobile_png="$RENDER_DIR/signoff-dashboard-mobile.png"
desktop_dom="$RENDER_DIR/signoff-dashboard-desktop.dom.html"
mobile_dom="$RENDER_DIR/signoff-dashboard-mobile.dom.html"
expected_queue_count="$(jq -r '.manual_signoff.pending_queue | length' "$SIGNOFF_STATUS_JSON_PATH")"
expected_next_key="$(jq -r '.manual_signoff.next_row.key // ""' "$SIGNOFF_STATUS_JSON_PATH")"
expected_pending_count="$(jq -r '.manual_signoff.pending_rows' "$SIGNOFF_STATUS_JSON_PATH")"

run_chromium() {
  local window_size="$1"
  local screenshot="$2"
  local dom_output="$3"
  local stderr_path
  stderr_path="$(mktemp /tmp/openpr-uf-signoff-dashboard-render.stderr.XXXXXX)"

  if "$CHROMIUM_BIN" \
    --headless=new \
    --disable-gpu \
    --disable-dev-shm-usage \
    --no-sandbox \
    --hide-scrollbars \
    --run-all-compositor-stages-before-draw \
    "--window-size=$window_size" \
    --virtual-time-budget=5000 \
    "--screenshot=$screenshot" \
    --dump-dom \
    "$dashboard_url" >"$dom_output" 2>"$stderr_path"; then
    rm -f "$stderr_path"
    return 0
  fi

  printf 'Chromium render failed for %s\n' "$screenshot" >&2
  if [[ -s "$stderr_path" ]]; then
    printf '  stderr:\n' >&2
    sed 's/^/    /' "$stderr_path" >&2
  fi
  rm -f "$stderr_path"
  return 1
}

run_chromium "1366,1200" "$desktop_png" "$desktop_dom"
run_chromium "390,844" "$mobile_png" "$mobile_dom"

for path in "$desktop_png" "$mobile_png" "$desktop_dom" "$mobile_dom"; do
  if [[ ! -s "$path" ]]; then
    echo "Missing rendered signoff dashboard artifact: $path" >&2
    exit 1
  fi
done

for dom in "$desktop_dom" "$mobile_dom"; do
  if ! rg -q --fixed-strings "data-signoff-dashboard-queue-count=\"$expected_queue_count\"" "$dom"; then
    echo "Rendered signoff dashboard did not expose expected queued reviewer rows ($expected_queue_count): $dom" >&2
    exit 1
  fi
  if ! rg -q --fixed-strings "data-next-signoff-key=\"$expected_next_key\"" "$dom"; then
    echo "Rendered signoff dashboard did not expose expected next reviewer key ($expected_next_key): $dom" >&2
    exit 1
  fi
  if ! rg -q --fixed-strings "data-signoff-start-key=\"$expected_next_key\"" "$dom"; then
    echo "Rendered signoff dashboard Start Here section did not expose expected next reviewer key ($expected_next_key): $dom" >&2
    exit 1
  fi
  if ! rg -q --fixed-strings 'Start Here' "$dom"; then
    echo "Rendered signoff dashboard did not expose Start Here handoff text: $dom" >&2
    exit 1
  fi
  if [[ "$expected_pending_count" == "0" ]]; then
    if ! rg -q --fixed-strings 'Finalization Action' "$dom"; then
      echo "Rendered signoff dashboard did not expose finalization guidance: $dom" >&2
      exit 1
    fi
    if ! rg -q --fixed-strings 'scripts/finalize-universal-forms-acceptance.sh' "$dom"; then
      echo "Rendered signoff dashboard did not expose finalizer command: $dom" >&2
      exit 1
    fi
  fi
  if ! rg -q --fixed-strings 'data-dashboard-image-count="3"' "$dom"; then
    echo "Rendered signoff dashboard did not expose 3 image slots: $dom" >&2
    exit 1
  fi
  if ! rg -q --fixed-strings 'data-dashboard-loaded-images="3"' "$dom"; then
    echo "Rendered signoff dashboard did not load all 3 primary images: $dom" >&2
    exit 1
  fi
done

desktop_desc="$(file "$desktop_png")"
mobile_desc="$(file "$mobile_png")"
if [[ "$desktop_desc" != *"PNG image data, 1366 x 1200,"* ]]; then
  echo "Unexpected desktop render dimensions: $desktop_desc" >&2
  exit 1
fi
if [[ "$mobile_desc" != *"PNG image data, 390 x 844,"* ]]; then
  echo "Unexpected mobile render dimensions: $mobile_desc" >&2
  exit 1
fi

printf 'Signoff dashboard browser render smoke passed\n'
printf '  desktop: %s\n' "$desktop_png"
printf '  mobile: %s\n' "$mobile_png"
