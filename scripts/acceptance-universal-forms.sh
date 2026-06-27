#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="quick"
REPORT_PATH="${OPENPR_ACCEPTANCE_REPORT:-}"
FRONTEND_LOCK="${OPENPR_FRONTEND_BUILD_LOCK:-/tmp/openpr-frontend-build.lock}"

usage() {
  cat <<'EOF'
Usage: scripts/acceptance-universal-forms.sh [--quick|--full] [--output PATH]

Collects automated acceptance evidence for the universal forms, WASM plugin,
MCP/webhook/connector, and restaurant ordering reference workflow.

Modes:
  --quick   Run focused acceptance smoke checks. Default.
  --full    Run CI-grade checks plus focused acceptance smoke checks.

Environment:
  OPENPR_ACCEPTANCE_REPORT  Optional output markdown path.
  OPENPR_FRONTEND_BUILD_LOCK Optional lock file for frontend check/build/smoke.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --quick)
      MODE="quick"
      shift
      ;;
    --full)
      MODE="full"
      shift
      ;;
    --output)
      REPORT_PATH="${2:-}"
      if [[ -z "$REPORT_PATH" ]]; then
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

if [[ -z "$REPORT_PATH" ]]; then
  REPORT_PATH="/opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-$(date +%Y%m%d-%H%M%S).md"
fi

mkdir -p "$(dirname "$REPORT_PATH")"
TMP_DIR="$(mktemp -d /tmp/openpr-universal-forms-acceptance.XXXXXX)"
CHECK_INDEX_PATH="$TMP_DIR/automated-check-index.tsv"
REPORT_TARGET_PATH="$REPORT_PATH"
output_tmp="$(mktemp "$(dirname "$REPORT_TARGET_PATH")/.acceptance-evidence.XXXXXX")"
REPORT_PATH="$output_tmp"
trap 'rm -rf "$TMP_DIR"; rm -f "$output_tmp"' EXIT

cd "$ROOT_DIR"

if ! command -v bun >/dev/null 2>&1 && [[ -x "$HOME/.bun/bin/bun" ]]; then
  export PATH="$HOME/.bun/bin:$PATH"
fi

if ! command -v bun >/dev/null 2>&1; then
  echo "Bun is required for frontend acceptance checks. Install Bun or add it to PATH." >&2
  exit 2
fi

FRONTEND_BUN="$(command -v bun)"

total_checks=0
failed_checks=0

append_report() {
  printf '%s\n' "$*" >>"$REPORT_PATH"
}

init_report() {
  : >"$CHECK_INDEX_PATH"
  : >"$REPORT_PATH"
  append_report "# OpenPR Universal Forms Acceptance Evidence"
  append_report ""
  append_report "- Generated at: $(date -Is)"
  append_report "- Repository: $ROOT_DIR"
  append_report "- Mode: $MODE"
  append_report ""
  append_report "This report collects automated evidence. It does not replace the manual user acceptance runbook:"
  append_report ""
  append_report '```text'
  append_report "report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
  append_report '```'
  append_report ""
  append_report "## Automated Checks"
  append_report ""
}

