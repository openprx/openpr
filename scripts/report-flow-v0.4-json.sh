#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 report generator.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md ("report" role,
# generic command bundle, and the v0.4 section's required_commands list)
# and /opt/working/sylvode-flow/gates/v0.4-gate.yaml.
#
# "report" runs only read-only checks and the product-provided verify
# scripts; it never marks anything passed that it did not itself observe,
# and it never invents an artifact. It writes evidence/v0.4/gate-result.json
# whenever every artifact docs/schemas/sylvode-flow-gate-v0.4.schema.json
# requires actually exists with a real, freshly computed checksum, even when
# one or more checks failed (the frozen report contract requires exit 1 while
# preserving gate-result.json in that case) --
# fabricating a schema-shaped file with missing/placeholder artifacts is
# exactly the fake-green pattern this v0.4 work exists to close.
#
# Every required verification command that has a repository producer is run
# here exactly once. In particular, forms_regression_verify is the canonical
# wrapper around ci-universal-forms-gates.sh; the generic bundle must not invoke
# that same entrypoint a second time. The deployed verifier reports an explicit failed gate when its real
# three-hop environment is unavailable; report invokes it instead of pretending
# its script is absent.
#
# Every invocation (pass or fail) records executed_count=1 and also writes an atomic run log so a
# failed report is never silently lost, per "有失败 exit 1，但仍保留报告".
#
# Exit codes: 0 = all required checks passed and gate-result.json was written.
# The optional compose-style generic.test_mcp probe may instead be recorded as
# environment_unavailable only when its stronger required MCP verifiers pass;
# this remains visible in checks/counts and is never rewritten as passed.
# Exit 1 = one or more checks failed or a required artifact is missing
# (run log is always written; gate-result.json is also written when all
# required artifacts exist), 2 = usage/tool/evidence malformed.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT=""
REPO_ROOT="$ROOT_DIR"
SKIP_GENERIC=0
LOAD_HARNESS_EVIDENCE=""
CACHE_EVIDENCE=""
DEDICATED_PG_CONTAINER="${OPENPR_FLOW_DEDICATED_PG_CONTAINER:-}"
RECEIPT_STATE_FILTER="$ROOT_DIR/scripts/lib/flow_gate_v0_4_receipt_state.jq"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/sylvode-flow-gate-v0.4.schema.json"

usage() {
  cat <<'EOF'
Usage: scripts/report-flow-v0.4-json.sh [OPTIONS]

Runs the read-only v0.4 report bundle: the generic cargo/bun/MCP commands,
then every required_commands verify step v0.4-gate.yaml names. The required
Forms verifier owns the single Universal Forms gate execution.
Records the exact command, exit code and duration for every step, then --
only if every artifact the v0.4 gate schema requires is actually present
with a real checksum -- atomically writes <evidence-root>/gate-result.json.
A run log is always written to <evidence-root>/report-run-log.json, pass
or fail, so a failed report is never silently lost.

This script never invents an artifact. Missing required artifacts remain
explicit blockers and make report exit 1.

Options:
  --evidence-root DIR    Required. Where evidence artifacts are written and read.
  --contracts-root DIR   Root for decisions/, contracts/, security/
                          artifact paths. Default: /opt/working/sylvode-flow
  --repo-root DIR         Repository the cargo/bun commands run in and
                          whose HEAD becomes source.head. Default: this
                          checkout.
  --load-harness-evidence PATH
                          Load-harness JSON passed to the architecture
                          verifier. Default: <evidence-root>/load-harness-result.json.
  --cache-evidence PATH  Cache-harness JSON passed to the architecture verifier.
                         Default: <evidence-root>/cache-evidence-result.json.
  --dedicated-pg-container NAME
                          Explicit dedicated PostgreSQL container declaration
                          passed through to the architecture verifier. Default:
                          $OPENPR_FLOW_DEDICATED_PG_CONTAINER.
  --skip-generic          Skip the generic cargo fmt/check/clippy/test +
                          bun check/build + test-mcp bundle (fast iteration
                          only; the required Forms verifier still runs once;
                          report
                          will still correctly fail to write
                          gate-result.json because those checks are
                          required).
  -h, --help              Show this help and exit 0.

Exit codes: 0 all required checks green and gate-result.json written
(generic.test_mcp may be visibly environment_unavailable only when stronger
required MCP coverage passes), 1 one or more checks failed / artifacts
missing, 2 usage/tool error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --load-harness-evidence) LOAD_HARNESS_EVIDENCE="${2:?--load-harness-evidence requires a PATH}"; shift 2 ;;
    --cache-evidence) CACHE_EVIDENCE="${2:?--cache-evidence requires a PATH}"; shift 2 ;;
    --dedicated-pg-container) DEDICATED_PG_CONTAINER="${2:?--dedicated-pg-container requires a NAME}"; shift 2 ;;
    --skip-generic) SKIP_GENERIC=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$EVIDENCE_ROOT" ]]; then
  echo "FAIL: --evidence-root is required; evidence must never default into the contract repository" >&2
  exit 2
