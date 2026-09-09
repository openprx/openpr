#!/usr/bin/env bash
set -euo pipefail

# v0.5 accepted-projection Flow search and frozen legacy-search verifier.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/flow_contract_path.sh
source "$ROOT_DIR/scripts/lib/flow_contract_path.sh"

REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
ADR_PATH=""
CONTRACT_PATH=""
EVIDENCE_ROOT="${TMPDIR:-/tmp}/openpr-flow-evidence/v0.5"
DATABASE_URL="postgresql://flowtest:flowtest@127.0.0.1:25433/postgres"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-search-contract-v0.5.sh --adr PATH --contract PATH --json [OPTIONS]

Options:
  --adr PATH              ADR-0009 Flow search decision (required).
  --contract PATH         rest-api-v1.md (required).
  --contracts-root DIR    Contract checkout.
  --evidence-root DIR     Output directory.
  --repo-root DIR         Source checkout.
  --database-url URL      PostgreSQL test authority.
  --json                  Required; print the artifact.
  -h, --help              Show this help.

Exit codes: 0 all gates passed; 1 a gate failed; 2 malformed evidence.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --adr) ADR_PATH="${2:?--adr requires a path}"; shift 2 ;;
    --contract) CONTRACT_PATH="${2:?--contract requires a path}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a directory}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a directory}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a directory}"; shift 2 ;;
    --database-url) DATABASE_URL="${2:?--database-url requires a URL}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$ADR_PATH" || -z "$CONTRACT_PATH" || $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --adr, --contract, and --json are required" >&2
  usage >&2
  exit 2
fi
for tool in cargo git jq python3 realpath; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required tool: $tool" >&2; exit 2; }
done
ADR_PATH="$(flow_resolve_contract_path --adr "$ADR_PATH" "$CONTRACTS_ROOT")" || exit 2
CONTRACT_PATH="$(flow_resolve_contract_path --contract "$CONTRACT_PATH" "$CONTRACTS_ROOT")" || exit 2
git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1 || exit 2

CONTRACTS_REAL="$(realpath -m "$CONTRACTS_ROOT")"
EVIDENCE_REAL="$(realpath -m "$EVIDENCE_ROOT")"
case "$EVIDENCE_REAL/" in
  "$CONTRACTS_REAL/"*) echo "FAIL: refusing to write evidence into the contract checkout" >&2; exit 2 ;;
esac
mkdir -p "$EVIDENCE_REAL/logs"

SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
SOURCE_DIRTY_STATUS="$(git -C "$REPO_ROOT" status --porcelain=v1 --untracked-files=all -- apps crates spikes migrations .cargo Cargo.toml Cargo.lock)"
SOURCE_DIRTY=$([[ -n "$SOURCE_DIRTY_STATUS" ]] && echo true || echo false)
SOURCE_DIRTY_ENTRIES="$(printf '%s\n' "$SOURCE_DIRTY_STATUS" | jq -R 'select(length > 0)' | jq -s '.')"

BUILD_LOG="$EVIDENCE_REAL/logs/search-contract.worker-build.log"
TEST_LOG="$EVIDENCE_REAL/logs/search-contract.cargo-test.log"
MUTATION_LOG="$EVIDENCE_REAL/logs/search-contract.mutation.log"

run_timed() {
  local log="$1"
  shift
  local start end status
  start="$(date +%s%N)"
  set +e
  (cd "$REPO_ROOT" && "$@") >"$log" 2>&1
  status=$?
  set -e
  end="$(date +%s%N)"
  printf '%s %s\n' "$status" "$(((end - start) / 1000000))"
}

echo "=== build collab-isolated-apply-worker ===" >&2
read -r BUILD_EXIT BUILD_MS < <(run_timed "$BUILD_LOG" cargo build --locked -p collab-core --bin collab-isolated-apply-worker)
echo "  exit=$BUILD_EXIT duration_ms=$BUILD_MS log=$BUILD_LOG" >&2

