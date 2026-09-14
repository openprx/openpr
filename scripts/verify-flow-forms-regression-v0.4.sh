#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 hard gate `forms_regression_no_degradation`.
#
# Contract: gates/gate-commands.md
#   - 「通用命令束」requires every report to run and record the exact command
#     `bash scripts/ci-universal-forms-gates.sh`; since the 2026-09-01 ledger
#     correction, v0.4 records this wrapper as the one canonical execution and
#     does not also run the same command in the generic section;
#   - 「防假绿」requires exact command + exit + duration + artifact checksum to
#     be recorded ("手工复制终端文字不是 evidence"), requires a targeted run to
#     prove executed check count > 0, and states that a missing script is a
#     failure and never a skip;
#   - 「防假绿」further states `Forms gate 不因 Flow 新增而删除或改成
#     allow-failure`, so this script never downgrades a non-zero Forms gate
#     exit into anything but `failed`.
#
# This script does NOT re-implement the Forms regression suite. It reuses the
# repository's existing Universal Forms CI gate bundle verbatim --
# scripts/ci-universal-forms-gates.sh, the same entrypoint GitHub Actions runs,
# which chains audit-universal-forms-{security-scope,source-coverage,
# production-readiness}.sh -- and turns its result into the evidence artifact
# evidence/v0.4/forms-regression-result.json that
# scripts/lib/flow_gate_v0_4_recompute.py bridges into the 52 hard-gate
# recomputation. Before this existed the bundle passed every round but nothing
# wrote it down, so the gate sat at `not_verified` forever.
#
# The verdict is `passed` only when ALL of the following hold:
#   1. the bundle exited 0;
#   2. its output contains zero `FAIL: ` assertion lines;
#   3. its output contains at least one `PASS: ` assertion line (a bundle that
#      exits 0 having executed nothing is a false green, not a pass);
#   4. its output ends with the bundle's own current completion marker.
#      completion marker (guards against a truncated/killed run).
# Anything else is `failed`. There is no `not_covered` branch: the bundle
# exists in this repository and is runnable, so "cannot be exercised here"
# is never true for this gate.
#
# Exit codes: 0 gate passed, 1 ran to completion with the gate not passed,
# 2 usage/tool/environment error (including a missing Forms gate script).

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-forms-regression-v0.4.sh --json [OPTIONS]

Runs the existing Universal Forms CI gate bundle
(`bash scripts/ci-universal-forms-gates.sh`) and records its exact command,
exit code, duration, assertion counts and log checksum as
evidence/v0.4/forms-regression-result.json, which backs the v0.4 hard gate
`forms_regression_no_degradation`.

Options:
  --repo-root DIR         Repository containing scripts/ci-universal-forms-
                          gates.sh. Default: this checkout.
  --evidence-root DIR     Where forms-regression-result.json and the run log
                          are written. Default:
                          /opt/working/sylvode-flow/evidence/v0.4
  --json                  Required for CLI-contract compatibility.
  -h, --help              Show this help and exit 0.

Exit codes: 0 gate passed, 1 gate not passed, 2 usage/tool/environment error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
for tool in jq git sha256sum; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    exit 2
  fi
done
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi

# gate-commands.md 「防假绿」: a missing script is a failure, never a skip.
# Fail closed at exit 2 (environment error) rather than writing an artifact
# that claims the Forms suite was evaluated.
FORMS_GATE_SCRIPT="$REPO_ROOT/scripts/ci-universal-forms-gates.sh"
if [[ ! -f "$FORMS_GATE_SCRIPT" ]]; then
  echo "FAIL: Forms regression gate script not found (missing script is a failure, not a skip): $FORMS_GATE_SCRIPT" >&2
  exit 2
fi
for sub in audit-universal-forms-security-scope.sh \
           audit-universal-forms-source-coverage.sh \
           audit-universal-forms-production-readiness.sh; do
  if [[ ! -f "$REPO_ROOT/scripts/$sub" ]]; then
    echo "FAIL: Forms gate bundle member not found (missing script is a failure, not a skip): $REPO_ROOT/scripts/$sub" >&2
    exit 2
  fi
done

mkdir -p "$EVIDENCE_ROOT" "$EVIDENCE_ROOT/logs"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
if [[ -n "$(git -C "$REPO_ROOT" status --porcelain)" ]]; then
  SOURCE_DIRTY=true
else
  SOURCE_DIRTY=false
fi
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

FORMS_COMMAND="bash scripts/ci-universal-forms-gates.sh"
LOG_FILE="$EVIDENCE_ROOT/logs/forms-regression.ci-universal-forms-gates.log"

echo "=== forms regression: $FORMS_COMMAND (cwd $REPO_ROOT) ===" >&2
START_MS="$(date +%s%3N)"
set +e
( cd "$REPO_ROOT" && bash scripts/ci-universal-forms-gates.sh ) > "$LOG_FILE" 2>&1
FORMS_EXIT=$?
set -e
END_MS="$(date +%s%3N)"
DURATION_MS=$((END_MS - START_MS))
LOG_SHA256="$(sha256sum "$LOG_FILE" | awk '{print $1}')"

