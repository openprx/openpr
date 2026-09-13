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
mkdir -p "$EVIDENCE_ROOT/logs"
LOG="$EVIDENCE_ROOT/logs/cardinality-v09-freeze.log"
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api --lib \
  flow::command::cardinality_gate_tests::v0_9_rc_freeze_adds_no_command_cardinality_declaration \
  -- --exact --nocapture >"$LOG" 2>&1

python3 - "$REPO_ROOT" "$ADR_PATH" "$ROUNDTRIP" "$EVIDENCE_ROOT" "$LOG" <<'PY'
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

repo, adr, roundtrip_path, evidence, log = map(pathlib.Path, sys.argv[1:])
roundtrip_artifact = json.loads(roundtrip_path.read_text())
trace = roundtrip_artifact.get("roundtrip", {})

def valid_trace(value):
    calls = value.get("command_trace", [])
    commits = {row.get("conflict_policy"): row for row in calls if row.get("tool") == "objects.import_commit"}
    reject = commits.get("reject_existing", {})
    reuse = commits.get("reuse_import_lineage", {})
    return bool(
        value.get("status") == "passed"
        and len(calls) == 7
        and reject.get("existing_document_cardinality") in (0, 1)
        and reject.get("canonical_writes", 0) > 0
        and reuse.get("existing_document_cardinality") == 0
        and reuse.get("canonical_writes") == 0
        and reuse.get("head_changes") == 0
    )

body = log.read_text(errors="replace")
summaries = re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;", body, re.M)
registry_executed = sum(int(passed) + int(failed) for _, passed, failed, _ in summaries)
registry_ok = registry_executed == 1 and all(state == "ok" and failed == "0" and ignored == "0"
                                             for state, _, failed, ignored in summaries)
adr_ok = "existing_document_cardinality = 0 | 1 | bounded_many" in adr.read_text()
mutated = copy.deepcopy(trace)
for row in mutated.get("command_trace", []):
    if row.get("tool") == "objects.import_commit" and row.get("conflict_policy") == "reuse_import_lineage":
        row["head_changes"] = 1
mutation_red = not valid_trace(mutated)
head = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
artifact_head = roundtrip_artifact.get("source_head")
artifact_current = artifact_head == head
passed = registry_ok and adr_ok and valid_trace(trace) and mutation_red and artifact_current
result = {
    "schema_version":"sylvode.flow.cardinality-result.v4", "release":"0.9.0", "since_release":"0.8",
    "source_head":head, "new_commands_found":0, "new_commands_reason":"not_required_zero_new_commands_rc_freeze",
    "existing_document_cardinality":{"new_commands":{}, "roundtrip_import_branches":trace.get("branches", [])},
    "checks":[
        {"id":"runtime_registry_freeze", "status":"passed" if registry_ok else "failed", "executed_count":registry_executed},
        {"id":"adr_cardinality_rule", "status":"passed" if adr_ok else "failed", "executed_count":1},
        {"id":"actual_roundtrip_trace", "status":"passed" if valid_trace(trace) else "failed", "executed_count":len(trace.get("command_trace", []))},
        {"id":"roundtrip_artifact_current_head", "status":"passed" if artifact_current else "failed", "executed_count":1,
         "artifact_head":artifact_head},
    ],
    "mutation":{"name":"reuse-import-advances-head", "red":mutation_red},
    "executed_count":registry_executed + 2 + len(trace.get("command_trace", [])),
    "log_sha256":hashlib.sha256(log.read_bytes()).hexdigest(), "passed":passed,
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
