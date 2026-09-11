#!/usr/bin/env bash
set -euo pipefail

# v0.5 relation-read policy verifier. ADR-0018 RF-1 requires every production
# fetch_relation_batch caller to fetch an unfiltered candidate stream, perform
# per-target authorization, and only then apply caller direction/type filters.
# The frozen caller inventory below fails closed when a new call site appears.

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
Usage: scripts/verify-flow-relation-policy-v0.5.sh --contract PATH --json [OPTIONS]

Options:
  --contract PATH         rest-api-v1.md (required).
  --contracts-root DIR    Contract checkout (default: /opt/working/sylvode-flow).
  --evidence-root DIR     Output directory (default: /tmp/openpr-flow-evidence/v0.5).
  --repo-root DIR         Source checkout (default: this checkout).
  --database-url URL      PostgreSQL test authority.
  --json                  Required; print the artifact.
  -h, --help              Show this help.

Exit codes: 0 all gates passed; 1 a gate is failed/not implemented; 2 malformed evidence.
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
git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1 || {
  echo "FAIL: --repo-root is not a git checkout: $REPO_ROOT" >&2
  exit 2
}

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

BUILD_LOG="$EVIDENCE_REAL/logs/relation-policy.worker-build.log"
TEST_LOG="$EVIDENCE_REAL/logs/relation-policy.cargo-test.log"
MUTATION_LOG="$EVIDENCE_REAL/logs/relation-policy.mutation.log"

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
flow::relations::database_tests::relation_read_paginates_and_unavailable_is_the_exact_one_field_union
flow::relations::database_tests::filtered_relation_view_drops_unauthorized_rows_before_type_or_direction_matching
flow::relations::database_tests::corrupted_cross_workspace_relation_fails_closed_and_records_integrity
EOF
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
echo "=== relation database evidence exit=$TEST_EXIT duration_ms=$TEST_MS log=$TEST_LOG ===" >&2

MUTATION_TEST="${TESTS[0]}"
echo "=== mutation: plaintext relation cursor must make the same test red ===" >&2
read -r MUTATION_EXIT MUTATION_MS < <(run_timed "$MUTATION_LOG" env \
  OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
  OPENPR_FLOW_TEST_MUTATION_RELATION_CURSOR_PLAINTEXT=1 \
  cargo test --locked -p api --lib --no-fail-fast "$MUTATION_TEST" -- --exact --nocapture --test-threads=1)
echo "  exit=$MUTATION_EXIT duration_ms=$MUTATION_MS log=$MUTATION_LOG" >&2

ANALYSIS_JSON="$(python3 - \
  "$REPO_ROOT" "$CONTRACT_PATH" "$BUILD_LOG" "$BUILD_EXIT" "$BUILD_MS" \
  "$TEST_LOG" "$TEST_EXIT" "$TEST_MS" "$MUTATION_LOG" "$MUTATION_EXIT" "$MUTATION_MS" \
  "$DATABASE_URL" "${TESTS[0]}" "${TESTS[1]}" "${TESTS[2]}" <<'PY'
import json
import pathlib
import re
import sys

(repo_s, contract_s, build_log_s, build_exit_s, build_ms_s, test_log_s, test_exit_s,
 test_ms_s, mutation_log_s, mutation_exit_s, mutation_ms_s, database_url,
 page_test, filter_test, corruption_test) = sys.argv[1:]
repo = pathlib.Path(repo_s)
contract = pathlib.Path(contract_s).read_text(encoding="utf-8")
relations = (repo / "apps/api/src/flow/relations.rs").read_text(encoding="utf-8")
model = (repo / "apps/api/src/flow/model.rs").read_text(encoding="utf-8")
test_log = pathlib.Path(test_log_s).read_text(encoding="utf-8", errors="replace")
mutation_log = pathlib.Path(mutation_log_s).read_text(encoding="utf-8", errors="replace")

