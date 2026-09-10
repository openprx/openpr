#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4/v0.5 collab-architecture verifier.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md, the v0.4
# section's "Architecture verifier" paragraph, and
# decisions/ADR-0010-collab-server-architecture.md ("量化接受与推翻门槛").
#
# Covers 6 hard gates: collab_architecture_adr_accepted,
# bounded_warm_cache_lock_hold_and_round_trip_budgets,
# minimal_snapshot_advancement_bounds_tail, snapshot_tail_restart_recovery,
# bootstrap_repeatable_read_and_ws_parity,
# accepted_egress_seq_monotonic_and_gap_resync.
#
# HOW IT WORKS (calls what already exists and independently scores its output):
#
#   1. ADR status (collab_architecture_adr_accepted): reads the literal
#      "- 状态：<word>" line from ADR-0010. It is currently "Proposed" (by
#      design -- v0.4-gate.yaml's own required_decisions section documents
#      this as the expected status AT ENTRY, with Accepted only required
#      at candidate time). This script reports the gate honestly: not yet
#      Accepted, so not yet passed -- it does not treat "expected at this
#      stage" as license to call the gate satisfied.
#
#   2. Frozen numeric budgets (part of
#      bounded_warm_cache_lock_hold_and_round_trip_budgets): extracts the
#      hard-ceiling numbers ADR-0010's own text freezes (lock wait <=
#      100ms, lock hold ceiling == 100ms, max 3 out-of-lock rebases) and
#      cross-checks them against the compiled constants in
#      apps/api/src/flow/collab/limits.rs (DOCUMENT_LOCK_WAIT_MS_MAX,
#      DOCUMENT_LOCK_HOLD_MS_MAX, MAX_REBASE_ATTEMPTS), then runs the two
#      existing real tests that assert those same constants end-to-end:
#      flow::collab::write::database_tests::lock_timeout_budgets_match_
#      the_frozen_limits_v1_numbers (a `#[test]`, asserts the Rust
#      constants) and flow::collab::snapshot::tests::frozen_limits_v1_
#      numbers_are_exactly_what_this_module_enforces (asserts the
#      snapshot soft/hard trigger constants). This is real, but it is NOT
#      the same claim as the gate's other two numbers -- see gap below.
#
#   3. Snapshot advancement + restart recovery (minimal_snapshot_
#      advancement_bounds_tail, snapshot_tail_restart_recovery): apps/
#      api/src/flow/collab/snapshot.rs's own `#[cfg(test)] mod
#      database_tests` carries doc comments of the literal shape
#      "Gate <N> `<hard_gate_id>`" directly above each test function --
#      this script parses those markers DIRECTLY FROM THE SOURCE FILE at
#      run time (never a hardcoded mapping baked into this script), then
#      actually runs `cargo test -p api flow::collab::snapshot` against a
#      real Postgres database and parses the real per-test ok/FAILED
#      lines. A gate is only "passed" if every test the source itself
#      attributes to it exists, actually ran (not silently skipped -- see
#      the OPENPR_TEST_DATABASE_URL skip-detection below) and passed.
#
#   4. Load distributions and REST/WS parity: consumes the JSON emitted by
#      apps/api/tests/flow_collab_load_harness.rs and independently rechecks
#      every frozen budget, sample count, server-log reconstruction invariant,
#      locked statement inventory, RR+RO transaction count, seq continuity,
#      tail/frontier validity and same-head REST/WS identity. The official run
#      is admissible only when it is release-built and its recorded PostgreSQL
#      log container exactly matches --dedicated-pg-container. Shared-instance
#      numbers are deliberately rejected because WAL/fsync contention changes
#      both round-trip and transaction-hold distributions.
#
#   5. accepted_egress_seq_monotonic_and_gap_resync: this script's own
#     "Gate <N> `<id>`" marker extraction (used identically for gates 7
#     and 8 above) is what actually decides this gate's status -- if
#     snapshot.rs ever grows a Gate 10 `accepted_egress_seq_monotonic_and_
#     gap_resync` marker with attributed tests, this gate starts scoring
#     "passed"/"failed" from those tests' real ok/FAILED results like any
#     other gate. Its current markers are scored from their live test output;
#     deleting or renaming either marker/test makes the gate red again.
#
# Exit codes: 0 = every one of the 6 gates recomputed to passed (does not
# happen today -- see gaps above), 1 = ran to completion and wrote
# evidence/v0.4/collab-architecture-result.json with one or more gates not
# passed, 2 = usage/tool/environment error, OR the snapshot database tests
# were silently skipped because OPENPR_TEST_DATABASE_URL is not set.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Shared --adr/--contract/--limits path resolution (absolute -> as-is;
# relative-to-CWD -> as-is; otherwise resolved against --contracts-root;
# unresolvable -> FAIL naming both attempted paths).
# shellcheck source=scripts/lib/flow_contract_path.sh
source "$ROOT_DIR/scripts/lib/flow_contract_path.sh"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT=""
REPO_ROOT="$ROOT_DIR"
RELEASE="0.4"
CLIENTS=""
ADR_PATH=""
LIMITS_PATH=""
JSON_MODE=0
SKIP_CARGO_TEST=0
LOAD_HARNESS_EVIDENCE="${OPENPR_FLOW_LOAD_HARNESS_EVIDENCE:-}"
CACHE_EVIDENCE="${OPENPR_FLOW_CACHE_EVIDENCE:-}"
DEDICATED_PG_CONTAINER="${OPENPR_FLOW_DEDICATED_PG_CONTAINER:-}"

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-collab-architecture.sh --release VER --json [OPTIONS]

Reads ADR-0010's DOCUMENT_CONTROL status and frozen numeric budgets
(read-only), cross-checks the lock-wait/lock-hold/rebase-attempt
constants against apps/api/src/flow/collab/limits.rs, then runs the real
snapshot-advancement and restart-recovery Rust tests
(flow::collab::snapshot::{tests,database_tests}) and attributes each to
a hard gate using the "Gate <N> `<id>`" doc-comment markers snapshot.rs
itself carries. Writes evidence/v0.4/collab-architecture-result.json.

Consumes a separately generated release-mode load-harness artifact for the p95
lock-hold / 10-client round-trip and REST/WS parity gates. The artifact must be
from an explicitly declared dedicated PostgreSQL container; shared-instance
measurements are not accepted as official evidence.

Options:
  --release VER            Gate release identifier (recorded in output).
                          Default: 0.4
  --clients N              Collaboration client count. Required for v0.5;
                          the v0.5 hard gate passes only at exactly 10.
  --adr PATH               Path to ADR-0010. Required. A relative path is
                          resolved against the current directory first,
                          then against --contracts-root (same for
                          --limits).
  --limits PATH            Path to contracts/limits-v1.md (recorded in
                          output; only frozen numbers embedded in the ADR
                          text itself are cross-checked against source
                          today). Required.
  --contracts-root DIR     Root containing decisions/. Default:
                          /opt/working/sylvode-flow
  --evidence-root DIR     Where collab-architecture-result.json is
                          written. Default: evidence/v<release> under
                          --contracts-root.
  --repo-root DIR         Repository containing apps/api and the cargo
                          workspace. Default: this checkout.
  --load-harness-evidence PATH
                          JSON emitted by flow_collab_load_harness.rs.
  --cache-evidence PATH   JSON emitted by flow_collab_cache_evidence.rs.
  --dedicated-pg-container NAME
                          Required with harness evidence. Must exactly match
                          environment.pg_log_container in that evidence; this
                          makes the official no-shared-load prerequisite an
                          explicit, audited input.
  --skip-cargo-test        Skip the snapshot database test run (fast
                          iteration only; the written evidence records
                          this and every gate that needed the dynamic
                          check is treated as failed).
  --json                  Required for CLI-contract compatibility.
  -h, --help              Show this help and exit 0.

Exit codes: 0 all 6 gates passed, 1 ran to completion with one or more
gates not passed (the honest, normal outcome today), 2 usage/tool/
environment error or the snapshot tests were silently skipped because
OPENPR_TEST_DATABASE_URL is not set.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release) RELEASE="${2:?--release requires a value}"; shift 2 ;;
    --clients) CLIENTS="${2:?--clients requires N}"; shift 2 ;;
    --adr) ADR_PATH="${2:?--adr requires a PATH argument}"; shift 2 ;;
    --limits) LIMITS_PATH="${2:?--limits requires a PATH argument}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --load-harness-evidence) LOAD_HARNESS_EVIDENCE="${2:?--load-harness-evidence requires a PATH}"; shift 2 ;;
    --cache-evidence) CACHE_EVIDENCE="${2:?--cache-evidence requires a PATH}"; shift 2 ;;
    --dedicated-pg-container) DEDICATED_PG_CONTAINER="${2:?--dedicated-pg-container requires a NAME}"; shift 2 ;;
    --skip-cargo-test) SKIP_CARGO_TEST=1; shift ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$EVIDENCE_ROOT" ]]; then
  EVIDENCE_ROOT="${TMPDIR:-/tmp}/openpr-flow-evidence/v$RELEASE"
fi
if [[ "$RELEASE" == "0.5" ]]; then
  [[ "$CLIENTS" =~ ^[1-9][0-9]*$ ]] || { echo "FAIL: v0.5 requires --clients N" >&2; exit 2; }
  [[ -n "$ADR_PATH" ]] || ADR_PATH="$CONTRACTS_ROOT/decisions/ADR-0010-collab-server-architecture.md"
  [[ -n "$LIMITS_PATH" ]] || LIMITS_PATH="$CONTRACTS_ROOT/contracts/limits-v1.md"
fi

if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
for tool in jq git python3 cargo realpath; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    exit 2
  fi
done

if [[ -z "$ADR_PATH" ]]; then
  echo "FAIL: --adr is required" >&2
  usage >&2
  exit 2
fi
if [[ -z "$LIMITS_PATH" ]]; then
  echo "FAIL: --limits is required" >&2
  usage >&2
  exit 2
fi
# v0.4-gate.yaml's required_commands invocation passes --adr/--limits as
# paths relative to the contracts root (e.g. "decisions/ADR-0010-...md",
# "contracts/limits-v1.md"); flow_resolve_contract_path accepts that exact
# invocation as well as a caller-supplied absolute path or a path relative
# to the current directory.
if ! ADR_PATH="$(flow_resolve_contract_path --adr "$ADR_PATH" "$CONTRACTS_ROOT")"; then
  exit 2
fi
if ! LIMITS_PATH="$(flow_resolve_contract_path --limits "$LIMITS_PATH" "$CONTRACTS_ROOT")"; then
  exit 2
