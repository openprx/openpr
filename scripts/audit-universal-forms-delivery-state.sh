#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRACKER_PATH="/opt/worker/report/openpr/docs/openpr-universal-form-development-execution-tracker-2026-05-31.md"
REPORT_PATH="/opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md"
RUNBOOK_PATH="${OPENPR_UNIVERSAL_FORMS_RUNBOOK:-/opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md}"
EXPECTED_AUTOMATED_CHECKS="${OPENPR_EXPECTED_AUTOMATED_CHECKS:-27}"
STRICT=0

usage() {
  cat <<'EOF'
Usage: scripts/audit-universal-forms-delivery-state.sh [--strict] [--tracker PATH] [--report PATH] [--runbook PATH]

Audits whether the universal forms delivery tracker, generated acceptance
evidence, runbook manual signoff state, and finalization scripts agree.

Modes:
  default   Allow the current pre-signoff state: automated gates green and
            user-side manual acceptance pending.
  --strict  Require final signoff and finalized tracker statuses.

Options:
  --tracker PATH  Development execution tracker path.
  --report PATH   Acceptance evidence report path.
  --runbook PATH  User acceptance runbook path.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --strict)
      STRICT=1
      shift
      ;;
    --tracker)
      TRACKER_PATH="${2:-}"
      if [[ -z "$TRACKER_PATH" ]]; then
        echo "--tracker requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --report)
      REPORT_PATH="${2:-}"
      if [[ -z "$REPORT_PATH" ]]; then
        echo "--report requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --runbook)
      RUNBOOK_PATH="${2:-}"
      if [[ -z "$RUNBOOK_PATH" ]]; then
        echo "--runbook requires a path" >&2
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

tracker_status() {
  local key="$1"
  awk -F'|' -v key="$key" '
    NF >= 4 {
      item = $2
      status = $3
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", item)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", status)
      if (item == key) {
        print status
        exit
      }
    }
  ' "$TRACKER_PATH"
}

manual_rows() {
  awk -F'|' '
    /^## Manual Acceptance Signoff$/ { in_section = 1; next }
    /^## / && in_section { exit }
    in_section && NF >= 5 {
      item = $2
      status = $3
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", item)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", status)
      if (item != "Item" && item !~ /^-+$/ && item != "") {
        print item "|" status
      }
    }
  ' "$REPORT_PATH"
}

is_pass_status() {
  local normalized
  normalized="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
  case "$normalized" in
    pass|passed|approved|accepted|通过|已通过|已验收)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

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

equals() {
  [[ "$1" == "$2" ]]
}

ge() {
  [[ "$1" =~ ^[0-9]+$ && "$2" =~ ^[0-9]+$ && "$1" -ge "$2" ]]
}

contains() {
  local needle="$1"
  local path="$2"
  rg -q --fixed-strings "$needle" "$path"
}

not_contains() {
  local needle="$1"
  local path="$2"
  ! rg -q --fixed-strings "$needle" "$path"
}

require_file "$TRACKER_PATH"
require_file "$REPORT_PATH"
require_file "$RUNBOOK_PATH"

for path in \
  "$ROOT_DIR/scripts/acceptance-universal-forms.sh" \
  "$ROOT_DIR/scripts/verify-universal-forms-acceptance-signoff.sh" \
  "$ROOT_DIR/scripts/finalize-universal-forms-acceptance.sh" \
  "$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh" \
  "$ROOT_DIR/docs/universal-forms-and-plugins.md" \
  "$ROOT_DIR/docs/universal-forms-acceptance.md"; do
  require_file "$path"
done

code_status="$(tracker_status "代码实现")"
forms_status="$(tracker_status "万能表单数据层/API")"
automation_status="$(tracker_status "自动化测试")"
architecture_status="$(tracker_status "架构设计")"
implementation_plan_status="$(tracker_status "开发实施方案")"
wasm_architecture_status="$(tracker_status "WASM 插件架构")"
tracker_status_value="$(tracker_status "工程执行台账")"
e2e_status="$(tracker_status "端到端验收")"
materials_status="$(tracker_status "用户侧验收材料")"
manual_status="$(tracker_status "用户侧人工验收")"

summary_total="$(sed -n 's/^- Total automated checks: //p' "$REPORT_PATH" | tail -n 1)"
summary_failed="$(sed -n 's/^- Failed automated checks: //p' "$REPORT_PATH" | tail -n 1)"
pass_count="$(rg -c '^\*\*Status:\*\* PASS$' "$REPORT_PATH" || true)"
check_index_rows="$(awk '
  /^## Automated Check Index$/ { in_section = 1; next }
  /^## / && in_section { exit }
  in_section && /^\| [0-9]+ \|/ { count++ }
  END { print count + 0 }
