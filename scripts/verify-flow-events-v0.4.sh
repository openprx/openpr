#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 events/dispatch verifier.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md, the v0.4
# section's "Events/dispatch verifier" paragraphs (the long
# `business_event_dispatch_same_transaction` / `dispatch_expansion_...` /
# `no_subscribers_...` / `dispatcher_liveness_...` /
# `flow_content_delivery_coalescing` / `coalescing_seal_and_source_first_
# expansion` block), and contracts/events-v1.md ("Event type registry",
# "稳定 envelope 与 audit 字段").
#
# Covers 8 hard gates: business_event_dispatch_same_transaction,
# dispatch_expansion_snapshot_semantics, no_subscribers_terminalized_and_
# reaped, dispatcher_liveness_and_backlog, flow_content_delivery_coalescing,
# coalescing_seal_and_source_first_expansion, flow_event_registry_
# payload_policy_complete, event_idempotency_audit_and_redaction.
#
# HOW IT WORKS (does not run its own fixtures -- calls what already exists):
#
#   1. Registry/payload-policy static check (flow_event_registry_payload_
#      policy_complete): parses contracts/events-v1.md's "Event type
#      registry" table into the set of contract-declared `flow.*` event
#      types, greps the actual v0.4 Rust producers (apps/api/src/flow/
#      command.rs, apps/api/src/flow/collab/write.rs, apps/api/src/flow/
#      collab/bootstrap.rs -- production code only, each file's own
#      #[cfg(test)] module is excluded by line-number cutoff) for the
#      `event_type` string literals they actually emit, and separately
#      greps the whole repo for a `flow_event_payload_policy`-shaped
#      registry constant (the Flow-specific analogue of
#      apps/api/src/forms/event_redaction.rs's `FORM_EVENT_PAYLOAD_
#      POLICIES`, which events-v1.md's own text names as the pattern Flow
#      must reuse). This is a real, structural, independently-recomputed
#      finding, not a guess: this script found there is currently no such
#      registry in the source tree, so this gate cannot pass -- it isn't
#      that verification is impossible, it's that the feature it verifies
#      has not been built yet.
#
#   2. Idempotent-event-insert static check (part of event_idempotency_
#      audit_and_redaction): greps apps/api/src/events/mod.rs for the
#      `ON CONFLICT (workspace_id, idempotency_key) DO NOTHING RETURNING
#      id` shaped idempotent insert and an `insert_flow_event` helper that
#      events-v1.md's own text says implementation "必须" add next to the
#      existing `insert_business_event`. Neither exists yet (confirmed by
#      this script's own grep, recorded in the output) -- another real,
#      not-yet-built gap, not a tooling limitation.
#
#   3. Dynamic dispatcher test run (the other 6 gates, and the delivery-
#      layer half of event_idempotency_audit_and_redaction): apps/api/src/
#      events/dispatcher.rs's own `#[cfg(test)] mod dispatcher_database_
#      tests` block is internally organised with literal
#      `// Gate: <hard_gate_id>` section-header comments the implementer
#      left specifically so a gate verifier could pick them up (see
#      `ca58078`, "6 -> 37" tests). This script parses those markers
#      DIRECTLY FROM THE SOURCE FILE at run time (never a hardcoded
#      mapping baked into this script) to build the gate->test-name table,
#      cross-checks every named test actually exists in the live
#      `cargo test ... -- --list` output (so a rename/deletion in
#      apps/api/** is caught as a mapping error, not silently
#      mis-attributed), then actually runs
#      `cargo test -p api events::dispatcher::dispatcher_database_tests`
#      against a real Postgres database and parses the real per-test
#      ok/FAILED lines. A gate is only "passed" if every test the source
#      itself attributes to it exists, was actually executed (not silently
#      skipped -- see the OPENPR_TEST_DATABASE_URL skip-detection below)
#      and passed.
#
# HONEST GAPS (recorded in the JSON, `passed` forced false while any of
# these remain open -- this script never rounds a partial result up to a
# pass):
#
#   - flow_event_registry_payload_policy_complete and the
#     event-insert-idempotency half of event_idempotency_audit_and_
#     redaction: the underlying `flow_event_payload_policy` registry and
#     `insert_flow_event` idempotent helper events-v1.md describes do not
#     exist in apps/api/** yet (see check 1/2 above). Building them is an
#     apps/** change, out of this script's --repo-root=read-only scope.
#   - coalescing_seal_and_source_first_expansion: the source-attributed
#     tests for this gate (12 of them) all currently pass, but
#     gate-commands.md's paragraph names several additional negative
#     fixtures this script did NOT find matching tests for when it read
#     dispatcher.rs by hand while writing this script:
#       * a reaper predicate test asserting the tombstone-cleanup query
#         specifically keys off `delivery_id IS NULL` (no `tombstone`
#         hit anywhere in dispatcher.rs);
#       * `requeue_failed` reviving a content row while clearing
#         `document_id` to NULL (no `requeue_failed` function exists in
#         dispatcher.rs at all -- grepped, zero hits);
#       * the FIFO-barrier test simulates a busy predecessor via a direct
#         `UPDATE event_dispatch SET lease_token=...` rather than gate-
#         commands.md's literal "lock the old sealed row in a second,
#         still-open transaction" -- a materially weaker proof of the
#         SKIP LOCKED behaviour under a *real* held row lock.
#     These three are recorded as `checklist` entries with
#     status="not_covered" and a `reason`; this script keeps `passed`
#     false for this one gate even though its 12 attributed tests are all
#     green, because "the tests that exist all pass" is not the same
#     claim as "the paragraph is fully covered" (see this repo's
#     `command_contended_document_cardinality` precedent in
#     verify-flow-cardinality-v0.4.sh for the same discipline).
#
# Exit codes: 0 = every one of the 8 gates recomputed to passed (does not
# happen today -- see gaps above), 1 = ran to completion and wrote
# evidence/v0.4/flow-events-result.json with one or more gates not passed,
# 2 = usage/tool/environment error, OR the dispatcher database tests were
# silently skipped because OPENPR_TEST_DATABASE_URL is not set (a "37
# passed" from a database-less run would be a false green, not a partial
# one, so this is fail-closed at exit 2, not folded into the honest
# partial-pass path at exit 1).

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
REPO_ROOT="$ROOT_DIR"
CONTRACT_PATH=""
JSON_MODE=0
SKIP_CARGO_TEST=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-events-v0.4.sh --contract PATH --json [OPTIONS]

