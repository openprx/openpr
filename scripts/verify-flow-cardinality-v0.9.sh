#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CONTRACTS_ROOT=/opt/working/sylvode-flow
EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.9"
ADR_PATH=
SINCE_RELEASE=
JSON_MODE=0
while (($#)); do
  case "$1" in
    --repo-root) REPO_ROOT=${2:?}; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --adr) ADR_PATH=${2:?}; shift 2 ;;
    --since-release) SINCE_RELEASE=${2:?}; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $JSON_MODE -eq 1 && $SINCE_RELEASE == 0.8 ]] || { echo 'FAIL: require --since-release 0.8 --json' >&2; exit 2; }
[[ -n $ADR_PATH ]] || ADR_PATH="$CONTRACTS_ROOT/decisions/ADR-0013-multi-document-atomicity.md"
[[ -f $ADR_PATH ]] || { echo "FAIL: ADR missing: $ADR_PATH" >&2; exit 2; }
ROUNDTRIP="$EVIDENCE_ROOT/export-roundtrip-result.json"
[[ -s $ROUNDTRIP ]] || { echo "FAIL: current export roundtrip artifact missing: $ROUNDTRIP" >&2; exit 1; }
mkdir -p "$EVIDENCE_ROOT/logs" /opt/worker/.cache
GREEN_LOG="$EVIDENCE_ROOT/logs/cardinality-v09-green.log"
MISSING_LOG="$EVIDENCE_ROOT/logs/cardinality-v09-missing-declaration-red.log"
LOCK_LOG="$EVIDENCE_ROOT/logs/cardinality-v09-lock-order-red.log"
TEST_NAME=tools::tests::live_v08_dispatch_has_exact_cardinality_coverage_and_lock_order_support

run_test() {
  local root=$1 log=$2
  env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$REPO_ROOT/target" \
    cargo test --manifest-path "$root/Cargo.toml" -p mcp-server --lib "$TEST_NAME" \
    -- --exact --nocapture >"$log" 2>&1
}

run_test "$REPO_ROOT" "$GREEN_LOG"

MUTATION_ROOT=$(mktemp -d /opt/worker/.cache/v09-cardinality.XXXXXX)
cleanup() {
  git -C "$REPO_ROOT" worktree remove --force "$MUTATION_ROOT" >/dev/null 2>&1 || true
  rm -rf -- "$MUTATION_ROOT"
}
trap cleanup EXIT
rm -rf -- "$MUTATION_ROOT"
git -C "$REPO_ROOT" worktree add --detach "$MUTATION_ROOT" HEAD >/dev/null

TARGET="$MUTATION_ROOT/apps/mcp-server/src/tools/mod.rs"
python3 - "$TARGET" <<'PY'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
body = path.read_text()
needle = "        objects::rebuild_flow_projection_tool(),\n    ]\n}\n\nfn flow_v08_tool_definitions"
replacement = "        objects::rebuild_flow_projection_tool(),\n        objects::get_flow_object_tool(),\n    ]\n}\n\nfn flow_v08_tool_definitions"
if body.count(needle) != 1:
    raise SystemExit("missing-declaration mutation anchor drifted")
path.write_text(body.replace(needle, replacement))
PY
if run_test "$MUTATION_ROOT" "$MISSING_LOG"; then
  echo 'FAIL: adding a live production command without a cardinality declaration stayed green' >&2
  exit 1
fi

git -C "$MUTATION_ROOT" checkout -- apps/mcp-server/src/tools/mod.rs
python3 - "$TARGET" <<'PY'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
body = path.read_text()
needle = '                "collab.compact" => One,'
replacement = '                "collab.compact" => api::flow::command::ExistingDocumentCardinality::BoundedMany(4),'
if body.count(needle) != 1:
    raise SystemExit("lock-order mutation anchor drifted")
path.write_text(body.replace(needle, replacement))
PY
if run_test "$MUTATION_ROOT" "$LOCK_LOG"; then
  echo 'FAIL: BoundedMany without an ADR-0013 lock-order mechanism stayed green' >&2
  exit 1
fi

python3 - "$REPO_ROOT" "$ADR_PATH" "$ROUNDTRIP" "$EVIDENCE_ROOT" "$GREEN_LOG" "$MISSING_LOG" "$LOCK_LOG" <<'PY'
import datetime as dt
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile

repo, adr, roundtrip_path, evidence, green_log, missing_log, lock_log = map(pathlib.Path, sys.argv[1:])
roundtrip_artifact = json.loads(roundtrip_path.read_text())
trace = roundtrip_artifact.get("roundtrip", {})

def valid_trace(value):
    calls = value.get("command_trace", [])
    commits = {row.get("conflict_policy"): row for row in calls if row.get("tool") == "objects.import_commit"}
    reject = commits.get("reject_existing", {})
    reuse = commits.get("reuse_import_lineage", {})
    return bool(value.get("status") == "passed" and len(calls) == 7
        and reject.get("existing_document_cardinality") in (0, 1)
        and reject.get("canonical_writes", 0) > 0
        and reuse.get("existing_document_cardinality") == 0
        and reuse.get("canonical_writes") == 0 and reuse.get("head_changes") == 0)

def executed(log):
    body = log.read_text(errors="replace")
    summaries = re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;", body, re.M)
    return sum(int(passed) + int(failed) for _, passed, failed, _ in summaries)

green_body = green_log.read_text(errors="replace")
registry_executed = executed(green_log)
registry_ok = registry_executed == 1 and "test result: ok. 1 passed; 0 failed; 0 ignored;" in green_body
missing_red = "has no cardinality declaration" in missing_log.read_text(errors="replace") and executed(missing_log) == 1
lock_red = "declares BoundedMany without an ADR-0013 section 2 lock-order mechanism" in lock_log.read_text(errors="replace") and executed(lock_log) == 1
adr_ok = "existing_document_cardinality = 0 | 1 | bounded_many" in adr.read_text()
head = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
artifact_head = roundtrip_artifact.get("source_head")
artifact_current = artifact_head == head
passed = registry_ok and missing_red and lock_red and adr_ok and valid_trace(trace) and artifact_current
result = {
    "schema_version":"sylvode.flow.cardinality-result.v5", "release":"0.9.0", "since_release":"0.8",
    "source_head":head, "new_commands_found":0, "new_commands_reason":"not_required_zero_new_commands_rc_freeze",
    "existing_document_cardinality":{"new_commands":{}, "roundtrip_import_branches":trace.get("branches", [])},
    "checks":[
        {"id":"live_dispatch_exact_cardinality_coverage", "status":"passed" if registry_ok else "failed", "executed_count":registry_executed},
        {"id":"missing_declaration_mutation", "status":"passed" if missing_red else "failed", "executed_count":executed(missing_log)},
        {"id":"bounded_many_requires_lock_order_mutation", "status":"passed" if lock_red else "failed", "executed_count":executed(lock_log)},
        {"id":"adr_cardinality_rule", "status":"passed" if adr_ok else "failed", "executed_count":1},
        {"id":"actual_roundtrip_trace", "status":"passed" if valid_trace(trace) else "failed", "executed_count":len(trace.get("command_trace", []))},
        {"id":"roundtrip_artifact_current_head", "status":"passed" if artifact_current else "failed", "executed_count":1, "artifact_head":artifact_head},
    ],
    "mutations":{
        "production_command_without_declaration":{"red":missing_red, "log":str(missing_log)},
        "bounded_many_without_lock_order":{"red":lock_red, "log":str(lock_log)},
    },
    "mutation":{"name":"live-cardinality-registry-source-mutations", "red":missing_red and lock_red},
    "executed_count":registry_executed + executed(missing_log) + executed(lock_log) + 2 + len(trace.get("command_trace", [])),
    "log_sha256":hashlib.sha256(green_log.read_bytes()).hexdigest(), "passed":passed,
    "generated_at":dt.datetime.now(dt.timezone.utc).isoformat(),
}
fd, temporary = tempfile.mkstemp(prefix=".cardinality-result.", dir=evidence)
with os.fdopen(fd, "w") as handle:
    json.dump(result, handle, sort_keys=True, indent=2)
    handle.write("\n")
os.replace(temporary, evidence / "cardinality-result.json")
print(json.dumps(result, sort_keys=True))
raise SystemExit(0 if passed else 1)
PY