fi
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi
CONTRACTS_REAL="$(realpath -m "$CONTRACTS_ROOT")"
EVIDENCE_REAL="$(realpath -m "$EVIDENCE_ROOT")"
case "$EVIDENCE_REAL/" in
  "$CONTRACTS_REAL/"*)
    echo "FAIL: refusing to write collaboration architecture evidence inside the read-only contract checkout: $EVIDENCE_REAL" >&2
    exit 2
    ;;
esac
EVIDENCE_ROOT="$EVIDENCE_REAL"

SNAPSHOT_RS="$REPO_ROOT/apps/api/src/flow/collab/snapshot.rs"
LIMITS_RS="$REPO_ROOT/apps/api/src/flow/collab/limits.rs"
for f in "$SNAPSHOT_RS" "$LIMITS_RS"; do
  if [[ ! -f "$f" ]]; then
    echo "FAIL: source file not found (nothing to statically verify): $f" >&2
    exit 2
  fi
done

mkdir -p "$EVIDENCE_ROOT" "$EVIDENCE_ROOT/logs"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

if [[ "$RELEASE" == "0.5" ]]; then
  FIXED_DATABASE_URL="postgresql://flowtest:flowtest@127.0.0.1:25433/postgres"
  SOURCE_DIRTY=false
  [[ -z "$(git -C "$REPO_ROOT" status --porcelain)" ]] || SOURCE_DIRTY=true
  V05_LOAD_DIR="$EVIDENCE_ROOT/logs/collab-load-v0.5"
  V05_LOAD_LOG="$EVIDENCE_ROOT/logs/collab-load-v0.5.log"
  V05_DYNAMIC_LOG="$EVIDENCE_ROOT/logs/collab-architecture-v0.5-tests.log"
  V05_RESUME_MUTATION_LOG="$EVIDENCE_ROOT/logs/resume-at-head-zero-frame-mutation.log"
  V05_EGRESS_MUTATION_LOG="$EVIDENCE_ROOT/logs/egress-duplicate-forward-mutation.log"
  V05_TOMBSTONE_MUTATION_LOG="$EVIDENCE_ROOT/logs/navigator-tombstone-hidden-growth-mutation.log"
  V05_RESUME_JSON="$EVIDENCE_ROOT/logs/resume-at-head-v0.5.json"
  V05_NAVIGATOR_JSON="$EVIDENCE_ROOT/logs/navigator-tombstone-v0.5.json"
  V05_SURFACE_JSON="$EVIDENCE_ROOT/logs/surface-coverage-v0.5.json"
  V05_LOAD_EXIT=0

  if [[ -z "$LOAD_HARNESS_EVIDENCE" ]]; then
    set +e
    "$REPO_ROOT/scripts/run-flow-collab-load-harness-v0.5.sh" \
      --clients "$CLIENTS" --repo-root "$REPO_ROOT" --database-url "$FIXED_DATABASE_URL" \
      --evidence-root "$V05_LOAD_DIR" --json >"$V05_LOAD_LOG" 2>&1
    V05_LOAD_EXIT=$?
    set -e
    LOAD_HARNESS_EVIDENCE="$V05_LOAD_DIR/collab-load-result.json"
  fi

  python3 "$REPO_ROOT/scripts/lib/flow_surface_coverage.py" \
    --contracts-root "$CONTRACTS_ROOT" --release 0.5 >"$V05_SURFACE_JSON"

  : >"$V05_DYNAMIC_LOG"
  : >"$V05_RESUME_JSON"
  V05_DYNAMIC_EXIT=0
  V05_TESTS=(
    flow::collab::session::tests::live_ws::inbound_updates_generated_from_one_base_are_accepted_in_injected_reverse_order
    flow::collab::session::tests::live_ws::outbound_duplicates_are_dropped_and_a_gap_is_backfilled_in_strict_seq_order
    flow::collab::session::tests::live_ws::an_unfillable_outbound_gap_resyncs_and_never_forwards_the_revealing_notice
    flow::collab::session::tests::live_ws::resume_at_head_returns_exact_ack_before_an_empty_replay
    flow::move_object::database_tests::navigator_tombstone_growth_is_measured_on_a_cascading_command
  )
  for test_name in "${V05_TESTS[@]}"; do
    set +e
    (cd "$REPO_ROOT" && OPENPR_TEST_DATABASE_URL="$FIXED_DATABASE_URL" \
      OPENPR_FLOW_RESUME_AT_HEAD_EVIDENCE_OUT="$V05_RESUME_JSON" \
      OPENPR_FLOW_NAVIGATOR_TOMBSTONE_EVIDENCE_OUT="$V05_NAVIGATOR_JSON" \
      cargo test --release -p api --lib "$test_name" -- --exact --test-threads=1 --nocapture) \
      >>"$V05_DYNAMIC_LOG" 2>&1
    test_exit=$?
    set -e
    if [[ $test_exit -ne 0 ]]; then V05_DYNAMIC_EXIT=1; fi
  done
  set +e
  (cd "$REPO_ROOT" && OPENPR_TEST_DATABASE_URL="$FIXED_DATABASE_URL" \
    cargo test --release -p api --lib 'flow::collab::snapshot::' -- --test-threads=1 --nocapture) \
    >>"$V05_DYNAMIC_LOG" 2>&1
  snapshot_exit=$?
  set -e
  if [[ $snapshot_exit -ne 0 ]]; then V05_DYNAMIC_EXIT=1; fi

  : >"$V05_RESUME_MUTATION_LOG"
  : >"$V05_EGRESS_MUTATION_LOG"
  : >"$V05_TOMBSTONE_MUTATION_LOG"
  mutation_started_ms="$(date +%s%3N)"
  set +e
  (cd "$REPO_ROOT" && OPENPR_TEST_DATABASE_URL="$FIXED_DATABASE_URL" \
    OPENPR_FLOW_TEST_MUTATION_EMPTY_RESUME_ZERO_FRAMES=1 \
    cargo test --release -p api --lib \
      flow::collab::session::tests::live_ws::resume_at_head_returns_exact_ack_before_an_empty_replay \
      -- --exact --test-threads=1 --nocapture) >>"$V05_RESUME_MUTATION_LOG" 2>&1
  V05_RESUME_MUTATION_EXIT=$?
  V05_RESUME_MUTATION_DURATION_MS=$(( $(date +%s%3N) - mutation_started_ms ))
  mutation_started_ms="$(date +%s%3N)"
  (cd "$REPO_ROOT" && OPENPR_TEST_DATABASE_URL="$FIXED_DATABASE_URL" \
    OPENPR_FLOW_TEST_MUTATION_FORWARD_EGRESS_DUPLICATE=1 \
    cargo test --release -p api --lib \
      flow::collab::session::tests::live_ws::outbound_duplicates_are_dropped_and_a_gap_is_backfilled_in_strict_seq_order \
      -- --exact --test-threads=1 --nocapture) >>"$V05_EGRESS_MUTATION_LOG" 2>&1
  V05_EGRESS_MUTATION_EXIT=$?
  V05_EGRESS_MUTATION_DURATION_MS=$(( $(date +%s%3N) - mutation_started_ms ))
  mutation_started_ms="$(date +%s%3N)"
  (cd "$REPO_ROOT" && OPENPR_TEST_DATABASE_URL="$FIXED_DATABASE_URL" \
    OPENPR_FLOW_TEST_MUTATION_HIDE_TOMBSTONE_GROWTH=1 \
    cargo test --release -p api --lib \
      flow::move_object::database_tests::navigator_tombstone_growth_is_measured_on_a_cascading_command \
      -- --exact --test-threads=1 --nocapture) >>"$V05_TOMBSTONE_MUTATION_LOG" 2>&1
  V05_TOMBSTONE_MUTATION_EXIT=$?
  V05_TOMBSTONE_MUTATION_DURATION_MS=$(( $(date +%s%3N) - mutation_started_ms ))
  set -e

  V05_SOURCE_DIRTY_AFTER=false
  [[ -z "$(git -C "$REPO_ROOT" status --porcelain)" ]] || V05_SOURCE_DIRTY_AFTER=true
  ADR_SHA256="$(sha256sum "$ADR_PATH" | awk '{print $1}')"
  LIMITS_SHA256="$(sha256sum "$LIMITS_PATH" | awk '{print $1}')"
  EXECUTOR="${USER:-$(id -un)}@$(hostname)"

  set +e
  python3 - "$LOAD_HARNESS_EVIDENCE" "$V05_DYNAMIC_LOG" \
    "$V05_RESUME_MUTATION_LOG" "$V05_EGRESS_MUTATION_LOG" "$V05_TOMBSTONE_MUTATION_LOG" \
    "$V05_RESUME_JSON" "$V05_NAVIGATOR_JSON" "$V05_SURFACE_JSON" \
    "$SOURCE_HEAD" "$SOURCE_DIRTY" "$V05_SOURCE_DIRTY_AFTER" "$CLIENTS" \
    "$ADR_SHA256" "$LIMITS_SHA256" "$GENERATED_AT" "$EXECUTOR" \
    "$V05_LOAD_EXIT" "$V05_DYNAMIC_EXIT" \
    "$V05_RESUME_MUTATION_EXIT" "$V05_RESUME_MUTATION_DURATION_MS" \
    "$V05_EGRESS_MUTATION_EXIT" "$V05_EGRESS_MUTATION_DURATION_MS" \
    "$V05_TOMBSTONE_MUTATION_EXIT" "$V05_TOMBSTONE_MUTATION_DURATION_MS" \
    "$EVIDENCE_ROOT/collab-architecture-result.json" <<'PY'
import hashlib
import json
import os
import re
import sys

(
    load_path, test_log_path, resume_mutation_log_path, egress_mutation_log_path,
    tombstone_mutation_log_path, resume_evidence_path, navigator_evidence_path, surface_path, source_head, source_dirty,
    source_dirty_after, clients_raw, adr_sha, limits_sha, generated_at,
    executor, load_exit_raw, dynamic_exit_raw, resume_mutation_exit_raw,
    resume_mutation_duration_raw, egress_mutation_exit_raw,
    egress_mutation_duration_raw, tombstone_mutation_exit_raw,
    tombstone_mutation_duration_raw, output_path,
) = sys.argv[1:]

def read_json(path):
    try:
        with open(path, encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, json.JSONDecodeError):
        return {}

load = read_json(load_path)
surface = read_json(surface_path)
try:
    test_log = open(test_log_path, encoding="utf-8").read()
except OSError:
    test_log = ""
try:
    resume_mutation_log = open(resume_mutation_log_path, encoding="utf-8").read()
except OSError:
    resume_mutation_log = ""
try:
    egress_mutation_log = open(egress_mutation_log_path, encoding="utf-8").read()
except OSError:
    egress_mutation_log = ""
try:
    tombstone_mutation_log = open(tombstone_mutation_log_path, encoding="utf-8").read()
