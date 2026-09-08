#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.5 gate-result verifier.
#
# This command runs no product action. It independently reloads the v0.5 YAML,
# evidence files, producer availability, source identity/dirty scope,
# predecessor and named budgets; recomputes all 31 gate verdicts; verifies log
# and artifact checksums; and compares every derived receipt field. Honest
# blocked receipts can be internally consistent but still exit 1.
#
# Exit codes: 0 = receipt consistent and candidate automation green; 1 =
# drift or any non-pass; 2 = usage/tool/structurally malformed evidence.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.5"
REPO_ROOT="$ROOT_DIR"
GATE_YAML=""
GATE_RESULT_PATH=""
JSON_MODE=0
STATE_LIBRARY="$ROOT_DIR/scripts/lib/flow_gate_v0_5_receipt_state.jq"

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-v0.5-json.sh GATE_RESULT_JSON [OPTIONS]

Independently recomputes all 31 v0.5 gate states from on-disk evidence.

Options:
  --evidence-root DIR   Artifact/log root. Default:
                        /opt/working/sylvode-flow/evidence/v0.5
  --contracts-root DIR Contract repository.
  --repo-root DIR       Source repository. Default: this checkout.
  --gate-yaml PATH      Gate YAML. Default:
                        <contracts-root>/gates/v0.5-gate.yaml
  --json                Required for command-contract compatibility.
  -h, --help            Show help and exit 0.

Exit codes: 0 consistent candidate, 1 drift/non-pass, 2 malformed.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires DIR}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires DIR}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires DIR}"; shift 2 ;;
    --gate-yaml) GATE_YAML="${2:?--gate-yaml requires PATH}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)
      if [[ -n "$GATE_RESULT_PATH" ]]; then
        echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2
      fi
      GATE_RESULT_PATH="$1"; shift ;;
  esac
done

if [[ -z "$GATE_RESULT_PATH" ]]; then
  echo "FAIL: GATE_RESULT_JSON is required" >&2
  exit 2
fi
if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  exit 2
fi
for tool in jq sha256sum git awk sed xargs; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    exit 2
  fi
done
if [[ ! -f "$GATE_RESULT_PATH" ]] || ! jq empty "$GATE_RESULT_PATH" >/dev/null 2>&1; then
  echo "FAIL: gate-result is missing or not valid JSON: $GATE_RESULT_PATH" >&2
  exit 2
fi
if [[ "$(jq -r type "$GATE_RESULT_PATH")" != object ]]; then
  echo "FAIL: gate-result top level must be an object" >&2
  exit 2
fi
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi
REPO_ROOT="$(cd "$REPO_ROOT" && pwd)"
[[ -n "$GATE_YAML" ]] || GATE_YAML="$CONTRACTS_ROOT/gates/v0.5-gate.yaml"
if [[ ! -f "$GATE_YAML" || ! -f "$STATE_LIBRARY" ]]; then
  echo "FAIL: gate YAML or shared state library is missing" >&2
  exit 2
fi

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
WIRING="$(jq -n -L "$ROOT_DIR/scripts/lib" 'include "flow_gate_v0_5_receipt_state"; flow_gate_wiring')"
if [[ "$(jq 'length' <<<"$YAML_HARD_GATES")" -ne 31 ]] || \
   ! jq -e --argjson wiring "$(jq 'keys' <<<"$WIRING")" 'keys==$wiring' >/dev/null <<<"$YAML_HARD_GATES"; then
  echo "FAIL: YAML hard_gates and shared 31-entry wiring do not match" >&2
  exit 2
fi

REQUIRED_TOP_KEYS='["schema_version","schema_path","release","source_baseline","source","generated_at","gate_contract","mode","automation_passed","candidate_ready","gate_passed","counts","checks","required_commands","artifacts","artifact_states","artifact_wiring","hard_gates","predecessor","budgets","manual_signoffs","blocking_reasons","pending_signoffs","blockers"]'
STRUCT_ERRORS='[]'
while IFS= read -r key; do
  if [[ "$(jq --arg k "$key" 'has($k)' "$GATE_RESULT_PATH")" != true ]]; then
    STRUCT_ERRORS="$(jq -c --arg e "missing top-level key: $key" '.+[$e]' <<<"$STRUCT_ERRORS")"
  fi
