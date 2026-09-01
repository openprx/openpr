#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.5 multi-document lock-order and atomicity verifier.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md, "Multi-document
# verifier" (including the 2026-08-31 R17 reachability refinement), and
# ADR-0013 section 2 plus its Gate wiring table.
#
# This verifier deliberately distinguishes three kinds of evidence:
#   1. real Rust tests for the two-document move and the layer-0 coordinator;
#   2. fail-closed source enumeration for R17's two database reachability
#      premises (transaction scope, not request scope); and
#   3. the mixed move/grant/subscription/content injection, which is not wired
#      in this source package yet and therefore remains `not_implemented`.
# It also refuses to attribute observations from dirty validated source to the
# clean `source_head`: any change under the Cargo workspace source/migrations
# scope makes the artifact a hard non-pass and is listed in the JSON.
#
# Exit codes: 0 = every assertion passed, 1 = an assertion failed or required
# evidence is not implemented, 2 = usage/tool/evidence malformed.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/flow_contract_path.sh
source "$ROOT_DIR/scripts/lib/flow_contract_path.sh"

REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.5"
ADR_PATH=""
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-multi-document-v0.5.sh --adr PATH --json [OPTIONS]

Verifies the v0.5 multi-document move/coordinator tests and R17's two static
database reachability premises. Writes multi-document-result.json.

Options:
  --adr PATH              Path to ADR-0013. Required. Relative paths are
                          resolved against the current directory first, then
                          against --contracts-root.
  --contracts-root DIR    Root containing decisions/. Default:
                          /opt/working/sylvode-flow
  --evidence-root DIR     Where multi-document-result.json is written.
                          Default: /opt/working/sylvode-flow/evidence/v0.5
  --repo-root DIR         Repository containing apps/api and the Cargo
                          workspace. Default: this checkout.
  --json                  Required for CLI-contract compatibility.
  -h, --help              Show this help and exit 0.

Exit codes: 0 all checks passed, 1 an assertion failed/not implemented,
2 usage/tool/evidence malformed.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --adr) ADR_PATH="${2:?--adr requires a PATH argument}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$ADR_PATH" ]]; then
  echo "FAIL: --adr is required" >&2
  usage >&2
  exit 2
fi
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
if ! ADR_PATH="$(flow_resolve_contract_path --adr "$ADR_PATH" "$CONTRACTS_ROOT")"; then
  exit 2
fi
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi

FLOW_ROOT="$REPO_ROOT/apps/api/src/flow"
for source_file in \
  "$FLOW_ROOT/collab/authz.rs" \
  "$FLOW_ROOT/collab/coordinator.rs" \
  "$FLOW_ROOT/collab/snapshot.rs" \
  "$FLOW_ROOT/collab/write.rs" \
  "$FLOW_ROOT/command.rs" \
  "$FLOW_ROOT/move_object.rs"; do
  if [[ ! -f "$source_file" ]]; then
    echo "FAIL: source file not found (nothing to verify): $source_file" >&2
    exit 2
  fi
done

mkdir -p "$EVIDENCE_ROOT/logs"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# `source_head` names a commit, while Cargo and the static parser read the work
# tree. Only changes that can alter this verifier's observed API/migration
# behavior belong to this check: the Cargo workspace source, migrations and
# root Cargo resolution files. The new verifier itself is intentionally not in
# this scope; otherwise an uncommitted evidence-producer file would obscure the
# separate question of whether the *validated source* matches HEAD.
SOURCE_DIRTY_SCOPE_JSON='["apps/","crates/","spikes/","migrations/",".cargo/","Cargo.toml","Cargo.lock"]'
SOURCE_DIRTY_STATUS="$(git -C "$REPO_ROOT" status --porcelain=v1 --untracked-files=all -- \
  apps crates spikes migrations .cargo Cargo.toml Cargo.lock)"