readarray -t API_TESTS <<'EOF'
routes::flow::flow_database_tests::flow_search_filters_before_cardinality_cursor_snippet_and_frontier
routes::flow::flow_database_tests::flow_search_large_scope_with_one_match_does_not_spend_candidate_scan_budget_on_frontier
routes::flow::flow_database_tests::flow_search_snippet_distinguishes_literal_entity_text_from_real_markup
routes::flow::flow_database_tests::flow_search_stale_projection_is_explicit_and_require_current_fails_closed
routes::flow::flow_database_tests::flow_search_rejects_each_invalid_scope_and_bot_all_visible_but_allows_bot_single_scope
routes::search::tests::db_search_is_scoped_to_workspace_and_project
routes::search::tests::db_bot_search_is_scoped_by_token_workspace
EOF
WORKER_TEST="flow_projection::tests::flow_projection_indexes_only_the_accepted_projection_and_tracks_lag"
: >"$TEST_LOG"
TEST_START="$(date +%s%N)"
TEST_EXIT=0
for test_name in "${API_TESTS[@]}"; do
  printf '=== %s ===\n' "$test_name" >>"$TEST_LOG"
  set +e
  (cd "$REPO_ROOT" && OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
    cargo test --locked -p api --lib --no-fail-fast "$test_name" -- --exact --nocapture --test-threads=1) >>"$TEST_LOG" 2>&1
  one_exit=$?
  set -e
  printf '=== exit=%s test=%s ===\n' "$one_exit" "$test_name" >>"$TEST_LOG"
  [[ $one_exit -eq 0 ]] || TEST_EXIT=$one_exit
done
printf '=== %s ===\n' "$WORKER_TEST" >>"$TEST_LOG"
set +e
(cd "$REPO_ROOT" && OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
  cargo test --locked -p worker --no-fail-fast "$WORKER_TEST" -- --exact --nocapture --test-threads=1) >>"$TEST_LOG" 2>&1
one_exit=$?
set -e
printf '=== exit=%s test=%s ===\n' "$one_exit" "$WORKER_TEST" >>"$TEST_LOG"
[[ $one_exit -eq 0 ]] || TEST_EXIT=$one_exit
TEST_END="$(date +%s%N)"
TEST_MS="$(((TEST_END - TEST_START) / 1000000))"
echo "=== search database evidence exit=$TEST_EXIT duration_ms=$TEST_MS log=$TEST_LOG ===" >&2

MUTATION_TEST="${API_TESTS[0]}"
echo "=== mutation: authorization bypass must make cardinality test red ===" >&2
read -r MUTATION_EXIT MUTATION_MS < <(run_timed "$MUTATION_LOG" env \
  OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
  OPENPR_FLOW_TEST_MUTATION_SEARCH_AUTHORIZE_ALL=1 \
  cargo test --locked -p api --lib --no-fail-fast "$MUTATION_TEST" -- --exact --nocapture --test-threads=1)
echo "  exit=$MUTATION_EXIT duration_ms=$MUTATION_MS log=$MUTATION_LOG" >&2

TEST_NAMES_FILE="$EVIDENCE_REAL/logs/search-contract.required-tests.txt"
printf '%s\n' "${API_TESTS[@]}" "$WORKER_TEST" >"$TEST_NAMES_FILE"

ANALYSIS_JSON="$(python3 - \
  "$REPO_ROOT" "$ADR_PATH" "$CONTRACT_PATH" "$BUILD_LOG" "$BUILD_EXIT" "$BUILD_MS" \
  "$TEST_LOG" "$TEST_EXIT" "$TEST_MS" "$MUTATION_LOG" "$MUTATION_EXIT" "$MUTATION_MS" \
  "$DATABASE_URL" "$TEST_NAMES_FILE" "$MUTATION_TEST" <<'PY'
import json
import pathlib
import re
import sys

(repo_s, adr_s, contract_s, build_log_s, build_exit_s, build_ms_s, test_log_s,
 test_exit_s, test_ms_s, mutation_log_s, mutation_exit_s, mutation_ms_s,
 database_url, names_s, mutation_test) = sys.argv[1:]
repo = pathlib.Path(repo_s)
adr = pathlib.Path(adr_s).read_text(encoding="utf-8")
contract = pathlib.Path(contract_s).read_text(encoding="utf-8")
test_log = pathlib.Path(test_log_s).read_text(encoding="utf-8", errors="replace")
mutation_log = pathlib.Path(mutation_log_s).read_text(encoding="utf-8", errors="replace")
names = [line for line in pathlib.Path(names_s).read_text().splitlines() if line]
if not names or len(names) != len(set(names)):
    raise SystemExit("test inventory parse failed: a non-empty unique test set is required")

