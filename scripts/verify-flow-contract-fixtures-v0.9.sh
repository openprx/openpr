#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.9"
JSON_MODE=0
while (($#)); do
  case "$1" in
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $JSON_MODE -eq 1 ]] || { echo 'FAIL: --json is required' >&2; exit 2; }
DATABASE_URL=${OPENPR_TEST_DATABASE_URL:-postgresql://flowtest:flowtest@127.0.0.1:25433/postgres}
CACHE_ROOT=/opt/worker/.cache/openpr-v09-contract-fixtures
LOG_ROOT="$CACHE_ROOT/logs"
PACKAGE_MUTATION="$CACHE_ROOT/package-mutation"
EVENT_MUTATION="$CACHE_ROOT/event-mutation"
mkdir -p "$LOG_ROOT" "$EVIDENCE_ROOT"
rm -rf "$PACKAGE_MUTATION" "$EVENT_MUTATION"
cp -a "$REPO_ROOT/testing/fixtures/flow-package-v1" "$PACKAGE_MUTATION"
cp -a "$REPO_ROOT/testing/fixtures/flow-event-v1" "$EVENT_MUTATION"

PACKAGE_TEST=flow::package::tests::locked_package_fixture_rebuilds_to_the_frozen_archive_hash
EVENT_FILTER=golden_wire_fixture_
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
  cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api --lib "$PACKAGE_TEST" -- --exact --nocapture \
  >"$LOG_ROOT/package-green.log" 2>&1
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
  cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api --lib "$EVENT_FILTER" -- --nocapture \
  >"$LOG_ROOT/event-green.log" 2>&1

python3 - "$PACKAGE_MUTATION/package-fixture.json" "$EVENT_MUTATION/coalesced.json" <<'PY'
import json, pathlib, sys
package_path, event_path = map(pathlib.Path, sys.argv[1:])
package = json.loads(package_path.read_text())
package["expected_package_sha256"] = "0" * 64
package_path.write_text(json.dumps(package, sort_keys=True, indent=2) + "\n")
event = json.loads(event_path.read_text())
del event["delivery"]["range"]
event_path.write_text(json.dumps(event, sort_keys=True, indent=2) + "\n")
PY

set +e
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
  OPENPR_TEST_FLOW_PACKAGE_FIXTURE_DIR="$PACKAGE_MUTATION" \
  cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api --lib "$PACKAGE_TEST" -- --exact --nocapture \
  >"$LOG_ROOT/package-mutation.log" 2>&1
PACKAGE_MUTATION_EXIT=$?
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
  OPENPR_TEST_FLOW_EVENT_FIXTURE_DIR="$EVENT_MUTATION" \
  cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api --lib \
    events::dispatcher::dispatcher_database_tests::golden_wire_fixture_coalesced_delivery_body_matches_the_frozen_shape \
    -- --exact --nocapture \
  >"$LOG_ROOT/event-mutation.log" 2>&1
EVENT_MUTATION_EXIT=$?
set -e

python3 - "$REPO_ROOT" "$EVIDENCE_ROOT" "$LOG_ROOT" "$PACKAGE_MUTATION_EXIT" "$EVENT_MUTATION_EXIT" <<'PY'
import datetime as dt
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile

repo, evidence, logs = map(pathlib.Path, sys.argv[1:4])
package_mutation_exit, event_mutation_exit = map(int, sys.argv[4:])

def test_result(name, expected):
    path = logs / f"{name}.log"
    text = path.read_text(errors="replace")
    summaries = re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;", text, re.M)
    executed = sum(int(passed) + int(failed) for _, passed, failed, _ in summaries)
    passed = executed == expected and all(state == "ok" and failed == "0" and ignored == "0"
                                                for state, _, failed, ignored in summaries)
    return {"passed": passed, "executed_count": executed, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}

def tree_hash(root):
    files = sorted(path for path in root.rglob("*") if path.is_file())
    digest = hashlib.sha256()
    for path in files:
        digest.update(path.relative_to(root).as_posix().encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return len(files), digest.hexdigest()

package_green = test_result("package-green", 1)
event_green = test_result("event-green", 3)
package_count, package_hash = tree_hash(repo / "testing/fixtures/flow-package-v1")
event_count, event_hash = tree_hash(repo / "testing/fixtures/flow-event-v1")
source_head = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
generated_at = dt.datetime.now(dt.timezone.utc).isoformat()
common = {"release":"0.9.0", "source_head":source_head, "generated_at":generated_at}
package = {**common, "schema_version":"sylvode.flow.export-package-fixture-result.v1",
           "fixture_count":package_count, "fixture_tree_sha256":package_hash,
           "reader_execution":package_green,
           "mutation":{"name":"archive-hash-drift", "exit_code":package_mutation_exit,
                       "red":package_mutation_exit != 0},
           "executed_count":2}
package["passed"] = package_green["passed"] and package_count > 0 and package["mutation"]["red"]
event = {**common, "schema_version":"sylvode.flow.event-consumer-fixture-result.v1",
         "fixture_count":event_count, "fixture_tree_sha256":event_hash,
         "producer_consumer_executions":event_green,
         "covered_shapes":["plain","coalesced","retry"],
         "consumer_dedupe_key":"delivery.id",
         "mutation":{"name":"coalesced-range-removed", "exit_code":event_mutation_exit,
                     "red":event_mutation_exit != 0},
         "executed_count":4}
event["passed"] = event_green["passed"] and event_count == 3 and event["mutation"]["red"]
for filename, payload in (("package-fixture-result.json", package), ("event-consumer-fixture-result.json", event)):
    fd, temporary = tempfile.mkstemp(prefix=f".{filename}.", dir=evidence)
    with os.fdopen(fd, "w") as handle:
        json.dump(payload, handle, sort_keys=True, indent=2)
        handle.write("\n")
    os.replace(temporary, evidence / filename)
print(json.dumps({"package":package,"event":event,"passed":package["passed"] and event["passed"]}, sort_keys=True))
raise SystemExit(0 if package["passed"] and event["passed"] else 1)
PY