fi

for tool in jq sha256sum git python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    echo "Fix: sudo apt-get install -y $tool" >&2
    exit 2
  fi
done
if [[ ! -f "$RECEIPT_STATE_FILTER" ]]; then
  echo "FAIL: receipt-state filter not found: $RECEIPT_STATE_FILTER" >&2
  exit 2
fi
if [[ ! -f "$SCHEMA_PATH" ]]; then
  echo "FAIL: v0.4 gate schema not found: $SCHEMA_PATH" >&2
  exit 2
fi
GATE_YAML="$CONTRACTS_ROOT/gates/v0.4-gate.yaml"
if [[ ! -f "$GATE_YAML" ]]; then
  echo "FAIL: v0.4 gate contract not found: $GATE_YAML" >&2
  exit 2
fi

if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi
REPO_ROOT="$(cd "$REPO_ROOT" && pwd)"

mkdir -p "$EVIDENCE_ROOT"
[[ -n "$LOAD_HARNESS_EVIDENCE" ]] || LOAD_HARNESS_EVIDENCE="$EVIDENCE_ROOT/load-harness-result.json"
[[ -n "$CACHE_EVIDENCE" ]] || CACHE_EVIDENCE="$EVIDENCE_ROOT/cache-evidence-result.json"

CHECKS_JSON="[]"
OVERALL_FAILED=0

sha256_of() { sha256sum "$1" | awk '{print $1}'; }

# Read only the top-level scalar maps used by the frozen gate ledger. This is
# intentionally the same bounded parser shape used by the newer v0.5 report;
# a nested/new YAML form cannot silently disappear because its key set must
# still equal the JSON schema before any product command runs.
yaml_map_json() {
  local section="$1"
  awk -v section="$section" '
    function trim(s) { sub(/^[[:space:]]+/, "", s); sub(/[[:space:]]+$/, "", s); return s }
    $0 == section ":" { inside=1; next }
    inside && $0 ~ /^[^[:space:]#]/ { exit }
    inside && $0 ~ /^  [A-Za-z0-9_]+:/ {
      line=substr($0,3); split_at=index(line, ":")
      key=substr(line,1,split_at-1); value=trim(substr(line,split_at+1))
      sub(/[[:space:]]+#.*$/, "", value)
      printf "%s\t%s\n", key, value
    }
  ' "$GATE_YAML" | jq -Rn '
    [inputs | capture("^(?<key>[^\\t]+)\\t(?<value>.*)$") | {key:.key,value:.value}] | from_entries'
}

YAML_ARTIFACTS="$(yaml_map_json artifacts)"
YAML_REQUIRED_COMMANDS="$(yaml_map_json required_commands)"
YAML_HARD_GATES="$(yaml_map_json hard_gates)"
SCHEMA_ARTIFACT_KEYS="$(jq -c '.properties.artifacts.required | sort' "$SCHEMA_PATH")"
SCHEMA_REQUIRED_COMMAND_KEYS="$(jq -c '.properties.required_commands.required | sort' "$SCHEMA_PATH")"
SCHEMA_HARD_GATE_KEYS="$(jq -c '.properties.hard_gates.required | sort' "$SCHEMA_PATH")"
EXPECTED_HARD_GATE_COUNT="$(jq 'length' <<<"$SCHEMA_HARD_GATE_KEYS")"

for ledger_section in artifacts required_commands hard_gates; do
  case "$ledger_section" in
    artifacts) yaml_json="$YAML_ARTIFACTS"; schema_keys="$SCHEMA_ARTIFACT_KEYS" ;;
    required_commands) yaml_json="$YAML_REQUIRED_COMMANDS"; schema_keys="$SCHEMA_REQUIRED_COMMAND_KEYS" ;;
    hard_gates) yaml_json="$YAML_HARD_GATES"; schema_keys="$SCHEMA_HARD_GATE_KEYS" ;;
  esac
  if ! jq -e --argjson expected "$schema_keys" 'keys == $expected' >/dev/null <<<"$yaml_json"; then
    echo "FAIL: v0.4 ledger drift: YAML $ledger_section keys do not equal schema required keys" >&2
    exit 2
  fi