Cross-checks contracts/events-v1.md's event-type registry and payload-
policy discipline against apps/api/** (read-only), then runs the real
`cargo test -p api events::dispatcher::dispatcher_database_tests` suite
and attributes each of its tests to a hard gate using the `// Gate: <id>`
markers dispatcher.rs itself carries. Writes
evidence/v0.4/flow-events-result.json.

Options:
  --contract PATH          Path to contracts/events-v1.md. Default:
                          <contracts-root>/contracts/events-v1.md
  --contracts-root DIR     Root containing contracts/. Default:
                          /opt/working/sylvode-flow
  --evidence-root DIR     Where flow-events-result.json is written.
                          Default: /opt/working/sylvode-flow/evidence/v0.4
  --repo-root DIR         Repository containing apps/api and the cargo
                          workspace. Default: this checkout.
  --skip-cargo-test        Skip the dispatcher database test run (fast
                          iteration only; the written evidence records
                          this and every gate that needed the dynamic
                          check is treated as failed).
  --json                  Required for CLI-contract compatibility.
  -h, --help              Show this help and exit 0.

Exit codes: 0 all 8 gates passed, 1 ran to completion with one or more
gates not passed (the honest, normal outcome today), 2 usage/tool/
environment error or the dispatcher tests were silently skipped because
OPENPR_TEST_DATABASE_URL is not set.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --contract) CONTRACT_PATH="${2:?--contract requires a PATH argument}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --skip-cargo-test) SKIP_CARGO_TEST=1; shift ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
for tool in jq git python3 cargo; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    exit 2
  fi
done

if [[ -z "$CONTRACT_PATH" ]]; then
  CONTRACT_PATH="$CONTRACTS_ROOT/contracts/events-v1.md"
fi
if [[ ! -f "$CONTRACT_PATH" ]]; then
  echo "FAIL: missing contract file: $CONTRACT_PATH" >&2
  exit 2
fi
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi

EVENTS_MOD_RS="$REPO_ROOT/apps/api/src/events/mod.rs"
DISPATCHER_RS="$REPO_ROOT/apps/api/src/events/dispatcher.rs"
COMMAND_RS="$REPO_ROOT/apps/api/src/flow/command.rs"
WRITE_RS="$REPO_ROOT/apps/api/src/flow/collab/write.rs"
BOOTSTRAP_RS="$REPO_ROOT/apps/api/src/flow/collab/bootstrap.rs"
for f in "$EVENTS_MOD_RS" "$DISPATCHER_RS" "$COMMAND_RS" "$WRITE_RS" "$BOOTSTRAP_RS"; do
  if [[ ! -f "$f" ]]; then
    echo "FAIL: source file not found (nothing to statically verify): $f" >&2
    exit 2
  fi
done

mkdir -p "$EVIDENCE_ROOT" "$EVIDENCE_ROOT/logs"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# ---- 1+2. static checks: registry/payload-policy + idempotent-insert helper ----
STATIC_JSON="$(python3 - "$CONTRACT_PATH" "$EVENTS_MOD_RS" "$COMMAND_RS" "$WRITE_RS" "$BOOTSTRAP_RS" "$REPO_ROOT" <<'PY'
import json
import re
import subprocess
import sys

contract_path, mod_rs, command_rs, write_rs, bootstrap_rs, repo_root = sys.argv[1:7]

# ---- parse the contract's "Event type registry" table ----
contract_text = open(contract_path, encoding="utf-8").read()
m = re.search(r"## Event type registry\n(.*?)\n## ", contract_text, re.S)
if not m:
    print(json.dumps({"error": "could not find '## Event type registry' section in " + contract_path}))
    sys.exit(0)
table = m.group(1)
rows = []
for line in table.splitlines():
    line = line.strip()
    if not line.startswith("|") or line.startswith("|---") or "Event type" in line:
        continue
    cells = [c.strip() for c in line.strip("|").split("|")]
    if len(cells) < 4:
        continue
    event_type = cells[0].strip("`")
    if not event_type.startswith("flow."):
        continue
    first_shipped = cells[1]
    rows.append({"event_type": event_type, "first_shipped": first_shipped})

expected_types = sorted({r["event_type"] for r in rows})
expected_v04_types = sorted({r["event_type"] for r in rows if "0.4" in r["first_shipped"]})

# ---- producer literals: production code only, cut off each file's own #[cfg(test)] module ----
def production_slice(path):
    text = open(path, encoding="utf-8").read()
    cut = re.search(r"^#\[cfg\(test\)\]\s*$", text, re.M)
    return text[: cut.start()] if cut else text

producer_literals = set()
literal_re = re.compile(r'"(flow\.[a-z_]+(?:\.[a-z_]+)+)"')
for path in (command_rs, write_rs, bootstrap_rs):
    for line in production_slice(path).splitlines():
        # `detected_by: "flow.command.create_object"` and similar are
        # flow_integrity_records subject markers (ADR-0013 §4), not
        # events-v1.md registry event types -- exclude explicitly rather
        # than let the shared "flow.x.y" shape produce a false
        # unknown-producer-literal violation.
        if "detected_by" in line:
            continue
        for lit in literal_re.findall(line):
            producer_literals.add(lit)
producer_literals = sorted(producer_literals)

unknown = sorted(set(producer_literals) - set(expected_types))
missing_v04 = sorted(set(expected_v04_types) - set(producer_literals))

# ---- registry constant existence: grep the whole repo (excluding target/) ----
def grep_repo(pattern):
    try:
        out = subprocess.run(
            ["grep", "-rl", "--include=*.rs", pattern, repo_root + "/apps", repo_root + "/crates"],
            capture_output=True, text=True, check=False,
        ).stdout
    except FileNotFoundError:
        out = ""
    return [line for line in out.splitlines() if "/target/" not in line]

registry_hits = grep_repo("flow_event_payload_policy")
registry_exists = len(registry_hits) > 0

# ---- idempotent-insert helper: apps/api/src/events/mod.rs ----
mod_text = open(mod_rs, encoding="utf-8").read()
on_conflict_exists = "ON CONFLICT" in mod_text
insert_flow_event_exists = bool(re.search(r"fn\s+insert_flow_event\b", mod_text))

print(json.dumps({
    "contract_event_types": {
        "total": len(expected_types),
        "v0_4_first_shipped": expected_v04_types,
        "all": expected_types,
    },
    "producer_literals": producer_literals,
    "unknown_producer_literals": unknown,
    "missing_v0_4_producer_literals": missing_v04,
    "registry": {
        "identifier_searched": "flow_event_payload_policy",
        "exists": registry_exists,
        "hit_files": registry_hits,
    },
    "idempotent_insert_helper": {
        "file": mod_rs,
        "insert_flow_event_fn_exists": insert_flow_event_exists,
        "on_conflict_do_nothing_exists": on_conflict_exists,
    },
}))
PY
)"

if ! jq -e . >/dev/null 2>&1 <<<"$STATIC_JSON"; then
  echo "FAIL: static registry/idempotency parser did not produce valid JSON" >&2
  echo "$STATIC_JSON" >&2
  exit 2
fi
if jq -e 'has("error")' >/dev/null 2>&1 <<<"$STATIC_JSON"; then
  echo "FAIL: $(jq -r '.error' <<<"$STATIC_JSON")" >&2
  exit 2
fi

REGISTRY_UNKNOWN_COUNT="$(jq '.unknown_producer_literals | length' <<<"$STATIC_JSON")"
REGISTRY_EXISTS="$(jq -r '.registry.exists' <<<"$STATIC_JSON")"
REGISTRY_PASSED=$([[ "$REGISTRY_UNKNOWN_COUNT" -eq 0 && "$REGISTRY_EXISTS" == "true" ]] && echo true || echo false)

INSERT_HELPER_EXISTS="$(jq -r '.idempotent_insert_helper.insert_flow_event_fn_exists' <<<"$STATIC_JSON")"
ON_CONFLICT_EXISTS="$(jq -r '.idempotent_insert_helper.on_conflict_do_nothing_exists' <<<"$STATIC_JSON")"
EVENT_INSERT_IDEMPOTENT_PASSED=$([[ "$INSERT_HELPER_EXISTS" == "true" && "$ON_CONFLICT_EXISTS" == "true" ]] && echo true || echo false)

echo "=== static check: contracts/events-v1.md registry vs apps/api producer literals ===" >&2
echo "  contract event types: $(jq '.contract_event_types.total' <<<"$STATIC_JSON")" >&2
echo "  producer literals found: $(jq '.producer_literals | length' <<<"$STATIC_JSON")" >&2
echo "  unknown producer literals (violation): $REGISTRY_UNKNOWN_COUNT" >&2
echo "  flow_event_payload_policy registry exists: $REGISTRY_EXISTS" >&2
echo "=== static check: idempotent event-insert helper (events-v1.md \"必须\" clause) ===" >&2
echo "  insert_flow_event() exists: $INSERT_HELPER_EXISTS" >&2
echo "  ON CONFLICT DO NOTHING exists in events/mod.rs: $ON_CONFLICT_EXISTS" >&2

# ---- 3. Gate: <id> marker extraction from dispatcher.rs (dynamic, re-parsed every run) ----
MARKERS_JSON="$(python3 - "$DISPATCHER_RS" <<'PY'
import json
import re
import sys

path = sys.argv[1]
lines = open(path, encoding="utf-8").read().split("\n")

gate_starts = []
for i, l in enumerate(lines):
    gm = re.match(r"\s*// Gate: (\S+)\s*$", l)
    if gm:
        gate_starts.append((i, gm.group(1)))