except OSError:
    tombstone_mutation_log = ""
navigator_evidence = read_json(navigator_evidence_path)

clients = int(clients_raw)
load_exit = int(load_exit_raw)
dynamic_exit = int(dynamic_exit_raw)
resume_mutation_exit = int(resume_mutation_exit_raw)
resume_mutation_duration_ms = int(resume_mutation_duration_raw)
egress_mutation_exit = int(egress_mutation_exit_raw)
egress_mutation_duration_ms = int(egress_mutation_duration_raw)
tombstone_mutation_exit = int(tombstone_mutation_exit_raw)
tombstone_mutation_duration_ms = int(tombstone_mutation_duration_raw)
dirty = source_dirty == "true" or source_dirty_after == "true"
problems = []

def require(name, condition):
    if not condition:
        problems.append(name)
    return bool(condition)

def test_ok(name):
    match = re.search(
        r"^test " + re.escape(name) + r" \.\.\.(.*?)(?=^test |^test result:)",
        test_log,
        re.M | re.S,
    )
    return bool(match and re.search(r"(?:^|\n)ok\b|\sok$", match.group(1), re.M))

expected_tests = {
    "inbound_reorder": "flow::collab::session::tests::live_ws::inbound_updates_generated_from_one_base_are_accepted_in_injected_reverse_order",
    "egress_duplicate_reorder": "flow::collab::session::tests::live_ws::outbound_duplicates_are_dropped_and_a_gap_is_backfilled_in_strict_seq_order",
    "egress_gap_resync": "flow::collab::session::tests::live_ws::an_unfillable_outbound_gap_resyncs_and_never_forwards_the_revealing_notice",
    "resume_at_head_ack": "flow::collab::session::tests::live_ws::resume_at_head_returns_exact_ack_before_an_empty_replay",
    "soft_count": "flow::collab::snapshot::tests::exactly_at_the_soft_update_count_is_soft_not_hard",
    "soft_bytes": "flow::collab::snapshot::tests::exactly_at_the_soft_byte_count_is_soft",
    "hard_count": "flow::collab::snapshot::tests::exactly_at_the_hard_update_count_is_hard_not_soft",
    "hard_bytes": "flow::collab::snapshot::tests::exactly_at_the_hard_byte_count_is_hard",
    "soft_rebuild": "flow::collab::snapshot::database_tests::soft_trigger_advances_snapshot_shortens_the_tail_and_keeps_every_old_update",
    "hard_checkpoint": "flow::collab::snapshot::database_tests::hard_boundary_forces_a_checkpoint_before_the_next_update_is_accepted",
    "checkpoint_race": "flow::collab::snapshot::database_tests::advancement_stays_correct_when_racing_a_concurrent_write",
    "snapshot_restart": "flow::collab::snapshot::database_tests::restart_recovery_reproduces_the_exact_same_semantic_hash_from_snapshot_plus_tail",
    "navigator_tombstones": "flow::move_object::database_tests::navigator_tombstone_growth_is_measured_on_a_cascading_command",
}
tests = {key: test_ok(name) for key, name in expected_tests.items()}
for key, value in tests.items():
    require(f"dynamic test did not pass: {key}", value)

resume_evidence = read_json(resume_evidence_path)
resume_position = resume_evidence.get("request_position") or {}
resume_response = resume_evidence.get("first_observable_response") or {}
resume_evidence_exact = all((
    bool(resume_position.get("document_id")),
    isinstance(resume_position.get("known_seq"), int),
    bool(resume_position.get("known_frontier")),
    resume_response.get("type") == "ack",
    resume_response.get("document_id") == resume_position.get("document_id"),
    resume_response.get("seq") == resume_position.get("known_seq"),
    resume_response.get("frontier") == resume_position.get("known_frontier"),
    resume_evidence.get("confirmation_exact") is True,
))
require("resume-at-head structured evidence is missing or not exact", resume_evidence_exact)

resume_mutation_detected = all((
    resume_mutation_exit != 0,
    expected_tests["resume_at_head_ack"] in resume_mutation_log,
    "test result: FAILED. 0 passed; 1 failed;" in resume_mutation_log,
    "a frame arrives before the timeout" in resume_mutation_log,
))
egress_mutation_detected = all((
    egress_mutation_exit != 0,
    expected_tests["egress_duplicate_reorder"] in egress_mutation_log,
    "test result: FAILED. 0 passed; 1 failed;" in egress_mutation_log,
    "the duplicate accepted must be dropped" in egress_mutation_log,
))
require("empty-resume zero-frame mutation did not make the exact gate test red", resume_mutation_detected)
require("egress duplicate-forward mutation did not make the exact gate test red", egress_mutation_detected)

navigator_documents = navigator_evidence.get("documents") or []
queried_navigators = navigator_evidence.get("queried_document_count")
measured_navigators = navigator_evidence.get("measured_document_count")
document_ids = [item.get("document_id") for item in navigator_documents if isinstance(item, dict)]

def nonnegative_int(value):
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0

navigator_documents_complete = all((
    navigator_evidence.get("schema_version") == "sylvode.flow.navigator-tombstone-evidence.v1",
    nonnegative_int(queried_navigators) and queried_navigators > 0,
    measured_navigators == queried_navigators == len(navigator_documents),
    len(document_ids) == len(navigator_documents),
    len(set(document_ids)) == len(document_ids),
    all(bool(document_id) for document_id in document_ids),
    all(
        nonnegative_int(item.get("live_entry_count"))
        and nonnegative_int(item.get("tombstone_count"))
        and nonnegative_int(item.get("snapshot_bytes"))
        and item.get("snapshot_bytes") > 0
        and item.get("reclamation_signal")
            == (item.get("tombstone_count") > item.get("live_entry_count"))
        for item in navigator_documents
        if isinstance(item, dict)
    ),
))
growth = navigator_evidence.get("growth") or {}
navigator_growth_observed = all((
    navigator_evidence.get("move_out_and_back") is True,
    growth.get("source_document_id") in set(document_ids),
    nonnegative_int(growth.get("before_tombstone_count")),
    nonnegative_int(growth.get("after_tombstone_count")),
    nonnegative_int(growth.get("delta")),
    growth.get("delta", 0) > 0,
    growth.get("after_tombstone_count") > growth.get("before_tombstone_count"),
    growth.get("delta")
        == growth.get("after_tombstone_count") - growth.get("before_tombstone_count"),
    nonnegative_int(growth.get("snapshot_bytes_after")),
    growth.get("snapshot_bytes_after", 0) > 0,
))
tombstone_mutation_detected = all((
    tombstone_mutation_exit != 0,
    expected_tests["navigator_tombstones"] in tombstone_mutation_log,
    "test result: FAILED. 0 passed; 1 failed;" in tombstone_mutation_log,
    "one command removed N entries" in tombstone_mutation_log,
))
navigator_gate = all((
    tests["navigator_tombstones"],
    navigator_documents_complete,
    navigator_growth_observed,
    tombstone_mutation_detected,
))
require("database-authoritative navigator census was incomplete or defaulted", navigator_documents_complete)
require("navigator move-out-and-back did not expose positive tombstone growth", navigator_growth_observed)
require("hidden tombstone-growth mutation did not make the exact gate test red", tombstone_mutation_detected)

environment = load.get("environment") or {}
fixture = load.get("fixture") or {}
concurrency_raw = load.get("concurrency") or {}
lock_raw = load.get("lock") or {}
cache_raw = load.get("cache") or {}
restart_raw = load.get("process_restart") or {}
boundaries = cache_raw.get("cache_boundaries") or {}
rebuild = cache_raw.get("rebuild_after_eviction") or {}
observers = cache_raw.get("observers_and_timers") or {}
no_lost = cache_raw.get("no_accepted_update_lost") or {}
retry = cache_raw.get("retry_exhaustion") or {}
bypass = cache_raw.get("bypass_and_removal") or {}
hold = lock_raw.get("lock_hold_p95") or {}
wait = lock_raw.get("lock_wait") or {}
round_trip = concurrency_raw.get("round_trip") or {}

environment_ok = all((
    load.get("schema_version") == "sylvode.flow.collab-load-v0.5-result.v1",
    load.get("release") == "0.5",
    load.get("source_head") == source_head,
    load.get("source_dirty") is False,
    environment.get("build_profile") == "release",
    environment.get("database_kind") == "real_postgresql",
    bool(environment.get("postgres_version")),
    environment.get("active_other_clients_before") == 0,
    environment.get("concurrent_cargo_before") == 0,
    environment.get("qualification_status") == "satisfied",
    load_exit == 0,
    load.get("passed") is True,
))
require("official release PostgreSQL load evidence is not admissible", environment_ok)
require("source tree was dirty", not dirty)

fixture_ok = all((
    clients == 10,
    fixture.get("clients") == 10,
    fixture.get("distinct_users") is True,
    fixture.get("warmup_rounds") == 5,
    fixture.get("min_measurements") == 30,
    fixture.get("locked") is True,
    bool(fixture.get("fixture_sha256")),
))
require("locked 10-client fixture, five warmups, or minimum sample declaration is wrong", fixture_ok)

lock_flags = {
    "samples_at_least_30": hold.get("samples", 0) >= 30 and wait.get("samples", 0) >= 30,
    "wait_ms_max": wait.get("max_ms", float("inf")) <= 100,
    "hold_ms_p95": hold.get("p95_ms", float("inf")) <= 25,
    "hold_ms_max": hold.get("max_ms", float("inf")) <= 100,
    "timeouts_rolled_back": ((retry.get("lock_wait_exhaustion") or {}).get("canonical_state_unchanged") is True),
    "canonical_state_unchanged": ((retry.get("lock_wait_exhaustion") or {}).get("canonical_state_unchanged") is True),
}
for key, value in lock_flags.items():
    require(f"lock check failed: {key}", value)

expected_total = 10 * (5 + 10)
expected_peer = expected_total * 9
rebase_exhausted = retry.get("head_mismatch_exhaustion") or {}
exhausted_call = rebase_exhausted.get("exhausts_at_the_ceiling") or {}
concurrency_flags = {
    "ten_clients": clients == 10 and fixture.get("clients") == 10,
    "round_trip_samples_at_least_30": round_trip.get("samples", 0) >= 30,
    "round_trip_p95_at_most_250_ms": round_trip.get("p95_ms", float("inf")) <= 250,
    "seq_contiguous": concurrency_raw.get("accepted_total") == expected_total
        and concurrency_raw.get("head_seq_final") == expected_total,
    "fanout_exact": concurrency_raw.get("fanout_rounds_complete") == expected_total
        and concurrency_raw.get("peer_updates_observed") == expected_peer
        and concurrency_raw.get("peer_accepted_observed") == expected_peer,
    "offline_resume_exercised": concurrency_raw.get("resumes_completed", 0) > 0,
    "rebase_exhaustion_rolled_back": exhausted_call.get("forced_head_mismatches") == 3
        and ((exhausted_call.get("victim_outcome") or {}).get("write_state") == "NotApplied"),
}
for key, value in concurrency_flags.items():
    require(f"concurrency check failed: {key}", value)

