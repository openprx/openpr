#!/usr/bin/env bash
set -euo pipefail

REPORT_DIR="/opt/worker/report/openpr/docs"
ARTIFACT_DIR="${OPENPR_UI_ARTIFACT_DIR:-/opt/worker/report/openpr/artifacts/universal-forms-ui-2026-05-31}"
MANIFEST_PATH="${OPENPR_UI_ARTIFACT_MANIFEST:-$REPORT_DIR/openpr-universal-form-ui-artifacts-2026-05-31.md}"

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-ui-artifacts.sh [--artifact-dir PATH] [--manifest PATH]

Verifies that universal forms UI acceptance artifacts are usable reviewer
evidence:
  - artifact manifest exists and references every screenshot
  - screenshots exist, are non-empty PNG files, and have expected dimensions
  - browser smoke logs exist and contain PASS output and screenshot capture output

Environment:
  OPENPR_UI_ARTIFACT_DIR       Optional screenshot artifact directory.
  OPENPR_UI_ARTIFACT_MANIFEST  Optional markdown manifest path.
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
    --manifest)
      MANIFEST_PATH="${2:-}"
      if [[ -z "$MANIFEST_PATH" ]]; then
        echo "--manifest requires a path" >&2
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

manifest_contains() {
  local needle="$1"
  rg -q --fixed-strings "$needle" "$MANIFEST_PATH"
}

log_contains() {
  local path="$1"
  local needle="$2"
  rg -q --fixed-strings "$needle" "$path"
}

png_dimensions_match() {
  local path="$1"
  local expected="$2"
  local description
  description="$(file "$path")"
  [[ "$description" == *"PNG image data, $expected,"* ]]
}

forms_desktop="$ARTIFACT_DIR/forms-ui/forms-ui-desktop.png"
forms_mobile="$ARTIFACT_DIR/forms-ui/forms-ui-mobile.png"
project_template_desktop="$ARTIFACT_DIR/project-template-wizard/project-template-wizard-desktop.png"
project_template_mobile="$ARTIFACT_DIR/project-template-wizard/project-template-wizard-mobile.png"
template_work_items_desktop="$ARTIFACT_DIR/template-work-items/template-work-items-desktop.png"
template_work_items_mobile="$ARTIFACT_DIR/template-work-items/template-work-items-mobile.png"
restaurant_desktop="$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-desktop.png"
restaurant_mobile="$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-mobile.png"
forms_log="$ARTIFACT_DIR/forms-ui.log"
project_template_log="$ARTIFACT_DIR/project-template-wizard.log"
template_work_items_log="$ARTIFACT_DIR/template-work-items.log"
restaurant_log="$ARTIFACT_DIR/restaurant-ordering.log"

printf 'Universal forms UI artifact verification\n'
printf '  manifest: %s\n' "$MANIFEST_PATH"
printf '  artifact dir: %s\n' "$ARTIFACT_DIR"
printf '\n'

check "manifest exists" file_exists "$MANIFEST_PATH"

for path in \
  "$project_template_desktop" \
  "$project_template_mobile" \
  "$template_work_items_desktop" \
  "$template_work_items_mobile" \
  "$forms_desktop" \
  "$forms_mobile" \
  "$restaurant_desktop" \
  "$restaurant_mobile" \
  "$project_template_log" \
  "$template_work_items_log" \
  "$forms_log" \
  "$restaurant_log"; do
  check "file exists: $path" file_exists "$path"
  check "file is non-empty: $path" non_empty_file "$path"
done

check "project template desktop screenshot is 1366x900 PNG" png_dimensions_match "$project_template_desktop" "1366 x 900"
check "project template mobile screenshot is 390x844 PNG" png_dimensions_match "$project_template_mobile" "390 x 844"
check "template work items desktop screenshot is 1366x900 PNG" png_dimensions_match "$template_work_items_desktop" "1366 x 900"
check "template work items mobile screenshot is 390x844 PNG" png_dimensions_match "$template_work_items_mobile" "390 x 844"
check "Forms desktop screenshot is 1366x900 PNG" png_dimensions_match "$forms_desktop" "1366 x 900"
check "Forms mobile screenshot is 390x844 PNG" png_dimensions_match "$forms_mobile" "390 x 844"
check "restaurant desktop screenshot is 1366x900 PNG" png_dimensions_match "$restaurant_desktop" "1366 x 900"
check "restaurant mobile screenshot is 390x844 PNG" png_dimensions_match "$restaurant_mobile" "390 x 844"

check "manifest references project template desktop screenshot" manifest_contains "$project_template_desktop"
check "manifest references project template mobile screenshot" manifest_contains "$project_template_mobile"
check "manifest references template work items desktop screenshot" manifest_contains "$template_work_items_desktop"
check "manifest references template work items mobile screenshot" manifest_contains "$template_work_items_mobile"
check "manifest references Forms desktop screenshot" manifest_contains "$forms_desktop"
check "manifest references Forms mobile screenshot" manifest_contains "$forms_mobile"
check "manifest references restaurant desktop screenshot" manifest_contains "$restaurant_desktop"
check "manifest references restaurant mobile screenshot" manifest_contains "$restaurant_mobile"
check "manifest references project template smoke log" manifest_contains "$project_template_log"
check "manifest references template work items smoke log" manifest_contains "$template_work_items_log"
check "manifest references Forms smoke log" manifest_contains "$forms_log"
check "manifest references restaurant smoke log" manifest_contains "$restaurant_log"

check "project template browser smoke log passed" log_contains "$project_template_log" "Project template wizard browser smoke passed"
check "template work items browser smoke log passed" log_contains "$template_work_items_log" "Template work-item browser smoke passed"
check "Forms browser smoke log passed" log_contains "$forms_log" "Forms UI browser smoke passed"
check "restaurant browser smoke log passed" log_contains "$restaurant_log" "Restaurant ordering browser smoke passed"
check "project template browser smoke captured screenshots" log_contains "$project_template_log" "Project template wizard screenshots:"
check "template work items browser smoke captured screenshots" log_contains "$template_work_items_log" "Template work-item screenshots:"
check "Forms browser smoke captured screenshots" log_contains "$forms_log" "Forms UI screenshots:"
check "restaurant browser smoke captured screenshots" log_contains "$restaurant_log" "Restaurant ordering screenshots:"

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms UI artifact verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms UI artifact verification passed.\n'