if not gate_starts:
    print(json.dumps({"error": "no '// Gate: <id>' markers found in " + path}))
    sys.exit(0)

fn_re = re.compile(r"(?:async fn|fn) (\w+)\(\)")

def tests_in(a, b):
    seg = "\n".join(lines[a:b])
    return fn_re.findall(seg)

by_gate = {}
for idx, (start, gate) in enumerate(gate_starts):
    end = gate_starts[idx + 1][0] if idx + 1 < len(gate_starts) else len(lines)
    by_gate.setdefault(gate, [])
    by_gate[gate].extend(tests_in(start, end))

supporting_tests = tests_in(0, gate_starts[0][0])

print(json.dumps({"by_gate": by_gate, "supporting_tests": supporting_tests}))
PY
)"

if ! jq -e . >/dev/null 2>&1 <<<"$MARKERS_JSON"; then
  echo "FAIL: Gate-marker parser did not produce valid JSON" >&2
  exit 2
fi
if jq -e 'has("error")' >/dev/null 2>&1 <<<"$MARKERS_JSON"; then
  echo "FAIL: $(jq -r '.error' <<<"$MARKERS_JSON")" >&2
  exit 2
fi

TOTAL_MAPPED_TESTS="$(jq '[.by_gate[] | length] | add // 0' <<<"$MARKERS_JSON")"
if [[ "$TOTAL_MAPPED_TESTS" -eq 0 ]]; then
  echo "FAIL: zero tests attributed to any gate -- refusing to write a vacuous result" >&2
  exit 2