' "$REPORT_PATH")"
check_index_failed_rows="$(awk -F'|' '
  /^## Automated Check Index$/ { in_section = 1; next }
  /^## / && in_section { exit }
  in_section && NF >= 4 {
    status = $4
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", status)
    if (status ~ /^FAIL/) {
      count++
    }
  }
  END { print count + 0 }
' "$REPORT_PATH")"
rows="$(manual_rows)"
manual_total=0
manual_pending=0
manual_not_passed=0

if [[ -z "$rows" ]]; then
  manual_not_passed=1
else
  while IFS='|' read -r _item status; do
    manual_total=$((manual_total + 1))
    if [[ "$status" == "Pending" || "$status" == "待验收" || -z "$status" ]]; then
      manual_pending=$((manual_pending + 1))
    fi
    if ! is_pass_status "$status"; then
      manual_not_passed=$((manual_not_passed + 1))
    fi
  done <<<"$rows"
fi

printf 'Universal forms delivery state audit\n'
printf '  tracker: %s\n' "$TRACKER_PATH"
printf '  evidence: %s\n' "$REPORT_PATH"
printf '  runbook: %s\n' "$RUNBOOK_PATH"
printf '  strict: %s\n' "$STRICT"
printf '\n'

check "code implementation status is 已测试" equals "$code_status" "已测试"
check "universal forms data/API status is 已测试" equals "$forms_status" "已测试"
check "automation status is 已测试" equals "$automation_status" "已测试"
check "architecture status is 已测试" equals "$architecture_status" "已测试"
check "implementation plan status is 已测试" equals "$implementation_plan_status" "已测试"
check "WASM architecture status is 已测试" equals "$wasm_architecture_status" "已测试"
check "execution tracker status is 已测试" equals "$tracker_status_value" "已测试"
check "user acceptance materials status is 已测试" equals "$materials_status" "已测试"
check "automated evidence has expected automated check count" equals "$summary_total" "$EXPECTED_AUTOMATED_CHECKS"
check "automated evidence failed checks is 0" equals "$summary_failed" "0"
check "all automated evidence checks are PASS" equals "$pass_count" "$summary_total"
check "automated check index row count matches total" equals "$check_index_rows" "$summary_total"
check "automated check index has no failed rows" equals "$check_index_failed_rows" "0"
check "manual signoff table has 7 rows" equals "$manual_total" "7"
check "acceptance script is documented" contains "scripts/acceptance-universal-forms.sh --full" "$ROOT_DIR/docs/universal-forms-acceptance.md"
check "finalize script is documented" contains "scripts/finalize-universal-forms-acceptance.sh" "$ROOT_DIR/docs/universal-forms-acceptance.md"
check "tracker records finalize verifier evidence" contains "scripts/finalize-universal-forms-acceptance.sh" "$TRACKER_PATH"
check "runbook/evidence manual signoff consistency passes" "$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh" "$RUNBOOK_PATH" "$REPORT_PATH"

if [[ "$STRICT" -eq 1 ]]; then
  check "end-to-end acceptance status is 已验收" equals "$e2e_status" "已验收"
  check "user-side manual acceptance status is 已验收" equals "$manual_status" "已验收"
  check "manual signoff has no pending rows" equals "$manual_pending" "0"
  check "manual signoff has no non-passing rows" equals "$manual_not_passed" "0"
  check "tracker unfinished section is clear" contains "无。" "$TRACKER_PATH"
  check "tracker has no pending manual acceptance wording" not_contains "用户侧人工验收仍未完成" "$TRACKER_PATH"
else
  check "end-to-end acceptance remains pre-signoff 已测试" equals "$e2e_status" "已测试"
  check "user-side manual acceptance remains 待处理" equals "$manual_status" "待处理"
  check "manual signoff pending rows are explicit" equals "$manual_pending" "7"
  check "tracker unfinished section names manual signoff" contains "用户侧人工验收：待验收人按" "$TRACKER_PATH"
fi

printf '\n'
printf 'Summary:\n'
printf '  code implementation: %s\n' "$code_status"
printf '  automated checks: %s total, %s failed, %s PASS lines\n' "$summary_total" "$summary_failed" "$pass_count"
printf '  manual signoff: %s rows, %s pending, %s not passed\n' "$manual_total" "$manual_pending" "$manual_not_passed"
printf '  runbook/evidence consistency: checked\n'
printf '  e2e acceptance: %s\n' "$e2e_status"
printf '  user-side manual acceptance: %s\n' "$manual_status"

if [[ "$failures" -ne 0 ]]; then
  printf '\nDelivery state audit failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

if [[ "$STRICT" -eq 1 ]]; then
  printf '\nDelivery state is finalized and accepted.\n'
else
  printf '\nDelivery state is internally consistent: automated gates are green; user-side manual signoff is still pending.\n'
fi
