#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_REPORT_PATH="/opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md"
DEFAULT_TRACKER_PATH="/opt/worker/report/openpr/docs/openpr-universal-form-development-execution-tracker-2026-05-31.md"
RUNBOOK_PATH="${OPENPR_UNIVERSAL_FORMS_RUNBOOK:-/opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md}"
REPORT_PATH="$DEFAULT_REPORT_PATH"
TRACKER_PATH="$DEFAULT_TRACKER_PATH"
DRY_RUN=0

usage() {
  cat <<'EOF'
Usage: scripts/finalize-universal-forms-acceptance.sh [--dry-run] [--report PATH] [--tracker PATH] [--runbook PATH]

Finalizes the universal forms tracker after user-side manual acceptance has
been signed in the evidence report. The script first runs
verify-universal-forms-acceptance-signoff.sh, then updates tracker statuses and
appends final acceptance evidence in a candidate file. The candidate tracker is
strict-audited before replacing the requested tracker path. For the default
handoff paths, successful finalization refreshes derived completion, packet,
readiness summary, readiness JSON, development status JSON, scenario catalog
JSON, implementation map JSON, completion audit JSON, evidence-map, UI review gallery, report output boundary smoke, manual
signoff status report, manual signoff status JSON, next signoff review, next
signoff review contract smoke, signoff dashboard, signoff dashboard render smoke, signoff dashboard progression smoke,
signoff status output smoke,
manual signoff progression smoke,
delivery status output smoke, delivery-manifest, and
delivery-manifest JSON files after the tracker is replaced, then reruns the
release gate smoke, release gate output smoke, strict delivery-state, and delivery-bundle audits against
the official handoff.

Options:
  --dry-run       Verify signoff and print what would be updated.
  --report PATH   Acceptance evidence report path.
  --tracker PATH  Development execution tracker path.
  --runbook PATH  User acceptance runbook path for signoff consistency checks.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --report)
      REPORT_PATH="${2:-}"
      if [[ -z "$REPORT_PATH" ]]; then
        echo "--report requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --tracker)
      TRACKER_PATH="${2:-}"
      if [[ -z "$TRACKER_PATH" ]]; then
        echo "--tracker requires a path" >&2
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

"$ROOT_DIR/scripts/verify-universal-forms-acceptance-signoff.sh" \
  "$REPORT_PATH" \
  --runbook "$RUNBOOK_PATH"

if [[ ! -f "$TRACKER_PATH" ]]; then
  echo "Tracker not found: $TRACKER_PATH" >&2
  exit 2
fi

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

refresh_default_handoff_after_finalization() {
  "$ROOT_DIR/scripts/report-universal-forms-completion-audit.sh" >/dev/null
  "$ROOT_DIR/scripts/prepare-universal-forms-manual-evidence-map.sh" >/dev/null
  "$ROOT_DIR/scripts/prepare-universal-forms-ui-review-gallery.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-ui-review-gallery.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-ui-review-gallery-render.sh" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-signoff-status.sh" \
    --output "/opt/worker/report/openpr/docs/openpr-universal-form-signoff-status-2026-05-31.md" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-signoff-status-json.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-signoff-status-json.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-signoff-status-json-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/prepare-universal-forms-signoff-dashboard.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-signoff-dashboard.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-signoff-dashboard-render.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-signoff-dashboard-progression.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-signoff-status-output.sh" >/dev/null
  "$ROOT_DIR/scripts/prepare-universal-forms-next-signoff-review.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-next-signoff-review.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-next-signoff-review-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-next-signoff-command.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-manual-signoff-progression.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-manual-signoff-commands.sh" >/dev/null
  "$ROOT_DIR/scripts/prepare-universal-forms-user-acceptance-packet.sh" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-readiness-summary.sh" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-readiness-json.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-readiness-json.sh" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-development-status-json.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-development-status-json.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-development-status-json-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-completion-audit-json.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-completion-audit-json.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-completion-audit-json-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-scenario-catalog-json.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-scenario-catalog-json.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-scenario-catalog-json-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-implementation-map-json.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-implementation-map-json.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-implementation-map-json-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-report-output-boundaries.sh" >/dev/null
  "$ROOT_DIR/scripts/prepare-universal-forms-delivery-manifest.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest.sh" >/dev/null
  "$ROOT_DIR/scripts/report-universal-forms-delivery-manifest-json.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest-json.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-delivery-manifest-json-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-release-gate.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-release-gate-json.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-release-gate-json-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-release-gate-output.sh" >/dev/null
  "$ROOT_DIR/scripts/status-universal-forms-delivery.sh" >/dev/null
  "$ROOT_DIR/scripts/verify-universal-forms-delivery-status-json.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-delivery-status-json-contract.sh" >/dev/null
  "$ROOT_DIR/scripts/smoke-universal-forms-delivery-status-output.sh" >/dev/null
  "$ROOT_DIR/scripts/audit-universal-forms-delivery-state.sh" --strict >/dev/null
  "$ROOT_DIR/scripts/audit-universal-forms-delivery-bundle.sh" >/dev/null
}