fi

echo "=== Gate: <id> markers found in $DISPATCHER_RS ===" >&2
jq -r '.by_gate | to_entries[] | "  \(.key): \(.value | length) test(s)"' <<<"$MARKERS_JSON" >&2

# ---- cross-check every mapped test exists in the LIVE test binary (catches rename/delete drift) ----
LIVE_LIST_LOG="$EVIDENCE_ROOT/logs/events.dispatcher_list.log"
if ! ( cd "$REPO_ROOT" && cargo test -p api events::dispatcher::dispatcher_database_tests -- --list ) > "$LIVE_LIST_LOG" 2>&1; then
  echo "FAIL: 'cargo test ... -- --list' did not succeed; see $LIVE_LIST_LOG" >&2
  exit 2
fi

MARKERS_JSON_FILE="$(mktemp)"
trap 'rm -f "$MARKERS_JSON_FILE"' EXIT
printf '%s' "$MARKERS_JSON" > "$MARKERS_JSON_FILE"
MAPPING_VIOLATIONS_JSON="$(python3 - "$LIVE_LIST_LOG" "$MARKERS_JSON_FILE" <<'PY'
import json
import re
import sys

markers = json.load(open(sys.argv[2], encoding="utf-8"))
live_log = open(sys.argv[1], encoding="utf-8").read()
live_names = set(re.findall(r"^events::dispatcher::dispatcher_database_tests::(\w+): test$", live_log, re.M))