contract_line = next((line for line in contract.splitlines() if "`RelationView`" in line), "")
corruption_line = next((line for line in contract.splitlines() if "跨 workspace relation" in line), "")
if (
    not contract_line
    or "{visibility:\"unavailable\"}" not in contract_line
    or not corruption_line
    or "invalid_update" not in corruption_line
    or "integrity alert" not in corruption_line
):
    raise SystemExit("contract parse failed: non-empty RelationView leak/fail-closed rule required")

def exact_test_passed(name):
    return (
        f"test {name} ... ok" in test_log
        and f"=== exit=0 test={name} ===" in test_log
    )

skipped = "skipped:" in test_log.lower()
page_pass = exact_test_passed(page_test)
filter_pass = exact_test_passed(filter_test)
corruption_pass = exact_test_passed(corruption_test)
summary_count = len(re.findall(r"^test result: ok\. 1 passed; 0 failed;", test_log, re.M))

fieldless_union = bool(re.search(
    r'enum\s+RelationView\s*\{.*?Unavailable\s*,', model, re.S
)) and '#[serde(tag = "visibility", rename_all = "snake_case")]' in model
per_target_reauth = (
    "authorize_flow_objects" in relations
    and "batch.into_iter().zip(visible)" in relations
    and "RelationView::Unavailable" in relations
    and "ensure_epoch_current" in relations
)
cursor_crypto = all(token in relations for token in [
    "CHACHA20_POLY1305", "seal_in_place_append_tag", "open_in_place", "CURSOR_AAD",
    "decode_cursor(state.cfg.jwt_secret.expose(), &cursor)",
])
corruption_static = all(token in relations for token in [
    "record_relation_integrity", "cross_workspace_relation", "invalid_update",
])

def matching_delimiter(source, open_at, opener, closer):
    depth = 0
    for index in range(open_at, len(source)):
        token = source[index]
        if token == opener:
            depth += 1
        elif token == closer:
            depth -= 1
            if depth == 0:
                return index
    return None

def split_top_level_arguments(raw):
    arguments = []
    start = 0
    depths = {"(": 0, "[": 0, "{": 0, "<": 0}
    pairs = {")": "(", "]": "[", "}": "{", ">": "<"}
    for index, token in enumerate(raw):
        if token in depths:
            depths[token] += 1
        elif token in pairs and depths[pairs[token]] > 0:
            depths[pairs[token]] -= 1
        elif token == "," and all(depth == 0 for depth in depths.values()):
            arguments.append(raw[start:index].strip())
            start = index + 1
    tail = raw[start:].strip()
    if tail:
        arguments.append(tail)
    return arguments

production_relations = relations.split("\n#[cfg(test)]", 1)[0]
function_ranges = []
for match in re.finditer(r"(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)[^\{]*\{", production_relations):
    close_at = matching_delimiter(production_relations, match.end() - 1, "{", "}")
    if close_at is not None:
        function_ranges.append((match.group(1), match.start(), close_at + 1))

fetch_calls = []
for match in re.finditer(r"\bfetch_relation_batch\s*\(", production_relations):
    prefix = production_relations[max(0, match.start() - 32):match.start()]
    if re.search(r"async\s+fn\s+$", prefix):
        continue
    close_at = matching_delimiter(production_relations, match.end() - 1, "(", ")")
    containing = [item for item in function_ranges if item[1] <= match.start() < item[2]]
    caller = min(containing, key=lambda item: item[2] - item[1])[0] if containing else None
    arguments = (
        split_top_level_arguments(production_relations[match.end():close_at])
        if close_at is not None else []
    )
    fetch_calls.append({
        "file": "apps/api/src/flow/relations.rs",
        "caller": caller,
        "line": production_relations.count("\n", 0, match.start()) + 1,
        "arguments": arguments,
    })

