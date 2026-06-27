#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d /tmp/openpr-uf-report-output-boundaries.XXXXXX)"
trap 'rm -rf "$TMP_DIR"' EXIT

failures=0

pass() {
  printf 'PASS: %s\n' "$1"
}

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  failures=$((failures + 1))
}

require_command() {
  local name="$1"
  if command -v "$name" >/dev/null 2>&1; then
    pass "$name is available"
  else
    fail "$name is available"
  fi
}

first_line() {
  head -n 1 "$1" || true
}

run_generator() {
  local label="$1"
  local kind="$2"
  local expected="$3"
  local output_path="$4"
  shift 4

  local stdout_path="$TMP_DIR/${label//[^A-Za-z0-9_]/_}.stdout"
  local stderr_path="$TMP_DIR/${label//[^A-Za-z0-9_]/_}.stderr"

  rm -f "$output_path" "$stdout_path" "$stderr_path"
  if "$@" >"$stdout_path" 2>"$stderr_path"; then
    pass "$label generator exits successfully"
  else
    fail "$label generator exits successfully"
    sed -n '1,80p' "$stderr_path" >&2 || true
    return
  fi

  if [[ -s "$stdout_path" ]]; then
    fail "$label generator leaves stdout empty when --output is used"
    sed -n '1,20p' "$stdout_path" >&2 || true
  else
    pass "$label generator leaves stdout empty when --output is used"
  fi

  if [[ -s "$output_path" ]]; then
    pass "$label output file is written"
  else
    fail "$label output file is written"
    return
  fi

  case "$kind" in
    markdown|html)
      local actual
      actual="$(first_line "$output_path")"
      if [[ "$actual" == "$expected" ]]; then
        pass "$label output starts with expected first line"
      else
        fail "$label output starts with expected first line"
        printf '  expected: %s\n  actual: %s\n' "$expected" "${actual:-<empty>}" >&2
      fi
      ;;
    json)
      if jq -e --arg expected "$expected" '.schema_version == $expected' "$output_path" >/dev/null; then
        pass "$label JSON schema version matches"
      else
        fail "$label JSON schema version matches"
      fi
      ;;
    *)
      fail "$label has known output kind"
      ;;
  esac
}

run_stdout_json_generator() {
  local label="$1"
  local expected="$2"
  shift 2

  local stdout_path="$TMP_DIR/${label//[^A-Za-z0-9_]/_}.stdout"
  local stderr_path="$TMP_DIR/${label//[^A-Za-z0-9_]/_}.stderr"
  local accidental_dash_path="$ROOT_DIR/-"

  rm -f "$stdout_path" "$stderr_path"
  if "$@" >"$stdout_path" 2>"$stderr_path"; then
    pass "$label stdout generator exits successfully"
  else
    fail "$label stdout generator exits successfully"
    sed -n '1,80p' "$stderr_path" >&2 || true
    return
  fi

  if [[ -s "$stdout_path" ]]; then
    pass "$label stdout JSON is written"
  else
    fail "$label stdout JSON is written"
    return
  fi

  if jq -e --arg expected "$expected" '.schema_version == $expected' "$stdout_path" >/dev/null; then
    pass "$label stdout JSON schema version matches"
  else
    fail "$label stdout JSON schema version matches"
  fi

  if [[ -e "$accidental_dash_path" ]]; then
    fail "$label does not create repository dash output file"
  else
    pass "$label does not create repository dash output file"
  fi
}

printf 'Universal forms report output boundary smoke\n'
printf '  repo: %s\n' "$ROOT_DIR"
printf '  temp dir: %s\n' "$TMP_DIR"
printf '\n'

require_command jq

run_generator \
  "completion audit" \
  markdown \
  "# OpenPR Universal Forms Completion Audit" \
  "$TMP_DIR/completion-audit.md" \
  "$ROOT_DIR/scripts/report-universal-forms-completion-audit.sh" --output "$TMP_DIR/completion-audit.md"

run_generator \
  "completion audit JSON" \
  json \
  "openpr.universal_forms.completion_audit.v1" \
  "$TMP_DIR/completion-audit.json" \
  "$ROOT_DIR/scripts/report-universal-forms-completion-audit-json.sh" --completion-audit "$TMP_DIR/completion-audit.md" --output "$TMP_DIR/completion-audit.json"

run_generator \
  "manual evidence map" \
  markdown \
  "# OpenPR Universal Forms Manual Evidence Map" \
  "$TMP_DIR/manual-evidence-map.md" \
  "$ROOT_DIR/scripts/prepare-universal-forms-manual-evidence-map.sh" --output "$TMP_DIR/manual-evidence-map.md"

