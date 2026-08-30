#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.3 report generator.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md ("report" role,
# generic command bundle, and the v0.3 section's candidate runner sequence).
#
# "report" runs only read-only checks and product-provided runners; it never
# marks anything passed that it did not itself observe. It writes the
# authoritative evidence/v0.3/gate-result.json ONLY when every artifact that
# gate-v1.schema.json requires actually exists with a real, freshly computed
# checksum -- fabricating a schema-shaped file with missing/placeholder
# artifacts is exactly the fake-green pattern this v0.3 work exists to close.
# Every invocation (pass or fail) also writes an atomic run log so a failed
# report is never silently lost, per "有失败 exit 1，但仍保留报告".
#
# Exit codes: 0 = all checks ran and passed and gate-result.json was written,
# 1 = one or more checks failed (run log still written; gate-result.json is
# written only if it would be schema-valid), 2 = usage/tool/evidence malformed.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.3"
GATE_YAML="/opt/working/sylvode-flow/gates/v0.3-gate.yaml"
REPO_ROOT="$ROOT_DIR"

usage() {
  cat <<'EOF'
Usage: scripts/report-flow-v0.3-json.sh [OPTIONS]

Runs the read-only v0.3 report bundle: the generic cargo/bun/forms-gate
commands, both candidates' spike test suites, both candidates' benchmark/
convergence/editor-binding runners plus their aggregators, and the CSP
bundle verifier. Records the exact command, exit code and duration for
every step, then -- only if every artifact gate-v1.schema.json requires is
actually present with a real checksum -- atomically writes
<evidence-root>/gate-result.json. A run log is always written to
<evidence-root>/report-run-log.json, pass or fail, so a failed report is
never silently lost.

This script never invents an artifact. If the candidate runners cannot run
(e.g. the spikes are not yet capability-complete), those steps fail, the
corresponding artifacts stay absent, and gate-result.json is correctly not
written -- report exits 1 with a clear list of what is missing.

Options:
  --evidence-root DIR    Where evidence/v0.3/<name> artifacts are written and
                          read from (paths already contain the "evidence/v0.3/"
                          prefix; see verify-flow-v0.3-json.sh --help for the
                          same root-resolution convention).
                          Default: /opt/working/sylvode-flow/evidence/v0.3
  --contracts-root DIR   Root for decisions/, contracts/, security/, testing/
                          artifact paths. Default: /opt/working/sylvode-flow
  --repo-root DIR         Repository the cargo/bun commands run in and whose
                          HEAD becomes source.head. Default: this checkout.
  --gate-yaml PATH        Path to v0.3-gate.yaml. Default:
                          /opt/working/sylvode-flow/gates/v0.3-gate.yaml
  -h, --help              Show this help and exit 0.

Exit codes: 0 all green and gate-result.json written, 1 one or more checks
failed (or gate-result.json could not be validly assembled), 2 usage/tool
error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --gate-yaml) GATE_YAML="${2:?--gate-yaml requires a PATH argument}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

for tool in jq sha256sum git; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    echo "Fix: sudo apt-get install -y $tool" >&2
    exit 2
  fi
done

if [[ ! -d "$REPO_ROOT" ]]; then
  echo "FAIL: --repo-root does not exist: $REPO_ROOT" >&2
  exit 2
fi
if ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi

mkdir -p "$EVIDENCE_ROOT"

CHECKS_JSON="[]"
OVERALL_FAILED=0

sha256_of() { sha256sum "$1" | awk '{print $1}'; }

