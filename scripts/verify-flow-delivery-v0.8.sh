#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CONTRACTS_ROOT=/opt/working/sylvode-flow
EVIDENCE_ROOT=
CONTRACT_PATH=
ADR_PATH=
JSON_MODE=0
while (($#)); do
  case "$1" in
    --repo-root) REPO_ROOT=${2:?}; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --contract) CONTRACT_PATH=${2:?}; shift 2 ;;
    --adr) ADR_PATH=${2:?}; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $JSON_MODE -eq 1 ]] || { echo 'FAIL: --json is required' >&2; exit 2; }
[[ -n ${OPENPR_TEST_DATABASE_URL:-} ]] || { echo 'FAIL: OPENPR_TEST_DATABASE_URL is required' >&2; exit 2; }
[[ -n $EVIDENCE_ROOT ]] || EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.8"
[[ -n $CONTRACT_PATH ]] || CONTRACT_PATH=contracts/events-v1.md
[[ -n $ADR_PATH ]] || ADR_PATH=decisions/ADR-0011-event-delivery-substrate.md
[[ $CONTRACT_PATH = /* ]] || CONTRACT_PATH="$CONTRACTS_ROOT/$CONTRACT_PATH"
[[ $ADR_PATH = /* ]] || ADR_PATH="$CONTRACTS_ROOT/$ADR_PATH"
LIMITS_PATH="$CONTRACTS_ROOT/contracts/limits-v1.md"
for path in "$CONTRACT_PATH" "$ADR_PATH" "$LIMITS_PATH"; do
  [[ -s $path ]] || { echo "FAIL: authoritative input missing or empty: $path" >&2; exit 2; }
done
mkdir -p "$EVIDENCE_ROOT/logs"

TEST_LOG="$EVIDENCE_ROOT/logs/delivery-tests.log"
MUTATION_LOG="$EVIDENCE_ROOT/logs/delivery-mutations.log"
set +e
env -u RUST_TEST_THREADS \
  OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" CARGO_BUILD_JOBS=4 \
  cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api --lib \
    events::dispatcher::dispatcher_database_tests:: -- --nocapture >"$TEST_LOG" 2>&1
TEST_STATUS=$?
env -u RUST_TEST_THREADS \
  OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" CARGO_BUILD_JOBS=4 \
  "$REPO_ROOT/scripts/verify-flow-replay-mutations-v0.8.sh" >"$MUTATION_LOG" 2>&1
MUTATION_STATUS=$?
set -e

python3 - \
  "$REPO_ROOT" "$CONTRACT_PATH" "$ADR_PATH" "$LIMITS_PATH" "$EVIDENCE_ROOT" \
  "$TEST_STATUS" "$MUTATION_STATUS" <<'PY'
import datetime as dt
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile

repo, contract, adr, limits, evidence = map(pathlib.Path, sys.argv[1:6])
test_status, mutation_status = map(int, sys.argv[6:8])
repo, contract, adr, limits, evidence = (p.resolve() for p in (repo, contract, adr, limits, evidence))
test_log = evidence / "logs/delivery-tests.log"
mutation_log = evidence / "logs/delivery-mutations.log"

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def summaries(path):
    return [
        (state, int(passed), int(failed), int(ignored))
        for state, passed, failed, ignored in re.findall(
            r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;",
            path.read_text(errors="replace"), re.M
        )
    ]

test_summaries = summaries(test_log)
test_executed = sum(passed + failed for _, passed, failed, _ in test_summaries)
tests_ok = (
    test_status == 0 and test_executed > 0 and
    all(state == "ok" and failed == 0 for state, _, failed, _ in test_summaries)
)

expected_green = {
    "retention_green_control", "terminated_anchor_green_control",
    "admin_idempotency_green_control", "delivery_backoff_green_control",
    "delivery_recovery_green_control", "consumer_dedupe_green_control",
}
expected_red = {
    "source_tombstone_expires_with_delivery", "requeue_filters_source_event_time",
    "replay_exact_oldest_boundary_allowed", "delivery_backoff_step_drift",
    "successful_delivery_never_terminalizes", "delivery_crash_lease_not_reclaimed",
    "coalesced_consumer_uses_event_id", "replay_accepts_non_admin_member",
}
mutation_rows = []
mutation_executed = 0
for label, code, expected, raw_path in re.findall(
    r"^([a-z0-9_]+) status=(\d+) expected=(green|red) log=(\S+)$",
    mutation_log.read_text(errors="replace"), re.M
):
    nested = pathlib.Path(raw_path)
    nested_summaries = summaries(nested) if nested.is_file() else []
    executed = sum(passed + failed for _, passed, failed, _ in nested_summaries)
    mutation_executed += executed
    code = int(code)
    detected = executed > 0 and ((expected == "green" and code == 0) or (expected == "red" and code != 0))
    mutation_rows.append({
        "id": label, "expected": expected, "exit_code": code,
        "executed_count": executed, "detected": detected,
        "log": str(nested), "sha256": digest(nested) if nested.is_file() else None,
    })
green_found = {row["id"] for row in mutation_rows if row["expected"] == "green"}
red_found = {row["id"] for row in mutation_rows if row["expected"] == "red"}
mutations_ok = (
    mutation_status == 0 and green_found == expected_green and red_found == expected_red and
    all(row["detected"] for row in mutation_rows)
)

source = (repo / "apps/api/src/events/dispatcher.rs").read_text()
def constant(name):
    match = re.search(rf"(?:pub )?const {name}: i64 = ([0-9_]+);", source)
    return int(match.group(1).replace("_", "")) if match else None

implementation = {
    "delivery_retention_days": constant("DELIVERY_RETENTION_DAYS"),
    "delivery_source_retention_days": constant("DELIVERY_SOURCE_RETENTION_DAYS"),
    "replay_max_window_days": constant("REPLAY_MAX_WINDOW_DAYS"),
}
relationship_ok = (
    implementation["delivery_source_retention_days"] is not None and
    implementation["replay_max_window_days"] is not None and
    implementation["delivery_retention_days"] is not None and
    implementation["delivery_source_retention_days"] > implementation["replay_max_window_days"] and
    implementation["delivery_source_retention_days"] >= implementation["delivery_retention_days"]
)
limits_text = limits.read_text()
def authority_cell(key):
    match = re.search(rf"\| `{key}` \| `([^`]*)` \|", limits_text)
    return match.group(1) if match else None

budget_cells = {
    "delivery_source_retention_days": authority_cell("delivery_source_retention_days"),
    "replay_max_window_days": authority_cell("replay_max_window_days"),
}
authority_unset = all(value == "status: unset" for value in budget_cells.values())
approved_values = {}
for key, value in budget_cells.items():
    if value and re.fullmatch(r"[1-9][0-9]*", value):
        approved_values[key] = int(value)

dirty = subprocess.check_output([
    "git", "-C", str(repo), "status", "--porcelain=v1", "--",
    "apps", "crates", "migrations", "scripts", "Cargo.toml", "Cargo.lock"
], text=True).splitlines()
functional_passed = tests_ok and mutations_ok and relationship_ok and not dirty
approved_budgets_locked = approved_values == {
    "delivery_source_retention_days": implementation["delivery_source_retention_days"],
    "replay_max_window_days": implementation["replay_max_window_days"],
}
passed = functional_passed and approved_budgets_locked
checks = [
    {"id": "delivery_database_suite", "status": "passed" if tests_ok else "failed",
     "exit_code": test_status, "executed_count": test_executed,
     "log": "logs/delivery-tests.log", "sha256": digest(test_log)},
    {"id": "delivery_production_mutations", "status": "passed" if mutations_ok else "failed",
     "exit_code": mutation_status, "executed_count": mutation_executed,
     "log": "logs/delivery-mutations.log", "sha256": digest(mutation_log)},
    {"id": "retention_relationship", "status": "passed" if relationship_ok else "failed",
     "executed_count": 1},
    {"id": "deferred_retention_budgets_frozen", "status": "passed" if approved_budgets_locked else "failed",
     "executed_count": 1, "reason": f"authoritative limits contract cells are {budget_cells}"},
]
result = {
    "schema_version": "sylvode.flow.delivery-result.v1", "release": "0.8.0",
    "source_head": subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip(),
    "authoritative_inputs": {
        "contract": {"path": str(contract), "sha256": digest(contract)},
        "adr": {"path": str(adr), "sha256": digest(adr)},
        "limits": {"path": str(limits), "sha256": digest(limits)},
    },
    "implementation_retention_days": implementation,
    "retention_relationship_strict": relationship_ok,
    "budget_authority_cells": budget_cells,
    "functional_passed": functional_passed,
    "approved_budgets_locked": approved_budgets_locked,
    "mutation_cases": mutation_rows,
    "checks": checks,
    "executed_count": test_executed + mutation_executed + 2,
    "source_dirty": bool(dirty), "dirty_entries": dirty,
    "passed": passed,
    "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
}
fd, tmp = tempfile.mkstemp(prefix=".delivery.", dir=evidence)
with os.fdopen(fd, "w") as handle:
    json.dump(result, handle, sort_keys=True, indent=2)
    handle.write("\n")
os.replace(tmp, evidence / "delivery-result.json")
print(json.dumps(result, sort_keys=True))
raise SystemExit(0 if passed else 1)
PY