run_generator \
  "manual signoff status" \
  markdown \
  "# OpenPR Universal Forms Manual Signoff Status" \
  "$TMP_DIR/manual-signoff-status.md" \
  "$ROOT_DIR/scripts/report-universal-forms-signoff-status.sh" --output "$TMP_DIR/manual-signoff-status.md"

run_generator \
  "manual signoff status JSON" \
  json \
  "openpr.universal_forms.signoff_status.v1" \
  "$TMP_DIR/manual-signoff-status.json" \
  "$ROOT_DIR/scripts/report-universal-forms-signoff-status-json.sh" --markdown "$TMP_DIR/manual-signoff-status.md" --output "$TMP_DIR/manual-signoff-status.json"

run_stdout_json_generator \
  "manual signoff status JSON --output -" \
  "openpr.universal_forms.signoff_status.v1" \
  "$ROOT_DIR/scripts/report-universal-forms-signoff-status-json.sh" --markdown "$TMP_DIR/manual-signoff-status.md" --output -

run_stdout_json_generator \
  "manual signoff status JSON --stdout" \
  "openpr.universal_forms.signoff_status.v1" \
  "$ROOT_DIR/scripts/report-universal-forms-signoff-status-json.sh" --markdown "$TMP_DIR/manual-signoff-status.md" --stdout

run_generator \
  "signoff dashboard" \
  html \
  "<!doctype html>" \
  "$TMP_DIR/signoff-dashboard.html" \
  "$ROOT_DIR/scripts/prepare-universal-forms-signoff-dashboard.sh" --status-json "$TMP_DIR/manual-signoff-status.json" --output "$TMP_DIR/signoff-dashboard.html"

run_generator \
  "next signoff review" \
  markdown \
  "# OpenPR Universal Forms Next Signoff Review" \
  "$TMP_DIR/next-signoff-review.md" \
  "$ROOT_DIR/scripts/prepare-universal-forms-next-signoff-review.sh" --output "$TMP_DIR/next-signoff-review.md"

run_generator \
  "user acceptance packet" \
  markdown \
  "# OpenPR Universal Forms User Acceptance Packet" \
  "$TMP_DIR/user-acceptance-packet.md" \
  "$ROOT_DIR/scripts/prepare-universal-forms-user-acceptance-packet.sh" --output "$TMP_DIR/user-acceptance-packet.md"

run_generator \
  "readiness summary" \
  markdown \
  "# OpenPR Universal Forms Readiness Summary" \
  "$TMP_DIR/readiness-summary.md" \
  "$ROOT_DIR/scripts/report-universal-forms-readiness-summary.sh" --output "$TMP_DIR/readiness-summary.md"

run_generator \
  "UI review gallery" \
  html \
  "<!doctype html>" \
  "$TMP_DIR/ui-review-gallery.html" \
  "$ROOT_DIR/scripts/prepare-universal-forms-ui-review-gallery.sh" --output "$TMP_DIR/ui-review-gallery.html"

run_generator \
  "readiness JSON" \
  json \
  "openpr.universal_forms.readiness.v1" \
  "$TMP_DIR/readiness.json" \
  "$ROOT_DIR/scripts/report-universal-forms-readiness-json.sh" --output "$TMP_DIR/readiness.json"

run_generator \
  "development status JSON" \
  json \
  "openpr.universal_forms.development_status.v1" \
  "$TMP_DIR/development-status.json" \
  "$ROOT_DIR/scripts/report-universal-forms-development-status-json.sh" --output "$TMP_DIR/development-status.json"

run_generator \
  "scenario catalog JSON" \
  json \
  "openpr.universal_forms.scenario_catalog.v1" \
  "$TMP_DIR/scenario-catalog.json" \
  "$ROOT_DIR/scripts/report-universal-forms-scenario-catalog-json.sh" --output "$TMP_DIR/scenario-catalog.json"

run_generator \
  "implementation map JSON" \
  json \
  "openpr.universal_forms.implementation_map.v1" \
  "$TMP_DIR/implementation-map.json" \
  "$ROOT_DIR/scripts/report-universal-forms-implementation-map-json.sh" --output "$TMP_DIR/implementation-map.json"

run_generator \
  "delivery manifest" \
  markdown \
  "# OpenPR Universal Forms Delivery Manifest" \
  "$TMP_DIR/delivery-manifest.md" \
  "$ROOT_DIR/scripts/prepare-universal-forms-delivery-manifest.sh" --output "$TMP_DIR/delivery-manifest.md"

run_generator \
  "delivery manifest JSON" \
  json \
  "openpr.universal_forms.delivery_manifest.v1" \
  "$TMP_DIR/delivery-manifest.json" \
  "$ROOT_DIR/scripts/report-universal-forms-delivery-manifest-json.sh" --manifest "$TMP_DIR/delivery-manifest.md" --output "$TMP_DIR/delivery-manifest.json"

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms report output boundary smoke failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms report output boundary smoke passed.\n'