SOURCE_DIRTY_ENTRIES_JSON="$(printf '%s\n' "$SOURCE_DIRTY_STATUS" | jq -R 'select(length > 0)' | jq -s '.')"
SOURCE_DIRTY=$([[ -n "$SOURCE_DIRTY_STATUS" ]] && echo true || echo false)

# ---- 1. R17 static premises (read-only source enumeration) ----
#
# The parser fails closed on any new production canonical-head SQL write or
# BoundedMany declaration. Tests are conventionally at the end of each Flow
# module and are excluded at the first #[cfg(test)] boundary; all currently
# discovered production sites and callers are returned in the artifact.
STATIC_JSON="$(python3 - "$FLOW_ROOT" <<'PY'
import json
import pathlib
import re
import sys

flow_root = pathlib.Path(sys.argv[1])


def read(relative):
    return (flow_root / relative).read_text(encoding="utf-8")


def production(text):
    return text.split("#[cfg(test)]", 1)[0]


def line_at(text, offset):
    return text.count("\n", 0, offset) + 1


def fn_slice(text, name, occurrence=0):
    matches = list(
        re.finditer(
            rf"(?:pub(?:\(crate\))?\s+)?(?:async\s+)?fn\s+{re.escape(name)}\s*(?:<[^{{]+>)?\s*\(",
            text,
        )
    )
    if len(matches) <= occurrence:
        return None
    start = matches[occurrence].start()
    brace = text.find("{", matches[occurrence].end())
    if brace < 0:
        return None
    # The inspected functions contain no brace-bearing raw strings before the
    # relevant calls. Brace matching gives a stable function boundary instead
    # of depending on line numbers.
    depth = 0
    for pos in range(brace, len(text)):
        char = text[pos]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return (start, pos + 1, text[start : pos + 1])
    return None


def uncomment_code(text):
    # Preserve offsets so evidence line numbers still refer to the real file.
    def blank(match):
        return "".join("\n" if char == "\n" else " " for char in match.group(0))

    text = re.sub(r"/\*.*?\*/", blank, text, flags=re.S)
    return re.sub(r"(?m)//.*$", blank, text)


sources = {}
for path in sorted(flow_root.rglob("*.rs")):
    relative = str(path.relative_to(flow_root))
    sources[relative] = production(path.read_text(encoding="utf-8"))

write = sources["collab/write.rs"]
move = sources["move_object.rs"]
authz = sources["collab/authz.rs"]
snapshot = sources["collab/snapshot.rs"]
command = sources["command.rs"]

head_violations = []
head_write_sites = []
head_sql_pattern = re.compile(
    r"UPDATE\s+collab_documents\s+SET\s+(.*?)\s+WHERE\b", re.I | re.S
)
for relative, src in sources.items():
    for match in head_sql_pattern.finditer(src):
        assigned = sorted(set(re.findall(r"\b(head_seq|head_frontier)\s*=", match.group(1), re.I)))
        if assigned:
            head_write_sites.append(
                {
                    "file": f"apps/api/src/flow/{relative}",
                    "line": line_at(src, match.start()),
                    "assigned_head_columns": assigned,
                }
            )

stage_one = fn_slice(write, "stage_one_document")
stage_locked = fn_slice(write, "stage_locked_writes")
move_locked = fn_slice(move, "run_locked_phase")
snapshot_commit = fn_slice(snapshot, "commit_candidate")

if stage_one is None or stage_locked is None or move_locked is None or snapshot_commit is None:
    head_violations.append("could not locate all required production transaction functions")
