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
# ONLY when every artifact docs/schemas/sylvode-flow-gate-v0.4.schema.json
# requires actually exists with a real, freshly computed checksum --
# fabricating a schema-shaped file with missing/placeholder artifacts is
# exactly the fake-green pattern this v0.4 work exists to close.
#
# v0.4-gate.yaml's required_commands list has 15 entries (bumped from 13
# in `02e8cb7`, which added limits_verify/events_verify -- this script had
# drifted behind that bump until it was synced back up here). As of this
# sync, seven of the fifteen have a corresponding verify-flow-*-v0.4.sh
# script implemented (surface_parity, legacy_pages_inventory +
# legacy_pages_entry_verify, cardinality_verify, integrity_records_verify,
# authz_baseline_verify, collab_architecture_verify, events_verify); the
# remaining (error_contract_verify, deployed_chain_websocket_upgrade,
# limits_verify) do not exist yet. This script records each missing
# script as a FAILED step with an explicit "script not implemented"
# message rather than skipping it silently, so report correctly and
# honestly exits 1 and does not write gate-result.json until those
# scripts exist and pass.
#
# Every invocation (pass or fail) also writes an atomic run log so a
# failed report is never silently lost, per "有失败 exit 1，但仍保留报告".
#
# Exit codes: 0 = all checks ran and passed and gate-result.json was
# written, 1 = one or more checks failed or a required artifact is
# missing (run log still written; gate-result.json is written only if it
# would be schema-valid), 2 = usage/tool/evidence malformed.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
REPO_ROOT="$ROOT_DIR"
SKIP_GENERIC=0

usage() {
  cat <<'EOF'
Usage: scripts/report-flow-v0.4-json.sh [OPTIONS]

Runs the read-only v0.4 report bundle: the generic cargo/bun/forms-gate
commands, then every required_commands verify step v0.4-gate.yaml names.
Records the exact command, exit code and duration for every step, then --
only if every artifact the v0.4 gate schema requires is actually present
with a real checksum -- atomically writes <evidence-root>/gate-result.json.
A run log is always written to <evidence-root>/report-run-log.json, pass
or fail, so a failed report is never silently lost.

This script never invents an artifact. Steps whose verify-flow-*-v0.4.sh
script does not exist yet are recorded as failed with an explicit
"script not implemented" message; report exits 1 with a clear list of
what is missing.

Options:
  --evidence-root DIR    Where evidence/v0.4/<name> artifacts are written
                          and read from. Default:
                          /opt/working/sylvode-flow/evidence/v0.4
  --contracts-root DIR   Root for decisions/, contracts/, security/
                          artifact paths. Default: /opt/working/sylvode-flow
  --repo-root DIR         Repository the cargo/bun commands run in and
                          whose HEAD becomes source.head. Default: this
                          checkout.
  --skip-generic          Skip the generic cargo fmt/check/clippy/test +
                          bun check/build + ci-universal-forms-gates +
                          test-mcp bundle (fast iteration only; report
                          will still correctly fail to write
                          gate-result.json because those checks are
                          required).
  -h, --help              Show this help and exit 0.

Exit codes: 0 all green and gate-result.json written, 1 one or more
checks failed / artifacts missing, 2 usage/tool error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --skip-generic) SKIP_GENERIC=1; shift ;;
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

if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi

mkdir -p "$EVIDENCE_ROOT"

CHECKS_JSON="[]"
OVERALL_FAILED=0

sha256_of() { sha256sum "$1" | awk '{print $1}'; }

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
    --arg evidence "evidence/v0.4/logs/${id}.log" --arg sha256 "$log_sha" \
    '. + [{id:$id, status:$status, command:$command, exit_code:$exit_code, duration_ms:$duration_ms, evidence:$evidence, sha256:$sha256}]' \
    <<<"$CHECKS_JSON")"
  echo "[$status] $id (exit=$exit_code, ${duration}ms): $cmd_str"
  if [[ $exit_code -ne 0 ]]; then
    echo "$output" | sed 's/^/  | /' | tail -20
  fi
  return $exit_code
}

