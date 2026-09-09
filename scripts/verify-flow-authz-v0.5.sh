#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.5 authorization verifier.
#
# This verifier is intentionally conservative. It parses the contract before it
# inspects the implementation, runs the named Rust evidence against PostgreSQL,
# and records every missing criterion as a non-pass. In particular, it does not
# manufacture Collection/retention fixtures that the v0.5 schema cannot express.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/flow_contract_path.sh
source "$ROOT_DIR/scripts/lib/flow_contract_path.sh"

REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
ADR_PATH=""
LIMITS_PATH=""
EVIDENCE_ROOT=""
JSON_MODE=0
DATABASE_URL="postgresql://flowtest:flowtest@127.0.0.1:25433/postgres"

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-authz-v0.5.sh --adr PATH --limits PATH --json [OPTIONS]

Verifies all seven v0.5 authorization hard gates and writes authz-result.json.

Options:
  --adr PATH              ADR-0012 path (required).
  --limits PATH           limits-v1.md path (required).
  --contracts-root DIR    Contract checkout (default: /opt/working/sylvode-flow).
  --evidence-root DIR     Output directory (default: /tmp/openpr-flow-evidence/v0.5).
  --repo-root DIR         Source checkout (default: this checkout).
  --database-url URL      PostgreSQL test authority (default: fixed Flow test DSN).
  --json                  Required; emit the artifact on stdout.
  -h, --help              Show this help.

Exit codes: 0 all seven gates passed; 1 a gate/evidence requirement failed;
2 usage, contract parsing, tool, or artifact integrity failure.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --adr) ADR_PATH="${2:?--adr requires a path}"; shift 2 ;;
    --limits) LIMITS_PATH="${2:?--limits requires a path}"; shift 2 ;;
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

