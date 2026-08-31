#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 limits verifier.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md, the v0.4
# section's "Limits verifier" paragraph ("Limits verifier 必须对每个
# caller-input 固定上限证明 exact boundary accepted、boundary+1 以正确
# limit_kind rejected、canonical head/event/event_dispatch 行零变化,并覆盖
# isolated CPU/wall/memory、rate/connection/slow queue 以及 Web
# Bootstrap.limits parity"), and contracts/limits-v1.md in full (every
# fixed-value table, the FlowLimitsV1 wire schema, and the v0.4 evidence
# schema for evidence/v0.4/limits-result.json).
#
# Covers 6 hard gates: flow_limits_exact_boundary_and_plus_one_rejection,
# isolated_decode_apply_cpu_wall_memory,
# websocket_rate_connection_and_backpressure_limits,
# bootstrap_limits_web_server_parity, limit_exceeded_kind_coverage,
# dispatch_numeric_budgets_locked.
#
# HOW IT WORKS (three layers, all re-derived from source every run --
# nothing here is a hardcoded verdict):
#
#   1. STATIC: parses every `| \`key\` | value | \`limit_kind\` | ... |`
#      row anywhere in contracts/limits-v1.md (never hand-copied numbers)
#      plus the frozen `FlowLimitsV1 = {...}` wire-schema block, and
#      cross-checks every value against the identically-UPPER_SNAKE-named
#      Rust constant in apps/api/src/flow/collab/limits.rs -- that
#      module's own naming convention makes this a 1:1, no-alias-table
#      cross-check. Also statically greps apps/api/src, crates/collab-core
#      and frontend/src for structural facts this script needs to judge
#      wire-correctness and reachability: whether each limit_kind's
#      enforcement code exists at all outside its own wire-report
#      constant, whether the REST envelope (apps/api/src/error.rs /
#      response.rs) can even carry a `details` object, whether specific
#      rejection call sites pass `details=None` instead of the required
#      `{limit_kind,limit}`, and whether frontend/src/lib/flow/types.ts's
#      FlowLimitsV1 interface's field set matches the server's 36-field
#      wire schema.
#
#   2. DYNAMIC: runs every real cargo test this script found that
#      exercises a limit_kind boundary -- pure-logic exact/+1 tests in
#      crates/collab-core/src/limits.rs and
#      apps/api/src/flow/collab/{registry,snapshot}.rs, plus two
#      DB-backed e2e tests (routes::collab / routes::flow) -- and folds
#      each test's real ok/FAILED status into its boundary_case.
#
#   3. DEFERRAL: `persistence_path` (document lock wait/hold/p95,
#      round-trip p95, rebase exhaustion, snapshot soft/hard triggers) and
#      part of `delivery_path` (no_subscribers reaping,
#      coalescing-cap-starts-new-row) are contractually owned by
#      scripts/verify-flow-collab-architecture.sh and
#      scripts/verify-flow-events-v0.4.sh respectively (limits-v1.md says
#      so explicitly: "这些是...内部架构预算...10-client fixture、cache
#      exact/eviction...写入 evidence/v0.4/collab-architecture-result.json").
#      This script reads those two sibling artifacts IF they exist AND
#      their own `source_head` matches this run's HEAD (a stale or
#      missing sibling is `not_covered`, never silently trusted or
#      re-fabricated), rather than re-running their expensive DB test
#      suites itself.
#
# HONEST RESULT (recorded in the JSON, `passed` forced false while any gap
# below remains open -- this script never rounds a partial result up to a
# pass):
#
#   The contract defines 32 fixed `limit_kind` values, but four package-import
#   kinds have no v0.4 surface and are recorded with the mandatory reason code
#   `not_applicable_until_v0_8`. The other 28 keep the full boundary and
#   caller-reachable wire criteria below; the exemption is an exact allowlist,
#   never an `import_*` wildcard.
#
#   Of the 28 applicable v0.4 `limit_kind` values, this run finds: 6 have a real
#   exact/+1 unit test but the enforcing code
#   (crates/collab-core/src/limits.rs::check_operation*) has ZERO call
#   sites in apps/api/src -- not wired into any endpoint. 2 (presence
#   entries per connection/document) have a real exact/+1 unit test AND
#   are genuinely wired (apps/api/src/flow/collab/registry.rs), but the
#   caller-visible WS rejection (session.rs) sends `details=None` for
#   both, so the wire response can never carry the required `limit_kind`
#   at all. 1 (update_bytes) has a real DB-backed e2e test, is wired on
#   the WS path, but the test uses a 70000-byte oversized update rather
#   than an exact 65536/65537 boundary, the WS details omit the
#   contract-required `limit` field, and the REST path drops `details`
#   entirely (apps/api/src/error.rs's ApiError only ever carries a plain
#   String -- there is no `details` field in the REST envelope at all,
#   for ANY limit_kind, which is why REST can never surface limit_kind
#   for anything). The 3 `isolation` kinds (decode_apply_cpu_ms,
#   decode_apply_wall_ms, isolated_apply_memory_bytes) are wired, not
#   absent: apps/api/src/flow/collab/write.rs's hydrate_and_apply calls
#   collab_core::isolation::isolated_apply(...), whose real enforcement
#   (a wall-clock watchdog, a counting-allocator memory ceiling, and a
#   SIGPROF CPU timer) lives in crates/collab-core/src/isolation/ -- but
#   no test anywhere proves the exact/+1 numeric boundary for any of the
#   three, only a qualitative "eventually kills a pathological input and
#   reports a ceiling" end-to-end test, so all 3 still fail this gate on
#   missing boundary evidence rather than missing enforcement. The
#   remaining 20 have zero enforcement call sites found anywhere outside
#   their own wire-report constant -- verified absent, not merely
#   untested. Most of `connection_rate_queue`, `bootstrap_parity`
#   (frontend's FlowLimitsV1 only declares 10 of 36 fields, 2 of those
#   under different names, and is never fetched from a live Bootstrap
#   response at all) and most of `delivery_path` (most numeric budgets
#   are still `status: unset` in the contract itself) are failed for the
#   same class of reason as those 20: this script CAN check them (and
#   did), and what it finds is either "does not exist" or "exists but is
#   wire-broken" -- never a shrug.
#
# Exit codes: 0 = all 6 gates recomputed to passed (does not happen today
# -- see above), 1 = ran to completion and wrote
# evidence/v0.4/limits-result.json with one or more gates not passed, 2 =
# usage/tool/environment error, OR any DB-backed dynamic test was silently
# skipped because OPENPR_TEST_DATABASE_URL is not set.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Shared --adr/--contract/--limits path resolution (absolute -> as-is;
# relative-to-CWD -> as-is; otherwise resolved against --contracts-root;
# unresolvable -> FAIL naming both attempted paths).
# shellcheck source=scripts/lib/flow_contract_path.sh
source "$ROOT_DIR/scripts/lib/flow_contract_path.sh"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT=""
REPO_ROOT="$ROOT_DIR"
CONTRACT_PATH=""
JSON_MODE=0
SKIP_CARGO_TEST=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-limits-v0.4.sh --contract PATH --json [OPTIONS]

Cross-checks every fixed-value row of contracts/limits-v1.md (all four
limit_kind tables plus the FlowLimitsV1 wire schema) against
apps/api/src/flow/collab/limits.rs's constants, statically determines
which limit_kind values have real caller-facing enforcement versus
verified-absent or wire-broken enforcement, runs every real matching
cargo test (crates/collab-core + apps/api, pure-logic and two DB-backed
e2e tests), and defers persistence_path / part of delivery_path to
evidence/v0.4/collab-architecture-result.json and
evidence/v0.4/flow-events-result.json when those are present and
source_head-matched. Writes evidence/v0.4/limits-result.json.

Options:
  --contract PATH          Path to contracts/limits-v1.md. Default:
                          <contracts-root>/contracts/limits-v1.md
                          A relative path is resolved against the
                          current directory first, then against
                          --contracts-root.
  --contracts-root DIR     Root containing contracts/. Default:
                          /opt/working/sylvode-flow
  --evidence-root DIR     Required. Where limits-result.json is written, and where
                          the sibling collab-architecture-result.json /
                          flow-events-result.json are read from.
  --repo-root DIR         Repository containing apps/api, crates/,
                          frontend/ and the cargo workspace. Default: this
                          checkout.
  --skip-cargo-test        Skip every dynamic cargo test (fast iteration
                          only; the written evidence records this and
                          every boundary_case that needed a dynamic test
                          is treated as failed with dynamic evidence
                          omitted).
  --json                  Required for CLI-contract compatibility.
  -h, --help              Show this help and exit 0.

Exit codes: 0 all 6 gates passed, 1 ran to completion with one or more
gates not passed (the honest, normal outcome today), 2 usage/tool/
environment error or a DB-backed dynamic test was silently skipped
because OPENPR_TEST_DATABASE_URL is not set.
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
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
if [[ -z "$EVIDENCE_ROOT" ]]; then
  echo "FAIL: --evidence-root is required; evidence must never default into the contract repository" >&2
  usage >&2
  exit 2
fi
for tool in jq git python3 cargo sha256sum; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    exit 2
  fi
done

if [[ -z "$CONTRACT_PATH" ]]; then
  CONTRACT_PATH="$CONTRACTS_ROOT/contracts/limits-v1.md"
fi
if ! CONTRACT_PATH="$(flow_resolve_contract_path --contract "$CONTRACT_PATH" "$CONTRACTS_ROOT")"; then
  exit 2
fi
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi
# The two v0.8-owned retention budgets are excluded from this release's freeze floor only while a
# v0.8 gate actually re-demands them (see V08_DEFERRED_DISPATCH_BUDGET_KEYS in the static pass).
# Without reading the v0.8 gate file, "deferred" and "permanently exempt" are indistinguishable,
# so the exclusion is conditioned on that file naming its paired gate.
V08_GATE_YAML="$CONTRACTS_ROOT/gates/v0.8-gate.yaml"
V05_GATE_YAML="$CONTRACTS_ROOT/gates/v0.5-gate.yaml"
for f in "$V08_GATE_YAML" "$V05_GATE_YAML"; do
  if [[ ! -f "$f" ]]; then
    echo "FAIL: later-release gate file not found, cannot prove a deferred budget is re-demanded later: $f" >&2
    exit 2
  fi
done

LIMITS_RS="$REPO_ROOT/apps/api/src/flow/collab/limits.rs"
COLLAB_CORE_LIMITS_RS="$REPO_ROOT/crates/collab-core/src/limits.rs"
COLLAB_CORE_ERROR_RS="$REPO_ROOT/crates/collab-core/src/error.rs"
REGISTRY_RS="$REPO_ROOT/apps/api/src/flow/collab/registry.rs"
SESSION_RS="$REPO_ROOT/apps/api/src/flow/collab/session.rs"
WRITE_RS="$REPO_ROOT/apps/api/src/flow/collab/write.rs"
COMMAND_RS="$REPO_ROOT/apps/api/src/flow/command.rs"
QUERY_RS="$REPO_ROOT/apps/api/src/flow/query.rs"
BOOTSTRAP_RS="$REPO_ROOT/apps/api/src/flow/collab/bootstrap.rs"
ERROR_RS="$REPO_ROOT/apps/api/src/error.rs"
RESPONSE_RS="$REPO_ROOT/apps/api/src/response.rs"
DISPATCHER_RS="$REPO_ROOT/apps/api/src/events/dispatcher.rs"
MIGRATION_SQL="$REPO_ROOT/migrations/0054_flow_data_layer.sql"
FRONTEND_TYPES_TS="$REPO_ROOT/frontend/src/lib/flow/types.ts"
FRONTEND_LIMITS_TS="$REPO_ROOT/frontend/src/lib/flow/limits.ts"
# The isolated-apply enforcement boundary (`decode_apply_cpu_ms_max`/`decode_apply_wall_ms_max`/
# `isolated_apply_memory_bytes_max`) lives in crates/collab-core/src/isolation/, not apps/api/src --
# read those source files too so this script can see the cross-crate call path
# (apps/api/src/flow/collab/write.rs's `collab_core::isolation::isolated_apply(...)` call, and the
# single-source constants/CPU-timer/allocator/wall-watchdog that actually enforce them) instead of
# concluding "does not exist" from an apps/api/src-only grep.
COLLAB_CORE_ISOLATION_LIMITS_RS="$REPO_ROOT/crates/collab-core/src/isolation/limits.rs"
COLLAB_CORE_ISOLATION_HOST_RS="$REPO_ROOT/crates/collab-core/src/isolation/host.rs"
COLLAB_CORE_ISOLATION_ALLOC_RS="$REPO_ROOT/crates/collab-core/src/isolation/alloc.rs"
COLLAB_CORE_ISOLATION_CHILD_RUNTIME_RS="$REPO_ROOT/crates/collab-core/src/isolation/child_runtime.rs"
COLLAB_CORE_ISOLATED_APPLY_WORKER_RS="$REPO_ROOT/crates/collab-core/src/bin/isolated_apply_worker.rs"
# `page_size`'s real REST enforcement (`query::validate_limit`, called from `list_flow_objects`)
# and its exact/+1 boundary test both live in the REST route handler file itself, not in
# flow/query.rs alone -- read it too so a real boundary test written here (rather than in
# query.rs, which has none) can actually be found.
ROUTES_FLOW_RS="$REPO_ROOT/apps/api/src/routes/flow.rs"
for f in "$LIMITS_RS" "$COLLAB_CORE_LIMITS_RS" "$COLLAB_CORE_ERROR_RS" "$REGISTRY_RS" "$SESSION_RS" \
         "$WRITE_RS" "$COMMAND_RS" "$QUERY_RS" "$BOOTSTRAP_RS" "$ERROR_RS" "$RESPONSE_RS" "$DISPATCHER_RS" \
         "$MIGRATION_SQL" "$FRONTEND_TYPES_TS" "$FRONTEND_LIMITS_TS" "$COLLAB_CORE_ISOLATION_LIMITS_RS" \
         "$COLLAB_CORE_ISOLATION_HOST_RS" "$COLLAB_CORE_ISOLATION_ALLOC_RS" \
         "$COLLAB_CORE_ISOLATION_CHILD_RUNTIME_RS" "$COLLAB_CORE_ISOLATED_APPLY_WORKER_RS" \
         "$ROUTES_FLOW_RS"; do
  if [[ ! -f "$f" ]]; then
    echo "FAIL: source file not found (nothing to statically verify): $f" >&2
    exit 2
  fi
done

mkdir -p "$EVIDENCE_ROOT" "$EVIDENCE_ROOT/logs"
FLOW_LIMITS_TARGET="${CARGO_TARGET_DIR:-/opt/worker/.cache/flow-v04-limits-target}"
mkdir -p "$FLOW_LIMITS_TARGET"
export CARGO_TARGET_DIR="$FLOW_LIMITS_TARGET"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
CONTRACT_SHA256="$(sha256sum "$CONTRACT_PATH" | awk '{print $1}')"

# ---- 1. static analysis (contract parse + source cross-check + structural greps) ----
STATIC_JSON_FILE="$EVIDENCE_ROOT/logs/limits.static.json"
if ! python3 - "$CONTRACT_PATH" "$LIMITS_RS" "$COLLAB_CORE_LIMITS_RS" "$COLLAB_CORE_ERROR_RS" \
      "$REGISTRY_RS" "$SESSION_RS" "$WRITE_RS" "$COMMAND_RS" "$QUERY_RS" "$BOOTSTRAP_RS" \
      "$ERROR_RS" "$RESPONSE_RS" "$DISPATCHER_RS" "$MIGRATION_SQL" "$FRONTEND_TYPES_TS" "$FRONTEND_LIMITS_TS" \
      "$COLLAB_CORE_ISOLATION_LIMITS_RS" "$COLLAB_CORE_ISOLATION_HOST_RS" "$COLLAB_CORE_ISOLATION_ALLOC_RS" \
      "$COLLAB_CORE_ISOLATION_CHILD_RUNTIME_RS" "$COLLAB_CORE_ISOLATED_APPLY_WORKER_RS" \
      "$ROUTES_FLOW_RS" "$V08_GATE_YAML" "$V05_GATE_YAML" \
      > "$STATIC_JSON_FILE" 2>"$EVIDENCE_ROOT/logs/limits.static.err.log" <<'PY'
import json
import re
import sys