# `PASS: `/`FAIL: ` are the audit helpers' own per-assertion markers
# (scripts/audit-universal-forms-*.sh pass()/fail()); stdout and stderr are
# captured into one log so both are counted.
PASS_COUNT="$(grep -c '^PASS: ' "$LOG_FILE" || true)"
FAIL_COUNT="$(grep -c '^FAIL: ' "$LOG_FILE" || true)"
if grep -qx 'Universal Forms static and Rust regression gates passed.' "$LOG_FILE"; then
  COMPLETION_MARKER=true
else
  COMPLETION_MARKER=false
fi

# `|| true` on the grep itself: no FAIL line is the good case, and grep's
# exit 1 must not be turned into a script failure by `set -o pipefail`.
FAILED_ASSERTIONS_JSON="$( { grep '^FAIL: ' "$LOG_FILE" || true; } | sed 's/^FAIL: //' | head -50 | jq -R . | jq -s .)"

REASON_PARTS=()
if [[ $FORMS_EXIT -ne 0 ]]; then
  REASON_PARTS+=("$FORMS_COMMAND exited $FORMS_EXIT")
fi
if [[ "$FAIL_COUNT" -ne 0 ]]; then
  REASON_PARTS+=("$FAIL_COUNT failing assertion(s)")
fi
if [[ "$PASS_COUNT" -le 0 ]]; then
  REASON_PARTS+=("zero PASS assertions executed (exit 0 with nothing run is a false green, not a pass)")
fi
if [[ "$COMPLETION_MARKER" != true ]]; then
  REASON_PARTS+=("bundle completion marker 'Universal Forms static and Rust regression gates passed.' absent (run truncated?)")
fi

if [[ ${#REASON_PARTS[@]} -eq 0 ]]; then
  GATE_STATUS="passed"
  GATE_REASON="$FORMS_COMMAND exit 0, $PASS_COUNT assertion(s) passed, 0 failed, completion marker present"
  OVERALL_PASSED=true
else
  GATE_STATUS="failed"
  GATE_REASON="$(printf '%s; ' "${REASON_PARTS[@]}")"
  GATE_REASON="${GATE_REASON%'; '}"
  OVERALL_PASSED=false
fi

echo "  exit=$FORMS_EXIT duration_ms=$DURATION_MS pass_assertions=$PASS_COUNT fail_assertions=$FAIL_COUNT" >&2
echo "  forms_regression_no_degradation: $GATE_STATUS" >&2

RESULT_JSON="$(jq -n \
  --arg schema_version "sylvode.flow.forms-regression-result.v1" \
  --arg release "0.4" \
  --arg head "$SOURCE_HEAD" \
  --argjson dirty "$SOURCE_DIRTY" \
  --arg generated_at "$GENERATED_AT" \
  --arg command "$FORMS_COMMAND" \
  --arg cwd "$REPO_ROOT" \
  --argjson exit_code "$FORMS_EXIT" \
  --argjson duration_ms "$DURATION_MS" \
  --arg evidence "evidence/v0.4/logs/$(basename "$LOG_FILE")" \
  --arg log_path "$LOG_FILE" \
  --arg sha256 "$LOG_SHA256" \
  --argjson pass_count "$PASS_COUNT" \
  --argjson fail_count "$FAIL_COUNT" \
  --argjson completion_marker "$COMPLETION_MARKER" \
  --argjson failed_assertions "$FAILED_ASSERTIONS_JSON" \
  --arg gate_status "$GATE_STATUS" \
  --arg gate_reason "$GATE_REASON" \
  --argjson passed "$OVERALL_PASSED" \
  '{
     schema_version: $schema_version,
     release: $release,
     source_head: $head,
     source_dirty: $dirty,
     generated_at: $generated_at,
     forms_gate_run: {
       command: $command,
       cwd: $cwd,
       exit_code: $exit_code,
       duration_ms: $duration_ms,
       evidence: $evidence,
       log_path: $log_path,
       sha256: $sha256,
       assertions: {
         passed: $pass_count,
         failed: $fail_count,
         completion_marker: $completion_marker,
         failed_names: $failed_assertions
       }
     },
     hard_gates: { forms_regression_no_degradation: $gate_status },
     hard_gate_reasons: { forms_regression_no_degradation: $gate_reason },
     passed: $passed
   }')"

OUT="$EVIDENCE_ROOT/forms-regression-result.json"
printf '%s\n' "$RESULT_JSON" | jq . > "$OUT.tmp"
mv -f "$OUT.tmp" "$OUT"
echo "Evidence written: $OUT" >&2

printf '%s\n' "$RESULT_JSON" | jq .

if [[ "$OVERALL_PASSED" == true ]]; then
  exit 0
fi
exit 1