else:
    stage_start, stage_end, stage_body = stage_one
    expected_sites = [site for site in head_write_sites if site["file"].endswith("collab/write.rs")]
    if len(head_write_sites) != 1 or len(expected_sites) != 1:
        head_violations.append(
            "production canonical-head SQL write enumeration changed; expected only collab/write.rs::stage_one_document"
        )
    else:
        site_offset = next(
            match.start()
            for match in head_sql_pattern.finditer(write)
            if re.search(r"\b(head_seq|head_frontier)\s*=", match.group(1), re.I)
        )
        if not (stage_start <= site_offset < stage_end):
            head_violations.append("the sole canonical-head SQL write is outside stage_one_document")

    if "FROM collab_documents WHERE id = $1 FOR UPDATE" not in stage_body:
        head_violations.append("stage_one_document no longer takes the document row FOR UPDATE")

    _, _, content_body = stage_locked
    content_epoch = content_body.find("fence_epoch_for_share(")
    content_stage = content_body.find("stage_one_document(")
    if content_epoch < 0 or content_stage < 0 or content_epoch >= content_stage:
        head_violations.append(
            "content-write transaction does not take the workspace epoch lock before stage_one_document"
        )

    _, _, move_body = move_locked
    move_epoch = move_body.find("lock_epoch_for_update(")
    move_document = move_body.find("lock_document_head(")
    move_stage = move_body.find("stage_one_document(")
    if min(move_epoch, move_document, move_stage) < 0 or not (move_epoch < move_document < move_stage):
        head_violations.append(
            "move transaction does not take its epoch conflict lock before document locks/head advancement"
        )

call_sites = []
for relative, src in sources.items():
    for match in re.finditer(r"(?:write::)?stage_one_document\s*\(", src):
        prefix = src[max(0, match.start() - 40) : match.start()]
        if re.search(r"fn\s+$", prefix):
            continue
        call_sites.append(
            {"file": f"apps/api/src/flow/{relative}", "line": line_at(src, match.start())}
        )
if sorted(site["file"] for site in call_sites) != sorted(
    ["apps/api/src/flow/collab/write.rs", "apps/api/src/flow/move_object.rs"]
):
    head_violations.append(
        "stage_one_document production callers changed; every new caller requires an epoch-before-document proof"
    )

if "SELECT authz_epoch FROM flow_workspace_settings WHERE workspace_id = $1 FOR SHARE" not in authz:
    head_violations.append("content epoch fence is not a workspace-row FOR SHARE lock")
if "SELECT authz_epoch FROM flow_workspace_settings WHERE workspace_id = $1 FOR UPDATE" not in authz:
    head_violations.append("move epoch lock is not a workspace-row FOR UPDATE conflict lock")

snapshot_info = {
    "function": "apps/api/src/flow/collab/snapshot.rs::commit_candidate",
    "classification": "allowed_non_head_transaction",
    "takes_collab_document_for_update": False,
    "advances_canonical_head": False,
    "requests_epoch_after_document_lock": False,
}
if snapshot_commit is not None:
    _, _, snapshot_body = snapshot_commit
    snapshot_info["takes_collab_document_for_update"] = (
        "FROM collab_documents WHERE id = $1 FOR UPDATE" in snapshot_body
    )
    snapshot_assignments = []
    for match in head_sql_pattern.finditer(snapshot_body):
        snapshot_assignments.extend(
            re.findall(r"\b(head_seq|head_frontier)\s*=", match.group(1), re.I)
        )
    snapshot_info["advances_canonical_head"] = bool(snapshot_assignments)
    document_pos = snapshot_body.find("FROM collab_documents WHERE id = $1 FOR UPDATE")
    tail = snapshot_body[document_pos:] if document_pos >= 0 else snapshot_body
    snapshot_info["requests_epoch_after_document_lock"] = bool(
        re.search(r"(?:lock|fence)_epoch|authz_epoch", tail)
    )
    if not snapshot_info["takes_collab_document_for_update"]:
        head_violations.append("snapshot checkpoint shape changed: document FOR UPDATE was not found")
    if snapshot_info["advances_canonical_head"]:
        head_violations.append("snapshot checkpoint now advances a canonical head and requires an epoch-order proof")
    if snapshot_info["requests_epoch_after_document_lock"]:
        head_violations.append("snapshot checkpoint requests epoch after its document lock, creating an inverse rank")