(contract_path, limits_rs, collab_core_limits_rs, collab_core_error_rs, registry_rs, session_rs,
 write_rs, command_rs, query_rs, bootstrap_rs, error_rs, response_rs, dispatcher_rs, migration_sql,
 frontend_types_ts, frontend_limits_ts, collab_core_isolation_limits_rs, collab_core_isolation_host_rs,
 collab_core_isolation_alloc_rs, collab_core_isolation_child_runtime_rs,
 collab_core_isolated_apply_worker_rs, routes_flow_rs, v08_gate_yaml, v05_gate_yaml) = sys.argv[1:25]


def read(p):
    with open(p, encoding="utf-8") as fh:
        return fh.read()


contract = read(contract_path)

row_re = re.compile(r"^\|\s*`([a-z0-9_]+)`\s*\|\s*([^|]+?)\s*\|\s*`([a-z0-9_]+)`\s*\|", re.M)
rows = []
for m in row_re.finditer(contract):
    key, raw_value, limit_kind = m.group(1), m.group(2), m.group(3)
    is_unset = "unset" in raw_value
    value = None
    if not is_unset:
        num_m = re.search(r"(\d[\d,]*)", raw_value)
        if num_m:
            value = int(num_m.group(1).replace(",", ""))
    rows.append({"key": key, "raw_value": raw_value, "limit_kind": limit_kind, "value": value, "unset": is_unset})

# `grants_per_request_max`/`object_grants_max` carry a v0.5 first-shipped
# marker in the contract's own prose even though one of them already has a
# concrete number in its table row; excluded here by name per that prose,
# not derivable from the row's own cell.
V05_DEFERRED_LIMIT_KINDS = {"grants_per_request", "object_grants"}
V08_IMPORT_LIMIT_KIND_ALLOWLIST = {
    "import_archive_bytes",
    "import_expanded_bytes",
    "import_entry_count",
    "import_compression_ratio",
}
VERSION_BOUNDARY_REASON_CODE = "not_applicable_until_v0_8"

# Delivery/dispatch budget freeze floor: which `status: unset` budgets THIS release must have
# frozen. gates/gate-commands.md's `dispatch_numeric_budgets_locked` paragraph calls its list
# closed, so the default here is "every `status: unset` budget row in contracts/limits-v1.md is
# required", and every exclusion has to earn itself from the contract, one named key at a time.
#
# An exclusion is legitimate only when BOTH hold, and both are re-derived from the contract on
# every run (see the guards below), never asserted by this script alone:
#   1. contracts/limits-v1.md assigns the budget to a LATER release's implementer (`set_by`), so
#      v0.4 would be freezing a number it does not own -- the criterion asking this release to
#      prove what the next one delivers; and
#   2. that later release's gate file re-demands the freeze, so the exclusion is a deferral with
#      a named owner rather than a permanent amnesty.
#
# Named keys ONLY -- deliberately not a `*retention*` pattern. A pattern would have swallowed
# `delivery_retention_days` and `dispatch_expanded_retention_days`, both v0.4-owned and frozen,
# and would silently absorb any future retention budget nobody got round to freezing.
V08_DEFERRED_DISPATCH_BUDGET_KEYS = {
    "delivery_source_retention_days",
    "replay_max_window_days",
}
# Each allowlisted key must prove its own deferral from the contract row it appears in, so the
# allowlist cannot be widened by editing this script alone: the row must still say `status: unset`
# (a key that got frozen must leave the allowlist and be verified, not stay excused) and must name
# the v0.8 implementer as its setter (a v0.4-owned budget can never qualify).
V08_SET_BY_MARKER = "`set_by`：v0.8 实现者"
V08_PAIRED_BUDGET_GATE = "deferred_retention_budgets_frozen"

# The v0.5 authorization surface's own unset budget. gate-commands.md gives `object_grants_max` to
# `authz_numeric_budgets_locked` (v0.5), not to this gate, and limits-v1.md's structured block
# assigns it to the v0.5 implementer. `grants_per_request_max` is already frozen at 100 and so is
# never in the unset set at all; it is named here only so the intent of this exclusion stays
# readable.
#
# `subscribers_per_workspace_max` was in this exclusion until 2026-08-31 and does NOT belong here:
# limits-v1.md's structured block sets it by the **v0.4 dispatcher 实现者**, and gate-commands.md
# names it in this gate's own closed 12-key list. Excluding it made this gate silently one key
# short of the list it claims to enforce. No version-boundary argument reaches a v0.4-owned
# budget, so it is required below like any other.
V05_AUTHZ_SURFACE_KEYS = {"object_grants_max", "grants_per_request_max"}
V05_SET_BY_MARKER = "set_by: v0.5 授权面实现者"
V05_PAIRED_BUDGET_GATE = "authz_numeric_budgets_locked"

fixed_limit_rows = [
    r for r in rows if not r["unset"] and r["limit_kind"] not in V05_DEFERRED_LIMIT_KINDS
]
version_boundary_exemptions = {
    r["limit_kind"]: {
        "limit_kind": r["limit_kind"],
        "status": VERSION_BOUNDARY_REASON_CODE,
        "reason_code": VERSION_BOUNDARY_REASON_CODE,
        "surface_version": "v0.8",
    }
    for r in fixed_limit_rows
    if r["limit_kind"] in V08_IMPORT_LIMIT_KIND_ALLOWLIST
}
non_allowlisted_exemptions = sorted(set(version_boundary_exemptions) - V08_IMPORT_LIMIT_KIND_ALLOWLIST)
missing_allowlisted_exemptions = sorted(V08_IMPORT_LIMIT_KIND_ALLOWLIST - set(version_boundary_exemptions))
if non_allowlisted_exemptions or missing_allowlisted_exemptions:
    raise ValueError(
        "version-boundary exemption allowlist violation: "
        f"non_allowlisted={non_allowlisted_exemptions}, missing_required={missing_allowlisted_exemptions}"
    )
if VERSION_BOUNDARY_REASON_CODE not in contract:
    raise ValueError(
        f"contract does not declare mandatory version-boundary reason code {VERSION_BOUNDARY_REASON_CODE!r}"
    )

v0_4_rows = [r for r in fixed_limit_rows if r["limit_kind"] not in version_boundary_exemptions]
deferred_or_unset_rows = [r for r in rows if r["unset"] or r["limit_kind"] in V05_DEFERRED_LIMIT_KINDS]
expected_limit_kinds = sorted({r["limit_kind"] for r in v0_4_rows})
fixed_limit_kind_count = len({r["limit_kind"] for r in fixed_limit_rows})
evaluated_limit_kind_count = len(expected_limit_kinds)
if fixed_limit_kind_count != 32 or evaluated_limit_kind_count != 28 or len(version_boundary_exemptions) != 4:
    raise ValueError(
        "v0.4 limit-kind accounting must be exactly 32 = 28 evaluated + 4 version-boundary "
        f"exemptions, got {fixed_limit_kind_count} = {evaluated_limit_kind_count} + "
        f"{len(version_boundary_exemptions)}"
    )

wire_m = re.search(r"FlowLimitsV1 = \{(.*?)\n\}", contract, re.S)
wire_fields = {}
if wire_m:
    for line in wire_m.group(1).splitlines():
        fm = re.match(r'\s*(\w+):\s*"?([\w.]+)"?,?\s*$', line)
        if fm:
            name, val = fm.group(1), fm.group(2)
            wire_fields[name] = val if name == "version" else int(val)

limits_rs_text = read(limits_rs)
collab_core_limits_text = read(collab_core_limits_rs)
registry_text = read(registry_rs)
session_text = read(session_rs)
write_text = read(write_rs)
command_text = read(command_rs)
query_text = read(query_rs)
bootstrap_text = read(bootstrap_rs)
error_rs_text = read(error_rs)
response_rs_text = read(response_rs)
dispatcher_text = read(dispatcher_rs)
migration_text = read(migration_sql)
frontend_types_text = read(frontend_types_ts)
frontend_limits_text = read(frontend_limits_ts)
collab_core_isolation_limits_text = read(collab_core_isolation_limits_rs)
collab_core_isolation_host_text = read(collab_core_isolation_host_rs)
collab_core_isolation_alloc_text = read(collab_core_isolation_alloc_rs)
collab_core_isolation_child_runtime_text = read(collab_core_isolation_child_runtime_rs)
collab_core_isolated_apply_worker_text = read(collab_core_isolated_apply_worker_rs)
routes_flow_rs_text = read(routes_flow_rs)

const_re = re.compile(r"pub const (\w+):\s*[\w<>&']+\s*=\s*([\d_]+)\s*;")
rust_consts = {m.group(1): int(m.group(2).replace("_", "")) for m in const_re.finditer(limits_rs_text)}

# `apps/api/src/flow/collab/limits.rs`'s own `pub const NAME = <literal>;` scan above is the
# primary source for the row/wire cross-check below, but since the isolation-limits and
# DocumentLimits-default single-source-of-truth fix, several of that file's constants are no
# longer bare literals -- they `pub use collab_core::isolation::{...}` or derive from
# `collab_core::DocumentLimits::DEFAULT`/`collab_core::limits::UPDATE_BYTES_MAX` instead (see that
# file's own comments). `rust_consts` alone would report `source_value=None` for every one of
# those and wrongly flag them as cross-check violations. Resolve those specific names against
# collab-core's own literal `pub const` declarations instead (crates/collab-core/src/limits.rs and
# crates/collab-core/src/isolation/limits.rs -- the two canonical single-source files), but only
# when apps/api's own file actually references that name somewhere (a `pub use` or a `= ...NAME`
# derivation) -- so a name apps/api's file never mentions at all still reports `source_value=None`
# rather than silently trusting collab-core's copy for something apps/api never wired up.
collab_core_const_re = re.compile(r"pub const (\w+):\s*[\w<>&']+\s*=\s*([\d_]+)\s*;")
collab_core_literal_consts = {
    m.group(1): int(m.group(2).replace("_", ""))
    for text in (collab_core_limits_text, collab_core_isolation_limits_text)
    for m in collab_core_const_re.finditer(text)
}


def resolve_const(const_name: str):
    if const_name in rust_consts:
        return rust_consts[const_name]
    if const_name in collab_core_literal_consts and re.search(rf"\b{re.escape(const_name)}\b", limits_rs_text):
        return collab_core_literal_consts[const_name]
    return None


def const_name_for_key(key: str) -> str:
    return key.upper()


row_violations = []
row_cross_check = []
for r in fixed_limit_rows:
    const_name = const_name_for_key(r["key"])
    source_value = resolve_const(const_name)
    ok = source_value is not None and source_value == r["value"]
    row_cross_check.append({"key": r["key"], "limit_kind": r["limit_kind"], "contract_value": r["value"],
                             "source_const": const_name, "source_value": source_value, "matches": ok})
    if not ok:
        row_violations.append(f"{r['key']}: contract={r['value']} source {const_name}={source_value}")

wire_violations = []
wire_cross_check = []
for name, expected in wire_fields.items():
    if name == "version":
        cn = "sylvode.flow.limits.v1"
        wire_cross_check.append({"field": name, "contract_value": expected, "source_value": cn, "matches": expected == cn})
        continue
    const_name = const_name_for_key(name)
    source_value = resolve_const(const_name)
    ok = source_value == expected
    wire_cross_check.append({"field": name, "contract_value": expected, "source_const": const_name, "source_value": source_value, "matches": ok})
    if not ok:
        wire_violations.append(f"{name}: contract={expected} source {const_name}={source_value}")


def count(pattern, text):
    return len(re.findall(pattern, text))


findings = {}
findings["collab_core_check_operation_call_sites_in_command_rs"] = (
    count(r"\bcheck_operation\(", command_text) + count(r"\bcheck_operation_batch_count\(", command_text)
)

response_struct_m = re.search(r"struct ApiResponse.*?\n\}", response_rs_text, re.S)
findings["rest_response_error_fn_has_details_param"] = bool(re.search(r"fn error\([^)]*details[^)]*\)", response_rs_text))
findings["rest_apiresponse_struct_has_details_field"] = bool(response_struct_m and "details" in response_struct_m.group(0))

map_fn_m = re.search(r"fn map_write_rejection.*?\n\}\n", command_text, re.S)
findings["map_write_rejection_reads_details_field"] = bool(map_fn_m and "rejected.details" in map_fn_m.group(0))

reject_fn_m = re.search(r"fn reject_from_collab_error.*?\n\}\n", write_text, re.S)
findings["write_rs_limit_exceeded_details_has_limit_kind"] = bool(reject_fn_m and '"limit_kind"' in reject_fn_m.group(0))
findings["write_rs_limit_exceeded_details_has_limit_field"] = bool(reject_fn_m and re.search(r'"limit"\s*:', reject_fn_m.group(0)))

presence_reject_m = re.search(r"Err\(PresenceLimit::PerConnection \| PresenceLimit::PerDocument\)\s*=>\s*\{(.*?)\}", session_text, re.S)
findings["session_rs_presence_limit_rejection_details_is_none"] = bool(
    presence_reject_m and re.search(r"rejected_frame\([^)]*,\s*None\s*\)", presence_reject_m.group(0))
)

ttl_block_m = re.search(r"ttl_seconds = ttl_seconds\.unwrap_or.*?upsert_presence\(", session_text, re.S)
findings["session_rs_presence_ttl_rejection_details_is_none"] = bool(
    ttl_block_m and re.search(r"RejectedCode::LimitExceeded,\s*false,\s*None", ttl_block_m.group(0))
)

findings["query_rs_validate_limit_returns_plain_string"] = bool(
    re.search(r"ApiError::BadRequest\(format!\(\"limit must be at most", query_text)
)

findings["presence_payload_bytes_max_enforcement_call_sites"] = count(r"PRESENCE_PAYLOAD_BYTES_MAX", session_text)

# `update_bytes_max` is 65536; the contract's "exact boundary accepted, boundary+1 rejected"
# requirement means a real test must exercise exactly 65537 bytes, not merely "some oversized
# value". Grepped dynamically so this flips the moment such a test is added.
findings["update_bytes_exact_boundary_test_exists"] = bool(re.search(r"\b65537\b", write_text))

# `semantic_patch_bytes` (`semantic_patch_json_bytes_max`) has no REST/MCP endpoint anywhere in
# this repo's apps/api sources that this script reads -- its wire constant exists only as a
# reported number. Any real enforcement call site (not just the wire-report constant in
# limits.rs, which is deliberately excluded here) would reference the field name outside
# limits.rs in one of these caller-facing modules.
findings["semantic_patch_bytes_enforcement_found"] = bool(
    re.search(r"semantic_patch_json_bytes_max|SEMANTIC_PATCH_JSON_BYTES_MAX",
              command_text + write_text + query_text + bootstrap_text + session_text)
)

# `unknown_version_read_only` (bootstrap_parity): whether the frontend has any version-negotiation
# code at all for an unrecognized `FlowLimitsV1.version`. Zero hits today -- but grepped, not
# asserted, so this stops being "not_covered" the moment such code is written.
findings["frontend_unknown_version_handling_found"] = bool(
    re.search(r"unknown_version|unknownVersion|version_negotiation|versionNegotiation",
              frontend_types_text + frontend_limits_text)
)


# ---- generic "does a real boundary test exist for this limit_kind" scan ----
#
# Several `limit_kind`s below (isolation, connection/rate/queue, import/scan, websocket_frame_bytes,
# presence_payload_bytes, presence_ttl_seconds, page_size) currently have ZERO enforcement call
# sites at all, so their `status` is unambiguously `failed` regardless of test coverage. But a
# `ref_count > 0` alone (call sites exist) must NEVER be sufficient for `passed` on its own -- that
# would let a future PR wire the check without ever proving the exact/+1 boundary and still turn
# this gate green. So every one of these cases additionally requires a real boundary test: scanned
# by finding every `#[test]`/`#[tokio::test]`-attributed function name across the caller-facing
# modules this script reads, and matching it against BOTH the limit_kind's own name tokens and its
# Rust constant's name tokens (a test is very likely to be named after one or the other; requiring
# only one of the two token sets, not literal substring equality, tolerates paraphrasing like
# "per_connection_presence_ceiling" for `presence_entries_per_connection`).
#
# Matching is whole-word (both sides split on `_`, never raw substring) and singular/plural
# insensitive (a trailing "s" is stripped before comparing), because Rust test names paraphrase
# freely: `user_connections`'s own tokens are {user, connections} but the real test is named
# `per_user_connection_ceiling_is_enforced_and_freed_on_unregister` (singular "connection"), and
# `slow_consumer_queue_frames`'s tokens include "frames" while its test says "queue_frame_ceiling"
# (singular "frame"). A required token set must be a SUBSET of the candidate test name's own token
# set for a match -- every token has to land on some whole word in the test name, in any order.
# This is intentionally not "any one token matches": that would let an unrelated test like
# `connection_registry_smoke` match every *_connections limit_kind through the word "connection"
# alone, without the specific "user"/"document"/"workspace" qualifier that makes it real coverage.
TEST_FN_RE = re.compile(
    # An attribute line may carry a trailing `// ...` comment after its closing `]` (common for
    # justifying a test-only `#[allow]`), so anything up to the newline is allowed after the
    # bracket -- requiring `]` to be followed immediately by a newline silently dropped such tests
    # from this scan and reported their limit_kind as having no boundary test at all.
    r"#\[(?:tokio::)?test\][^\n]*\n(?:\s*#\[[^\n]*\][^\n]*\n)*\s*(?:pub(?:\([^)]*\))?\s+)?(?:async fn|fn) (\w+)\s*\("
)


