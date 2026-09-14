#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.9"
PACKAGE_SCHEMA=
SOURCE_WORKSPACE=
FRESH_TARGET=0
COMPARE=
JSON_MODE=0
while (($#)); do
  case "$1" in
    --package-schema) PACKAGE_SCHEMA=${2:?}; shift 2 ;;
    --source-workspace) SOURCE_WORKSPACE=${2:?}; shift 2 ;;
    --fresh-target-workspace) FRESH_TARGET=1; shift ;;
    --compare) COMPARE=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $PACKAGE_SCHEMA == v1 ]] || { echo 'FAIL: --package-schema must be v1' >&2; exit 2; }
[[ $SOURCE_WORKSPACE == FIXTURE ]] || { echo 'FAIL: --source-workspace must be FIXTURE' >&2; exit 2; }
[[ $FRESH_TARGET -eq 1 ]] || { echo 'FAIL: --fresh-target-workspace is required' >&2; exit 2; }
[[ $COMPARE == objects,documents,relations,lineage,semantic_hash,import_report ]] || {
  echo 'FAIL: --compare must be objects,documents,relations,lineage,semantic_hash,import_report' >&2
  exit 2
}
[[ $JSON_MODE -eq 1 ]] || { echo 'FAIL: --json is required' >&2; exit 2; }

