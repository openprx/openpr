#!/usr/bin/env bash
set -euo pipefail

# v0.5 Flow event registry, audit-origin and causation verifier. The accepted
# contract's explicit origin-header residual trust gap is reported as a failure,
# never hidden by otherwise-passing transport tests.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/flow_contract_path.sh
source "$ROOT_DIR/scripts/lib/flow_contract_path.sh"

REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
CONTRACT_PATH=""
EVIDENCE_ROOT="${TMPDIR:-/tmp}/openpr-flow-evidence/v0.5"
DATABASE_URL="postgresql://flowtest:flowtest@127.0.0.1:25433/postgres"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-audit-causation-v0.5.sh --contract PATH --json [OPTIONS]

Options:
  --contract PATH         events-v1.md (required).
  --contracts-root DIR    Contract checkout.
  --evidence-root DIR     Output directory.
  --repo-root DIR         Source checkout.
  --database-url URL      PostgreSQL test authority.
  --json                  Required; print the artifact.
  -h, --help              Show this help.

Exit codes: 0 all gates passed; 1 a gate failed/not implemented; 2 malformed evidence.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
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

if [[ -z "$CONTRACT_PATH" || $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --contract and --json are required" >&2
  usage >&2
  exit 2
fi
for tool in cargo git jq python3 realpath; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required tool: $tool" >&2; exit 2; }
done
CONTRACT_PATH="$(flow_resolve_contract_path --contract "$CONTRACT_PATH" "$CONTRACTS_ROOT")" || exit 2
GATE_COMMANDS="$CONTRACTS_ROOT/gates/gate-commands.md"
[[ -f "$GATE_COMMANDS" ]] || { echo "FAIL: missing gate-commands.md" >&2; exit 2; }
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

BUILD_LOG="$EVIDENCE_REAL/logs/audit-causation.worker-build.log"
TEST_LOG="$EVIDENCE_REAL/logs/audit-causation.cargo-test.log"
MUTATION_LOG="$EVIDENCE_REAL/logs/audit-causation.mutation.log"
TEST_NAMES_FILE="$EVIDENCE_REAL/logs/audit-causation.required-tests.txt"

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

readarray -t TESTS <<'EOF'
flow::event_policy::tests::every_emitted_flow_event_type_declares_a_payload_policy
events::database_tests::same_idempotency_key_replay_returns_the_original_event_id_and_appends_no_second_dispatch_row
events::database_tests::business_dispatch_row_carries_document_id_and_accepted_seq_for_content_accepted
flow::relations::database_tests::link_and_unlink_are_real_zero_document_writes_with_exact_events_and_replay
flow::move_object::database_tests::a_cross_project_move_cascades_the_whole_subtree_into_the_target_navigator
flow::move_object::database_tests::a_moves_derived_content_events_share_its_correlation_and_name_it_as_their_causation
routes::flow::flow_database_tests::a_rejected_command_is_audited_for_a_bot_exactly_as_it_is_for_a_user
routes::flow::flow_database_tests::the_bot_behind_an_event_is_recoverable_through_the_request_id_the_middleware_minted
routes::flow::flow_database_tests::the_same_route_records_the_transport_it_was_reached_over_and_one_request_id_per_request
routes::flow::flow_database_tests::every_event_a_rest_request_writes_carries_the_rest_surface_and_a_server_request_id
EOF
printf '%s\n' "${TESTS[@]}" >"$TEST_NAMES_FILE"
: >"$TEST_LOG"
TEST_START="$(date +%s%N)"
TEST_EXIT=0
for test_name in "${TESTS[@]}"; do
  printf '=== %s ===\n' "$test_name" >>"$TEST_LOG"
  set +e
  (cd "$REPO_ROOT" && OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
    cargo test --locked -p api --lib --no-fail-fast "$test_name" -- --exact --nocapture --test-threads=1) >>"$TEST_LOG" 2>&1
  one_exit=$?
  set -e
  printf '=== exit=%s test=%s ===\n' "$one_exit" "$test_name" >>"$TEST_LOG"
  [[ $one_exit -eq 0 ]] || TEST_EXIT=$one_exit
done
TEST_END="$(date +%s%N)"
TEST_MS="$(((TEST_END - TEST_START) / 1000000))"
echo "=== audit/causation tests exit=$TEST_EXIT duration_ms=$TEST_MS log=$TEST_LOG ===" >&2

MUTATION_TEST="routes::flow::flow_database_tests::the_same_route_records_the_transport_it_was_reached_over_and_one_request_id_per_request"
echo "=== mutation: force every serialized event source to REST ===" >&2
read -r MUTATION_EXIT MUTATION_MS < <(run_timed "$MUTATION_LOG" env \
  OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
  OPENPR_FLOW_TEST_MUTATION_EVENT_SOURCE_FORCE_REST=1 \
  cargo test --locked -p api --lib --no-fail-fast "$MUTATION_TEST" -- --exact --nocapture --test-threads=1)
echo "  exit=$MUTATION_EXIT duration_ms=$MUTATION_MS log=$MUTATION_LOG" >&2

ANALYSIS_JSON="$(python3 - \
  "$REPO_ROOT" "$CONTRACT_PATH" "$GATE_COMMANDS" "$BUILD_LOG" "$BUILD_EXIT" "$BUILD_MS" \
  "$TEST_LOG" "$TEST_EXIT" "$TEST_MS" "$DATABASE_URL" "$TEST_NAMES_FILE" \
  "$MUTATION_LOG" "$MUTATION_EXIT" "$MUTATION_MS" "$MUTATION_TEST" <<'PY'
import collections
import json
import pathlib
import re
import sys

(repo_s, contract_s, commands_s, build_log_s, build_exit_s, build_ms_s,
 test_log_s, test_exit_s, test_ms_s, database_url, names_s,
 mutation_log_s, mutation_exit_s, mutation_ms_s, mutation_test) = sys.argv[1:]
repo = pathlib.Path(repo_s)
contract = pathlib.Path(contract_s).read_text(encoding="utf-8")
commands = pathlib.Path(commands_s).read_text(encoding="utf-8")
test_log = pathlib.Path(test_log_s).read_text(encoding="utf-8", errors="replace")
mutation_log = pathlib.Path(mutation_log_s).read_text(encoding="utf-8", errors="replace")
tests = [line for line in pathlib.Path(names_s).read_text().splitlines() if line]
if not tests or len(tests) != len(set(tests)):
    raise SystemExit("test inventory parse failed: non-empty unique list required")

registry_section = contract.split("## Event type registry", 1)[1].split("## 投递", 1)[0]
registry_rows = []
registry_first_release = {}
for line in registry_section.splitlines():
    match = re.match(r"^\| `(flow\.[a-z0-9_]+\.[a-z0-9_]+)` \| (0\.[0-9]+)(?: conditional.*)? \|", line)
    if match:
        event_type, release = match.groups()
        registry_first_release[event_type] = release
        if tuple(map(int, release.split("."))) <= (0, 5):
            registry_rows.append(event_type)
if not registry_rows:
    raise SystemExit("event contract parse failed: v0.4/v0.5 registry is empty")
registry_counts = collections.Counter(registry_rows)

flow_root = repo / "apps/api/src/flow"
producer_files = []
producer_sites = collections.defaultdict(list)
for path in sorted(flow_root.rglob("*.rs")):
    raw = path.read_text(encoding="utf-8").split("\n#[cfg(test)]", 1)[0]
    without_blocks = re.sub(r"/\*.*?\*/", "", raw, flags=re.S)
    code = re.sub(r"(?m)//.*$", "", without_blocks)
    if not re.search(r"\binsert_flow_event(?:_with_id)?\s*\(", code):
        continue
    producer_files.append(str(path.relative_to(repo)))
    for line_no, line in enumerate(without_blocks.splitlines(), 1):
        if "detected_by" in line:
            continue
        line = line.split("//", 1)[0]
        for event_type in re.findall(r'"(flow\.[a-z0-9_]+\.[a-z0-9_]+)"', line):
            producer_sites[event_type].append({"file": str(path.relative_to(repo)), "line": line_no})
producer_types = sorted(producer_sites)
if not producer_files or not producer_types:
    raise SystemExit("producer scan failed: files and literals must be non-empty")

# A v0.5 predecessor gate may be rerun on a later source head. Producers whose
# registry row explicitly says they first ship after v0.5 are outside this
# gate; unknown producer types remain in scope and therefore fail closed.
v05_producer_types = sorted(
    name for name in producer_types
    if name not in registry_first_release
    or tuple(map(int, registry_first_release[name].split("."))) <= (0, 5)
)
later_release_producer_types = sorted(set(producer_types) - set(v05_producer_types))

policy_source = (flow_root / "event_policy.rs").read_text(encoding="utf-8").split("\n#[cfg(test)]", 1)[0]
policy_rows = []
pattern = r'\(\s*"(flow\.[a-z0-9_]+\.[a-z0-9_]+)"\s*,\s*public_payload\(&\[(.*?)\]\)\s*,?\s*\)'
for match in re.finditer(pattern, policy_source, re.S):
    keys = re.findall(r'"([a-z0-9_]+)"', match.group(2))
    policy_rows.append((match.group(1), keys))
if not policy_rows:
    raise SystemExit("payload policy parse failed: policy table is empty")
policy_counts = collections.Counter(name for name, _ in policy_rows)
policy_keys = {name: keys for name, keys in policy_rows}

missing_contract = sorted(name for name in v05_producer_types if registry_counts[name] != 1)
missing_policy = sorted(name for name in v05_producer_types if policy_counts[name] != 1 or not policy_keys.get(name))
forbidden_policy_keys = sorted({
    key for name in v05_producer_types for key in policy_keys.get(name, [])
    if key in {"title", "body", "text", "snapshot", "update", "token", "peer_id", "properties"}
})
registry_pass = not missing_contract and not missing_policy and not forbidden_policy_keys

origin_gap = all(fragment in contract for fragment in [
    "surface 实际来自 **HTTP 头**", "没有关闭", "不得声称已解决",
])
metadata_contract = all(fragment in contract for fragment in [
    "before_frontier?", "after_frontier?", "semantic_summary",
])
write_source = (flow_root / "collab/write.rs").read_text(encoding="utf-8").split("\n#[cfg(test)]", 1)[0]
content_start = write_source.find('event_type: "flow.content.accepted"')
content_end = write_source.find("correlation_id:", content_start)
content_event = write_source[content_start:content_end] if content_start >= 0 and content_end > content_start else ""
content_metadata_fields = sorted(set(re.findall(r'"([a-z_]+)"\s*:', content_event.split("metadata:", 1)[-1])))
metadata_complete = metadata_contract and all(
    required in content_metadata_fields for required in ("before_frontier", "after_frontier", "semantic_summary")
)

events_mod = (repo / "apps/api/src/events/mod.rs").read_text(encoding="utf-8")
dispatch_static = all(fragment in events_mod for fragment in [
    "if was_new && let Some(spec) = dispatch", "INSERT INTO event_dispatch",
    "ON CONFLICT (workspace_id, idempotency_key)",
])
affected_dynamic_requirement = all(fragment in commands for fragment in [
    "affected_object_ids[]", "moved object + descendants", "navigator",
])
move_source = (flow_root / "move_object.rs").read_text(encoding="utf-8")
affected_static = all(fragment in move_source for fragment in [
    '"affected_object_ids": subtree_ids', "fn subtree_ids(&self)", "ids.sort_unstable()",
    "cascaded_node_count", "insert_flow_event_with_id",
])
move_event_start = move_source.find("event_type: GovernanceCommandType::MoveObject")
move_event_end = move_source.find("correlation_id:", move_event_start)
move_event_block = move_source[move_event_start:move_event_end] if move_event_start >= 0 and move_event_end > move_event_start else ""
payload_duplicate_count = '"cascaded_node_count"' in move_event_block
route_source = (repo / "apps/api/src/routes/flow.rs").read_text(encoding="utf-8")
route_start = route_source.find("the_same_route_records_the_transport_it_was_reached_over_and_one_request_id_per_request")
route_end = route_source.find("async fn every_event_a_rest_request", route_start)
route_test_source = route_source[route_start:route_end] if route_start >= 0 and route_end > route_start else ""
transport_literals = sorted(set(re.findall(r'"(rest|mcp_http|mcp_sse|mcp_stdio)"', route_test_source)))
if not transport_literals:
    raise SystemExit("transport fixture parse failed: same-route transport set is empty")

skipped = "skipped:" in test_log.lower()
passed_tests = [name for name in tests if f"test {name} ... ok" in test_log and f"=== exit=0 test={name} ===" in test_log]
summary_count = len(re.findall(r"^test result: ok\. 1 passed; 0 failed;", test_log, re.M))
dynamic_pass = (
    int(build_exit_s) == 0 and int(test_exit_s) == 0 and not skipped
    and len(passed_tests) == len(tests) and summary_count == len(tests)
)

mutation_red = (
    int(mutation_exit_s) != 0
    and "WP28_MUTATION_EVENT_SOURCE_FORCE_REST_ACTIVE" in mutation_log
    and f"test {mutation_test} ..." in mutation_log
    and "\nFAILED\n" in mutation_log
    and bool(re.search(r"^test result: FAILED\. 0 passed; 1 failed;", mutation_log, re.M))
)

# The first gate cannot pass while the contract itself records caller-controlled
# transport labeling, or while the canonical content event omits its required
# audit summary/frontier fields. Passing runtime surface propagation proves only
# the implemented half of the contract.
audit_gate = dynamic_pass and mutation_red and not origin_gap and metadata_complete
relation_move_gate = (
    dynamic_pass and mutation_red and registry_pass and dispatch_static
    and affected_dynamic_requirement and affected_static and not payload_duplicate_count
)

observed = [
    {"kind": "contract_registry_parse", "path": contract_s, "v04_v05_rows": len(registry_rows),
     "unique_rows": len(registry_counts), "parse_nonempty": True},
    {"kind": "producer_literal_scan", "producer_files": producer_files,
     "producer_type_count": len(producer_types), "producer_types": producer_types,
     "sites": producer_sites},
    {"kind": "payload_policy_cross_check", "policy_row_count": len(policy_rows),
     "v0_5_scoped_producer_types": v05_producer_types,
     "later_release_producer_types_excluded": later_release_producer_types,
     "missing_or_duplicate_contract_rows": missing_contract,
     "missing_empty_or_duplicate_payload_policies": missing_policy,
     "forbidden_policy_keys": forbidden_policy_keys, "passed": registry_pass},
    {"kind": "audit_metadata_cross_check", "content_event_metadata_fields": content_metadata_fields,
     "required_fields": ["before_frontier", "after_frontier", "semantic_summary"],
     "complete": metadata_complete,
     "reason_code": None if metadata_complete else "audit_metadata_envelope_incomplete"},
    {"kind": "known_contract_gap", "id": "origin_header_trust", "present": origin_gap,
     "reason_code": "origin_transport_header_not_credential_bound",
     "detail": "allow-listed transport label remains caller-controlled and workspace_bots has no bound surface"},
    {"kind": "dispatch_source_cross_check", "centralized_exactly_one_path": dispatch_static},
    {"kind": "affected_set_cross_check", "dynamic_requirement_parsed": affected_dynamic_requirement,
     "sorted_subtree_envelope_source": affected_static,
     "payload_has_duplicate_count": payload_duplicate_count},
    {"kind": "cargo_build", "target": "collab-isolated-apply-worker", "exit": int(build_exit_s),
     "duration_ms": int(build_ms_s), "log": build_log_s},
    {"kind": "cargo_test", "database_url": database_url, "required_count": len(tests),
     "passed_count": len(passed_tests), "strict_ok_summary_count": summary_count,
     "exit": int(test_exit_s), "duration_ms": int(test_ms_s), "skipped_marker_seen": skipped,
     "tests": tests, "log": test_log_s},
    {"kind": "fixture_threshold", "test": mutation_test,
     "transport_count": len(transport_literals), "transports": transport_literals,
     "same_request_multi_event": "REST leg's two events" in route_test_source},
    {"kind": "mutation", "criterion": "same route preserves distinct server-resolved transport origins",
     "same_test": mutation_test, "mutation": "test-only EventSource serialization forces REST",
     "exit": int(mutation_exit_s), "duration_ms": int(mutation_ms_s), "red": mutation_red,
     "log": mutation_log_s},
]
print(json.dumps({
    "audit_gate": audit_gate, "relation_move_gate": relation_move_gate,
    "origin_gap": origin_gap, "metadata_complete": metadata_complete,
    "mutation_red": mutation_red, "observed": observed,
}, separators=(",", ":")))
PY
)" || { echo "FAIL: audit/causation evidence parsing failed" >&2; exit 2; }

AUDIT_PASS="$(jq -r '.audit_gate' <<<"$ANALYSIS_JSON")"
RELATION_MOVE_PASS="$(jq -r '.relation_move_gate' <<<"$ANALYSIS_JSON")"
ORIGIN_GAP="$(jq -r '.origin_gap' <<<"$ANALYSIS_JSON")"
METADATA_COMPLETE="$(jq -r '.metadata_complete' <<<"$ANALYSIS_JSON")"
MUTATION_RED="$(jq -r '.mutation_red' <<<"$ANALYSIS_JSON")"

if [[ "$AUDIT_PASS" == true ]]; then
  AUDIT_GATE='{"status":"passed","passed":true,"reason_code":null,"blocking_reasons":[]}'
else
  BLOCKING_REASONS='[]'
  [[ "$ORIGIN_GAP" == false ]] || BLOCKING_REASONS="$(jq -c '. + ["origin_transport_header_not_credential_bound"]' <<<"$BLOCKING_REASONS")"
  [[ "$METADATA_COMPLETE" == true ]] || BLOCKING_REASONS="$(jq -c '. + ["audit_metadata_envelope_incomplete"]' <<<"$BLOCKING_REASONS")"
  [[ "$MUTATION_RED" == true ]] || BLOCKING_REASONS="$(jq -c '. + ["audit_origin_mutation_not_red"]' <<<"$BLOCKING_REASONS")"
  AUDIT_GATE="$(jq -nc --argjson reasons "$BLOCKING_REASONS" '{status:"failed",passed:false,reason_code:($reasons[0] // "audit_causation_evidence_failed"),blocking_reasons:$reasons}')"
fi
if [[ "$RELATION_MOVE_PASS" == true ]]; then
  RELATION_MOVE_GATE='{"status":"passed","passed":true,"reason_code":null}'
else
  RELATION_MOVE_GATE='{"status":"failed","passed":false,"reason_code":"relation_move_registry_transport_causation_evidence_failed"}'
fi
PASSED=false
[[ "$AUDIT_PASS" == true && "$RELATION_MOVE_PASS" == true && "$SOURCE_DIRTY" == false ]] && PASSED=true

ARTIFACT="$EVIDENCE_REAL/audit-causation-result.json"
TMP_ARTIFACT="$ARTIFACT.tmp.$$"
jq -n --arg schema "openpr.flow.audit-causation.v0.5" --arg generated_at "$GENERATED_AT" \
  --arg source_head "$SOURCE_HEAD" --arg contract "$CONTRACT_PATH" \
  --argjson source_dirty "$SOURCE_DIRTY" --argjson source_dirty_entries "$SOURCE_DIRTY_ENTRIES" \
  --argjson passed "$PASSED" --argjson mutation_red "$MUTATION_RED" \
  --argjson audit_gate "$AUDIT_GATE" --argjson relation_move_gate "$RELATION_MOVE_GATE" \
  --argjson analysis "$ANALYSIS_JSON" \
  '{schema:$schema,generated_at:$generated_at,source_head:$source_head,
    source_dirty:$source_dirty,source_dirty_entries:$source_dirty_entries,
    contract:$contract,passed:$passed,
    gates:{audit_actor_origin_causation:$audit_gate,
      flow_relation_move_event_registry_and_transport_causation:$relation_move_gate},
    mutation:{required:true,red:$mutation_red},observed:$analysis.observed}' >"$TMP_ARTIFACT"
mv "$TMP_ARTIFACT" "$ARTIFACT"
jq empty "$ARTIFACT" || exit 2
cat "$ARTIFACT"
[[ "$PASSED" == true ]] && exit 0
exit 1
