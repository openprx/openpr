#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.5 report/receipt generator.
#
# Default mode runs every v0.5 producer command that the gate contract names
# and has an executable repository producer, records exact command exits/logs,
# then aggregates the resulting JSON evidence.  --collect-only is the safe
# read-existing-evidence mode: no producer is run and every such command stays
# visibly not_run, which can never satisfy the gate.
#
# Unlike the old structural failure where a missing producer prevented any
# gate-result.json from existing, this report always writes an honest blocked
# receipt for semantic non-pass states. Malformed JSON/tool/usage remains exit
# 2. The shared jq library owns the 31 gate wiring and all derived receipt
# state.
#
# Exit codes: 0 = automated candidate rule satisfied (manual rows may remain
# pending); 1 = evidence/producer/predecessor/budget/source non-pass; 2 =
# usage/tool/malformed evidence.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT=""
REPO_ROOT="$ROOT_DIR"
COLLECT_ONLY=0
GATE_YAML=""
STATE_LIBRARY="$ROOT_DIR/scripts/lib/flow_gate_v0_5_receipt_state.jq"

usage() {
  cat <<'EOF'
Usage: scripts/report-flow-v0.5-json.sh [OPTIONS]

Runs available v0.5 evidence producers and atomically writes gate-result.json.
Missing producers/artifacts, failed artifacts, dirty/unproven source identity,
an unaccepted v0.4 predecessor, and unfrozen named budgets remain distinct
blocking states; none is converted to passed.

Options:
  --evidence-root DIR   Evidence directory. Default:
                        <contracts-root>/evidence/v0.5
  --contracts-root DIR Contract repository. Default:
                        /opt/working/sylvode-flow
  --repo-root DIR       Source repository. Default: this checkout.
  --gate-yaml PATH      v0.5 gate YAML. Default:
                        <contracts-root>/gates/v0.5-gate.yaml
  --collect-only        Do not run producers; aggregate existing JSON only.
                        Producer checks are recorded not_run and block.
  --json                Emit the final summary as JSON (accepted for symmetry;
                        output is JSON even when omitted).
  -h, --help            Show help and exit 0.

Exit codes: 0 candidate automation green, 1 blocked/non-pass, 2 malformed.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires DIR}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires DIR}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires DIR}"; shift 2 ;;
    --gate-yaml) GATE_YAML="${2:?--gate-yaml requires PATH}"; shift 2 ;;
    --collect-only) COLLECT_ONLY=1; shift ;;
    --json) shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

for tool in jq sha256sum git awk sed xargs; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    exit 2
  fi
done
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi
REPO_ROOT="$(cd "$REPO_ROOT" && pwd)"
[[ -n "$EVIDENCE_ROOT" ]] || EVIDENCE_ROOT="$CONTRACTS_ROOT/evidence/v0.5"
[[ -n "$GATE_YAML" ]] || GATE_YAML="$CONTRACTS_ROOT/gates/v0.5-gate.yaml"
if [[ ! -f "$GATE_YAML" ]]; then
  echo "FAIL: gate YAML not found: $GATE_YAML" >&2
  exit 2
fi
if [[ ! -f "$STATE_LIBRARY" ]]; then
  echo "FAIL: shared receipt-state library not found: $STATE_LIBRARY" >&2
  exit 2
fi
if ! jq -n -L "$ROOT_DIR/scripts/lib" 'include "flow_gate_v0_5_receipt_state"; flow_gate_wiring | length == 31' >/dev/null; then
  echo "FAIL: shared v0.5 gate wiring is malformed or not exactly 31 entries" >&2
  exit 2
fi
mkdir -p "$EVIDENCE_ROOT/logs"

sha256_of() { sha256sum "$1" | awk '{print $1}'; }

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

ARTIFACT_PATHS="$(yaml_map_json artifacts)"
REQUIRED_COMMAND_STRINGS="$(yaml_map_json required_commands)"
YAML_HARD_GATES="$(yaml_map_json hard_gates | jq 'with_entries(.value="pending")')"
PREDECESSOR_REQUIREMENT="$(yaml_map_json required_predecessor)"
SOURCE_BASELINE="$(yaml_map_json source_baseline)"