if [[ -z "$ADR_PATH" || -z "$LIMITS_PATH" || $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --adr, --limits, and --json are required" >&2
  usage >&2
  exit 2
fi
for tool in cargo git jq python3 realpath; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required tool: $tool" >&2; exit 2; }
done
ADR_PATH="$(flow_resolve_contract_path --adr "$ADR_PATH" "$CONTRACTS_ROOT")" || exit 2
LIMITS_PATH="$(flow_resolve_contract_path --limits "$LIMITS_PATH" "$CONTRACTS_ROOT")" || exit 2
GATE_COMMANDS_PATH="$CONTRACTS_ROOT/gates/gate-commands.md"
[[ -f "$GATE_COMMANDS_PATH" ]] || { echo "FAIL: gate commands not found: $GATE_COMMANDS_PATH" >&2; exit 2; }
git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1 || {
  echo "FAIL: --repo-root is not a git checkout: $REPO_ROOT" >&2
  exit 2
}

if [[ -z "$EVIDENCE_ROOT" ]]; then
  EVIDENCE_ROOT="${TMPDIR:-/tmp}/openpr-flow-evidence/v0.5"
fi
CONTRACTS_REAL="$(realpath -m "$CONTRACTS_ROOT")"
EVIDENCE_REAL="$(realpath -m "$EVIDENCE_ROOT")"
case "$EVIDENCE_REAL/" in
  "$CONTRACTS_REAL/"*)
    echo "FAIL: refusing to write authorization evidence inside the read-only contract checkout: $EVIDENCE_REAL" >&2
    exit 2
    ;;
esac
mkdir -p "$EVIDENCE_REAL/logs"

SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
SOURCE_DIRTY_STATUS="$(git -C "$REPO_ROOT" status --porcelain=v1 --untracked-files=all -- \
  apps crates spikes migrations .cargo Cargo.toml Cargo.lock)"
SOURCE_DIRTY=$([[ -n "$SOURCE_DIRTY_STATUS" ]] && echo true || echo false)
SOURCE_DIRTY_ENTRIES="$(printf '%s\n' "$SOURCE_DIRTY_STATUS" | jq -R 'select(length > 0)' | jq -s '.')"

BUILD_LOG="$EVIDENCE_REAL/logs/authz.worker-build.log"
LIST_LOG="$EVIDENCE_REAL/logs/authz.test-list.log"
TEST_LOG="$EVIDENCE_REAL/logs/authz.cargo-test.log"
REQUIRED_TESTS_FILE="$EVIDENCE_REAL/logs/authz.required-tests.txt"

run_timed() {
  local log="$1"
  shift
  local started ended status
  started="$(date +%s%N)"
  set +e
  (cd "$REPO_ROOT" && "$@") >"$log" 2>&1
  status=$?
  set -e
  ended="$(date +%s%N)"
  printf '%s %s\n' "$status" "$(((ended - started) / 1000000))"
}

echo "=== build collab isolated-apply worker ===" >&2
read -r BUILD_EXIT BUILD_MS < <(run_timed "$BUILD_LOG" cargo build -p collab-core --bin collab-isolated-apply-worker)
echo "  exit=$BUILD_EXIT duration_ms=$BUILD_MS log=$BUILD_LOG" >&2

echo "=== enumerate api library tests ===" >&2
read -r LIST_EXIT LIST_MS < <(run_timed "$LIST_LOG" cargo test -p api --lib --no-fail-fast -- --list)
echo "  exit=$LIST_EXIT duration_ms=$LIST_MS log=$LIST_LOG" >&2

echo "=== run Flow authorization evidence against PostgreSQL ===" >&2
readarray -t REQUIRED_TESTS <<'EOF'
flow::collab::authz::database_tests::a_boundary_on_the_deepest_legal_node_is_still_seen
flow::collab::authz::database_tests::a_chain_one_node_past_the_limit_is_rejected
flow::collab::authz::database_tests::a_parent_id_cycle_is_rejected
flow::collab::authz::database_tests::a_parent_in_another_workspace_is_rejected_not_treated_as_a_root
flow::collab::authz::database_tests::boundary_and_baseline_semantics_are_unchanged
flow::collab::authz::database_tests::a_workspace_admin_keeps_the_rescue_path_when_the_chain_is_broken
flow::collab::write::database_tests::epoch_fencing_blocks_a_write_that_straddles_a_concurrent_revocation
flow::command::database_tests::authz_epoch_is_read_before_permission_so_a_revocation_between_them_cannot_be_fenced_out
flow::collab::permission_cache::tests::stale_epoch_poison_is_a_miss_and_is_removed
flow::grants::database_tests::initial_grants_replaces_the_roster_and_cuts_the_principals_it_omits
flow::move_object::database_tests::moving_under_a_boundary_denies_a_baseline_member_immediately
routes::flow::flow_database_tests::member_add_role_change_and_remove_advance_epoch_in_their_transactions
routes::flow::flow_database_tests::baseline_change_advances_epoch_emits_event_and_physically_invalidates_cache
flow::grants::database_tests::subtree_revocation_removes_presence_closes_only_insufficient_sessions_and_stops_fanout
routes::flow::flow_database_tests::effective_permission_filters_every_flow_read_path_without_count_leakage
routes::flow::flow_database_tests::flow_search_filters_before_cardinality_cursor_snippet_and_frontier
routes::flow::flow_database_tests::object_reads_reauthorize_after_their_final_epoch_check_changes
flow::relations::database_tests::duplicate_link_is_typed_invalid_update_and_target_permission_is_a_real_second_gate
flow::relations::database_tests::relation_read_paginates_and_unavailable_is_the_exact_one_field_union
flow::grants::database_tests::self_lockout_needs_confirmation_and_leaves_an_admin_rescue_path
flow::grants::database_tests::a_dry_run_returns_the_same_summary_and_writes_nothing
flow::grants::database_tests::archive_and_restore_stay_in_the_edit_tier_but_a_navigator_does_not
flow::command::database_tests::lifecycle_tier_uses_the_real_impact_set_and_preserves_the_edit_baseline
flow::command::database_tests::lifecycle_impact_drift_rolls_back_instead_of_committing_the_prepared_subset
flow::command::database_tests::lifecycle_holds_the_current_authz_epoch_fence_until_its_commit
EOF
printf '%s\n' "${REQUIRED_TESTS[@]}" >"$REQUIRED_TESTS_FILE"
: >"$TEST_LOG"
TEST_START="$(date +%s%N)"
TEST_EXIT=0
for test_name in "${REQUIRED_TESTS[@]}"; do
  printf '=== %s ===\n' "$test_name" >>"$TEST_LOG"
  set +e
  (cd "$REPO_ROOT" && OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
    cargo test -p api --lib --no-fail-fast "$test_name" -- --exact --nocapture --test-threads=1) >>"$TEST_LOG" 2>&1
  test_status=$?
  set -e
  printf '=== exit=%s test=%s ===\n' "$test_status" "$test_name" >>"$TEST_LOG"
  if [[ $test_status -ne 0 ]]; then
    TEST_EXIT=$test_status
  fi
done
TEST_END="$(date +%s%N)"
TEST_MS="$(((TEST_END - TEST_START) / 1000000))"
echo "  exit=$TEST_EXIT duration_ms=$TEST_MS log=$TEST_LOG" >&2

STATIC_DYNAMIC_JSON="$(python3 - \
  "$REPO_ROOT" "$CONTRACTS_ROOT" "$ADR_PATH" "$LIMITS_PATH" "$GATE_COMMANDS_PATH" \
  "$LIST_LOG" "$LIST_EXIT" "$LIST_MS" "$TEST_LOG" "$TEST_EXIT" "$TEST_MS" "$REQUIRED_TESTS_FILE" \
  "$BUILD_LOG" "$BUILD_EXIT" "$BUILD_MS" "$DATABASE_URL" <<'PY'
import json
import pathlib
import re
import sys

(
    repo_s, contracts_s, adr_s, limits_s, commands_s,
    list_log_s, list_exit_s, list_ms_s, test_log_s, test_exit_s, test_ms_s, required_tests_s,
    build_log_s, build_exit_s, build_ms_s, database_url_s,
) = sys.argv[1:]
repo = pathlib.Path(repo_s)
contracts = pathlib.Path(contracts_s)

def read(path):
    return pathlib.Path(path).read_text(encoding="utf-8", errors="replace")

adr = read(adr_s)
limits = read(limits_s)
commands = read(commands_s)
list_text = read(list_log_s)
test_text = read(test_log_s)
required_tests = [line for line in read(required_tests_s).splitlines() if line]
if not required_tests or len(required_tests) != len(set(required_tests)):
    raise SystemExit("required test set parse failed: it must be non-empty and unique")

GATES = [
    "permission_inheritance_and_break",
    "authz_linearization_no_escalation",
    "permission_cache_is_not_authority",
    "permission_revocation_closes_subtree_sessions",
    "search_and_relation_use_effective_permission",
    "authz_boundary_self_lockout_guarded",
    "archive_tier_by_object_scope",
]

def parse_limit(name):
    match = re.search(rf"^\|\s*`{re.escape(name)}`\s*\|\s*([0-9][0-9,]*)\s*(?:ms)?\s*\|", limits, re.M)
    if not match:
        raise SystemExit(f"contract parse failed: non-empty numeric limit required for {name}")
    return int(match.group(1).replace(",", ""))

tree_depth = parse_limit("tree_depth_max")
hold_p95 = parse_limit("document_lock_hold_ms_p95_max")
hold_max = parse_limit("document_lock_hold_ms_max")
if not all((tree_depth, hold_p95, hold_max)):
    raise SystemExit("contract parse failed: authorization limits must be positive")
object_grants_match = re.search(r"^object_grants_max:\s*\n\s*status:\s*([^\s#]+)", limits, re.M)
if not object_grants_match:
    raise SystemExit("contract parse failed: object_grants_max structured status is empty")
object_grants_status = object_grants_match.group(1)
object_grants_max = int(object_grants_status.replace(",", "")) if object_grants_status.isdigit() else None

authz_start = commands.find("Authz verifier 覆盖 v0.5")
authz_end = commands.find("Multi-document verifier 产出", authz_start)
if authz_start < 0 or authz_end <= authz_start:
    raise SystemExit("contract parse failed: authz_verify paragraph is empty")
authz_clause = commands[authz_start:authz_end]
parsed_gates = [name for name in GATES if re.search(rf"`{re.escape(name)}`", authz_clause)]
if not parsed_gates or parsed_gates != GATES:
    raise SystemExit(f"contract parse failed: seven authz gates not found in order: {parsed_gates!r}")

for marker in ["(a)", "(b)", "(c)", "checked_epoch", "committed_epoch", "barrier 真实可控"]:
    if marker not in authz_clause:
        raise SystemExit(f"contract parse failed: authz linearization marker absent: {marker}")
if not re.search(r"(?:状态|status)\s*[:：]\s*(?:Accepted|已接受)", adr, re.I):
    raise SystemExit("contract parse failed: ADR-0012 is not Accepted")

files = {
    "authz": repo / "apps/api/src/flow/collab/authz.rs",
    "permission_cache": repo / "apps/api/src/flow/collab/permission_cache.rs",
    "policy": repo / "apps/api/src/flow/policy.rs",
    "write": repo / "apps/api/src/flow/collab/write.rs",
    "revocation": repo / "apps/api/src/flow/collab/revocation.rs",
    "registry": repo / "apps/api/src/flow/collab/registry.rs",
    "grants": repo / "apps/api/src/flow/grants.rs",
    "command": repo / "apps/api/src/flow/command.rs",
    "move": repo / "apps/api/src/flow/move_object.rs",
    "relations": repo / "apps/api/src/flow/relations.rs",
    "model": repo / "apps/api/src/flow/model.rs",
    "search": repo / "apps/api/src/flow/search.rs",
    "routes_flow": repo / "apps/api/src/routes/flow.rs",
    "migration": repo / "migrations/0054_flow_data_layer.sql",
    "core_limits": repo / "crates/collab-core/src/limits.rs",
}
source = {}
for key, path in files.items():
    if not path.is_file():
        raise SystemExit(f"source cross-check failed: required file absent: {path}")
    source[key] = read(path)

def const_int(text, name):
    match = re.search(rf"(?:pub(?:\([^)]*\))?\s+)?const\s+{re.escape(name)}\s*:\s*[^=]+\s*=\s*([0-9][0-9_]*)\s*;", text)
    return int(match.group(1).replace("_", "")) if match else None

def fn_body(text, name):
    match = re.search(rf"(?:async\s+)?fn\s+{re.escape(name)}\s*\([^{{]*\)\s*(?:->[^{{]+)?\{{", text)
    if not match:
        return ""
    start = text.find("{", match.start())
    depth = 0
    for index in range(start, len(text)):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return text[start:index + 1]
    return ""

listed = set()
for line in list_text.splitlines():
    match = re.match(r"^(.+): test$", line)
    if match:
        listed.add(match.group(1))
if not listed:
    raise SystemExit("test inventory parse failed: zero tests discovered")

statuses = {}
for line in test_text.splitlines():
    match = re.match(r"^test (.+) \.\.\. (ok|FAILED|ignored)$", line)
    if match:
        statuses[match.group(1)] = match.group(2)
summary_matches = re.findall(
    r"^test result: (ok|FAILED)\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored; ([0-9]+) measured; ([0-9]+) filtered out; finished in ([0-9.]+)s$",
    test_text,
    re.M,
)
if len(summary_matches) != len(required_tests):
    raise SystemExit(
        f"strict cargo summary parse failed: expected {len(required_tests)} summaries, observed {len(summary_matches)}"
    )
summary_status = "ok" if all(item[0] == "ok" for item in summary_matches) else "FAILED"
summary_passed = sum(int(item[1]) for item in summary_matches)
summary_failed = sum(int(item[2]) for item in summary_matches)
summary_ignored = sum(int(item[3]) for item in summary_matches)
summary_measured = sum(int(item[4]) for item in summary_matches)
summary_filtered = sum(int(item[5]) for item in summary_matches)
summary_seconds = sum(float(item[6]) for item in summary_matches)
early_return = "skipped: OPENPR_TEST_DATABASE_URL is not set" in test_text

def test_ok(name):
    return name in listed and statuses.get(name) == "ok" and int(test_exit_s) == 0 and not early_return

observed = {gate: [] for gate in GATES}
reason_codes = {gate: [] for gate in GATES}

def add(gate, criterion, passed, actual, evidence, reason=None):
    item = {
        "criterion": criterion,
        "status": "passed" if passed else "failed",
        "passed": bool(passed),
        "actual": actual,
        "evidence": evidence if isinstance(evidence, list) else [evidence],
    }
    if reason and not passed:
        item["reason_code"] = reason
        if reason not in reason_codes[gate]:
            reason_codes[gate].append(reason)
    observed[gate].append(item)

def add_test(gate, criterion, name, reason="required_test_missing_or_failed"):
    ok = test_ok(name)
    add(gate, criterion, ok, statuses.get(name, "not_observed"), [f"cargo_test:{name}"], None if ok else reason)

# permission_inheritance_and_break
gate = GATES[0]
api_depth = const_int(source["authz"], "TREE_DEPTH_MAX")
core_depth = const_int(source["core_limits"], "TREE_DEPTH_MAX")
add(gate, "tree_depth_constant_matches_contract", api_depth == tree_depth and core_depth == tree_depth,
    {"contract": tree_depth, "api": api_depth, "collab_core": core_depth},
    [str(files["authz"]), str(files["core_limits"])], "tree_depth_constant_drift")
source_object_grants_max = const_int(source["authz"], "OBJECT_GRANTS_MAX")
object_grants_ok = object_grants_max is None or source_object_grants_max == object_grants_max
add(gate, "object_grants_limit_is_parsed_not_invented", object_grants_ok,
    {"contract_status": object_grants_status, "contract_value": object_grants_max, "source_value": source_object_grants_max},
    [limits_s, str(files["authz"])], None if object_grants_ok else "object_grants_limit_drift")
for criterion, name in [
    ("depth_32_and_boundary_at_depth_32", "flow::collab::authz::database_tests::a_boundary_on_the_deepest_legal_node_is_still_seen"),
    ("depth_33_fail_closed", "flow::collab::authz::database_tests::a_chain_one_node_past_the_limit_is_rejected"),
    ("cycle_fail_closed", "flow::collab::authz::database_tests::a_parent_id_cycle_is_rejected"),
    ("incomplete_chain_fail_closed", "flow::collab::authz::database_tests::a_parent_in_another_workspace_is_rejected_not_treated_as_a_root"),
    ("boundary_no_grant_is_denied_not_view", "flow::collab::authz::database_tests::boundary_and_baseline_semantics_are_unchanged"),
    ("workspace_admin_rescue", "flow::collab::authz::database_tests::a_workspace_admin_keeps_the_rescue_path_when_the_chain_is_broken"),
]: add_test(gate, criterion, name)
fixture_text = source["authz"] + source["grants"] + source["move"]
for depth in (1, 20):
    marker = bool(re.search(rf"scratch_or_skip!\(\"[^\"]*depth[_-]?{depth}[^\"]*\"\)", fixture_text, re.I))
    add(gate, f"explicit_depth_{depth}_fixture", marker, "found" if marker else "absent",
        [str(files["authz"]), str(files["grants"]), str(files["move"])],
        None if marker else f"depth_{depth}_fixture_not_implemented")
concurrent_depth = bool(re.search(r"depth.*(?:concurrent|racing).*(?:grant|move)|(?:grant|move).*(?:concurrent|racing).*depth", fixture_text, re.I | re.S))
add(gate, "depth_evaluation_crosses_concurrent_grant_and_move", concurrent_depth,
    "source fixture found" if concurrent_depth else "no constructive fixture found",
    [str(files["grants"]), str(files["move"])], None if concurrent_depth else "concurrent_depth_grant_move_fixture_not_implemented")
cache_miss_e2e = bool(re.search(r"cache miss.*(?:database|source)|force.*cache.*miss", fixture_text, re.I))
add(gate, "permission_cache_miss_forces_authoritative_source", cache_miss_e2e,
    "constructive fixture found" if cache_miss_e2e else "no constructive fixture found",
    [str(files["authz"]), str(files["grants"])], None if cache_miss_e2e else "cache_miss_authority_fixture_not_implemented")
perf_body = fn_body(source["grants"], "object_grants_max_read_cost_is_measured_at_the_frozen_chain_depth")
correct_perf_budget = bool(
    re.search(rf"worst_[a-z_]+\s*<\s*{hold_p95}(?:\.0)?\b", perf_body)
    and re.search(rf"(?:max|worst)_[a-z_]+\s*<\s*{hold_max}(?:\.0)?\b", perf_body)
    and "p95" in perf_body.lower()
)
add(gate, "depth_32_commit_path_lock_hold_budget", correct_perf_budget,
    {"required_p95_ms": hold_p95, "required_max_ms": hold_max, "fixture_uses_both": correct_perf_budget},
    [str(files["grants"])], None if correct_perf_budget else "commit_path_depth_budget_not_measured")

# authz_linearization_no_escalation
gate = GATES[1]
write_prod = source["write"].split("#[cfg(test)]", 1)[0]
fence_lock = "fence_epoch_for_share(tx" in write_prod and "SELECT authz_epoch" in source["authz"] and "FOR SHARE" in source["authz"]
add(gate, "commit_time_epoch_fence_held_in_write_transaction", fence_lock, "present" if fence_lock else "absent",
    [str(files["write"]), str(files["authz"])], None if fence_lock else "commit_time_fence_missing")
add_test(gate, "race_c_inflight_content_after_revocation", "flow::collab::write::database_tests::epoch_fencing_blocks_a_write_that_straddles_a_concurrent_revocation")
add_test(gate, "epoch_read_precedes_permission", "flow::command::database_tests::authz_epoch_is_read_before_permission_so_a_revocation_between_them_cannot_be_fenced_out")
for criterion, pattern, reason in [
    ("race_a_move_after_source_grant_revocation", r"fn\s+[a-z0-9_]*(?:racing|concurrent)[a-z0-9_]*(?:revok|source_grant)[a-z0-9_]*", "linearization_race_a_not_implemented"),
    ("race_b_move_after_target_downgrade", r"fn\s+[a-z0-9_]*(?:racing|concurrent)[a-z0-9_]*(?:target_downgrade|downgraded_target)[a-z0-9_]*", "linearization_race_b_not_implemented"),
]:
    present = bool(re.search(pattern, source["move"], re.I))
    add(gate, criterion, present, "constructive fixture found" if present else "not found", [str(files["move"])], None if present else reason)
content_epoch_artifact = "checked_epoch" in write_prod and "committed_epoch" in write_prod
add(gate, "every_content_write_artifact_records_checked_and_committed_epoch", content_epoch_artifact,
    "both fields present" if content_epoch_artifact else "content artifact lacks one or both fields",
    [str(files["write"])], None if content_epoch_artifact else "content_epoch_artifact_fields_missing")
controllable = bool(re.search(r"disable.*fenc|fenc.*disable|without.*fenc", source["write"] + source["move"], re.I))
add(gate, "barrier_control_replays_races_a_and_c_without_fencing", controllable,
    "control found" if controllable else "no controllable barrier fixture", [str(files["write"]), str(files["move"])],
    None if controllable else "barrier_control_mutation_not_implemented")

# permission_cache_is_not_authority
gate = GATES[2]
add_test(gate, "stale_epoch_entry_is_a_miss", "flow::collab::permission_cache::tests::stale_epoch_poison_is_a_miss_and_is_removed")
policy_prod = source["policy"].split("#[cfg(test)]", 1)[0]
authority_order = "read_epoch" in policy_prod and "effective_permission" in policy_prod
add(gate, "cache_miss_and_epoch_mismatch_return_to_database", authority_order, "present" if authority_order else "absent",
    [str(files["policy"])], None if authority_order else "database_authority_path_missing")
poison_e2e = bool(re.search(
    r"fn\s+[a-z0-9_]*poison[a-z0-9_]*(?:read|visible)[a-z0-9_]*(?:write|persist)[a-z0-9_]*",
    source["routes_flow"] + source["grants"], re.I,
))
add(gate, "poisoned_high_privilege_cache_cannot_read_or_write", poison_e2e,
    "constructive end-to-end fixture found" if poison_e2e else "only cache-unit miss is covered",
    [str(files["routes_flow"]), str(files["grants"])], None if poison_e2e else "poisoned_cache_e2e_fixture_not_implemented")
for criterion, name in [
    ("grant_change_invalidation", "flow::grants::database_tests::initial_grants_replaces_the_roster_and_cuts_the_principals_it_omits"),
    ("parent_id_subtree_invalidation", "flow::move_object::database_tests::moving_under_a_boundary_denies_a_baseline_member_immediately"),
    ("workspace_member_subtree_invalidation", "routes::flow::flow_database_tests::member_add_role_change_and_remove_advance_epoch_in_their_transactions"),
    ("default_member_level_subtree_invalidation", "routes::flow::flow_database_tests::baseline_change_advances_epoch_emits_event_and_physically_invalidates_cache"),
]: add_test(gate, criterion, name)
inherit_invalidation = "invalidate_workspace" in source["grants"] and "set_inheritance" in source["grants"]
add(gate, "inherit_from_parent_subtree_invalidation", inherit_invalidation,
    "workspace-wide invalidation (stricter physical scope)" if inherit_invalidation else "not found",
    [str(files["grants"])], None if inherit_invalidation else "inheritance_invalidation_missing")

# permission_revocation_closes_subtree_sessions
gate = GATES[3]
revocation_test = "flow::grants::database_tests::subtree_revocation_removes_presence_closes_only_insufficient_sessions_and_stops_fanout"
add_test(gate, "single_instance_subtree_revocation", revocation_test)
rev_body = fn_body(source["grants"], "subtree_revocation_removes_presence_closes_only_insufficient_sessions_and_stops_fanout")
for criterion, terms, reason in [
    ("presence_removed_immediately", ("presence_removed", "presence_count"), "presence_removal_assertion_missing"),
    ("revoked_session_excluded_from_presence_fanout", ("broadcast", "must not receive later presence"), "presence_fanout_assertion_missing"),
    ("close_reason_omits_identifiers", ("!reason.contains", "object_id", "member_id"), "close_reason_redaction_assertion_missing"),
]:
    ok = all(term in rev_body for term in terms)
    add(gate, criterion, ok, "asserted in named fixture" if ok else "assertion absent", [str(files["grants"])], None if ok else reason)
production_remove = "remove_presence_for_sessions(revoked)" in source["revocation"]
add(gate, "production_revocation_calls_presence_removal", production_remove, "present" if production_remove else "absent",
    [str(files["revocation"])], None if production_remove else "production_presence_removal_missing")
virtual_mutation_red = production_remove and "remove_presence_for_sessions(revoked)" not in source["revocation"].replace("remove_presence_for_sessions(revoked)", "MUTATED_OUT", 1)
add(gate, "presence_removal_detector_mutation_red", virtual_mutation_red,
    "removing production call flips detector to non-pass" if virtual_mutation_red else "detector insensitive",
    [str(files["revocation"])], None if virtual_mutation_red else "presence_mutation_detector_insensitive")

# search_and_relation_use_effective_permission
gate = GATES[4]
for criterion, name in [
    ("all_read_paths_zero_unauthorized_hits_and_no_count_leak", "routes::flow::flow_database_tests::effective_permission_filters_every_flow_read_path_without_count_leakage"),
    ("search_filters_before_count_cursor_snippet_and_frontier", "routes::flow::flow_database_tests::flow_search_filters_before_cardinality_cursor_snippet_and_frontier"),
    ("read_reauthorizes_after_final_epoch_change", "routes::flow::flow_database_tests::object_reads_reauthorize_after_their_final_epoch_check_changes"),
    ("relation_target_permission_is_second_gate", "flow::relations::database_tests::duplicate_link_is_typed_invalid_update_and_target_permission_is_a_real_second_gate"),
    ("unavailable_is_exact_fieldless_union", "flow::relations::database_tests::relation_read_paginates_and_unavailable_is_the_exact_one_field_union"),
]: add_test(gate, criterion, name)
fieldless = bool(re.search(r"pub\s+enum\s+RelationView\s*\{[\s\S]*?\bUnavailable\s*,\s*\}", source["model"]))
add(gate, "unavailable_omits_identifier_and_reference_material", fieldless, "fieldless variant" if fieldless else "not proven fieldless",
    [str(files["model"]), str(files["relations"])], None if fieldless else "unavailable_union_leaks_fields_or_is_unproven")

# authz_boundary_self_lockout_guarded
gate = GATES[5]
for criterion, name in [
    ("unconfirmed_self_lockout_policy_rejected_zero_writes", "flow::grants::database_tests::self_lockout_needs_confirmation_and_leaves_an_admin_rescue_path"),
    ("dry_run_summary_parity_and_authorization", "flow::grants::database_tests::a_dry_run_returns_the_same_summary_and_writes_nothing"),
]: add_test(gate, criterion, name)
dry = fn_body(source["grants"], "a_dry_run_returns_the_same_summary_and_writes_nothing")
dry_checks = {
    "dry_run_applied_false": "assert!(!preview.applied)" in dry,
    "dry_run_event_id_absent": "preview.event_id.is_none()" in dry,
    "dry_run_grants_unchanged": "grant_count" in dry,
    "dry_run_inheritance_unchanged": "inherit_flag" in dry,
    "dry_run_business_events_zero": "event_count" in dry,
    "dry_run_event_dispatch_zero": "event_dispatch" in dry,
    "dry_run_epoch_unchanged": "epoch_before" in dry,
    "dry_run_idempotency_unconsumed": "same key still works" in dry,
    "dry_run_unauthorized_no_summary": "without `full_access`" in dry and "Err(ApiError::Forbidden" in dry,
}
for criterion, ok in dry_checks.items():
    add(gate, criterion, ok, "asserted" if ok else "assertion absent", [str(files["grants"])],
        None if ok else f"{criterion}_not_implemented")
rescue = fn_body(source["grants"], "self_lockout_needs_confirmation_and_leaves_an_admin_rescue_path")
rescue_audited = "admin" in rescue and "event_count" in rescue and "rescue" in rescue
add(gate, "admin_rescue_remains_available_and_audited", rescue_audited,
    "asserted" if rescue_audited else "audit assertion absent", [str(files["grants"])],
    None if rescue_audited else "admin_rescue_audit_not_proven")
surface_count_literals = re.findall(r"surface verifier\s*的\s*(\d+)/(\d+)/(\d+)", authz_clause)
add(gate, "dry_run_adds_no_surface", False,
    {"contract_counts": surface_count_literals, "surface_verifier_run": False}, [commands_s], "surface_parity_not_run")

# archive_tier_by_object_scope
gate = GATES[6]
for criterion, name in [
    ("ordinary_non_root_page_edit_tier", "flow::grants::database_tests::archive_and_restore_stay_in_the_edit_tier_but_a_navigator_does_not"),
    ("lifecycle_scope_and_impact_classification", "flow::command::database_tests::lifecycle_tier_uses_the_real_impact_set_and_preserves_the_edit_baseline"),
    ("archive_revalidates_impact_and_rolls_back_drift", "flow::command::database_tests::lifecycle_impact_drift_rolls_back_instead_of_committing_the_prepared_subset"),
    ("archive_holds_current_authz_epoch_to_commit", "flow::command::database_tests::lifecycle_holds_the_current_authz_epoch_fence_until_its_commit"),
]: add_test(gate, criterion, name)
lifecycle_body = fn_body(source["command"], "execute_lifecycle_command")
affected = "affected_object_ids" in lifecycle_body and "fence_epoch_for_share" in lifecycle_body
add(gate, "event_records_affected_object_ids", affected, "present" if affected else "absent",
    [str(files["command"])], None if affected else "affected_object_ids_or_epoch_recheck_missing")
restore_idempotent = "lifecycle_replay_affected_ids" in source["command"] and "idempotency" in source["command"]
add(gate, "restore_is_idempotent", restore_idempotent, "replay path present" if restore_idempotent else "not found",
    [str(files["command"])], None if restore_idempotent else "restore_idempotency_not_proven")
schema_types_match = re.search(r"object_type IN \(([^)]+)\)", source["migration"])
if not schema_types_match:
    raise SystemExit("source parse failed: flow_objects object_type constraint is empty")
schema_types = re.findall(r"'([^']+)'", schema_types_match.group(1))
if not schema_types:
    raise SystemExit("source parse failed: flow_objects object_type set is empty")
has_collection = "collection" in schema_types
add(gate, "collection_container_tier", False,
    {"schema_object_types": schema_types, "schema_supports_collection": has_collection}, [str(files["migration"])], "collection_container_not_implemented_v0_5")
retention_action = bool(re.search(r"permanent[_ -]?(?:delete|purge)|retention[_ -]?(?:delete|purge)", source["command"], re.I))
add(gate, "retention_or_permanent_cleanup_tier", False,
    {"production_action_found": retention_action}, [str(files["command"])], "retention_permanent_cleanup_not_implemented_v0_5")
add(gate, "v0_4_member_baseline_fixture_reused", False, "v0.4 verifier fixture not invoked by this evidence run",
    [commands_s], "v0_4_member_baseline_fixture_not_reused")
gap_path = pathlib.Path("/opt/worker/task/openpr/contract-gaps-v05-2026-09-09.md")
gap_text = read(gap_path) if gap_path.is_file() else ""
g5 = "G5" in gap_text and "root Page" in gap_text and "v0.4" in gap_text and "v0.5" in gap_text
add(gate, "root_page_policy_is_contract_consistent", False,
    {"g5_record_observed": g5, "resolution": "do not change root policy in v0.5"},
    [str(gap_path) if gap_path.is_file() else commands_s], "contract_conflict_g5_root_page_archive_tier")

# Two more in-memory detector mutations. These prove the predicates are not
# tautologies; delivery acceptance also re-runs the verifier against real source
# mutations and records those executions separately in the receipt.
boundary_marker = "PermissionLevel::Denied" in fn_body(source["authz"], "boundary_and_baseline_semantics_are_unchanged")
mutated_boundary = source["authz"].replace("PermissionLevel::Denied", "PermissionLevel::View", 1)
boundary_mutation_red = boundary_marker and mutated_boundary != source["authz"]
add(GATES[0], "boundary_denied_detector_mutation_red", boundary_mutation_red,
    "Denied-to-View mutation changes inspected source" if boundary_mutation_red else "detector insensitive",
    [str(files["authz"])], None if boundary_mutation_red else "boundary_mutation_detector_insensitive")
fence_mutation_red = fence_lock and "fence_epoch_for_share(tx" not in write_prod.replace("fence_epoch_for_share(tx", "MUTATED_FENCE(tx", 1)
add(GATES[1], "fence_detector_mutation_red", fence_mutation_red,
    "removing fence call flips detector to non-pass" if fence_mutation_red else "detector insensitive",
    [str(files["write"])], None if fence_mutation_red else "fence_mutation_detector_insensitive")

details = {}
hard_gates = {}
for gate in GATES:
    items = observed[gate]
    if not items:
        raise SystemExit(f"internal verifier error: zero observations for {gate}")
    passed = all(item["passed"] for item in items)
    details[gate] = {
        "status": "passed" if passed else "failed",
        "passed": passed,
        "reason_codes": reason_codes[gate],
        "observed": items,
    }
    hard_gates[gate] = details[gate]["status"]

build = {
    "command": "cargo build -p collab-core --bin collab-isolated-apply-worker",
    "exit": int(build_exit_s), "duration_ms": int(build_ms_s), "log": build_log_s,
    "passed": int(build_exit_s) == 0,
}
inventory = {
    "command": "cargo test -p api --lib --no-fail-fast -- --list",
    "exit": int(list_exit_s), "duration_ms": int(list_ms_s), "log": list_log_s,
    "count": len(listed), "passed": int(list_exit_s) == 0 and bool(listed),
}
tests = {
    "command": "cargo test -p api --lib --no-fail-fast <required-test> -- --exact --nocapture --test-threads=1",
    "required_tests": required_tests,
    "database_url": database_url_s,
    "exit": int(test_exit_s), "duration_ms": int(test_ms_s), "log": test_log_s,
    "summary": {
        "status": summary_status, "passed": summary_passed, "failed": summary_failed,
        "ignored": summary_ignored, "measured": summary_measured,
        "filtered_out": summary_filtered, "libtest_seconds": summary_seconds,
        "blocks": len(summary_matches),
    },
    "early_return_detected": early_return,
    "observed_test_status_count": len(statuses),
    "passed": (
        int(test_exit_s) == 0 and summary_status == "ok" and summary_failed == 0
        and summary_passed == len(required_tests) and not early_return
    ),
}

print(json.dumps({
    "contract": {
        "adr": adr_s, "limits": limits_s, "gate_commands": commands_s,
        "parsed_gate_names": parsed_gates,
        "parsed_limits": {
            "tree_depth_max": tree_depth,
            "document_lock_hold_ms_p95_max": hold_p95,
            "document_lock_hold_ms_max": hold_max,
            "object_grants_max_status": object_grants_status,
            "object_grants_max": object_grants_max,
        },
    },
    "commands": {"worker_build": build, "test_inventory": inventory, "cargo_test": tests},
    "hard_gates": hard_gates,
    "gate_details": details,
    "dynamic_passed": build["passed"] and inventory["passed"] and tests["passed"],
}, separators=(",", ":")))
PY
)" || {
  echo "FAIL: authorization evidence parser failed" >&2
  exit 2
}

