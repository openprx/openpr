#!/usr/bin/env bash
set -euo pipefail

REPORT_DIR="/opt/worker/report/openpr/docs"
ARTIFACT_DIR="${OPENPR_UI_ARTIFACT_DIR:-/opt/worker/report/openpr/artifacts/universal-forms-ui-2026-05-31}"
GALLERY_PATH="${OPENPR_UI_REVIEW_GALLERY:-$REPORT_DIR/openpr-universal-form-ui-review-gallery-2026-05-31.html}"
RENDER_DIR="${OPENPR_UI_REVIEW_GALLERY_RENDER_DIR:-$ARTIFACT_DIR/ui-review-gallery}"
CHROMIUM_BIN="${CHROMIUM_BIN:-/usr/bin/chromium}"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-ui-review-gallery-render.sh [--gallery PATH] [--render-dir PATH]

Renders the universal forms UI review gallery in headless Chromium, verifies
that all eight gallery screenshots load, and captures desktop/mobile render
screenshots for reviewer evidence.

Environment:
  OPENPR_UI_REVIEW_GALLERY             Optional HTML gallery path.
  OPENPR_UI_REVIEW_GALLERY_RENDER_DIR  Optional rendered screenshot directory.
  CHROMIUM_BIN                         Optional Chromium binary path.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --gallery)
      GALLERY_PATH="${2:-}"
      if [[ -z "$GALLERY_PATH" ]]; then
        echo "--gallery requires a path" >&2
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
if [[ ! -f "$GALLERY_PATH" ]]; then
  echo "UI review gallery not found: $GALLERY_PATH" >&2
  exit 1
fi

mkdir -p "$RENDER_DIR"

gallery_url="file://$GALLERY_PATH"
desktop_png="$RENDER_DIR/ui-review-gallery-desktop.png"
mobile_png="$RENDER_DIR/ui-review-gallery-mobile.png"
desktop_dom="$RENDER_DIR/ui-review-gallery-desktop.dom.html"
mobile_dom="$RENDER_DIR/ui-review-gallery-mobile.dom.html"

run_chromium() {
  local window_size="$1"
  local screenshot="$2"
  local dom_output="$3"
  local stderr_path
  stderr_path="$(mktemp /tmp/openpr-uf-ui-review-gallery-render.stderr.XXXXXX)"

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
    "$gallery_url" >"$dom_output" 2>"$stderr_path"; then
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
    echo "Missing rendered gallery artifact: $path" >&2
    exit 1
  fi
done

if ! rg -q --fixed-strings 'data-gallery-image-count="8"' "$desktop_dom"; then
  echo "Desktop gallery render did not expose 8 image slots" >&2
  exit 1
fi
if ! rg -q --fixed-strings 'data-gallery-loaded-images="8"' "$desktop_dom"; then
  echo "Desktop gallery render did not load all 8 images" >&2
  exit 1
fi
if ! rg -q --fixed-strings 'data-gallery-image-count="8"' "$mobile_dom"; then
  echo "Mobile gallery render did not expose 8 image slots" >&2
  exit 1
fi
if ! rg -q --fixed-strings 'data-gallery-loaded-images="8"' "$mobile_dom"; then
  echo "Mobile gallery render did not load all 8 images" >&2
  exit 1
fi

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

printf 'UI review gallery browser render smoke passed\n'
printf '  desktop: %s\n' "$desktop_png"
printf '  mobile: %s\n' "$mobile_png"