done

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
  elif [[ "$id" == "generic.bun_check" || "$id" == "generic.bun_build" ]]; then
    # ADR-0017 moved every UI/TypeScript criterion out of the v0.4 release
    # gate. Keep the attempted command and its real exit visible, but do not
    # turn an unavailable frontend toolchain into a backend-track blocker.
    status="deferred_to_frontend_track"
  elif [[ "$id" == "generic.test_mcp" && $exit_code -eq 69 ]]; then
    # test-mcp.sh actively probes its configured endpoint. Exit 69 means the
    # external compose-style MCP environment is unavailable, not that product
    # behavior failed. The receipt keeps that fact visible; the shared state
    # derivation only clears it as a blocker after the required live
    # three-transport and full-registry verifiers both pass.
    status="environment_unavailable"
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
    --argjson executed_count 1 \
    --arg evidence "evidence/v0.4/logs/${id}.log" --arg sha256 "$log_sha" \
    '. + [{id:$id, status:$status, command:$command, exit_code:$exit_code, duration_ms:$duration_ms, executed_count:$executed_count, evidence:$evidence, sha256:$sha256}]' \
    <<<"$CHECKS_JSON")"
  echo "[$status] $id (exit=$exit_code, ${duration}ms): $cmd_str"
  if [[ $exit_code -ne 0 ]]; then
    echo "$output" | sed 's/^/  | /' | tail -20
  fi
  return $exit_code
}

cd "$REPO_ROOT"

if [[ $SKIP_GENERIC -eq 1 ]]; then
  echo "=== Sylvode Flow v0.4 report: generic command bundle SKIPPED (--skip-generic) ===" >&2
else
  echo "=== Sylvode Flow v0.4 report: generic command bundle ==="
  run_step generic.cargo_fmt cargo fmt --all -- --check || true
  run_step generic.cargo_check cargo check --workspace --all-targets || true
  run_step generic.cargo_clippy cargo clippy --workspace --all-targets -- -D warnings || true
  run_step generic.cargo_test cargo test --workspace --no-fail-fast || true
  run_step generic.bun_check bun run --cwd frontend check || true
  run_step generic.bun_build bun run --cwd frontend build || true
  run_step generic.test_mcp bash scripts/test-mcp.sh || true
fi

echo "=== Sylvode Flow v0.4 report: surface coverage ==="
run_step required.surface_parity "$ROOT_DIR/scripts/verify-flow-surface-coverage.sh" --release 0.4 --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: legacy pages inventory + entry verify ==="
run_step required.legacy_pages_inventory "$ROOT_DIR/scripts/inventory-flow-legacy-pages.sh" --environments development,test,target_deployment --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json || true
run_step required.legacy_pages_entry_verify "$ROOT_DIR/scripts/verify-flow-legacy-pages-v0.4.sh" "$EVIDENCE_ROOT/legacy-pages-inventory.json" --evidence-root "$EVIDENCE_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: cardinality verify ==="
run_step required.cardinality_verify "$ROOT_DIR/scripts/verify-flow-cardinality-v0.4.sh" --adr "$CONTRACTS_ROOT/decisions/ADR-0013-multi-document-atomicity.md" --max-cardinality 1 --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: integrity-records + authz-baseline (live api binary) ==="
run_step required.integrity_records_verify "$ROOT_DIR/scripts/verify-flow-integrity-records-v0.4.sh" --adr "$CONTRACTS_ROOT/decisions/ADR-0013-multi-document-atomicity.md" --repo-root "$REPO_ROOT" --evidence-root "$EVIDENCE_ROOT" --json || true
run_step required.authz_baseline_verify "$ROOT_DIR/scripts/verify-flow-authz-baseline-v0.4.sh" --adr "$CONTRACTS_ROOT/decisions/ADR-0012-object-authorization-and-sharing.md" --repo-root "$REPO_ROOT" --evidence-root "$EVIDENCE_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: live REST success contract + document integrity ==="
run_step required.rest_contract_verify "$ROOT_DIR/scripts/verify-flow-rest-contract-v0.4.sh" --release 0.4 --repo-root "$REPO_ROOT" --evidence-root "$EVIDENCE_ROOT" --json || true
run_step required.document_integrity_verify "$ROOT_DIR/scripts/verify-flow-document-integrity-v0.4.sh" --release 0.4 --repo-root "$REPO_ROOT" --evidence-root "$EVIDENCE_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: collab-architecture verify ==="
ARCHITECTURE_ARGS=(
  --release 0.4
  --adr "$CONTRACTS_ROOT/decisions/ADR-0010-collab-server-architecture.md"
  --limits "$CONTRACTS_ROOT/contracts/limits-v1.md"
  --load-harness-evidence "$LOAD_HARNESS_EVIDENCE"
  --cache-evidence "$CACHE_EVIDENCE"
  --contracts-root "$CONTRACTS_ROOT"
  --evidence-root "$EVIDENCE_ROOT"
  --repo-root "$REPO_ROOT"
  --json
)
if [[ -n "$DEDICATED_PG_CONTAINER" ]]; then
  ARCHITECTURE_ARGS+=(--dedicated-pg-container "$DEDICATED_PG_CONTAINER")
