#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
MANUAL_EVIDENCE_MAP_PATH="$REPORT_DIR/openpr-universal-form-manual-evidence-map-2026-05-31.md"
MARKDOWN_STATUS_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.md"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-signoff-status.schema.json"
REVIEWER_PLACEHOLDER="<name>"
OUTPUT_PATH="${OPENPR_SIGNOFF_STATUS_JSON_REPORT:-$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json}"

usage() {
  cat <<'EOF'
Usage: scripts/report-universal-forms-signoff-status-json.sh [options]
       scripts/report-universal-forms-signoff-status-json.sh [options] [REPORT_PATH]

Generates a machine-readable JSON mirror of the user-side manual signoff state
for CI, MCP tools, webhook consumers, and release automation. This report is
read-only and never marks a row accepted.

Options:
  --report PATH        Acceptance evidence report path.
  --runbook PATH       User acceptance runbook path.
  --manual-map PATH    Manual evidence map path.
  --markdown PATH      Markdown signoff status report path.
  --reviewer NAME      Reviewer name to place in command templates.
  --output PATH        Write the JSON status report to PATH.
  --stdout             Write the JSON status report to stdout.

Environment:
  OPENPR_SIGNOFF_STATUS_JSON_REPORT  Optional output path.
EOF
}

positional_report_seen=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --report)
      EVIDENCE_PATH="${2:-}"
      shift 2
      ;;
    --runbook)
      RUNBOOK_PATH="${2:-}"
      shift 2
      ;;
    --manual-map)
      MANUAL_EVIDENCE_MAP_PATH="${2:-}"
      shift 2
      ;;
    --markdown)
      MARKDOWN_STATUS_PATH="${2:-}"
      shift 2
      ;;
    --reviewer)
      REVIEWER_PLACEHOLDER="${2:-}"
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
    --stdout)
      OUTPUT_PATH="-"
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      if [[ "$1" == -* ]]; then
        echo "Unknown argument: $1" >&2
        usage >&2
        exit 2
      fi
      if [[ "$positional_report_seen" -eq 1 ]]; then
        echo "Only one positional REPORT_PATH is supported: $1" >&2
        usage >&2
        exit 2
      fi
      EVIDENCE_PATH="$1"
      positional_report_seen=1
      shift
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

shell_single_quote() {
  printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

require_command jq
for path in "$EVIDENCE_PATH" "$RUNBOOK_PATH" "$MANUAL_EVIDENCE_MAP_PATH" "$SCHEMA_PATH"; do
  require_file "$path"
done

mkdir -p "$(dirname "$MARKDOWN_STATUS_PATH")" "$(dirname "$OUTPUT_PATH")"
"$ROOT_DIR/scripts/report-universal-forms-signoff-status.sh" \
  --report "$EVIDENCE_PATH" \
  --runbook "$RUNBOOK_PATH" \
  --manual-map "$MANUAL_EVIDENCE_MAP_PATH" \
  --reviewer "$REVIEWER_PLACEHOLDER" \
  --output "$MARKDOWN_STATUS_PATH"
require_file "$MARKDOWN_STATUS_PATH"

summary_total="$(sed -n 's/^- Total automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
summary_failed="$(sed -n 's/^- Failed automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
pass_count="$(rg -c '^\*\*Status:\*\* PASS$' "$EVIDENCE_PATH" || true)"

consistency_status="failed"
if "$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh" "$RUNBOOK_PATH" "$EVIDENCE_PATH" >/dev/null; then
  consistency_status="passed"
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

rows_tsv="$(mktemp /tmp/openpr-uf-signoff-rows.XXXXXX.tsv)"
output_tmp=""
trap 'rm -f "$rows_tsv" "$output_tmp"' EXIT

accepted_count=0
pending_count=0
blocked_count=0
next_index=-1
accepted_prerequisites=0

for index in "${!keys[@]}"; do
  key="${keys[$index]}"
  item="${items[$index]}"
  raw_status="$(manual_row_cell "$item" 3)"
  reviewer="$(manual_row_cell "$item" 4)"
  evidence="$(manual_row_cell "$item" 5)"
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
  recorder_command="scripts/record-universal-forms-manual-signoff.sh --item $key --status accepted --reviewer $(shell_single_quote "$REVIEWER_PLACEHOLDER") --evidence $(shell_single_quote "$suggested_note")"

  if [[ "$normalized" == "accepted" ]]; then
    accepted_count=$((accepted_count + 1))
    if [[ "$index" -lt 6 ]]; then
      accepted_prerequisites=$((accepted_prerequisites + 1))
    fi
  else
    pending_count=$((pending_count + 1))
    if [[ "$next_index" -lt 0 ]]; then
      next_index="$index"
    fi
    if [[ "$normalized" == "failed" || "$normalized" == "rework" || "$normalized" == "unknown" ]]; then
      blocked_count=$((blocked_count + 1))
    fi
  fi

  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$key" \
    "$item" \
    "$normalized" \
    "${raw_status:-missing}" \
    "$reviewer" \
    "$evidence" \
    "$automated_evidence" \
    "$reviewer_check" \
    "$suggested_note" \
    "$recorder_command" >>"$rows_tsv"
done

next_key=""
next_item=""
next_status="pending"
next_actionable=false
next_automated_evidence=""
next_reviewer_check=""
next_evidence_note=""
next_recorder_command=""
if [[ "$pending_count" -gt 0 && "$next_index" -ge 0 ]]; then
  next_key="${keys[$next_index]}"
  next_item="${items[$next_index]}"
  next_status="$(normalize_status "$(manual_row_cell "$next_item" 3)")"
  next_automated_evidence="$(evidence_map_cell "$next_item" 3)"
  next_reviewer_check="$(evidence_map_cell "$next_item" 4)"
  next_evidence_note="$(suggested_note_for "$next_item")"
  if [[ -z "$next_automated_evidence" ]]; then
    next_automated_evidence="missing"
  fi
  if [[ -z "$next_reviewer_check" ]]; then
    next_reviewer_check="missing"
  fi
  if [[ -z "$next_evidence_note" ]]; then
    next_evidence_note="<review note>"
  fi
  if [[ "$next_key" != "overall" || "$accepted_prerequisites" -ge 6 ]]; then
    next_actionable=true
  fi
  next_recorder_command="scripts/record-universal-forms-manual-signoff.sh --item $next_key --status accepted --reviewer $(shell_single_quote "$REVIEWER_PLACEHOLDER") --evidence $(shell_single_quote "$next_evidence_note")"
fi

rows_json="$(
  jq -R -s '
    split("\n")
    | map(select(length > 0) | split("\t") | {
        key: .[0],
        item: .[1],
        status: .[2],
        raw_status: .[3],
        reviewer: .[4],
        evidence: .[5],
        automated_evidence: .[6],
        reviewer_check: .[7],
        suggested_evidence_note: .[8],
        recorder_command: .[9]
      })
  ' "$rows_tsv"
)"

