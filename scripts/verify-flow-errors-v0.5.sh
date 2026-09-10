#!/usr/bin/env bash
set -euo pipefail

# v0.5 invalid_update named-reason and MCP/CLI error-surface verifier.
# Exit 0 means every required producer/consumer observation passed; exit 1
# means the artifact was produced with an honest failed verdict; exit 2 is a
# usage/tool/malformed-environment error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/flow_contract_path.sh
source "$ROOT_DIR/scripts/lib/flow_contract_path.sh"
CONTRACTS_ROOT=/opt/working/sylvode-flow
REPO_ROOT="$ROOT_DIR"
EVIDENCE_ROOT=""
CONTRACT_PATH=""
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-errors-v0.5.sh --contract PATH --json [OPTIONS]

Options:
  --contract PATH       error-mapping-v1.md (required by command contract)
  --contracts-root DIR  Default: /opt/working/sylvode-flow
  --evidence-root DIR   Required; never defaults into the contract repository
  --repo-root DIR       Default: this checkout
  --json                Required
  -h, --help            Show help

Writes invalid-update-reason-result.json atomically. Exit 0 passed, 1 failed,
2 usage/tool/environment error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --contract) CONTRACT_PATH="${2:?--contract requires PATH}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires DIR}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires DIR}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires DIR}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ $JSON_MODE -eq 1 ]] || { echo "FAIL: --json is required" >&2; exit 2; }
[[ -n "$EVIDENCE_ROOT" ]] || { echo "FAIL: --evidence-root is required" >&2; exit 2; }
[[ -n "$CONTRACT_PATH" ]] || CONTRACT_PATH="$CONTRACTS_ROOT/contracts/error-mapping-v1.md"
CONTRACT_PATH="$(flow_resolve_contract_path --contract "$CONTRACT_PATH" "$CONTRACTS_ROOT")" || exit 2
for tool in cargo git jq python3 sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required command: $tool" >&2; exit 2; }
done
[[ -d "$REPO_ROOT/.git" ]] || { echo "FAIL: --repo-root is not a git checkout" >&2; exit 2; }
REPO_ROOT="$(cd "$REPO_ROOT" && pwd)"

COMMAND_RS="$REPO_ROOT/apps/api/src/flow/command.rs"
MOVE_RS="$REPO_ROOT/apps/api/src/flow/move_object.rs"
ERROR_RS="$REPO_ROOT/apps/api/src/error.rs"
RESPONSE_RS="$REPO_ROOT/apps/api/src/response.rs"
MIGRATION_SQL="$REPO_ROOT/migrations/0056_flow_objects_parent_project_invariant.sql"
MCP_OBJECTS_RS="$REPO_ROOT/apps/mcp-server/src/tools/objects.rs"
CLI_ERROR_RS="$REPO_ROOT/apps/mcp-server/src/cli_app/error.rs"
UI_ERRORS_TS="$REPO_ROOT/frontend/src/lib/flow/errors.ts"
UI_COMMAND_TS="$REPO_ROOT/frontend/src/lib/flow/command-service.ts"
for path in "$COMMAND_RS" "$MOVE_RS" "$ERROR_RS" "$RESPONSE_RS" "$MIGRATION_SQL" \
  "$MCP_OBJECTS_RS" "$CLI_ERROR_RS" "$UI_ERRORS_TS" "$UI_COMMAND_TS"; do
  [[ -f "$path" ]] || { echo "FAIL: required source is missing: $path" >&2; exit 2; }
done

mkdir -p "$EVIDENCE_ROOT/logs"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
SOURCE_DIRTY_STATUS="$(git -C "$REPO_ROOT" status --porcelain=v1 --untracked-files=all -- \
  apps crates spikes migrations .cargo Cargo.toml Cargo.lock)"
SOURCE_DIRTY=false
[[ -n "$SOURCE_DIRTY_STATUS" ]] && SOURCE_DIRTY=true
SOURCE_DIRTY_ENTRIES="$(printf '%s\n' "$SOURCE_DIRTY_STATUS" | jq -R 'select(length>0)' | jq -s '.')"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
CONTRACT_SHA256="$(sha256sum "$CONTRACT_PATH" | awk '{print $1}')"

