#!/usr/bin/env bash
set -euo pipefail

REPORT_DIR="/opt/worker/report/openpr/docs"
ARTIFACT_DIR="${OPENPR_UI_ARTIFACT_DIR:-/opt/worker/report/openpr/artifacts/universal-forms-ui-2026-05-31}"
OUTPUT_PATH="${OPENPR_UI_REVIEW_GALLERY:-$REPORT_DIR/openpr-universal-form-ui-review-gallery-2026-05-31.html}"

usage() {
  cat <<'EOF'
Usage: scripts/prepare-universal-forms-ui-review-gallery.sh [--artifact-dir PATH] [--output PATH]

Generates a local HTML gallery for universal forms reviewer screenshots and
smoke logs. The gallery is a manual acceptance aid only; it does not mark
acceptance as passed.

Environment:
  OPENPR_UI_ARTIFACT_DIR      Optional screenshot artifact directory.
  OPENPR_UI_REVIEW_GALLERY    Optional HTML output path.
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
    --output)
      OUTPUT_PATH="${2:-}"
      if [[ -z "$OUTPUT_PATH" ]]; then
        echo "--output requires a path" >&2
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

required_files=(
  "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-desktop.png"
  "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-mobile.png"
  "$ARTIFACT_DIR/template-work-items/template-work-items-desktop.png"
  "$ARTIFACT_DIR/template-work-items/template-work-items-mobile.png"
  "$ARTIFACT_DIR/forms-ui/forms-ui-desktop.png"
  "$ARTIFACT_DIR/forms-ui/forms-ui-mobile.png"
  "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-desktop.png"
  "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-mobile.png"
  "$ARTIFACT_DIR/project-template-wizard.log"
  "$ARTIFACT_DIR/template-work-items.log"
  "$ARTIFACT_DIR/forms-ui.log"
  "$ARTIFACT_DIR/restaurant-ordering.log"
)

for path in "${required_files[@]}"; do
  if [[ ! -s "$path" ]]; then
    echo "Missing UI review gallery input: $path" >&2
    exit 1
  fi
done

mkdir -p "$(dirname "$OUTPUT_PATH")"
output_tmp="$(mktemp "$(dirname "$OUTPUT_PATH")/.ui-review-gallery.XXXXXX")"
trap 'rm -f "$output_tmp"' EXIT