fi
run_step required.collab_architecture_verify "$ROOT_DIR/scripts/verify-flow-collab-architecture.sh" "${ARCHITECTURE_ARGS[@]}" || true

echo "=== Sylvode Flow v0.4 report: events/dispatch verify ==="
run_step required.events_verify "$ROOT_DIR/scripts/verify-flow-events-v0.4.sh" --contract "$CONTRACTS_ROOT/contracts/events-v1.md" --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: limits verify ==="
run_step required.limits_verify "$ROOT_DIR/scripts/verify-flow-limits-v0.4.sh" --contract "$CONTRACTS_ROOT/contracts/limits-v1.md" --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: error-contract verify ==="
run_step required.error_contract_verify "$ROOT_DIR/scripts/verify-flow-errors-v0.4.sh" --contract "$CONTRACTS_ROOT/contracts/error-mapping-v1.md" --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: forms regression (no degradation) ==="
run_step required.forms_regression_verify "$ROOT_DIR/scripts/verify-flow-forms-regression-v0.4.sh" --repo-root "$REPO_ROOT" --evidence-root "$EVIDENCE_ROOT" --json || true
echo "=== Sylvode Flow v0.4 report: migration forward/rollback verify ==="
run_step required.migration_verify "$ROOT_DIR/scripts/verify-flow-migration-v0.4.sh" --migration migrations/0054_flow_data_layer.sql --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: document row lock / seq uniqueness verify ==="
run_step required.document_seq_verify "$ROOT_DIR/scripts/verify-flow-document-seq-v0.4.sh" --concurrency 8 --rounds 3 --instances 3 --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: MCP three-transport contract verify ==="
run_step required.mcp_transport_verify "$ROOT_DIR/scripts/verify-flow-mcp-transports-v0.4.sh" --transports http,sse,stdio --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: feature flag surfaces + disabled UI verify ==="
run_step required.feature_flag_verify "$ROOT_DIR/scripts/verify-flow-feature-flags-v0.4.sh" --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: MCP tool registry count verify ==="
run_step required.tool_registry_verify "$ROOT_DIR/scripts/verify-flow-tool-registry-v0.4.sh" --baseline "$CONTRACTS_ROOT/contracts/tool-count-baseline.md" --release 0.4 --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: CLI JSON/exit-code contract verify ==="
run_step required.cli_contract_verify "$ROOT_DIR/scripts/verify-flow-cli-contract-v0.4.sh" --contract "$CONTRACTS_ROOT/contracts/error-mapping-v1.md" --release 0.4 --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: collab transport auth (ticket / cookie / unauthorized update) ==="
run_step required.transport_auth_verify "$ROOT_DIR/scripts/verify-flow-transport-auth-v0.4.sh" --adr "$CONTRACTS_ROOT/decisions/ADR-0007-collab-transport-auth.md" --repo-root "$REPO_ROOT" --evidence-root "$EVIDENCE_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: cross-workspace / policy-bypass negatives ==="
run_step required.cross_workspace_verify "$ROOT_DIR/scripts/verify-flow-cross-workspace-v0.4.sh" --threat-model "$CONTRACTS_ROOT/security/threat-model.md" --repo-root "$REPO_ROOT" --evidence-root "$EVIDENCE_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: deployed three-hop WebSocket verify ==="
run_step required.deployed_chain_websocket_upgrade "$ROOT_DIR/scripts/verify-flow-deployed-websocket-v0.4.sh" --chain caddy,nginx,api --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json || true