STATIC_PATH="$EVIDENCE_ROOT/logs/invalid-update-reason.static.json"
python3 - "$CONTRACT_PATH" "$CONTRACTS_ROOT/versions/v0.5-collaboration.md" "$COMMAND_RS" "$MOVE_RS" "$ERROR_RS" "$RESPONSE_RS" \
  "$MIGRATION_SQL" "$MCP_OBJECTS_RS" "$CLI_ERROR_RS" "$UI_ERRORS_TS" "$UI_COMMAND_TS" \
  >"$STATIC_PATH" <<'PY'
import json
import re
import sys

(contract_path, version_path, command_path, move_path, error_path, response_path, migration_path,
 mcp_path, cli_path, ui_errors_path, ui_command_path) = sys.argv[1:]

def read(path):
    with open(path, encoding="utf-8") as handle:
        return handle.read()

contract = read(contract_path)
command = read(command_path)
move = read(move_path)
error = read(error_path)
response = read(response_path)
migration = read(migration_path)
mcp = read(mcp_path)
cli = read(cli_path)
ui = read(ui_errors_path) + "\n" + read(ui_command_path)

rows = re.findall(r"^\|\s*`([^`]+)`\s*\|[^\n]*$", contract, re.M)
approved = sorted(value for value in rows if value in {
    "child_project_must_match_parent", "subtree_spans_multiple_projects"
})
contract_codes_match = re.search(
    r"MCP/CLI\s*错误统一[：:]\s*forbidden、conflict、stale_frontier、resync_required、invalid_command、limit_exceeded",
    read(version_path),
)

checks = {}
def check(name, passed, actual, reason):
    checks[name] = {"passed": bool(passed), "actual": actual,
                    "reason_code": None if passed else reason}

check("approved_reason_table_exact", approved == ["child_project_must_match_parent", "subtree_spans_multiple_projects"],
      approved, "approved_reason_table_drift")
check("create_reason_constant", bool(re.search(
    r'CHILD_PROJECT_MUST_MATCH_PARENT:\s*&str\s*=\s*"child_project_must_match_parent"', command)),
    "source scan", "reachable_reason_constant_missing")
create_branch = re.search(r"Some\(declared\).*?invalid_update_with_details\((.*?)\n\s*\}\n", command, re.S)
check("reachable_create_producer_uses_details_reason",
      bool(create_branch and "CHILD_PROJECT_MUST_MATCH_PARENT" in create_branch.group(1)
           and 'json!({ "reason"' in create_branch.group(1)),
      "create_object mismatch branch", "reachable_create_reason_not_structured")
check("reachable_create_fixture_exists",
      "create_object_refuses_a_parent_in_a_different_project_scope" in command,
      "named database fixture", "reachable_create_fixture_missing")
check("defensive_reason_constant", bool(re.search(
    r'SUBTREE_SPANS_MULTIPLE_PROJECTS:\s*&str\s*=\s*"subtree_spans_multiple_projects"', move)),
    "source scan", "defensive_reason_constant_missing")
check("defensive_producer_uses_details_reason",
      "json!({ \"reason\": SUBTREE_SPANS_MULTIPLE_PROJECTS })" in move,
      "move_object integrity branch", "defensive_reason_not_structured")
check("legal_state_invariant_is_enforced",
      "flow_objects_parent_project_fk" in migration and "FOREIGN KEY (parent_id, project_scope_id)" in migration,
      "migration 0056", "parent_project_invariant_missing")
check("rest_envelope_preserves_typed_details",
      "pub error_code: Option<&'static str>" in response and "pub details: Option<Value>" in response
      and "details: merged_details.map(Value::Object)" in error,
      "ApiResponse and ApiError::into_response", "rest_details_not_preserved")
check("mcp_preserves_code_details_and_recoverability",
      "CallToolResult::business_error" in mcp and "error.details.as_ref()" in mcp
      and "recoverable_business_error(code)" in mcp,
      "objects tool structured response", "mcp_invalid_update_contract_missing")
check("cli_preserves_invalid_update_details",
      '"invalid_update" => Self::from_kind(ApiErrorKind::InvalidUpdate, message, details)' in cli
      and "pub details: Value" in cli and "exit::INVALID" in cli,
      "CliError::from_structured", "cli_invalid_update_contract_missing")