run_check() {
  local name="$1"
  local command="$2"
  local log_file="$TMP_DIR/check-$total_checks.log"
  total_checks=$((total_checks + 1))

  echo "==> $name"
  append_report "### $name"
  append_report ""
  append_report '```bash'
  append_report "$command"
  append_report '```'
  append_report ""

  set +e
  bash -lc "$command" >"$log_file" 2>&1
  local status=$?
  set -e

  local vite_preview_hint
  vite_preview_hint='Run npm '"run preview to preview your production build locally."
  awk -v hint="$vite_preview_hint" '$0 != hint { print }' "$log_file" >"$log_file.filtered"
  mv "$log_file.filtered" "$log_file"

  local check_status
  if [[ $status -eq 0 ]]; then
    check_status="PASS"
    append_report "**Status:** PASS"
    echo "PASS: $name"
  else
    check_status="FAIL ($status)"
    failed_checks=$((failed_checks + 1))
    append_report "**Status:** FAIL ($status)"
    echo "FAIL: $name" >&2
  fi

  printf '%s\t%s\t%s\n' "$total_checks" "$name" "$check_status" >>"$CHECK_INDEX_PATH"

  append_report ""
  append_report "Key PASS/FAIL lines:"
  append_report ""
  append_report '```text'
  if rg -q '^(PASS|FAIL):' "$log_file"; then
    rg '^(PASS|FAIL):' "$log_file" >>"$REPORT_PATH"
  else
    append_report "No PASS/FAIL lines emitted by this command."
  fi
  append_report '```'
  append_report ""
  append_report "<details><summary>Output</summary>"
  append_report ""
  append_report '```text'
  tail -n 120 "$log_file" >>"$REPORT_PATH"
  append_report '```'
  append_report ""
  append_report "</details>"
  append_report ""
}

append_check_index() {
  append_report "## Automated Check Index"
  append_report ""
  append_report "| # | Check | Status |"
  append_report "| --- | --- | --- |"
  awk -F'\t' '{ printf "| %s | %s | %s |\n", $1, $2, $3 }' "$CHECK_INDEX_PATH" >>"$REPORT_PATH"
  append_report ""
}

run_quick_checks() {
  run_check "Universal forms source coverage audit" "./scripts/audit-universal-forms-source-coverage.sh"
  run_check "Universal forms production readiness audit" "./scripts/audit-universal-forms-production-readiness.sh"
  run_check "PostgreSQL-only security scope audit" "./scripts/audit-universal-forms-security-scope.sh"
  run_check "Universal forms docs and protocol audit" "OPENPR_DOCS_AUDIT_SKIP_EVIDENCE=1 ./scripts/audit-universal-forms-docs.sh"
  run_check "WASM plugin runtime smoke" "./scripts/smoke-wasm-plugin-runtime.sh"
  run_check "Generic webhook consumer smoke" "./scripts/smoke-webhook-generic-consumer.sh"
  run_check "Project type/resource API and MCP smoke" "./scripts/smoke-phase1-project-types-resources.sh"
  run_check "User settings profile password preferences smoke" "./scripts/smoke-user-settings-api.sh"
  run_check "Phase 2 connectors and invocation lifecycle smoke" "./scripts/smoke-phase2-connectors-invocations.sh"
  run_check "Phase 3 MCP governance acceptance smoke" "./scripts/smoke-phase3-mcp-governance-acceptance.sh"
  run_check "Universal forms MCP and generic CLI smoke" "./scripts/smoke-forms-mcp.sh"
  run_check "Phase 5 release readiness live API and MCP smoke" "./scripts/smoke-phase5-release-readiness.sh"
  run_check "Scenario template forms and plugin smoke" "./scripts/smoke-scenario-template-forms.sh"
  run_check "Restaurant ordering backend smoke" "./scripts/smoke-restaurant-ordering.sh"
  run_check "Restaurant demo bootstrap MCP HTTP smoke" "./scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh"
  run_check "Frontend project template wizard smoke" "flock '$FRONTEND_LOCK' -c 'cd frontend && $FRONTEND_BUN run smoke:project-template'"
  run_check "Frontend template work items smoke" "flock '$FRONTEND_LOCK' -c 'cd frontend && $FRONTEND_BUN run smoke:template-work-items'"
  run_check "Frontend forms browser smoke" "flock '$FRONTEND_LOCK' -c 'cd frontend && $FRONTEND_BUN run smoke:forms-ui'"
  run_check "Frontend restaurant browser smoke" "flock '$FRONTEND_LOCK' -c 'cd frontend && $FRONTEND_BUN run smoke:restaurant-ordering'"
}

