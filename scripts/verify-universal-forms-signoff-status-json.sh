#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
JSON_PATH="${1:-$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json}"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
MANUAL_EVIDENCE_MAP_PATH="$REPORT_DIR/openpr-universal-form-manual-evidence-map-2026-05-31.md"
MARKDOWN_STATUS_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.md"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-signoff-status.schema.json"

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-signoff-status-json.sh [JSON_PATH]

Verifies the machine-readable manual signoff status JSON against the acceptance
evidence, manual evidence map, Markdown signoff status report, and repository
schema.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
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

trim() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  printf '%s' "$value"
}

normalize_status() {
  local raw
  raw="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
  raw="$(trim "$raw")"
  case "$raw" in
    accepted|approved|passed|pass|通过|已通过|已验收)
      printf 'accepted'
      ;;
    pending|待验收|'')
      printf 'pending'
      ;;
    failed|fail|失败)
      printf 'failed'
      ;;
    rework|'needs rework'|需整改)
      printf 'rework'
      ;;
    *)
      printf 'unknown'
      ;;
  esac
}

manual_row_cell() {
  local item="$1"
  local column="$2"
  awk -F'|' -v expected="$item" -v column="$column" '
    /^## Manual Acceptance Signoff$/ { in_section = 1; next }
    /^## / && in_section { exit }
    in_section && NF >= 5 {
      current = $2
      value = $(column)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", current)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
      if (current == expected) {
        print value
        exit
      }
    }
  ' "$EVIDENCE_PATH"
}

suggested_note_for() {
  local item="$1"
  awk -F'|' -v expected="$item" '
    /^## Evidence Map$/ { in_section = 1; next }
    /^## / && in_section { exit }
    in_section && NF >= 6 {
      current = $2
      note = $5
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", current)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", note)
      if (current == expected) {
        gsub(/^`|`$/, "", note)
        print note
        exit
      }
    }
  ' "$MANUAL_EVIDENCE_MAP_PATH"
}

evidence_map_cell() {
  local item="$1"
  local column="$2"
  awk -F'|' -v expected="$item" -v column="$column" '
    /^## Evidence Map$/ { in_section = 1; next }
    /^## / && in_section { exit }
    in_section && NF >= 6 {
      current = $2
      value = $(column)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", current)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
      if (current == expected) {
        print value
        exit
      }
    }
  ' "$MANUAL_EVIDENCE_MAP_PATH"
}

printf 'Universal forms signoff status JSON verification\n'
printf '  JSON: %s\n' "$JSON_PATH"
printf '\n'

if command -v jq >/dev/null 2>&1; then
  pass "jq is available"
else
  fail "jq is available"
fi

for path in "$JSON_PATH" "$EVIDENCE_PATH" "$RUNBOOK_PATH" "$MANUAL_EVIDENCE_MAP_PATH" "$MARKDOWN_STATUS_PATH" "$SCHEMA_PATH"; do
  require_file "$path"
done

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms signoff status JSON verification failed before consistency checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

if jq empty "$JSON_PATH" >/dev/null; then
  pass "signoff status JSON is valid JSON"
else
  fail "signoff status JSON is valid JSON"
fi
if jq empty "$SCHEMA_PATH" >/dev/null; then
  pass "signoff status JSON schema is valid JSON"
else
  fail "signoff status JSON schema is valid JSON"
fi

keys=(
  restaurant_template
  frontend_usability
  amounts
  workflow
  hub_consistency
  docs
  overall
)
items=(
  "Restaurant template can create a project directly"
  "Universal forms frontend is usable by a non-technical operator"
  "Amount, quantity, and subtotal behavior is acceptable"
  "Order, order line, table change, print, and report workflow is acceptable"
  "MCP/API/Webhook/Connector consistency is acceptable"
  "README/docs are sufficient for a new user to reproduce"
  "Overall acceptance"
)

