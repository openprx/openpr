#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
ARTIFACT_DIR="${OPENPR_UI_ARTIFACT_DIR:-/opt/worker/report/openpr/artifacts/universal-forms-ui-2026-05-31}"
MANIFEST_PATH="${OPENPR_UI_ARTIFACT_MANIFEST:-$REPORT_DIR/openpr-universal-form-ui-artifacts-2026-05-31.md}"
FRONTEND_LOCK="${OPENPR_FRONTEND_BUILD_LOCK:-/tmp/openpr-frontend-build.lock}"

usage() {
  cat <<'EOF'
Usage: scripts/collect-universal-forms-ui-artifacts.sh [--artifact-dir PATH] [--manifest PATH]

Runs the project template, universal forms, and restaurant browser smoke checks
with screenshot capture enabled, then writes a reviewer-facing artifact manifest.

Environment:
  OPENPR_UI_ARTIFACT_DIR       Optional screenshot output directory.
  OPENPR_UI_ARTIFACT_MANIFEST  Optional markdown manifest path.
  OPENPR_UI_REVIEW_GALLERY     Optional HTML reviewer gallery path.
  OPENPR_FRONTEND_BUILD_LOCK   Optional lock file for frontend build/smoke.
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

mkdir -p \
  "$ARTIFACT_DIR/project-template-wizard" \
  "$ARTIFACT_DIR/template-work-items" \
  "$ARTIFACT_DIR/forms-ui" \
  "$ARTIFACT_DIR/restaurant-ordering" \
  "$(dirname "$MANIFEST_PATH")"

manifest_tmp="$(mktemp "$(dirname "$MANIFEST_PATH")/.ui-artifacts.XXXXXX")"
trap 'rm -f "$manifest_tmp"' EXIT

if ! command -v bun >/dev/null 2>&1 && [[ -x "$HOME/.bun/bin/bun" ]]; then
  export PATH="$HOME/.bun/bin:$PATH"
fi

if ! command -v bun >/dev/null 2>&1; then
  echo "Bun is required for frontend UI artifact collection. Install Bun or add it to PATH." >&2
  exit 2
fi

FRONTEND_BUN="$(command -v bun)"

exec 9>"$FRONTEND_LOCK"
flock 9

(cd "$ROOT_DIR/frontend" && "$FRONTEND_BUN" run build)

forms_log="$ARTIFACT_DIR/forms-ui.log"
project_template_log="$ARTIFACT_DIR/project-template-wizard.log"
template_work_items_log="$ARTIFACT_DIR/template-work-items.log"
restaurant_log="$ARTIFACT_DIR/restaurant-ordering.log"

(
  cd "$ROOT_DIR/frontend"
  OPENPR_PROJECT_TEMPLATE_SCREENSHOT_DIR="$ARTIFACT_DIR/project-template-wizard" "$FRONTEND_BUN" run smoke:project-template
) >"$project_template_log" 2>&1

(
  cd "$ROOT_DIR/frontend"
  OPENPR_TEMPLATE_WORK_ITEMS_SCREENSHOT_DIR="$ARTIFACT_DIR/template-work-items" "$FRONTEND_BUN" run smoke:template-work-items
) >"$template_work_items_log" 2>&1

(
  cd "$ROOT_DIR/frontend"
  OPENPR_FORMS_UI_SCREENSHOT_DIR="$ARTIFACT_DIR/forms-ui" "$FRONTEND_BUN" run smoke:forms-ui
) >"$forms_log" 2>&1

(
  cd "$ROOT_DIR/frontend"
  OPENPR_RESTAURANT_SCREENSHOT_DIR="$ARTIFACT_DIR/restaurant-ordering" "$FRONTEND_BUN" run smoke:restaurant-ordering
) >"$restaurant_log" 2>&1

flock -u 9

required_files=(
  "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-desktop.png"
  "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-mobile.png"
  "$ARTIFACT_DIR/template-work-items/template-work-items-desktop.png"
  "$ARTIFACT_DIR/template-work-items/template-work-items-mobile.png"
  "$ARTIFACT_DIR/forms-ui/forms-ui-desktop.png"
  "$ARTIFACT_DIR/forms-ui/forms-ui-mobile.png"
  "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-desktop.png"
  "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-mobile.png"
)

for path in "${required_files[@]}"; do
  if [[ ! -s "$path" ]]; then
    echo "Missing screenshot artifact: $path" >&2
    exit 1
  fi
done

{
  printf '# OpenPR Universal Forms UI Acceptance Artifacts\n\n'
  printf '%s\n' "- Generated at: $(date -Is)"
  printf '%s\n' "- Repository: \`$ROOT_DIR\`"
  printf '%s\n' "- Artifact directory: \`$ARTIFACT_DIR\`"
  printf '\n'

  printf '## Browser Smoke Screenshots\n\n'
  printf '| Scenario | Viewport | Artifact |\n'
  printf '| --- | --- | --- |\n'
  printf '| Project template wizard | Desktop 1366x900 | `%s` |\n' "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-desktop.png"
  printf '| Project template wizard | Mobile 390x844 | `%s` |\n' "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-mobile.png"
  printf '| Template work items | Desktop 1366x900 | `%s` |\n' "$ARTIFACT_DIR/template-work-items/template-work-items-desktop.png"
  printf '| Template work items | Mobile 390x844 | `%s` |\n' "$ARTIFACT_DIR/template-work-items/template-work-items-mobile.png"
  printf '| Forms UI | Desktop 1366x900 | `%s` |\n' "$ARTIFACT_DIR/forms-ui/forms-ui-desktop.png"
  printf '| Forms UI | Mobile 390x844 | `%s` |\n' "$ARTIFACT_DIR/forms-ui/forms-ui-mobile.png"
  printf '| Restaurant ordering | Desktop 1366x900 | `%s` |\n' "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-desktop.png"
  printf '| Restaurant ordering | Mobile 390x844 | `%s` |\n' "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-mobile.png"
  printf '\n'

  printf '## Smoke Logs\n\n'
  printf '| Scenario | Log |\n'
  printf '| --- | --- |\n'
  printf '| Project template wizard | `%s` |\n' "$project_template_log"
  printf '| Template work items | `%s` |\n' "$template_work_items_log"
  printf '| Forms UI | `%s` |\n' "$forms_log"
  printf '| Restaurant ordering | `%s` |\n' "$restaurant_log"
  printf '\n'

  printf '## Acceptance Use\n\n'
  printf '%s\n' '- Use these screenshots as supporting evidence for the manual acceptance rows covering project template creation, frontend usability, amount display, restaurant workflow, print jobs, report workflow, and mobile layout.'
  printf '%s\n' '- These screenshots do not replace manual signoff; they only make the browser smoke output easier for reviewers to inspect.'
} >"$manifest_tmp"

"$ROOT_DIR/scripts/verify-universal-forms-ui-artifacts.sh" \
  --artifact-dir "$ARTIFACT_DIR" \
  --manifest "$manifest_tmp"

if [[ -f "$MANIFEST_PATH" ]]; then
  chmod --reference="$MANIFEST_PATH" "$manifest_tmp"
fi
mv -f "$manifest_tmp" "$MANIFEST_PATH"
manifest_tmp=""

"$ROOT_DIR/scripts/verify-universal-forms-ui-artifacts.sh" \
  --artifact-dir "$ARTIFACT_DIR" \
  --manifest "$MANIFEST_PATH"

"$ROOT_DIR/scripts/prepare-universal-forms-ui-review-gallery.sh" \
  --artifact-dir "$ARTIFACT_DIR"

"$ROOT_DIR/scripts/verify-universal-forms-ui-review-gallery.sh" \
  --artifact-dir "$ARTIFACT_DIR"

"$ROOT_DIR/scripts/smoke-universal-forms-ui-review-gallery-render.sh" \
  --render-dir "$ARTIFACT_DIR/ui-review-gallery"

"$ROOT_DIR/scripts/verify-universal-forms-ui-review-gallery.sh" \
  --artifact-dir "$ARTIFACT_DIR"

echo "UI acceptance artifact manifest: $MANIFEST_PATH"