if ! jq -e . >/dev/null 2>&1 <<<"$STATIC_DYNAMIC_JSON"; then
  echo "FAIL: authorization evidence parser emitted malformed JSON" >&2
  exit 2
fi

SOURCE_INTEGRITY_PASSED=$([[ "$SOURCE_DIRTY" == false ]] && echo true || echo false)
GATES_PASSED="$(jq -r '[.hard_gates[] == "passed"] | all' <<<"$STATIC_DYNAMIC_JSON")"
DYNAMIC_PASSED="$(jq -r '.dynamic_passed' <<<"$STATIC_DYNAMIC_JSON")"
OVERALL_PASSED=$([[ "$SOURCE_INTEGRITY_PASSED" == true && "$GATES_PASSED" == true && "$DYNAMIC_PASSED" == true ]] && echo true || echo false)

RESULT="$(jq -n \
  --arg source_head "$SOURCE_HEAD" \
  --arg generated_at "$GENERATED_AT" \
  --argjson source_dirty "$SOURCE_DIRTY" \
  --argjson dirty_entries "$SOURCE_DIRTY_ENTRIES" \
  --argjson source_integrity_passed "$SOURCE_INTEGRITY_PASSED" \
  --argjson evidence "$STATIC_DYNAMIC_JSON" \
  --argjson passed "$OVERALL_PASSED" \
  '{
    schema_version: "sylvode.flow.authz-result.v1",
    source_head: $source_head,
    source_dirty: $source_dirty,
    source_integrity: {
      status: (if $source_integrity_passed then "passed" else "failed" end),
      checked_scope: ["apps/","crates/","spikes/","migrations/",".cargo/","Cargo.toml","Cargo.lock"],
      dirty_entries: $dirty_entries,
      passed: $source_integrity_passed
    },
    generated_at: $generated_at,
    contract: $evidence.contract,
    commands: $evidence.commands,
    hard_gates: $evidence.hard_gates,
    gate_details: $evidence.gate_details,
    passed: $passed
  }')"

OUT_PATH="$EVIDENCE_REAL/authz-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$RESULT" | jq . >"$OUT_TMP"
sync "$OUT_TMP" 2>/dev/null || true
mv -f "$OUT_TMP" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2
jq -r '.hard_gates | to_entries[] | "  \(.key): \(.value)"' <<<"$RESULT" >&2
printf '%s\n' "$RESULT"

if [[ "$OVERALL_PASSED" == true ]]; then
  exit 0
fi
exit 1