generic_mcp = "CallToolResult::business_error" in mcp and "error.details.as_ref()" in mcp
unified = {
    "forbidden": generic_mcp and '"forbidden"' in cli,
    "conflict": "exit::CONFLICT" in cli,
    "stale_frontier": generic_mcp and '"stale_frontier"' in cli,
    "resync_required": generic_mcp and '"resync_required"' in cli,
    "invalid_command": generic_mcp and '"invalid_update"' in cli,
    "limit_exceeded": generic_mcp and '"limit_exceeded"' in cli,
}
check("mcp_cli_unified_error_surface", bool(contract_codes_match) and all(unified.values()), unified,
      "mcp_cli_unified_error_surface_incomplete")

# The named reason is meaningful only when consumers branch on details.reason.
# Generic invalid_update handling is deliberately not credited as coverage.
ui_named = (
    "child_project_must_match_parent" in ui
    and "subtree_spans_multiple_projects" in ui
    and re.search(r"details[^\n]{0,120}reason|reason[^\n]{0,120}details", ui)
)
check("ui_named_reason_consumer", bool(ui_named), "production TypeScript scan",
      "ui_named_reason_consumer_missing")
check("unknown_reason_falls_back_without_message_branching",
      bool(ui_named and re.search(r"unknown|default|otherwise", ui, re.I)
           and not re.search(r"message\.(?:includes|match)|message\.includes", ui)),
      "production TypeScript scan", "unknown_reason_fallback_not_proven")
check("defensive_reason_enters_integrity_repair",
      bool(ui_named and re.search(r"integrity|repair", ui, re.I)),
      "production TypeScript scan", "integrity_repair_consumer_missing")

print(json.dumps({"approved_reasons": approved, "unified_error_surface": unified,
                  "checks": checks, "passed": all(item["passed"] for item in checks.values())},
                 separators=(",", ":")))
PY

run_timed() {
  local log="$1"; shift
  local started ended status
  started="$(date +%s%N)"
  set +e
  "$@" >"$log" 2>&1
  status=$?
  set -e
  ended="$(date +%s%N)"
  printf '%s %s\n' "$status" "$(((ended - started) / 1000000))"
}

export OPENPR_TEST_DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-postgresql://flowtest:flowtest@127.0.0.1:25433/postgres}"
API_LOG="$EVIDENCE_ROOT/logs/invalid-update-reason.api-test.log"
read -r API_EXIT API_MS < <(run_timed "$API_LOG" cargo test -p api \
  create_object_refuses_a_parent_in_a_different_project_scope -- --nocapture)
CLI_LOG="$EVIDENCE_ROOT/logs/invalid-update-reason.cli-test.log"
read -r CLI_EXIT CLI_MS < <(run_timed "$CLI_LOG" cargo test -p mcp-server \
  from_structured_maps_every_stable_code_to_the_frozen_exit_table -- --nocapture)

strict_one_passed() {
  local path="$1"
  [[ "$(grep -Ec '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out; finished in [0-9.]+s$' "$path")" -eq 1 ]]
}
API_DYNAMIC=false
CLI_DYNAMIC=false
if [[ $API_EXIT -eq 0 ]] && strict_one_passed "$API_LOG" && \
   ! grep -Fq 'skipped: OPENPR_TEST_DATABASE_URL is not set' "$API_LOG"; then API_DYNAMIC=true; fi
if [[ $CLI_EXIT -eq 0 ]] && strict_one_passed "$CLI_LOG"; then CLI_DYNAMIC=true; fi

MUTATION_DIR="$(mktemp -d /opt/worker/.cache/flow-v05-errors-mutation.XXXXXX)"
trap 'rm -rf "$MUTATION_DIR"' EXIT
sed 's/child_project_must_match_parent/child_project_reason_mutated/g' "$COMMAND_RS" >"$MUTATION_DIR/command.rs"
sed 's/subtree_spans_multiple_projects/subtree_reason_mutated/g' "$MOVE_RS" >"$MUTATION_DIR/move_object.rs"
CREATE_MUTATION_RED=false
DEFENSIVE_MUTATION_RED=false
if ! grep -Fq 'pub const CHILD_PROJECT_MUST_MATCH_PARENT: &str = "child_project_must_match_parent"' "$MUTATION_DIR/command.rs"; then
  CREATE_MUTATION_RED=true
