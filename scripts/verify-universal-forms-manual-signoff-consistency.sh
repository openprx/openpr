#!/usr/bin/env bash
set -euo pipefail

RUNBOOK_PATH="${1:-/opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md}"
EVIDENCE_PATH="${2:-/opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md}"

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-manual-signoff-consistency.sh [RUNBOOK_PATH] [EVIDENCE_PATH]

Verifies that the Chinese user acceptance runbook conclusion table and the
formal acceptance evidence Manual Acceptance Signoff table describe the same
manual acceptance state. This prevents reviewers from signing one artifact
while leaving the other stale.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if [[ ! -f "$RUNBOOK_PATH" ]]; then
  echo "Runbook not found: $RUNBOOK_PATH" >&2
  exit 2
fi

if [[ ! -f "$EVIDENCE_PATH" ]]; then
  echo "Acceptance evidence report not found: $EVIDENCE_PATH" >&2
  exit 2
fi

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
    pending|待验收|'')
      printf 'pending'
      ;;
    pass|passed|approved|accepted|通过|已通过|已验收)
      printf 'passed'
      ;;
    fail|failed|失败)
      printf 'failed'
      ;;
    rework|'needs rework'|需整改)
      printf 'rework'
      ;;
    *)
      printf 'unknown:%s' "$raw"
      ;;
  esac
}

row_for_item() {
  local path="$1"
  local heading="$2"
  local item_name="$3"
  awk -F'|' -v heading="$heading" -v expected="$item_name" '
    $0 == heading { in_section = 1; next }
    /^## / && in_section { exit }
    in_section && NF >= 5 {
      item = $2
      status = $3
      reviewer = $4
      evidence = $5
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", item)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", status)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", reviewer)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", evidence)
      if (item == expected) {
        print status "|" reviewer "|" evidence
        found = 1
        exit
      }
    }
    END {
      if (!found) {
        exit 1
      }
    }
  ' "$path"
}

check_pair() {
  local runbook_item="$1"
  local evidence_item="$2"
  local runbook_row evidence_row
  local runbook_status runbook_reviewer runbook_evidence
  local evidence_status evidence_reviewer evidence_evidence
  local runbook_normalized evidence_normalized

  if ! runbook_row="$(row_for_item "$RUNBOOK_PATH" "## 验收结论记录" "$runbook_item")"; then
    echo "Runbook signoff row missing: $runbook_item" >&2
    return 1
  fi
  if ! evidence_row="$(row_for_item "$EVIDENCE_PATH" "## Manual Acceptance Signoff" "$evidence_item")"; then
    echo "Evidence signoff row missing: $evidence_item" >&2
    return 1
  fi

  IFS='|' read -r runbook_status runbook_reviewer runbook_evidence <<<"$runbook_row"
  IFS='|' read -r evidence_status evidence_reviewer evidence_evidence <<<"$evidence_row"
  runbook_normalized="$(normalize_status "$runbook_status")"
  evidence_normalized="$(normalize_status "$evidence_status")"

  if [[ "$runbook_normalized" != "$evidence_normalized" ]]; then
    echo "Signoff status mismatch: $runbook_item => $runbook_status, $evidence_item => $evidence_status" >&2
    return 1
  fi

  if [[ "$runbook_normalized" == unknown:* || "$evidence_normalized" == unknown:* ]]; then
    echo "Unknown signoff status: $runbook_item => $runbook_status, $evidence_item => $evidence_status" >&2
    return 1
  fi

  if [[ "$runbook_normalized" == "passed" ]]; then
    if [[ -z "$runbook_reviewer" || -z "$evidence_reviewer" ]]; then
      echo "Passed signoff row is missing reviewer: $runbook_item / $evidence_item" >&2
      return 1
    fi
    if [[ -z "$runbook_evidence" || -z "$evidence_evidence" ]]; then
      echo "Passed signoff row is missing evidence note: $runbook_item / $evidence_item" >&2
      return 1
    fi
  fi

  printf 'PASS: %s <-> %s are both %s\n' "$runbook_item" "$evidence_item" "$runbook_normalized"
}

failures=0

printf 'Universal forms manual signoff consistency verification\n'
printf '  runbook: %s\n' "$RUNBOOK_PATH"
printf '  evidence: %s\n' "$EVIDENCE_PATH"
printf '\n'

pairs=(
  "餐厅模板可直接创建项目|Restaurant template can create a project directly"
  "万能表单前端可被非技术用户操作|Universal forms frontend is usable by a non-technical operator"
  "金额/数量/小计展示符合业务预期|Amount, quantity, and subtotal behavior is acceptable"
  "订单、明细、换桌、打印、报表链路可用|Order, order line, table change, print, and report workflow is acceptable"
  "MCP/API/Webhook/Connector 业务枢纽一致性可接受|MCP/API/Webhook/Connector consistency is acceptable"
  "README/docs 足够让新用户理解和复现|README/docs are sufficient for a new user to reproduce"
  "总体验收|Overall acceptance"
)

for pair in "${pairs[@]}"; do
  IFS='|' read -r runbook_item evidence_item <<<"$pair"
  if ! check_pair "$runbook_item" "$evidence_item"; then
    failures=$((failures + 1))
  fi
done

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms manual signoff consistency verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms manual signoff consistency verification passed.\n'
