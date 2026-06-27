#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
TRACKER_PATH="$REPORT_DIR/openpr-universal-form-development-execution-tracker-2026-05-31.md"
READINESS_JSON_PATH="$REPORT_DIR/openpr-universal-form-readiness-2026-05-31.json"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-development-status.schema.json"
OUTPUT_PATH="${OPENPR_DEVELOPMENT_STATUS_JSON_REPORT:-$REPORT_DIR/openpr-universal-form-development-status-2026-05-31.json}"

usage() {
  cat <<'EOF'
Usage: scripts/report-universal-forms-development-status-json.sh [--output PATH]

Generates a machine-readable JSON view of the development readiness matrix in
the universal forms execution tracker. The report is read-only and does not
change tracker or signoff state.

Environment:
  OPENPR_DEVELOPMENT_STATUS_JSON_REPORT  Optional output path.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
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

require_file() {
  local path="$1"
  if [[ ! -f "$path" ]]; then
    echo "Missing required file: $path" >&2
    exit 1
  fi
}

require_command() {
  local name="$1"
  if ! command -v "$name" >/dev/null 2>&1; then
    echo "Missing required command: $name" >&2
    exit 2
  fi
}

phase_key() {
  case "$1" in
    "需求/协议") printf 'requirements_protocol' ;;
    "数据库/模型") printf 'database_model' ;;
    "后端 API") printf 'backend_api' ;;
    "MCP") printf 'mcp' ;;
    "Webhook/Connector") printf 'webhook_connector' ;;
    "WASM 插件") printf 'wasm_plugin' ;;
    "前端 UI") printf 'frontend_ui' ;;
    "场景模板") printf 'scenario_template' ;;
    "交付包") printf 'delivery_bundle' ;;
    "用户侧人工验收") printf 'user_manual_acceptance' ;;
    *)
      echo "Unknown development matrix phase: $1" >&2
      exit 1
      ;;
  esac
}

require_command jq
require_file "$TRACKER_PATH"
require_file "$READINESS_JSON_PATH"
require_file "$SCHEMA_PATH"

rows_tsv="$(mktemp)"
output_tmp=""
trap 'rm -f "$rows_tsv" "$output_tmp"' EXIT

awk -F'|' '
  /^## 开发阶段准入与交付判定矩阵$/ { seen_heading = 1; next }
  seen_heading && /^\| 环节 \|/ { in_table = 1; next }
  in_table && /^\| --- / { next }
  in_table && /^$/ { exit }
  in_table && NF >= 6 {
    phase = $2
    requirement = $3
    completion = $4
    evidence = $5
    status = $6
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", phase)
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", requirement)
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", completion)
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", evidence)
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", status)
    if (phase != "") {
      print phase "\t" requirement "\t" completion "\t" evidence "\t" status
    }
  }
' "$TRACKER_PATH" >"$rows_tsv"

row_count="$(wc -l <"$rows_tsv" | tr -d ' ')"
if [[ "$row_count" != "10" ]]; then
  echo "Expected 10 development matrix rows, got $row_count" >&2
  exit 1
fi

rows_json="$(
  while IFS=$'\t' read -r phase requirement completion evidence status; do
    key="$(phase_key "$phase")"
    manual_gate=false
    if [[ "$key" == "user_manual_acceptance" ]]; then
      manual_gate=true
    fi
    jq -n \
      --arg key "$key" \
      --arg phase "$phase" \
      --arg status "$status" \
      --arg requirement "$requirement" \
      --arg completion "$completion" \
      --arg evidence "$evidence" \
      --argjson manual_gate "$manual_gate" \
      '{
        key: $key,
        phase: $phase,
        status: $status,
        engineering_requirement: $requirement,
        completion_rule: $completion,
        evidence: $evidence,
        manual_gate: $manual_gate
      }'
  done <"$rows_tsv" | jq -s '.'
)"

tested_or_accepted_rows="$(jq -r '[.[] | select(.status == "已测试" or .status == "已验收")] | length' <<<"$rows_json")"
accepted_rows="$(jq -r '[.[] | select(.status == "已验收")] | length' <<<"$rows_json")"
pending_rows="$(jq -r '[.[] | select(.status == "待处理")] | length' <<<"$rows_json")"
non_manual_unresolved_rows="$(jq -r '[.[] | select(.key != "user_manual_acceptance" and (.status != "已测试" and .status != "已验收"))] | length' <<<"$rows_json")"
manual_pending_rows="$(jq -r '.manual_signoff.pending_rows' "$READINESS_JSON_PATH")"
failed_automated_checks="$(jq -r '.gates.failed_automated_checks' "$READINESS_JSON_PATH")"
manual_tracker_status="$(jq -r '.tracker_status.user_side_manual_acceptance' "$READINESS_JSON_PATH")"
final_release_allowed=false
if [[ "$failed_automated_checks" == "0" && "$non_manual_unresolved_rows" == "0" && "$manual_pending_rows" == "0" && "$manual_tracker_status" == "已验收" ]]; then
  final_release_allowed=true
fi

next_action="Run user-side manual acceptance, record all seven signoff rows, then finalize and run strict delivery audits."
if [[ "$non_manual_unresolved_rows" != "0" || "$failed_automated_checks" != "0" ]]; then
  next_action="Resolve non-manual development or automated gate failures before manual acceptance."
elif [[ "$final_release_allowed" == "true" ]]; then
  next_action="Final release is allowed by the development matrix. Re-run strict delivery-state and delivery-bundle audits before cutting a release."
fi

mkdir -p "$(dirname "$OUTPUT_PATH")"
output_tmp="$(mktemp "$(dirname "$OUTPUT_PATH")/.development-status-json.XXXXXX")"

jq -n \
  --arg schema_version "openpr.universal_forms.development_status.v1" \
  --arg schema_path "$SCHEMA_PATH" \
  --arg generated_at "$(date -Is)" \
  --arg tracker_path "$TRACKER_PATH" \
  --arg readiness_json_path "$READINESS_JSON_PATH" \
  --argjson total_rows "$row_count" \
  --argjson tested_or_accepted_rows "$tested_or_accepted_rows" \
  --argjson accepted_rows "$accepted_rows" \
  --argjson pending_rows "$pending_rows" \
  --argjson non_manual_unresolved_rows "$non_manual_unresolved_rows" \
  --argjson manual_pending_rows "$manual_pending_rows" \
  --argjson failed_automated_checks "$failed_automated_checks" \
  --argjson final_release_allowed "$final_release_allowed" \
  --argjson rows "$rows_json" \
  --arg next_action "$next_action" \
  '{
    schema_version: $schema_version,
    schema_path: $schema_path,
    generated_at: $generated_at,
    tracker_path: $tracker_path,
    readiness_json_path: $readiness_json_path,
    status_summary: {
      total_rows: $total_rows,
      tested_or_accepted_rows: $tested_or_accepted_rows,
      accepted_rows: $accepted_rows,
      pending_rows: $pending_rows,
      non_manual_unresolved_rows: $non_manual_unresolved_rows,
      manual_signoff_pending_rows: $manual_pending_rows,
      failed_automated_checks: $failed_automated_checks,
      final_release_allowed: $final_release_allowed
    },
    rows: $rows,
    next_action: $next_action
  }' >"$output_tmp"

if [[ -f "$OUTPUT_PATH" ]]; then
  chmod --reference="$OUTPUT_PATH" "$output_tmp"
fi
mv -f "$output_tmp" "$OUTPUT_PATH"
output_tmp=""

echo "development status JSON: $OUTPUT_PATH" >&2