if [[ "$(jq 'length' <<<"$YAML_HARD_GATES")" -ne 31 ]]; then
  echo "FAIL: $GATE_YAML hard_gates count is not 31" >&2
  exit 2
fi
WIRING_KEYS="$(jq -n -L "$ROOT_DIR/scripts/lib" 'include "flow_gate_v0_5_receipt_state"; flow_gate_wiring | keys')"
if ! jq -e --argjson wiring "$WIRING_KEYS" 'keys == $wiring' >/dev/null <<<"$YAML_HARD_GATES"; then
  echo "FAIL: shared gate wiring keys do not exactly match v0.5-gate.yaml hard_gates" >&2
  exit 2
fi

SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
SOURCE_DIRTY_SCOPE='["apps/","crates/","spikes/","migrations/",".cargo/","Cargo.toml","Cargo.lock"]'
SOURCE_DIRTY_STATUS="$(git -C "$REPO_ROOT" status --porcelain=v1 --untracked-files=all -- \
  apps crates spikes migrations .cargo Cargo.toml Cargo.lock)"
SOURCE_DIRTY_ENTRIES="$(printf '%s\n' "$SOURCE_DIRTY_STATUS" | jq -R 'select(length>0)' | jq -s '.')"
SOURCE_DIRTY=$([[ -n "$SOURCE_DIRTY_STATUS" ]] && echo true || echo false)
RUST_WORKSPACE_VERSION="$(sed -n '/^\[workspace.package\]$/,/^\[/s/^version = "\([^"]*\)"/\1/p' "$REPO_ROOT/Cargo.toml" | head -1)"
FRONTEND_PACKAGE_VERSION="$(jq -r '.version // empty' "$REPO_ROOT/frontend/package.json")"
if [[ -z "$RUST_WORKSPACE_VERSION" || -z "$FRONTEND_PACKAGE_VERSION" ]]; then
  echo "FAIL: could not derive source package versions" >&2
  exit 2
fi

CHECKS_JSON='[]'

record_check() {
  local id="$1" status="$2" exit_json="$3" command="$4" output="$5" duration_json="${6:-null}" executed_count="${7:-0}"
  local log_file="$EVIDENCE_ROOT/logs/${id}.log" log_sha
  printf '%s\n' "$output" > "$log_file"
  log_sha="$(sha256_of "$log_file")"
  CHECKS_JSON="$(jq -c \
    --arg id "$id" --arg status "$status" --arg command "$command" \
    --argjson exit_code "$exit_json" --argjson duration_ms "$duration_json" \
    --argjson executed_count "$executed_count" \
    --arg evidence "evidence/v0.5/logs/${id}.log" --arg sha256 "$log_sha" \
    '. + [{id:$id,status:$status,command:$command,exit_code:$exit_code,duration_ms:$duration_ms,executed_count:$executed_count,evidence:$evidence,sha256:$sha256}]' \
    <<<"$CHECKS_JSON")"
}

run_command() {
  local id="$1" command_display="$2"; shift 2
  local output exit_code status started ended duration_ms
  started="$(date +%s%N)"
  set +e
  output="$("$@" 2>&1)"
  exit_code=$?
  ended="$(date +%s%N)"
  set -e
  status=$([[ $exit_code -eq 0 ]] && echo passed || echo failed)
  duration_ms="$(((ended - started) / 1000000))"
  record_check "$id" "$status" "$exit_code" "$command_display" "$output" "$duration_ms" 1
}

record_or_run_required_producer() {
  local key="$1" command script_rel
  command="$(jq -r --arg k "$key" '.[$k] // empty' <<<"$REQUIRED_COMMAND_STRINGS")"
  script_rel="${command%% *}"
  if [[ -z "$command" || ! -f "$REPO_ROOT/$script_rel" ]]; then
    record_check "required.$key" producer_missing null "$command" \
      "producer missing: ${script_rel:-contract command is empty}"
    return
  fi
  if [[ $COLLECT_ONLY -eq 1 ]]; then
    record_check "required.$key" not_run null "$command" \
      "not run: --collect-only; existing evidence is inspected without executing producers"
    return
  fi
  case "$key" in
    surface_parity)
      run_command "required.$key" "$command" \
        "$REPO_ROOT/$script_rel" --release 0.5 --contracts-root "$CONTRACTS_ROOT" \
        --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json
      ;;
    collab_architecture_verify)
      # Keep the frozen --clients 10 token: the current producer does not
      # accept it, so this honestly records the contract/producer mismatch.
      run_command "required.$key" "$command" \
        "$REPO_ROOT/$script_rel" --release 0.5 --clients 10 \
        --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" \
        --repo-root "$REPO_ROOT" --json
      ;;
    authz_verify)
      run_command "required.$key" "$command" \
        "$REPO_ROOT/$script_rel" --adr "$CONTRACTS_ROOT/decisions/ADR-0012-object-authorization-and-sharing.md" \
        --limits "$CONTRACTS_ROOT/contracts/limits-v1.md" --contracts-root "$CONTRACTS_ROOT" \
        --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json
      ;;
    multi_document_verify)
      run_command "required.$key" "$command" \
        "$REPO_ROOT/$script_rel" --adr "$CONTRACTS_ROOT/decisions/ADR-0013-multi-document-atomicity.md" \
        --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" \
        --repo-root "$REPO_ROOT" --json
      ;;
    cardinality_verify)
      run_command "required.$key" "$command" \
        "$REPO_ROOT/$script_rel" --adr "$CONTRACTS_ROOT/decisions/ADR-0013-multi-document-atomicity.md" \
        --since-release 0.4 --contracts-root "$CONTRACTS_ROOT" \
        --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json
      ;;
    invalid_update_reason_verify)
      run_command "required.$key" "$command" \
        "$REPO_ROOT/$script_rel" --contract "$CONTRACTS_ROOT/contracts/error-mapping-v1.md" \
        --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" \
        --repo-root "$REPO_ROOT" --json
      ;;
    audit_causation_verify)
      run_command "required.$key" "$command" \
        "$REPO_ROOT/$script_rel" --contract "$CONTRACTS_ROOT/contracts/events-v1.md" \
        --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" \
        --repo-root "$REPO_ROOT" --json
      ;;
    relation_policy_verify)
      run_command "required.$key" "$command" \
        "$REPO_ROOT/$script_rel" --contract "$CONTRACTS_ROOT/contracts/rest-api-v1.md" \
        --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" \
        --repo-root "$REPO_ROOT" --json
      ;;
    search_contract_verify)
      run_command "required.$key" "$command" \
        "$REPO_ROOT/$script_rel" --adr "$CONTRACTS_ROOT/decisions/ADR-0009-flow-search-surface.md" \
        --contract "$CONTRACTS_ROOT/contracts/rest-api-v1.md" \
        --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" \
        --repo-root "$REPO_ROOT" --json
      ;;
    mcp_cli_equivalence_verify)
      run_command "required.$key" "$command" \
        "$REPO_ROOT/$script_rel" --mcp-contract "$CONTRACTS_ROOT/contracts/mcp-surface-v1.md" \
        --surface-contract "$CONTRACTS_ROOT/contracts/surface-coverage-v1.md" \
        --adr "$CONTRACTS_ROOT/decisions/ADR-0009-flow-search-surface.md" \
        --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" \
        --repo-root "$REPO_ROOT" --json
      ;;
    *)
      record_check "required.$key" producer_unspecified null "$command" \
        "no WP-29 invocation mapping exists for required producer: $key"
      ;;
  esac
}