done < <(jq -r '.[]' <<<"$REQUIRED_TOP_KEYS")
for typed in source:object source_baseline:object gate_contract:object counts:object \
  checks:array required_commands:object artifacts:object \
  artifact_states:object artifact_wiring:object hard_gates:object predecessor:object \
  budgets:object manual_signoffs:object blocking_reasons:array pending_signoffs:array blockers:array; do
  key="${typed%%:*}"; expected="${typed##*:}"
  actual="$(jq -r --arg k "$key" 'if has($k) then (.[$k]|type) else "missing" end' "$GATE_RESULT_PATH")"
  if [[ "$actual" != missing && "$actual" != "$expected" ]]; then
    STRUCT_ERRORS="$(jq -c --arg e "$key must be $expected, found $actual" '.+[$e]' <<<"$STRUCT_ERRORS")"
  fi
done
if ! jq -e '
  all(.checks[]; type == "object") and
  all(.required_commands[]; type == "object") and
  all(.artifacts[]; type == "object") and
  all(.artifact_states[]; type == "object") and
  all(.manual_signoffs[]; type == "object")
' "$GATE_RESULT_PATH" >/dev/null 2>&1; then
  STRUCT_ERRORS="$(jq -c --arg e "receipt collection entries must be objects" '.+[$e]' <<<"$STRUCT_ERRORS")"
fi
if [[ "$(jq length <<<"$STRUCT_ERRORS")" -gt 0 ]]; then
  jq -cn --argjson errors "$STRUCT_ERRORS" '{passed:false,malformed:true,errors:$errors}'
  exit 2
fi

DRIFT='[]'
add_drift() { DRIFT="$(jq -c --arg e "$1" '.+[$e]' <<<"$DRIFT")"; }

[[ "$(jq -r '.schema_version' "$GATE_RESULT_PATH")" == sylvode.flow.gate-result.v1 ]] || add_drift "schema_version mismatch"
[[ "$(jq -r '.schema_path' "$GATE_RESULT_PATH")" == gates/v0.5-gate.yaml ]] || add_drift "schema_path mismatch"
[[ "$(jq -r '.release' "$GATE_RESULT_PATH")" == 0.5.0 ]] || add_drift "release mismatch"

for section in artifacts required_commands hard_gates; do
  case "$section" in
    artifacts) expected="$(jq 'keys' <<<"$ARTIFACT_PATHS")" ;;
    required_commands) expected="$(jq 'keys' <<<"$REQUIRED_COMMAND_STRINGS")" ;;
    hard_gates) expected="$(jq 'keys' <<<"$YAML_HARD_GATES")" ;;
  esac
  actual="$(jq ".$section|keys" "$GATE_RESULT_PATH")"
  [[ "$actual" == "$expected" ]] || add_drift "$section keys do not exactly match v0.5-gate.yaml"
done
# ADR-0017: multi_user and offline_recovery moved to the frontend track.
MANUAL_KEYS='["audit_causation","permission_revocation"]'
[[ "$(jq -c '.manual_signoffs|keys' "$GATE_RESULT_PATH")" == "$MANUAL_KEYS" ]] || add_drift "manual_signoffs keys mismatch"
while IFS= read -r manual_key; do
  manual_status="$(jq -r --arg k "$manual_key" '.manual_signoffs[$k].status // "missing"' "$GATE_RESULT_PATH")"
  case "$manual_status" in
    pending|passed|failed|needs_rework) ;;
    *) add_drift "manual_signoffs.$manual_key has invalid status $manual_status" ;;
  esac
done < <(jq -r '.[]' <<<"$MANUAL_KEYS")
[[ "$(jq -c '.artifact_wiring' "$GATE_RESULT_PATH")" == "$(jq -c . <<<"$WIRING")" ]] || add_drift "artifact_wiring drift"
[[ "$(jq -c '.source_baseline' "$GATE_RESULT_PATH")" == "$(jq -c . <<<"$SOURCE_BASELINE")" ]] || add_drift "source_baseline drift from YAML"

SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
SOURCE_DIRTY_SCOPE='["apps/","crates/","spikes/","migrations/",".cargo/","Cargo.toml","Cargo.lock"]'
SOURCE_DIRTY_STATUS="$(git -C "$REPO_ROOT" status --porcelain=v1 --untracked-files=all -- \
  apps crates spikes migrations .cargo Cargo.toml Cargo.lock)"
SOURCE_DIRTY_ENTRIES="$(printf '%s\n' "$SOURCE_DIRTY_STATUS" | jq -R 'select(length>0)' | jq -s '.')"
SOURCE_DIRTY=$([[ -n "$SOURCE_DIRTY_STATUS" ]] && echo true || echo false)
[[ "$(jq -r '.source.head' "$GATE_RESULT_PATH")" == "$SOURCE_HEAD" ]] || add_drift "source.head does not match actual repository HEAD"
[[ "$(jq -r '.source.dirty|tostring' "$GATE_RESULT_PATH")" == "$SOURCE_DIRTY" ]] || add_drift "source.dirty does not match scoped working tree"
[[ "$(jq -c '.source.dirty_scope' "$GATE_RESULT_PATH")" == "$SOURCE_DIRTY_SCOPE" ]] || add_drift "source.dirty_scope mismatch"
[[ "$(jq -c '.source.dirty_entries' "$GATE_RESULT_PATH")" == "$(jq -c . <<<"$SOURCE_DIRTY_ENTRIES")" ]] || add_drift "source.dirty_entries mismatch"
GATE_YAML_SHA="$(sha256_of "$GATE_YAML")"
[[ "$(jq -r '.gate_contract.sha256' "$GATE_RESULT_PATH")" == "$GATE_YAML_SHA" ]] || add_drift "gate contract checksum mismatch"

producer_metadata() {
  local artifact="$1" command_key="" command="" script_rel="" status=producer_unspecified
  case "$artifact" in
    surface_coverage_result) command_key=surface_parity ;;
    collab_architecture_result) command_key=collab_architecture_verify ;;
    authz_result) command_key=authz_verify ;;
    multi_document_result) command_key=multi_document_verify ;;
    cardinality_result) command_key=cardinality_verify ;;
    invalid_update_reason_result) command_key=invalid_update_reason_verify ;;
    convergence_result)
      command='scripts/verify-flow-convergence-v0.5.sh --clients 10 --json'
      script_rel=scripts/verify-flow-convergence-v0.5.sh
      ;;
  esac
  if [[ -n "$command_key" ]]; then
    command="$(jq -r --arg k "$command_key" '.[$k] // empty' <<<"$REQUIRED_COMMAND_STRINGS")"
    script_rel="${command%% *}"
  fi
  if [[ -n "$command" ]]; then
    if [[ -f "$REPO_ROOT/$script_rel" ]]; then status=available; else status=producer_missing; fi
  fi
  jq -cn --arg status "$status" --arg command "$command" \
    '{producer_status:$status,producer_command:(if $command=="" then null else $command end)}'
}

ARTIFACT_INPUTS='{}'
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
      valid_json=true; sha="\"$(sha256_of "$artifact_path")\""; document="$(jq -c . "$artifact_path")"
    else
      MALFORMED_ARTIFACTS=$((MALFORMED_ARTIFACTS + 1))
    fi
  fi
  input="$(jq -cn --arg path "$canonical_path" --argjson exists "$exists" --argjson valid_json "$valid_json" \
    --argjson sha256 "$sha" --argjson document "$document" --argjson metadata "$metadata" \
    '$metadata+{path:$path,exists:$exists,valid_json:$valid_json,sha256:$sha256,document:$document}')"
  ARTIFACT_INPUTS="$(jq -c --arg k "$artifact" --argjson v "$input" '.[$k]=$v' <<<"$ARTIFACT_INPUTS")"
done < <(jq -r 'keys[]' <<<"$ARTIFACT_PATHS")

RECOMPUTED_STATES="$(jq -cn -L "$ROOT_DIR/scripts/lib" --argjson inputs "$ARTIFACT_INPUTS" --arg head "$SOURCE_HEAD" \
  'include "flow_gate_v0_5_receipt_state"; flow_compute_artifact_states($inputs;$head)')"