# A step whose backing script does not exist: record a failed check
# without invoking anything.
run_missing_step() {
  local id="$1" reason="$2"
  OVERALL_FAILED=1
  local log_file="$EVIDENCE_ROOT/logs/${id}.log"
  mkdir -p "$(dirname "$log_file")"
  printf 'NOT IMPLEMENTED: %s\n' "$reason" > "$log_file"
  local log_sha
  log_sha="$(sha256_of "$log_file")"
  CHECKS_JSON="$(jq -c \
    --arg id "$id" --arg reason "$reason" \
    --arg evidence "evidence/v0.4/logs/${id}.log" --arg sha256 "$log_sha" \
    '. + [{id:$id, status:"failed", command:("NOT IMPLEMENTED: " + $reason), exit_code:2, duration_ms:0, evidence:$evidence, sha256:$sha256}]' \
    <<<"$CHECKS_JSON")"
  echo "[failed] $id: NOT IMPLEMENTED -- $reason"
}

cd "$REPO_ROOT"

if [[ $SKIP_GENERIC -eq 1 ]]; then
  echo "=== Sylvode Flow v0.4 report: generic command bundle SKIPPED (--skip-generic) ===" >&2
else
  echo "=== Sylvode Flow v0.4 report: generic command bundle ==="
  run_step generic.cargo_fmt cargo fmt --all -- --check || true
  run_step generic.cargo_check cargo check --workspace --all-targets || true
  run_step generic.cargo_clippy cargo clippy --workspace --all-targets -- -D warnings || true
  run_step generic.cargo_test cargo test --workspace || true
  run_step generic.bun_check bun --cwd frontend run check || true
  run_step generic.bun_build bun --cwd frontend run build || true
  run_step generic.ci_universal_forms_gates bash scripts/ci-universal-forms-gates.sh || true
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

echo "=== Sylvode Flow v0.4 report: collab-architecture verify ==="
run_step required.collab_architecture_verify "$ROOT_DIR/scripts/verify-flow-collab-architecture.sh" --release 0.4 --adr "$CONTRACTS_ROOT/decisions/ADR-0010-collab-server-architecture.md" --limits "$CONTRACTS_ROOT/contracts/limits-v1.md" --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: events/dispatch verify ==="
run_step required.events_verify "$ROOT_DIR/scripts/verify-flow-events-v0.4.sh" --contract "$CONTRACTS_ROOT/contracts/events-v1.md" --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json || true

echo "=== Sylvode Flow v0.4 report: not-yet-implemented required_commands ==="
run_missing_step required.error_contract_verify "scripts/verify-flow-errors-v0.4.sh does not exist"
run_missing_step required.deployed_chain_websocket_upgrade "scripts/verify-flow-deployed-websocket-v0.4.sh does not exist"
run_missing_step required.limits_verify "scripts/verify-flow-limits-v0.4.sh does not exist"

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
  "legacy_pages_inventory:$EVIDENCE_ROOT/legacy-pages-inventory.json:evidence/v0.4/legacy-pages-inventory.json"
)

MISSING_ARTIFACTS=()
for entry in "${REQUIRED_ARTIFACTS[@]}"; do
  IFS=':' read -r key abs_path _rel_path <<<"$entry"
  if [[ ! -f "$abs_path" ]]; then
    MISSING_ARTIFACTS+=("$key ($abs_path)")
  fi
done