for producer_key in surface_parity collab_architecture_verify authz_verify \
  multi_document_verify cardinality_verify invalid_update_reason_verify relation_policy_verify \
  search_contract_verify mcp_cli_equivalence_verify audit_causation_verify; do
  record_or_run_required_producer "$producer_key"
done

CONVERGENCE_COMMAND='scripts/verify-flow-convergence-v0.5.sh --clients 10 --json'
if [[ ! -f "$REPO_ROOT/scripts/verify-flow-convergence-v0.5.sh" ]]; then
  record_check extra.convergence_verify producer_missing null "$CONVERGENCE_COMMAND" \
    "producer missing: scripts/verify-flow-convergence-v0.5.sh"
elif [[ $COLLECT_ONLY -eq 1 ]]; then
  record_check extra.convergence_verify not_run null "$CONVERGENCE_COMMAND" \
    "not run: --collect-only; existing evidence is inspected without executing producers"
else
  run_command extra.convergence_verify "$CONVERGENCE_COMMAND" \
    "$REPO_ROOT/scripts/verify-flow-convergence-v0.5.sh" --clients 10 \
    --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json
fi

check_for_required_key() {
  local key="$1"
  jq -c --arg id "required.$key" '[.[] | select(.id==$id)][0] // null' <<<"$CHECKS_JSON"
}