multi_violations = []
bounded_many_declarations = []
for relative, src in sources.items():
    code = uncomment_code(src)
    for match in re.finditer(
        r"Self::(\w+)\s*=>\s*ExistingDocumentCardinality::BoundedMany\(([^)]+)\)", code
    ):
        bounded_many_declarations.append(
            {
                "variant": match.group(1),
                "bound_expression": match.group(2).strip(),
                "file": f"apps/api/src/flow/{relative}",
                "line": line_at(src, match.start()),
            }
        )

expected_bounded = [
    entry
    for entry in bounded_many_declarations
    if entry["variant"] == "MoveObject"
    and entry["file"] == "apps/api/src/flow/move_object.rs"
]
if len(bounded_many_declarations) != 1 or len(expected_bounded) != 1:
    multi_violations.append(
        "BoundedMany production declaration set changed; expected only move_object at v0.5"
    )
if not re.search(r"MOVE_OBJECT_CONTENDED_DOCUMENT_MAX\s*:\s*u8\s*=\s*([2-9]|[1-9][0-9]+)\s*;", move):
    multi_violations.append("move_object's declared existing-document bound is missing or less than two")

registry = fn_slice(command, "v0_5_command_cardinality_registry")
if registry is None or "GovernanceCommandType::MoveObject" not in registry[2]:
    multi_violations.append("v0.5 cardinality registry does not enumerate move_object")

if move_locked is not None:
    _, _, move_body = move_locked
    epoch_pos = move_body.find("lock_epoch_for_update(")
    doc_pos = move_body.find("lock_document_head(")
    if epoch_pos < 0 or doc_pos < 0 or epoch_pos >= doc_pos:
        multi_violations.append(
            "the BoundedMany move transaction does not take epoch FOR UPDATE before document locks"
        )
if "SELECT authz_epoch FROM flow_workspace_settings WHERE workspace_id = $1 FOR UPDATE" not in authz:
    multi_violations.append("lock_epoch_for_update does not implement an epoch conflict lock")

print(
    json.dumps(
        {
            "canonical_head_transactions_epoch_before_document_lock": {
                "status": "passed" if not head_violations else "failed",
                "canonical_head_write_sites": head_write_sites,
                "stage_one_document_call_sites": call_sites,
                "content_write_epoch_lock": "FOR SHARE",
                "move_epoch_lock": "FOR UPDATE",
                "snapshot_checkpoint": snapshot_info,
                "violations": head_violations,
                "passed": not head_violations,
            },
            "multi_document_transactions_epoch_conflict_lock": {
                "status": "passed" if not multi_violations else "failed",
                "bounded_many_declarations": bounded_many_declarations,
                "conflict_lock": "SELECT authz_epoch ... FOR UPDATE",
                "violations": multi_violations,
                "passed": not multi_violations,
            },
        },
        separators=(",", ":"),
    )
)
PY
)"

if ! jq -e . >/dev/null 2>&1 <<<"$STATIC_JSON"; then
  echo "FAIL: R17 static parser did not produce valid JSON" >&2
  echo "$STATIC_JSON" >&2
  exit 2
fi

# ---- 2. Dynamic move/coordinator tests ----
MOVE_LOG="$EVIDENCE_ROOT/logs/multi-document.move_object.cargo_test.log"
COORDINATOR_LOG="$EVIDENCE_ROOT/logs/multi-document.coordinator.cargo_test.log"

run_cargo_suite() {
  local filter="$1" log="$2" start_ns end_ns status
  start_ns="$(date +%s%N)"
  set +e
  (cd "$REPO_ROOT" && cargo test -p api --all-features "$filter" -- --nocapture --test-threads=1) >"$log" 2>&1
  status=$?
  set -e
  end_ns="$(date +%s%N)"
  printf '%s %s\n' "$status" "$(((end_ns - start_ns) / 1000000))"
}

echo "=== cargo test -p api --all-features move_object ===" >&2
read -r MOVE_EXIT MOVE_WALL_MS < <(run_cargo_suite move_object "$MOVE_LOG")
echo "  exit=$MOVE_EXIT wall_ms=$MOVE_WALL_MS log=$MOVE_LOG" >&2