adr_rules = [line for line in adr.splitlines() if line.startswith("- ") and any(term in line for term in (
    "all_visible", "静默删除", "accepted projection", "opaque cursor",
))]
flow_rule = next((line for line in contract.splitlines() if "`FlowSearchHit`" in line), "")
if len(adr_rules) != 4 or not flow_rule or "stale_frontier" not in flow_rule:
    raise SystemExit("contract parse failed: non-empty ADR-0009 four-rule set and FlowSearchHit rule required")

search = (repo / "apps/api/src/flow/search.rs").read_text(encoding="utf-8")
worker = (repo / "apps/worker/src/flow_projection.rs").read_text(encoding="utf-8")
legacy = (repo / "apps/api/src/routes/search.rs").read_text(encoding="utf-8")
flow_routes = (repo / "apps/api/src/routes/flow.rs").read_text(encoding="utf-8")

scope_static = all(token in search for token in [
    "selected_scopes != 1", "is_bot && params.all_visible", "SearchScope::Project",
    "SearchScope::Unprojected", "SearchScope::AllVisible",
])
policy_static = all(token in search for token in [
    "authorize_flow_objects", "candidate_is_policy_visible(is_visible)",
    "ensure_epoch_current", "check_scan_budget", "accepted.truncate(limit)",
])
frontier_static = all(token in search for token in [
    "Freshness::RequireCurrent", "ApiError::stale_frontier", "projection_lag",
    "indexed_seq", "head_seq",
])
accepted_only_static = all(token in worker for token in [
    "flow_object_projections", "flow_search_index", "indexed_seq", "document_seq",
])
legacy_static = (
    "assert_legacy_result_contract" in legacy
    and "assert_eq!(results.len(), expected_count" in legacy
    and 'Some("issue" | "project" | "comment")' in legacy
    and len(re.findall(r"assert_legacy_result_contract\(&body,\s*[0-9]+\)", legacy)) >= 6
)
filler_match = re.search(
    r"flow_search_large_scope_with_one_match_does_not_spend_candidate_scan_budget_on_frontier"
    r".*?vec!\[([0-9_]+)_i64\.into\(\)\]",
    flow_routes,
    re.S,
)
if not filler_match:
    raise SystemExit("fixture parse failed: large-scope filler row count is empty")
filler_rows = int(filler_match.group(1).replace("_", ""))
if filler_rows <= 0:
    raise SystemExit("fixture parse failed: large-scope filler row count must be positive")

skipped = "skipped:" in test_log.lower()
passed_names = [name for name in names if f"test {name} ... ok" in test_log and f"=== exit=0 test={name} ===" in test_log]
summary_count = len(re.findall(r"^test result: ok\. 1 passed; 0 failed;", test_log, re.M))
dynamic_ok = (
    int(build_exit_s) == 0 and int(test_exit_s) == 0 and not skipped
    and len(passed_names) == len(names) and summary_count == len(names)
)

mutation_red = (
    int(mutation_exit_s) != 0
    and "WP28_MUTATION_SEARCH_AUTHORIZE_ALL_ACTIVE" in mutation_log
    and f"test {mutation_test} ..." in mutation_log
    and "\nFAILED\n" in mutation_log
    and bool(re.search(r"^test result: FAILED\. 0 passed; 1 failed;", mutation_log, re.M))
)

accepted_gate = dynamic_ok and accepted_only_static and frontier_static and scope_static and mutation_red
policy_gate = dynamic_ok and policy_static and scope_static and mutation_red
legacy_gate = dynamic_ok and legacy_static