producer_metadata() {
  local artifact="$1" command_key="" command="" script_rel="" status="producer_unspecified"
  case "$artifact" in
    surface_coverage_result) command_key=surface_parity ;;
    collab_architecture_result) command_key=collab_architecture_verify ;;
    authz_result) command_key=authz_verify ;;
    multi_document_result) command_key=multi_document_verify ;;
    cardinality_result) command_key=cardinality_verify ;;
    invalid_update_reason_result) command_key=invalid_update_reason_verify ;;
    relation_policy_result) command_key=relation_policy_verify ;;
    search_contract_result) command_key=search_contract_verify ;;
    mcp_cli_equivalence_result) command_key=mcp_cli_equivalence_verify ;;
    audit_causation_result) command_key=audit_causation_verify ;;
    convergence_result)
      command="$CONVERGENCE_COMMAND"; script_rel=scripts/verify-flow-convergence-v0.5.sh
      ;;
  esac
  if [[ -n "$command_key" ]]; then
    command="$(jq -r --arg k "$command_key" '.[$k] // empty' <<<"$REQUIRED_COMMAND_STRINGS")"
    script_rel="${command%% *}"
  fi
  if [[ -n "$command" ]]; then
    if [[ -f "$REPO_ROOT/$script_rel" ]]; then status=available
    else status=producer_missing
    fi
  fi
  local check='null' executed_count=0 execution_status=null
  if [[ -n "$command_key" ]]; then
    check="$(check_for_required_key "$command_key")"
  elif [[ "$artifact" == convergence_result ]]; then
    check="$(jq -c '[.[] | select(.id=="extra.convergence_verify")][0] // null' <<<"$CHECKS_JSON")"
  fi
  if [[ "$check" != null ]]; then
    executed_count="$(jq -r '.executed_count // 0' <<<"$check")"
    execution_status="$(jq -r '.status // "not_run" | @json' <<<"$check")"
  fi
  jq -cn --arg status "$status" --arg command "$command" \
    --argjson executed_count "$executed_count" --argjson execution_status "$execution_status" \
    '{producer_status:$status,producer_command:(if $command=="" then null else $command end),
      producer_executed_count:$executed_count,producer_execution_status:$execution_status}'
}

ARTIFACT_INPUTS='{}'
ARTIFACTS_JSON='{}'
MALFORMED_ARTIFACTS=0
while IFS= read -r artifact; do
  [[ "$artifact" == gate_result ]] && continue
  canonical_path="$(jq -r --arg k "$artifact" '.[$k]' <<<"$ARTIFACT_PATHS")"
  artifact_path="$EVIDENCE_ROOT/$(basename "$canonical_path")"
  metadata="$(producer_metadata "$artifact")"
  exists=false valid_json=false sha=null document='{}'
  if [[ -f "$artifact_path" ]]; then
    exists=true
    if jq empty "$artifact_path" >/dev/null 2>&1 && [[ "$(jq -r type "$artifact_path")" == object ]]; then
      valid_json=true
      sha="\"$(sha256_of "$artifact_path")\""
      document="$(jq -c . "$artifact_path")"
    else
      MALFORMED_ARTIFACTS=$((MALFORMED_ARTIFACTS + 1))
    fi
  fi
  input="$(jq -cn --arg path "$canonical_path" --argjson exists "$exists" \
    --argjson valid_json "$valid_json" --argjson sha256 "$sha" \
    --argjson document "$document" --argjson metadata "$metadata" \
    '$metadata + {path:$path,exists:$exists,valid_json:$valid_json,sha256:$sha256,document:$document}')"
  ARTIFACT_INPUTS="$(jq -c --arg k "$artifact" --argjson v "$input" '.[$k]=$v' <<<"$ARTIFACT_INPUTS")"
done < <(jq -r 'keys[]' <<<"$ARTIFACT_PATHS")

ARTIFACT_STATES="$(jq -cn -L "$ROOT_DIR/scripts/lib" --argjson inputs "$ARTIFACT_INPUTS" \
  --arg head "$SOURCE_HEAD" 'include "flow_gate_v0_5_receipt_state"; flow_compute_artifact_states($inputs;$head)')"
