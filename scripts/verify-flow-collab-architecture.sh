#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 collab-architecture verifier.
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
# HOW IT WORKS (calls what already exists; does not fabricate a load-test
# harness):
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
# HONEST GAPS (recorded in the JSON, `passed` forced false while any of
# these remain open):
#
#   - bounded_warm_cache_lock_hold_and_round_trip_budgets: ADR-0010 §"量化
#     接受与推翻门槛" freezes two numbers this script CANNOT check without
#     a load-generation harness: "hold p95 不超过 25 ms" (a *distribution*
#     statistic under concurrent load, not the DOCUMENT_LOCK_HOLD_MS_
#     MAX=100ms hard rollback ceiling this script does verify) and "10 个
#     并发 client 的 accepted round-trip p95 不超过 250 ms". This script
#     extracts both numbers from ADR-0010's own text (`load_test_targets_
#     not_covered`) AND, every run, re-greps apps/ and crates/ for any
#     plausible load-harness marker (`load_harness_grep` -- patterns like
#     `round_trip_p95`, `10_client`, `LoadHarness`) instead of assuming
#     from memory that none exists. It never claims to have MEASURED the
#     targets even if a harness turns up (running one and reading its
#     output is still a human/CI job, not something this script fabricates)
#     -- but the "nothing exists yet" half of the reason is now something
#     this script re-proves every run rather than repeats verbatim.
#   - bootstrap_repeatable_read_and_ws_parity: snapshot.rs's own doc
#     comment on `advancement_stays_correct_when_racing_a_concurrent_
#     write` literally says "Gate 9 groundwork ... Not the full gate 9
#     fixture (gate-commands.md's REST/WS 10-client load harness is out
#     of this task's scope)". This script takes that self-assessment at
#     face value (it is source-code-adjacent, re-checked every run, not a
#     one-off claim this script trusts blindly), reuses the same
#     `load_harness_grep` re-search described above for the harness half
#     of its reason, and keeps this gate "failed" even when the groundwork
#     test passes.
#   - accepted_egress_seq_monotonic_and_gap_resync: this script's own
#     "Gate <N> `<id>`" marker extraction (used identically for gates 7
#     and 8 above) is what actually decides this gate's status -- if
#     snapshot.rs ever grows a Gate 10 `accepted_egress_seq_monotonic_and_
#     gap_resync` marker with attributed tests, this gate starts scoring
#     "passed"/"failed" from those tests' real ok/FAILED results like any
#     other gate. Today no such marker exists, so status is "not_covered"
#     -- computed by checking the live marker map, not a value baked into
#     this script.
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
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
REPO_ROOT="$ROOT_DIR"
RELEASE="0.4"
ADR_PATH=""
LIMITS_PATH=""
JSON_MODE=0
SKIP_CARGO_TEST=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-collab-architecture.sh --release 0.4 --adr PATH --limits PATH --json [OPTIONS]

Reads ADR-0010's DOCUMENT_CONTROL status and frozen numeric budgets
(read-only), cross-checks the lock-wait/lock-hold/rebase-attempt
constants against apps/api/src/flow/collab/limits.rs, then runs the real
snapshot-advancement and restart-recovery Rust tests
(flow::collab::snapshot::{tests,database_tests}) and attributes each to
a hard gate using the "Gate <N> `<id>`" doc-comment markers snapshot.rs
itself carries. Writes evidence/v0.4/collab-architecture-result.json.

Explicitly does NOT attempt the p95 lock-hold / 10-client round-trip load
test gate-commands.md also requires (no load-generation harness exists in
this repository) -- that portion is recorded as not_covered, never as a
placeholder pass.

Options:
  --release VER            Gate release identifier (recorded in output).
                          Default: 0.4
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
                          written. Default:
                          /opt/working/sylvode-flow/evidence/v0.4
  --repo-root DIR         Repository containing apps/api and the cargo
                          workspace. Default: this checkout.
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
    --adr) ADR_PATH="${2:?--adr requires a PATH argument}"; shift 2 ;;
    --limits) LIMITS_PATH="${2:?--limits requires a PATH argument}"; shift 2 ;;
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