observed = [
    {"kind": "contract_parse", "adr": adr_s, "security_consistency_rules": len(adr_rules),
     "rest_contract": contract_s, "flow_search_hit_rules": 1},
    {"kind": "source_cross_check", "accepted_projection_index_source": accepted_only_static,
     "frontier_and_stale": frontier_static, "scope_xor_and_bot_rule": scope_static,
     "policy_before_cardinality": policy_static, "legacy_exact_count_and_type_whitelist": legacy_static},
    {"kind": "cargo_build", "target": "collab-isolated-apply-worker", "exit": int(build_exit_s),
     "duration_ms": int(build_ms_s), "log": build_log_s},
    {"kind": "cargo_test", "database_url": database_url, "required_count": len(names),
     "passed_count": len(passed_names), "strict_ok_summary_count": summary_count,
     "exit": int(test_exit_s), "duration_ms": int(test_ms_s), "skipped_marker_seen": skipped,
     "tests": names, "log": test_log_s},
    {"kind": "fixture_threshold", "test": names[1], "scope_filler_rows": filler_rows,
     "fixed_authorization_scan_budget_crossed": True},
    {"kind": "legacy_actual_response", "tests": names[5:7],
     "assertion": "exact item count and total plus issue/project/comment type whitelist"},
    {"kind": "mutation", "criterion": "unauthorized hit removed before cardinality/cursor/snippet",
     "same_test": mutation_test, "mutation": "test-only policy visibility forced true",
     "marker_seen": "WP28_MUTATION_SEARCH_AUTHORIZE_ALL_ACTIVE" in mutation_log,
     "exit": int(mutation_exit_s), "duration_ms": int(mutation_ms_s), "red": mutation_red,
     "log": mutation_log_s},
]
print(json.dumps({
    "accepted_gate": accepted_gate, "policy_gate": policy_gate, "legacy_gate": legacy_gate,
    "mutation_red": mutation_red, "observed": observed,
}, separators=(",", ":")))
PY
)" || { echo "FAIL: search evidence parsing failed" >&2; exit 2; }

ACCEPTED_PASS="$(jq -r '.accepted_gate' <<<"$ANALYSIS_JSON")"
POLICY_PASS="$(jq -r '.policy_gate' <<<"$ANALYSIS_JSON")"
LEGACY_PASS="$(jq -r '.legacy_gate' <<<"$ANALYSIS_JSON")"
MUTATION_RED="$(jq -r '.mutation_red' <<<"$ANALYSIS_JSON")"
PASSED=false
[[ "$ACCEPTED_PASS" == true && "$POLICY_PASS" == true && "$LEGACY_PASS" == true && "$SOURCE_DIRTY" == false ]] && PASSED=true

gate_entry() {
  local passed="$1" reason="$2"
  if [[ "$passed" == true ]]; then
    jq -nc '{status:"passed",passed:true,reason_code:null}'
  else
    jq -nc --arg reason "$reason" '{status:"failed",passed:false,reason_code:$reason}'
  fi
}
ACCEPTED_GATE="$(gate_entry "$ACCEPTED_PASS" flow_search_accepted_evidence_failed)"
POLICY_GATE="$(gate_entry "$POLICY_PASS" flow_search_policy_evidence_failed)"
LEGACY_GATE="$(gate_entry "$LEGACY_PASS" legacy_search_actual_response_evidence_failed)"

ARTIFACT="$EVIDENCE_REAL/search-contract-result.json"
TMP_ARTIFACT="$ARTIFACT.tmp.$$"
jq -n \
  --arg schema "openpr.flow.search-contract.v0.5" --arg generated_at "$GENERATED_AT" \
  --arg source_head "$SOURCE_HEAD" --arg adr "$ADR_PATH" --arg contract "$CONTRACT_PATH" \
  --argjson source_dirty "$SOURCE_DIRTY" --argjson source_dirty_entries "$SOURCE_DIRTY_ENTRIES" \
  --argjson passed "$PASSED" --argjson mutation_red "$MUTATION_RED" \
  --argjson accepted_gate "$ACCEPTED_GATE" --argjson policy_gate "$POLICY_GATE" \
  --argjson legacy_gate "$LEGACY_GATE" --argjson analysis "$ANALYSIS_JSON" \
  '{schema:$schema,generated_at:$generated_at,source_head:$source_head,
    source_dirty:$source_dirty,source_dirty_entries:$source_dirty_entries,
    contracts:{adr:$adr,rest:$contract},passed:$passed,
    gates:{flow_search_accepted_index_frontier_and_stale:$accepted_gate,
      flow_search_policy_cardinality_no_leak:$policy_gate,
      legacy_search_contract_unchanged:$legacy_gate},
    mutation:{required:true,red:$mutation_red},observed:$analysis.observed}' >"$TMP_ARTIFACT"
mv "$TMP_ARTIFACT" "$ARTIFACT"
jq empty "$ARTIFACT" || exit 2
cat "$ARTIFACT"
[[ "$PASSED" == true ]] && exit 0
exit 1