HARD_GATES="$(jq -cn -L "$ROOT_DIR/scripts/lib" --argjson states "$ARTIFACT_STATES" \
  'include "flow_gate_v0_5_receipt_state"; flow_compute_hard_gates($states)')"
if ! jq -e --argjson expected "$(jq 'keys' <<<"$YAML_HARD_GATES")" \
  'length==31 and keys==$expected' >/dev/null <<<"$HARD_GATES"; then
  echo "FAIL: computed hard gates do not exactly match the 31 YAML keys" >&2
  exit 2
fi

while IFS= read -r artifact; do
  canonical_path="$(jq -r --arg k "$artifact" '.[$k]' <<<"$ARTIFACT_PATHS")"
  if [[ "$artifact" == gate_result ]]; then
    entry="$(jq -cn --arg path "$canonical_path" '{path:$path,status:"self",sha256:null}')"
  else
    entry="$(jq -cn --arg path "$canonical_path" \
      --argjson state "$(jq --arg k "$artifact" '.[$k]' <<<"$ARTIFACT_STATES")" \
      '{path:$path,status:$state.status,sha256:$state.sha256}')"
  fi
  ARTIFACTS_JSON="$(jq -c --arg k "$artifact" --argjson v "$entry" '.[$k]=$v' <<<"$ARTIFACTS_JSON")"
done < <(jq -r 'keys[]' <<<"$ARTIFACT_PATHS")

budget_state() {
  local budget="$1" block status set_by rule frozen
  block="$(awk -v key="$budget" '
    $0 == key ":" {inside=1; next}
    inside && $0 ~ /^[A-Za-z0-9_]+:$/ {exit}
    inside {print}
  ' "$CONTRACTS_ROOT/contracts/limits-v1.md")"
  status="$(sed -n 's/^[[:space:]]*status:[[:space:]]*\([^#]*\).*/\1/p' <<<"$block" | head -1 | xargs)"
  set_by="$(sed -n 's/^[[:space:]]*set_by:[[:space:]]*\(.*\)$/\1/p' <<<"$block" | head -1 | xargs)"
  rule="$(sed -n 's/^[[:space:]]*rule:[[:space:]]*\(.*\)$/\1/p' <<<"$block" | head -1 | xargs)"
  frozen=false
  if [[ "$status" =~ ^[0-9]+$ && -n "$set_by" && -n "$rule" ]]; then frozen=true; fi
  jq -cn --arg status "${status:-missing}" --arg set_by "$set_by" --arg rule "$rule" \
    --argjson frozen "$frozen" \
    '{status:$status,value:(if ($status|test("^[0-9]+$")) then ($status|tonumber) else null end),set_by:(if $set_by=="" then null else $set_by end),rule:(if $rule=="" then null else $rule end),frozen:$frozen}'
}

BUDGETS="$(jq -cn --argjson object_grants_max "$(budget_state object_grants_max)" \
  --argjson move_subtree_nodes_max "$(budget_state move_subtree_nodes_max)" \
  '{object_grants_max:$object_grants_max,move_subtree_nodes_max:$move_subtree_nodes_max}')"

V04_GATE_YAML="$CONTRACTS_ROOT/gates/v0.4-gate.yaml"
V04_GATE_RESULT="$CONTRACTS_ROOT/evidence/v0.4/gate-result.json"
V04_CONTRACT_STATUS="$(sed -n 's/^status:[[:space:]]*//p' "$V04_GATE_YAML" | head -1)"
PREDECESSOR_STATUS=not_accepted
PREDECESSOR_REASON="v0.4 contract status is ${V04_CONTRACT_STATUS:-missing}, expected accepted"
if [[ "$V04_CONTRACT_STATUS" == accepted ]]; then
  if [[ ! -f "$V04_GATE_RESULT" ]]; then
    PREDECESSOR_STATUS=artifact_missing
    PREDECESSOR_REASON="v0.4 gate-result.json is missing"
  elif ! jq empty "$V04_GATE_RESULT" >/dev/null 2>&1; then
    PREDECESSOR_STATUS=artifact_malformed
    PREDECESSOR_REASON="v0.4 gate-result.json is malformed"
  elif [[ "$(jq -r '.release // empty' "$V04_GATE_RESULT")" == 0.4.0 && \
          "$(jq -r '.gate_passed // false' "$V04_GATE_RESULT")" == true ]] && \
       jq -e '(.source | type) == "object" and (.source | has("dirty")) and
              (.source.dirty | type) == "boolean" and .source.dirty == false' \
         "$V04_GATE_RESULT" >/dev/null; then
    PREDECESSOR_STATUS=accepted
    PREDECESSOR_REASON="v0.4 contract and receipt both record acceptance"
  else
    PREDECESSOR_STATUS=receipt_not_accepted
    PREDECESSOR_REASON="v0.4 receipt does not prove a clean accepted gate"
  fi