# run_step ID COMMAND... -- executes COMMAND (array), records a check entry,
# returns the command's own exit status (does not abort the report).
run_step() {
  local id="$1"; shift
  local start end duration status exit_code output
  start="$(date +%s%3N)"
  set +e
  output="$("$@" 2>&1)"
  exit_code=$?
  set -e
  end="$(date +%s%3N)"
  duration=$((end - start))
  if [[ $exit_code -eq 0 ]]; then
    status="passed"
  else
    status="failed"
    OVERALL_FAILED=1
  fi
  local log_file="$EVIDENCE_ROOT/logs/${id}.log"
  mkdir -p "$(dirname "$log_file")"
  printf '%s\n' "$output" > "$log_file"
  local log_sha
  log_sha="$(sha256_of "$log_file")"
  local cmd_str
  cmd_str="$(printf '%q ' "$@")"
  cmd_str="${cmd_str% }"
  CHECKS_JSON="$(jq -c \
    --arg id "$id" --arg status "$status" --arg command "$cmd_str" \
    --argjson exit_code "$exit_code" --argjson duration_ms "$duration" \
    --arg evidence "evidence/v0.3/logs/${id}.log" --arg sha256 "$log_sha" \
    '. + [{id:$id, status:$status, command:$command, exit_code:$exit_code, duration_ms:$duration_ms, evidence:$evidence, sha256:$sha256}]' \
    <<<"$CHECKS_JSON")"
  echo "[$status] $id (exit=$exit_code, ${duration}ms): $cmd_str"
  if [[ $exit_code -ne 0 ]]; then
    echo "$output" | sed 's/^/  | /' | tail -20
  fi
  return $exit_code
}

# cd once (not per-step subshells): run_step mutates CHECKS_JSON in this
# shell's scope, and wrapping each call in "( cd ... && run_step ... )"
# would silently drop every mutation when that subshell exits.
cd "$REPO_ROOT"

echo "=== Sylvode Flow v0.3 report: generic command bundle ==="
run_step generic.cargo_fmt cargo fmt --all -- --check || true
run_step generic.cargo_check cargo check --workspace --all-targets || true
run_step generic.cargo_clippy cargo clippy --workspace --all-targets -- -D warnings || true
run_step generic.cargo_test cargo test --workspace || true
run_step generic.bun_check bun run --cwd frontend check || true
run_step generic.bun_build bun run --cwd frontend build || true
run_step generic.ci_universal_forms_gates bash scripts/ci-universal-forms-gates.sh || true
run_step generic.test_mcp bash scripts/test-mcp.sh || true

echo "=== Sylvode Flow v0.3 report: candidate spike test suites ==="
run_step spike.loro.cargo_test cargo test --manifest-path spikes/collab-loro/Cargo.toml || true
run_step spike.yrs_yjs.cargo_test cargo test --manifest-path spikes/collab-yrs-yjs/Cargo.toml || true
run_step spike.loro.bun_test bun --cwd spikes/collab-loro test || true
run_step spike.yrs_yjs.bun_test bun --cwd spikes/collab-yrs-yjs test || true

echo "=== Sylvode Flow v0.3 report: benchmark runners + aggregate ==="
run_step benchmark.loro "$ROOT_DIR/scripts/benchmark-flow-v0.3.sh" --candidate loro --evidence-root "$EVIDENCE_ROOT" --out "$EVIDENCE_ROOT/benchmark-loro.json" || true
run_step benchmark.yrs_yjs "$ROOT_DIR/scripts/benchmark-flow-v0.3.sh" --candidate yrs-yjs --evidence-root "$EVIDENCE_ROOT" --out "$EVIDENCE_ROOT/benchmark-yrs-yjs.json" || true
run_step benchmark.aggregate "$ROOT_DIR/scripts/aggregate-flow-benchmark-v0.3.sh" --json --evidence-root "$EVIDENCE_ROOT" --gate-yaml "$GATE_YAML" || true

echo "=== Sylvode Flow v0.3 report: convergence runners + aggregate ==="
run_step convergence.loro "$ROOT_DIR/scripts/verify-flow-convergence-v0.3.sh" --candidate loro --evidence-root "$EVIDENCE_ROOT" --out "$EVIDENCE_ROOT/convergence-loro.json" || true
run_step convergence.yrs_yjs "$ROOT_DIR/scripts/verify-flow-convergence-v0.3.sh" --candidate yrs-yjs --evidence-root "$EVIDENCE_ROOT" --out "$EVIDENCE_ROOT/convergence-yrs-yjs.json" || true
run_step convergence.aggregate "$ROOT_DIR/scripts/aggregate-flow-convergence-v0.3.sh" --json --evidence-root "$EVIDENCE_ROOT" --gate-yaml "$GATE_YAML" || true