{
  printf '<!doctype html>\n'
  printf '<html lang="en">\n'
  printf '<head>\n'
  printf '<meta charset="utf-8">\n'
  printf '<meta name="viewport" content="width=device-width, initial-scale=1">\n'
  printf '<title>OpenPR Universal Forms UI Review Gallery</title>\n'
  printf '<style>\n'
  printf 'body{font-family:Arial,sans-serif;margin:0;color:#1f2937;background:#f8fafc;}main{max-width:1180px;margin:0 auto;padding:32px 20px;}h1{font-size:28px;margin:0 0 8px;}h2{font-size:20px;margin:32px 0 12px;}p{line-height:1.55;}code{background:#e5e7eb;padding:2px 5px;border-radius:4px;}table{border-collapse:collapse;width:100%%;background:#fff;}th,td{border:1px solid #d1d5db;padding:8px;text-align:left;vertical-align:top;}th{background:#f3f4f6;}section{margin-top:24px;}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));gap:18px;}.shot{background:#fff;border:1px solid #d1d5db;border-radius:6px;padding:12px;}.shot h3{font-size:16px;margin:0 0 8px;}.shot img{display:block;width:100%%;height:auto;border:1px solid #e5e7eb;background:#fff;}.meta{font-size:13px;color:#4b5563;word-break:break-all;}@media(max-width:640px){main{padding:20px 12px;}h1{font-size:22px;}}\n'
  printf '</style>\n'
  printf '</head>\n'
  printf '<body>\n'
  printf '<main>\n'
  printf '<h1>OpenPR Universal Forms UI Review Gallery</h1>\n'
  printf '<p>Generated at: <code>%s</code></p>\n' "$(date -Is)"
  printf '<p>This gallery is a reviewer aid for manual acceptance. It groups the browser smoke screenshots and logs for project templates, template work items, universal forms, and restaurant ordering.</p>\n'
  printf '<section>\n'
  printf '<h2>Reviewer Checklist</h2>\n'
  printf '<table><thead><tr><th>Manual row</th><th>Inspect here</th></tr></thead><tbody>\n'
  printf '<tr><td>Restaurant template can create a project directly</td><td>Project template wizard screenshots and template work item screenshots.</td></tr>\n'
  printf '<tr><td>Universal forms frontend is usable by a non-technical operator</td><td>Project template wizard, template work item, and Forms UI screenshots.</td></tr>\n'
  printf '<tr><td>Amount, quantity, and subtotal behavior is acceptable</td><td>Forms UI and restaurant ordering screenshots.</td></tr>\n'
  printf '<tr><td>Order, order line, table change, print, and report workflow is acceptable</td><td>Restaurant ordering screenshots.</td></tr>\n'
  printf '</tbody></table>\n'
  printf '</section>\n'
  printf '<section><h2>Screenshots</h2><div class="grid">\n'
  printf '<article class="shot"><h3>Project template wizard - desktop</h3><img alt="Project template wizard desktop" src="%s"><p class="meta">%s</p></article>\n' "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-desktop.png" "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-desktop.png"
  printf '<article class="shot"><h3>Project template wizard - mobile</h3><img alt="Project template wizard mobile" src="%s"><p class="meta">%s</p></article>\n' "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-mobile.png" "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-mobile.png"
  printf '<article class="shot"><h3>Template work items - desktop</h3><img alt="Template work items desktop" src="%s"><p class="meta">%s</p></article>\n' "$ARTIFACT_DIR/template-work-items/template-work-items-desktop.png" "$ARTIFACT_DIR/template-work-items/template-work-items-desktop.png"
  printf '<article class="shot"><h3>Template work items - mobile</h3><img alt="Template work items mobile" src="%s"><p class="meta">%s</p></article>\n' "$ARTIFACT_DIR/template-work-items/template-work-items-mobile.png" "$ARTIFACT_DIR/template-work-items/template-work-items-mobile.png"
  printf '<article class="shot"><h3>Forms UI - desktop</h3><img alt="Forms UI desktop" src="%s"><p class="meta">%s</p></article>\n' "$ARTIFACT_DIR/forms-ui/forms-ui-desktop.png" "$ARTIFACT_DIR/forms-ui/forms-ui-desktop.png"
  printf '<article class="shot"><h3>Forms UI - mobile</h3><img alt="Forms UI mobile" src="%s"><p class="meta">%s</p></article>\n' "$ARTIFACT_DIR/forms-ui/forms-ui-mobile.png" "$ARTIFACT_DIR/forms-ui/forms-ui-mobile.png"
  printf '<article class="shot"><h3>Restaurant ordering - desktop</h3><img alt="Restaurant ordering desktop" src="%s"><p class="meta">%s</p></article>\n' "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-desktop.png" "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-desktop.png"
  printf '<article class="shot"><h3>Restaurant ordering - mobile</h3><img alt="Restaurant ordering mobile" src="%s"><p class="meta">%s</p></article>\n' "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-mobile.png" "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-mobile.png"
  printf '</div></section>\n'
  printf '<section>\n'
  printf '<h2>Smoke Logs</h2>\n'
  printf '<table><thead><tr><th>Scenario</th><th>Log</th></tr></thead><tbody>\n'
  printf '<tr><td>Project template wizard</td><td><code>%s</code></td></tr>\n' "$ARTIFACT_DIR/project-template-wizard.log"
  printf '<tr><td>Template work items</td><td><code>%s</code></td></tr>\n' "$ARTIFACT_DIR/template-work-items.log"
  printf '<tr><td>Forms UI</td><td><code>%s</code></td></tr>\n' "$ARTIFACT_DIR/forms-ui.log"
  printf '<tr><td>Restaurant ordering</td><td><code>%s</code></td></tr>\n' "$ARTIFACT_DIR/restaurant-ordering.log"
  printf '</tbody></table>\n'
  printf '</section>\n'
  printf '<script>\n'
  printf "window.addEventListener('load',function(){var imgs=Array.from(document.querySelectorAll('img'));var loaded=imgs.filter(function(img){return img.complete&&img.naturalWidth>0&&img.naturalHeight>0;});document.body.setAttribute('data-gallery-image-count',String(imgs.length));document.body.setAttribute('data-gallery-loaded-images',String(loaded.length));});\n"
  printf '</script>\n'
  printf '</main>\n'
  printf '</body>\n'
  printf '</html>\n'
} >"$output_tmp"

if [[ -f "$OUTPUT_PATH" ]]; then
  chmod --reference="$OUTPUT_PATH" "$output_tmp"
fi
mv -f "$output_tmp" "$OUTPUT_PATH"
output_tmp=""

echo "UI review gallery: $OUTPUT_PATH" >&2