entry_ceiling = boundaries.get("entry_count_ceiling")
bytes_ceiling = boundaries.get("decoded_bytes_ceiling")
cache_flags = {
    "entry_exact": isinstance(entry_ceiling, int) and boundaries.get("entry_exact_all_resident") is True
        and boundaries.get("entry_exact_resident_count") == entry_ceiling,
    "entry_plus_one_evicted": isinstance(entry_ceiling, int) and boundaries.get("entry_plus_one_lru_reclaimed") is True
        and boundaries.get("entry_plus_one_resident_count") == entry_ceiling,
    "bytes_exact": isinstance(bytes_ceiling, int) and boundaries.get("bytes_exact_equals_ceiling") is True
        and boundaries.get("bytes_exact_total_bytes") == bytes_ceiling,
    "bytes_plus_one_evicted": isinstance(bytes_ceiling, int) and boundaries.get("bytes_plus_one_lru_reclaimed") is True
        and boundaries.get("bytes_plus_one_within_ceiling") is True,
    "idle_ttl_reclaimed": boundaries.get("idle_ttl_alive_before_expiry") is True
        and boundaries.get("idle_ttl_reclaimed") is True
        and boundaries.get("idle_ttl_alive_probe_seconds", float("inf")) < boundaries.get("idle_ttl_seconds", -1)
        < boundaries.get("idle_ttl_reclaim_probe_seconds", -1),
    "restart_hash_equal": rebuild.get("semantic_hash_equal") is True
        and rebuild.get("head_equal") is True and restart_raw.get("semantic_hash_equal") is True
        and restart_raw.get("distinct_processes") is True
        and restart_raw.get("first_process_observed_dead") is True
        and restart_raw.get("build_profile") == "release",
    "observers_after_destroy": observers.get("cycle_original_entries_remaining") == 0
        and observers.get("open_fds_delta") == 0
        and observers.get("engine_observer_api_exists_in_collab_core") is False,
    "timers_after_destroy": observers.get("os_threads_delta") == 0
        and observers.get("static_background_constructs_in_cache_and_coordinator") == [],
}
for key, value in cache_flags.items():
    require(f"cache check failed: {key}", value)

cache_no_loss = set(no_lost.get("conditions_exercised") or []) >= {"miss", "warmup", "hit", "eviction", "restart"} \
    and no_lost.get("seq_contiguous_from_one") is True \
    and no_lost.get("collab_updates_rows") == no_lost.get("accepted_total")
row_lock_authority = ((bypass.get("row_lock_negative_control") or {}).get("row_lock_is_the_authority") is True)
require("cache conditions did not preserve all accepted updates", cache_no_loss)
require("row-lock negative control did not prove authority", row_lock_authority)

snapshot = {
    "soft_count_triggered": tests["soft_count"] and tests["soft_rebuild"],
    "soft_bytes_triggered": tests["soft_bytes"],
    "rebuild_triggered": tests["soft_rebuild"],
    "hard_count_blocked": tests["hard_count"] and tests["hard_checkpoint"],
    "hard_bytes_blocked": tests["hard_bytes"],
    "checkpoint_head_race_safe": tests["checkpoint_race"],
    "restart_hash_equal": tests["snapshot_restart"] and cache_flags["restart_hash_equal"],
}
for key, value in snapshot.items():
    require(f"snapshot check failed: {key}", value)

egress = {
    "duplicates_ignored": tests["egress_duplicate_reorder"],
    "reorder_monotonic": tests["egress_duplicate_reorder"],
    "gap_resync": tests["egress_gap_resync"],
    "saved_never_crossed_gap": tests["egress_gap_resync"],
    "resume_at_head_ack": tests["resume_at_head_ack"] and resume_evidence_exact,
    "resume_request_position": resume_position,
    "resume_first_observable_frame": resume_response,
}

surface_violations = surface.get("violations") or {}
surface_ok = surface.get("passed") is True and all(not values for values in surface_violations.values())
require("surface contract fixed-schema checks failed", surface_ok)
for required_hash in ("rest_sha256", "mcp_sha256", "cli_sha256", "ui_sha256", "matrix_sha256"):
    require(f"contract hash missing: {required_hash}", bool((surface.get("contracts") or {}).get(required_hash)))

budget_gate = environment_ok and fixture_ok and all(lock_flags.values()) and all(concurrency_flags.values())
egress_gate = tests["inbound_reorder"] and all(egress[key] for key in (
    "duplicates_ignored", "reorder_monotonic", "gap_resync", "saved_never_crossed_gap", "resume_at_head_ack"
)) and resume_mutation_detected and egress_mutation_detected
cache_gate = all(cache_flags.values()) and cache_no_loss and row_lock_authority
snapshot_gate = all(snapshot.values())

hard_gates = {
    "ten_client_round_trip_and_lock_budget_no_regression": {
        "status": "passed" if budget_gate else "failed",
        "reason": "independently recomputed from release PostgreSQL distributions and exact 10-client fan-out counts",
    },
    "accepted_egress_duplicate_reorder_gap_resync": {
        "status": "passed" if egress_gate else "failed",
        "reason": "separate controlled inbound reorder and outbound duplicate/reorder/gap/resume-at-head injections",
    },
    "warm_cache_eviction_restart_semantic_equivalence": {
        "status": "passed" if cache_gate else "failed",
        "reason": "exact cache boundaries plus fresh release API process re-exec semantic equality",
    },
    "navigator_tombstone_growth_measured": {
        "status": "passed" if navigator_gate else "failed",
        "reason": "database-authoritative complete navigator census plus move-out-and-back growth and mutation-red proof",
    },
}

adr_quantitative_gates = {
    "lock_budget": all(lock_flags.values()),
    "ten_client_round_trip_and_no_lost_update": all(concurrency_flags.values()) and cache_no_loss,
    "bounded_cache_and_restart": cache_gate,
    "snapshot_bounds_and_recovery": snapshot_gate,
    "cache_coordinator_are_performance_only_db_lock_is_authority": row_lock_authority,
}
for key, value in adr_quantitative_gates.items():
    require(f"ADR-0010 quantitative gate failed: {key}", value)
require("dynamic collaboration test command failed", dynamic_exit == 0)

result = {
    "schema_version": "sylvode.flow.collab-architecture-result.v1",
    "release": "0.5",
    "source_head": source_head,
    "source_tree_dirty": dirty,
    "adr_sha256": adr_sha,
    "limits_sha256": limits_sha,
    "generated_at": generated_at,
    "executor": executor,
    "environment": {
        "postgres_version": environment.get("postgres_version"),
        "cpu": environment.get("cpu"),
        "cores": environment.get("cores"),
        "ram": environment.get("ram_bytes"),
        "engine": "loro",
        "build_profile": environment.get("build_profile"),
        "fixture_sha256": fixture.get("fixture_sha256"),
        "load_average_before": environment.get("load_average_before"),
        "load_average_after": environment.get("load_average_after"),
        "active_other_clients_before": environment.get("active_other_clients_before"),
        "concurrent_cargo_before": environment.get("concurrent_cargo_before"),
        "database_kind": environment.get("database_kind"),
    },
    "cache": cache_flags,
    "cache_evidence": {
        "available": bool(cache_raw),
        "cache": cache_flags,
        "supplemental_checks": {
            "cache_conditions_no_lost_update": cache_no_loss,
            "row_lock_is_authority": row_lock_authority,
            "fresh_release_api_reexec": restart_raw.get("fresh_v0_5_run") is True
                and restart_raw.get("build_profile") == "release",
        },
        "violations": [item for item in problems if item.startswith("cache check failed:")],
        "passed": cache_gate,
    },
    "lock": {
        "samples": hold.get("samples"),
        "wait_ms_max": wait.get("max_ms"),
        "hold_ms_p95": hold.get("p95_ms"),
        "hold_ms_max": hold.get("max_ms"),
        "timeouts_rolled_back": lock_flags["timeouts_rolled_back"],
        "canonical_state_unchanged": lock_flags["canonical_state_unchanged"],
    },
    "concurrency": {
        "clients": clients,
        "warmup_rounds": fixture.get("warmup_rounds"),
        "round_trip_samples": round_trip.get("samples"),
        "accepted_round_trip_ms_p95": round_trip.get("p95_ms"),
        "seq_contiguous": concurrency_flags["seq_contiguous"],
        "rebase_exhaustion_rolled_back": concurrency_flags["rebase_exhaustion_rolled_back"],
        "fanout_rounds": concurrency_raw.get("fanout_rounds_complete"),
        "peer_update_frames": concurrency_raw.get("peer_updates_observed"),
        "peer_accepted_frames": concurrency_raw.get("peer_accepted_observed"),
        "offline_resumes": concurrency_raw.get("resumes_completed"),
    },
    "snapshot": snapshot,
    "inbound": {
        "update_reorder_injected": tests["inbound_reorder"],
        "barrier": "both concurrent updates generated before reverse-order sends; ping after each accepted",
    },
    "egress": egress,
    "navigator_tombstone_growth_measured": {
        "status": "passed" if navigator_gate else "failed",
        "authoritative_enumeration": {
            "queried_document_count": queried_navigators,
            "measured_document_count": measured_navigators,
            "documents": navigator_documents,
            "complete": navigator_documents_complete,
        },
        "move_out_and_back": navigator_evidence.get("move_out_and_back"),
        "growth": growth,
        "growth_observed": navigator_growth_observed,
        "reclamation_signal_is_non_rejecting": True,
        "passed": navigator_gate,
    },
    "falsification": {
        "empty_resume_zero_frames": {
            "mutation_log": os.path.abspath(resume_mutation_log_path),
            "protection_disabled_by": "OPENPR_FLOW_TEST_MUTATION_EMPTY_RESUME_ZERO_FRAMES=1",
            "same_test_replayed": expected_tests["resume_at_head_ack"],
            "exit_code": resume_mutation_exit,
            "duration_ms": resume_mutation_duration_ms,
            "observed": "no first response before timeout",
            "gate_went_red": resume_mutation_detected,
        },
        "egress_duplicate_forwarded": {
            "mutation_log": os.path.abspath(egress_mutation_log_path),
            "protection_disabled_by": "OPENPR_FLOW_TEST_MUTATION_FORWARD_EGRESS_DUPLICATE=1",
            "same_test_replayed": expected_tests["egress_duplicate_reorder"],
            "exit_code": egress_mutation_exit,
            "duration_ms": egress_mutation_duration_ms,
            "observed": "duplicate Accepted crossed the sequencer before the marker",
            "gate_went_red": egress_mutation_detected,
        },
        "navigator_tombstone_growth_hidden": {
            "mutation_log": os.path.abspath(tombstone_mutation_log_path),
            "protection_disabled_by": "OPENPR_FLOW_TEST_MUTATION_HIDE_TOMBSTONE_GROWTH=1",
            "same_test_replayed": expected_tests["navigator_tombstones"],
            "exit_code": tombstone_mutation_exit,
            "duration_ms": tombstone_mutation_duration_ms,
            "observed": "known positive N-node growth was reported as zero",
            "gate_went_red": tombstone_mutation_detected,
        },
    },
    "process_restart": restart_raw,
    "contracts": surface.get("contracts") or {},
    "counts": surface.get("counts") or {},
    "violations": surface_violations,
    "verification_violations": problems,
    "adr_quantitative_gates": adr_quantitative_gates,
    "numeric_budgets_check": {
        "load_harness": {
            "available": bool(load),
            "official_environment": {
                "required": "release build on the dispatch-fixed, uncontended real PostgreSQL endpoint",
                "qualification_status": environment.get("qualification_status"),
                "active_other_clients_at_preflight": environment.get("active_other_clients_before"),
                "shared_database_measurements_accepted": False,
            },
            "official_environment_ok": environment_ok,
            "budget_gate_passed": budget_gate,
            "parity_gate_passed": ((load.get("components") or {}).get("lock_load") or {}).get("passed") is True,
            "violations": [item for item in problems if item.startswith("lock check failed:")
                or item.startswith("concurrency check failed:")
                or item.startswith("official release PostgreSQL")],
        },
    },
    "hard_gates": hard_gates,
    "gates": hard_gates,
    "inputs": {
        "load_harness_evidence": os.path.abspath(load_path),
        "load_harness_sha256": hashlib.sha256(open(load_path, "rb").read()).hexdigest() if os.path.isfile(load_path) else None,
        "dynamic_test_log": os.path.abspath(test_log_path),
        "navigator_tombstone_evidence": os.path.abspath(navigator_evidence_path),
    },
}
result["passed"] = not problems and surface_ok and snapshot_gate and all(
    gate["status"] == "passed" for gate in hard_gates.values()
)
os.makedirs(os.path.dirname(output_path), exist_ok=True)
with open(output_path + ".tmp", "w", encoding="utf-8") as handle:
    json.dump(result, handle, indent=2, sort_keys=True)
    handle.write("\n")