def test_fn_names(text):
    return TEST_FN_RE.findall(text)


TEST_SOURCE_TEXT_PARTS = (
    ("collab/session.rs", session_text),
    ("collab/registry.rs", registry_text),
    ("collab/write.rs", write_text),
    ("flow/command.rs", command_text),
    ("flow/query.rs", query_text),
    ("collab/bootstrap.rs", bootstrap_text),
    ("response.rs", response_rs_text),
    ("error.rs", error_rs_text),
    ("events/dispatcher.rs", dispatcher_text),
    # The isolation boundary's own enforcement/test sites -- included so a real exact/+1 boundary
    # test for decode_apply_cpu_ms_max/decode_apply_wall_ms_max/isolated_apply_memory_bytes_max
    # would actually be found here if one existed, rather than this scan only ever looking at
    # apps/api/src (where that test could not live, since the enforcement itself is in this crate).
    ("collab-core/isolation/host.rs", collab_core_isolation_host_text),
    ("collab-core/isolation/alloc.rs", collab_core_isolation_alloc_text),
    ("collab-core/isolation/child_runtime.rs", collab_core_isolation_child_runtime_text),
    ("collab-core/bin/isolated_apply_worker.rs", collab_core_isolated_apply_worker_text),
    # The isolation boundary's own single-source ceiling declaration: its own module test
    # (`frozen_values_match_contracts_limits_v1`) pins the three frozen values but is not itself
    # an exact/+1 boundary test, so including it here does not by itself make any isolation
    # limit_kind pass -- it just makes this scan consistent with the fact that this module is
    # dynamically covered (the `limits::tests::` cargo filter's substring match already runs it;
    # see the isolation run_group below).
    ("collab-core/isolation/limits.rs", collab_core_isolation_limits_text),
    # `page_size`'s real boundary test (`list_objects_endpoint_rejects_page_size_over_...`) lives
    # in the REST route handler file, not in flow/query.rs (which has no #[test] at all for this
    # limit_kind) -- see the ROUTES_FLOW_RS read above.
    ("routes/flow.rs", routes_flow_rs_text),
)
TEST_SOURCE_BY_LABEL = {label: text for label, text in TEST_SOURCE_TEXT_PARTS}
ALL_TEST_FN_NAMES = [
    (src_label, name) for src_label, text in TEST_SOURCE_TEXT_PARTS for name in test_fn_names(text)
]


def _singularize(word: str) -> str:
    # Minimal, deliberately conservative stemmer: only strips a bare trailing "s" (never "ss"),
    # and only on words long enough that the strip cannot hollow the word out entirely. This is
    # enough to unify the plural/singular pairs that actually occur in limit_kind/const/test-name
    # tokens here (connections/connection, frames/frame, bytes/byte, rates/rate) without the
    # false-equivalence risk of a fuller stemmer (e.g. "status" must not become "statu").
    if len(word) > 3 and word.endswith("s") and not word.endswith("ss"):
        return word[:-1]
    return word


def name_tokens(s: str) -> set:
    return {_singularize(t) for t in s.lower().split("_") if t and t != "max"}


_FN_NAME_TOKENS_CACHE: dict = {}


def _fn_name_tokens(fn_name: str) -> set:
    cached = _FN_NAME_TOKENS_CACHE.get(fn_name)
    if cached is None:
        cached = name_tokens(fn_name)
        _FN_NAME_TOKENS_CACHE[fn_name] = cached
    return cached


# This gate is named `flow_limits_exact_boundary_and_plus_one_rejection`, so a name match alone is
# not enough: the matched test has to actually prove the rejection half. `scan_budget` was passing
# on `check_scan_budget_accepts_the_exact_..._boundary`, a test that only asserts the ceiling
# itself is accepted -- renaming its sibling `..._rejects_one_row_past_...` left the case green,
# which means nothing here was checking that going over the limit is refused.
#
# So: collect every name match, then keep only those whose body asserts a rejection. A test that
# merely accepts the boundary cannot satisfy a gate that promises +1 is refused.
REJECTION_IN_NAME_RE = re.compile(r"reject|refus|denie|plus_one|exceed|over_|_over\\b|too_(?:many|large|big)")
REJECTION_ASSERTION_RE = re.compile(
    r"\bis_err\(\)|\bexpect_err\(|\bunwrap_err\(|\.err\(\)|\bErr\(|Rejected|limit_exceeded|LimitExceeded|Exceeded|Refused|Denied"
)


def _test_body(src_text: str, fn_name: str) -> str:
    """The source between this test's `fn` line and whatever attribute starts the next item.

    Deliberately coarse -- it only feeds a substring search for rejection assertions, so
    overshooting into a following non-test item costs nothing that a false *negative* would not
    cost more.
    """
    match = re.search(r"(?:async fn|fn)\s+" + re.escape(fn_name) + r"\s*\(", src_text)
    if not match:
        return ""
    rest = src_text[match.end():]
    # Stop at the next item's FIRST line, doc comment included -- stopping only at its `#[test]`
    # swept the following test's `///` lines into this one's body, and those lines name the very
    # rejection this scan looks for. That is how `check_scan_budget_accepts_the_exact_...` kept
    # qualifying: its own body only asserts `is_ok()`, but its neighbour's doc comment says
    # "rejects one row past ...".
    next_item = re.search(r"\n\s*(?:///|#\[)", rest)
    return rest[: next_item.start()] if next_item else rest


def boundary_test_covering(limit_kind: str, const: str | None = None):
    token_sets = [name_tokens(limit_kind)]
    if const:
        token_sets.append(name_tokens(const))
    for src_label, fn_name in ALL_TEST_FN_NAMES:
        fn_tokens = _fn_name_tokens(fn_name)
        if not any(tokens and tokens.issubset(fn_tokens) for tokens in token_sets):
            continue
        # Either half is enough, because neither alone is reliable: a rejection can be asserted
        # through a plain enum variant (`RateOutcome::Exceeded`) that no Result-shaped pattern
        # matches, and a test can prove rejection without saying so in its name
        # (`per_workspace_connection_ceiling_is_enforced` asserts on `.err()`). What both halves
        # do exclude is the case this check exists for: a test that only asserts the ceiling
        # itself is accepted, named accordingly and asserting `is_ok()`.
        if REJECTION_IN_NAME_RE.search(fn_name) or REJECTION_ASSERTION_RE.search(
            _test_body(TEST_SOURCE_BY_LABEL[src_label], fn_name)
        ):
            return f"{src_label}::{fn_name}"
    return None


findings["boundary_test_covering"] = {
    limit_kind: boundary_test_covering(limit_kind, const)
    for limit_kind, const in (
        ("decode_apply_cpu_ms", "DECODE_APPLY_CPU_MS_MAX"),
        ("decode_apply_wall_ms", "DECODE_APPLY_WALL_MS_MAX"),
        ("isolated_apply_memory_bytes", "ISOLATED_APPLY_MEMORY_BYTES_MAX"),
        ("open_documents", "OPEN_DOCUMENTS_PER_CONNECTION_MAX"),
        ("user_connections", "CONNECTIONS_PER_USER_MAX"),
        ("document_connections", "CONNECTIONS_PER_DOCUMENT_MAX"),
        ("workspace_connections", "CONNECTIONS_PER_WORKSPACE_MAX"),
        ("frame_rate", "FRAMES_PER_CONNECTION_PER_SECOND"),
        ("update_rate", "UPDATES_PER_CONNECTION_PER_SECOND"),
        ("slow_consumer_queue_frames", "SLOW_CONSUMER_QUEUE_FRAMES_MAX"),
        ("slow_consumer_queue_bytes", "SLOW_CONSUMER_QUEUE_BYTES_MAX"),
        ("scan_budget", "AUTHORIZED_SCAN_ROWS_MAX"),
        ("import_archive_bytes", "IMPORT_ARCHIVE_BYTES_MAX"),
        ("import_expanded_bytes", "IMPORT_EXPANDED_BYTES_MAX"),
        ("import_entry_count", "IMPORT_ENTRY_COUNT_MAX"),
        ("import_compression_ratio", "IMPORT_COMPRESSION_RATIO_MAX"),
        ("websocket_frame_bytes", "WEBSOCKET_FRAME_BYTES_MAX"),
        ("presence_payload_bytes", "PRESENCE_PAYLOAD_BYTES_MAX"),
        ("presence_ttl_seconds", "PRESENCE_TTL_SECONDS_MAX"),
        ("semantic_patch_bytes", "SEMANTIC_PATCH_JSON_BYTES_MAX"),
        ("page_size", None),
        # The two bootstrap ceilings were absent from this map, so their `.get()` below always
        # returned None and their case could never pass no matter what test was written -- the
        # same shape as a hardcoded "failed", just one indirection further away.
        ("bootstrap_decoded_bytes", "BOOTSTRAP_DECODED_BYTES_MAX"),
        ("bootstrap_response_bytes", "BOOTSTRAP_RESPONSE_BYTES_MAX"),
    )
}

findings["bootstrap_rs_mentions_limit_exceeded"] = "limit_exceeded" in bootstrap_text
findings["bootstrap_rs_mentions_bootstrap_decoded_bytes"] = "bootstrap_decoded_bytes" in bootstrap_text
findings["bootstrap_rs_mentions_bootstrap_response_bytes"] = "bootstrap_response_bytes" in bootstrap_text

for const in (
    "OPEN_DOCUMENTS_PER_CONNECTION_MAX", "CONNECTIONS_PER_USER_MAX", "CONNECTIONS_PER_DOCUMENT_MAX",
    "CONNECTIONS_PER_WORKSPACE_MAX", "FRAMES_PER_CONNECTION_PER_SECOND", "UPDATES_PER_CONNECTION_PER_SECOND",
    "SLOW_CONSUMER_QUEUE_FRAMES_MAX", "SLOW_CONSUMER_QUEUE_BYTES_MAX", "AUTHORIZED_SCAN_ROWS_MAX",
    "IMPORT_ARCHIVE_BYTES_MAX", "IMPORT_EXPANDED_BYTES_MAX", "IMPORT_ENTRY_COUNT_MAX", "IMPORT_COMPRESSION_RATIO_MAX",
):
    findings[f"{const.lower()}_referenced_outside_limits_rs"] = (
        count(re.escape(const), session_text) + count(re.escape(const), registry_text)
        + count(re.escape(const), command_text) + count(re.escape(const), bootstrap_text)
        + count(re.escape(const), query_text)
    )

# `decode_apply_cpu_ms_max`/`decode_apply_wall_ms_max`/`isolated_apply_memory_bytes_max`: the real
# enforcement path is apps/api/src/flow/collab/write.rs's `collab_core::isolation::isolated_apply(...)`
# call, which runs inside crates/collab-core/src/isolation/{host,alloc,child_runtime}.rs -- not
# anywhere apps/api/src's own session/registry/command/bootstrap/query modules would ever mention
# it. An apps/api/src-only scan (the loop above) always finds zero references for these three and
# concludes "does not exist", which is wrong: it never looked at the crate that actually enforces
# them. Count references in write.rs (the call site) plus the isolation module's own enforcement
# files (host.rs's wall watchdog, alloc.rs's counting-allocator threshold, child_runtime.rs's
# SIGPROF timer arm, and the worker binary that runs them) -- excluding
# crates/collab-core/src/isolation/limits.rs itself, the single-source declaration, so this stays
# "referenced outside its own declaration" the same way the loop above excludes apps/api's
# limits.rs.
for const in ("DECODE_APPLY_CPU_MS_MAX", "DECODE_APPLY_WALL_MS_MAX", "ISOLATED_APPLY_MEMORY_BYTES_MAX"):
    findings[f"{const.lower()}_referenced_outside_limits_rs"] = (
        count(re.escape(const), write_text)
        + count(re.escape(const), collab_core_isolation_host_text)
        + count(re.escape(const), collab_core_isolation_alloc_text)
        + count(re.escape(const), collab_core_isolation_child_runtime_text)
        + count(re.escape(const), collab_core_isolated_apply_worker_text)
    )

# Every delivery-path budget the contract has frozen is cross-checked against the constant the
# dispatcher actually executes -- "locked" is meaningless if the frozen number and the running
# number disagree, and gate-commands.md says so outright ("已冻结的值必须与 evidence/v0.4/
# flow-events-result.json 里实际执行的 fixture 取同一常量...二者不一致即失败"). Until 2026-08-31
# only three constants were compared here, so a contract freeze that contradicted the code passed
# unnoticed. Contract values are parsed from the row's own value cell, never hand-copied.
delivery_const_re = re.compile(r"^(?:pub )?const (\w+):\s*\w+\s*=\s*([^;]+);", re.M)
dispatcher_const_exprs = {m.group(1): m.group(2).strip() for m in delivery_const_re.finditer(dispatcher_text)}


def resolve_dispatcher_const(name, depth=0):
    """Literal, or a one-level `OTHER_CONST * <int>` (delivery_lease_ttl_ms is defined that way)."""
    expr = dispatcher_const_exprs.get(name)
    if expr is None or depth > 4:
        return None
    if re.fullmatch(r"[\d_]+", expr):
        return int(expr.replace("_", ""))
    m = re.fullmatch(r"(\w+)\s*\*\s*([\d_]+)", expr)
    if m:
        base = resolve_dispatcher_const(m.group(1), depth + 1)
        return None if base is None else base * int(m.group(2).replace("_", ""))
    return None


def contract_row_value_numbers(key):
    """Integers in the row's VALUE cell only -- the 依据 cell is full of unrelated numbers."""
    row_m = re.search(r"^\|\s*`" + re.escape(key) + r"`\s*\|(.*)$", contract, re.M)
    if row_m is None:
        return None
    value_cell = row_m.group(1).split("|")[0]
    return [int(n.replace(",", "").replace("_", "")) for n in re.findall(r"\d[\d,_]*", value_cell)]


# key -> dispatcher constants, in the order they appear in the contract's value cell. A formula
# cell (`min(attempts*30000, 300000)`) carries two numbers and therefore two constants.
DELIVERY_BUDGET_CONSTS = {
    "dispatch_max_lease_reclaims": ["DISPATCH_MAX_LEASE_RECLAIMS"],
    "dispatch_lease_ttl_ms": ["DISPATCH_LEASE_TTL_MS"],
    "dispatch_backoff_ms": ["DISPATCH_BACKOFF_STEP_MS", "DISPATCH_BACKOFF_CAP_MS"],
    "delivery_backoff_ms": ["DELIVERY_BACKOFF_STEP_MS", "DELIVERY_BACKOFF_CAP_MS"],
    "webhook_request_timeout_ms": ["WEBHOOK_REQUEST_TIMEOUT_MS"],
    "delivery_lease_ttl_ms": ["DELIVERY_LEASE_TTL_MS"],
    "content_delivery_debounce_ms": ["CONTENT_DELIVERY_DEBOUNCE_MS"],
    "coalesced_source_events_max": ["COALESCED_SOURCE_EVENTS_MAX"],
    "changed_block_ids_per_delivery_max": ["CHANGED_BLOCK_IDS_PER_DELIVERY_MAX"],
    "delivery_retention_days": ["DELIVERY_RETENTION_DAYS"],
    "dispatch_no_subscribers_retention_hours": ["DISPATCH_NO_SUBSCRIBERS_RETENTION_HOURS"],
    "dispatch_expanded_retention_days": ["DISPATCH_EXPANDED_RETENTION_DAYS"],
    "dispatch_failed_retention_days": ["DISPATCH_FAILED_RETENTION_DAYS"],
}
# Frozen budgets with no single module constant to compare against: `dispatch_max_attempts` is a
# per-row `event_dispatch.max_attempts` column the caller supplies (the migration deliberately
# gives it no DEFAULT), and `dispatch_head_wait_backoff_ms` is realized structurally -- a non-head
# work item is simply not selected. Recorded as `matches: null` rather than omitted, so they are
# visibly unverified here instead of silently absent; their behaviour is covered by
# verify-flow-events-v0.4.sh's fixtures.
DELIVERY_BUDGETS_WITHOUT_CONSTANT = {
    "dispatch_max_attempts": "per-row event_dispatch.max_attempts supplied by the caller (no module constant)",
    "dispatch_head_wait_backoff_ms": "realized structurally: a non-head work item is not selected (no module constant)",
}
delivery_cross_check = {}
for key, const_names in DELIVERY_BUDGET_CONSTS.items():
    contract_values = contract_row_value_numbers(key)
    source_values = [resolve_dispatcher_const(n) for n in const_names]
    entry = {
        "contract": contract_values[0] if contract_values and len(const_names) == 1 else contract_values,
        "source_const": const_names[0] if len(const_names) == 1 else const_names,
        "source_value": source_values[0] if len(const_names) == 1 else source_values,
    }
    if contract_values is None:
        entry["matches"] = False
        entry["note"] = "no such row in contracts/limits-v1.md"
    elif len(contract_values) != len(const_names):
        entry["matches"] = False
        entry["note"] = (
            f"contract value cell carries {len(contract_values)} number(s), "
            f"{len(const_names)} constant(s) mapped"
        )
    else:
        entry["matches"] = contract_values == source_values
    delivery_cross_check[key] = entry