RECOMPUTED_GATES="$(jq -cn -L "$ROOT_DIR/scripts/lib" --argjson states "$RECOMPUTED_STATES" \
  'include "flow_gate_v0_5_receipt_state"; flow_compute_hard_gates($states)')"
[[ "$(jq -c '.artifact_states' "$GATE_RESULT_PATH")" == "$(jq -c . <<<"$RECOMPUTED_STATES")" ]] || add_drift "artifact_states drift from on-disk evidence"
[[ "$(jq -c '.hard_gates' "$GATE_RESULT_PATH")" == "$(jq -c . <<<"$RECOMPUTED_GATES")" ]] || add_drift "hard_gates drift from on-disk evidence"

while IFS= read -r artifact; do
  path="$(jq -r --arg k "$artifact" '.[$k]' <<<"$ARTIFACT_PATHS")"
  [[ "$(jq -r --arg k "$artifact" '.artifacts[$k].path // empty' "$GATE_RESULT_PATH")" == "$path" ]] || add_drift "artifacts.$artifact.path mismatch"
  if [[ "$artifact" != gate_result ]]; then
    expected_sha="$(jq -r --arg k "$artifact" '.[$k].sha256 // ""' <<<"$RECOMPUTED_STATES")"
    recorded_sha="$(jq -r --arg k "$artifact" '.artifacts[$k].sha256 // ""' "$GATE_RESULT_PATH")"
    expected_status="$(jq -r --arg k "$artifact" '.[$k].status' <<<"$RECOMPUTED_STATES")"
    recorded_status="$(jq -r --arg k "$artifact" '.artifacts[$k].status // ""' "$GATE_RESULT_PATH")"
    [[ "$recorded_sha" == "$expected_sha" ]] || add_drift "artifacts.$artifact.sha256 mismatch"
    [[ "$recorded_status" == "$expected_status" ]] || add_drift "artifacts.$artifact.status mismatch"
  fi
done < <(jq -r 'keys[]' <<<"$ARTIFACT_PATHS")

CHECK_COUNT="$(jq '.checks|length' "$GATE_RESULT_PATH")"
for ((i=0; i<CHECK_COUNT; i++)); do
  status="$(jq -r ".checks[$i].status // empty" "$GATE_RESULT_PATH")"
  id="$(jq -r ".checks[$i].id // empty" "$GATE_RESULT_PATH")"
  rel="$(jq -r ".checks[$i].evidence // empty" "$GATE_RESULT_PATH")"
  sha="$(jq -r ".checks[$i].sha256 // empty" "$GATE_RESULT_PATH")"
  exit_type="$(jq -r ".checks[$i].exit_code|type" "$GATE_RESULT_PATH")"
  case "$status" in passed|failed|not_run|producer_missing|producer_unspecified) ;; *) add_drift "check $id has unknown status $status" ;; esac
  if [[ "$status" == passed && "$(jq -r ".checks[$i].exit_code" "$GATE_RESULT_PATH")" != 0 ]]; then add_drift "check $id passed without exit 0"; fi
  if [[ "$status" == failed && "$exit_type" != number ]]; then add_drift "check $id failed without numeric exit"; fi
  if [[ "$status" != passed && "$status" != failed && "$exit_type" != null ]]; then add_drift "check $id $status must have exit null"; fi
  log="$EVIDENCE_ROOT/logs/$(basename "$rel")"
  if [[ -z "$rel" || ! -f "$log" ]]; then add_drift "check $id log missing"
  elif [[ "$(sha256_of "$log")" != "$sha" ]]; then add_drift "check $id log checksum mismatch"
  fi
done

for key in surface_parity collab_architecture_verify authz_verify multi_document_verify cardinality_verify invalid_update_reason_verify; do
  command="$(jq -r --arg k "$key" '.[$k]' <<<"$REQUIRED_COMMAND_STRINGS")"
  [[ "$(jq -r --arg k "$key" '.required_commands[$k].command // empty' "$GATE_RESULT_PATH")" == "$command" ]] || add_drift "required_commands.$key command drift"
  check="$(jq -c --arg id "required.$key" '[.checks[]|select(.id==$id)][0]//null' "$GATE_RESULT_PATH")"
  if [[ "$check" == null ]]; then add_drift "required producer check missing: $key"
  else
    [[ "$(jq -c --arg k "$key" '.required_commands[$k] | {status,exit_code,evidence,sha256}' "$GATE_RESULT_PATH")" == \
       "$(jq -c '{status,exit_code,evidence,sha256}' <<<"$check")" ]] || add_drift "required_commands.$key does not match its check"
  fi