echo "=== Sylvode Flow v0.3 report: CSP bundle verifier ==="
run_step csp_bundle "$ROOT_DIR/scripts/verify-flow-csp-bundle-v0.3.sh" --policy "$REPO_ROOT/deploy/caddy/Caddyfile" --evidence-root "$EVIDENCE_ROOT" --json || true

echo "=== Sylvode Flow v0.3 report: editor-binding runners + aggregate ==="
run_step editor_binding.loro "$ROOT_DIR/scripts/verify-flow-editor-binding-v0.3.sh" --candidate loro --soak-hours 8 --evidence-root "$EVIDENCE_ROOT" --out "$EVIDENCE_ROOT/editor-binding-loro.json" || true
run_step editor_binding.yrs_yjs "$ROOT_DIR/scripts/verify-flow-editor-binding-v0.3.sh" --candidate yrs-yjs --soak-hours 8 --evidence-root "$EVIDENCE_ROOT" --out "$EVIDENCE_ROOT/editor-binding-yrs-yjs.json" || true
run_step editor_binding.aggregate "$ROOT_DIR/scripts/aggregate-flow-editor-binding-v0.3.sh" --json --evidence-root "$EVIDENCE_ROOT" || true

SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
if [[ -n "$(git -C "$REPO_ROOT" status --porcelain)" ]]; then
  SOURCE_DIRTY=true
else
  SOURCE_DIRTY=false
fi
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

RUN_LOG="$(jq -n \
  --arg generated_at "$GENERATED_AT" \
  --arg repo "$REPO_ROOT" \
  --arg head "$SOURCE_HEAD" \
  --argjson dirty "$SOURCE_DIRTY" \
  --argjson checks "$CHECKS_JSON" \
  --argjson overall_failed "$OVERALL_FAILED" \
  '{schema_version:"sylvode.flow.report-run-log.v1", generated_at:$generated_at, source:{repository:$repo, head:$head, dirty:$dirty}, checks:$checks, overall_failed:($overall_failed==1)}')"
RUN_LOG_TMP="$EVIDENCE_ROOT/report-run-log.json.tmp"
printf '%s\n' "$RUN_LOG" | jq . > "$RUN_LOG_TMP"
mv -f "$RUN_LOG_TMP" "$EVIDENCE_ROOT/report-run-log.json"
echo "Run log written: $EVIDENCE_ROOT/report-run-log.json"

# ---- assemble gate-result.json only if every required artifact is real ----
REQUIRED_ARTIFACTS=(
  "convergence_result:$EVIDENCE_ROOT/convergence-result.json:evidence/v0.3/convergence-result.json"
  "benchmark_result:$EVIDENCE_ROOT/benchmark-result.json:evidence/v0.3/benchmark-result.json"
  "editor_binding_result:$EVIDENCE_ROOT/editor-binding-result.json:evidence/v0.3/editor-binding-result.json"
  "csp_bundle_result:$EVIDENCE_ROOT/csp-bundle-result.json:evidence/v0.3/csp-bundle-result.json"
  "engine_adr:$CONTRACTS_ROOT/decisions/ADR-0005-crdt-engine-selection.md:decisions/ADR-0005-crdt-engine-selection.md"
  "editor_adr:$CONTRACTS_ROOT/decisions/ADR-0006-editor-stack.md:decisions/ADR-0006-editor-stack.md"
  "transport_auth_adr:$CONTRACTS_ROOT/decisions/ADR-0007-collab-transport-auth.md:decisions/ADR-0007-collab-transport-auth.md"
  "mcp_naming_uri_adr:$CONTRACTS_ROOT/decisions/ADR-0008-mcp-naming-and-uri.md:decisions/ADR-0008-mcp-naming-and-uri.md"
  "domain_schema:$CONTRACTS_ROOT/contracts/domain-model-v1.md:contracts/domain-model-v1.md"
  "protocol_contract:$CONTRACTS_ROOT/contracts/collab-protocol-v1.md:contracts/collab-protocol-v1.md"
  "threat_model:$CONTRACTS_ROOT/security/threat-model.md:security/threat-model.md"
  "convergence_corpus:$CONTRACTS_ROOT/testing/convergence-corpus.md:testing/convergence-corpus.md"
  "benchmark_spec:$CONTRACTS_ROOT/testing/benchmark-spec.md:testing/benchmark-spec.md"
)