for key, why in DELIVERY_BUDGETS_WITHOUT_CONSTANT.items():
    contract_values = contract_row_value_numbers(key)
    delivery_cross_check[key] = {
        "contract": contract_values[0] if contract_values else None,
        "source_const": None,
        "source_value": None,
        "matches": None,
        "note": why,
    }
mig_m = re.search(r"CREATE TABLE IF NOT EXISTS event_deliveries.*?\n\);", migration_text, re.S)
delivery_max_attempts_default = None
if mig_m:
    dm = re.search(r"max_attempts\s+INTEGER\s+NOT NULL\s+DEFAULT\s+(\d+)", mig_m.group(0))
    if dm:
        delivery_max_attempts_default = int(dm.group(1))
delivery_max_attempts_contract = contract_row_value_numbers("delivery_max_attempts")
delivery_cross_check["delivery_max_attempts"] = {
    "contract": delivery_max_attempts_contract[0] if delivery_max_attempts_contract else None,
    "source": "migrations/0054_flow_data_layer.sql event_deliveries.max_attempts DEFAULT",
    "source_const": "event_deliveries.max_attempts DEFAULT",
    "source_value": delivery_max_attempts_default,
}
delivery_cross_check["delivery_max_attempts"]["matches"] = (
    delivery_cross_check["delivery_max_attempts"]["source_value"]
    == delivery_cross_check["delivery_max_attempts"]["contract"]
)

# Delivery-path `status: unset` rows use a 3-column shape (Key | value |
# 执行与依据) with no `limit_kind` column at all, so `row_re` above (which
# requires a trailing backtick-quoted limit_kind cell) never matches them.
# Capture them with a dedicated 2-column-prefix regex instead -- this is
# the only way `dispatch_numeric_budgets_locked` can see that most
# delivery-path budgets are still unset.
unset_status_re = re.compile(r"^\|\s*`([a-z0-9_]+)`\s*\|\s*`status: unset`", re.M)
all_unset_status_keys = sorted(set(unset_status_re.findall(contract)))
# Population of this gate's freeze floor: every `status: unset` budget row in the contract. What
# leaves this set has to be justified key by key below; nothing leaves it by pattern or by table.
v08_gate_text = read(v08_gate_yaml)
v05_gate_text = read(v05_gate_yaml)


def _contract_row(key):
    row_m = re.search(r"^\|\s*`" + re.escape(key) + r"`\s*\|.*$", contract, re.M)
    if row_m is None:
        raise ValueError(f"budget {key!r} has no row in contracts/limits-v1.md")
    return row_m.group(0)


def _contract_yaml_block(key):
    block_m = re.search(r"^" + re.escape(key) + r":\n((?:[ \t]+\S.*\n)+)", contract, re.M)
    return block_m.group(1) if block_m else ""


# Deferral 1: the two retention budgets limits-v1.md hands to the v0.8 implementer, one of which
# (`replay_max_window_days`) bounds an admin replay surface with no code in v0.4 at all. Re-derived
# from the contract every run; any failure here is a hard error, because an exclusion this script
# cannot justify from the contract is exactly what the allowlist exists to prevent.
if not re.search(r"^\s*" + re.escape(V08_PAIRED_BUDGET_GATE) + r"\s*:", v08_gate_text, re.M):
    raise ValueError(
        f"v0.8 gate does not declare the paired gate {V08_PAIRED_BUDGET_GATE!r}; without it the v0.4 "
        "exclusion of the deferred retention budgets is a permanent amnesty, not a deferral"
    )
deferred_dispatch_budget_exemptions = []
for key in sorted(V08_DEFERRED_DISPATCH_BUDGET_KEYS):
    row_text = _contract_row(key)
    if "`status: unset`" not in row_text:
        raise ValueError(
            f"deferred dispatch budget {key!r} is no longer `status: unset` in contracts/limits-v1.md; "
            "a frozen budget must leave V08_DEFERRED_DISPATCH_BUDGET_KEYS and be verified, not stay excused"
        )
    if V08_SET_BY_MARKER not in row_text:
        raise ValueError(
            f"deferred dispatch budget {key!r} does not declare {V08_SET_BY_MARKER!r} in its contract row; "
            "only budgets the contract itself assigns to the v0.8 implementer may be deferred"
        )
    deferred_dispatch_budget_exemptions.append({
        "key": key,
        "status": VERSION_BOUNDARY_REASON_CODE,
        "reason_code": VERSION_BOUNDARY_REASON_CODE,
        "surface_version": "v0.8",
        "contract_set_by": "v0.8 实现者",
        "contract_evidence": "contracts/limits-v1.md row declares " + V08_SET_BY_MARKER,
        "paired_gate": f"gates/v0.8-gate.yaml::{V08_PAIRED_BUDGET_GATE}",
    })

# Deferral 2 (pre-existing, now justified the same way): the v0.5 authorization surface's own unset
# budget, which gate-commands.md assigns to `authz_numeric_budgets_locked`, not to this gate.
if not re.search(r"^\s*" + re.escape(V05_PAIRED_BUDGET_GATE) + r"\s*:", v05_gate_text, re.M):
    raise ValueError(
        f"v0.5 gate does not declare the paired gate {V05_PAIRED_BUDGET_GATE!r}; without it the v0.4 "
        "exclusion of the v0.5 authorization budgets is a permanent amnesty, not a deferral"
    )
for key in sorted(V05_AUTHZ_SURFACE_KEYS & set(all_unset_status_keys)):
    if V05_SET_BY_MARKER not in _contract_yaml_block(key):
        raise ValueError(
            f"v0.5 authorization budget {key!r} does not declare {V05_SET_BY_MARKER!r} in its structured "
            "block; only budgets the contract itself assigns to the v0.5 implementer may be deferred"
        )
    deferred_dispatch_budget_exemptions.append({
        "key": key,
        "status": "not_applicable_until_v0_5",
        "reason_code": "not_applicable_until_v0_5",
        "surface_version": "v0.5",
        "contract_set_by": "v0.5 授权面实现者",
        "contract_evidence": "contracts/limits-v1.md structured block declares " + V05_SET_BY_MARKER,
        "paired_gate": f"gates/v0.5-gate.yaml::{V05_PAIRED_BUDGET_GATE}",
    })

exempt_budget_keys = {e["key"] for e in deferred_dispatch_budget_exemptions}
non_allowlisted_budget_exemptions = sorted(
    exempt_budget_keys - (V08_DEFERRED_DISPATCH_BUDGET_KEYS | V05_AUTHZ_SURFACE_KEYS)
)
if non_allowlisted_budget_exemptions:
    raise ValueError(
        f"dispatch budget exemption allowlist violation: non_allowlisted={non_allowlisted_budget_exemptions}"
    )

# Everything the contract still leaves `status: unset` and nobody deferred is required NOW. This is
# where `subscribers_per_workspace_max` re-enters: `set_by: v0.4 dispatcher 实现者`, named in
# gate-commands.md's closed list for this gate, so it can only be closed by freezing it.
unset_delivery_keys = sorted(set(all_unset_status_keys) - exempt_budget_keys)

# Accounting over the "Delivery path budgets" section, so the evidence can state
# total = frozen + exempt rather than only listing what is still missing.
delivery_section_m = re.search(r"^### Delivery path budgets\s*$(.*?)^### ", contract, re.M | re.S)
if delivery_section_m is None:
    raise ValueError("contract has no '### Delivery path budgets' section to account for")
dispatch_budget_keys = []
for m in re.finditer(r"^\|\s*`([a-z0-9_]+)`\s*\|", delivery_section_m.group(1), re.M):
    if m.group(1) not in dispatch_budget_keys:
        dispatch_budget_keys.append(m.group(1))
for e in deferred_dispatch_budget_exemptions:
    if e["surface_version"] == "v0.8" and e["key"] not in dispatch_budget_keys:
        raise ValueError(
            f"deferred dispatch budget {e['key']!r} is not in the 'Delivery path budgets' section"
        )
section_frozen = sorted(set(dispatch_budget_keys) - set(all_unset_status_keys))
section_exempt = sorted(set(dispatch_budget_keys) & exempt_budget_keys)
section_required = sorted(set(dispatch_budget_keys) & set(unset_delivery_keys))
dispatch_budget_accounting = {
    "delivery_section_total": len(dispatch_budget_keys),
    "delivery_section_frozen": len(section_frozen),
    "delivery_section_exempt": len(section_exempt),
    "delivery_section_still_unset": len(section_required),
    "delivery_section_frozen_keys": section_frozen,
    "delivery_section_exempt_keys": section_exempt,
    "unset_budget_rows_total": len(all_unset_status_keys),
    "unset_budget_rows_exempt": len(exempt_budget_keys),
    "unset_budget_rows_required": len(unset_delivery_keys),
    "unset_budget_rows_exempt_keys": sorted(exempt_budget_keys),
    "unset_budget_rows_required_keys": unset_delivery_keys,
}
if len(section_frozen) + len(section_exempt) + len(section_required) != len(dispatch_budget_keys):
    raise ValueError("delivery-section budget accounting does not close")
if len(exempt_budget_keys) + len(unset_delivery_keys) != len(all_unset_status_keys):
    raise ValueError("unset budget accounting does not close")
dispatch_budget_accounting["identity"] = (
    f"Delivery path budgets: {len(dispatch_budget_keys)} total = {len(section_frozen)} frozen + "
    f"{len(section_exempt)} deferred + {len(section_required)} still unset; "
    f"contract-wide `status: unset` budget rows: {len(all_unset_status_keys)} = "
    f"{len(exempt_budget_keys)} deferred + {len(unset_delivery_keys)} required by v0.4"
)

fe_field_re = re.compile(r"readonly (\w+):")
fe_iface_m = re.search(r"interface FlowLimitsV1 \{(.*?)\n\}", frontend_types_text, re.S)
fe_fields = fe_field_re.findall(fe_iface_m.group(1)) if fe_iface_m else []


def camel_to_snake(name: str) -> str:
    return re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", name).lower()


fe_fields_snake = sorted({camel_to_snake(x) for x in fe_fields})
server_fields_snake = sorted({k for k in wire_fields if k != "version"})
missing_in_frontend = sorted(set(server_fields_snake) - set(fe_fields_snake))
extra_in_frontend = sorted(set(fe_fields_snake) - set(server_fields_snake))

default_limits_m = re.search(
    r"export\s+const\s+DEFAULT_FLOW_LIMITS\s*:\s*FlowLimitsV1\s*=\s*\{(.*?)\n\};",
    frontend_limits_text,
    re.S,
)
frontend_default_wire_fields = {}
frontend_default_parse_errors = []
frontend_version_m = re.search(
    r"export\s+const\s+FLOW_LIMITS_VERSION\s*=\s*'([^']+)'\s*;",
    frontend_limits_text,
)
frontend_default_version = frontend_version_m.group(1) if frontend_version_m else None
if frontend_default_version is None:
    frontend_default_parse_errors.append("FLOW_LIMITS_VERSION string literal was not found")
if default_limits_m:
    entry_re = re.compile(r"^\s*([A-Za-z][A-Za-z0-9]*)\s*:\s*([0-9][0-9_]*)\s*,?\s*$", re.M)
    entries = entry_re.findall(default_limits_m.group(1))
    for field, raw_value in entries:
        wire_name = camel_to_snake(field)
        if wire_name in frontend_default_wire_fields:
            frontend_default_parse_errors.append(f"duplicate DEFAULT_FLOW_LIMITS field {field}")
            continue
        frontend_default_wire_fields[wire_name] = int(raw_value.replace("_", ""))
else:
    frontend_default_parse_errors.append("DEFAULT_FLOW_LIMITS literal object was not found")

frontend_default_missing = sorted(set(server_fields_snake) - set(frontend_default_wire_fields))
frontend_default_extra = sorted(set(frontend_default_wire_fields) - set(server_fields_snake))
if len(frontend_default_wire_fields) != 35:
    frontend_default_parse_errors.append(
        f"DEFAULT_FLOW_LIMITS parsed {len(frontend_default_wire_fields)} numeric fields, expected 35"
    )

print(json.dumps({
    "expected_limit_kinds": expected_limit_kinds,
    "fixed_limit_kind_count": fixed_limit_kind_count,
    "evaluated_limit_kind_count": evaluated_limit_kind_count,
    "version_boundary_exemptions": [version_boundary_exemptions[k] for k in sorted(version_boundary_exemptions)],
    "v0_4_rows": v0_4_rows,
    "deferred_or_unset_rows": deferred_or_unset_rows,
    "row_cross_check": row_cross_check,
    "row_violations": row_violations,
    "wire_fields": wire_fields,
    "wire_cross_check": wire_cross_check,
    "wire_violations": wire_violations,
    "findings": findings,
    "delivery_cross_check": delivery_cross_check,
    "unset_delivery_keys": unset_delivery_keys,
    "deferred_dispatch_budget_exemptions": deferred_dispatch_budget_exemptions,
    "dispatch_budget_accounting": dispatch_budget_accounting,
    "frontend_bootstrap_parity": {
        "server_field_count": len(server_fields_snake),
        "frontend_field_count": len(fe_fields_snake),
        "missing_in_frontend": missing_in_frontend,
        "extra_in_frontend": extra_in_frontend,
        "default_wire_fields": frontend_default_wire_fields,
        "default_version": frontend_default_version,
        "default_missing": frontend_default_missing,
        "default_extra": frontend_default_extra,
        "default_parse_errors": frontend_default_parse_errors,
    },
}))
PY
then
  echo "FAIL: static analysis (contract parse + source cross-check) crashed; see $EVIDENCE_ROOT/logs/limits.static.err.log" >&2
  cat "$EVIDENCE_ROOT/logs/limits.static.err.log" >&2
  exit 2
fi
if ! jq -e . >/dev/null 2>&1 "$STATIC_JSON_FILE"; then
  echo "FAIL: static analysis did not produce valid JSON; see $STATIC_JSON_FILE" >&2
  exit 2
fi

EXPECTED_COUNT="$(jq '.expected_limit_kinds | length' "$STATIC_JSON_FILE")"
if [[ "$EXPECTED_COUNT" -eq 0 ]]; then
  echo "FAIL: parsed zero v0.4 limit_kind rows out of contracts/limits-v1.md -- refusing to write a vacuous result (contract format may have changed; row regex needs updating)" >&2
  exit 2
fi
echo "=== static check: contracts/limits-v1.md limit_kind rows vs apps/api/src/flow/collab/limits.rs constants ===" >&2
echo "  v0.4 limit_kind accounting: $(jq -r '.fixed_limit_kind_count' "$STATIC_JSON_FILE") = $EXPECTED_COUNT evaluated + $(jq -r '.version_boundary_exemptions | length' "$STATIC_JSON_FILE") not_applicable_until_v0_8" >&2
echo "  version-boundary exemptions: $(jq -r '[.version_boundary_exemptions[].limit_kind] | join(", ")' "$STATIC_JSON_FILE")" >&2
echo "  dispatch budget accounting: $(jq -r '.dispatch_budget_accounting.identity' "$STATIC_JSON_FILE")" >&2
echo "  deferred dispatch budgets: $(jq -r '[.deferred_dispatch_budget_exemptions[] | "\(.key) (\(.reason_code), surface \(.surface_version))"] | join(", ")' "$STATIC_JSON_FILE")" >&2
echo "  row cross-check violations: $(jq '.row_violations | length' "$STATIC_JSON_FILE")" >&2
echo "  wire-schema cross-check violations: $(jq '.wire_violations | length' "$STATIC_JSON_FILE")" >&2
jq -r '.row_violations[] | "    VIOLATION(row): " + .' "$STATIC_JSON_FILE" >&2
jq -r '.wire_violations[] | "    VIOLATION(wire): " + .' "$STATIC_JSON_FILE" >&2