expected_callers = [{"file": "apps/api/src/flow/relations.rs", "caller": "list_relations"}]
discovered_callers = [{"file": call["file"], "caller": call["caller"]} for call in fetch_calls]
caller_inventory_frozen = discovered_callers == expected_callers
unfiltered_fetch_arguments = (
    caller_inventory_frozen
    and len(fetch_calls[0]["arguments"]) == 6
    and fetch_calls[0]["arguments"][2] == "RelationDirection::Both"
    and fetch_calls[0]["arguments"][3] == "None"
)

list_range = next((item for item in function_ranges if item[0] == "list_relations"), None)
list_body = production_relations[list_range[1]:list_range[2]] if list_range else ""
ordered_fragments = [
    "fetch_relation_batch(",
    "policy::authorize_flow_objects",
    "batch.into_iter().zip(visible)",
    "if !is_visible",
    "let direction_matches",
    "let type_matches",
]
ordered_positions = [list_body.find(fragment) for fragment in ordered_fragments]
post_authorization_filtering = (
    all(position >= 0 for position in ordered_positions)
    and ordered_positions == sorted(ordered_positions)
    and "params.direction" in list_body[ordered_positions[4]:]
    and bool(re.search(r"params\s*\.\s*relation_type", list_body[ordered_positions[5]:]))
)
g4_callsite_oracle_pass = (
    per_target_reauth
    and caller_inventory_frozen
    and unfiltered_fetch_arguments
    and post_authorization_filtering
)

mutation_red = (
    int(mutation_exit_s) != 0
    and "WP28_MUTATION_RELATION_CURSOR_PLAINTEXT_ACTIVE" in mutation_log
    and "the server must authenticate and decrypt the opaque relation cursor" in mutation_log
    and f"test {page_test} ..." in mutation_log
    and "\nFAILED\n" in mutation_log
    and bool(re.search(r"^test result: FAILED\. 0 passed; 1 failed;", mutation_log, re.M))
)

static_ok = fieldless_union and per_target_reauth and cursor_crypto and corruption_static
dynamic_ok = int(build_exit_s) == 0 and int(test_exit_s) == 0 and not skipped and summary_count == 3
cross_pass = static_ok and dynamic_ok and corruption_pass and mutation_red
pagination_mechanics = static_ok and dynamic_ok and page_pass and filter_pass and mutation_red

observed = [
    {"kind": "contract_parse", "path": contract_s, "relation_view_rule_count": 1,
     "unavailable_exact_union": True, "cross_workspace_invalid_update": True},
    {"kind": "source_cross_check", "fieldless_unavailable_variant": fieldless_union,
     "per_target_reauthorization": per_target_reauth, "authenticated_cursor_round_trip": cursor_crypto,
     "cross_workspace_integrity_path": corruption_static},
    {"kind": "cargo_build", "target": "collab-isolated-apply-worker", "exit": int(build_exit_s),
     "duration_ms": int(build_ms_s), "log": build_log_s},
    {"kind": "cargo_test", "database_url": database_url,
     "required_tests": [page_test, filter_test, corruption_test],
     "exact_pass_summaries": summary_count, "exit": int(test_exit_s), "duration_ms": int(test_ms_s),
     "skipped_marker_seen": skipped, "log": test_log_s},
    {"kind": "g4_callsite_oracle", "id": "G4", "passed": g4_callsite_oracle_pass,
     "expected_callers": expected_callers, "discovered_calls": fetch_calls,
     "caller_inventory_frozen": caller_inventory_frozen,
     "unfiltered_fetch_arguments": unfiltered_fetch_arguments,
     "post_authorization_filtering": post_authorization_filtering,
     "ordered_fragments": ordered_fragments, "ordered_positions": ordered_positions,
     "reason_code": None if g4_callsite_oracle_pass else "relation_filter_oracle_g4",
     "detail": "all frozen production callers fetch Both/None before per-target authorization and caller filtering"},
    {"kind": "mutation", "criterion": "authenticated opaque next_cursor", "same_test": page_test,
     "mutation": "test-only cursor encoder emits base64 plaintext", "marker_seen":
     "WP28_MUTATION_RELATION_CURSOR_PLAINTEXT_ACTIVE" in mutation_log,
     "exit": int(mutation_exit_s), "duration_ms": int(mutation_ms_s), "red": mutation_red,
     "log": mutation_log_s},
]