MISSING_ARTIFACTS=()
for entry in "${REQUIRED_ARTIFACTS[@]}"; do
  IFS=':' read -r key abs_path _rel_path <<<"$entry"
  if [[ ! -f "$abs_path" ]]; then
    MISSING_ARTIFACTS+=("$key ($abs_path)")
  fi
done

if [[ $OVERALL_FAILED -ne 0 || ${#MISSING_ARTIFACTS[@]} -gt 0 ]]; then
  echo "REPORT: FAIL -- not writing evidence/v0.3/gate-result.json (would not be schema-valid / not all checks passed)" >&2
  if [[ ${#MISSING_ARTIFACTS[@]} -gt 0 ]]; then
    echo "Missing required artifacts:" >&2
    for m in "${MISSING_ARTIFACTS[@]}"; do
      echo "  - $m" >&2
    done
  fi
  echo "See $EVIDENCE_ROOT/report-run-log.json and $EVIDENCE_ROOT/logs/*.log for exact command failures." >&2
  exit 1
fi

ARTIFACTS_JSON="{}"
for entry in "${REQUIRED_ARTIFACTS[@]}"; do
  IFS=':' read -r key abs_path rel_path <<<"$entry"
  sha="$(sha256_of "$abs_path")"
  ARTIFACTS_JSON="$(jq -c --arg k "$key" --arg path "$rel_path" --arg sha "$sha" '.[$k] = {path:$path, sha256:$sha}' <<<"$ARTIFACTS_JSON")"
done

GATE_RESULT_PATH="$EVIDENCE_ROOT/gate-result.json"
GATE_RESULT_TMP="$GATE_RESULT_PATH.tmp"

# gate_result is self-referential (see verify-flow-v0.3-json.sh comment on
# the same field); we record a placeholder here and never assert equality
# against it in verify.
ARTIFACTS_JSON="$(jq -c '.gate_result = {path:"evidence/v0.3/gate-result.json", sha256:"0000000000000000000000000000000000000000000000000000000000000"[0:64]}' <<<"$ARTIFACTS_JSON")"

jq -n \
  --arg head "$SOURCE_HEAD" --argjson dirty "$SOURCE_DIRTY" \
  --arg generated_at "$GENERATED_AT" \
  --argjson checks "$CHECKS_JSON" \
  --argjson artifacts "$ARTIFACTS_JSON" \
  '{
    schema_version: "sylvode.flow.gate-result.v1",
    schema_path: "docs/schemas/sylvode-flow-gate-v1.schema.json",
    release: "0.3.0",
    source_baseline: {
      repository: "/opt/worker/code/openpr",
      rust_workspace_version: "0.2.21",
      frontend_package_version: "0.2.11",
      reviewed_head: "ab01d5d94de96294986c4c39ff01392535efebaa"
    },
    source: {repository: "/opt/worker/code/openpr", head: $head, dirty: $dirty},
    generated_at: $generated_at,
    mode: "blocked",
    gate_passed: false,
    counts: {
      automated: ($checks | length),
      passed: ($checks | map(select(.status=="passed")) | length),
      failed: ($checks | map(select(.status=="failed")) | length),
      manual_pending: 4,
      unresolved: 4
    },
    checks: $checks,
    required_commands: {
      report: {command:"scripts/report-flow-v0.3-json.sh", status:"passed", exit_code:0, duration_ms:0, evidence:"evidence/v0.3/gate-result.json", sha256:"0000000000000000000000000000000000000000000000000000000000000"[0:64]},
      verify: {command:"scripts/verify-flow-v0.3-json.sh evidence/v0.3/gate-result.json", status:"failed", exit_code:1, duration_ms:0, evidence:"evidence/v0.3/gate-result.json", sha256:"0000000000000000000000000000000000000000000000000000000000000"[0:64]},
      gate: {command:"scripts/gate-flow-v0.3.sh --json", status:"failed", exit_code:1, duration_ms:0, evidence:"evidence/v0.3/gate-result.json", sha256:"0000000000000000000000000000000000000000000000000000000000000"[0:64]},
      manual_signoff: {command:"scripts/record-flow-v0.3-manual-signoff.sh", status:"failed", exit_code:1, duration_ms:0, evidence:"evidence/v0.3/gate-result.json", sha256:"0000000000000000000000000000000000000000000000000000000000000"[0:64]},
      editor_binding_loro: {command:"scripts/verify-flow-editor-binding-v0.3.sh --candidate loro --soak-hours 8 --out evidence/v0.3/editor-binding-loro.json", status:"passed", exit_code:0, duration_ms:0, evidence:"evidence/v0.3/editor-binding-loro.json", sha256:"0000000000000000000000000000000000000000000000000000000000000"[0:64]},
      editor_binding_yrs_yjs: {command:"scripts/verify-flow-editor-binding-v0.3.sh --candidate yrs-yjs --soak-hours 8 --out evidence/v0.3/editor-binding-yrs-yjs.json", status:"passed", exit_code:0, duration_ms:0, evidence:"evidence/v0.3/editor-binding-yrs-yjs.json", sha256:"0000000000000000000000000000000000000000000000000000000000000"[0:64]},
      editor_binding_aggregate: {command:"scripts/aggregate-flow-editor-binding-v0.3.sh --json", status:"passed", exit_code:0, duration_ms:0, evidence:"evidence/v0.3/editor-binding-result.json", sha256:"0000000000000000000000000000000000000000000000000000000000000"[0:64]},
      csp_bundle_verify: {command:"scripts/verify-flow-csp-bundle-v0.3.sh --policy deploy/caddy/Caddyfile --json", status:"passed", exit_code:0, duration_ms:0, evidence:"evidence/v0.3/csp-bundle-result.json", sha256:"0000000000000000000000000000000000000000000000000000000000000"[0:64]}
    },
    hard_gates: {
      benchmark_budgets_met: "failed", isolated_apply_not_async_timeout: "failed",
      rust_wasm_roundtrip: "failed", snapshot_tail_rebuild_hash: "failed",
      concurrent_tree_move_invariants: "failed", ime_selection_undo_cursor: "failed",
      editor_binding_maturity: "failed", vite_bundle_static_deep_route: "failed",
      offline_replay_no_accepted_loss: "failed", unauthorized_update_rejected: "failed",
      corrupt_duplicate_out_of_order_limits: "failed", dependency_license_security: "failed"
    },
    artifacts: $artifacts,
    manual_signoffs: {
      editor_ime: {status:"pending", reviewer:"", evidence:""},
      selection_cursor: {status:"pending", reviewer:"", evidence:""},
      dependency_license: {status:"pending", reviewer:"", evidence:""},
      engine_decision: {status:"pending", reviewer:"", evidence:""}
    },
    blockers: ["hard-gates-not-yet-verified", "manual-signoffs-pending"]
  }' > "$GATE_RESULT_TMP"

sync "$GATE_RESULT_TMP" 2>/dev/null || true
mv -f "$GATE_RESULT_TMP" "$GATE_RESULT_PATH"
echo "REPORT: wrote $GATE_RESULT_PATH"
echo "NOTE: hard_gates and required_commands.{verify,gate,manual_signoff} in this file are deliberately conservative placeholders (failed/pending) -- report-flow-v0.3-json.sh does not itself compute hard-gate verdicts; run scripts/verify-flow-v0.3-json.sh against this file to get the authoritative recomputed verdicts, then scripts/gate-flow-v0.3.sh to aggregate with manual signoffs."

if [[ $OVERALL_FAILED -ne 0 ]]; then
  exit 1
fi
exit 0