# ---- 2. dynamic cargo tests ----
LOG_DIR="$EVIDENCE_ROOT/logs"
run_group() {
  local name="$1" pkg="$2" filter="$3"
  local extra_args="${4:-}"
  local log="$LOG_DIR/limits.dyn.${name}.log"
  if [[ $SKIP_CARGO_TEST -eq 1 ]]; then
    echo "(skipped by --skip-cargo-test)" > "$log"
    return
  fi
  echo "  running: cargo test -p $pkg $extra_args '$filter'" >&2
  set +e
  # shellcheck disable=SC2086
  ( cd "$REPO_ROOT" && cargo test -p "$pkg" $extra_args "$filter" -- --test-threads=4 ) > "$log" 2>&1
  set -e
}

echo "=== dynamic: cargo test groups ===" >&2
if [[ $SKIP_CARGO_TEST -ne 1 ]]; then
  echo "  building collab-isolated-apply-worker for real isolation boundary tests" >&2
  ( cd "$REPO_ROOT" && cargo build -q -p collab-core --bin collab-isolated-apply-worker ) || {
    echo "FAIL: collab-isolated-apply-worker failed to build" >&2
    exit 2
  }
fi
# Broadened from the old "presence_ceiling" filter (which only matched the two presence-ceiling
# tests) to the whole `flow::collab::registry::tests::` module, matching the convention already
# used for `snapshot_pure` below. `registry.rs`'s `tests` module also holds the real per-user/
# per-document/per-workspace connection-ceiling tests and the slow-consumer queue-frame-ceiling
# test that `boundary_test_covering()` above can now name-match -- without running them here they
# would never appear in `dtest()`'s pass/fail lookup and every case that requires one would stay
# `failed` no matter how well the static name-matching works.
run_group registry_tests api "flow::collab::registry::tests::" "--lib"
run_group snapshot_pure api "flow::collab::snapshot::tests::" "--lib"
run_group flow_command_tests api "flow::command::typed_error_mapping_tests::" "--lib"
run_group collab_core_limits collab-core "limits::tests::"
# The isolation boundary's own enforcement/test sites (crates/collab-core/src/isolation/{host,
# alloc,child_runtime,limits}.rs -- see TEST_SOURCE_TEXT_PARTS above) live under the `isolation::`
# module path, which the "limits::tests::" filter above only reaches for isolation/limits.rs
# itself (its full path, `isolation::limits::tests::...`, happens to contain that substring) --
# host.rs/alloc.rs/child_runtime.rs's own `isolation::{host,alloc,child_runtime}::tests::` paths do
# not. Without this, every one of those files' tests would be statically visible (in the scan
# list) but never actually run, so dtest() could never resolve their ok/FAILED status no matter
# how well boundary_test_covering() name-matches them. host.rs's end-to-end tests spawn the real
# `collab-isolated-apply-worker` subprocess, so it must be built first
# (`cargo build -p collab-core --bin collab-isolated-apply-worker`); a missing binary makes those
# specific tests skip themselves (see host.rs's own `isolated_apply_with_real_worker` early-return)
# rather than fail, which is a pre-existing property of those tests, not something this run_group
# changes.
run_group collab_core_isolation collab-core "isolation::"
# crates/collab-core/src/bin/isolated_apply_worker.rs is a separate binary target (its own crate
# root, per Cargo.toml's `[[bin]]`), so its `#[cfg(test)] mod tests` is NOT nested under
# `isolation::` like the lib's modules are -- its test paths are bare `tests::run_metered_...`.
# The "isolation::" filter above cannot reach it; this dedicated group is required for that file's
# two tests (already in the static scan list) to get any dynamic ok/FAILED status at all.
run_group collab_core_isolated_worker_bin collab-core "tests::run_metered"
run_group effective_limits_wire api "flow::collab::limits::tests::effective_limits_serializes_every_frozen_field_non_null" "--lib"
# Pure-logic (non-DB) unit tests for the WS rate limiter (frame_rate / update_rate token-bucket
# boundary + 3-consecutive-window close), colocated in session.rs's own `tests` module (distinct
# from `database_tests`, which stays DB-backed and is exercised separately below).
run_group session_tests api "flow::collab::session::tests::" "--lib"
run_group isolation_wire api "flow::collab::write::isolation_rejection_tests::" "--lib"
# scan_budget's enforcement (`check_scan_budget`) and its boundary tests both belong in
# flow/query.rs, which the static scan reads but no filter above ran -- so dtest() could never
# resolve those tests' status and the case failed no matter how well it was covered.
run_group query_tests api "flow::query::tests::" "--lib"

DB_SKIPPED=0
DB_LOGS=(
  "$LOG_DIR/limits.dyn.update_bytes_e2e.log"
  "$LOG_DIR/limits.dyn.bootstrap_wire.log"
  "$LOG_DIR/limits.dyn.rest_call_direction.log"
  "$LOG_DIR/limits.dyn.ws_structural_call_direction.log"
  "$LOG_DIR/limits.dyn.page_size_call_direction.log"
  "$LOG_DIR/limits.dyn.session_observable_kinds.log"
  "$LOG_DIR/limits.dyn.session_user_connections_kind.log"
  "$LOG_DIR/limits.dyn.session_wire_boundaries.log"
)
if [[ $SKIP_CARGO_TEST -eq 1 ]]; then
  for f in "${DB_LOGS[@]}"; do
    echo "(skipped by --skip-cargo-test)" > "$f"
  done
else
  echo "  running: cargo test -p api routes::collab::...full_session_hello... (DB-backed)" >&2
  set +e
  ( cd "$REPO_ROOT" && cargo test -p api --lib "routes::collab::collab_database_tests::full_session_hello_open_snapshot_update_accepted_and_two_rejections" -- --test-threads=1 ) > "$LOG_DIR/limits.dyn.update_bytes_e2e.log" 2>&1
  set -e
  echo "  running: cargo test -p api routes::flow::...bootstrap_endpoint... (DB-backed)" >&2
  set +e
  ( cd "$REPO_ROOT" && cargo test -p api --lib "routes::flow::flow_database_tests::bootstrap_endpoint_returns_the_full_shape_for_a_user_and_rejects_a_bot" -- --test-threads=1 ) > "$LOG_DIR/limits.dyn.bootstrap_wire.log" 2>&1
  set -e
  # Call-direction proofs for the structural limits (tree_depth, container_count,
  # document_block_count, text_block_chars, document_text_chars, semantic_patch_operations):
  # real e2e tests that go through the actual REST command handler
  # (routes/flow.rs::flow_database_tests) and the actual WebSocket write path
  # (flow/collab/write.rs::database_tests), not just collab-core's crate-internal unit tests.
  # This is what lets boundary_cases below tell "wired AND proven by a caller-facing endpoint"
  # apart from "wired but only ever unit-tested in isolation".
  echo "  running: cargo test -p api routes::flow::...commands_endpoint_...tree_depth/batch_count... (DB-backed)" >&2
  set +e
  ( cd "$REPO_ROOT" && cargo test -p api --lib "routes::flow::flow_database_tests::commands_endpoint_" -- --test-threads=1 ) > "$LOG_DIR/limits.dyn.rest_call_direction.log" 2>&1
  set -e
  echo "  running: cargo test -p api flow::collab::write::database_tests::ws_structural_limit_... (DB-backed)" >&2
  set +e
  ( cd "$REPO_ROOT" && cargo test -p api --lib "flow::collab::write::database_tests::ws_structural_limit_" -- --test-threads=1 ) > "$LOG_DIR/limits.dyn.ws_structural_call_direction.log" 2>&1
  set -e
  # `page_size`'s exact/+1 boundary test lives in routes/flow.rs::flow_database_tests (a real
  # REST list-endpoint call, not a query.rs-only unit test) -- see the ROUTES_FLOW_RS static scan
  # entry above. Neither of the two existing routes::flow::flow_database_tests filters (
  # bootstrap_wire's exact-name match, rest_call_direction's "commands_endpoint_" prefix) reaches
  # this test's name, so it needs its own dedicated run here.
  echo "  running: cargo test -p api routes::flow::...list_objects_endpoint_...page_size... (DB-backed)" >&2
  set +e
  ( cd "$REPO_ROOT" && cargo test -p api --lib "routes::flow::flow_database_tests::list_objects_endpoint_rejects_page_size_over_page_limit_max_and_accepts_exact_boundary" -- --test-threads=1 ) > "$LOG_DIR/limits.dyn.page_size_call_direction.log" 2>&1
  echo "  running: cargo test -p api --lib session DB wire kind producers" >&2
  set +e
  ( cd "$REPO_ROOT" && cargo test -p api --lib "ceiling_is_observable_as_limit_exceeded" -- --test-threads=1 ) > "$LOG_DIR/limits.dyn.session_observable_kinds.log" 2>&1
  set -e
  set +e
  ( cd "$REPO_ROOT" && cargo test -p api --lib "user_connections_ceiling_refuses_the_seventeenth_session_with_the_frozen_limit_kind" -- --test-threads=1 ) > "$LOG_DIR/limits.dyn.session_user_connections_kind.log" 2>&1
  set -e
  set +e
  ( cd "$REPO_ROOT" && cargo test -p api --lib "flow::collab::session::database_tests::" -- --test-threads=1 ) > "$LOG_DIR/limits.dyn.session_wire_boundaries.log" 2>&1
  set -e
  if grep -q "skipped: OPENPR_TEST_DATABASE_URL is not set" "${DB_LOGS[@]}" 2>/dev/null; then
    DB_SKIPPED=1
  fi
fi

if [[ $DB_SKIPPED -eq 1 ]]; then
  echo "FAIL: a DB-backed dynamic test was silently skipped (OPENPR_TEST_DATABASE_URL is not set)." >&2
  echo "Fix: export OPENPR_TEST_DATABASE_URL and re-run. A 'passed' result from a database-less skip" >&2
  echo "     run is a false green, not a partial one -- refusing to write evidence for it." >&2
  exit 2
fi

DYNAMIC_JSON_FILE="$LOG_DIR/limits.dynamic.json"
python3 - "$LOG_DIR" "$DB_SKIPPED" > "$DYNAMIC_JSON_FILE" <<'PY'
import glob
import json
import os
import re
import sys

log_dir, db_skipped = sys.argv[1], sys.argv[2] == "1"
groups = {}
for path in glob.glob(os.path.join(log_dir, "limits.dyn.*.log")):
    name = os.path.basename(path)[len("limits.dyn."):-len(".log")]
    text = open(path, encoding="utf-8", errors="replace").read()
    tests = dict(re.findall(r"^test (?:\S+::)*?(\w+) \.\.\. (ok|FAILED)$", text, re.M))
    groups[name] = {"log": path, "tests": tests}
print(json.dumps({"groups": groups, "db_skipped": db_skipped}))
PY

echo "=== dynamic test results ===" >&2
jq -r '.groups | to_entries[] | "  \(.key): \(.value.tests | to_entries | map(select(.value!="ok")) | length) not-ok of \(.value.tests | length)"' "$DYNAMIC_JSON_FILE" >&2

# ---- 3. assemble (reads sibling collab-architecture / flow-events results if source_head-matched) ----
OUT_PATH="$EVIDENCE_ROOT/limits-result.json"
OUT_TMP="$OUT_PATH.tmp"
COLLAB_ARCH_PATH="$EVIDENCE_ROOT/collab-architecture-result.json"
EVENTS_PATH="$EVIDENCE_ROOT/flow-events-result.json"
[[ -f "$COLLAB_ARCH_PATH" ]] || COLLAB_ARCH_PATH="-"
[[ -f "$EVENTS_PATH" ]] || EVENTS_PATH="-"

FINAL_JSON="$(python3 - "$STATIC_JSON_FILE" "$DYNAMIC_JSON_FILE" "$COLLAB_ARCH_PATH" "$EVENTS_PATH" \
    "$SOURCE_HEAD" "$GENERATED_AT" "$CONTRACT_SHA256" "$OUT_TMP" <<'PY'
import hashlib
import json
import re
import sys

static_path, dynamic_path, collab_arch_path, events_path, source_head, generated_at, contract_sha256, out_path = sys.argv[1:9]

static = json.load(open(static_path, encoding="utf-8"))
dynamic = json.load(open(dynamic_path, encoding="utf-8"))
f = static["findings"]

collab_arch = None
if collab_arch_path != "-":
    try:
        candidate = json.load(open(collab_arch_path, encoding="utf-8"))
        if candidate.get("source_head") == source_head:
            collab_arch = candidate
    except (OSError, json.JSONDecodeError):
        collab_arch = None

events = None
if events_path != "-":
    try:
        candidate = json.load(open(events_path, encoding="utf-8"))
        if candidate.get("source_head") == source_head:
            events = candidate
    except (OSError, json.JSONDecodeError):
        events = None


def dtest(name):
    for group in dynamic.get("groups", {}).values():
        if name in group.get("tests", {}):
            return group["tests"][name]
    return None


def case(key, limit_kind, limit, status, reason, exact=None, plus_one=None, evidence=None):
    return {
        "key": key, "limit_kind": limit_kind, "limit": limit,
        "exact": exact or {"accepted": None, "head_after": None},
        "plus_one": plus_one or {"code": None, "limit_kind": None, "head_unchanged": None, "event_dispatch_zero": None},
        "status": status, "reason": reason, "evidence": evidence or {},
    }


row_by_kind = {r["limit_kind"]: r for r in static["v0_4_rows"]}
boundary_cases = []

