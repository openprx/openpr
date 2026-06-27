#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
READINESS_JSON_PATH="$REPORT_DIR/openpr-universal-form-readiness-2026-05-31.json"
DEVELOPMENT_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-development-status-2026-05-31.json"
SIGNOFF_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-release-gate.schema.json"
JSON_PATH="${1:-}"
GENERATED_TMP=""

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-release-gate-json.sh [JSON_PATH]

Verifies the machine-readable release gate JSON emitted by
scripts/gate-universal-forms-release.sh --allow-pending --json. When JSON_PATH
is omitted, the verifier generates a temporary pre-signoff release gate JSON
from the current handoff bundle.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

cleanup() {
  if [[ -n "$GENERATED_TMP" ]]; then
    rm -f "$GENERATED_TMP"
  fi
}
trap cleanup EXIT

if [[ -z "$JSON_PATH" ]]; then
  GENERATED_TMP="$(mktemp /tmp/openpr-uf-release-gate.XXXXXX.json)"
  "$ROOT_DIR/scripts/gate-universal-forms-release.sh" --allow-pending --json >"$GENERATED_TMP"
  JSON_PATH="$GENERATED_TMP"
fi

failures=0

pass() {
  printf 'PASS: %s\n' "$1"
}

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  failures=$((failures + 1))
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

require_file() {
  local path="$1"
  if [[ -f "$path" ]]; then
    pass "file exists: $path"
  else
    fail "file missing: $path"
  fi
}

json_value() {
  jq -r "$1" "$JSON_PATH"
}

printf 'Universal forms release gate JSON verification\n'
printf '  JSON: %s\n' "$JSON_PATH"
printf '\n'

if command -v jq >/dev/null 2>&1; then
  pass "jq is available"
else
  fail "jq is available"
fi

for path in \
  "$JSON_PATH" \
  "$SCHEMA_PATH" \
  "$READINESS_JSON_PATH" \
  "$DEVELOPMENT_STATUS_JSON_PATH" \
  "$SIGNOFF_STATUS_JSON_PATH" \
  "$EVIDENCE_PATH" \
  "$RUNBOOK_PATH"; do
  require_file "$path"
done

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms release gate JSON verification failed before consistency checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

if jq empty "$JSON_PATH" >/dev/null; then
  pass "release gate JSON is valid JSON"
else
  fail "release gate JSON is valid JSON"
fi
if jq empty "$SCHEMA_PATH" >/dev/null; then
  pass "release gate JSON schema is valid JSON"
else
  fail "release gate JSON schema is valid JSON"
fi