summary_total="$(sed -n 's/^- Total automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
summary_failed="$(sed -n 's/^- Failed automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
pass_count="$(rg -c '^\*\*Status:\*\* PASS$' "$EVIDENCE_PATH" || true)"
manual_row_count="$(awk -F'|' '
  /^## Manual Acceptance Signoff$/ { in_section = 1; next }
  /^## / && in_section { exit }
  in_section && NF >= 5 {
    item = $2
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", item)
    if (item != "" && item != "Item" && item !~ /^-+$/) {
      count += 1
    }
  }
  END { print count + 0 }
' "$EVIDENCE_PATH")"

expected_accepted=0
expected_pending=0
expected_blocked=0
expected_next_key=""
expected_next_item=""
expected_next_status="pending"
expected_next_automated_evidence=""
expected_next_reviewer_check=""
expected_next_note=""
for index in "${!keys[@]}"; do
  item="${items[$index]}"
  status="$(normalize_status "$(manual_row_cell "$item" 3)")"
  if [[ "$status" == "accepted" ]]; then
    expected_accepted=$((expected_accepted + 1))
  else
    expected_pending=$((expected_pending + 1))
    if [[ -z "$expected_next_key" ]]; then
      expected_next_key="${keys[$index]}"
      expected_next_item="$item"
      expected_next_status="$status"
      expected_next_automated_evidence="$(evidence_map_cell "$item" 3)"
      expected_next_reviewer_check="$(evidence_map_cell "$item" 4)"
      expected_next_note="$(suggested_note_for "$item")"
    fi
    if [[ "$status" == "failed" || "$status" == "rework" || "$status" == "unknown" ]]; then
      expected_blocked=$((expected_blocked + 1))
    fi
  fi
done
if [[ -z "$expected_next_automated_evidence" && -n "$expected_next_key" ]]; then
  expected_next_automated_evidence="missing"
fi
if [[ -z "$expected_next_reviewer_check" && -n "$expected_next_key" ]]; then
  expected_next_reviewer_check="missing"
fi
if [[ -z "$expected_next_note" && -n "$expected_next_key" ]]; then
  expected_next_note="<review note>"
fi

expected_final_allowed=false
if [[ "${summary_failed:-missing}" == "0" && "$expected_pending" -eq 0 && "$expected_blocked" -eq 0 && "$(json_value '.gate_summary.runbook_evidence_consistency')" == "passed" ]]; then
  expected_final_allowed=true
fi

markdown_next_key="$(sed -n 's/^Next row: `\([^`]*\)` -.*/\1/p' "$MARKDOWN_STATUS_PATH" | head -n 1)"
markdown_next_note="$(
  awk '
    /^Suggested evidence note:$/ { want_fence = 1; next }
    want_fence && /^```/ { in_block = 1; want_fence = 0; next }
    in_block && /^```/ { exit }
    in_block { print }
  ' "$MARKDOWN_STATUS_PATH"
)"
markdown_next_command="$(
  awk '
    /^Recorder command after reviewer approval:$/ { want_fence = 1; next }
    want_fence && /^```/ { in_block = 1; want_fence = 0; next }
    in_block && /^```/ { exit }
    in_block { print }
  ' "$MARKDOWN_STATUS_PATH"
)"