# ---- structural limits: tree_depth, container_count, document_block_count, text_block_chars,
# document_text_chars, semantic_patch_operations ----
#
# `status` here is derived from THREE independently-computed pieces of evidence, all re-checked
# every run -- never a fixed verdict:
#   1. `wired`: `collab_core::limits::check_operation`/`check_operation_batch_count` has a real,
#      non-comment call site in apps/api/src/flow/command.rs (CALL_SITES, from the static regex
#      count above -- doc-comment references like `[`check_operation`]` don't match it because
#      they're never followed immediately by `(`).
#   2. `crate_ok`: the crate-internal exact/+1 unit test for this limit_kind
#      (crates/collab-core/src/limits.rs::tests) currently passes.
#   3. `call_direction_ok`: at least one *call-direction* boundary test -- one that goes through
#      an actual caller-facing endpoint (the REST command handler in routes/flow.rs, and/or the
#      WebSocket write path via flow/collab/write.rs's `submit`/`accept_update`) rather than only
#      exercising the limits module directly -- exists AND currently passes for this limit_kind.
#      A passing crate-internal unit test alone is explicitly NOT sufficient for `passed`: that is
#      exactly the gap this whole verifier exists to catch (a check that is correct in isolation
#      but never reachable from any real request).
# `passed` requires all three. Any one of them being false keeps the case `failed`, with the
# reason built from whichever piece(s) actually failed -- so the prose can never describe a state
# ("0 call sites" / "not wired") that contradicts what the case's own evidence just measured.
CALL_SITES = f["collab_core_check_operation_call_sites_in_command_rs"]
STRUCTURAL_LIMIT_CASES = (
    ("tree_depth_max", "tree_depth",
     "create_node_at_exact_depth_is_accepted_one_past_is_rejected",
     "ws_structural_limit_tree_depth_exact_boundary_accepted_plus_one_rejected_zero_side_effects",
     "commands_endpoint_insert_block_rejects_tree_depth_plus_one_and_accepts_exact_boundary"),
    ("container_count_max", "container_count",
     "container_count_and_document_block_count_are_independent_counters",
     "ws_structural_limit_container_count_exact_boundary_accepted_plus_one_rejected_zero_side_effects",
     None),
    ("document_block_count_max", "document_block_count",
     "container_count_and_document_block_count_are_independent_counters",
     "ws_structural_limit_document_block_count_exact_boundary_accepted_plus_one_rejected_zero_side_effects",
     None),
    ("text_block_chars_max", "text_block_chars",
     "text_block_chars_checked_before_document_text_chars",
     "ws_structural_limit_text_block_chars_exact_boundary_accepted_plus_one_rejected_zero_side_effects",
     None),
    ("document_text_chars_max", "document_text_chars",
     "document_text_chars_rejects_even_when_the_target_block_is_small",
     "ws_structural_limit_document_text_chars_exact_boundary_accepted_plus_one_rejected_zero_side_effects",
     None),
    # semantic_patch_operations has no WS-layer equivalent: it bounds a REST-only concept (the
    # operation count of one `update_block` command's `properties` batch), and write.rs's
    # `check_snapshot` backstop -- which only re-derives tree/container/text aggregates from the
    # merged document -- structurally cannot catch it (see the REST test's own doc comment).
    ("semantic_patch_operations_max", "semantic_patch_operations",
     "batch_count_exact_accepted_plus_one_rejected",
     None,
     "commands_endpoint_update_block_rejects_semantic_patch_operations_batch_plus_one_and_accepts_exact_boundary"),
)
for key, limit_kind, crate_test_name, ws_test_name, rest_test_name in STRUCTURAL_LIMIT_CASES:
    r = row_by_kind[limit_kind]
    crate_status = dtest(crate_test_name)
    ws_status = dtest(ws_test_name) if ws_test_name else None
    rest_status = dtest(rest_test_name) if rest_test_name else None

    wired = CALL_SITES > 0
    crate_ok = crate_status == "ok"
    call_direction_results = [s for s in (ws_status, rest_status) if s is not None]
    call_direction_ok = bool(call_direction_results) and all(s == "ok" for s in call_direction_results)
    passed = wired and crate_ok and call_direction_ok

    reason_bits = []
    if wired:
        reason_bits.append(
            f"check_operation/check_operation_batch_count has {CALL_SITES} real call site(s) in "
            "apps/api/src/flow/command.rs::apply_content_command (not a doc comment)"
        )
    else:
        reason_bits.append(
            f"crates/collab-core/src/limits.rs's check_operation/check_operation_batch_count has "
            f"{CALL_SITES} call sites in apps/api/src/flow/command.rs -- not wired into any "
            "REST/MCP/CLI/WS endpoint, so a passing unit test does not prove caller-facing enforcement"
        )
    reason_bits.append(f"crate-internal unit test limits::tests::{crate_test_name}: {crate_status}")
    if ws_test_name:
        reason_bits.append(
            f"WS call-direction test flow::collab::write::database_tests::{ws_test_name}: {ws_status}"
        )
    if rest_test_name:
        reason_bits.append(
            f"REST call-direction test routes::flow::flow_database_tests::{rest_test_name}: {rest_status}"
        )
    if wired and crate_ok and not call_direction_ok:
        reason_bits.append(
            "wired and crate-internal-tested, but no call-direction (REST or WS) boundary test "
            "currently passes for this limit_kind -- a crate-internal unit test alone does not prove "
            "a real request can reach this check"
        )

    boundary_cases.append(case(
        key, limit_kind, r["value"], "passed" if passed else "failed", "; ".join(reason_bits),
        exact={
            "accepted": (
                (ws_status == "ok" if ws_status is not None else None)
                if ws_test_name is not None
                else (rest_status == "ok" if rest_status is not None else None)
            ),
            "head_after": "unchanged (asserted by the call-direction test)" if call_direction_ok
            else "n/a: no passing call-direction test to observe it from",
        },
        plus_one={
            "code": "limit_exceeded" if call_direction_ok else None,
            "limit_kind": limit_kind if call_direction_ok else None,
            "head_unchanged": True if call_direction_ok else None,
            "event_dispatch_zero": True if call_direction_ok else None,
        },
        evidence={
            "call_sites_in_apps_api_command_rs": CALL_SITES,
            "dynamic_test_crate_unit": {
                "crate": "collab-core", "test": f"limits::tests::{crate_test_name}", "status": crate_status,
            },
            "dynamic_test_ws_call_direction": (
                {"test": f"flow::collab::write::database_tests::{ws_test_name}", "status": ws_status}
                if ws_test_name else None
            ),
            "dynamic_test_rest_call_direction": (
                {"test": f"routes::flow::flow_database_tests::{rest_test_name}", "status": rest_status}
                if rest_test_name else None
            ),
        },
    ))

r = row_by_kind["semantic_patch_bytes"]
spb_enforcement_found = f["semantic_patch_bytes_enforcement_found"]
spb_test = f["boundary_test_covering"].get("semantic_patch_bytes")
spb_required_test_name = "semantic_patch_bytes_exact_boundary_is_accepted_and_plus_one_is_rejected_before_writes"
spb_exact_test_found = spb_test is not None and spb_test.rsplit("::", 1)[-1] == spb_required_test_name
spb_test_status = dtest(spb_required_test_name)
spb_passed = spb_enforcement_found and spb_exact_test_found and spb_test_status == "ok"
boundary_cases.append(case(
    "semantic_patch_json_bytes_max", "semantic_patch_bytes", r["value"],
    "passed" if spb_passed else "failed",
    (
        f"semantic_patch_json_bytes_max is referenced outside limits.rs (enforcement_found="
        f"{spb_enforcement_found}); required boundary test {spb_required_test_name} found="
        f"{spb_exact_test_found}, dynamic status={spb_test_status}"
        if spb_enforcement_found
        else "no semantic_patch REST/MCP endpoint or byte-length check exists anywhere in "
        "apps/api/src (grepped for semantic_patch_json_bytes_max/SEMANTIC_PATCH_JSON_BYTES_MAX "
        "outside limits.rs's own wire-report constant: zero hits) -- the feature this ceiling "
        "would guard has not been built"
    ),
    evidence={"semantic_patch_bytes_enforcement_found": spb_enforcement_found,
              "boundary_test_covering": spb_test,
              "required_test": spb_required_test_name,
              "required_test_found": spb_exact_test_found,
              "dynamic_test_status": spb_test_status},
))

r = row_by_kind["update_bytes"]
ub_test = dtest("full_session_hello_open_snapshot_update_accepted_and_two_rejections")
ub_exact_boundary_test_exists = f["update_bytes_exact_boundary_test_exists"]
ub_passed = (
    ub_test == "ok"
    and ub_exact_boundary_test_exists
    and f["write_rs_limit_exceeded_details_has_limit_field"]
    and f["map_write_rejection_reads_details_field"]
)
ub_reason_parts = []
if not ub_exact_boundary_test_exists:
    ub_reason_parts.append(
        "real DB-backed e2e test (routes::collab::collab_database_tests::"
        "full_session_hello_open_snapshot_update_accepted_and_two_rejections) sends a 70,000-byte "
        "update and asserts LimitExceeded -- but no test in write.rs references the exact "
        "65537-byte boundary+1 value, so the contract's exact-boundary requirement is not met "
        "(the current test only proves 'some oversized update is rejected', not the boundary itself)"
    )
if not f["write_rs_limit_exceeded_details_has_limit_field"]:
    ub_reason_parts.append(
        "the WS-layer rejection (write.rs::reject_from_collab_error) sets details={\"limit_kind\":...} "
        "but omits the contract-required `limit` field"
    )
if not f["map_write_rejection_reads_details_field"]:
    ub_reason_parts.append(
        "the REST path (command.rs::map_write_rejection) never reads rejected.details at all -- "
        "apps/api/src/error.rs's ApiError/ApiResponse carry no `details` field whatsoever, so a REST "
        "caller cannot receive limit_kind for this or any limit_exceeded rejection"
    )
if ub_test != "ok":
    ub_reason_parts.append(f"the DB-backed e2e test itself does not pass (status={ub_test})")
if not ub_reason_parts:
    ub_reason_parts.append(
        "exact 65536-accepted/65537-rejected boundary test passes, WS details carry limit_kind and "
        "limit, and REST reads rejected.details"
    )
boundary_cases.append(case(
    "update_bytes_max", "update_bytes", r["value"], "passed" if ub_passed else "failed",
    "; ".join(ub_reason_parts),
    plus_one={"code": "limit_exceeded" if ub_test == "ok" else None,
              "limit_kind": "update_bytes" if ub_test == "ok" else None,
              "head_unchanged": True if ub_test == "ok" else None, "event_dispatch_zero": None},
    evidence={"dynamic_test": {"crate": "api", "test": "routes::collab::collab_database_tests::"
                                "full_session_hello_open_snapshot_update_accepted_and_two_rejections", "status": ub_test},
              "exact_boundary_test_exists": ub_exact_boundary_test_exists,
              "write_rs_details_has_limit_kind": f["write_rs_limit_exceeded_details_has_limit_kind"],
              "write_rs_details_has_limit_field": f["write_rs_limit_exceeded_details_has_limit_field"],
              "map_write_rejection_reads_details_field": f["map_write_rejection_reads_details_field"]},
))

for key, limit_kind, test_name in (
    ("presence_entries_per_connection_max", "presence_entries_per_connection", "per_connection_presence_ceiling_is_enforced"),
    ("presence_entries_per_document_max", "presence_entries_per_document", "per_document_presence_ceiling_is_enforced_and_does_not_evict_others"),
):
    r = row_by_kind[limit_kind]
    test_status = dtest(test_name)
    details_is_none = f["session_rs_presence_limit_rejection_details_is_none"]
    presence_passed = test_status == "ok" and not details_is_none
    presence_reason = (
        "apps/api/src/flow/collab/registry.rs enforces this exactly (real exact-boundary/+1 unit test "
        f"{test_name}: {test_status})"
        + (
            ", and session.rs's rejection carries structured details naming the limit_kind"
            if not details_is_none
            else ", but apps/api/src/flow/collab/session.rs's own match arm "
            "(`Err(PresenceLimit::PerConnection | PresenceLimit::PerDocument) => rejected_frame(..., "
            "RejectedCode::LimitExceeded, false, None)`) sends details=None for BOTH ceilings -- the "
            "wire response carries no limit_kind at all and cannot distinguish the two ceilings"
        )
    )
    boundary_cases.append(case(
        key, limit_kind, r["value"], "passed" if presence_passed else "failed", presence_reason,
        exact={"accepted": test_status == "ok" if test_status else None,
               "head_after": "n/a: presence never touches canonical head"},
        plus_one={"code": "limit_exceeded" if test_status == "ok" else None, "limit_kind": None,
                  "head_unchanged": True if test_status == "ok" else None,
                  "event_dispatch_zero": True if test_status == "ok" else None},
        evidence={"dynamic_test": {"crate": "api", "test": f"flow::collab::registry::tests::{test_name}", "status": test_status},
                  "session_rs_presence_limit_rejection_details_is_none": f["session_rs_presence_limit_rejection_details_is_none"]},
    ))

r = row_by_kind["websocket_frame_bytes"]
wfb_test = f["boundary_test_covering"].get("websocket_frame_bytes")
wfb_passed = wfb_test is not None and dtest(wfb_test.rsplit("::", 1)[-1]) == "ok"
boundary_cases.append(case(
    "websocket_frame_bytes_max", "websocket_frame_bytes", r["value"],
    "passed" if wfb_passed else "failed",
    (
        f"WEBSOCKET_FRAME_BYTES_MAX is checked pre-decode in apps/api/src/flow/collab/session.rs, "
        f"and a boundary test was found: {wfb_test}"
        if wfb_test
        else "WEBSOCKET_FRAME_BYTES_MAX is checked pre-decode in apps/api/src/flow/collab/session.rs, "
        "but no unit or e2e test exercises the exact 131072-accepted/131073-rejected boundary "
        "(scanned every #[test]/#[tokio::test] function name in the caller-facing modules this "
        "script reads: zero match)"
    ),
    evidence={"boundary_test_covering": wfb_test},
))

r = row_by_kind["presence_payload_bytes"]
ppb_call_sites = f["presence_payload_bytes_max_enforcement_call_sites"]
ppb_test = f["boundary_test_covering"].get("presence_payload_bytes")
ppb_passed = ppb_call_sites > 0 and ppb_test is not None and dtest(ppb_test.rsplit("::", 1)[-1]) == "ok"
boundary_cases.append(case(
    "presence_payload_bytes_max", "presence_payload_bytes", r["value"],
    "passed" if ppb_passed else "failed",
    (
        f"PRESENCE_PAYLOAD_BYTES_MAX has {ppb_call_sites} enforcement reference(s) outside its own "
        f"wire-report constant, and a boundary test was found: {ppb_test}"
        if ppb_call_sites > 0
        else "verified absent: PRESENCE_PAYLOAD_BYTES_MAX has "
        f"{ppb_call_sites} references in apps/api/src/flow/collab/session.rs outside its own "
        "wire-report constant in limits.rs -- the presence frame's payload byte length is never "
        "checked against it"
    ),
    evidence={"presence_payload_bytes_max_enforcement_call_sites": ppb_call_sites,
              "boundary_test_covering": ppb_test},
))

r = row_by_kind["presence_ttl_seconds"]
ttl_details_is_none = f["session_rs_presence_ttl_rejection_details_is_none"]
ttl_test = f["boundary_test_covering"].get("presence_ttl_seconds")
ttl_passed = (not ttl_details_is_none) and ttl_test is not None and dtest(ttl_test.rsplit("::", 1)[-1]) == "ok"
boundary_cases.append(case(
    "presence_ttl_seconds_max", "presence_ttl_seconds", r["value"],
    "passed" if ttl_passed else "failed",
    (
        "apps/api/src/flow/collab/session.rs correctly implements 0->invalid_update, "
        ">30->limit_exceeded, omitted->30 default"
        + (f", and a boundary test was found: {ttl_test}" if ttl_test
           else ", but no unit or e2e test exercises this branch (only reachable via a live WS "
           "session; scanned every #[test]/#[tokio::test] function name: zero match)")
        + (
            "; the limit_exceeded rejected_frame call passes details=None (no limit_kind)"
            if ttl_details_is_none else ""
        )
    ),
    evidence={"session_rs_presence_ttl_rejection_details_is_none": ttl_details_is_none,
              "boundary_test_covering": ttl_test},
))

for key, limit_kind, finding_key in (
    ("bootstrap_decoded_bytes_max", "bootstrap_decoded_bytes", "bootstrap_rs_mentions_bootstrap_decoded_bytes"),
    ("bootstrap_response_bytes_max", "bootstrap_response_bytes", "bootstrap_rs_mentions_bootstrap_response_bytes"),
):
    r = row_by_kind[limit_kind]
    mentions = f[finding_key]
    bootstrap_test = f["boundary_test_covering"].get(limit_kind)
    bootstrap_passed = mentions and bootstrap_test is not None and dtest(bootstrap_test.rsplit("::", 1)[-1]) == "ok"
    boundary_cases.append(case(
        key, limit_kind, r["value"], "passed" if bootstrap_passed else "failed",
        (
            f"apps/api/src/flow/collab/bootstrap.rs mentions '{limit_kind}' and constructs a "
            f"limit_exceeded rejection for it, with boundary test {bootstrap_test}"
            if mentions
            else f"verified absent: apps/api/src/flow/collab/bootstrap.rs never mentions '{limit_kind}' or "
            "constructs a limit_exceeded rejection for it -- the only place this number appears is the "
            "wire-report test confirming the JSON *reports* the right number "
            "(routes/flow.rs bootstrap_endpoint_returns_the_full_shape_for_a_user_and_rejects_a_bot), "
            "which is reporting, not enforcement"
        ),
        evidence={finding_key: mentions, "boundary_test_covering": bootstrap_test},
    ))