equals "schema version is v1" "$(json_value '.schema_version')" "openpr.universal_forms.release_gate.v1"
equals "JSON schema path matches repository schema" "$(json_value '.schema_path')" "$SCHEMA_PATH"
equals "schema file pins release gate JSON v1" "$(jq -r '.properties.schema_version.const' "$SCHEMA_PATH")" "openpr.universal_forms.release_gate.v1"
equals "schema file disallows top-level additional properties" "$(jq -r '.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file disallows reports additional properties" "$(jq -r '.properties.reports.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file disallows tracker status additional properties" "$(jq -r '.properties.tracker_status.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file enumerates gate modes" "$(jq -r '.properties.mode.enum | sort | join(",")' "$SCHEMA_PATH")" "blocked,pre_signoff,release"
equals "schema file enumerates seven manual signoff keys" "$(jq -r '.["$defs"].manual_key.enum | length' "$SCHEMA_PATH")" "7"
equals "schema file constrains next manual key to manual key enum" "$(jq -r '.properties.next_manual_signoff_key.anyOf[]? | select(.["$ref"] == "#/$defs/manual_key") | .["$ref"]' "$SCHEMA_PATH")" '#/$defs/manual_key'
equals "schema file allows empty final next manual key" "$(jq -r '.properties.next_manual_signoff_key.anyOf[]? | select(.const == "") | .const' "$SCHEMA_PATH")" ""
equals "JSON has every schema top-level required key" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].required - (keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON has no extra top-level keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(keys_unsorted - ($schema[0].properties | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON reports object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.reports.required - (.reports | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON reports object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.reports | keys_unsorted) - ($schema[0].properties.reports.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON tracker status object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.tracker_status.required - (.tracker_status | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON tracker status object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.tracker_status | keys_unsorted) - ($schema[0].properties.tracker_status.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON mode is allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '.mode as $mode | $schema[0].properties.mode.enum | index($mode) != null' "$JSON_PATH")" "true"
equals "JSON next manual key is allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '.next_manual_signoff_key as $key | ($key == "" or ($schema[0].["$defs"].manual_key.enum | index($key) != null))' "$JSON_PATH")" "true"
equals "integer counters are typed as numbers" "$(jq -r '[
  (.automated_checks | type),
  (.failed_automated_checks | type),
  (.non_manual_unresolved_items | type),
  (.manual_signoff_pending_rows | type)
] | unique | join(",")' "$JSON_PATH")" "number"
equals "boolean flags are typed as booleans" "$(jq -r '[
  (.release_allowed | type),
  (.final_acceptance_complete | type),
  (.manual_final_signoff_allowed | type),
  (.development_final_release_allowed | type)
] | unique | join(",")' "$JSON_PATH")" "boolean"

equals "readiness JSON path matches current report" "$(json_value '.reports.readiness_json')" "$READINESS_JSON_PATH"
equals "development status JSON path matches current report" "$(json_value '.reports.development_status_json')" "$DEVELOPMENT_STATUS_JSON_PATH"
equals "signoff status JSON path matches current report" "$(json_value '.reports.signoff_status_json')" "$SIGNOFF_STATUS_JSON_PATH"
equals "evidence path matches current report" "$(json_value '.reports.evidence')" "$EVIDENCE_PATH"
equals "runbook path matches current report" "$(json_value '.reports.runbook')" "$RUNBOOK_PATH"

equals "stage matches readiness JSON" "$(json_value '.stage')" "$(jq -r '.stage' "$READINESS_JSON_PATH")"
equals "automated check count matches readiness JSON" "$(json_value '.automated_checks')" "$(jq -r '.gates.automated_checks' "$READINESS_JSON_PATH")"
equals "failed automated check count matches readiness JSON" "$(json_value '.failed_automated_checks')" "$(jq -r '.gates.failed_automated_checks' "$READINESS_JSON_PATH")"
equals "non-manual unresolved count matches readiness JSON" "$(json_value '.non_manual_unresolved_items')" "$(jq -r '.gates.tracker_non_manual_unresolved_items' "$READINESS_JSON_PATH")"
equals "manual pending rows match readiness JSON" "$(json_value '.manual_signoff_pending_rows')" "$(jq -r '.manual_signoff.pending_rows' "$READINESS_JSON_PATH")"
equals "next manual key matches readiness JSON" "$(json_value '.next_manual_signoff_key')" "$(jq -r '.manual_signoff.next_row.key // ""' "$READINESS_JSON_PATH")"
equals "final acceptance flag matches readiness JSON" "$(json_value '.final_acceptance_complete')" "$(jq -r '.final_acceptance_complete' "$READINESS_JSON_PATH")"
equals "tracker end-to-end status matches readiness JSON" "$(json_value '.tracker_status.end_to_end_acceptance')" "$(jq -r '.tracker_status.end_to_end_acceptance' "$READINESS_JSON_PATH")"
equals "tracker manual status matches readiness JSON" "$(json_value '.tracker_status.user_side_manual_acceptance')" "$(jq -r '.tracker_status.user_side_manual_acceptance' "$READINESS_JSON_PATH")"
equals "manual final flag matches signoff status JSON" "$(json_value '.manual_final_signoff_allowed')" "$(jq -r '.manual_signoff.final_signoff_allowed' "$SIGNOFF_STATUS_JSON_PATH")"
equals "development final flag matches development status JSON" "$(json_value '.development_final_release_allowed')" "$(jq -r '.status_summary.final_release_allowed' "$DEVELOPMENT_STATUS_JSON_PATH")"

expected_release_allowed=false
if [[ "$(json_value '.failed_automated_checks')" == "0" && "$(json_value '.non_manual_unresolved_items')" == "0" && "$(json_value '.manual_signoff_pending_rows')" == "0" && "$(json_value '.final_acceptance_complete')" == "true" && "$(json_value '.manual_final_signoff_allowed')" == "true" && "$(json_value '.development_final_release_allowed')" == "true" && "$(json_value '.tracker_status.end_to_end_acceptance')" == "已验收" && "$(json_value '.tracker_status.user_side_manual_acceptance')" == "已验收" ]]; then
  expected_release_allowed=true
fi
equals "release allowed flag matches final state" "$(json_value '.release_allowed')" "$expected_release_allowed"

mode="$(json_value '.mode')"
reason="$(json_value '.reason')"
if [[ "$expected_release_allowed" == "true" ]]; then
  equals "release mode is final when allowed" "$mode" "release"
  equals "release reason is final when allowed" "$reason" "final release gate passed"
elif [[ "$(json_value '.failed_automated_checks')" != "0" || "$(json_value '.non_manual_unresolved_items')" != "0" ]]; then
  equals "blocked reason reports automated/non-manual failure" "$reason" "automated or non-manual development gates are not green"
elif [[ "$(json_value '.manual_signoff_pending_rows')" != "0" && "$mode" == "pre_signoff" ]]; then
  equals "pre-signoff reason reports pending manual signoff" "$reason" "pre-release handoff is ready; user-side manual signoff is still pending"
elif [[ "$(json_value '.manual_signoff_pending_rows')" != "0" ]]; then
  equals "blocked reason reports pending manual signoff" "$reason" "user-side manual signoff is incomplete"
else
  equals "blocked reason reports incomplete finalization" "$reason" "manual signoff is complete but tracker finalization is incomplete"
fi

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms release gate JSON verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms release gate JSON verification passed.\n'
