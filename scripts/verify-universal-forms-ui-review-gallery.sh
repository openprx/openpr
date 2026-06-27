#!/usr/bin/env bash
set -euo pipefail

REPORT_DIR="/opt/worker/report/openpr/docs"
ARTIFACT_DIR="${OPENPR_UI_ARTIFACT_DIR:-/opt/worker/report/openpr/artifacts/universal-forms-ui-2026-05-31}"
GALLERY_PATH="${OPENPR_UI_REVIEW_GALLERY:-$REPORT_DIR/openpr-universal-form-ui-review-gallery-2026-05-31.html}"
RENDER_DIR="${OPENPR_UI_REVIEW_GALLERY_RENDER_DIR:-$ARTIFACT_DIR/ui-review-gallery}"

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-ui-review-gallery.sh [--artifact-dir PATH] [--gallery PATH]

Verifies that the universal forms UI review gallery exists and references all
required reviewer screenshots and smoke logs.

Environment:
  OPENPR_UI_ARTIFACT_DIR      Optional screenshot artifact directory.
  OPENPR_UI_REVIEW_GALLERY    Optional HTML gallery path.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --artifact-dir)
      ARTIFACT_DIR="${2:-}"
      if [[ -z "$ARTIFACT_DIR" ]]; then
        echo "--artifact-dir requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --gallery)
      GALLERY_PATH="${2:-}"
      if [[ -z "$GALLERY_PATH" ]]; then
        echo "--gallery requires a path" >&2
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

failures=0

check() {
  local description="$1"
  shift
  if "$@"; then
    printf 'PASS: %s\n' "$description"
  else
    printf 'FAIL: %s\n' "$description" >&2
    failures=$((failures + 1))
  fi
}

file_exists() {
  [[ -f "$1" ]]
}

non_empty_file() {
  [[ -s "$1" ]]
}

gallery_contains() {
  local needle="$1"
  rg -q --fixed-strings "$needle" "$GALLERY_PATH"
}

printf 'Universal forms UI review gallery verification\n'
printf '  gallery: %s\n' "$GALLERY_PATH"
printf '  artifact dir: %s\n' "$ARTIFACT_DIR"
printf '\n'

check "gallery exists" file_exists "$GALLERY_PATH"
check "gallery is non-empty" non_empty_file "$GALLERY_PATH"

for path in \
  "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-desktop.png" \
  "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-mobile.png" \
  "$ARTIFACT_DIR/template-work-items/template-work-items-desktop.png" \
  "$ARTIFACT_DIR/template-work-items/template-work-items-mobile.png" \
  "$ARTIFACT_DIR/forms-ui/forms-ui-desktop.png" \
  "$ARTIFACT_DIR/forms-ui/forms-ui-mobile.png" \
  "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-desktop.png" \
  "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-mobile.png" \
  "$ARTIFACT_DIR/project-template-wizard.log" \
  "$ARTIFACT_DIR/template-work-items.log" \
  "$ARTIFACT_DIR/forms-ui.log" \
  "$ARTIFACT_DIR/restaurant-ordering.log"; do
  check "artifact input exists: $path" file_exists "$path"
  check "gallery references: $path" gallery_contains "$path"
done

check "gallery has reviewer checklist" gallery_contains "Reviewer Checklist"
check "gallery links restaurant manual row" gallery_contains "Restaurant template can create a project directly"
check "gallery links frontend usability row" gallery_contains "Universal forms frontend is usable by a non-technical operator"
check "gallery has screenshot images" gallery_contains "<img alt="
check "gallery exposes browser image count marker" gallery_contains "data-gallery-image-count"
check "gallery exposes browser loaded image marker" gallery_contains "data-gallery-loaded-images"

render_desktop="$RENDER_DIR/ui-review-gallery-desktop.png"
render_mobile="$RENDER_DIR/ui-review-gallery-mobile.png"
if [[ -f "$render_desktop" || -f "$render_mobile" ]]; then
  check "rendered desktop gallery screenshot exists" file_exists "$render_desktop"
  check "rendered desktop gallery screenshot is non-empty" non_empty_file "$render_desktop"
  check "rendered mobile gallery screenshot exists" file_exists "$render_mobile"
  check "rendered mobile gallery screenshot is non-empty" non_empty_file "$render_mobile"
fi

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms UI review gallery verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms UI review gallery verification passed.\n'