for key, limit_kind, const in (
    ("decode_apply_cpu_ms_max", "decode_apply_cpu_ms", "DECODE_APPLY_CPU_MS_MAX"),
    ("decode_apply_wall_ms_max", "decode_apply_wall_ms", "DECODE_APPLY_WALL_MS_MAX"),
    ("isolated_apply_memory_bytes_max", "isolated_apply_memory_bytes", "ISOLATED_APPLY_MEMORY_BYTES_MAX"),
):
    r = row_by_kind[limit_kind]
    ref_count = f.get(f"{const.lower()}_referenced_outside_limits_rs", 0)
    iso_test = f["boundary_test_covering"].get(limit_kind)
    iso_passed = ref_count > 0 and iso_test is not None and dtest(iso_test.rsplit("::", 1)[-1]) == "ok"
    if ref_count > 0 and iso_test is not None:
        iso_reason = (
            f"{const} has {ref_count} enforcement call site(s) outside its own single-source "
            "declaration (crates/collab-core/src/isolation/limits.rs), and a boundary test was "
            f"found: {iso_test}"
        )
    elif ref_count > 0:
        # Wired, but not verified at the numeric boundary: apps/api/src/flow/collab/write.rs calls
        # collab_core::isolation::isolated_apply(...), whose real enforcement is
        # crates/collab-core/src/isolation/host.rs's independent wall-clock watchdog, alloc.rs's
        # counting-allocator threshold, and child_runtime.rs's SIGPROF CPU timer -- all real,
        # reachable code, not "does not exist". What is still missing is a #[test]/#[tokio::test]
        # whose name proves the exact/+1 boundary for this specific ceiling; the isolation module's
        # own end-to-end tests (isolation::host::tests::isolated_apply_end_to_end_*) exercise
        # qualitative kills (a pathologically deep chain, an oversized text block via
        # check_snapshot) rather than a numeric CPU-ms/wall-ms/byte boundary, so none of their names
        # match this limit_kind's or this constant's tokens.
        iso_reason = (
            f"wired but untested at the boundary: {const} has {ref_count} enforcement call site(s) "
            "outside its own single-source declaration (crates/collab-core/src/isolation/limits.rs) "
            "-- apps/api/src/flow/collab/write.rs's hydrate_and_apply calls "
            "collab_core::isolation::isolated_apply(...), whose real enforcement lives in "
            "crates/collab-core/src/isolation/{host,alloc,child_runtime}.rs -- but no "
            "#[test]/#[tokio::test] function name found (scanned apps/api's caller-facing modules "
            "plus crates/collab-core/src/isolation/ and its worker binary) proves the exact/+1 "
            f"boundary for {limit_kind}"
        )
    else:
        iso_reason = (
            f"verified absent: {const} has {ref_count} enforcement call sites in "
            "apps/api/src/flow/collab/write.rs or crates/collab-core/src/isolation outside its own "
            "single-source declaration -- the isolated-apply call path itself is missing, not just "
            "untested"
        )
    boundary_cases.append(case(
        key, limit_kind, r["value"], "passed" if iso_passed else "failed", iso_reason,
        evidence={f"{const.lower()}_referenced_outside_limits_rs": ref_count, "boundary_test_covering": iso_test},
    ))

for key, limit_kind, const in (
    ("open_documents_per_connection_max", "open_documents", "OPEN_DOCUMENTS_PER_CONNECTION_MAX"),
    ("connections_per_user_max", "user_connections", "CONNECTIONS_PER_USER_MAX"),
    ("connections_per_document_max", "document_connections", "CONNECTIONS_PER_DOCUMENT_MAX"),
    ("connections_per_workspace_max", "workspace_connections", "CONNECTIONS_PER_WORKSPACE_MAX"),
    ("frames_per_connection_per_second", "frame_rate", "FRAMES_PER_CONNECTION_PER_SECOND"),
    ("updates_per_connection_per_second", "update_rate", "UPDATES_PER_CONNECTION_PER_SECOND"),
    ("slow_consumer_queue_frames_max", "slow_consumer_queue_frames", "SLOW_CONSUMER_QUEUE_FRAMES_MAX"),
    ("slow_consumer_queue_bytes_max", "slow_consumer_queue_bytes", "SLOW_CONSUMER_QUEUE_BYTES_MAX"),
):
    r = row_by_kind[limit_kind]
    ref_count = f.get(f"{const.lower()}_referenced_outside_limits_rs", 0)
    crq_test = f["boundary_test_covering"].get(limit_kind)
    crq_passed = ref_count > 0 and crq_test is not None and dtest(crq_test.rsplit("::", 1)[-1]) == "ok"
    boundary_cases.append(case(
        key, limit_kind, r["value"], "passed" if crq_passed else "failed",
        (
            f"{const} has {ref_count} enforcement call site(s) outside its own wire-report "
            f"declaration, and a boundary test was found: {crq_test}"
            if ref_count > 0
            else f"verified absent: {const} has {ref_count} enforcement call sites in apps/api/src outside "
            "its own wire-report declaration in collab/limits.rs -- no connection/session-count "
            "registry, token-bucket rate limiter, or slow-consumer queue exists. The contract also "
            "requires a 'deterministic virtual clock' for the rate fixtures specifically; none exists "
            "in this repository (grepped for VirtualClock/virtual_clock/FakeClock: zero hits)."
        ),
        evidence={f"{const.lower()}_referenced_outside_limits_rs": ref_count, "boundary_test_covering": crq_test},
    ))

r = row_by_kind["page_size"]
ps_plain_string = f["query_rs_validate_limit_returns_plain_string"]
ps_test = f["boundary_test_covering"].get("page_size")
ps_passed = (
    not ps_plain_string
    and f["rest_apiresponse_struct_has_details_field"]
    and ps_test is not None
    and dtest(ps_test.rsplit("::", 1)[-1]) == "ok"
)
boundary_cases.append(case(
    "page_limit_default/page_limit_max", "page_size", r["value"], "passed" if ps_passed else "failed",
    (
        "apps/api/src/flow/query.rs::validate_limit enforces MAX_LIST_LIMIT=100 for real, but returns "
        "ApiError::BadRequest(format!(\"limit must be at most {MAX_LIST_LIMIT}\")) -- a plain message "
        "string, not a structured details/limit_kind object (apps/api's REST envelope has no `details` "
        "field at all, see rest_apiresponse_struct_has_details_field)"
        if ps_plain_string
        else "apps/api/src/flow/query.rs::validate_limit returns a structured details/limit_kind object"
    )
    + (f"; boundary test found: {ps_test}" if ps_test
       else "; no unit or e2e test exercises the exact 100-accepted/101-rejected boundary (scanned "
       "every #[test]/#[tokio::test] function name in apps/api/src/flow/query.rs and "
       "apps/api/src/routes/flow.rs: zero match)"),
    evidence={"query_rs_validate_limit_returns_plain_string": ps_plain_string,
              "rest_apiresponse_struct_has_details_field": f["rest_apiresponse_struct_has_details_field"],
              "boundary_test_covering": ps_test},
))

for key, limit_kind, const in (
    ("authorized_scan_rows_max", "scan_budget", "AUTHORIZED_SCAN_ROWS_MAX"),
):
    r = row_by_kind[limit_kind]
    ref_count = f.get(f"{const.lower()}_referenced_outside_limits_rs", 0)
    scan_test = f["boundary_test_covering"].get(limit_kind)
    scan_passed = ref_count > 0 and scan_test is not None and dtest(scan_test.rsplit("::", 1)[-1]) == "ok"
    boundary_cases.append(case(
        key, limit_kind, r["value"], "passed" if scan_passed else "failed",
        (
            f"{const} has {ref_count} enforcement call site(s) outside its own wire-report "
            f"declaration, and a boundary test was found: {scan_test}"
            if ref_count > 0
            else f"verified absent: {const} has {ref_count} enforcement call sites outside its own "
            "wire-report declaration -- no import/scan endpoint validates archive/expanded bytes, "
            "entry count, compression ratio, or authorized-scan row budget anywhere in apps/api/src"
        ),
        evidence={f"{const.lower()}_referenced_outside_limits_rs": ref_count, "boundary_test_covering": scan_test},
    ))

if len(boundary_cases) != len(static["expected_limit_kinds"]):
    print(json.dumps({"error": f"boundary_cases ({len(boundary_cases)}) does not cover exactly the "
                                f"v0.4 expected_limit_kinds ({len(static['expected_limit_kinds'])})"}))
    sys.exit(0)

# `isolation` aggregates the three boundary_cases built in the isolation for-loop above
# (decode_apply_cpu_ms_max/decode_apply_wall_ms_max/isolated_apply_memory_bytes_max) rather than
# re-asserting its own separate verdict -- there must be exactly one place in this file that
# decides whether each of those three ceilings is enforced, or the two could drift apart the same
# way the old hardcoded "failed" literal drifted from its own evidence.
ISOLATION_LIMIT_KINDS = ("decode_apply_cpu_ms", "decode_apply_wall_ms", "isolated_apply_memory_bytes")
isolation_cases = [bc for bc in boundary_cases if bc["limit_kind"] in ISOLATION_LIMIT_KINDS]
isolation_passed_count = sum(1 for bc in isolation_cases if bc["status"] == "passed")
isolation = {
    "cpu_ms": None, "wall_ms": None, "peak_bytes": None, "terminated": None, "canonical_state_unchanged": None,
    "call_sites_outside_limits_rs": {
        bc["key"]: next(v for k, v in bc["evidence"].items() if k.endswith("_referenced_outside_limits_rs"))
        for bc in isolation_cases
    },
    "status": "passed" if isolation_passed_count == len(isolation_cases) else "failed",
    "reason": (
        f"{isolation_passed_count} of {len(isolation_cases)} isolation boundary_cases "
        "(decode_apply_cpu_ms_max/decode_apply_wall_ms_max/isolated_apply_memory_bytes_max) pass; "
        + "; ".join(f"{bc['key']}: {bc['reason']}" for bc in isolation_cases if bc["status"] != "passed")
    ) if isolation_passed_count < len(isolation_cases) else (
        f"all {len(isolation_cases)} isolation boundary_cases pass -- see boundary_cases for each "
        "ceiling's individual call-site and test evidence"
    ),
    "platform_note": (
        "per ADR-0014, decode_apply_cpu_ms and isolated_apply_memory_bytes are not_applicable_web/"
        "diagnostic_only on the browser platform by design (no trustworthy per-worker CPU/allocation "
        "meter exists in browsers); decode_apply_wall_ms plus forced termination is required on both "
        "platforms regardless of this note"
    ),
}

crq_keys = {"open_documents", "user_connections", "document_connections", "workspace_connections",
            "presence_entries_per_connection", "presence_entries_per_document", "frame_rate", "update_rate",
            "slow_consumer_queue_frames", "slow_consumer_queue_bytes"}
crq_cases = [bc for bc in boundary_cases if bc["limit_kind"] in crq_keys]
connection_rate_queue = {
    "cases": [{"limit_kind": c["limit_kind"], "status": c["status"], "reason": c["reason"]} for c in crq_cases],
    "all_passed": all(c["status"] == "passed" for c in crq_cases),
    "deterministic_virtual_clock_exists": False,
}

if collab_arch is not None:
    nb = collab_arch.get("numeric_budgets_check", {})
    gates = collab_arch.get("gates", {})
    persistence_path = {
        "source": "evidence/v0.4/collab-architecture-result.json (source_head-matched)",
        "cache_cases": nb.get("verified_portion_passed"),
        "lock_wait_ms": collab_arch.get("adr_check", {}).get("frozen_budgets", {}).get("lock_wait_ceiling_ms", {}).get("source_const"),
        "lock_hold_ms_p95": nb.get("load_test_targets_not_covered", {}).get("lock_hold_p95_ms"),
        "lock_hold_ms_max": collab_arch.get("adr_check", {}).get("frozen_budgets", {}).get("lock_hold_ceiling_ms", {}).get("source_const"),
        "round_trip_ms_p95": nb.get("load_test_targets_not_covered", {}).get("round_trip_ms_p95_10_clients"),
        "rebase_exhaustion_rollback": None,
        "snapshot_trigger_cases": gates.get("minimal_snapshot_advancement_bounds_tail", {}).get("status"),
        "all_passed": collab_arch.get("passed", False),
        "status": "failed" if collab_arch.get("passed") is False else "not_covered",
        "reason": (
            "collab-architecture-result.json's own p95 lock-hold/round-trip gate "
            "(bounded_warm_cache_lock_hold_and_round_trip_budgets) is failed: hard-ceiling constants "
            "verified against source and passing real tests, but no load-generation harness exists in "
            "this repository to measure p95 lock-hold<=25ms or 10-client round-trip p95<=250ms; "
            "document_prepare_rebase_attempts_max=3 exhaustion has no dynamic fixture forcing 3 real "
            "exhausted rebases either"
        ),
    }
else:
    persistence_path = {
        "source": None, "cache_cases": None, "lock_wait_ms": None, "lock_hold_ms_p95": None,
        "lock_hold_ms_max": None, "round_trip_ms_p95": None, "rebase_exhaustion_rollback": None,
        "snapshot_trigger_cases": None, "all_passed": False, "status": "not_covered",
        "reason": (
            "evidence/v0.4/collab-architecture-result.json not found (or its source_head does not "
            "match this run's HEAD) -- run scripts/verify-flow-collab-architecture.sh first; this "
            "verifier deliberately does not re-run its expensive DB-backed lock/snapshot test suite"
        ),
    }

dcc = static["delivery_cross_check"]
# `matches: null` means "no single constant to compare against" (recorded, not silently dropped);
# only an actual contract-vs-source disagreement is a violation.
delivery_constant_violations = [k for k, v in dcc.items() if v["matches"] is False]
delivery_constants_not_cross_checkable = [k for k, v in dcc.items() if v["matches"] is None]
delivery_path = {
    "max_attempts": dcc["delivery_max_attempts"]["source_value"],
    "retry_schedule": "next_attempt_at = now + min(attempts*30s, 300s) (delivery_backoff_ms formula; "
                       "not a frozen constant, contract gives only the formula)",
    "debounce_ms": dcc["content_delivery_debounce_ms"]["source_value"],
    "retention_days": dcc["delivery_retention_days"]["source_value"],
    "no_subscribers_retention_hours": dcc["dispatch_no_subscribers_retention_hours"]["source_value"],
    "no_subscribers_reaped": None,
    "coalesced_source_events_max": (
        "unset" if "coalesced_source_events_max" in static["unset_delivery_keys"] else "locked"
    ),
    "coalesced_cap_starts_new_row": None,
    "constant_cross_check": dcc,
    "constant_violations": delivery_constant_violations,
    "constants_not_cross_checkable": delivery_constants_not_cross_checkable,
    "unset_dispatch_budget_keys": static["unset_delivery_keys"],
    "deferred_dispatch_budget_exemptions": static["deferred_dispatch_budget_exemptions"],
    "dispatch_budget_accounting": static["dispatch_budget_accounting"],
}
if events is not None:
    egates = events.get("gates", {})
    delivery_path["no_subscribers_reaped"] = egates.get("no_subscribers_terminalized_and_reaped", {}).get("status") == "passed"
    delivery_path["coalesced_cap_starts_new_row"] = egates.get("coalescing_seal_and_source_first_expansion", {}).get("dynamic_passed")
    delivery_path["events_result_source"] = "evidence/v0.4/flow-events-result.json (source_head-matched)"
else:
    delivery_path["events_result_source"] = None

delivery_path["all_passed"] = (
    len(delivery_constant_violations) == 0 and len(static["unset_delivery_keys"]) == 0
    and delivery_path["no_subscribers_reaped"] is True
)
delivery_path["status"] = "passed" if delivery_path["all_passed"] else "failed"
if delivery_path["all_passed"]:
    delivery_path["reason"] = (
        "all frozen delivery-path constants match source, "
        + static["dispatch_budget_accounting"]["identity"]
        + " ("
        + "; ".join(
            f"{e['key']}: {e['reason_code']}, set_by {e['contract_set_by']}, re-demanded by {e['paired_gate']}"
            for e in static["deferred_dispatch_budget_exemptions"]
        )
        + "), zero required budget keys remain `status: unset` in contracts/limits-v1.md, and "
        "no_subscribers reaping per flow-events-result.json is confirmed"
    )
else:
    delivery_path["reason"] = (
        (f"{len(delivery_constant_violations)} frozen delivery-path constant mismatch(es): {delivery_constant_violations}; "
         if delivery_constant_violations else "")
        + f"{len(static['unset_delivery_keys'])} delivery-path budget key(s) still `status: unset` in "
          f"contracts/limits-v1.md, cannot be 'locked' while undefined: {static['unset_delivery_keys']}; "
        + ("no_subscribers reaping not independently confirmable (flow-events-result.json missing/stale)"
           if events is None else f"no_subscribers reaping per flow-events-result.json: {delivery_path['no_subscribers_reaped']}")
    )

dispatch_numeric_budgets_locked_status = (
    "passed" if len(static["unset_delivery_keys"]) == 0 and len(delivery_constant_violations) == 0 else "failed"
)

server_canonical = json.dumps(static["wire_fields"], sort_keys=True, separators=(",", ":"))
server_limits_sha256 = hashlib.sha256(server_canonical.encode()).hexdigest()
fe_parity = static["frontend_bootstrap_parity"]
web_wire_fields = {"version": fe_parity["default_version"], **fe_parity["default_wire_fields"]}
web_canonical = json.dumps(web_wire_fields, sort_keys=True, separators=(",", ":"))
web_limits_sha256 = hashlib.sha256(web_canonical.encode()).hexdigest()
web_value_mismatches = {
    field: {"server": static["wire_fields"].get(field), "web": web_wire_fields.get(field)}
    for field in sorted(set(static["wire_fields"]) | set(web_wire_fields))
    if static["wire_fields"].get(field) != web_wire_fields.get(field)
}