run_full_checks() {
  run_check "Rust format check" "cargo fmt --all -- --check"
  run_check "Rust workspace clippy" "cargo clippy --workspace --all-targets --all-features -- -D warnings"
  run_check "Unused dependency check" "cargo machete"
  run_check "Rust security audit" "cargo audit"
  run_check "Rust workspace tests" "cargo test --workspace --all-features"
  run_check "Rust release build" "cargo build --workspace --release"
  run_check "Frontend check" "flock '$FRONTEND_LOCK' -c 'cd frontend && $FRONTEND_BUN run check'"
  run_check "Frontend build" "flock '$FRONTEND_LOCK' -c 'cd frontend && $FRONTEND_BUN run build'"
  run_quick_checks
}

capture_existing_manual_signoff_rows() {
  local source_path="$1"
  if [[ ! -f "$source_path" ]]; then
    return 1
  fi

  local rows
  rows="$(
    awk '
      /^## Manual Acceptance Signoff$/ { in_section = 1; next }
      in_section && /^## / { exit }
      in_section && /^\| / && $0 !~ /^\| Item \|/ && $0 !~ /^\| --- \|/ { print }
    ' "$source_path"
  )"

  if [[ -z "$rows" ]]; then
    return 1
  fi

  local item
  for item in \
    "Restaurant template can create a project directly" \
    "Universal forms frontend is usable by a non-technical operator" \
    "Amount, quantity, and subtotal behavior is acceptable" \
    "Order, order line, table change, print, and report workflow is acceptable" \
    "MCP/API/Webhook/Connector consistency is acceptable" \
    "README/docs are sufficient for a new user to reproduce" \
    "Overall acceptance"; do
    if ! printf '%s\n' "$rows" | rg -q --fixed-strings -- "| $item |"; then
      return 1
    fi
  done

  printf '%s\n' "$rows"
}

PRESERVED_MANUAL_SIGNOFF_ROWS="$(capture_existing_manual_signoff_rows "$REPORT_TARGET_PATH" || true)"

append_manual_section() {
  append_report "## Manual Acceptance Signoff"
  append_report ""
  append_report "| Item | Status | Reviewer | Evidence |"
  append_report "| --- | --- | --- | --- |"
  if [[ -n "$PRESERVED_MANUAL_SIGNOFF_ROWS" ]]; then
    printf '%s\n' "$PRESERVED_MANUAL_SIGNOFF_ROWS" >>"$REPORT_PATH"
  else
    append_report "| Restaurant template can create a project directly | Pending |  |  |"
    append_report "| Universal forms frontend is usable by a non-technical operator | Pending |  |  |"
    append_report "| Amount, quantity, and subtotal behavior is acceptable | Pending |  |  |"
    append_report "| Order, order line, table change, print, and report workflow is acceptable | Pending |  |  |"
    append_report "| MCP/API/Webhook/Connector consistency is acceptable | Pending |  |  |"
    append_report "| README/docs are sufficient for a new user to reproduce | Pending |  |  |"
    append_report "| Overall acceptance | Pending |  |  |"
  fi
  append_report ""
}

init_report

case "$MODE" in
  quick)
    run_quick_checks
    ;;
  full)
    run_full_checks
    ;;
  *)
    echo "Unsupported mode: $MODE" >&2
    exit 2
    ;;
esac

append_check_index
append_manual_section

append_report "## Summary"
append_report ""
append_report "- Total automated checks: $total_checks"
append_report "- Failed automated checks: $failed_checks"
append_report ""

if [[ -f "$REPORT_TARGET_PATH" ]]; then
  chmod --reference="$REPORT_TARGET_PATH" "$output_tmp"
fi
mv -f "$output_tmp" "$REPORT_TARGET_PATH"
REPORT_PATH="$REPORT_TARGET_PATH"
output_tmp=""

echo "Acceptance evidence report: $REPORT_PATH"

if [[ $failed_checks -ne 0 ]]; then
  exit 1
fi

echo "universal forms acceptance evidence collection passed"