os.replace(output_path + ".tmp", output_path)
print(json.dumps(result, indent=2, sort_keys=True))
raise SystemExit(0 if result["passed"] else 1)
PY
  V05_RESULT_EXIT=$?
  set -e
  exit "$V05_RESULT_EXIT"
fi

# The load run is intentionally separate from the cheap architecture checks: it needs a release
# build, PostgreSQL statement logging, and an uncontended dedicated database instance. This block
# validates the harness artifact rather than trusting its top-level `passed` bit. The declared
# dedicated container must match the server-log authority recorded by the artifact; omitting that
# declaration keeps both load-dependent gates red.
HARNESS_CHECK_JSON="$(python3 - "$LOAD_HARNESS_EVIDENCE" "$DEDICATED_PG_CONTAINER" <<'PY'
import json
import os
import sys
import hashlib
from collections import defaultdict

path, dedicated_container = sys.argv[1:3]
if not path:
    print(json.dumps({
        "available": False,
        "evidence_path": None,
        "official_environment_ok": False,
        "budget_gate_passed": False,
        "parity_gate_passed": False,
        "violations": ["no --load-harness-evidence was supplied"],
    }))
    raise SystemExit(0)
if not os.path.isfile(path):
    print(json.dumps({"error": f"load harness evidence does not exist: {path}"}))
    raise SystemExit(0)
try:
    raw = json.load(open(path, encoding="utf-8"))
except (OSError, json.JSONDecodeError) as exc:
    print(json.dumps({"error": f"load harness evidence is not valid JSON: {exc}"}))
    raise SystemExit(0)
if raw.get("schema_version") == "sylvode.flow.collab-load-harness-environment.v1":
    gate = raw.get("environment_gate") or {}
    execution = raw.get("execution") or {}
    detail = gate.get("detail") or "load harness environment prerequisite was not satisfied"
    print(json.dumps({
        "available": True,
        "evidence_path": os.path.abspath(path),
        "evidence_sha256": hashlib.sha256(open(path, "rb").read()).hexdigest(),
        "execution_status": execution.get("status"),
        "official_environment": {
            "required": "release build on an explicitly declared, uncontended dedicated PostgreSQL container",
            "declared_dedicated_pg_container": gate.get("declared_dedicated_pg_container"),
            "recorded_pg_log_container": gate.get("pg_log_container"),
            "qualification_status": gate.get("status"),
            "reason_code": gate.get("reason_code"),
            "active_other_clients_at_preflight": gate.get("active_other_clients"),
            "shared_database_measurements_accepted": False,
        },
        "official_environment_ok": False,
        "budget_checks": {},
        "parity_checks": {},
        "surface_comparisons": 0,
        "divergent_heads": [],
        "budget_gate_passed": False,
        "parity_gate_passed": False,
        "measurements": {},
        "violations": [f"load harness environment not satisfied: {detail}"],
    }))
    raise SystemExit(0)
if raw.get("schema_version") != "sylvode.flow.collab-load-harness.v1":
    print(json.dumps({"error": "load harness evidence has the wrong schema_version"}))
    raise SystemExit(0)

env = raw.get("environment") or {}
ten = raw.get("10_client") or {}
lock = raw.get("lock") or {}
parity = raw.get("bootstrap_parity") or {}
budgets = raw.get("budgets") or {}
round_trip = ten.get("round_trip_p95") or {}
hold = lock.get("lock_hold_p95") or {}
wait = lock.get("lock_wait") or {}
gap = lock.get("intra_lock_app_gap") or {}
reconstruction = lock.get("reconstruction") or {}
observations = parity.get("observations") or []
violations = []

actual_container = env.get("pg_log_container")
official_environment_ok = bool(
    dedicated_container
    and actual_container == dedicated_container
    and env.get("declared_dedicated_pg_container") == dedicated_container
    and env.get("qualification_status") == "satisfied"
    and env.get("active_other_clients_at_preflight") == 0
    and env.get("shared_database_measurements_accepted") is False
    and env.get("build_profile") == "release"
    and isinstance(env.get("postgres_version"), str)
    and env.get("postgres_version")
    and "postgresql server statement log" in str(env.get("measurement_authority", "")).lower()
)
if not dedicated_container:
    violations.append("official run requires --dedicated-pg-container; shared PostgreSQL measurements are inadmissible")
elif actual_container != dedicated_container:
    violations.append(
        f"declared dedicated PostgreSQL container {dedicated_container!r} does not match evidence {actual_container!r}"
    )
if env.get("qualification_status") != "satisfied":
    violations.append("load harness artifact lacks a satisfied preflight environment qualification")
if env.get("declared_dedicated_pg_container") != dedicated_container:
    violations.append("load harness preflight declaration does not match --dedicated-pg-container")
if env.get("active_other_clients_at_preflight") != 0:
    violations.append("load harness preflight did not prove an uncontended dedicated PostgreSQL instance")
if env.get("shared_database_measurements_accepted") is not False:
    violations.append("load harness artifact does not explicitly reject shared-database measurements")
if env.get("build_profile") != "release":
    violations.append("load harness evidence is not from a release build")
if not official_environment_ok and dedicated_container and actual_container == dedicated_container:
    violations.append("load harness environment lacks PostgreSQL version or server-log measurement authority")

min_samples = budgets.get("min_samples")
budget_checks = {
    "ten_clients": ten.get("clients") == 10,
    "round_trip_samples": isinstance(min_samples, (int, float)) and round_trip.get("samples", 0) >= min_samples,
    "round_trip_p95": round_trip.get("p95_ms", float("inf")) <= budgets.get("round_trip_p95_ms_max", -1),
    "lock_hold_samples": isinstance(min_samples, (int, float)) and hold.get("samples", 0) >= min_samples,
    "lock_hold_p95": hold.get("p95_ms", float("inf")) <= budgets.get("lock_hold_p95_ms_max", -1),
    "lock_hold_single": hold.get("max_ms", float("inf")) <= budgets.get("lock_hold_single_ms_max", -1),
    "lock_wait_single": wait.get("max_ms", float("inf")) <= budgets.get("lock_wait_ms_max", -1),
    "intra_lock_app_gap_p95": gap.get("p95_ms", float("inf")) <= budgets.get("intra_lock_app_gap_ms_max", -1),
    "committed_write_coverage": lock.get("committed_write_transactions") == ten.get("accepted_total"),
    "locked_phase_statement_allowlist_exact": len(lock.get("locked_phase_statement_inventory") or []) == 11,
    "statement_log_resolved": reconstruction.get("unresolved_statements") == 0
        and lock.get("unterminated_transactions") == 0,
}

identity_fields = ("wire_hash", "semantic_hash", "head_frontier", "snapshot_seq")
by_head = defaultdict(list)
observation_shapes_ok = True
for item in observations:
    if not isinstance(item, dict):
        observation_shapes_ok = False
        continue
    if item.get("tail_contiguous") is not True or item.get("frontier_equivalent_to_replay") is not True:
        observation_shapes_ok = False
    by_head[item.get("head_seq")].append(item)
surface_comparisons = 0
divergent_heads = []
for head, group in by_head.items():
    surfaces = {item.get("surface") for item in group}
    if {"rest_bootstrap", "ws_snapshot"}.issubset(surfaces):
        surface_comparisons += 1
        identities = {tuple(item.get(field) for field in identity_fields) for item in group}
        if len(identities) != 1:
            divergent_heads.append(head)

parity_checks = {
    "seq_contiguous": ten.get("seq_contiguous") is True,
    "head_matches_accepted": ten.get("head_seq") == ten.get("accepted_total") and ten.get("accepted_total", 0) > 0,
    "bootstrap_repeatable_read_read_only": parity.get("bootstrap_transactions", 0) > 0
        and parity.get("bootstrap_transactions") == parity.get("repeatable_read_read_only_transactions"),
    "observation_count": len(observations) >= 4,
    "tail_frontier_observations": observation_shapes_ok,
    "rest_ws_comparisons": surface_comparisons > 0,
    "rest_ws_divergence_zero": not divergent_heads,
}

raw_integrity_ok = raw.get("passed") is True and raw.get("violations") == []
budget_gate_passed = official_environment_ok and raw_integrity_ok and all(budget_checks.values())
parity_gate_passed = official_environment_ok and raw_integrity_ok and all(parity_checks.values())
for name, ok in {**budget_checks, **parity_checks}.items():
    if not ok:
        violations.append(f"harness check failed: {name}")