done
for key in report verify gate manual_signoff; do
  command="$(jq -r --arg k "$key" '.[$k]' <<<"$REQUIRED_COMMAND_STRINGS")"
  [[ "$(jq -r --arg k "$key" '.required_commands[$k].command // empty' "$GATE_RESULT_PATH")" == "$command" ]] || add_drift "required_commands.$key command drift"
done

budget_state() {
  local budget="$1" block status set_by rule frozen
  block="$(awk -v key="$budget" '$0==key ":"{inside=1;next} inside&&$0~/^[A-Za-z0-9_]+:$/{exit} inside{print}' "$CONTRACTS_ROOT/contracts/limits-v1.md")"
  status="$(sed -n 's/^[[:space:]]*status:[[:space:]]*\([^#]*\).*/\1/p' <<<"$block" | head -1 | xargs)"
  set_by="$(sed -n 's/^[[:space:]]*set_by:[[:space:]]*\(.*\)$/\1/p' <<<"$block" | head -1 | xargs)"
  rule="$(sed -n 's/^[[:space:]]*rule:[[:space:]]*\(.*\)$/\1/p' <<<"$block" | head -1 | xargs)"
  frozen=false; if [[ "$status" =~ ^[0-9]+$ && -n "$set_by" && -n "$rule" ]]; then frozen=true; fi
  jq -cn --arg status "${status:-missing}" --arg set_by "$set_by" --arg rule "$rule" --argjson frozen "$frozen" \
    '{status:$status,value:(if ($status|test("^[0-9]+$")) then ($status|tonumber) else null end),set_by:(if $set_by=="" then null else $set_by end),rule:(if $rule=="" then null else $rule end),frozen:$frozen}'
}
RECOMPUTED_BUDGETS="$(jq -cn --argjson object_grants_max "$(budget_state object_grants_max)" \
  --argjson move_subtree_nodes_max "$(budget_state move_subtree_nodes_max)" \
  '{object_grants_max:$object_grants_max,move_subtree_nodes_max:$move_subtree_nodes_max}')"

V04_STATUS="$(sed -n 's/^status:[[:space:]]*//p' "$CONTRACTS_ROOT/gates/v0.4-gate.yaml" | head -1)"
V04_RECEIPT="$CONTRACTS_ROOT/evidence/v0.4/gate-result.json"
PSTATUS=not_accepted; PREASON="v0.4 contract status is ${V04_STATUS:-missing}, expected accepted"
if [[ "$V04_STATUS" == accepted ]]; then
  if [[ ! -f "$V04_RECEIPT" ]]; then PSTATUS=artifact_missing; PREASON="v0.4 gate-result.json is missing"
  elif ! jq empty "$V04_RECEIPT" >/dev/null 2>&1; then PSTATUS=artifact_malformed; PREASON="v0.4 gate-result.json is malformed"
  elif [[ "$(jq -r '.release//empty' "$V04_RECEIPT")" == 0.4.0 && "$(jq -r '.gate_passed//false' "$V04_RECEIPT")" == true && "$(jq -r '.source.dirty//true' "$V04_RECEIPT")" == false ]]; then
    PSTATUS=accepted; PREASON="v0.4 contract and receipt both record acceptance"
  else PSTATUS=receipt_not_accepted; PREASON="v0.4 receipt does not prove a clean accepted gate"; fi
fi
RECOMPUTED_PREDECESSOR="$(jq -cn --arg status "$PSTATUS" --arg reason "$PREASON" --arg contract_status "${V04_STATUS:-missing}" \
  --arg receipt "$V04_RECEIPT" --argjson requirement "$PREDECESSOR_REQUIREMENT" \
  '{status:$status,reason:$reason,contract_status:$contract_status,receipt:$receipt,requirement:$requirement}')"