# Every required command before report/verify/gate/manual_signoff is a producer
# owned by this report run. Check the runtime ledger rather than trusting the
# presence of a hard-coded shell line: deleting an invocation, duplicating one,
# or recording zero executions must make this run red.
PRODUCER_KEYS="$(jq -r 'keys[] | select(. != "report" and . != "verify" and . != "gate" and . != "manual_signoff")' <<<"$YAML_REQUIRED_COMMANDS")"
PRODUCER_EXECUTION_ERRORS=()
while IFS= read -r key; do
  [[ -z "$key" ]] && continue
  check_id="required.$key"
  matching_count="$(jq --arg id "$check_id" '[.[] | select(.id == $id)] | length' <<<"$CHECKS_JSON")"
  executed_count="$(jq --arg id "$check_id" '[.[] | select(.id == $id) | .executed_count] | add // 0' <<<"$CHECKS_JSON")"
  if [[ "$matching_count" -ne 1 || "$executed_count" -ne 1 ]]; then
    PRODUCER_EXECUTION_ERRORS+=("$key: matching_checks=$matching_count executed_count=$executed_count")
    OVERALL_FAILED=1
  fi
done <<<"$PRODUCER_KEYS"
if [[ ${#PRODUCER_EXECUTION_ERRORS[@]} -gt 0 ]]; then
  echo "FAIL: required producer execution ledger is incomplete or duplicated:" >&2
  printf '  - %s\n' "${PRODUCER_EXECUTION_ERRORS[@]}" >&2
fi

SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
RUST_WORKSPACE_VERSION="$(sed -n '/^\[workspace.package\]$/,/^\[/s/^version = "\([^"]*\)"/\1/p' "$REPO_ROOT/Cargo.toml" | head -1)"
FRONTEND_PACKAGE_VERSION="$(jq -r '.version // empty' "$REPO_ROOT/frontend/package.json")"
if [[ -z "$RUST_WORKSPACE_VERSION" || -z "$FRONTEND_PACKAGE_VERSION" ]]; then
  echo "FAIL: could not derive workspace/frontend versions from the checked-out source" >&2
  exit 2
fi
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
  '{schema_version:"sylvode.flow.report-run-log.v1", release:"0.4.0", generated_at:$generated_at, source:{repository:$repo, head:$head, dirty:$dirty}, checks:$checks, overall_failed:($overall_failed==1)}')"
RUN_LOG_TMP="$EVIDENCE_ROOT/report-run-log.json.tmp"
printf '%s\n' "$RUN_LOG" | jq . > "$RUN_LOG_TMP"
mv -f "$RUN_LOG_TMP" "$EVIDENCE_ROOT/report-run-log.json"
echo "Run log written: $EVIDENCE_ROOT/report-run-log.json"

# ---- assemble gate-result.json only if every required artifact is real ----
REQUIRED_ARTIFACTS=(
  "migration:$REPO_ROOT/migrations/0054_flow_data_layer.sql:migrations/0054_flow_data_layer.sql"
  "api_contract_fixture:$EVIDENCE_ROOT/rest-contract-result.json:evidence/v0.4/rest-contract-result.json"
  "error_contract_result:$EVIDENCE_ROOT/error-contract-result.json:evidence/v0.4/error-contract-result.json"
  "mcp_contract_fixture:$EVIDENCE_ROOT/mcp-contract-result.json:evidence/v0.4/mcp-contract-result.json"
  "feature_flag_result:$EVIDENCE_ROOT/feature-flag-result.json:evidence/v0.4/feature-flag-result.json"
  "cli_contract_fixture:$EVIDENCE_ROOT/cli-contract-result.json:evidence/v0.4/cli-contract-result.json"
  "ui_e2e_result:$EVIDENCE_ROOT/ui-e2e-result.json:evidence/v0.4/ui-e2e-result.json"
  "deployed_chain_websocket_result:$EVIDENCE_ROOT/deployed-chain-websocket-result.json:evidence/v0.4/deployed-chain-websocket-result.json"
  "integrity_result:$EVIDENCE_ROOT/document-integrity-result.json:evidence/v0.4/document-integrity-result.json"
  "collab_architecture_result:$EVIDENCE_ROOT/collab-architecture-result.json:evidence/v0.4/collab-architecture-result.json"
  "forms_regression_result:$EVIDENCE_ROOT/forms-regression-result.json:evidence/v0.4/forms-regression-result.json"
  "limits_result:$EVIDENCE_ROOT/limits-result.json:evidence/v0.4/limits-result.json"
  "flow_events_result:$EVIDENCE_ROOT/flow-events-result.json:evidence/v0.4/flow-events-result.json"
  "cardinality_result:$EVIDENCE_ROOT/cardinality-result.json:evidence/v0.4/cardinality-result.json"
  "integrity_records_result:$EVIDENCE_ROOT/integrity-records-result.json:evidence/v0.4/integrity-records-result.json"
  "authz_baseline_result:$EVIDENCE_ROOT/authz-baseline-result.json:evidence/v0.4/authz-baseline-result.json"
  "surface_coverage_result:$EVIDENCE_ROOT/surface-coverage-result.json:evidence/v0.4/surface-coverage-result.json"
  "migration_result:$EVIDENCE_ROOT/migration-result.json:evidence/v0.4/migration-result.json"
  "document_seq_result:$EVIDENCE_ROOT/document-seq-result.json:evidence/v0.4/document-seq-result.json"
  "tool_registry_result:$EVIDENCE_ROOT/tool-registry-result.json:evidence/v0.4/tool-registry-result.json"
  "legacy_pages_inventory:$EVIDENCE_ROOT/legacy-pages-inventory.json:evidence/v0.4/legacy-pages-inventory.json"
)

REPORT_ARTIFACT_KEYS="$(printf '%s\n' "${REQUIRED_ARTIFACTS[@]}" | awk -F: '{print $1}' | jq -R . | jq -sc 'sort + ["gate_result"] | unique')"
if [[ "$REPORT_ARTIFACT_KEYS" != "$SCHEMA_ARTIFACT_KEYS" ]]; then
  echo "FAIL: report REQUIRED_ARTIFACTS keys do not equal the aligned YAML/schema artifact ledger" >&2
  exit 2
fi

MISSING_ARTIFACTS=()
for entry in "${REQUIRED_ARTIFACTS[@]}"; do
  IFS=':' read -r key abs_path _rel_path <<<"$entry"
  if [[ ! -f "$abs_path" ]]; then
    MISSING_ARTIFACTS+=("$key ($abs_path)")
  fi
done

if [[ ${#MISSING_ARTIFACTS[@]} -gt 0 ]]; then
  echo "REPORT: FAIL -- not writing evidence/v0.4/gate-result.json because required artifacts are missing" >&2
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
ARTIFACTS_JSON="$(jq -c '.gate_result = {path:"evidence/v0.4/gate-result.json", sha256:("0" * 64)}' <<<"$ARTIFACTS_JSON")"

echo "REPORT: all required artifacts present -- independently recomputing $EXPECTED_HARD_GATE_COUNT hard gates" >&2
set +e
RECOMPUTE_JSON="$(python3 "$ROOT_DIR/scripts/lib/flow_gate_v0_4_recompute.py" \
  --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT")"
RECOMPUTE_EXIT=$?
set -e
if [[ $RECOMPUTE_EXIT -ne 0 ]] || ! jq -e --argjson expected_count "$EXPECTED_HARD_GATE_COUNT" --argjson expected_keys "$SCHEMA_HARD_GATE_KEYS" '
  type == "object"
  and (.hard_gates | type == "object" and length == $expected_count and (keys | sort) == $expected_keys)
  and (.reasons | type == "object")
' >/dev/null 2>&1 <<<"$RECOMPUTE_JSON"; then
  echo "FAIL: hard-gate recomputation failed or did not return the exact aligned $EXPECTED_HARD_GATE_COUNT-key ledger" >&2
  [[ -n "$RECOMPUTE_JSON" ]] && echo "$RECOMPUTE_JSON" >&2
  exit 2
fi
HARD_GATES_JSON="$(jq -c '.hard_gates' <<<"$RECOMPUTE_JSON")"
HARD_GATE_NON_PASS_COUNT="$(jq '[.hard_gates[] | select(. != "passed")] | length' <<<"$RECOMPUTE_JSON")"
echo "REPORT: recomputed hard gates: $(jq -c '.hard_gates | to_entries | group_by(.value) | map({(.[0].value):length}) | add' <<<"$RECOMPUTE_JSON")" >&2

get_check() {
  jq -c --arg id "$1" '[.[] | select(.id==$id)][0] // {status:"failed",command:"(not run)",exit_code:2,duration_ms:0,executed_count:0,evidence:"",sha256:("0" * 64)}' <<<"$CHECKS_JSON" | \
    jq -c '{command:.command, status:.status, exit_code:.exit_code, duration_ms:.duration_ms, executed_count:.executed_count, evidence:.evidence, sha256:.sha256}'
}
ZERO_SHA="$(printf '%064d' 0)"
if [[ $OVERALL_FAILED -eq 0 ]]; then
  REPORT_COMMAND_STATUS="passed"
  REPORT_COMMAND_EXIT=0
else
  REPORT_COMMAND_STATUS="failed"
  REPORT_COMMAND_EXIT=1
fi

REQUIRED_COMMANDS_JSON="$(jq -n \
  --argjson surface_parity "$(get_check required.surface_parity)" \
  --argjson legacy_pages_inventory "$(get_check required.legacy_pages_inventory)" \
  --argjson legacy_pages_entry_verify "$(get_check required.legacy_pages_entry_verify)" \
  --argjson collab_architecture_verify "$(get_check required.collab_architecture_verify)" \
  --argjson rest_contract_verify "$(get_check required.rest_contract_verify)" \
  --argjson document_integrity_verify "$(get_check required.document_integrity_verify)" \
  --argjson error_contract_verify "$(get_check required.error_contract_verify)" \
  --argjson deployed_chain_websocket_upgrade "$(get_check required.deployed_chain_websocket_upgrade)" \
  --argjson cardinality_verify "$(get_check required.cardinality_verify)" \
  --argjson integrity_records_verify "$(get_check required.integrity_records_verify)" \
  --argjson authz_baseline_verify "$(get_check required.authz_baseline_verify)" \
  --argjson limits_verify "$(get_check required.limits_verify)" \
  --argjson events_verify "$(get_check required.events_verify)" \
  --argjson migration_verify "$(get_check required.migration_verify)" \
  --argjson document_seq_verify "$(get_check required.document_seq_verify)" \
  --argjson mcp_transport_verify "$(get_check required.mcp_transport_verify)" \
  --argjson feature_flag_verify "$(get_check required.feature_flag_verify)" \
  --argjson forms_regression_verify "$(get_check required.forms_regression_verify)" \
  --argjson tool_registry_verify "$(get_check required.tool_registry_verify)" \
  --argjson cli_contract_verify "$(get_check required.cli_contract_verify)" \
  --argjson transport_auth_verify "$(get_check required.transport_auth_verify)" \
  --argjson cross_workspace_verify "$(get_check required.cross_workspace_verify)" \
  --arg report_status "$REPORT_COMMAND_STATUS" \
  --argjson report_exit "$REPORT_COMMAND_EXIT" \
  --arg zero_sha "$ZERO_SHA" \
  '{
    surface_parity:$surface_parity,
    legacy_pages_inventory:$legacy_pages_inventory,
    legacy_pages_entry_verify:$legacy_pages_entry_verify,
    collab_architecture_verify:$collab_architecture_verify,
    rest_contract_verify:$rest_contract_verify,
    document_integrity_verify:$document_integrity_verify,
    error_contract_verify:$error_contract_verify,
    deployed_chain_websocket_upgrade:$deployed_chain_websocket_upgrade,
    cardinality_verify:$cardinality_verify,
    integrity_records_verify:$integrity_records_verify,
    authz_baseline_verify:$authz_baseline_verify,
    limits_verify:$limits_verify,
    events_verify:$events_verify,
    migration_verify:$migration_verify,
    document_seq_verify:$document_seq_verify,
    mcp_transport_verify:$mcp_transport_verify,
    feature_flag_verify:$feature_flag_verify,
    forms_regression_verify:$forms_regression_verify,
    tool_registry_verify:$tool_registry_verify,
    cli_contract_verify:$cli_contract_verify,
    transport_auth_verify:$transport_auth_verify,
    cross_workspace_verify:$cross_workspace_verify,
    report:{command:"scripts/report-flow-v0.4-json.sh", status:$report_status, exit_code:$report_exit, duration_ms:0, executed_count:1, evidence:"evidence/v0.4/gate-result.json", sha256:$zero_sha},
    verify:{command:"scripts/verify-flow-v0.4-json.sh evidence/v0.4/gate-result.json --json", status:"not_run", exit_code:null, duration_ms:0, executed_count:0, evidence:"evidence/v0.4/gate-result.json", sha256:$zero_sha},
    gate:{command:"scripts/gate-flow-v0.4.sh --json", status:"not_run", exit_code:null, duration_ms:0, executed_count:0, evidence:"evidence/v0.4/gate-result.json", sha256:$zero_sha},
    manual_signoff:{command:"scripts/record-flow-v0.4-manual-signoff.sh", status:"not_run", exit_code:null, duration_ms:0, executed_count:0, evidence:"evidence/v0.4/gate-result.json", sha256:$zero_sha}
  }')"

if ! jq -e --argjson expected "$SCHEMA_REQUIRED_COMMAND_KEYS" 'keys == $expected' >/dev/null <<<"$REQUIRED_COMMANDS_JSON"; then
  echo "FAIL: report required_commands keys do not equal the aligned YAML/schema command ledger" >&2
  exit 2
fi

GATE_RESULT_PATH="$EVIDENCE_ROOT/gate-result.json"
GATE_RESULT_TMP="$GATE_RESULT_PATH.tmp"
jq -n \
  --arg head "$SOURCE_HEAD" --argjson dirty "$SOURCE_DIRTY" \
  --arg repository "$REPO_ROOT" --arg rust_version "$RUST_WORKSPACE_VERSION" \
  --arg frontend_version "$FRONTEND_PACKAGE_VERSION" \
  --arg generated_at "$GENERATED_AT" \
  --argjson checks "$CHECKS_JSON" \
  --argjson artifacts "$ARTIFACTS_JSON" \
  --argjson required_commands "$REQUIRED_COMMANDS_JSON" \
  --argjson hard_gates "$HARD_GATES_JSON" \
  --argjson hard_gate_non_pass_count "$HARD_GATE_NON_PASS_COUNT" \
  '{
    schema_version: "sylvode.flow.gate-result.v1",
    schema_path: "docs/schemas/sylvode-flow-gate-v0.4.schema.json",
    release: "0.4.0",
    source_baseline: {
      repository: $repository,
      rust_workspace_version: $rust_version,
      frontend_package_version: $frontend_version,
      reviewed_head: $head
    },
    source: {repository: $repository, head: $head, dirty: $dirty},
    generated_at: $generated_at,
    mode: "blocked",
    gate_passed: false,
    counts: {
      automated: 0,
      passed: 0,
      failed: 0,
      environment_unavailable: 0,
      manual_pending: 5,
      unresolved: 5
    },
    checks: $checks,
    required_commands: $required_commands,
    hard_gates: $hard_gates,
    artifacts: $artifacts,
    manual_signoffs: {
      page_editor: {status:"deferred_to_frontend_track", reviewer:"", evidence:"ADR-0017: gates/vF-frontend-gate.yaml#page_editor"},
      navigator_a11y: {status:"deferred_to_frontend_track", reviewer:"", evidence:"ADR-0017: gates/vF-frontend-gate.yaml#navigator_a11y"},
      restart_recovery: {status:"pending", reviewer:"", evidence:""},
      feature_flag: {status:"pending", reviewer:"", evidence:""},
      forms_regression: {status:"pending", reviewer:"", evidence:""}
    },
    blockers: []
  }' | jq -f "$RECEIPT_STATE_FILTER" > "$GATE_RESULT_TMP"

sync "$GATE_RESULT_TMP" 2>/dev/null || true
mv -f "$GATE_RESULT_TMP" "$GATE_RESULT_PATH"
echo "REPORT: wrote $GATE_RESULT_PATH" >&2
exit "$REPORT_COMMAND_EXIT"