if not raw_integrity_ok:
    violations.append("harness self-verdict is not passed with an empty violations array")

print(json.dumps({
    "available": True,
    "evidence_path": os.path.abspath(path),
    "evidence_sha256": hashlib.sha256(open(path, "rb").read()).hexdigest(),
    "official_environment": {
        "required": "release build on an explicitly declared dedicated PostgreSQL container",
        "declared_dedicated_pg_container": dedicated_container or None,
        "recorded_pg_log_container": actual_container,
        "build_profile": env.get("build_profile"),
        "postgres_version": env.get("postgres_version"),
        "measurement_authority": env.get("measurement_authority"),
        "qualification_status": env.get("qualification_status"),
        "active_other_clients_at_preflight": env.get("active_other_clients_at_preflight"),
        "shared_database_measurements_accepted": False,
    },
    "official_environment_ok": official_environment_ok,
    "budget_checks": budget_checks,
    "parity_checks": parity_checks,
    "surface_comparisons": surface_comparisons,
    "divergent_heads": divergent_heads,
    "budget_gate_passed": budget_gate_passed,
    "parity_gate_passed": parity_gate_passed,
    "measurements": {
        "round_trip": round_trip,
        "lock_hold": hold,
        "lock_wait": wait,
        "intra_lock_app_gap": gap,
        "commit_wal_flush": lock.get("commit_wal_flush"),
        "accepted_total": ten.get("accepted_total"),
        "head_seq": ten.get("head_seq"),
        "committed_write_transactions": lock.get("committed_write_transactions"),
        "locked_phase_statement_count": len(lock.get("locked_phase_statement_inventory") or []),
        "bootstrap_transactions": parity.get("bootstrap_transactions"),
        "repeatable_read_read_only_transactions": parity.get("repeatable_read_read_only_transactions"),
        "parity_observations": len(observations),
    },
    "violations": violations,
}))
PY
)"
if ! jq -e . >/dev/null 2>&1 <<<"$HARNESS_CHECK_JSON"; then
  echo "FAIL: load harness evidence parser did not produce valid JSON" >&2
  exit 2
fi
if jq -e 'has("error")' >/dev/null 2>&1 <<<"$HARNESS_CHECK_JSON"; then
  echo "FAIL: $(jq -r '.error' <<<"$HARNESS_CHECK_JSON")" >&2
  exit 2
fi
echo "=== load harness evidence ===" >&2
jq -r '"  available=\(.available) dedicated_environment=\(.official_environment_ok) budget_gate=\(.budget_gate_passed) parity_gate=\(.parity_gate_passed)"' <<<"$HARNESS_CHECK_JSON" >&2
jq -r '.violations[] | "  VIOLATION: " + .' <<<"$HARNESS_CHECK_JSON" >&2

# Recompute ADR-0010's fixed eight-field cache block from the raw harness
# observations.  The harness's own `passed` bit is an input integrity check,
# never a substitute for these field-by-field predicates.
CACHE_CHECK_JSON="$(python3 - "$CACHE_EVIDENCE" <<'PY'
import hashlib, json, os, sys
path = sys.argv[1]
if not path:
    print(json.dumps({"available": False, "evidence_path": None, "source_head": None, "source_tree_dirty": None,
        "supplemental_checks": {k: False for k in ("cache_conditions_no_lost_update","retry_exhaustion_rolled_back","bypass_unique_seq")},
        "violations": ["no --cache-evidence was supplied"], "passed": False,
        "cache": {k: False for k in ("entry_exact","entry_plus_one_evicted","bytes_exact","bytes_plus_one_evicted","idle_ttl_reclaimed","restart_hash_equal","observers_after_destroy","timers_after_destroy")}}))
    raise SystemExit
if not os.path.isfile(path):
    print(json.dumps({"error": f"cache evidence does not exist: {path}"})); raise SystemExit
try: raw = json.load(open(path, encoding="utf-8"))
except (OSError, json.JSONDecodeError) as exc:
    print(json.dumps({"error": f"cache evidence is not valid JSON: {exc}"})); raise SystemExit
if raw.get("schema_version") != "sylvode.flow.collab-cache-evidence.v1":
    print(json.dumps({"error": "cache evidence has the wrong schema_version"})); raise SystemExit
b = raw.get("cache_boundaries") or {}; o = raw.get("observers_and_timers") or {}
r = raw.get("rebuild_after_eviction") or {}; n = raw.get("no_accepted_update_lost") or {}
x = raw.get("retry_exhaustion") or {}; y = raw.get("bypass_and_removal") or {}
ce = b.get("entry_count_ceiling"); bc = b.get("decoded_bytes_ceiling")
ce_ok = isinstance(ce, int) and ce > 0
bc_ok = isinstance(bc, int) and bc > 0
cache = {
 "entry_exact": ce_ok and b.get("entry_exact_all_resident") is True and b.get("entry_exact_resident_count") == ce,
 "entry_plus_one_evicted": ce_ok and b.get("entry_plus_one_lru_reclaimed") is True and b.get("entry_plus_one_newest_resident") is True and b.get("entry_plus_one_resident_count") == ce and b.get("entry_plus_one_non_lru_survivors") == ce - 1,
 "bytes_exact": bc_ok and b.get("bytes_exact_equals_ceiling") is True and b.get("bytes_exact_total_bytes") == bc,
 "bytes_plus_one_evicted": bc_ok and b.get("bytes_plus_one_lru_reclaimed") is True and b.get("bytes_plus_one_within_ceiling") is True and isinstance(b.get("bytes_plus_one_total_bytes"), int) and b.get("bytes_plus_one_total_bytes") <= bc,
 "idle_ttl_reclaimed": b.get("idle_ttl_alive_before_expiry") is True and b.get("idle_ttl_reclaimed") is True and b.get("idle_ttl_alive_probe_seconds", 0) < b.get("idle_ttl_seconds", 0) < b.get("idle_ttl_reclaim_probe_seconds", 0),
 "restart_hash_equal": r.get("semantic_hash_equal") is True and r.get("head_equal") is True and n.get("post_restart_rebuilt_head_seq") == n.get("canonical_head_seq") == n.get("accepted_total") and bool(n.get("post_restart_rebuilt_semantic_hash")),
 "observers_after_destroy": o.get("cycle_original_entries_remaining") == 0 and o.get("open_fds_delta") == 0 and o.get("engine_observer_api_exists_in_collab_core") is False,
 "timers_after_destroy": o.get("os_threads_delta") == 0 and o.get("static_background_constructs_in_cache_and_coordinator") == [],
}
supplemental = {
 "cache_conditions_no_lost_update": set(n.get("conditions_exercised") or []) >= {"miss","warmup","hit","eviction","restart"} and n.get("seq_contiguous_from_one") is True and n.get("collab_updates_rows") == n.get("accepted_total"),
 "retry_exhaustion_rolled_back": (x.get("lock_wait_exhaustion") or {}).get("canonical_state_unchanged") is True and not (x.get("head_mismatch_exhaustion") or {}).get("rejected_victims_that_left_a_row"),
 "bypass_unique_seq": (y.get("cache_and_coordinator_deleted") or {}).get("seqs_unique") is True and y.get("correctness_unchanged_without_cache_or_coordinator") is True and (y.get("row_lock_negative_control") or {}).get("row_lock_is_the_authority") is True,
}
violations = [f"cache check failed: {k}" for k,v in {**cache, **supplemental}.items() if not v]
if raw.get("passed") is not True or raw.get("violations") != []: violations.append("cache harness self-verdict is not passed with an empty violations array")
print(json.dumps({"available": True, "evidence_path": os.path.abspath(path), "evidence_sha256": hashlib.sha256(open(path,"rb").read()).hexdigest(), "source_head": raw.get("source_head"), "source_tree_dirty": raw.get("source_tree_dirty"), "cache": cache, "supplemental_checks": supplemental, "violations": violations, "passed": not violations}))
PY
)"
if ! jq -e . >/dev/null 2>&1 <<<"$CACHE_CHECK_JSON" || jq -e 'has("error")' >/dev/null 2>&1 <<<"$CACHE_CHECK_JSON"; then
  echo "FAIL: $(jq -r '.error // "cache evidence parser produced invalid JSON"' <<<"$CACHE_CHECK_JSON" 2>/dev/null || true)" >&2
  exit 2
fi
echo "=== cache evidence: available=$(jq -r .available <<<"$CACHE_CHECK_JSON") passed=$(jq -r .passed <<<"$CACHE_CHECK_JSON") ===" >&2
jq -r '.violations[] | "  VIOLATION: " + .' <<<"$CACHE_CHECK_JSON" >&2

# ---- 1+2. static checks: ADR status + frozen numeric budgets ----
STATIC_JSON="$(python3 - "$ADR_PATH" "$LIMITS_RS" <<'PY'
import json
import re
import sys

adr_path, limits_rs = sys.argv[1], sys.argv[2]
adr_text = open(adr_path, encoding="utf-8").read()

sm = re.search(r"^-\s*状态：\s*(.+?)\s*$", adr_text, re.M)
if not sm:
    print(json.dumps({"error": "could not find '- 状态：<status>' line in " + adr_path}))
    sys.exit(0)
status_text = sm.group(1).strip()
status_match = re.match(r"^(?:\*\*)?([A-Za-z][A-Za-z_-]*)(?:\*\*)?(?:\s*[（(].*)?$", status_text)
if not status_match:
    print(json.dumps({"error": f"could not parse ADR status from {status_text!r} in {adr_path}"}))
    sys.exit(0)
adr_status = status_match.group(1)

def find_int(pattern):
    m = re.search(pattern, adr_text)
    return int(m.group(1)) if m else None

max_rebase_attempts_adr = find_int(r"最多\s*(\d+)\s*次锁外\s*rebase")
ceiling_m = re.search(r"lock wait 超过\s*(\d+)\s*ms 或 lock hold 达到\s*(\d+)\s*ms", adr_text)
lock_wait_ceiling_ms_adr = int(ceiling_m.group(1)) if ceiling_m else None
lock_hold_ceiling_ms_adr = int(ceiling_m.group(2)) if ceiling_m else None
lock_hold_p95_ms_adr = find_int(r"hold p95\s*不超过\s*(\d+)\s*ms")
round_trip_p95_ms_adr = find_int(r"round-trip p95\s*不超过\s*(\d+)\s*ms")

limits_text = open(limits_rs, encoding="utf-8").read()

def find_const(name):
    m = re.search(rf"pub const {name}:\s*\w+\s*=\s*([\d_]+)", limits_text)
    return int(m.group(1).replace("_", "")) if m else None