echo "=== cargo test -p api --all-features collab::coordinator ===" >&2
read -r COORDINATOR_EXIT COORDINATOR_WALL_MS < <(run_cargo_suite collab::coordinator "$COORDINATOR_LOG")
echo "  exit=$COORDINATOR_EXIT wall_ms=$COORDINATOR_WALL_MS log=$COORDINATOR_LOG" >&2

DYNAMIC_JSON="$(python3 - \
  "$MOVE_LOG" "$MOVE_EXIT" "$MOVE_WALL_MS" \
  "$COORDINATOR_LOG" "$COORDINATOR_EXIT" "$COORDINATOR_WALL_MS" <<'PY'
import json
import pathlib
import re
import sys

move_log, move_exit, move_wall, coordinator_log, coordinator_exit, coordinator_wall = sys.argv[1:]


def parse_suite(path, command, exit_code, wall_ms, tests):
    text = pathlib.Path(path).read_text(encoding="utf-8", errors="replace")
    lines = text.splitlines()
    summary_seconds = None
    summaries = re.findall(r"test result:.*?finished in ([0-9]+(?:\.[0-9]+)?)s", text)
    if summaries:
        # The first test binary is the api library that owns these filtered tests;
        # later binaries commonly report a misleading 0 tests / 0.00s.
        summary_seconds = float(summaries[0])
    assertions = []
    for name, requires_database in tests:
        indices = [index for index, line in enumerate(lines) if line.startswith("test ") and name in line]
        observed = bool(indices)
        segment = ""
        if observed:
            start = indices[0]
            end = len(lines)
            for index in range(start + 1, len(lines)):
                if lines[index].startswith("test ") or lines[index].startswith("test result:"):
                    end = index
                    break
            segment = "\n".join(lines[start:end])
        passed_by_harness = observed and bool(re.search(r"(?:\.\.\.\s*ok\b|(?:^|\n)ok\b)", segment))
        skipped = requires_database and "skipped: OPENPR_TEST_DATABASE_URL is not set" in segment
        near_zero = requires_database and summary_seconds is not None and summary_seconds <= 0.01
        passed = exit_code == 0 and passed_by_harness and not skipped and not near_zero
        reasons = []
        if not observed:
            reasons.append("named test was not observed in cargo output")
        elif not passed_by_harness:
            reasons.append("named test did not report ok")
        if skipped:
            reasons.append("real-database test early-returned because OPENPR_TEST_DATABASE_URL was not set")
        if near_zero:
            reasons.append("real-database suite reported near-zero libtest duration (possible early-return false green)")
        if exit_code != 0:
            reasons.append(f"cargo exited {exit_code}")
        assertions.append(
            {
                "name": name,
                "requires_real_database": requires_database,
                "observed": observed,
                "harness_status": "passed" if passed_by_harness else "failed",
                "executed": observed and not skipped and not near_zero,
                "suite_elapsed_seconds": summary_seconds,
                "suite_wall_clock_ms": int(wall_ms),
                "status": "passed" if passed else "failed",
                "reasons": reasons,
                "passed": passed,
            }
        )
    return {
        "command": command,
        "exit_code": int(exit_code),
        "wall_clock_ms": int(wall_ms),
        "libtest_elapsed_seconds": summary_seconds,
        "log": str(path),
        "assertions": assertions,
        "passed": bool(assertions) and all(item["passed"] for item in assertions),
    }


move = parse_suite(
    move_log,
    "cargo test -p api --all-features move_object",
    int(move_exit),
    move_wall,
    [
        ("document_lock_order_is_identical_whichever_direction_the_move_goes", True),
        ("cross_project_move_advances_both_navigator_heads_in_one_ascending_ordered_transaction", True),
        ("two_concurrent_moves_with_reversed_contended_sets_both_complete_cleanly", True),
    ],
)
coordinator = parse_suite(
    coordinator_log,
    "cargo test -p api --all-features collab::coordinator",
    int(coordinator_exit),
    coordinator_wall,
    [("reversed_multi_document_requests_both_succeed_because_acquire_many_sorts", False)],
)
print(
    json.dumps(
        {
            "move_object": move,
            "collab_coordinator": coordinator,
            "passed": move["passed"] and coordinator["passed"],
        },
        separators=(",", ":"),
    )
)
PY
)"