violations = []
for gate, tests in markers["by_gate"].items():
    for t in tests:
        if t not in live_names:
            violations.append(f"gate '{gate}' attributes test '{t}' which does not exist in the live test binary (renamed or deleted)")

print(json.dumps({"violations": violations, "live_test_count": len(live_names)}))
PY
)"

MAPPING_VIOLATION_COUNT="$(jq '.violations | length' <<<"$MAPPING_VIOLATIONS_JSON")"
if [[ "$MAPPING_VIOLATION_COUNT" -gt 0 ]]; then
  echo "FAIL: gate<->test mapping is stale:" >&2
  jq -r '.violations[] | "  - " + .' <<<"$MAPPING_VIOLATIONS_JSON" >&2
  exit 1
fi
echo "  live test binary confirms all $TOTAL_MAPPED_TESTS mapped test name(s) exist ($(jq -r '.live_test_count' <<<"$MAPPING_VIOLATIONS_JSON") total in the module)" >&2

# ---- 4. run the real dispatcher database tests ----
RUN_LOG="$EVIDENCE_ROOT/logs/events.dispatcher_test.log"
if [[ $SKIP_CARGO_TEST -eq 1 ]]; then
  echo "=== SKIPPED (--skip-cargo-test): cargo test -p api events::dispatcher::dispatcher_database_tests ===" >&2
  echo "(skipped by --skip-cargo-test)" > "$RUN_LOG"
  DYNAMIC_RAN=0