[[ "$(jq -c '.budgets' "$GATE_RESULT_PATH")" == "$(jq -c . <<<"$RECOMPUTED_BUDGETS")" ]] || add_drift "budget state drift"
[[ "$(jq -c '.predecessor' "$GATE_RESULT_PATH")" == "$(jq -c . <<<"$RECOMPUTED_PREDECESSOR")" ]] || add_drift "predecessor state drift"

EXPECTED_BASE="$(jq -c --argjson states "$RECOMPUTED_STATES" --argjson gates "$RECOMPUTED_GATES" \
  --argjson budgets "$RECOMPUTED_BUDGETS" --argjson predecessor "$RECOMPUTED_PREDECESSOR" \
  --argjson dirty "$SOURCE_DIRTY" --argjson dirty_scope "$SOURCE_DIRTY_SCOPE" --argjson dirty_entries "$SOURCE_DIRTY_ENTRIES" \
  '.artifact_states=$states | .hard_gates=$gates | .budgets=$budgets | .predecessor=$predecessor |
   .source.dirty=$dirty | .source.dirty_scope=$dirty_scope | .source.dirty_entries=$dirty_entries |
   .required_commands.report.status="passed" | .required_commands.report.exit_code=0' "$GATE_RESULT_PATH")"
PROVISIONAL="$(jq -c -L "$ROOT_DIR/scripts/lib" 'include "flow_gate_v0_5_receipt_state"; flow_derive_receipt' <<<"$EXPECTED_BASE")"
EXPECTED_REPORT_STATUS=failed; EXPECTED_REPORT_EXIT=1
if [[ "$(jq -r '.candidate_ready' <<<"$PROVISIONAL")" == true ]]; then EXPECTED_REPORT_STATUS=passed; EXPECTED_REPORT_EXIT=0; fi
EXPECTED="$(jq -c --arg status "$EXPECTED_REPORT_STATUS" --argjson exit_code "$EXPECTED_REPORT_EXIT" \
  '.required_commands.report.status=$status | .required_commands.report.exit_code=$exit_code' <<<"$EXPECTED_BASE" \
  | jq -c -L "$ROOT_DIR/scripts/lib" 'include "flow_gate_v0_5_receipt_state"; flow_derive_receipt')"
for path in mode automation_passed candidate_ready gate_passed counts blocking_reasons pending_signoffs blockers; do
  [[ "$(jq -c ".$path" "$GATE_RESULT_PATH")" == "$(jq -c ".$path" <<<"$EXPECTED")" ]] || add_drift "derived $path drift"
done
[[ "$(jq -c '.required_commands.report|{status,exit_code}' "$GATE_RESULT_PATH")" == "$(jq -c '.required_commands.report|{status,exit_code}' <<<"$EXPECTED")" ]] || add_drift "report command status drift"

AUTOMATION_PASSED="$(jq -r '.candidate_ready' <<<"$EXPECTED")"
PASSED=false
if [[ "$(jq length <<<"$DRIFT")" -eq 0 && "$AUTOMATION_PASSED" == true ]]; then PASSED=true; fi
RESULT="$(jq -cn --arg gate_result "$GATE_RESULT_PATH" --argjson receipt_consistent "$([[ "$(jq length <<<"$DRIFT")" -eq 0 ]] && echo true || echo false)" \
  --argjson automation_passed "$AUTOMATION_PASSED" --argjson drift "$DRIFT" \
  --argjson hard_gates "$RECOMPUTED_GATES" --argjson artifact_states "$RECOMPUTED_STATES" --argjson passed "$PASSED" \
  '{gate_result:$gate_result,receipt_consistent:$receipt_consistent,automation_passed:$automation_passed,hard_gate_counts:($hard_gates|to_entries|group_by(.value)|map({(.[0].value):length})|add),artifact_states:($artifact_states|with_entries(.value=.value.status)),drift:$drift,passed:$passed}')"
printf '%s\n' "$RESULT"
if [[ $MALFORMED_ARTIFACTS -gt 0 ]]; then exit 2; fi
if [[ "$PASSED" == true ]]; then exit 0; fi
exit 1