fi
PREDECESSOR="$(jq -cn --arg status "$PREDECESSOR_STATUS" --arg reason "$PREDECESSOR_REASON" \
  --arg contract_status "${V04_CONTRACT_STATUS:-missing}" --arg receipt "$V04_GATE_RESULT" \
  --argjson requirement "$PREDECESSOR_REQUIREMENT" \
  '{status:$status,reason:$reason,contract_status:$contract_status,receipt:$receipt,requirement:$requirement}')"

required_command_entry() {
  local key="$1" command check
  command="$(jq -r --arg k "$key" '.[$k]' <<<"$REQUIRED_COMMAND_STRINGS")"
  case "$key" in
    surface_parity|collab_architecture_verify|authz_verify|multi_document_verify|cardinality_verify|invalid_update_reason_verify|relation_policy_verify|search_contract_verify|mcp_cli_equivalence_verify|audit_causation_verify)
      check="$(check_for_required_key "$key")"
      jq -cn --arg command "$command" --argjson check "$check" \
        '{command:$command,status:$check.status,exit_code:$check.exit_code,duration_ms:$check.duration_ms,executed_count:$check.executed_count,evidence:$check.evidence,sha256:$check.sha256}'
      ;;
    report)
      jq -cn --arg command "$command" '{command:$command,status:"passed",exit_code:0,duration_ms:null,executed_count:1,evidence:"evidence/v0.5/gate-result.json",sha256:null}'
      ;;
    verify|gate|manual_signoff)
      jq -cn --arg command "$command" '{command:$command,status:"not_run",exit_code:null,duration_ms:null,executed_count:0,evidence:"evidence/v0.5/gate-result.json",sha256:null}'
      ;;
    *)
      jq -cn --arg command "$command" '{command:$command,status:"producer_unspecified",exit_code:null,duration_ms:null,executed_count:0,evidence:null,sha256:null}'
      ;;
  esac
}

REQUIRED_COMMANDS='{}'
while IFS= read -r key; do
  REQUIRED_COMMANDS="$(jq -c --arg k "$key" --argjson v "$(required_command_entry "$key")" '.[$k]=$v' <<<"$REQUIRED_COMMANDS")"
done < <(jq -r 'keys[]' <<<"$REQUIRED_COMMAND_STRINGS")

GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
GATE_YAML_SHA="$(sha256_of "$GATE_YAML")"
VERIFICATION_ASSURANCE='{
  "artifact_states_and_hard_gate_verdicts": {
    "classification": "independent_dual_implementation",
    "implementations": ["jq", "python"],
    "wiring_fields_cross_checked": ["artifact", "top_level_fallback"]
  },
  "receipt_derivation": {
    "classification": "shared_single_implementation",
    "implementation": "flow_derive_receipt",
    "fields": ["blocking_reasons", "counts", "mode", "automation_passed", "candidate_ready", "gate_passed", "pending_signoffs", "blockers"]
  },
  "producer_execution_metadata": {
    "classification": "self_reported_with_integrity_checks",
    "fields": ["checks", "required_commands.status", "required_commands.exit_code", "required_commands.executed_count"],
    "log_checksums_verified": true,
    "log_contents_independently_recomputed": false
  }
}'
BASE_RECEIPT="$(jq -cn \
  --arg generated_at "$GENERATED_AT" --arg repository "$REPO_ROOT" --arg head "$SOURCE_HEAD" \
  --arg rust_version "$RUST_WORKSPACE_VERSION" --arg frontend_version "$FRONTEND_PACKAGE_VERSION" \
  --arg gate_yaml "$GATE_YAML" --arg gate_yaml_sha "$GATE_YAML_SHA" \
  --argjson source_dirty "$SOURCE_DIRTY" --argjson source_dirty_scope "$SOURCE_DIRTY_SCOPE" \
  --argjson source_dirty_entries "$SOURCE_DIRTY_ENTRIES" --argjson source_baseline "$SOURCE_BASELINE" \
  --argjson checks "$CHECKS_JSON" --argjson required_commands "$REQUIRED_COMMANDS" \
  --argjson artifacts "$ARTIFACTS_JSON" --argjson artifact_states "$ARTIFACT_STATES" \
  --argjson hard_gates "$HARD_GATES" --argjson predecessor "$PREDECESSOR" \
  --argjson budgets "$BUDGETS" \
  --argjson verification_assurance "$VERIFICATION_ASSURANCE" \
  --argjson wiring "$(jq -n -L "$ROOT_DIR/scripts/lib" 'include "flow_gate_v0_5_receipt_state"; flow_gate_wiring')" \
  '{
    schema_version:"sylvode.flow.gate-result.v1",
    schema_path:"gates/v0.5-gate.yaml",
    release:"0.5.0",
    source_baseline:$source_baseline,
    source:{repository:$repository,head:$head,dirty:$source_dirty,dirty_scope:$source_dirty_scope,dirty_entries:$source_dirty_entries,rust_workspace_version:$rust_version,frontend_package_version:$frontend_version},
    generated_at:$generated_at,
    gate_contract:{path:$gate_yaml,sha256:$gate_yaml_sha},
    mode:"blocked",automation_passed:false,candidate_ready:false,gate_passed:false,
    counts:{},checks:$checks,required_commands:$required_commands,
    artifacts:$artifacts,artifact_states:$artifact_states,artifact_wiring:$wiring,
    hard_gates:$hard_gates,predecessor:$predecessor,budgets:$budgets,
    verification_assurance:$verification_assurance,
    manual_signoffs:{
      permission_revocation:{status:"pending",reviewer:"",evidence:""},
      audit_causation:{status:"pending",reviewer:"",evidence:""}
    },
    blocking_reasons:[],pending_signoffs:[],blockers:[]
  }')"

# First derive with report provisionally passed. If every other automated
# condition is green, report itself exits/persists passed; otherwise it records
# its real exit 1 and the shared derivation is applied again.
PROVISIONAL="$(jq -c -L "$ROOT_DIR/scripts/lib" 'include "flow_gate_v0_5_receipt_state"; flow_derive_receipt' <<<"$BASE_RECEIPT")"
REPORT_EXIT=1
REPORT_STATUS=failed
if [[ "$(jq -r '.candidate_ready' <<<"$PROVISIONAL")" == true ]]; then
  REPORT_EXIT=0
  REPORT_STATUS=passed
fi
RECEIPT="$(jq -c --arg status "$REPORT_STATUS" --argjson exit_code "$REPORT_EXIT" \
  '.required_commands.report.status=$status | .required_commands.report.exit_code=$exit_code' <<<"$BASE_RECEIPT" \
  | jq -c -L "$ROOT_DIR/scripts/lib" 'include "flow_gate_v0_5_receipt_state"; flow_derive_receipt')"

GATE_RESULT_PATH="$EVIDENCE_ROOT/gate-result.json"
GATE_RESULT_TMP="$GATE_RESULT_PATH.tmp"
printf '%s\n' "$RECEIPT" | jq . > "$GATE_RESULT_TMP"
sync "$GATE_RESULT_TMP" 2>/dev/null || true
mv -f "$GATE_RESULT_TMP" "$GATE_RESULT_PATH"

SUMMARY="$(jq -c --arg gate_result "$GATE_RESULT_PATH" \
  '{release,gate_result:$gate_result,mode,candidate_ready,gate_passed,counts,predecessor_status:.predecessor.status,budgets,source_dirty:.source.dirty,failure_states:([.blocking_reasons[] | split(":")[-1]]|unique)}' \
  <<<"$RECEIPT")"
printf '%s\n' "$SUMMARY"

if [[ $MALFORMED_ARTIFACTS -gt 0 ]]; then
  exit 2
fi
exit "$REPORT_EXIT"