document_lock_wait_ms_max = find_const("DOCUMENT_LOCK_WAIT_MS_MAX")
document_lock_hold_ms_max = find_const("DOCUMENT_LOCK_HOLD_MS_MAX")
max_rebase_attempts_src = find_const("MAX_REBASE_ATTEMPTS")
warm_cache_constants = {
    name: find_const(name)
    for name in (
        "WARM_CACHE_DOCUMENTS_MAX",
        "WARM_CACHE_DECODED_BYTES_MAX",
        "WARM_CACHE_ENTRY_DECODED_BYTES_MAX",
        "WARM_CACHE_IDLE_TTL_SECONDS",
    )
}

violations = []
if lock_wait_ceiling_ms_adr is not None and lock_wait_ceiling_ms_adr != document_lock_wait_ms_max:
    violations.append(f"ADR-0010 freezes lock-wait ceiling={lock_wait_ceiling_ms_adr}ms but DOCUMENT_LOCK_WAIT_MS_MAX={document_lock_wait_ms_max}")
if lock_hold_ceiling_ms_adr is not None and lock_hold_ceiling_ms_adr != document_lock_hold_ms_max:
    violations.append(f"ADR-0010 freezes lock-hold ceiling={lock_hold_ceiling_ms_adr}ms but DOCUMENT_LOCK_HOLD_MS_MAX={document_lock_hold_ms_max}")
if max_rebase_attempts_adr is not None and max_rebase_attempts_adr != max_rebase_attempts_src:
    violations.append(f"ADR-0010 freezes max_rebase_attempts={max_rebase_attempts_adr} but MAX_REBASE_ATTEMPTS={max_rebase_attempts_src}")
for name, val in warm_cache_constants.items():
    if val is None:
        violations.append(f"expected warm-cache constant {name} not found in {limits_rs}")

print(json.dumps({
    "adr_status": adr_status,
    "adr_status_required_for_candidate": "Accepted",
    "adr_accepted": adr_status == "Accepted",
    "frozen_budgets": {
        "lock_wait_ceiling_ms": {"adr": lock_wait_ceiling_ms_adr, "source_const": document_lock_wait_ms_max},
        "lock_hold_ceiling_ms": {"adr": lock_hold_ceiling_ms_adr, "source_const": document_lock_hold_ms_max},
        "max_rebase_attempts": {"adr": max_rebase_attempts_adr, "source_const": max_rebase_attempts_src},
        "warm_cache_constants_present": warm_cache_constants,
    },
    "load_test_targets": {
        "lock_hold_p95_ms": lock_hold_p95_ms_adr,
        "round_trip_p95_ms_10_clients": round_trip_p95_ms_adr,
        "reason": "frozen distribution targets independently re-evaluated from the supplied load-harness evidence",
    },
    "constant_cross_check_violations": violations,
}))
PY
)"

if ! jq -e . >/dev/null 2>&1 <<<"$STATIC_JSON"; then
  echo "FAIL: static ADR/limits parser did not produce valid JSON" >&2
  echo "$STATIC_JSON" >&2
  exit 2
fi
if jq -e 'has("error")' >/dev/null 2>&1 <<<"$STATIC_JSON"; then
  echo "FAIL: $(jq -r '.error' <<<"$STATIC_JSON")" >&2
  exit 2
fi

ADR_STATUS="$(jq -r '.adr_status' <<<"$STATIC_JSON")"
ADR_ACCEPTED="$(jq -r '.adr_accepted' <<<"$STATIC_JSON")"
CONSTANT_VIOLATION_COUNT="$(jq '.constant_cross_check_violations | length' <<<"$STATIC_JSON")"
CONSTANTS_PASSED=$([[ "$CONSTANT_VIOLATION_COUNT" -eq 0 ]] && echo true || echo false)

echo "=== ADR-0010 status: $ADR_STATUS (required for candidate: Accepted) ===" >&2
echo "=== frozen numeric budgets: ADR-0010 text vs apps/api/src/flow/collab/limits.rs ===" >&2
jq -r '.frozen_budgets | to_entries[] | select(.key != "warm_cache_constants_present") | "  \(.key): adr=\(.value.adr // .value) source_const=\(.value.source_const // "n/a")"' <<<"$STATIC_JSON" >&2
if [[ "$CONSTANT_VIOLATION_COUNT" -gt 0 ]]; then
  jq -r '.constant_cross_check_violations[] | "  VIOLATION: " + .' <<<"$STATIC_JSON" >&2
fi
echo "=== frozen load-test distribution targets ===" >&2
jq -r '.load_test_targets | "  lock_hold_p95_ms=\(.lock_hold_p95_ms) round_trip_p95_ms_10_clients=\(.round_trip_p95_ms_10_clients)"' <<<"$STATIC_JSON" >&2
echo "=== load-generation harness authority: supplied evidence JSON (no source-string proxy) ===" >&2
echo "  evidence available: $(jq -r '.available' <<<"$HARNESS_CHECK_JSON")" >&2
echo "  official environment qualified: $(jq -r '.official_environment_ok' <<<"$HARNESS_CHECK_JSON")" >&2

# ---- 3. "Gate <N> `<id>`" marker extraction from snapshot.rs (dynamic, re-parsed every run) ----
MARKERS_JSON="$(python3 - "$SNAPSHOT_RS" <<'PY'
import json
import re
import sys

path = sys.argv[1]
text = open(path, encoding="utf-8").read()

# Doc-comment shape: "/// Gate 7 `minimal_snapshot_advancement_bounds_tail`, ..."
# possibly followed by more doc-comment lines, then a #[tokio::test]/#[test]
# attribute, then the `async fn NAME` / `fn NAME` line. A second shape,
# "/// Gate 9 groundwork: ...", names a gate NUMBER but deliberately no
# hard_gate id (self-disclaiming full coverage) -- captured separately into
# `groundwork` so callers can report the test without ever attributing it
# to a hard_gate id the source itself did not claim.
marker_re = re.compile(r"Gate\s+(\d+)\s+`([a-z_0-9]+)`")
groundwork_re = re.compile(r"Gate\s+(\d+)\s+groundwork", re.I)
fn_re = re.compile(r"(?:async fn|fn)\s+(\w+)\(")

def next_fn(start_line):
    for j in range(start_line, min(start_line + 40, len(lines))):
        fm = fn_re.search(lines[j])
        if fm:
            return fm.group(1)
    return None

lines = text.split("\n")
by_gate = {}
notes = {}
groundwork = {}
for i, line in enumerate(lines):
    m = marker_re.search(line)
    if m:
        gate_id = m.group(2)
        target = next_fn(i)
        if target is not None:
            by_gate.setdefault(gate_id, [])
            if target not in by_gate[gate_id]:
                by_gate[gate_id].append(target)
        if "groundwork" in line.lower() or "groundwork" in " ".join(lines[i:i+3]).lower():
            notes[gate_id] = "source comment explicitly marks this as groundwork, not the full gate fixture"
        continue
    gm = groundwork_re.search(line)
    if gm:
        target = next_fn(i)
        if target is not None:
            groundwork[gm.group(1)] = target

print(json.dumps({"by_gate": by_gate, "notes": notes, "groundwork": groundwork}))
PY
)"

if ! jq -e . >/dev/null 2>&1 <<<"$MARKERS_JSON"; then
  echo "FAIL: Gate-marker parser did not produce valid JSON" >&2
  exit 2
fi
TOTAL_MAPPED="$(jq '[.by_gate[] | length] | add // 0' <<<"$MARKERS_JSON")"
if [[ "$TOTAL_MAPPED" -eq 0 ]]; then
  echo "FAIL: zero tests attributed to any 'Gate <N>' marker in $SNAPSHOT_RS -- refusing to write a vacuous result" >&2
  exit 2
fi

echo "=== 'Gate <N> \`<id>\`' markers found in $SNAPSHOT_RS ===" >&2
jq -r '.by_gate | to_entries[] | "  \(.key): \(.value | join(", "))"' <<<"$MARKERS_JSON" >&2

# ---- cross-check every mapped test exists in the LIVE test binary ----
LIVE_LIST_LOG="$EVIDENCE_ROOT/logs/collab.snapshot_list.log"
if ! ( cd "$REPO_ROOT" && cargo test -p api --lib flow::collab::snapshot:: -- --list ) > "$LIVE_LIST_LOG" 2>&1; then
  echo "FAIL: 'cargo test ... -- --list' did not succeed; see $LIVE_LIST_LOG" >&2
  exit 2
fi
LOCK_TEST_LIST_LOG="$EVIDENCE_ROOT/logs/collab.write_lock_list.log"
if ! ( cd "$REPO_ROOT" && cargo test -p api --lib flow::collab::write::database_tests::lock_timeout_budgets_match_the_frozen_limits_v1_numbers -- --list ) > "$LOCK_TEST_LIST_LOG" 2>&1; then
  echo "FAIL: 'cargo test ... -- --list' did not succeed for the write.rs lock-timeout test; see $LOCK_TEST_LIST_LOG" >&2
  exit 2
fi

MARKERS_JSON_FILE="$EVIDENCE_ROOT/logs/collab.gate-markers.json"
printf '%s' "$MARKERS_JSON" > "$MARKERS_JSON_FILE"

MAPPING_VIOLATIONS_JSON="$(python3 - "$LIVE_LIST_LOG" "$MARKERS_JSON_FILE" <<'PY'
import json
import re
import sys

markers = json.load(open(sys.argv[2], encoding="utf-8"))
live_log = open(sys.argv[1], encoding="utf-8").read()
live_names = set(re.findall(r"^flow::collab::snapshot::(?:tests|database_tests)::(\w+): test$", live_log, re.M))

violations = []
for gate, tests in markers["by_gate"].items():
    for t in tests:
        if t not in live_names:
            violations.append(f"gate '{gate}' attributes test '{t}' which does not exist in the live test binary (renamed or deleted)")
for gate_number, t in markers.get("groundwork", {}).items():
    if t not in live_names:
        violations.append(f"Gate {gate_number} groundwork attributes test '{t}' which does not exist in the live test binary (renamed or deleted)")

print(json.dumps({"violations": violations, "live_test_count": len(live_names)}))
PY
)"
MAPPING_VIOLATION_COUNT="$(jq '.violations | length' <<<"$MAPPING_VIOLATIONS_JSON")"
if [[ "$MAPPING_VIOLATION_COUNT" -gt 0 ]]; then
  echo "FAIL: gate<->test mapping is stale:" >&2
  jq -r '.violations[] | "  - " + .' <<<"$MAPPING_VIOLATIONS_JSON" >&2
  exit 1