equals "schema version is v1" "$(json_value '.schema_version')" "openpr.universal_forms.signoff_status.v1"
equals "JSON schema path matches repository schema" "$(json_value '.schema_path')" "$SCHEMA_PATH"
equals "schema file pins signoff status JSON v1" "$(jq -r '.properties.schema_version.const' "$SCHEMA_PATH")" "openpr.universal_forms.signoff_status.v1"
equals "schema file disallows top-level additional properties" "$(jq -r '.additionalProperties' "$SCHEMA_PATH")" "false"
equals "schema file enumerates seven manual signoff keys" "$(jq -r '.["$defs"].manual_key.enum | length' "$SCHEMA_PATH")" "7"
equals "schema file reuses manual key enum for rows" "$(jq -r '.["$defs"].manual_row.properties.key["$ref"]' "$SCHEMA_PATH")" '#/$defs/manual_key'
equals "schema file constrains next row key to manual key enum" "$(jq -r '.properties.manual_signoff.properties.next_row.properties.key.anyOf[]? | select(.["$ref"] == "#/$defs/manual_key") | .["$ref"]' "$SCHEMA_PATH")" '#/$defs/manual_key'
equals "schema file allows empty next row key after final signoff" "$(jq -r '.properties.manual_signoff.properties.next_row.properties.key.anyOf[]? | select(.const == "") | has("const")' "$SCHEMA_PATH")" "true"
equals "schema file requires final signoff flag" "$(jq -r '.properties.manual_signoff.required | index("final_signoff_allowed") != null' "$SCHEMA_PATH")" "true"
equals "schema file requires per-row recorder command" "$(jq -r '.["$defs"].manual_row.required | index("recorder_command") != null' "$SCHEMA_PATH")" "true"
equals "schema file requires per-row automated evidence" "$(jq -r '.["$defs"].manual_row.required | index("automated_evidence") != null' "$SCHEMA_PATH")" "true"
equals "schema file requires per-row reviewer check" "$(jq -r '.["$defs"].manual_row.required | index("reviewer_check") != null' "$SCHEMA_PATH")" "true"
equals "schema file requires next-row automated evidence" "$(jq -r '.properties.manual_signoff.properties.next_row.required | index("automated_evidence") != null' "$SCHEMA_PATH")" "true"
equals "schema file requires next-row reviewer check" "$(jq -r '.properties.manual_signoff.properties.next_row.required | index("reviewer_check") != null' "$SCHEMA_PATH")" "true"
equals "schema file requires pending review queue" "$(jq -r '.properties.manual_signoff.required | index("pending_queue") != null' "$SCHEMA_PATH")" "true"
equals "schema file defines pending queue row shape" "$(jq -r '.["$defs"].pending_queue_row.required | index("review_order") != null and index("actionable") != null' "$SCHEMA_PATH")" "true"
equals "schema file constrains pending queue key to manual key enum" "$(jq -r '.["$defs"].pending_queue_row.properties.key["$ref"]' "$SCHEMA_PATH")" '#/$defs/manual_key'
equals "schema file pins manual row order length" "$(jq -r '.properties.manual_signoff.properties.rows.prefixItems | length' "$SCHEMA_PATH")" "7"
equals "schema file pins manual row order" "$(jq -r '[.properties.manual_signoff.properties.rows.prefixItems[].allOf[1].properties.key.const] | join(",")' "$SCHEMA_PATH")" "$(IFS=,; printf '%s' "${keys[*]}")"
equals "schema file disallows extra manual rows" "$(jq -r '.properties.manual_signoff.properties.rows.items' "$SCHEMA_PATH")" "false"
equals "JSON has every schema top-level required key" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].required - (keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON has no extra top-level keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(keys_unsorted - ($schema[0].properties | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON reports object matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.reports.required - (.reports | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON reports object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.reports | keys_unsorted) - ($schema[0].properties.reports.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON gate summary matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.gate_summary.required - (.gate_summary | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON gate summary object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.gate_summary | keys_unsorted) - ($schema[0].properties.gate_summary.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON manual signoff matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.manual_signoff.required - (.manual_signoff | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON manual signoff object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.manual_signoff | keys_unsorted) - ($schema[0].properties.manual_signoff.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON next row matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.manual_signoff.properties.next_row.required - (.manual_signoff.next_row | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON next row object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.manual_signoff.next_row | keys_unsorted) - ($schema[0].properties.manual_signoff.properties.next_row.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON pending queue rows have schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.manual_signoff.pending_queue[]; (($schema[0].["$defs"].pending_queue_row.required - (keys_unsorted)) | length) == 0)' "$JSON_PATH")" "true"
equals "JSON pending queue rows have no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.manual_signoff.pending_queue[]; ((keys_unsorted - ($schema[0].["$defs"].pending_queue_row.properties | keys_unsorted)) | length) == 0)' "$JSON_PATH")" "true"
equals "JSON release requirement matches schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '($schema[0].properties.release_requirement.required - (.release_requirement | keys_unsorted)) | join(",")' "$JSON_PATH")" ""
equals "JSON release requirement object has no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '(.release_requirement | keys_unsorted) - ($schema[0].properties.release_requirement.properties | keys_unsorted) | join(",")' "$JSON_PATH")" ""
equals "JSON manual rows have schema required keys" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.manual_signoff.rows[]; (($schema[0].["$defs"].manual_row.required - (keys_unsorted)) | length) == 0)' "$JSON_PATH")" "true"
equals "JSON manual rows have no extra keys beyond schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" 'all(.manual_signoff.rows[]; ((keys_unsorted - ($schema[0].["$defs"].manual_row.properties | keys_unsorted)) | length) == 0)' "$JSON_PATH")" "true"
equals "JSON manual row keys are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '([.manual_signoff.rows[].key] - $schema[0].["$defs"].manual_key.enum) | join(",")' "$JSON_PATH")" ""
equals "JSON manual row keys exactly match schema enum" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '([.manual_signoff.rows[].key] | sort) == ($schema[0].["$defs"].manual_key.enum | sort)' "$JSON_PATH")" "true"
equals "JSON manual row keys match schema order" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.manual_signoff.rows[].key] == [$schema[0].properties.manual_signoff.properties.rows.prefixItems[].allOf[1].properties.key.const]' "$JSON_PATH")" "true"
equals "JSON next row key is allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '.manual_signoff.next_row.key as $key | ($key == "" or ($schema[0].["$defs"].manual_key.enum | index($key) != null))' "$JSON_PATH")" "true"
equals "JSON manual row statuses are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.manual_signoff.rows[].status] as $statuses | ($schema[0].["$defs"].manual_status.enum) as $allowed | all($statuses[]; $allowed | index(.) != null)' "$JSON_PATH")" "true"
equals "JSON pending queue keys are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '([.manual_signoff.pending_queue[].key] - $schema[0].["$defs"].manual_key.enum) | join(",")' "$JSON_PATH")" ""
equals "JSON pending queue statuses are allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '[.manual_signoff.pending_queue[].status] as $statuses | ($schema[0].["$defs"].manual_status.enum) as $allowed | all($statuses[]; $allowed | index(.) != null)' "$JSON_PATH")" "true"
equals "JSON gate status is allowed by schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '.gate_summary.runbook_evidence_consistency as $status | $schema[0].["$defs"].gate_status.enum | index($status) != null' "$JSON_PATH")" "true"
equals "JSON counters are integers" "$(jq -r '[.gate_summary.automated_checks, .gate_summary.pass_status_lines, .gate_summary.failed_automated_checks, .manual_signoff.total_rows, .manual_signoff.accepted_rows, .manual_signoff.pending_rows, .manual_signoff.blocked_rows] | all(type == "number" and floor == .)' "$JSON_PATH")" "true"
equals "JSON final signoff flag is boolean" "$(jq -r '.manual_signoff.final_signoff_allowed | type' "$JSON_PATH")" "boolean"
equals "JSON next actionable flag is boolean" "$(jq -r '.manual_signoff.next_row.actionable | type' "$JSON_PATH")" "boolean"
equals "JSON release requirement constants match schema" "$(jq -r --slurpfile schema "$SCHEMA_PATH" '.release_requirement as $release | $schema[0].properties.release_requirement.properties as $props | ($release.failed_automated_checks == $props.failed_automated_checks.const and $release.runbook_evidence_consistency == $props.runbook_evidence_consistency.const and $release.pending_rows == $props.pending_rows.const and $release.blocked_rows == $props.blocked_rows.const)' "$JSON_PATH")" "true"

equals "JSON evidence path matches default evidence" "$(json_value '.reports.evidence')" "$EVIDENCE_PATH"
equals "JSON runbook path matches default runbook" "$(json_value '.reports.runbook')" "$RUNBOOK_PATH"
equals "JSON manual evidence map path matches default map" "$(json_value '.reports.manual_evidence_map')" "$MANUAL_EVIDENCE_MAP_PATH"
equals "JSON Markdown status path matches default report" "$(json_value '.reports.markdown_status')" "$MARKDOWN_STATUS_PATH"
equals "JSON automated checks match evidence" "$(json_value '.gate_summary.automated_checks')" "$summary_total"
equals "JSON PASS status lines match evidence" "$(json_value '.gate_summary.pass_status_lines')" "$pass_count"
equals "JSON failed checks match evidence" "$(json_value '.gate_summary.failed_automated_checks')" "$summary_failed"
equals "JSON manual total rows match evidence" "$(json_value '.manual_signoff.total_rows')" "$manual_row_count"
equals "JSON accepted rows match evidence" "$(json_value '.manual_signoff.accepted_rows')" "$expected_accepted"
equals "JSON pending rows match evidence" "$(json_value '.manual_signoff.pending_rows')" "$expected_pending"
equals "JSON blocked rows match evidence" "$(json_value '.manual_signoff.blocked_rows')" "$expected_blocked"
equals "JSON final signoff flag matches evidence" "$(json_value '.manual_signoff.final_signoff_allowed')" "$expected_final_allowed"
equals "JSON pending queue length matches evidence" "$(json_value '.manual_signoff.pending_queue | length')" "$expected_pending"
equals "JSON pending queue mirrors pending manual rows" "$(jq -r '[.manual_signoff.rows | to_entries[] | select(.value.status != "accepted") | .value.key] == [.manual_signoff.pending_queue[].key]' "$JSON_PATH")" "true"
equals "JSON pending queue review order mirrors row positions" "$(jq -r '[.manual_signoff.rows | to_entries[] | select(.value.status != "accepted") | .key + 1] == [.manual_signoff.pending_queue[].review_order]' "$JSON_PATH")" "true"
equals "JSON pending queue excludes accepted rows" "$(jq -r 'all(.manual_signoff.pending_queue[]; .status != "accepted")' "$JSON_PATH")" "true"
equals "JSON pending queue next marker matches next row" "$(jq -r 'if (.manual_signoff.pending_queue | length) == 0 then .manual_signoff.next_row.key == "" else ([.manual_signoff.pending_queue[] | select(.is_next == true) | .key] == [.manual_signoff.next_row.key]) end' "$JSON_PATH")" "true"
equals "JSON pending queue has at most one actionable row" "$(jq -r '[.manual_signoff.pending_queue[] | select(.actionable == true)] | length <= 1' "$JSON_PATH")" "true"
equals "JSON pending queue actionable row mirrors next actionable" "$(jq -r 'if (.manual_signoff.pending_queue | length) == 0 then true else ([.manual_signoff.pending_queue[] | select(.actionable == true) | .key] == (if .manual_signoff.next_row.actionable then [.manual_signoff.next_row.key] else [] end)) end' "$JSON_PATH")" "true"
equals "JSON pending queue first row mirrors next row key" "$(jq -r 'if (.manual_signoff.pending_queue | length) == 0 then .manual_signoff.next_row.key == "" else .manual_signoff.pending_queue[0].key == .manual_signoff.next_row.key end' "$JSON_PATH")" "true"
equals "JSON pending queue first row mirrors next row evidence" "$(jq -r 'if (.manual_signoff.pending_queue | length) == 0 then true else (.manual_signoff.pending_queue[0].automated_evidence == .manual_signoff.next_row.automated_evidence and .manual_signoff.pending_queue[0].reviewer_check == .manual_signoff.next_row.reviewer_check and .manual_signoff.pending_queue[0].suggested_evidence_note == .manual_signoff.next_row.suggested_evidence_note and .manual_signoff.pending_queue[0].recorder_command == .manual_signoff.next_row.recorder_command) end' "$JSON_PATH")" "true"
equals "JSON next row key matches evidence" "$(json_value '.manual_signoff.next_row.key')" "$expected_next_key"
equals "JSON next row item matches evidence" "$(json_value '.manual_signoff.next_row.item')" "$expected_next_item"
equals "JSON next row status matches evidence" "$(json_value '.manual_signoff.next_row.status')" "$expected_next_status"
equals "JSON next row automated evidence matches evidence map" "$(json_value '.manual_signoff.next_row.automated_evidence')" "$expected_next_automated_evidence"
equals "JSON next row reviewer check matches evidence map" "$(json_value '.manual_signoff.next_row.reviewer_check')" "$expected_next_reviewer_check"
equals "JSON next row note matches evidence map" "$(json_value '.manual_signoff.next_row.suggested_evidence_note')" "$expected_next_note"
equals "JSON next row key matches Markdown status" "$(json_value '.manual_signoff.next_row.key')" "$markdown_next_key"
equals "JSON next row note matches Markdown status" "$(json_value '.manual_signoff.next_row.suggested_evidence_note')" "$markdown_next_note"
equals "JSON next row command matches Markdown status" "$(json_value '.manual_signoff.next_row.recorder_command')" "$markdown_next_command"

for index in "${!keys[@]}"; do
  key="${keys[$index]}"
  item="${items[$index]}"
  raw_status="$(manual_row_cell "$item" 3)"
  normalized="$(normalize_status "$raw_status")"
  automated_evidence="$(evidence_map_cell "$item" 3)"
  reviewer_check="$(evidence_map_cell "$item" 4)"
  suggested_note="$(suggested_note_for "$item")"
  if [[ -z "$automated_evidence" ]]; then
    automated_evidence="missing"
  fi
  if [[ -z "$reviewer_check" ]]; then
    reviewer_check="missing"
  fi
  if [[ -z "$suggested_note" ]]; then
    suggested_note="<review note>"
  fi
  expected_command="scripts/record-universal-forms-manual-signoff.sh --item $key --status accepted --reviewer '<name>' --evidence '$(printf '%s' "$suggested_note" | sed "s/'/'\\\\''/g")'"
  equals "JSON row status mirrors evidence: $key" "$(jq -r --arg key "$key" '.manual_signoff.rows[] | select(.key == $key) | .status' "$JSON_PATH")" "$normalized"
  equals "JSON row raw status mirrors evidence: $key" "$(jq -r --arg key "$key" '.manual_signoff.rows[] | select(.key == $key) | .raw_status' "$JSON_PATH")" "${raw_status:-missing}"
  equals "JSON row automated evidence mirrors evidence map: $key" "$(jq -r --arg key "$key" '.manual_signoff.rows[] | select(.key == $key) | .automated_evidence' "$JSON_PATH")" "$automated_evidence"
  equals "JSON row reviewer check mirrors evidence map: $key" "$(jq -r --arg key "$key" '.manual_signoff.rows[] | select(.key == $key) | .reviewer_check' "$JSON_PATH")" "$reviewer_check"
  equals "JSON row suggested note mirrors evidence map: $key" "$(jq -r --arg key "$key" '.manual_signoff.rows[] | select(.key == $key) | .suggested_evidence_note' "$JSON_PATH")" "$suggested_note"
  equals "JSON row recorder command mirrors evidence map: $key" "$(jq -r --arg key "$key" '.manual_signoff.rows[] | select(.key == $key) | .recorder_command' "$JSON_PATH")" "$expected_command"
  equals "JSON row recorder command targets row: $key" "$(jq -r --arg key "$key" '.manual_signoff.rows[] | select(.key == $key) | .recorder_command | contains("--item " + $key)' "$JSON_PATH")" "true"
  if [[ "$normalized" != "accepted" ]]; then
    equals "JSON pending queue mirrors row: $key" "$(jq -r --arg key "$key" '
      (.manual_signoff.rows[] | select(.key == $key)) as $row
      | (.manual_signoff.pending_queue[] | select(.key == $key)) as $queue
      | ($queue.item == $row.item and
         $queue.status == $row.status and
         $queue.automated_evidence == $row.automated_evidence and
         $queue.reviewer_check == $row.reviewer_check and
         $queue.suggested_evidence_note == $row.suggested_evidence_note and
         $queue.recorder_command == $row.recorder_command)
    ' "$JSON_PATH")" "true"
  fi
done

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms signoff status JSON verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms signoff status JSON verification passed.\n'