DATABASE_URL=${OPENPR_TEST_DATABASE_URL:-postgresql://flowtest:flowtest@127.0.0.1:25433/postgres}
CACHE_ROOT=/opt/worker/.cache/openpr-v09-export-roundtrip
LOG_ROOT="$CACHE_ROOT/logs"
mkdir -p "$LOG_ROOT" "$EVIDENCE_ROOT"

cleanup_scratch() {
  local names
  names=$(PGPASSWORD=flowtest psql -h 127.0.0.1 -p 25433 -U flowtest -d postgres -Atc \
    "SELECT datname FROM pg_database WHERE datname LIKE 'openpr_v09_roundtrip_%' ORDER BY datname" 2>/dev/null || true)
  if [[ -n $names ]]; then
    while IFS= read -r name; do
      PGPASSWORD=flowtest dropdb -h 127.0.0.1 -p 25433 -U flowtest --force "$name" >/dev/null
    done <<<"$names"
  fi
}
trap cleanup_scratch EXIT
cleanup_scratch

ROUNDTRIP_LOG="$LOG_ROOT/roundtrip-green.log"
NEGATIVE_LOG="$LOG_ROOT/package-negative-controls.log"
env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="$DATABASE_URL" CARGO_BUILD_JOBS=4 \
  cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p mcp-server \
    --test flow_workspace_roundtrip_real -- --nocapture >"$ROUNDTRIP_LOG" 2>&1

: >"$NEGATIVE_LOG"
for test_name in \
  flow::package::tests::package_hash_member_hash_and_checksum_file_are_independent_fail_closed_checks \
  flow::package_import::tests::flow_package_import_preview_writes_no_canonical_state_and_commit_remaps_exact_document_heads \
  flow::package_import::tests::flow_package_import_promotion_fault_rolls_back_every_canonical_row_and_completion_event
do
  env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="$DATABASE_URL" CARGO_BUILD_JOBS=4 \
    cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api --lib "$test_name" -- --exact --nocapture \
      >>"$NEGATIVE_LOG" 2>&1
done
env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="$DATABASE_URL" CARGO_BUILD_JOBS=4 \
  cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api --lib \
    routes::flow::flow_database_tests::flow_package_import_wire_limits_accept_exact_boundary_and_reject_plus_one_with_zero_writes \
    -- --exact --nocapture >>"$NEGATIVE_LOG" 2>&1

python3 - "$REPO_ROOT" "$EVIDENCE_ROOT" "$ROUNDTRIP_LOG" "$NEGATIVE_LOG" <<'PY'
import copy
import datetime as dt
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile

repo, evidence, roundtrip_log, negative_log = map(pathlib.Path, sys.argv[1:])
text = roundtrip_log.read_text(errors="replace")
rows = [line.split("FLOW_V09_ROUNDTRIP_RESULT=", 1)[1] for line in text.splitlines()
        if line.startswith("FLOW_V09_ROUNDTRIP_RESULT=")]
trace = json.loads(rows[0]) if len(rows) == 1 else {}

def valid(value):
    documents = value.get("documents_enumerated", 0)
    comparisons = value.get("comparison", {})
    branches = {row.get("conflict_policy"): row for row in value.get("branches", [])}
    reject = branches.get("reject_existing", {})
    reuse = branches.get("reuse_import_lineage", {})
    parent_edges = value.get("parent_edges", [])
    source_to_target = {row.get("source_object_id"): row.get("target_object_id") for row in parent_edges}
    child_edges = [row for row in parent_edges if row.get("source_parent_id") is not None]
    parent_graph_ok = bool(
        len(parent_edges) == documents
        and len(child_edges) == 3
        and len({row.get("source_parent_id") for row in child_edges}) == 3
        and sum(row.get("source_parent_id") is None for row in parent_edges) == 1
        and all(row.get("expected_target_parent_id") == source_to_target.get(row.get("source_parent_id"))
                and row.get("target_parent_id") == row.get("expected_target_parent_id")
                for row in child_edges)
    )
    return bool(
        value.get("status") == "passed"
        and value.get("package_schema") == "v1"
        and documents > 0
        and value.get("documents_compared") == documents
        and value.get("objects_compared") == documents
        and value.get("relations_compared", 0) > 0
        and value.get("lineage_rows_compared") == documents * 2 + value.get("relations_compared")
        and all(comparisons.get(key) is True for key in (
            "head_seq", "head_frontier", "semantic_hash", "projection_seq", "projection_frontier",
            "object_metadata", "parent_graph", "relation_graph", "lineage", "import_report"))
        and parent_graph_ok
        and value.get("mcp_import_chain") == ["objects.import_artifact", "objects.import_preview", "objects.import_commit", "objects.import_status"]
        and [row.get("ordinal") for row in value.get("command_trace", [])] == list(range(1, 8))
        and reject.get("existing_document_cardinality") in (0, 1)
        and reject.get("canonical_writes") == documents
        and reuse.get("existing_document_cardinality") == 0
        and reuse.get("canonical_writes") == 0
        and reuse.get("head_changes") == 0
        and reuse.get("before") == reuse.get("after")
        and re.fullmatch(r"[0-9a-f]{64}", value.get("report_checksum", "")) is not None
    )

missing_document = copy.deepcopy(trace)
missing_document["documents_compared"] = max(0, missing_document.get("documents_compared", 0) - 1)
reuse_write = copy.deepcopy(trace)
for branch in reuse_write.get("branches", []):
    if branch.get("conflict_policy") == "reuse_import_lineage":
        branch["canonical_writes"] = 1
drop_all_parents = copy.deepcopy(trace)
for edge in drop_all_parents.get("parent_edges", []):
    edge["source_parent_id"] = None
    edge["expected_target_parent_id"] = None
    edge["target_parent_id"] = None
wrong_parent = copy.deepcopy(trace)
wrong_parent_edges = [edge for edge in wrong_parent.get("parent_edges", []) if edge.get("source_parent_id") is not None]
if wrong_parent_edges:
    wrong_parent_edges[0]["target_parent_id"] = next(
        edge.get("target_object_id") for edge in wrong_parent.get("parent_edges", [])
        if edge.get("target_object_id") != wrong_parent_edges[0].get("expected_target_parent_id")
    )
mutations = {
    "missing_document_comparison": {"red": not valid(missing_document)},
    "reuse_performs_canonical_write": {"red": not valid(reuse_write)},
    "drop_all_parent_edges": {"red": not valid(drop_all_parents)},
    "wrong_parent_mapping": {"red": not valid(wrong_parent)},
}

negative = negative_log.read_text(errors="replace")
summaries = re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;", negative, re.M)
negative_executed = sum(int(passed) + int(failed) for _, passed, failed, _ in summaries)
negative_ok = negative_executed == 4 and all(state == "ok" and failed == "0" and ignored == "0"
                                                for state, _, failed, ignored in summaries)
negative_names = {
    "checksum_and_member_mismatch": "package_hash_member_hash_and_checksum_file_are_independent_fail_closed_checks",
    "permission_and_duplicate_import": "flow_package_import_preview_writes_no_canonical_state_and_commit_remaps_exact_document_heads",
    "promotion_interruption_zero_canonical_change": "flow_package_import_promotion_fault_rolls_back_every_canonical_row_and_completion_event",
    "wire_limit_zero_canonical_change": "flow_package_import_wire_limits_accept_exact_boundary_and_reject_plus_one_with_zero_writes",
}
negative_controls = {name: test_name in negative for name, test_name in negative_names.items()}
passed = valid(trace) and negative_ok and all(row["red"] for row in mutations.values())
passed = passed and all(negative_controls.values())
result = {
    "schema_version": "sylvode.flow.export-roundtrip-result.v1",
    "release": "0.9.0",
    "source_head": subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip(),
    "source_dirty": bool(subprocess.check_output(["git", "-C", str(repo), "status", "--porcelain=v1"], text=True).splitlines()),
    "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
    "executed_count": 5,
    "roundtrip": trace,
    "negative_controls": {
        **negative_controls,
        "executed_count": negative_executed,
    },
    "mutation_controls": mutations,
    "logs": {
        "roundtrip": str(roundtrip_log),
        "roundtrip_sha256": hashlib.sha256(roundtrip_log.read_bytes()).hexdigest(),
        "negative": str(negative_log),
        "negative_sha256": hashlib.sha256(negative_log.read_bytes()).hexdigest(),
    },
    "passed": passed,
}
fd, temporary = tempfile.mkstemp(prefix=".export-roundtrip-result.", dir=evidence)
with os.fdopen(fd, "w") as handle:
    json.dump(result, handle, sort_keys=True, indent=2)
    handle.write("\n")
os.replace(temporary, evidence / "export-roundtrip-result.json")
print(json.dumps(result, sort_keys=True))
raise SystemExit(0 if passed else 1)
PY