total_rows="${#keys[@]}"
final_allowed=false
if [[ "${summary_failed:-missing}" == "0" && "$consistency_status" == "passed" && "$pending_count" -eq 0 && "$blocked_count" -eq 0 ]]; then
  final_allowed=true
fi

emit_json() {
  jq -n \
    --arg generated_at "$(date -Is)" \
    --arg root_dir "$ROOT_DIR" \
    --arg schema_path "$SCHEMA_PATH" \
    --arg evidence "$EVIDENCE_PATH" \
    --arg runbook "$RUNBOOK_PATH" \
    --arg manual_map "$MANUAL_EVIDENCE_MAP_PATH" \
    --arg markdown_status "$MARKDOWN_STATUS_PATH" \
    --arg consistency_status "$consistency_status" \
    --arg next_key "$next_key" \
    --arg next_item "$next_item" \
    --arg next_status "$next_status" \
    --arg next_automated_evidence "$next_automated_evidence" \
    --arg next_reviewer_check "$next_reviewer_check" \
    --arg next_evidence_note "$next_evidence_note" \
    --arg next_recorder_command "$next_recorder_command" \
    --argjson total_checks "${summary_total:-0}" \
    --argjson failed_checks "${summary_failed:-0}" \
    --argjson pass_count "$pass_count" \
    --argjson total_rows "$total_rows" \
    --argjson accepted_count "$accepted_count" \
    --argjson pending_count "$pending_count" \
    --argjson blocked_count "$blocked_count" \
    --argjson final_allowed "$final_allowed" \
    --argjson next_actionable "$next_actionable" \
    --argjson rows "$rows_json" \
    '{
      schema_version: "openpr.universal_forms.signoff_status.v1",
      schema_path: $schema_path,
      generated_at: $generated_at,
      repository: $root_dir,
      reports: {
        evidence: $evidence,
        runbook: $runbook,
        manual_evidence_map: $manual_map,
        markdown_status: $markdown_status
      },
      gate_summary: {
        automated_checks: $total_checks,
        pass_status_lines: $pass_count,
        failed_automated_checks: $failed_checks,
        runbook_evidence_consistency: $consistency_status
      },
      manual_signoff: {
        total_rows: $total_rows,
        accepted_rows: $accepted_count,
        pending_rows: $pending_count,
        blocked_rows: $blocked_count,
        final_signoff_allowed: $final_allowed,
        rows: $rows,
        pending_queue: (
          $rows
          | to_entries
          | map(
              select(.value.status != "accepted")
              | {
                  review_order: (.key + 1),
                  key: .value.key,
                  item: .value.item,
                  status: .value.status,
                  is_next: (.value.key == $next_key),
                  actionable: (.value.key == $next_key and $next_actionable),
                  automated_evidence: .value.automated_evidence,
                  reviewer_check: .value.reviewer_check,
                  suggested_evidence_note: .value.suggested_evidence_note,
                  recorder_command: .value.recorder_command
                }
            )
        ),
        next_row: {
          key: $next_key,
          item: $next_item,
          status: $next_status,
          actionable: $next_actionable,
          automated_evidence: $next_automated_evidence,
          reviewer_check: $next_reviewer_check,
          suggested_evidence_note: $next_evidence_note,
          recorder_command: $next_recorder_command
        }
      },
      release_requirement: {
        failed_automated_checks: 0,
        runbook_evidence_consistency: "passed",
        pending_rows: 0,
        blocked_rows: 0
      }
    }'
}

if [[ "$OUTPUT_PATH" == "-" ]]; then
  emit_json
  echo "signoff status JSON report: stdout" >&2
else
  mkdir -p "$(dirname "$OUTPUT_PATH")"
  output_tmp="$(mktemp "$(dirname "$OUTPUT_PATH")/.signoff-status-json.XXXXXX")"
  emit_json >"$output_tmp"

  if [[ -f "$OUTPUT_PATH" ]]; then
    chmod --reference="$OUTPUT_PATH" "$output_tmp"
  fi
  mv -f "$output_tmp" "$OUTPUT_PATH"
  output_tmp=""

  echo "signoff status JSON report: $OUTPUT_PATH" >&2
fi