fi
if ! grep -Fq 'pub const SUBTREE_SPANS_MULTIPLE_PROJECTS: &str = "subtree_spans_multiple_projects"' "$MUTATION_DIR/move_object.rs"; then
  DEFENSIVE_MUTATION_RED=true
fi

STATIC_PASSED="$(jq -r .passed "$STATIC_PATH")"
OVERALL_PASSED=false
if [[ "$STATIC_PASSED" == true && "$API_DYNAMIC" == true && "$CLI_DYNAMIC" == true && \
      "$CREATE_MUTATION_RED" == true && "$DEFENSIVE_MUTATION_RED" == true && "$SOURCE_DIRTY" == false ]]; then
  OVERALL_PASSED=true
fi

REASONS="$(jq -c --argjson api "$API_DYNAMIC" --argjson cli "$CLI_DYNAMIC" \
  --argjson cm "$CREATE_MUTATION_RED" --argjson dm "$DEFENSIVE_MUTATION_RED" \
  --argjson clean "$([[ "$SOURCE_DIRTY" == false ]] && echo true || echo false)" '
  [.checks|to_entries[]|select(.value.passed != true)|.value.reason_code]
  + (if $api then [] else ["reachable_create_dynamic_fixture_failed_or_skipped"] end)
  + (if $cli then [] else ["cli_error_mapping_dynamic_fixture_failed"] end)
  + (if $cm then [] else ["reachable_reason_mutation_did_not_turn_red"] end)
  + (if $dm then [] else ["defensive_reason_mutation_did_not_turn_red"] end)
  + (if $clean then [] else ["source_dirty"] end) | unique' "$STATIC_PATH")"
GATE="$(jq -cn --argjson passed "$OVERALL_PASSED" --argjson reasons "$REASONS" \
  '{status:(if $passed then "passed" else "failed" end),passed:$passed,
    reason_code:(if $passed then null else ($reasons[0] // "invalid_update_reason_evidence_failed") end),
    reason_codes:$reasons}')"

RESULT="$(jq -n --arg head "$SOURCE_HEAD" --arg generated "$GENERATED_AT" \
  --arg contract "$CONTRACT_PATH" --arg contract_sha "$CONTRACT_SHA256" \
  --argjson dirty "$SOURCE_DIRTY" --argjson dirty_entries "$SOURCE_DIRTY_ENTRIES" \
  --argjson static "$(cat "$STATIC_PATH")" --argjson api_exit "$API_EXIT" --argjson api_ms "$API_MS" \
  --arg api_log "evidence/v0.5/logs/$(basename "$API_LOG")" \
  --argjson api_passed "$API_DYNAMIC" --argjson cli_exit "$CLI_EXIT" --argjson cli_ms "$CLI_MS" \
  --arg cli_log "evidence/v0.5/logs/$(basename "$CLI_LOG")" --argjson cli_passed "$CLI_DYNAMIC" \
  --argjson create_red "$CREATE_MUTATION_RED" --argjson defensive_red "$DEFENSIVE_MUTATION_RED" \
  --argjson gate "$GATE" --argjson passed "$OVERALL_PASSED" '{
    schema_version:"sylvode.flow.invalid-update-reason-result.v1",
    source_head:$head,source_dirty:$dirty,source_dirty_entries:$dirty_entries,generated_at:$generated,
    contract:{path:$contract,sha256:$contract_sha},
    static:$static,
    commands:{
      reachable_create_fixture:{exit:$api_exit,duration_ms:$api_ms,log:$api_log,passed:$api_passed},
      cli_error_mapping_fixture:{exit:$cli_exit,duration_ms:$cli_ms,log:$cli_log,passed:$cli_passed}
    },
    mutations:{reachable_reason_rename:{red:$create_red},defensive_reason_rename:{red:$defensive_red}},
    hard_gates:{invalid_update_named_reason_producer_consumer_contract:$gate.status},
    gate_details:{invalid_update_named_reason_producer_consumer_contract:$gate},
    passed:$passed
  }')"

OUT="$EVIDENCE_ROOT/invalid-update-reason-result.json"
TMP="$OUT.tmp"
printf '%s\n' "$RESULT" | jq . >"$TMP"
sync "$TMP" 2>/dev/null || true
mv -f "$TMP" "$OUT"
printf '%s\n' "$RESULT"
[[ "$OVERALL_PASSED" == true ]] && exit 0
exit 1