print(json.dumps({
    "static_ok": static_ok,
    "dynamic_ok": dynamic_ok,
    "pagination_mechanics": pagination_mechanics,
    "g4_callsite_oracle_pass": g4_callsite_oracle_pass,
    "cross_pass": cross_pass,
    "mutation_red": mutation_red,
    "observed": observed,
}, separators=(",", ":")))
PY
)" || { echo "FAIL: relation evidence parsing failed" >&2; exit 2; }

PAGINATION_MECHANICS="$(jq -r '.pagination_mechanics' <<<"$ANALYSIS_JSON")"
G4_CALLSITE_ORACLE_PASS="$(jq -r '.g4_callsite_oracle_pass' <<<"$ANALYSIS_JSON")"
CROSS_PASS="$(jq -r '.cross_pass' <<<"$ANALYSIS_JSON")"
MUTATION_RED="$(jq -r '.mutation_red' <<<"$ANALYSIS_JSON")"

if [[ "$PAGINATION_MECHANICS" == true && "$G4_CALLSITE_ORACLE_PASS" == true ]]; then
  PAGINATION_STATUS="passed"
  PAGINATION_REASON=""
elif [[ "$PAGINATION_MECHANICS" == true ]]; then
  PAGINATION_STATUS="failed"
  PAGINATION_REASON="relation_filter_oracle_g4"
else
  PAGINATION_STATUS="failed"
  PAGINATION_REASON="relation_pagination_evidence_failed"
fi
if [[ "$CROSS_PASS" == true ]]; then
  CROSS_STATUS="passed"
  CROSS_REASON=""
else
  CROSS_STATUS="failed"
  CROSS_REASON="cross_workspace_fail_closed_evidence_failed"
fi

PASSED=false
[[ "$PAGINATION_STATUS" == passed && "$CROSS_STATUS" == passed && "$SOURCE_DIRTY" == false ]] && PASSED=true

ARTIFACT="$EVIDENCE_REAL/relation-policy-result.json"
TMP_ARTIFACT="$ARTIFACT.tmp.$$"
jq -n \
  --arg schema "openpr.flow.relation-policy.v0.5" \
  --arg generated_at "$GENERATED_AT" \
  --arg source_head "$SOURCE_HEAD" \
  --arg contract "$CONTRACT_PATH" \
  --arg pagination_status "$PAGINATION_STATUS" \
  --arg pagination_reason "$PAGINATION_REASON" \
  --arg cross_status "$CROSS_STATUS" \
  --arg cross_reason "$CROSS_REASON" \
  --argjson source_dirty "$SOURCE_DIRTY" \
  --argjson source_dirty_entries "$SOURCE_DIRTY_ENTRIES" \
  --argjson passed "$PASSED" \
  --argjson mutation_red "$MUTATION_RED" \
  --argjson analysis "$ANALYSIS_JSON" \
  '{schema:$schema,generated_at:$generated_at,source_head:$source_head,
    source_dirty:$source_dirty,source_dirty_entries:$source_dirty_entries,
    contract:$contract,passed:$passed,
    gates:{
      relation_pagination_reauthorization_no_leak:{status:$pagination_status,passed:($pagination_status=="passed"),reason_code:(if $pagination_reason=="" then null else $pagination_reason end)},
      cross_workspace_relation_fail_closed:{status:$cross_status,passed:($cross_status=="passed"),reason_code:(if $cross_reason=="" then null else $cross_reason end)}
    },
    mutation:{required:true,red:$mutation_red},observed:$analysis.observed}' >"$TMP_ARTIFACT"
mv "$TMP_ARTIFACT" "$ARTIFACT"

jq empty "$ARTIFACT" || exit 2
cat "$ARTIFACT"
[[ "$PASSED" == true ]] && exit 0
exit 1