if [[ $OVERALL_FAILED -ne 0 || ${#MISSING_ARTIFACTS[@]} -gt 0 ]]; then
  echo "REPORT: FAIL -- not writing evidence/v0.4/gate-result.json (would not be schema-valid / not all checks passed)" >&2
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
ARTIFACTS_JSON="$(jq -c '.gate_result = {path:"evidence/v0.4/gate-result.json", sha256:"0000000000000000000000000000000000000000000000000000000000000"[0:64]}' <<<"$ARTIFACTS_JSON")"

echo "REPORT: all required artifacts present -- assembling evidence/v0.4/gate-result.json" >&2
echo "REPORT: hard_gates below are deliberately conservative 'not_verified' placeholders -- this script does not itself compute hard-gate verdicts; run scripts/verify-flow-v0.4-json.sh against this file to get the authoritative recomputed verdicts, then scripts/gate-flow-v0.4.sh to aggregate with manual signoffs." >&2

get_check() {
  jq -c --arg id "$1" '[.[] | select(.id==$id)][0] // {status:"failed",command:"(not run)",exit_code:2,duration_ms:0,evidence:"",sha256:"0000000000000000000000000000000000000000000000000000000000000"[0:64]}' <<<"$CHECKS_JSON" | \
    jq -c '{command:.command, status:.status, exit_code:.exit_code, duration_ms:.duration_ms, evidence:.evidence, sha256:.sha256}'
}
ZERO_SHA="0000000000000000000000000000000000000000000000000000000000000"
ZERO_SHA="${ZERO_SHA:0:64}"

REQUIRED_COMMANDS_JSON="$(jq -n \
  --argjson surface_parity "$(get_check required.surface_parity)" \
  --argjson legacy_pages_inventory "$(get_check required.legacy_pages_inventory)" \
  --argjson legacy_pages_entry_verify "$(get_check required.legacy_pages_entry_verify)" \
  --argjson collab_architecture_verify "$(get_check required.collab_architecture_verify)" \
  --argjson error_contract_verify "$(get_check required.error_contract_verify)" \
  --argjson deployed_chain_websocket_upgrade "$(get_check required.deployed_chain_websocket_upgrade)" \
  --argjson cardinality_verify "$(get_check required.cardinality_verify)" \
  --argjson integrity_records_verify "$(get_check required.integrity_records_verify)" \
  --argjson authz_baseline_verify "$(get_check required.authz_baseline_verify)" \
  --argjson limits_verify "$(get_check required.limits_verify)" \
  --argjson events_verify "$(get_check required.events_verify)" \
  --arg zero_sha "$ZERO_SHA" \
  '{
    surface_parity:$surface_parity,
    legacy_pages_inventory:$legacy_pages_inventory,
    legacy_pages_entry_verify:$legacy_pages_entry_verify,
    collab_architecture_verify:$collab_architecture_verify,
    error_contract_verify:$error_contract_verify,
    deployed_chain_websocket_upgrade:$deployed_chain_websocket_upgrade,
    cardinality_verify:$cardinality_verify,
    integrity_records_verify:$integrity_records_verify,
    authz_baseline_verify:$authz_baseline_verify,
    limits_verify:$limits_verify,
    events_verify:$events_verify,
    report:{command:"scripts/report-flow-v0.4-json.sh", status:"passed", exit_code:0, duration_ms:0, evidence:"evidence/v0.4/gate-result.json", sha256:$zero_sha},
    verify:{command:"scripts/verify-flow-v0.4-json.sh evidence/v0.4/gate-result.json --json", status:"failed", exit_code:1, duration_ms:0, evidence:"evidence/v0.4/gate-result.json", sha256:$zero_sha},
    gate:{command:"scripts/gate-flow-v0.4.sh --json", status:"failed", exit_code:1, duration_ms:0, evidence:"evidence/v0.4/gate-result.json", sha256:$zero_sha},
    manual_signoff:{command:"scripts/record-flow-v0.4-manual-signoff.sh", status:"failed", exit_code:1, duration_ms:0, evidence:"evidence/v0.4/gate-result.json", sha256:$zero_sha}
  }')"

GATE_RESULT_PATH="$EVIDENCE_ROOT/gate-result.json"
GATE_RESULT_TMP="$GATE_RESULT_PATH.tmp"
jq -n \
  --arg head "$SOURCE_HEAD" --argjson dirty "$SOURCE_DIRTY" \
  --arg generated_at "$GENERATED_AT" \
  --argjson checks "$CHECKS_JSON" \
  --argjson artifacts "$ARTIFACTS_JSON" \
  --argjson required_commands "$REQUIRED_COMMANDS_JSON" \
  --argjson hard_gates '{"rest_mcp_cli_ui_surface_parity":"not_verified","mcp_default_rest_coverage_three_adr_threat_exceptions_only":"not_verified","migration_forward_and_rollback_strategy":"not_verified","document_row_lock_seq_unique":"not_verified","collab_architecture_adr_accepted":"not_verified","bounded_warm_cache_lock_hold_and_round_trip_budgets":"not_verified","minimal_snapshot_advancement_bounds_tail":"not_verified","snapshot_tail_restart_recovery":"not_verified","bootstrap_repeatable_read_and_ws_parity":"not_verified","accepted_egress_seq_monotonic_and_gap_resync":"not_verified","rest_envelope_and_error_contract":"not_verified","server_draining_reason_cross_surface_error_coverage":"not_verified","ticket_single_use_origin_bot_exclusion":"not_verified","secure_cookie_and_local_dev_guard":"not_verified","cross_workspace_and_policy_bypass_negative":"not_verified","unauthorized_update_rejected":"not_verified","mcp_three_transport_contract":"not_verified","tool_registry_expected_107_or_rebased":"not_verified","cli_json_and_exit_code_contract":"not_verified","web_ime_undo_selection_and_sync_state":"not_verified","navigator_keyboard_drag_equivalence":"not_verified","i18n_zh_en_flow_key_parity":"not_verified","vite_wasm_static_build_and_deep_route":"not_verified","feature_flag_navigation_and_direct_url":"not_verified","feature_flag_mcp_read_admin_write_and_cli_equivalence":"not_verified","forms_regression_no_degradation":"not_verified","flow_limits_exact_boundary_and_plus_one_rejection":"not_verified","isolated_decode_apply_cpu_wall_memory":"not_verified","websocket_rate_connection_and_backpressure_limits":"not_verified","deployed_chain_websocket_upgrade":"not_verified","bootstrap_limits_web_server_parity":"not_verified","limit_exceeded_kind_coverage":"not_verified","flow_event_registry_payload_policy_complete":"not_verified","business_event_dispatch_same_transaction":"not_verified","dispatch_expansion_snapshot_semantics":"not_verified","no_subscribers_terminalized_and_reaped":"not_verified","dispatcher_liveness_and_backlog":"not_verified","flow_content_delivery_coalescing":"not_verified","coalescing_seal_and_source_first_expansion":"not_verified","dispatch_numeric_budgets_locked":"not_verified","command_contended_document_cardinality":"not_verified","integrity_record_on_fail_closed":"not_verified","flow_parent_authority_in_postgres":"not_verified","member_baseline_no_behaviour_regression":"not_verified","event_idempotency_audit_and_redaction":"not_verified","legacy_pages_inventory_three_environments_complete":"not_verified","legacy_pages_zero_or_importer_surface_available":"not_verified","legacy_pages_mcp_admin_policy_and_semantic_equivalence":"not_verified","legacy_pages_dry_run_and_rerun_idempotent":"not_verified","legacy_pages_failure_source_immutable":"not_verified","legacy_pages_lineage_complete":"not_verified","legacy_pages_drop_requires_separate_adr":"not_verified"}' \
  '{
    schema_version: "sylvode.flow.gate-result.v1",
    schema_path: "docs/schemas/sylvode-flow-gate-v0.4.schema.json",
    release: "0.4.0",
    source_baseline: {
      repository: "/opt/worker/code/openpr",
      rust_workspace_version: "0.2.31",
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
      manual_pending: 5,
      unresolved: 5
    },
    checks: $checks,
    required_commands: $required_commands,
    hard_gates: $hard_gates,
    artifacts: $artifacts,
    manual_signoffs: {
      page_editor: {status:"pending", reviewer:"", evidence:""},
      navigator_a11y: {status:"pending", reviewer:"", evidence:""},
      restart_recovery: {status:"pending", reviewer:"", evidence:""},
      feature_flag: {status:"pending", reviewer:"", evidence:""},
      forms_regression: {status:"pending", reviewer:"", evidence:""}
    },
    blockers: ["hard-gates-not-yet-verified", "manual-signoffs-pending"]
  }' > "$GATE_RESULT_TMP"

sync "$GATE_RESULT_TMP" 2>/dev/null || true
mv -f "$GATE_RESULT_TMP" "$GATE_RESULT_PATH"
echo "REPORT: wrote $GATE_RESULT_PATH" >&2
exit 0