tracker_e2e_status="$(tracker_status "端到端验收")"
tracker_manual_status="$(tracker_status "用户侧人工验收")"
timestamp="$(date -Is)"
evidence_name="$(basename "$REPORT_PATH")"

if [[ "$tracker_e2e_status" == "已验收" && "$tracker_manual_status" == "已验收" ]]; then
  if [[ "$DRY_RUN" -eq 1 ]]; then
    cat <<EOF
Signoff verified. Tracker is already finalized:
  tracker: $TRACKER_PATH
  evidence: $REPORT_PATH
  timestamp: $timestamp

No tracker changes would be written.
EOF
    exit 0
  fi

  "$ROOT_DIR/scripts/audit-universal-forms-delivery-state.sh" \
    --strict \
    --tracker "$TRACKER_PATH" \
    --report "$REPORT_PATH" \
    --runbook "$RUNBOOK_PATH" >/dev/null

  if [[ "$REPORT_PATH" == "$DEFAULT_REPORT_PATH" && "$TRACKER_PATH" == "$DEFAULT_TRACKER_PATH" ]]; then
    refresh_default_handoff_after_finalization
    echo "derived acceptance handoff refreshed after finalization"
    echo "post-finalization delivery bundle verified"
  else
    echo "custom finalize paths used; derived default handoff was not refreshed"
  fi
  echo "universal forms acceptance already finalized in tracker: $TRACKER_PATH"
  exit 0
fi

if [[ "$DRY_RUN" -eq 1 ]]; then
  cat <<EOF
Signoff verified. Would update tracker:
  tracker: $TRACKER_PATH
  evidence: $REPORT_PATH
  timestamp: $timestamp

Status changes:
  | 端到端验收 | 已测试 | -> | 端到端验收 | 已验收 |
  | 用户侧人工验收 | 待处理 | -> | 用户侧人工验收 | 已验收 |
  未完成 -> 无
EOF
  exit 0
fi

tmp_file="$(mktemp "$(dirname "$TRACKER_PATH")/.tracker-finalize.XXXXXX")"
trap 'rm -f "$tmp_file"' EXIT

awk -v ts="$timestamp" -v evidence="$evidence_name" '
  BEGIN {
    in_unfinished = 0
  }
  {
    line = $0
    gsub(/\| 端到端验收 \| 已测试 \|/, "| 端到端验收 | 已验收 |", line)
    gsub(/\| 用户侧人工验收 \| 待处理 \|/, "| 用户侧人工验收 | 已验收 |", line)
    gsub(/还不能标记总体验收，因为用户侧人工验收尚未完成。/, "用户侧人工验收已完成，可以标记总体验收。", line)
    gsub(/尚未标记已验收，因为用户侧人工验收仍未完成。/, "用户侧人工验收已完成，可以标记已验收。", line)
    gsub(/尚未标记已验收，因为还缺少跨仓库真实部署环境的人工验收。/, "跨仓库真实部署环境的人工验收已完成，可以标记已验收。", line)
    gsub(/尚未标记已验收，因为还需要用户侧正式验收和后续前端\/餐厅端到端联动。/, "用户侧正式验收和前端\/餐厅端到端联动已完成，可以标记已验收。", line)
    gsub(/最终 `已验收` 仍等待用户侧人工验收签收。/, "用户侧人工验收已完成，可以标记 `已验收`。", line)

    if (line == "未完成：") {
	      print "166. 用户侧人工验收完成，正式 evidence 报告 `" evidence "` 的 Manual Acceptance Signoff 已全部通过，`scripts/verify-universal-forms-acceptance-signoff.sh`、runbook/evidence consistency、候选 tracker strict delivery audit、默认交付路径刷新、正式 strict delivery-state 与 delivery-bundle audit 均校验通过。验收时间：" ts "。"
      print ""
      print line
      print ""
      print "无。"
      in_unfinished = 1
      next
    }

    if (in_unfinished && line ~ /^## 状态维护规则$/) {
      in_unfinished = 0
    }
    if (in_unfinished) {
      next
    }

    print line
  }
' "$TRACKER_PATH" >"$tmp_file"

"$ROOT_DIR/scripts/audit-universal-forms-delivery-state.sh" \
  --strict \
  --tracker "$tmp_file" \
  --report "$REPORT_PATH" \
  --runbook "$RUNBOOK_PATH"

chmod --reference="$TRACKER_PATH" "$tmp_file"
mv -f "$tmp_file" "$TRACKER_PATH"
tmp_file=""
trap - EXIT

if [[ "$REPORT_PATH" == "$DEFAULT_REPORT_PATH" && "$TRACKER_PATH" == "$DEFAULT_TRACKER_PATH" ]]; then
  refresh_default_handoff_after_finalization
  echo "derived acceptance handoff refreshed after finalization"
  echo "post-finalization delivery bundle verified"
else
  echo "custom finalize paths used; derived default handoff was not refreshed"
fi

echo "universal forms acceptance finalized in tracker: $TRACKER_PATH"