# ---- 1+2. static checks: ADR status + frozen numeric budgets ----
STATIC_JSON="$(python3 - "$ADR_PATH" "$LIMITS_RS" "$REPO_ROOT" <<'PY'
import json
import re
import subprocess
import sys

adr_path, limits_rs, repo_root = sys.argv[1], sys.argv[2], sys.argv[3]
adr_text = open(adr_path, encoding="utf-8").read()

sm = re.search(r"^-\s*状态：\s*(\S+)\s*$", adr_text, re.M)
if not sm:
    print(json.dumps({"error": "could not find '- 状态：<word>' line in " + adr_path}))
    sys.exit(0)
adr_status = sm.group(1)

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

# ---- load-generation harness existence: re-grepped from the live tree every run, never assumed.
# The p95 numbers above are distribution statistics under concurrent load -- this script cannot
# derive "no harness exists" from memory; it has to keep re-searching the tree for one on every
# run, exactly like the flow_event_payload_policy registry search in
# verify-flow-events-v0.4.sh does for its own "not built yet" gap. If a harness plausibly matching
# these patterns is ever added, this flips to True and the gates below stop asserting its absence
# (they still cannot claim the numeric target is *met* without running it -- that remains a human
# call -- but they stop repeating a now-false "nothing exists" claim).
LOAD_HARNESS_PATTERNS = [
    r"round_trip_p95",
    r"lock_hold_p95",
    r"p95_ms",
    r"LoadHarness",
    r"load_generat",
    r"ten_client",
    r"10_client",
    r"concurrent_client",
]


def grep_repo_for_load_harness(root, patterns):
    hits = []
    for sub in ("apps", "crates"):
        d = f"{root}/{sub}"
        try:
            out = subprocess.run(
                ["grep", "-rlE", "--include=*.rs", "|".join(patterns), d],
                capture_output=True, text=True, check=False,
            ).stdout
        except FileNotFoundError:
            out = ""
        hits.extend(line for line in out.splitlines() if "/target/" not in line)
    return sorted(set(hits))