fi
echo "  live test binary confirms all $TOTAL_MAPPED mapped test name(s) exist ($(jq -r '.live_test_count' <<<"$MAPPING_VIOLATIONS_JSON") total across flow::collab::snapshot::{tests,database_tests})" >&2

# ---- 4. run the real snapshot tests (pure-logic + real-database) and the write.rs lock-timeout test ----
RUN_LOG="$EVIDENCE_ROOT/logs/collab.snapshot_test.log"
if [[ $SKIP_CARGO_TEST -eq 1 ]]; then
  echo "=== SKIPPED (--skip-cargo-test): cargo test -p api flow::collab::snapshot / write lock-timeout test ===" >&2
  echo "(skipped by --skip-cargo-test)" > "$RUN_LOG"
  DYNAMIC_RAN=0
else
  echo "=== running: cargo test -p api flow::collab::snapshot:: (in $REPO_ROOT) ===" >&2
  set +e
  ( cd "$REPO_ROOT" && cargo test -p api --lib flow::collab::snapshot:: -- --test-threads=4 ) > "$RUN_LOG" 2>&1
  SNAPSHOT_TEST_EXIT=$?
  ( cd "$REPO_ROOT" && cargo test -p api --lib flow::collab::write::database_tests::lock_timeout_budgets_match_the_frozen_limits_v1_numbers -- --test-threads=1 ) >> "$RUN_LOG" 2>&1
  LOCK_TEST_EXIT=$?
  set -e
  echo "cargo test exit codes: snapshot=$SNAPSHOT_TEST_EXIT lock_timeout=$LOCK_TEST_EXIT (informational only -- pass/fail below is derived from parsing each test's own ok/FAILED line)" >&2
  DYNAMIC_RAN=1
  tail -15 "$RUN_LOG" >&2
fi

if [[ $DYNAMIC_RAN -eq 1 ]] && grep -q "skipped: OPENPR_TEST_DATABASE_URL is not set" "$RUN_LOG"; then
  echo "FAIL: snapshot database tests were silently skipped (OPENPR_TEST_DATABASE_URL is not set)." >&2
  echo "Fix: export OPENPR_TEST_DATABASE_URL and re-run. A 'passed' result from a database-less" >&2
  echo "     skip run is a false green, not a partial one -- refusing to write evidence for it." >&2
  exit 2
fi

# ---- 5. parse per-test results and attribute to gates ----
RESULT_JSON="$(python3 - "$RUN_LOG" "$DYNAMIC_RAN" "$MARKERS_JSON_FILE" <<'PY'
import json
import re
import sys

markers = json.load(open(sys.argv[3], encoding="utf-8"))
run_log_path, dynamic_ran = sys.argv[1], sys.argv[2] == "1"

log = open(run_log_path, encoding="utf-8").read() if dynamic_ran else ""
test_status = dict(re.findall(
    r"^test flow::collab::(?:snapshot::(?:tests|database_tests)|write::database_tests)::(\w+) \.\.\. (ok|FAILED)$",
    log, re.M,
))

gates = {}
for gate, tests in markers["by_gate"].items():
    tests = sorted(set(tests))
    per_test = []
    all_ok = dynamic_ran and len(tests) > 0
    for t in tests:
        status = test_status.get(t, "not_run")
        per_test.append({"name": t, "status": status})
        if status != "ok":
            all_ok = False
    gates[gate] = {"tests": per_test, "dynamic_passed": all_ok, "note": markers["notes"].get(gate)}

lock_test_status = test_status.get("lock_timeout_budgets_match_the_frozen_limits_v1_numbers", "not_run")
frozen_limits_status = test_status.get("frozen_limits_v1_numbers_are_exactly_what_this_module_enforces", "not_run")

groundwork = {}
for gate_number, t in markers.get("groundwork", {}).items():
    groundwork[gate_number] = {"test": t, "status": test_status.get(t, "not_run")}

print(json.dumps({
    "dynamic_ran": dynamic_ran,
    "gates": gates,
    "lock_timeout_test_status": lock_test_status,
    "frozen_limits_test_status": frozen_limits_status,
    "groundwork": groundwork,
}))
PY
)"

echo "=== snapshot test results by gate ===" >&2
jq -r '.gates | to_entries[] | "  \(.key): dynamic_passed=\(.value.dynamic_passed) (\(.value.tests | map(select(.status!="ok")) | length) not ok of \(.value.tests | length))\(if .value.note then " [" + .value.note + "]" else "" end)"' <<<"$RESULT_JSON" >&2
echo "  lock_timeout_budgets_match_the_frozen_limits_v1_numbers: $(jq -r '.lock_timeout_test_status' <<<"$RESULT_JSON")" >&2
echo "  frozen_limits_v1_numbers_are_exactly_what_this_module_enforces: $(jq -r '.frozen_limits_test_status' <<<"$RESULT_JSON")" >&2

LOCK_TEST_PASSED="$(jq -r '.lock_timeout_test_status == "ok"' <<<"$RESULT_JSON")"
FROZEN_LIMITS_TEST_PASSED="$(jq -r '.frozen_limits_test_status == "ok"' <<<"$RESULT_JSON")"

# ---- 6. assemble the 6 gate verdicts ----
FINAL_JSON="$(jq -n \
  --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" --arg release "$RELEASE" \
  --arg adr "$ADR_PATH" --arg limits "$LIMITS_PATH" --arg snapshot_rs "$SNAPSHOT_RS" \
  --argjson static_check "$STATIC_JSON" \
  --argjson adr_accepted "$ADR_ACCEPTED" \
  --argjson constants_passed "$CONSTANTS_PASSED" \
  --argjson lock_test_passed "$LOCK_TEST_PASSED" \
  --argjson frozen_limits_test_passed "$FROZEN_LIMITS_TEST_PASSED" \
  --argjson harness "$HARNESS_CHECK_JSON" \
  --argjson cache_evidence "$CACHE_CHECK_JSON" \
  --argjson snap "$RESULT_JSON" \
  '
  def gate($id): ($snap.gates[$id].dynamic_passed // false);
  def gate_tests($id): ($snap.gates[$id].tests // []);
  def gate_note($id): ($snap.gates[$id].note // null);

  ($constants_passed and $lock_test_passed and $frozen_limits_test_passed) as $numeric_budgets_verified_portion |

  {
    schema_version: "sylvode.flow.collab-architecture-result.v1",
    source_head: $head,
    generated_at: $generated_at,
    release: $release,
    adr: $adr,
    limits_contract: $limits,
    adr_check: $static_check,
    numeric_budgets_check: {
      constant_cross_check_passed: $constants_passed,
      lock_timeout_budgets_match_the_frozen_limits_v1_numbers_test: $lock_test_passed,
      frozen_limits_v1_numbers_are_exactly_what_this_module_enforces_test: $frozen_limits_test_passed,
      verified_portion_passed: $numeric_budgets_verified_portion,
      load_harness: $harness
    },
    cache: $cache_evidence.cache,
    cache_evidence: $cache_evidence,
    gates: {
      collab_architecture_adr_accepted: {
        status: (if $adr_accepted then "passed" else "failed" end),
        reason: (if $adr_accepted then null else ("ADR-0010 status is \"" + $static_check.adr_status + "\", not \"Accepted\"") end)
      },
      bounded_warm_cache_lock_hold_and_round_trip_budgets: {
        status: (if ($numeric_budgets_verified_portion and $harness.budget_gate_passed and $cache_evidence.passed) then "passed" else "failed" end),
        reason: (
          if ($numeric_budgets_verified_portion and $harness.budget_gate_passed and $cache_evidence.passed) then
            "frozen constants, dedicated load budgets, and the ADR fixed cache block all pass"
          elif $harness.execution_status == "not_run_environment_not_satisfied" then
            "load harness environment not satisfied; no load distribution was run or classified as an implementation failure"
          else
            "numeric constants, dedicated load-harness checks, or cache evidence failed; see load_harness/cache_evidence violations"
          end
        ),
        verified_portion: {
          constant_cross_check_passed: $constants_passed,
          lock_timeout_test_passed: $lock_test_passed,
          frozen_limits_test_passed: $frozen_limits_test_passed
        },
        load_harness: $harness,
        cache_evidence: $cache_evidence
      },
      minimal_snapshot_advancement_bounds_tail: {
        status: (if (gate("minimal_snapshot_advancement_bounds_tail") and $frozen_limits_test_passed) then "passed" else "failed" end),
        tests: gate_tests("minimal_snapshot_advancement_bounds_tail")
      },
      snapshot_tail_restart_recovery: {
        status: (if gate("snapshot_tail_restart_recovery") then "passed" else "failed" end),
        tests: gate_tests("snapshot_tail_restart_recovery")
      },
      bootstrap_repeatable_read_and_ws_parity: {
        status: (if $harness.parity_gate_passed then "passed" else "failed" end),
        groundwork_test: ($snap.groundwork["9"] // null),
        reason: (
          if $harness.parity_gate_passed then
            "dedicated release PostgreSQL harness independently confirms REPEATABLE READ READ ONLY bootstrap transactions, contiguous accepted seq/tails, frontier equivalence and zero REST/WS identity divergence"
          elif $harness.execution_status == "not_run_environment_not_satisfied" then
            "load harness environment not satisfied; REST/WS parity was not measured and is not passed or skipped"
          else
            "dedicated load-harness parity checks failed; see load_harness.violations and parity_checks"
          end
        ),
        load_harness: $harness,
        note: "the snapshot groundwork test remains supplemental; only the full harness evidence can satisfy this gate"
      },
      accepted_egress_seq_monotonic_and_gap_resync: {
        status: (if ($snap.gates | has("accepted_egress_seq_monotonic_and_gap_resync")) then (if gate("accepted_egress_seq_monotonic_and_gap_resync") then "passed" else "failed" end) else "not_covered" end),
        tests: gate_tests("accepted_egress_seq_monotonic_and_gap_resync"),
        reason: (if ($snap.gates | has("accepted_egress_seq_monotonic_and_gap_resync")) then null else ("no \"Gate 10 `accepted_egress_seq_monotonic_and_gap_resync`\" doc-comment marker found in " + $snapshot_rs + " (re-parsed from the live source every run, see the \"Gate <N> `<id>`\" marker extraction step above) -- nothing exists yet for this script to run") end)
      }
    }
  }
  | .passed = ([.gates[].status] | all(. == "passed"))
  ')"

OUT_PATH="$EVIDENCE_ROOT/collab-architecture-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$FINAL_JSON" | jq . > "$OUT_TMP"
sync "$OUT_TMP" 2>/dev/null || true
mv -f "$OUT_TMP" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2

echo "$FINAL_JSON"

OVERALL_PASSED="$(jq -r '.passed' <<<"$FINAL_JSON")"
if [[ "$OVERALL_PASSED" == "true" ]]; then
  exit 0
fi
exit 1