else
  echo "=== running: cargo test -p api events::dispatcher::dispatcher_database_tests (in $REPO_ROOT) ===" >&2
  set +e
  ( cd "$REPO_ROOT" && cargo test -p api events::dispatcher::dispatcher_database_tests -- --test-threads=4 ) > "$RUN_LOG" 2>&1
  CARGO_TEST_EXIT=$?
  set -e
  echo "cargo test exit code: $CARGO_TEST_EXIT (informational only -- pass/fail below is derived from parsing each test's own ok/FAILED line, not this aggregate exit code)" >&2
  DYNAMIC_RAN=1
  tail -10 "$RUN_LOG" >&2
fi

if [[ $DYNAMIC_RAN -eq 1 ]] && grep -q "skipped: OPENPR_TEST_DATABASE_URL is not set" "$RUN_LOG"; then
  echo "FAIL: dispatcher database tests were silently skipped (OPENPR_TEST_DATABASE_URL is not set)." >&2
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
    r"^test events::dispatcher::dispatcher_database_tests::(\w+) \.\.\. (ok|FAILED)$", log, re.M
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
    gates[gate] = {"tests": per_test, "dynamic_passed": all_ok}

print(json.dumps({"dynamic_ran": dynamic_ran, "gates": gates, "supporting_tests": sorted(markers["supporting_tests"])}))
PY
)"

echo "=== dispatcher test results by gate ===" >&2
jq -r '.gates | to_entries[] | "  \(.key): dynamic_passed=\(.value.dynamic_passed) (\(.value.tests | map(select(.status!="ok")) | length) not ok of \(.value.tests | length))"' <<<"$RESULT_JSON" >&2

# ---- 6. coalescing_seal_and_source_first_expansion: additional hand-audited checklist ----
# These three items were checked by hand while writing this script (see the
# file header's "HONEST GAPS" section) -- no matching test/source hook was
# found for any of them, so they are recorded as not_covered regardless of
# how the source's own Gate: marker attribution scores.
COALESCING_SEAL_CHECKLIST='[
  {"requirement":"reaper deletes only rows matched by a delivery_id IS NULL tombstone predicate","status":"not_covered","reason":"no occurrence of the string \"tombstone\" anywhere in apps/api/src/events/dispatcher.rs; no test asserts the reaper query shape"},
  {"requirement":"requeue_failed revives a content delivery row and clears document_id to NULL","status":"not_covered","reason":"no fn requeue_failed exists in apps/api/src/events/dispatcher.rs (grepped, zero hits) -- the feature this fixture would exercise is not implemented yet"},
  {"requirement":"FIFO barrier fixture holds the older sealed/pending row lock in a second, still-open transaction (not a direct column UPDATE) before starting the second worker","status":"not_covered","reason":"dispatch_fifo_barrier_blocks_a_newer_same_document_work_item_while_an_older_one_is_in_flight simulates the busy predecessor via a direct UPDATE event_dispatch SET lease_token=... in the same connection, not a second held transaction -- a materially weaker proof of SKIP LOCKED behaviour under a real row lock"}
]'