wire_spotcheck = None
bootstrap_test_status = dtest("bootstrap_endpoint_returns_the_full_shape_for_a_user_and_rejects_a_bot")
if bootstrap_test_status is not None:
    wire_spotcheck = {
        "test": "routes::flow::flow_database_tests::bootstrap_endpoint_returns_the_full_shape_for_a_user_and_rejects_a_bot",
        "status": bootstrap_test_status,
        "asserts": ["limits.version==sylvode.flow.limits.v1", "update_bytes_max==65536",
                    "bootstrap_decoded_bytes_max==8388608", "import_compression_ratio_max==100"],
    }

bootstrap_parity_field_sets_match = (
    len(fe_parity["missing_in_frontend"]) == 0
    and len(fe_parity["extra_in_frontend"]) == 0
    and len(fe_parity["default_missing"]) == 0
    and len(fe_parity["default_extra"]) == 0
    and len(fe_parity["default_parse_errors"]) == 0
)
bootstrap_parity_unknown_version_found = f["frontend_unknown_version_handling_found"]
bootstrap_parity_values_match = web_limits_sha256 == server_limits_sha256 and not web_value_mismatches
bootstrap_parity_ok = (
    bootstrap_parity_field_sets_match
    and bootstrap_parity_values_match
    and bootstrap_parity_unknown_version_found
)

bootstrap_parity = {
    "server_limits_sha256": server_limits_sha256,
    "server_limits_field_count": fe_parity["server_field_count"],
    "server_wire_spotcheck": wire_spotcheck,
    "web_limits_sha256": web_limits_sha256,
    "web_limits_field_count": fe_parity["frontend_field_count"],
    "unknown_version_read_only": "not_covered" if not bootstrap_parity_unknown_version_found else "found",
    "missing_in_frontend": fe_parity["missing_in_frontend"],
    "extra_in_frontend": fe_parity["extra_in_frontend"],
    "web_default_parse_errors": fe_parity["default_parse_errors"],
    "value_mismatches": web_value_mismatches,
    "status": "passed" if bootstrap_parity_ok else "failed",
    "reason": (
        "frontend/src/lib/flow/types.ts's FlowLimitsV1 interface and limits.ts's 35 numeric "
        "DEFAULT_FLOW_LIMITS values match the server wire schema byte-for-byte under canonical "
        f"JSON sha256={server_limits_sha256}, and unknown-version read-only negotiation is present"
        if bootstrap_parity_ok
        else (
            f"frontend/src/lib/flow/types.ts's FlowLimitsV1 interface declares only "
            f"{fe_parity['frontend_field_count']} of the server's {fe_parity['server_field_count']} wire "
            f"fields ({len(fe_parity['missing_in_frontend'])} missing entirely, including "
            "bootstrap_decoded_bytes_max/isolated_apply_memory_bytes_max/all connection+rate+import "
            f"fields), plus {len(fe_parity['extra_in_frontend'])} field(s) under different names than "
            f"the wire schema ({fe_parity['extra_in_frontend']} vs the server's frame/presence byte "
            "field names); "
            if not bootstrap_parity_field_sets_match else ""
        )
        + (
            "frontend/src/lib/flow/limits.ts's DEFAULT_FLOW_LIMITS could not be parsed as exactly 35 "
            f"numeric literal fields: {fe_parity['default_parse_errors']}; "
            if fe_parity["default_parse_errors"] else ""
        )
        + (
            f"server/web canonical sha256 differs ({server_limits_sha256} != {web_limits_sha256}); "
            f"value mismatches: {web_value_mismatches}; "
            if not bootstrap_parity_values_match else ""
        )
        + (
            "`unknown_version_read_only` is not_covered: no version-negotiation code for an "
            "unrecognized limits.version was found in frontend/src (grepped types.ts + limits.ts)."
            if not bootstrap_parity_unknown_version_found
            else "version-negotiation code for an unrecognized limits.version was found in frontend/src."
        )
    ),
}

WIRE_KIND_TESTS = {
    "bootstrap_decoded_bytes": ["bootstrap_decoded_bytes_exact_boundary_is_accepted_and_plus_one_is_rejected"],
    "bootstrap_response_bytes": ["bootstrap_response_bytes_exact_boundary_is_accepted_and_plus_one_is_rejected"],
    "container_count": ["ws_structural_limit_container_count_exact_boundary_accepted_plus_one_rejected_zero_side_effects"],
    "decode_apply_cpu_ms": [
        "decode_apply_cpu_ms_ceiling_accepts_just_under_and_kills_with_cpu_ceiling_just_over_the_boundary",
        "every_isolated_apply_resource_ceiling_rejects_with_its_frozen_limit_kind_and_a_numeric_limit",
    ],
    "decode_apply_wall_ms": [
        "decode_apply_wall_ms_ceiling_accepts_just_under_and_kills_with_wall_ceiling_just_over_the_boundary",
        "every_isolated_apply_resource_ceiling_rejects_with_its_frozen_limit_kind_and_a_numeric_limit",
    ],
    "document_block_count": ["ws_structural_limit_document_block_count_exact_boundary_accepted_plus_one_rejected_zero_side_effects"],
    "document_connections": [
        "per_document_connection_ceiling_is_enforced",
        "every_connection_ceiling_renders_its_frozen_limit_kind_and_limit_into_the_rejection_frame",
    ],
    "document_text_chars": ["ws_structural_limit_document_text_chars_exact_boundary_accepted_plus_one_rejected_zero_side_effects"],
    "frame_rate": ["frame_rate_ceiling_is_observable_as_limit_exceeded_without_closing_the_connection"],
    "isolated_apply_memory_bytes": [
        "isolated_apply_memory_bytes_ceiling_accepts_the_exact_byte_and_rejects_the_next_byte_over",
        "every_isolated_apply_resource_ceiling_rejects_with_its_frozen_limit_kind_and_a_numeric_limit",
    ],
    "open_documents": ["open_documents_ceiling_is_observable_as_limit_exceeded_on_the_ninth_attempt"],
    "page_size": ["list_objects_endpoint_rejects_page_size_over_page_limit_max_and_accepts_exact_boundary"],
    "presence_entries_per_connection": [
        "per_connection_presence_ceiling_is_enforced",
        "both_presence_ceilings_render_their_frozen_limit_kind_and_limit_into_the_rejection_frame",
    ],
    "presence_entries_per_document": [
        "per_document_presence_ceiling_is_enforced_and_does_not_evict_others",
        "both_presence_ceilings_render_their_frozen_limit_kind_and_limit_into_the_rejection_frame",
    ],
    "presence_payload_bytes": ["presence_payload_bytes_exact_boundary_accepted_plus_one_rejected_zero_side_effects"],
    "presence_ttl_seconds": ["presence_ttl_seconds_exact_boundary_accepted_plus_one_rejected_zero_side_effects"],
    "scan_budget": ["check_scan_budget_rejects_one_row_past_the_authorized_scan_rows_max_boundary"],
    "semantic_patch_bytes": ["commands_endpoint_semantic_patch_bytes_exact_boundary_accepted_plus_one_rejected_zero_writes"],
    "semantic_patch_operations": ["commands_endpoint_update_block_rejects_semantic_patch_operations_batch_plus_one_and_accepts_exact_boundary"],
    "slow_consumer_queue_bytes": ["a_slow_consumer_is_force_closed_once_the_queue_byte_ceiling_is_exceeded_independent_of_frame_count"],
    "slow_consumer_queue_frames": ["a_slow_consumer_is_force_closed_once_the_queue_frame_ceiling_is_exceeded"],
    "text_block_chars": ["ws_structural_limit_text_block_chars_exact_boundary_accepted_plus_one_rejected_zero_side_effects"],
    "tree_depth": ["ws_structural_limit_tree_depth_exact_boundary_accepted_plus_one_rejected_zero_side_effects"],
    "update_bytes": ["ws_structural_limit_update_bytes_exact_boundary_accepted_plus_one_rejected_zero_side_effects"],
    "update_rate": ["update_rate_ceiling_is_observable_as_limit_exceeded_separately_from_the_frame_rate"],
    "user_connections": [
        "user_connections_ceiling_refuses_the_seventeenth_session_with_the_frozen_limit_kind",
        "every_connection_ceiling_renders_its_frozen_limit_kind_and_limit_into_the_rejection_frame",
    ],
    "websocket_frame_bytes": ["websocket_frame_bytes_exact_boundary_accepted_plus_one_rejected_zero_side_effects"],
    "workspace_connections": [
        "per_workspace_connection_ceiling_is_enforced",
        "every_connection_ceiling_renders_its_frozen_limit_kind_and_limit_into_the_rejection_frame",
    ],
}
privacy_omits_observed = {
    "user_connections",
    "document_connections",
    "workspace_connections",
    "presence_entries_per_document",
}
producer_evidence_by_kind = {}
observed = []
for limit_kind, tests in WIRE_KIND_TESTS.items():
    statuses = {name: dtest(name) for name in tests}
    passed = all(status == "ok" for status in statuses.values())
    producer_evidence_by_kind[limit_kind] = {
        "tests": statuses,
        "limit_kind_and_numeric_limit_required": True,
        "observed_field_required": limit_kind not in privacy_omits_observed,
        "observed_field_policy": (
            "omitted_by_contract_to_avoid_disclosing_other_user_or_session_counts"
            if limit_kind in privacy_omits_observed
            else "optional_on_wire_but_present_where_the_producer_can_report_it_safely"
        ),
        "status": "passed" if passed else "failed",
    }
    if passed:
        observed.append(limit_kind)
observed.sort()
expected = static["expected_limit_kinds"]
missing = sorted(set(expected) - set(observed))
unknown = sorted(set(observed) - set(expected))
error_kind_coverage = {
    "defined": static["fixed_limit_kind_count"],
    "evaluated": static["evaluated_limit_kind_count"],
    "expected": expected,
    "observed": observed,
    "missing": missing,
    "unknown": unknown,
    "not_applicable": static["version_boundary_exemptions"],
    "producer_evidence_by_kind": producer_evidence_by_kind,
    "note": (
        "observed[] is derived only from currently-passing producer and wire-shape tests named in "
        "producer_evidence_by_kind. Every counted rejection carries its exact limit_kind and a numeric "
        "limit. Connection ceilings and presence_entries_per_document intentionally omit `observed` "
        "because returning aggregate peer counts would disclose other users/sessions; that contract-"
        "required omission is not treated as missing evidence. The four package-import kinds remain "
        "outside expected[] under not_applicable_until_v0_8 and are not permanent exemptions."
    ),
}

# ---- self-consistency assertion: a case's reason may never contradict its own evidence ----
#
# This is the exact bug this script shipped with until 2026-08-30: every boundary_case's `status`
# was a hardcoded string literal (`"failed"`), so when `check_operation`/`check_operation_batch_count`
# actually got wired into apps/api/src/flow/command.rs, the dynamically-computed
# `call_sites_in_apps_api_command_rs` evidence field correctly flipped from 0 to a positive count --
# but the hand-written prose right next to it, in the SAME case object, kept reading "has 0 call
# sites ... not wired into any endpoint". A verifier able to assert those two things in the same
# breath is not trustworthy input to a release decision, and no amount of fixing individual
# `status` computations (above) rules out some future case, or some future edit to one of these
# case()-building blocks, reintroducing the same drift by accident. So this check runs
# unconditionally, on every run, on the actual objects this script is about to write out -- not on
# the code that built them -- and refuses to write evidence at all if it ever finds one.
#
# It looks for two specific self-contradicting phrase families in `reason` text ("zero/0/never/
# verified absent/not wired"-style claims of nonexistence, and "N of M pass"-style claims of a
# subset), and cross-checks them against that same object's own positive-integer evidence. A
# `reason` claiming "0 call sites" while `evidence.call_sites_in_apps_api_command_rs == 2` sits
# right next to it is exactly the shape of contradiction this exists to catch.
ZERO_CLAIM_RE = re.compile(
    r"\b(?:has\s+)?0\s+(?:call\s+sites?|references?|enforcement\s+call\s+sites?)\b"
    r"|\bzero\s+(?:call\s+sites?|references?|hits?)\b"
    r"|\bverified\s+absent\b"
    r"|\bnot\s+wired\b"
    r"|\bnever\s+mentions\b"
    r"|\bnever\s+reads\b",
    re.I,
)


def _positive_ints(obj):
    found = []

    def walk(o):
        if isinstance(o, dict):
            for k, v in o.items():
                if isinstance(v, bool):
                    continue
                if isinstance(v, int):
                    if v > 0:
                        found.append(v)
                elif isinstance(v, dict):
                    walk(v)

    walk(obj)
    return found


def assert_no_self_contradiction(named_objects):
    problems = []
    for name, obj in named_objects:
        reason = obj.get("reason") or ""
        if not ZERO_CLAIM_RE.search(reason):
            continue
        nonzero = _positive_ints(obj.get("evidence", {}))
        if nonzero:
            problems.append(
                f"{name}: reason claims zero/absent/unwired/never ({reason[:160]!r}...) but its own "
                f"evidence dict carries positive count(s) {nonzero}"
            )
    return problems


self_consistency_problems = assert_no_self_contradiction(
    [(f"boundary_cases[key={bc['key']}]", bc) for bc in boundary_cases]
)
if self_consistency_problems:
    print(json.dumps({
        "error": "self-consistency check failed -- refusing to write evidence with a case whose "
                  "reason contradicts its own evidence: " + "; ".join(self_consistency_problems)
    }))
    sys.exit(0)

result = {
    "schema_version": "sylvode.flow.limits-result.v1",
    "source_head": source_head,
    "contract_sha256": contract_sha256,
    "generated_at": generated_at,
    "executor": "scripts/verify-flow-limits-v0.4.sh",
    "engine": "loro",
    "limit_kind_accounting": {
        "defined": static["fixed_limit_kind_count"],
        "evaluated": static["evaluated_limit_kind_count"],
        "not_applicable_count": len(static["version_boundary_exemptions"]),
        "equation": (
            f"{static['fixed_limit_kind_count']} = {static['evaluated_limit_kind_count']} evaluated + "
            f"{len(static['version_boundary_exemptions'])} not_applicable_until_v0_8"
        ),
        "not_applicable": static["version_boundary_exemptions"],
    },
    "boundary_cases": boundary_cases,
    "isolation": isolation,
    "connection_rate_queue": connection_rate_queue,
    "persistence_path": persistence_path,
    "delivery_path": delivery_path,
    "bootstrap_parity": bootstrap_parity,
    "error_kind_coverage": error_kind_coverage,
    "row_cross_check": static["row_cross_check"],
    "row_violations": static["row_violations"],
    "wire_cross_check_violations": static["wire_violations"],
    "hard_gates": {
        "flow_limits_exact_boundary_and_plus_one_rejection": (
            "passed" if all(c["status"] == "passed" for c in boundary_cases) else "failed"
        ),
        "isolated_decode_apply_cpu_wall_memory": isolation["status"],
        "websocket_rate_connection_and_backpressure_limits": "passed" if connection_rate_queue["all_passed"] else "failed",
        "bootstrap_limits_web_server_parity": bootstrap_parity["status"],
        "limit_exceeded_kind_coverage": "passed" if not missing and not unknown else "failed",
        "dispatch_numeric_budgets_locked": dispatch_numeric_budgets_locked_status,
    },
}
result["passed"] = all(v == "passed" for v in result["hard_gates"].values())

with open(out_path, "w", encoding="utf-8") as fh:
    json.dump(result, fh, indent=2, sort_keys=False)
    fh.write("\n")
print(json.dumps(result))
PY
)"

if jq -e 'has("error")' >/dev/null 2>&1 <<<"$FINAL_JSON"; then
  echo "FAIL: $(jq -r '.error' <<<"$FINAL_JSON")" >&2
  exit 2
fi

sync "$OUT_TMP" 2>/dev/null || true
mv -f "$OUT_TMP" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2

echo "=== hard gate verdicts ===" >&2
jq -r '.hard_gates | to_entries[] | "  \(.key): \(.value)"' <<<"$FINAL_JSON" >&2

echo "$FINAL_JSON"

OVERALL_PASSED="$(jq -r '.passed' <<<"$FINAL_JSON")"
if [[ "$OVERALL_PASSED" == "true" ]]; then
  exit 0
fi
exit 1