if ! jq -e . >/dev/null 2>&1 <<<"$DYNAMIC_JSON"; then
  echo "FAIL: dynamic test parser did not produce valid JSON" >&2
  echo "$DYNAMIC_JSON" >&2
  exit 2
fi

# The grant and cross-instance subscription-change producers belong to other
# v0.5 work packages. A source/test-only approximation is not evidence for the
# contract's controlled four-way injection, so this remains a hard non-pass.
MIXED_INJECTION_JSON="$(jq -n '{
  status: "not_implemented",
  components: ["move_object", "grant_change", "subscription_change", "content_write"],
  reason: "grant-change and subscription-change injection producers are not both available in this package; the required controlled mixed injection was not run",
  passed: false
}')"

STATIC_PASSED="$(jq -r '[.canonical_head_transactions_epoch_before_document_lock.passed, .multi_document_transactions_epoch_conflict_lock.passed] | all' <<<"$STATIC_JSON")"
DYNAMIC_PASSED="$(jq -r '.passed' <<<"$DYNAMIC_JSON")"
MIXED_PASSED="$(jq -r '.passed' <<<"$MIXED_INJECTION_JSON")"
SOURCE_INTEGRITY_PASSED=$([[ "$SOURCE_DIRTY" == false ]] && echo true || echo false)
OVERALL_PASSED=$([[ "$STATIC_PASSED" == true && "$DYNAMIC_PASSED" == true && "$MIXED_PASSED" == true && "$SOURCE_INTEGRITY_PASSED" == true ]] && echo true || echo false)

RESULT="$(jq -n \
  --arg head "$SOURCE_HEAD" \
  --arg generated_at "$GENERATED_AT" \
  --arg adr "$ADR_PATH" \
  --argjson source_dirty "$SOURCE_DIRTY" \
  --argjson source_dirty_scope "$SOURCE_DIRTY_SCOPE_JSON" \
  --argjson source_dirty_entries "$SOURCE_DIRTY_ENTRIES_JSON" \
  --argjson source_integrity_passed "$SOURCE_INTEGRITY_PASSED" \
  --argjson dynamic "$DYNAMIC_JSON" \
  --argjson static "$STATIC_JSON" \
  --argjson mixed "$MIXED_INJECTION_JSON" \
  --argjson passed "$OVERALL_PASSED" \
  '{
    schema_version: "sylvode.flow.multi-document-result.v1",
    source_head: $head,
    source_dirty: $source_dirty,
    source_integrity: {
      status: (if $source_integrity_passed then "passed" else "failed" end),
      checked_scope: $source_dirty_scope,
      dirty_entries: $source_dirty_entries,
      reason: (if $source_integrity_passed then null else "validated source differs from source_head; gate artifacts require one clean HEAD" end),
      passed: $source_integrity_passed
    },
    generated_at: $generated_at,
    adr: $adr,
    dynamic_tests: $dynamic,
    r17_static_assertions: $static,
    mixed_move_grant_subscription_content_injection: $mixed,
    multi_document_lock_order_and_atomicity: {
      status: (if $passed then "passed" else "failed" end),
      passed: $passed
    },
    passed: $passed
  }')"

OUT_PATH="$EVIDENCE_ROOT/multi-document-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$RESULT" | jq . >"$OUT_TMP"
sync "$OUT_TMP" 2>/dev/null || true
mv -f "$OUT_TMP" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2

printf '%s\n' "$RESULT"
if [[ "$OVERALL_PASSED" == true ]]; then
  exit 0
fi
exit 1