# ---- 7. assemble the 8 gate verdicts ----
FINAL_JSON="$(jq -n \
  --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" --arg contract "$CONTRACT_PATH" \
  --argjson static_check "$STATIC_JSON" \
  --argjson registry_passed "$REGISTRY_PASSED" \
  --argjson event_insert_idempotent_passed "$EVENT_INSERT_IDEMPOTENT_PASSED" \
  --argjson dispatch "$RESULT_JSON" \
  --argjson coalescing_seal_checklist "$COALESCING_SEAL_CHECKLIST" \
  '
  def gate($id): $dispatch.gates[$id].dynamic_passed;
  def gate_tests($id): $dispatch.gates[$id].tests;

  ($coalescing_seal_checklist | map(select(.status != "covered")) | length == 0) as $coalescing_seal_checklist_clean |

  {
    schema_version: "sylvode.flow.events-result.v1",
    source_head: $head,
    generated_at: $generated_at,
    contract: $contract,
    registry_check: {
      contract_event_types: $static_check.contract_event_types,
      producer_literals: $static_check.producer_literals,
      unknown_producer_literals: $static_check.unknown_producer_literals,
      missing_v0_4_producer_literals: $static_check.missing_v0_4_producer_literals,
      registry: $static_check.registry,
      passed: $registry_passed
    },
    idempotent_insert_check: {
      idempotent_insert_helper: $static_check.idempotent_insert_helper,
      passed: $event_insert_idempotent_passed
    },
    dispatcher_dynamic: {
      dynamic_ran: $dispatch.dynamic_ran,
      supporting_tests_not_gate_attributed: $dispatch.supporting_tests
    },
    gates: {
      business_event_dispatch_same_transaction: {
        status: (if gate("business_event_dispatch_same_transaction") then "passed" else "failed" end),
        tests: gate_tests("business_event_dispatch_same_transaction")
      },
      dispatch_expansion_snapshot_semantics: {
        status: (if gate("dispatch_expansion_snapshot_semantics") then "passed" else "failed" end),
        tests: gate_tests("dispatch_expansion_snapshot_semantics")
      },
      no_subscribers_terminalized_and_reaped: {
        status: (if gate("no_subscribers_terminalized_and_reaped") then "passed" else "failed" end),
        tests: gate_tests("no_subscribers_terminalized_and_reaped")
      },
      dispatcher_liveness_and_backlog: {
        status: (if gate("dispatcher_liveness_and_backlog") then "passed" else "failed" end),
        tests: gate_tests("dispatcher_liveness_and_backlog")
      },
      flow_content_delivery_coalescing: {
        status: (if gate("flow_content_delivery_coalescing") then "passed" else "failed" end),
        tests: gate_tests("flow_content_delivery_coalescing")
      },
      coalescing_seal_and_source_first_expansion: {
        status: (if (gate("coalescing_seal_and_source_first_expansion") and $coalescing_seal_checklist_clean) then "passed" else "failed" end),
        tests: gate_tests("coalescing_seal_and_source_first_expansion"),
        dynamic_passed: gate("coalescing_seal_and_source_first_expansion"),
        additional_checklist: $coalescing_seal_checklist,
        note: "dynamic_passed reflects only the tests dispatcher.rs itself attributes to this gate via its own // Gate: marker; additional_checklist covers narrower textual requirements from gate-commands.md this script could not find matching test/source evidence for -- either one being false keeps status=failed"
      },
      flow_event_registry_payload_policy_complete: {
        status: (if $registry_passed then "passed" else "failed" end),
        reason: (if $registry_passed then null else "flow_event_payload_policy registry does not exist in apps/api/** (or an unknown producer literal was found) -- see registry_check above" end)
      },
      event_idempotency_audit_and_redaction: {
        status: (if ($event_insert_idempotent_passed and $registry_passed and gate("flow_content_delivery_coalescing")) then "passed" else "failed" end),
        components: {
          event_insert_idempotency: (if $event_insert_idempotent_passed then "passed" else "failed (insert_flow_event/ON CONFLICT helper not implemented -- see idempotent_insert_check above)" end),
          payload_redaction_registry: (if $registry_passed then "passed" else "failed (shares flow_event_payload_policy gap with flow_event_registry_payload_policy_complete)" end),
          delivery_layer_idempotency: (if gate("flow_content_delivery_coalescing") then "passed (build_delivery_body_reuses_the_same_delivery_id_across_retries_only_the_attempt_number_changes and related re-expansion/golden-wire tests pass)" else "failed" end)
        }
      }
    }
  }
  | .passed = ([.gates[].status] | all(. == "passed"))
  ')"

OUT_PATH="$EVIDENCE_ROOT/flow-events-result.json"
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