load_harness_hits = grep_repo_for_load_harness(repo_root, LOAD_HARNESS_PATTERNS)

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
    "load_test_targets_not_covered": {
        "lock_hold_p95_ms": lock_hold_p95_ms_adr,
        "round_trip_p95_ms_10_clients": round_trip_p95_ms_adr,
        "reason": "distribution statistics under concurrent load; see load_harness_grep for whether a harness now exists to measure them",
    },
    "load_harness_grep": {
        "patterns_searched": LOAD_HARNESS_PATTERNS,
        "hit_files": load_harness_hits,
        "exists": len(load_harness_hits) > 0,
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
LOAD_HARNESS_EXISTS="$(jq -r '.load_harness_grep.exists' <<<"$STATIC_JSON")"

echo "=== ADR-0010 status: $ADR_STATUS (required for candidate: Accepted) ===" >&2
echo "=== frozen numeric budgets: ADR-0010 text vs apps/api/src/flow/collab/limits.rs ===" >&2
jq -r '.frozen_budgets | to_entries[] | select(.key != "warm_cache_constants_present") | "  \(.key): adr=\(.value.adr // .value) source_const=\(.value.source_const // "n/a")"' <<<"$STATIC_JSON" >&2
if [[ "$CONSTANT_VIOLATION_COUNT" -gt 0 ]]; then
  jq -r '.constant_cross_check_violations[] | "  VIOLATION: " + .' <<<"$STATIC_JSON" >&2
fi
echo "=== load-test targets this script cannot itself measure (distribution stats under concurrent load) ===" >&2
jq -r '.load_test_targets_not_covered | "  lock_hold_p95_ms=\(.lock_hold_p95_ms) round_trip_p95_ms_10_clients=\(.round_trip_p95_ms_10_clients)"' <<<"$STATIC_JSON" >&2
echo "=== load-generation harness existence (re-grepped from apps/ and crates/ every run, never assumed) ===" >&2
echo "  patterns searched: $(jq -c '.load_harness_grep.patterns_searched' <<<"$STATIC_JSON")" >&2
echo "  harness found: $LOAD_HARNESS_EXISTS" >&2
if [[ "$LOAD_HARNESS_EXISTS" == "true" ]]; then
  jq -r '.load_harness_grep.hit_files[] | "  hit: " + .' <<<"$STATIC_JSON" >&2
fi

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
if ! ( cd "$REPO_ROOT" && cargo test -p api flow::collab::snapshot:: -- --list ) > "$LIVE_LIST_LOG" 2>&1; then
  echo "FAIL: 'cargo test ... -- --list' did not succeed; see $LIVE_LIST_LOG" >&2
  exit 2
fi
LOCK_TEST_LIST_LOG="$EVIDENCE_ROOT/logs/collab.write_lock_list.log"
if ! ( cd "$REPO_ROOT" && cargo test -p api flow::collab::write::database_tests::lock_timeout_budgets_match_the_frozen_limits_v1_numbers -- --list ) > "$LOCK_TEST_LIST_LOG" 2>&1; then
  echo "FAIL: 'cargo test ... -- --list' did not succeed for the write.rs lock-timeout test; see $LOCK_TEST_LIST_LOG" >&2
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
  ( cd "$REPO_ROOT" && cargo test -p api flow::collab::snapshot:: -- --test-threads=4 ) > "$RUN_LOG" 2>&1
  SNAPSHOT_TEST_EXIT=$?
  ( cd "$REPO_ROOT" && cargo test -p api flow::collab::write::database_tests::lock_timeout_budgets_match_the_frozen_limits_v1_numbers -- --test-threads=1 ) >> "$RUN_LOG" 2>&1
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
  --argjson load_harness_exists "$LOAD_HARNESS_EXISTS" \
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
      load_test_targets_not_covered: $static_check.load_test_targets_not_covered
    },
    gates: {
      collab_architecture_adr_accepted: {
        status: (if $adr_accepted then "passed" else "failed" end),
        reason: (if $adr_accepted then null else ("ADR-0010 status is \"" + $static_check.adr_status + "\", not \"Accepted\"") end)
      },
      bounded_warm_cache_lock_hold_and_round_trip_budgets: {
        status: "failed",
        reason: (
          "hard-ceiling constants (lock wait/hold/rebase-attempts) verified against source and passing real tests, but the gate additionally requires p95 lock-hold <=25ms and 10-client accepted round-trip p95 <=250ms under concurrent load, which needs a load-generation harness to measure -- "
          + (if $load_harness_exists then
              "a possible harness now exists in this repository (see load_harness_grep.hit_files below) that this script cannot yet interpret; this gate needs MANUAL review to check whether it actually measures and meets the p95 targets, not a repeat of the old \"nothing exists\" claim"
            else
              "repo-wide grep for " + ($static_check.load_harness_grep.patterns_searched | join(", ")) + " found zero hits under apps/ and crates/ (re-checked every run) -- no such harness exists yet, this is a build gap, not a tooling limitation"
            end)
        ),
        verified_portion: {
          constant_cross_check_passed: $constants_passed,
          lock_timeout_test_passed: $lock_test_passed,
          frozen_limits_test_passed: $frozen_limits_test_passed
        },
        load_harness_grep: $static_check.load_harness_grep,
        not_covered: $static_check.load_test_targets_not_covered
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
        status: "failed",
        groundwork_test: ($snap.groundwork["9"] // null),
        reason: (
          "gate-commands.md requires a REST/WS 10-client load harness proving the same REPEATABLE READ view and cross-surface seq/hash/frontier parity -- "
          + (if $load_harness_exists then
              "a possible harness now exists in this repository (see load_harness_grep.hit_files below) that this script cannot yet interpret; this gate needs MANUAL review, not a repeat of the old \"nothing exists\" claim"
            else
              "repo-wide grep for " + ($static_check.load_harness_grep.patterns_searched | join(", ")) + " found zero hits under apps/ and crates/ (re-checked every run) -- no such harness exists yet"
            end)
        ),
        load_harness_grep: $static_check.load_harness_grep,
        note: "the source own doc comment on the groundwork test above explicitly disclaims full gate-9 coverage (\"Not the full gate 9 fixture ... out of this task scope\"); this script honors that disclaimer and never rounds the groundwork test pass up to gate passed, even when groundwork_test.status is ok"
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
