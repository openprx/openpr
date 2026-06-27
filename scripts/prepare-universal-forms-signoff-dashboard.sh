#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
ARTIFACT_DIR="${OPENPR_UI_ARTIFACT_DIR:-/opt/worker/report/openpr/artifacts/universal-forms-ui-2026-05-31}"
SIGNOFF_STATUS_JSON_PATH="${OPENPR_SIGNOFF_STATUS_JSON:-$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json}"
OUTPUT_PATH="${OPENPR_SIGNOFF_DASHBOARD:-$REPORT_DIR/openpr-universal-form-signoff-dashboard-2026-05-31.html}"

usage() {
  cat <<'EOF'
Usage: scripts/prepare-universal-forms-signoff-dashboard.sh [--status-json PATH] [--artifact-dir PATH] [--output PATH]

Generates a local HTML dashboard for the universal forms manual signoff queue.
The dashboard is a reviewer aid only; it does not mark acceptance as passed.

Environment:
  OPENPR_SIGNOFF_STATUS_JSON   Optional signoff status JSON path.
  OPENPR_UI_ARTIFACT_DIR       Optional screenshot artifact directory.
  OPENPR_SIGNOFF_DASHBOARD     Optional HTML output path.
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

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to generate the signoff dashboard" >&2
  exit 2
fi

if [[ ! -s "$SIGNOFF_STATUS_JSON_PATH" ]]; then
  echo "Signoff status JSON not found: $SIGNOFF_STATUS_JSON_PATH" >&2
  exit 1
fi

for path in \
  "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-desktop.png" \
  "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-mobile.png" \
  "$ARTIFACT_DIR/forms-ui/forms-ui-desktop.png" \
  "$ARTIFACT_DIR/forms-ui/forms-ui-mobile.png" \
  "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-desktop.png" \
  "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-mobile.png"; do
  if [[ ! -s "$path" ]]; then
    echo "Missing signoff dashboard screenshot input: $path" >&2
    exit 1
  fi
done

queue_count="$(jq -r '.manual_signoff.pending_queue | length' "$SIGNOFF_STATUS_JSON_PATH")"
accepted_count="$(jq -r '.manual_signoff.accepted_rows' "$SIGNOFF_STATUS_JSON_PATH")"
total_count="$(jq -r '.manual_signoff.total_rows' "$SIGNOFF_STATUS_JSON_PATH")"
pending_count="$(jq -r '.manual_signoff.pending_rows' "$SIGNOFF_STATUS_JSON_PATH")"
next_key="$(jq -r '.manual_signoff.next_row.key' "$SIGNOFF_STATUS_JSON_PATH")"
next_item="$(jq -r '.manual_signoff.next_row.item' "$SIGNOFF_STATUS_JSON_PATH")"
next_recorder_command="$(jq -r '.manual_signoff.next_row.recorder_command // ""' "$SIGNOFF_STATUS_JSON_PATH")"
generated_from="$(jq -r '.generated_at' "$SIGNOFF_STATUS_JSON_PATH")"
final_signoff_complete="$(jq -r '.manual_signoff.final_signoff_complete' "$SIGNOFF_STATUS_JSON_PATH")"

html_escape() {
  jq -Rr @html <<<"$1"
}

attr_escape() {
  jq -Rr @html <<<"$1"
}

mkdir -p "$(dirname "$OUTPUT_PATH")"
output_tmp="$(mktemp "$(dirname "$OUTPUT_PATH")/.signoff-dashboard.XXXXXX")"
trap 'rm -f "$output_tmp"' EXIT

{
  printf '<!doctype html>\n'
  printf '<html lang="en">\n'
  printf '<head>\n'
  printf '<meta charset="utf-8">\n'
  printf '<meta name="viewport" content="width=device-width, initial-scale=1">\n'
  printf '<title>OpenPR Universal Forms Signoff Dashboard</title>\n'
  printf '<style>\n'
  printf ':root{color-scheme:light;--ink:#172026;--muted:#5d6875;--line:#ccd5df;--panel:#ffffff;--paper:#f4f6f8;--accent:#0f766e;--warn:#b45309;--done:#166534;--code:#e7ecef;}*{box-sizing:border-box;}body{margin:0;background:var(--paper);color:var(--ink);font-family:ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;}main{max-width:1240px;margin:0 auto;padding:28px 18px 40px;}header{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:20px;align-items:end;border-bottom:2px solid var(--ink);padding-bottom:18px;}h1{font-size:30px;line-height:1.08;margin:0;letter-spacing:0;}h2{font-size:18px;margin:30px 0 12px;}p{line-height:1.55;margin:8px 0;}ol{margin:10px 0 0;padding-left:22px;}li{margin:7px 0;line-height:1.5;}.muted{color:var(--muted);}.summary{display:grid;grid-template-columns:repeat(4,minmax(130px,1fr));gap:10px;margin-top:18px;}.metric{background:var(--panel);border:1px solid var(--line);border-radius:6px;padding:12px;}.metric strong{display:block;font-size:24px;line-height:1.1;}.start{background:var(--panel);border:2px solid var(--accent);border-radius:6px;margin-top:22px;padding:14px;}.queue{display:grid;gap:12px;}.row{background:var(--panel);border:1px solid var(--line);border-left:5px solid var(--line);border-radius:6px;padding:14px;}.row.next{border-left-color:var(--accent);box-shadow:0 0 0 2px rgba(15,118,110,.08);}.row h3{font-size:16px;margin:0 0 8px;}.badge{display:inline-block;border:1px solid var(--line);border-radius:999px;padding:2px 9px;font-size:12px;line-height:1.6;background:#f8fafc;color:var(--muted);}.badge.next{border-color:var(--accent);color:var(--accent);}.badge.pending{border-color:var(--warn);color:var(--warn);}.badge.accepted{border-color:var(--done);color:var(--done);}.details{display:grid;grid-template-columns:1fr 1fr;gap:12px;margin-top:12px;}.box{border:1px solid var(--line);border-radius:6px;background:#fbfcfd;padding:10px;min-width:0;}.box h4{font-size:13px;margin:0 0 6px;color:var(--muted);text-transform:uppercase;letter-spacing:.04em;}code{background:var(--code);border-radius:4px;padding:2px 5px;word-break:break-word;}pre{white-space:pre-wrap;word-break:break-word;background:var(--code);border-radius:6px;padding:10px;margin:8px 0 0;}.links{display:grid;grid-template-columns:repeat(auto-fit,minmax(230px,1fr));gap:10px;}.linkcard{display:block;background:var(--panel);border:1px solid var(--line);border-radius:6px;padding:12px;color:var(--ink);text-decoration:none;}.shots{display:grid;grid-template-columns:repeat(auto-fit,minmax(230px,1fr));gap:12px;}.shot{background:var(--panel);border:1px solid var(--line);border-radius:6px;padding:10px;}.shot img{display:block;width:100%%;height:auto;border:1px solid #dfe5eb;background:white;}@media(max-width:760px){header{grid-template-columns:1fr}.summary{grid-template-columns:repeat(2,minmax(0,1fr));}.details{grid-template-columns:1fr;}h1{font-size:24px;}main{padding:20px 12px 32px;}}\n'
  printf '</style>\n'
  printf '</head>\n'
  printf '<body data-signoff-dashboard-queue-count="%s" data-next-signoff-key="%s" data-final-signoff-complete="%s">\n' \
    "$(attr_escape "$queue_count")" "$(attr_escape "$next_key")" "$(attr_escape "$final_signoff_complete")"
  printf '<main>\n'
  if [[ "$pending_count" == "0" ]]; then
    printf '<header><div><h1>OpenPR Universal Forms Signoff Dashboard</h1><p class="muted">Reviewer-facing queue generated from <code>%s</code>. All manual rows are accepted; use this page to finalize and recheck the delivery bundle.</p></div><div><span class="badge accepted">Finalize</span></div></header>\n' \
      "$(html_escape "$SIGNOFF_STATUS_JSON_PATH")"
  else
    printf '<header><div><h1>OpenPR Universal Forms Signoff Dashboard</h1><p class="muted">Reviewer-facing queue generated from <code>%s</code>. It helps reviewers inspect evidence and copy the exact recorder command; it does not change signoff state.</p></div><div><span class="badge next">Next: %s</span></div></header>\n' \
      "$(html_escape "$SIGNOFF_STATUS_JSON_PATH")" "$(html_escape "$next_key")"
  fi
  printf '<section class="summary" aria-label="Signoff summary">\n'
  printf '<div class="metric"><span class="muted">Accepted</span><strong>%s/%s</strong></div>\n' "$(html_escape "$accepted_count")" "$(html_escape "$total_count")"
  printf '<div class="metric"><span class="muted">Pending</span><strong>%s</strong></div>\n' "$(html_escape "$pending_count")"
  printf '<div class="metric"><span class="muted">Queue</span><strong>%s</strong></div>\n' "$(html_escape "$queue_count")"
  printf '<div class="metric"><span class="muted">Final signoff</span><strong>%s</strong></div>\n' "$(html_escape "$final_signoff_complete")"
  printf '</section>\n'
  printf '<section class="start" aria-label="Start here" data-signoff-start-key="%s">\n' "$(attr_escape "$next_key")"
  printf '<h2>Start Here</h2>\n'
  if [[ "$pending_count" == "0" ]]; then
    printf '<p><strong>Current state:</strong> %s/%s manual rows accepted, %s remaining. Finalization is now the next action.</p>\n' \
      "$(html_escape "$accepted_count")" "$(html_escape "$total_count")" "$(html_escape "$pending_count")"
    printf '<ol>\n'
    printf '<li>Run <code>scripts/verify-universal-forms-acceptance-signoff.sh</code> to recheck the signed runbook/evidence pair.</li>\n'
    printf '<li>Run the finalizer to mark the tracker accepted and refresh derived reports.</li>\n'
    printf '<li>Run strict delivery-state and delivery-bundle audits after finalization.</li>\n'
    printf '</ol>\n'
    printf '<pre>scripts/finalize-universal-forms-acceptance.sh\nscripts/audit-universal-forms-delivery-state.sh --strict\nscripts/audit-universal-forms-delivery-bundle.sh</pre>\n'
  else
    printf '<p><strong>Current state:</strong> %s/%s manual rows accepted, %s remaining. Release stays blocked until every row is accepted and finalization passes.</p>\n' \
      "$(html_escape "$accepted_count")" "$(html_escape "$total_count")" "$(html_escape "$pending_count")"
    printf '<ol>\n'
    printf '<li>Review the highlighted next row: <code>%s</code>.</li>\n' "$(html_escape "$next_key")"
    printf '<li>Open the linked evidence files and the primary screenshots below.</li>\n'
    printf '<li>Run <code>scripts/status-universal-forms-delivery.sh</code> before recording the decision.</li>\n'
    printf '<li>If accepted, run this recorder command with the real reviewer name and evidence note.</li>\n'
    printf '</ol>\n'
    printf '<pre>%s</pre>\n' "$(html_escape "$next_recorder_command")"
  fi
  printf '</section>\n'
  if [[ "$pending_count" == "0" ]]; then
    printf '<section><h2>Finalization Action</h2><p><strong>All manual rows are accepted. Run finalization instead of recording another row.</strong></p><p class="muted">Status JSON generated at <code>%s</code>.</p></section>\n' \
      "$(html_escape "$generated_from")"
  else
    printf '<section><h2>Next Reviewer Action</h2><p><strong>%s</strong></p><p class="muted">Status JSON generated at <code>%s</code>.</p></section>\n' \
      "$(html_escape "$next_item")" "$(html_escape "$generated_from")"
  fi
  printf '<section><h2>Manual Signoff Queue</h2><div class="queue">\n'
  if [[ "$pending_count" == "0" ]]; then
    printf '<article class="row next" data-signoff-finalization="ready"><h3>All manual rows accepted</h3><p><span class="badge accepted">ready for finalization</span></p><div class="box"><h4>Next command</h4><pre>scripts/finalize-universal-forms-acceptance.sh</pre></div></article>\n'
  fi
  jq -r '
    .manual_signoff.pending_queue[]
    | [
        (.review_order | tostring),
        .key,
        .item,
        .status,
        (.is_next | tostring),
        (.actionable | tostring),
        .automated_evidence,
        .reviewer_check,
        .suggested_evidence_note,
        .recorder_command
      ]
    | @tsv
  ' "$SIGNOFF_STATUS_JSON_PATH" | while IFS=$'\t' read -r review_order key item status is_next actionable automated_evidence reviewer_check suggested_evidence_note recorder_command; do
    row_class="row"
    next_badge=""
    if [[ "$is_next" == "true" ]]; then
      row_class="row next"
      next_badge='<span class="badge next">next</span>'
    fi
    printf '<article class="%s" data-signoff-key="%s" data-actionable="%s">\n' "$row_class" "$(attr_escape "$key")" "$(attr_escape "$actionable")"
    printf '<h3>%s. %s</h3>\n' "$(html_escape "$review_order")" "$(html_escape "$item")"
    printf '<p><span class="badge pending">%s</span> %s <span class="badge">key: %s</span></p>\n' "$(html_escape "$status")" "$next_badge" "$(html_escape "$key")"
    printf '<div class="details">\n'
    printf '<div class="box"><h4>Automated evidence</h4><p>%s</p></div>\n' "$(html_escape "$automated_evidence")"
    printf '<div class="box"><h4>Reviewer check</h4><p>%s</p></div>\n' "$(html_escape "$reviewer_check")"
    printf '</div>\n'
    printf '<div class="box"><h4>Suggested evidence note</h4><p>%s</p><h4>Recorder command</h4><pre>%s</pre></div>\n' "$(html_escape "$suggested_evidence_note")" "$(html_escape "$recorder_command")"
    printf '</article>\n'
  done
  printf '</div></section>\n'
  printf '<section><h2>Evidence Links</h2><div class="links">\n'
  printf '<a class="linkcard" href="%s">User acceptance packet</a>\n' "$REPORT_DIR/openpr-universal-form-user-acceptance-packet-2026-05-31.md"
  printf '<a class="linkcard" href="%s">Manual evidence map</a>\n' "$REPORT_DIR/openpr-universal-form-manual-evidence-map-2026-05-31.md"
  printf '<a class="linkcard" href="%s">Manual signoff status JSON</a>\n' "$SIGNOFF_STATUS_JSON_PATH"
  printf '<a class="linkcard" href="%s">UI review gallery</a>\n' "$REPORT_DIR/openpr-universal-form-ui-review-gallery-2026-05-31.html"
  printf '<a class="linkcard" href="%s">User acceptance runbook</a>\n' "$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
  printf '<a class="linkcard" href="%s">Delivery manifest</a>\n' "$REPORT_DIR/openpr-universal-form-delivery-manifest-2026-05-31.md"
  printf '</div></section>\n'
  printf '<section><h2>Primary Screenshots</h2><div class="shots">\n'
  printf '<article class="shot"><h3>Project template wizard</h3><img alt="Project template wizard desktop" src="%s"></article>\n' "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-desktop.png"
  printf '<article class="shot"><h3>Forms UI</h3><img alt="Forms UI desktop" src="%s"></article>\n' "$ARTIFACT_DIR/forms-ui/forms-ui-desktop.png"
  printf '<article class="shot"><h3>Restaurant ordering</h3><img alt="Restaurant ordering desktop" src="%s"></article>\n' "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-desktop.png"
  printf '</div></section>\n'
  printf '<script>\n'
  printf "window.addEventListener('load',function(){var imgs=Array.from(document.querySelectorAll('img'));var loaded=imgs.filter(function(img){return img.complete&&img.naturalWidth>0&&img.naturalHeight>0;});document.body.setAttribute('data-dashboard-image-count',String(imgs.length));document.body.setAttribute('data-dashboard-loaded-images',String(loaded.length));});\n"
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

echo "Signoff dashboard: $OUTPUT_PATH" >&2
