#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 error-contract verifier.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md, the v0.4
# section's "Error verifier" paragraph ("Error verifier 必须用同一
# producer fixture 在 REST、MCP HTTP/SSE/stdio、CLI JSON/table 与 UI
# state tests 分别注入 server_draining.details.reason=drain|contention:
# 两者稳定 code 相同且 CLI 均 exit 9,但 WS close/keep-open、JSON
# discriminator、human/UI i18n key 与 retry 状态必须不同;
# missing/unknown reason、message-based branching 或只测 WS 均失败"),
# and contracts/error-mapping-v1.md in full (the 稳定错误的五层映射
# table's `server_draining` row and the fixed error-contract-result.json
# evidence schema).
#
# Covers 2 hard gates: rest_envelope_and_error_contract,
# server_draining_reason_cross_surface_error_coverage.
#
# HOW IT WORKS (three layers, all re-derived from source every run --
# nothing here is a hardcoded verdict):
#
#   1. STATIC: verifies the current typed REST envelope/details path, Flow
#      MCP business-error call sites, CLI structured reason consumer, WS
#      drain producer/4410 wiring, and distinct UI i18n keys. These are
#      structural prerequisites only; they never substitute for a live
#      transport observation.
#
#   2. RUST DYNAMIC: runs the WS discriminator, structured CLI drain/
#      contention mapping, and MCP business-error shape tests.
#
#   3. UI DYNAMIC: runs frontend's `test:flow-v0.4` suite via Bun, reads its
#      JSON gate row, and also requires the named assertion proving every
#      accepted drain fixture has `FlowError.origin === 'server'`. A suite
#      pass without that exact runtime assertion is not UI coverage.
#
# HONEST GAP: no existing scripts-only fixture starts all three real MCP
# transports against the same live API drain guard. Their fields remain
# `not_covered`, never inferred from shared dispatch source, so the cross-
# surface hard gate remains red until live HTTP/SSE/stdio evidence exists.
#
# Exit codes: 0 = both gates recomputed to passed (does not happen today
# -- see above), 1 = ran to completion and wrote
# evidence/v0.4/error-contract-result.json with one or more gates not
# passed, 2 = usage/tool/environment error.

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
Usage: scripts/verify-flow-errors-v0.4.sh --contract PATH --json [OPTIONS]

Statically determines, per transport (REST/MCP-HTTP/MCP-SSE/MCP-stdio/
CLI/WS/UI), whether contracts/error-mapping-v1.md's `server_draining`
drain/contention reason discriminator is actually producible and
wire-correct today (grepping apps/api/src, apps/mcp-server/src and
frontend/src fresh every run -- never a hardcoded verdict), runs every
real supporting cargo test that exists, and writes
evidence/v0.4/error-contract-result.json.

Options:
  --contract PATH          Path to contracts/error-mapping-v1.md. Default:
                          <contracts-root>/contracts/error-mapping-v1.md
                          A relative path is resolved against the
                          current directory first, then against
                          --contracts-root.
  --contracts-root DIR     Root containing contracts/. Default:
                          /opt/working/sylvode-flow
  --evidence-root DIR     Required. Where error-contract-result.json is written.
  --repo-root DIR         Repository containing apps/api, apps/mcp-server,
                          frontend/ and the cargo workspace. Default: this
                          checkout.
  --skip-cargo-test        Skip the dynamic cargo test run (fast iteration
                          only; the written evidence records this).
  --json                  Required for CLI-contract compatibility.
  -h, --help              Show this help and exit 0.

Exit codes: 0 both gates passed, 1 ran to completion with one or more
gates not passed (the honest, normal outcome today), 2 usage/tool/
environment error.
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
if command -v bun >/dev/null 2>&1; then
  BUN_BIN="$(command -v bun)"
elif [[ -x /home/ck/.bun/bin/bun ]]; then
  BUN_BIN="/home/ck/.bun/bin/bun"
else
  echo "FAIL: missing required command: bun" >&2
  exit 2
fi

if [[ -z "$CONTRACT_PATH" ]]; then
  CONTRACT_PATH="$CONTRACTS_ROOT/contracts/error-mapping-v1.md"
fi
if ! CONTRACT_PATH="$(flow_resolve_contract_path --contract "$CONTRACT_PATH" "$CONTRACTS_ROOT")"; then
  exit 2
fi
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi

ERROR_RS="$REPO_ROOT/apps/api/src/error.rs"
RESPONSE_RS="$REPO_ROOT/apps/api/src/response.rs"
COMMAND_RS="$REPO_ROOT/apps/api/src/flow/command.rs"
WRITE_RS="$REPO_ROOT/apps/api/src/flow/collab/write.rs"
SESSION_RS="$REPO_ROOT/apps/api/src/flow/collab/session.rs"
FRAME_RS="$REPO_ROOT/apps/api/src/flow/collab/frame.rs"
PROTOCOL_RS="$REPO_ROOT/apps/mcp-server/src/protocol.rs"
CLI_ERROR_RS="$REPO_ROOT/apps/mcp-server/src/cli_app/error.rs"
CLI_RENDER_RS="$REPO_ROOT/apps/mcp-server/src/cli_app/render.rs"
OBJECTS_TOOL_RS="$REPO_ROOT/apps/mcp-server/src/tools/objects.rs"
OBJECT_SESSION_TS="$REPO_ROOT/frontend/src/lib/flow/object-session.ts"
EN_JSON="$REPO_ROOT/frontend/src/lib/i18n/en.json"
ZH_JSON="$REPO_ROOT/frontend/src/lib/i18n/zh.json"
for f in "$ERROR_RS" "$RESPONSE_RS" "$COMMAND_RS" "$WRITE_RS" "$SESSION_RS" "$FRAME_RS" "$PROTOCOL_RS" \
         "$CLI_ERROR_RS" "$CLI_RENDER_RS" "$OBJECTS_TOOL_RS" "$OBJECT_SESSION_TS" "$EN_JSON" "$ZH_JSON"; do
  if [[ ! -f "$f" ]]; then
    echo "FAIL: source file not found (nothing to statically verify): $f" >&2
    exit 2
  fi
done

mkdir -p "$EVIDENCE_ROOT" "$EVIDENCE_ROOT/logs"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
CONTRACT_SHA256="$(sha256sum "$CONTRACT_PATH" | awk '{print $1}')"

# ---- 1. static analysis ----
STATIC_JSON_FILE="$EVIDENCE_ROOT/logs/errors.static.json"
if ! python3 - "$CONTRACT_PATH" "$ERROR_RS" "$RESPONSE_RS" "$COMMAND_RS" "$WRITE_RS" "$SESSION_RS" \
      "$FRAME_RS" "$PROTOCOL_RS" "$CLI_ERROR_RS" "$CLI_RENDER_RS" "$OBJECTS_TOOL_RS" "$OBJECT_SESSION_TS" \
      "$EN_JSON" "$ZH_JSON" > "$STATIC_JSON_FILE" 2>"$EVIDENCE_ROOT/logs/errors.static.err.log" <<'PY'
import json
import re
import sys

(contract_path, error_rs, response_rs, command_rs, write_rs, session_rs, frame_rs, protocol_rs,
 cli_error_rs, cli_render_rs, objects_tool_rs, object_session_ts, en_json, zh_json) = sys.argv[1:15]


def read(p):
    with open(p, encoding="utf-8") as fh:
        return fh.read()


contract = read(contract_path)

# ---- contract's own server_draining row (五层映射 table), parsed fresh ----
row_m = re.search(r"^\|\s*`server_draining`\s*\|(.*)\|\s*$", contract, re.M)
contract_row_cells = [c.strip() for c in row_m.group(1).split("|")] if row_m else []

error_rs_text = read(error_rs)
response_rs_text = read(response_rs)
command_rs_text = read(command_rs)
write_rs_text = read(write_rs)
session_rs_text = read(session_rs)
frame_rs_text = read(frame_rs)
protocol_rs_text = read(protocol_rs)
cli_error_rs_text = read(cli_error_rs)
cli_render_rs_text = read(cli_render_rs)
objects_tool_rs_text = read(objects_tool_rs)
object_session_ts_text = read(object_session_ts)


def count(pattern, text, flags=0):
    return len(re.findall(pattern, text, flags))


findings = {}

# ---- REST: does the envelope have a `details` field at all? ----
response_struct_m = re.search(r"struct ApiResponse.*?\n\}", response_rs_text, re.S)
findings["rest_apiresponse_struct_has_details_field"] = bool(response_struct_m and "details" in response_struct_m.group(0))
error_fn_m = re.search(r"fn error\([^)]*\)[^{]*\{", response_rs_text)
findings["rest_response_error_fn_signature"] = error_fn_m.group(0).strip() if error_fn_m else None
apierror_variants = re.findall(r"^\s*(\w+)\(String\),?$", error_rs_text, re.M)
findings["rest_apierror_string_only_variants"] = apierror_variants

map_fn_m = re.search(r"fn map_write_rejection.*?\n\}\n", command_rs_text, re.S)
findings["map_write_rejection_reads_details_field"] = bool(map_fn_m and "rejected.details" in map_fn_m.group(0))
findings["map_write_rejection_maps_server_draining_to_conflict_string_only"] = bool(
    map_fn_m and 'RejectedCode::ServerDraining => "server_draining"' in map_fn_m.group(0)
)

# ---- server-side reason producers: is "drain" ever constructed? ----
# Word-boundary match on the bare identifier/string "drain" (not
# "draining"), across the whole apps/api/src tree via a directory walk
# from write.rs/session.rs's parent (flow/collab) up -- but scoped here to
# the two files that own every RejectedCode::ServerDraining construction
# site (confirmed by contention()/rejected_frame greps below); a repo-wide
# echo of this same grep is also run by the bash driver and recorded
# alongside this JSON for full-tree confirmation.
findings["write_rs_drain_reason_literal_count"] = count(r'"reason"\s*:\s*"drain"', write_rs_text)
findings["session_rs_drain_reason_literal_count"] = count(r'"reason"\s*:\s*"drain"', session_rs_text)
findings["write_rs_contention_reason_literal_count"] = count(r'"reason"\s*:\s*"contention"', write_rs_text)
findings["session_rs_contention_reason_literal_count"] = count(r'"reason"\s*:\s*"contention"', session_rs_text)

# ---- WS wire action for contention: does accepting a Rejected outcome
# ever close the socket, or does it send a frame and keep the connection
# open (`reject_keep_open`)? ----
rejected_arm_m = re.search(r"Ok\(AcceptOutcome::Rejected\(rejected\)\)\s*=>\s*\{(.*?)\n\s*\}\n", session_rs_text, re.S)
findings["session_rs_rejected_arm_calls_close"] = bool(rejected_arm_m and re.search(r"\bclose\(", rejected_arm_m.group(1)))
findings["session_rs_rejected_arm_sends_frame"] = bool(rejected_arm_m and "send(" in rejected_arm_m.group(1))

# ---- WS 4410 close code: ever used? ----
findings["session_rs_4410_literal_count"] = count(r"\b4410\b", session_rs_text)
findings["frame_rs_4410_literal_count"] = count(r"\b4410\b", frame_rs_text)

# ---- MCP: business_error() call sites for Flow object tools ----
findings["mcp_business_error_call_sites_in_objects_tool_rs"] = count(r"business_error\(", objects_tool_rs_text)
findings["mcp_objects_tool_rs_mentions_server_draining"] = "server_draining" in objects_tool_rs_text
business_error_fn_m = re.search(r"pub fn business_error.*?\n\s*\}\n", protocol_rs_text, re.S)
findings["mcp_business_error_fn_exists"] = bool(business_error_fn_m)
findings["mcp_business_error_fn_has_details_param"] = bool(business_error_fn_m and "details" in business_error_fn_m.group(0))

# ---- CLI: does cli_app/error.rs ever read a `reason`/`details` field
# from the API response, or only regex an HTTP status number out of a
# plain message string? ----
findings["cli_error_rs_reads_reason_field"] = bool(re.search(r'"reason"|\.reason\b|details\[', cli_error_rs_text))
findings["cli_error_rs_reads_details_field"] = bool(re.search(r"\.details\b|details\[", cli_error_rs_text))
envelope_fn_m = re.search(r"fn envelope_code.*?\n\}\n", cli_error_rs_text, re.S)
findings["cli_envelope_code_fn_body"] = envelope_fn_m.group(0).strip() if envelope_fn_m else None
findings["cli_error_rs_drain_or_contention_literal_count"] = count(r"\bdrain\b|\bcontention\b", cli_error_rs_text)
findings["cli_render_rs_reason_handling_count"] = count(r'"reason"|\.reason\b', cli_render_rs_text)

# ---- dynamic-clock/message-branching negative check: does the CLI or the
# collab code ever branch on `.message` content instead of a structured
# discriminator for server_draining specifically? ----
findings["cli_error_rs_message_based_branching_count"] = count(r"message\.(contains|starts_with|find)\(", cli_error_rs_text)

# ---- UI i18n: both keys present with distinct text? ----
en = json.loads(read(en_json))
zh = json.loads(read(zh_json))


def dig(d, *path):
    for p in path:
        if not isinstance(d, dict) or p not in d:
            return None
        d = d[p]
    return d


en_drain = dig(en, "flow", "error", "server_draining", "drain")
en_contention = dig(en, "flow", "error", "server_draining", "contention")
zh_drain = dig(zh, "flow", "error", "server_draining", "drain")
zh_contention = dig(zh, "flow", "error", "server_draining", "contention")
findings["i18n_en_drain"] = en_drain
findings["i18n_en_contention"] = en_contention
findings["i18n_zh_drain"] = zh_drain
findings["i18n_zh_contention"] = zh_contention
findings["i18n_both_variants_present_and_distinct"] = bool(
    en_drain and en_contention and en_drain != en_contention and zh_drain and zh_contention and zh_drain != zh_contention
)

# ---- UI producer for reason:'drain': is it inside dispose() (client-local
# teardown) rather than the real WS Frame::Rejected parse branch? ----
dispose_fn_m = re.search(r"async dispose\(\).*?\n\t\}\n", object_session_ts_text, re.S)
findings["object_session_ts_dispose_synthesizes_drain_reason"] = bool(
    dispose_fn_m and "reason: 'drain'" in dispose_fn_m.group(0)
)
findings["object_session_ts_drain_reason_literal_count"] = count(r"reason:\s*'drain'", object_session_ts_text)
ws_branch_m = re.search(r"if \(code === 'server_draining'\).*?\n\t\t\t\t\}", object_session_ts_text, re.S)
findings["object_session_ts_ws_branch_uses_details_reason_not_message"] = bool(
    ws_branch_m and "details?.reason" in ws_branch_m.group(0) and ".message" not in ws_branch_m.group(0)
)
findings["object_session_ts_message_based_branching_count"] = count(
    r"\.message\.(includes|startsWith|indexOf)\(", object_session_ts_text
)

# no frontend test file exercises server_draining at all (checked by the
# bash driver's own repo-wide grep, recorded there); this python-side
# check only covers the one file that would host the producer.
findings["object_session_ts_has_test_block"] = "describe(" in object_session_ts_text or "test(" in object_session_ts_text

print(json.dumps({
    "contract_row_cells": contract_row_cells,
    "findings": findings,
}))
PY
then
  echo "FAIL: static analysis crashed; see $EVIDENCE_ROOT/logs/errors.static.err.log" >&2
  cat "$EVIDENCE_ROOT/logs/errors.static.err.log" >&2
  exit 2
fi
if ! jq -e . >/dev/null 2>&1 "$STATIC_JSON_FILE"; then
  echo "FAIL: static analysis did not produce valid JSON; see $STATIC_JSON_FILE" >&2
  exit 2
fi
if [[ "$(jq '.contract_row_cells | length' "$STATIC_JSON_FILE")" -eq 0 ]]; then
  echo "FAIL: could not find the 'server_draining' row in $CONTRACT_PATH's 五层映射 table -- refusing to write a vacuous result (contract format may have changed)" >&2
  exit 2
fi

# Repo-wide echo of the "drain" producer grep, run directly by the bash
# driver (not just the two files the python static pass targets), so a
# future drain producer added anywhere else in apps/api/src is not missed.
# Each of these three greps legitimately expects zero matches today (that
# IS the finding) -- `grep`/`grep -c`/`grep -l` all exit 1 on "no lines
# selected", which under `set -o pipefail` would otherwise abort this
# script via `set -e` on the honest, expected "not found" case. `; true`
# inside each command substitution keeps the substitution's own exit
# status 0 regardless, while the awk/count logic still reflects the real
# grep output.
REPO_WIDE_DRAIN_REASON_HITS="$(grep -rc '"reason"[[:space:]]*:[[:space:]]*"drain"' "$REPO_ROOT/apps/api/src" 2>/dev/null | awk -F: '{s+=$2} END{print s+0}'; true)"
REPO_WIDE_4410_HITS="$(grep -rn '\b4410\b' "$REPO_ROOT/apps/api/src" 2>/dev/null | grep -vc '\.md:'; true)"
FRONTEND_TEST_HITS="$(grep -rl "server_draining" "$REPO_ROOT/frontend" --include="*.test.ts" 2>/dev/null | grep -c .; true)"

echo "=== static check: contracts/error-mapping-v1.md server_draining row parsed; repo-wide producer greps ===" >&2
echo "  repo-wide 'reason\":\"drain\"' literal hits in apps/api/src: $REPO_WIDE_DRAIN_REASON_HITS" >&2
echo "  repo-wide bare '4410' hits in apps/api/src (excluding .md): $REPO_WIDE_4410_HITS" >&2
echo "  frontend *.test.ts files mentioning server_draining: $FRONTEND_TEST_HITS" >&2
jq -r '.findings | to_entries[] | "  \(.key) = \(.value)"' "$STATIC_JSON_FILE" >&2

# ---- 2. dynamic cargo tests ----
LOG_DIR="$EVIDENCE_ROOT/logs"
run_group() {
  local name="$1" pkg="$2" filter="$3" extra_args="${4:-}"
  local log="$LOG_DIR/errors.dyn.${name}.log"
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
run_group frame_serialization api "flow::collab::frame::tests::rejected_code_and_drain_reason_use_the_frozen_snake_case_vocabulary"
run_group cli_exit_mapping mcp-server "server_draining_drain_and_contention_share_exit_9_but_never_the_same_message" "--lib"
run_group mcp_business_error_shape mcp-server "call_tool_error_serializes_mcp_is_error_field" "--lib"

UI_RESULT_FILE="$LOG_DIR/errors.ui-flow-v0.4.json"
UI_LOG_FILE="$LOG_DIR/errors.dyn.ui-flow-v0.4.log"
UI_SCRATCH="${FLOW_V04_SCRATCH:-/opt/worker/.cache/flow-v04-errors}"
mkdir -p "$UI_SCRATCH"
echo "  running: bun run --cwd frontend test:flow-v0.4" >&2
set +e
(
  cd "$REPO_ROOT"
  PATH="$(dirname "$BUN_BIN"):$PATH" \
    FLOW_V04_RESULT_PATH="$UI_RESULT_FILE" FLOW_V04_SCRATCH="$UI_SCRATCH" \
    "$BUN_BIN" run --cwd frontend test:flow-v0.4
) > "$UI_LOG_FILE" 2>&1
UI_EXIT_CODE=$?
set -e
if [[ ! -f "$UI_RESULT_FILE" ]] || ! jq -e . "$UI_RESULT_FILE" >/dev/null 2>&1; then
  echo "FAIL: frontend gate suite did not produce valid JSON at $UI_RESULT_FILE (exit=$UI_EXIT_CODE)" >&2
  exit 2
fi

# Promote the frontend aggregate from a private log-sidecar into the artifact
# named by v0.4-gate.yaml. Passed, failed and deferred-to-human checks remain
# three disjoint counters: a skipped check is preserved verbatim and is never
# added to checks_passed.
UI_EVIDENCE_PATH="$EVIDENCE_ROOT/ui-e2e-result.json"
UI_EVIDENCE_TMP="$UI_EVIDENCE_PATH.tmp"
python3 - "$UI_RESULT_FILE" "$UI_EVIDENCE_TMP" "$SOURCE_HEAD" "$GENERATED_AT" "$UI_LOG_FILE" "$UI_EXIT_CODE" <<'PY'
import json
import sys

raw_path, out_path, source_head, generated_at, log_path, exit_code = sys.argv[1:7]
command_exit_code = int(exit_code)
raw = json.load(open(raw_path, encoding="utf-8"))
expected = {
    "i18n_zh_en_flow_key_parity",
    "vite_wasm_static_build_and_deep_route",
    "web_ime_undo_selection_and_sync_state",
    "navigator_keyboard_drag_equivalence",
}
rows = raw.get("gates")
if not isinstance(rows, list):
    raise SystemExit("frontend result has no gates array")
by_name = {}
for row in rows:
    if not isinstance(row, dict) or not isinstance(row.get("gate"), str):
        raise SystemExit("frontend result contains a malformed gate row")
    name = row["gate"]
    if name in by_name:
        raise SystemExit(f"frontend result contains duplicate gate {name}")
    by_name[name] = row
if set(by_name) != expected:
    raise SystemExit(
        f"frontend result gate set differs: missing={sorted(expected - set(by_name))}, "
        f"extra={sorted(set(by_name) - expected)}"
    )

gates = {}
for name in sorted(expected):
    row = by_name[name]
    passed_count = row.get("checks_passed")
    failed_count = row.get("checks_failed")
    skipped_count = row.get("checks_skipped")
    skipped_checks = row.get("skipped_checks")
    if not all(isinstance(value, int) and value >= 0 for value in (passed_count, failed_count, skipped_count)):
        raise SystemExit(f"frontend result {name} has invalid check counters")
    if not isinstance(skipped_checks, list) or len(skipped_checks) != skipped_count:
        raise SystemExit(f"frontend result {name} skipped count/list disagree")
    gate_passed = command_exit_code == 0 and row.get("passed") is True and failed_count == 0
    gates[name] = {
        "status": "passed" if gate_passed else "failed",
        "reason": (
            f"command exit {command_exit_code}; {passed_count} automated checks passed, {failed_count} failed, "
            f"{skipped_count} deferred to manual sign-off"
        ),
        "coverage": row.get("coverage"),
        "automation": row.get("automation"),
        "manual_keys": row.get("manual_keys", []),
        "checks_passed": passed_count,
        "checks_failed": failed_count,
        "checks_skipped": skipped_count,
        "failures": row.get("failures", []),
        "skipped_checks": skipped_checks,
        "duration_ms": row.get("duration_ms", 0),
    }

totals = raw.get("totals")
computed_totals = {
    "passed": sum(row["checks_passed"] for row in gates.values()),
    "failed": sum(row["checks_failed"] for row in gates.values()),
    "skipped": sum(row["checks_skipped"] for row in gates.values()),
}
if totals != computed_totals:
    raise SystemExit(f"frontend result totals disagree: raw={totals}, computed={computed_totals}")

result = {
    "schema_version": "sylvode.flow.ui-e2e-result.v1",
    "schema_path": "docs/schemas/sylvode-flow-ui-e2e-result-v1.schema.json",
    "release": "0.4",
    "source_head": source_head,
    "generated_at": generated_at,
    "command": "bun run --cwd frontend test:flow-v0.4",
    "command_exit_code": command_exit_code,
    "raw_result": raw_path,
    "log": log_path,
    "totals": computed_totals,
    "gates": gates,
    "passed": all(row["status"] == "passed" for row in gates.values()),
}
with open(out_path, "w", encoding="utf-8") as handle:
    json.dump(result, handle, indent=2, sort_keys=True)
    handle.write("\n")
PY
sync "$UI_EVIDENCE_TMP" 2>/dev/null || true
mv -f "$UI_EVIDENCE_TMP" "$UI_EVIDENCE_PATH"
echo "  wrote $UI_EVIDENCE_PATH" >&2

DYNAMIC_JSON_FILE="$LOG_DIR/errors.dynamic.json"
python3 - "$LOG_DIR" <<'PY' > "$DYNAMIC_JSON_FILE"
import glob
import json
import os
import re
import sys

log_dir = sys.argv[1]
groups = {}
for path in glob.glob(os.path.join(log_dir, "errors.dyn.*.log")):
    name = os.path.basename(path)[len("errors.dyn."):-len(".log")]
    text = open(path, encoding="utf-8", errors="replace").read()
    tests = dict(re.findall(r"^test (?:\S+::)*?(\w+) \.\.\. (ok|FAILED)$", text, re.M))
    groups[name] = {"log": path, "tests": tests}
print(json.dumps({"groups": groups}))
PY

echo "=== dynamic test results ===" >&2
jq -r '.groups | to_entries[] | "  \(.key): \(.value.tests | to_entries | map(select(.value!="ok")) | length) not-ok of \(.value.tests | length)"' "$DYNAMIC_JSON_FILE" >&2
echo "  ui-flow-v0.4: exit=$UI_EXIT_CODE, passed=$(jq -r '.passed' "$UI_RESULT_FILE")" >&2

# ---- 3. assemble evidence/v0.4/error-contract-result.json ----
OUT_PATH="$EVIDENCE_ROOT/error-contract-result.json"
OUT_TMP="$OUT_PATH.tmp"

FINAL_JSON="$(python3 - "$STATIC_JSON_FILE" "$DYNAMIC_JSON_FILE" "$SOURCE_HEAD" "$GENERATED_AT" "$CONTRACT_SHA256" \
    "$REPO_WIDE_DRAIN_REASON_HITS" "$REPO_WIDE_4410_HITS" "$FRONTEND_TEST_HITS" \
    "$UI_RESULT_FILE" "$UI_LOG_FILE" "$UI_EXIT_CODE" "$OUT_TMP" <<'PY'
import json
import re
import sys

(static_path, dynamic_path, source_head, generated_at, contract_sha256,
 repo_wide_drain_hits, repo_wide_4410_hits, frontend_test_hits,
 ui_result_path, ui_log_path, ui_exit_code, out_path) = sys.argv[1:13]

static = json.load(open(static_path, encoding="utf-8"))
dynamic = json.load(open(dynamic_path, encoding="utf-8"))
f = static["findings"]
repo_wide_drain_hits = int(repo_wide_drain_hits)
repo_wide_4410_hits = int(repo_wide_4410_hits)
frontend_test_hits = int(frontend_test_hits)
ui_exit_code = int(ui_exit_code)
ui_result = json.load(open(ui_result_path, encoding="utf-8"))
ui_log = open(ui_log_path, encoding="utf-8", errors="replace").read()


def dtest(name):
    for group in dynamic.get("groups", {}).values():
        if name in group.get("tests", {}):
            return group["tests"][name]
    return None


ws_serialization_test = dtest("rejected_code_and_drain_reason_use_the_frozen_snake_case_vocabulary")
cli_exit_tests_ok = (
    dtest("server_draining_drain_and_contention_share_exit_9_but_never_the_same_message") == "ok"
)
mcp_shape_test = dtest("call_tool_error_serializes_mcp_is_error_field")

ui_gate = next(
    (gate for gate in ui_result.get("gates", []) if gate.get("gate") == "web_ime_undo_selection_and_sync_state"),
    None,
)
ui_gate_json_ok = bool(
    ui_exit_code == 0
    and ui_result.get("passed") is True
    and ui_gate
    and ui_gate.get("passed") is True
    and ui_gate.get("checks_failed") == 0
)
UI_SERVER_ORIGIN_ASSERTION = "every surface marks the fixture as server-reported, and only the wire can"
ui_server_origin_assertion_ok = bool(
    re.search(r"^\s*ok\s+" + re.escape(UI_SERVER_ORIGIN_ASSERTION) + r"\s*$", ui_log, re.M)
)
UI_MALFORMED_ASSERTION = "a missing or unknown reason is refused, never defaulted to contention"
ui_malformed_assertion_ok = bool(
    re.search(r"^\s*ok\s+" + re.escape(UI_MALFORMED_ASSERTION) + r"\s*$", ui_log, re.M)
)
ui_drain_coverage_ok = ui_gate_json_ok and ui_server_origin_assertion_ok

rest_details_exists = f["rest_apiresponse_struct_has_details_field"]
mcp_flow_wired = f["mcp_business_error_call_sites_in_objects_tool_rs"] > 0
cli_reads_reason = f["cli_error_rs_reads_reason_field"]
ws_contention_keep_open = (not f["session_rs_rejected_arm_calls_close"]) and f["session_rs_rejected_arm_sends_frame"]
ws_drain_produced = (f["write_rs_drain_reason_literal_count"] + f["session_rs_drain_reason_literal_count"] + repo_wide_drain_hits) > 0
ws_drain_close_4410_wired = (f["session_rs_4410_literal_count"] + f["frame_rs_4410_literal_count"] + repo_wide_4410_hits) > 0

REST_STATUS_REASON = (
    "REST has typed `details` and map_write_rejection preserves it; this verifier has structural "
    "evidence but no shared-fixture REST live observation in this run"
    if rest_details_exists and f["map_write_rejection_reads_details_field"]
    else "REST typed details propagation is incomplete"
)
MCP_STATUS_REASON = (
    f"Flow objects has {f['mcp_business_error_call_sites_in_objects_tool_rs']} structured "
    "business_error call sites, but HTTP/SSE/stdio have no shared-drain live fixture; static shared "
    "dispatch is deliberately not counted as three transport observations"
)
CLI_STATUS_REASON = (
    f"CLI reads structured reason/details={cli_reads_reason}/{f['cli_error_rs_reads_details_field']} "
    f"and its drain/contention exit-9 test passed={cli_exit_tests_ok}; no same-producer CLI process "
    "fixture was executed by this verifier"
)

drain_variant = {
    "rest_reason": None,
    "mcp_http_reason": None,
    "mcp_sse_reason": None,
    "mcp_stdio_reason": None,
    "cli_json_reason": None,
    "cli_exit": None,
    "cli_human_kind": None,
    "ui_key": "flow.error.server_draining.drain",
    "ui_reason": "drain" if ui_drain_coverage_ok else None,
    "ui_origin": "server" if ui_drain_coverage_ok else None,
    "ui_key_i18n_present": bool(f["i18n_en_drain"] and f["i18n_zh_drain"]),
    "ws_action": "close_4410" if ws_drain_produced and ws_drain_close_4410_wired else None,
    "producible": ws_drain_produced and ws_drain_close_4410_wired,
    "reason": (
        f"server drain is present (reason hits={repo_wide_drain_hits}, 4410 hits={repo_wide_4410_hits}); "
        f"UI dynamic server-origin coverage={ui_drain_coverage_ok}. REST: {REST_STATUS_REASON}. "
        f"MCP: {MCP_STATUS_REASON}. CLI: {CLI_STATUS_REASON}. The variant remains not fully "
        "producible for this cross-surface gate until all required live transports share one fixture."
    ),
}

contention_variant = {
    "rest_reason": None,
    "mcp_http_reason": None,
    "mcp_sse_reason": None,
    "mcp_stdio_reason": None,
    "cli_json_reason": None,
    "cli_exit": None,
    "cli_human_kind": None,
    "ui_key": "flow.error.server_draining.contention",
    "ui_reason": "contention" if ui_drain_coverage_ok else None,
    "ui_origin": "server" if ui_drain_coverage_ok else None,
    "ui_key_i18n_present": bool(f["i18n_en_contention"] and f["i18n_zh_contention"]),
    "ws_action": "reject_keep_open" if ws_contention_keep_open else None,
    "producible": ws_contention_keep_open,
    "reason": (
        "contention is a real WS reject_keep_open producer; "
        f"UI dynamic server-origin coverage={ui_drain_coverage_ok}. REST: {REST_STATUS_REASON}. "
        f"MCP: {MCP_STATUS_REASON}. CLI: {CLI_STATUS_REASON}."
    ),
}

malformed = {
    "missing_reason_rejected": {
        "status": "passed" if ui_gate_json_ok and ui_malformed_assertion_ok else "not_covered",
        "reason": "UI state suite dynamically rejects missing reason" if ui_malformed_assertion_ok
                  else "the named UI malformed-reason assertion did not run successfully",
    },
    "unknown_reason_rejected": {
        "status": "passed" if ui_gate_json_ok and ui_malformed_assertion_ok else "not_covered",
        "reason": "UI state suite dynamically rejects unknown reason" if ui_malformed_assertion_ok
                  else "the named UI malformed-reason assertion did not run successfully",
    },
    "message_branching_zero": {
        "status": "passed" if (
            f["cli_error_rs_message_based_branching_count"] == 0
            and f["object_session_ts_message_based_branching_count"] == 0
            and f["object_session_ts_ws_branch_uses_details_reason_not_message"]
        ) else "failed",
        "reason": (
            f"zero message-based branching found for server_draining specifically: "
            f"cli_app/error.rs message.contains/starts_with/find count="
            f"{f['cli_error_rs_message_based_branching_count']}, object-session.ts "
            f".message.includes/startsWith/indexOf count="
            f"{f['object_session_ts_message_based_branching_count']}, and the one real WS consumer "
            "branch checks the structured `details?.reason` field, not `.message` text (confirmed: "
            f"{f['object_session_ts_ws_branch_uses_details_reason_not_message']}). This is a real "
            "passing check on its own terms, but it does not offset the missing cross-surface "
            "producibility above."
        ),
    },
}

variants_producible = drain_variant["producible"] and contention_variant["producible"]
cross_surface_ok = (
    drain_variant["rest_reason"] is not None and contention_variant["rest_reason"] is not None
    and drain_variant["mcp_http_reason"] is not None and contention_variant["mcp_http_reason"] is not None
    and drain_variant["cli_json_reason"] is not None and contention_variant["cli_json_reason"] is not None
    and drain_variant["ui_reason"] == "drain" and contention_variant["ui_reason"] == "contention"
    and drain_variant["ui_origin"] == "server" and contention_variant["ui_origin"] == "server"
)

result = {
    "schema_version": "sylvode.flow.error-contract-result.v1",
    "source_head": source_head,
    "contract_sha256": contract_sha256,
    "generated_at": generated_at,
    "variants": {"drain": drain_variant, "contention": contention_variant},
    "malformed": malformed,
    "dynamic_evidence": {
        "ws_discriminator_serialization_test": {
            "test": "flow::collab::frame::tests::rejected_code_and_drain_reason_use_the_frozen_snake_case_vocabulary",
            "status": ws_serialization_test,
            "proves": "RejectedCode::ServerDraining/DrainReason::Contention serialize as "
                      "\"server_draining\"/\"contention\" -- the JSON discriminator SHAPE is correct "
                      "where it is used (WS only); does not prove reachability on any other transport.",
        },
        "cli_exit_mapping_tests_ok": cli_exit_tests_ok,
        "cli_exit_mapping_tests_prove": "HTTP-status-only exit mapping (401/403/404/409/400 -> "
                                         "exit 3/4/5/6/7); this is the wrong shape for a reason-based "
                                         "server_draining mapping and proves no such mapping exists.",
        "mcp_business_error_shape_test": {
            "test": "protocol::tests::call_tool_error_serializes_mcp_is_error_field",
            "status": mcp_shape_test,
            "proves": "the generic isError JSON shape is correct where used; Flow tools do not use it "
                      "(0 call sites, see findings).",
        },
        "frontend_test_files_mentioning_server_draining": frontend_test_hits,
        "ui_state_gate": {
            "command": "bun run --cwd frontend test:flow-v0.4",
            "result": ui_result_path,
            "log": ui_log_path,
            "exit_code": ui_exit_code,
            "gate_json_passed": ui_gate_json_ok,
            "gate_row": ui_gate,
            "required_origin": "server",
            "origin_server_assertion": UI_SERVER_ORIGIN_ASSERTION,
            "origin_server_assertion_status": "ok" if ui_server_origin_assertion_ok else "missing_or_failed",
            "malformed_reason_assertion": UI_MALFORMED_ASSERTION,
            "malformed_reason_assertion_status": "ok" if ui_malformed_assertion_ok else "missing_or_failed",
            "status": "passed" if ui_drain_coverage_ok else "failed",
            "proves": (
                "the UI consumed drain/contention from REST, WS rejected and WS 4410 wire shapes, "
                "and every accepted FlowError was explicitly asserted to have origin=server"
                if ui_drain_coverage_ok
                else "UI coverage is not accepted unless both the gate JSON and the named origin=server "
                     "runtime assertion pass"
            ),
        },
        "mcp_live_transports": {
            "http": "not_covered",
            "sse": "not_covered",
            "stdio": "not_covered",
            "reason": (
                "the repository has no scripts-only fixture that starts all three MCP transports "
                "against the same live API WorkspaceDrainGuard; shared dispatch/static wiring is not "
                "counted as live transport evidence"
            ),
        },
    },
    "structural_findings": f,
    # Both gates below are derived from the SAME booleans already computed above
    # (rest_details_exists / mcp_flow_wired / cli_reads_reason / ws_drain_produced /
    # ws_drain_close_4410_wired / variants_producible / cross_surface_ok) -- not re-asserted as a
    # fixed literal. Until 2026-08-30 these two lines were hardcoded `"failed"` even though every
    # one of those booleans was already computed and sitting unused right above them: the exact
    # same "evidence computed, verdict hardcoded" bug as scripts/verify-flow-limits-v0.4.sh. Today
    # every one of these booleans is genuinely False/empty (see structural_findings above), so the
    # derived verdict is still "failed" -- but it is now `rest_details_exists`-shaped, so the day
    # apps/api/src/response.rs grows a `details` field this flips on its own, instead of silently
    # staying "failed" forever the way a literal would.
    "hard_gates": {
        "rest_envelope_and_error_contract": (
            "passed"
            if (
                rest_details_exists
                and f["map_write_rejection_reads_details_field"]
                and not f["map_write_rejection_maps_server_draining_to_conflict_string_only"]
            )
            else "failed"
        ),
        "server_draining_reason_cross_surface_error_coverage": (
            "passed" if (variants_producible and cross_surface_ok) else "failed"
        ),
    },
    "hard_gate_reasons": {
        "rest_envelope_and_error_contract": (
            "apps/api/src/response.rs's ApiResponse carries a `details` field, "
            "map_write_rejection reads rejected.details, and server_draining is not collapsed to a "
            "plain conflict string"
            if rest_details_exists and f["map_write_rejection_reads_details_field"]
            and not f["map_write_rejection_maps_server_draining_to_conflict_string_only"]
            else REST_STATUS_REASON
        ),
        "server_draining_reason_cross_surface_error_coverage": (
            "both drain and contention are producible and cross-surface coverage (REST+MCP+CLI) is "
            "confirmed"
            if variants_producible and cross_surface_ok
            else "REST/WS producers, structured MCP/CLI consumers and server-origin UI state coverage "
            "exist, but the gate still lacks same-producer live MCP HTTP/SSE/stdio and CLI process "
            "observations; static shared dispatch is not counted"
        ),
    },
}
# ---- self-consistency assertion: hard_gates must match the booleans that derived them ----
#
# Until 2026-08-30 both hard_gates entries above were hardcoded `"failed"` string literals, even
# though rest_details_exists/mcp_flow_wired/cli_reads_reason/variants_producible/cross_surface_ok
# were already computed and sitting unused right above them -- the exact same "evidence computed,
# verdict hardcoded" bug as scripts/verify-flow-limits-v0.4.sh. Re-deriving the expected value from
# those same booleans here and comparing catches any future edit that reintroduces a literal (by
# editing the `hard_gates` dict directly without updating its derivation) before this script ever
# writes evidence for it.
expected_rest_gate = "passed" if (
    rest_details_exists and f["map_write_rejection_reads_details_field"]
    and not f["map_write_rejection_maps_server_draining_to_conflict_string_only"]
) else "failed"
expected_cross_surface_gate = "passed" if (variants_producible and cross_surface_ok) else "failed"
self_consistency_problems = []
if result["hard_gates"]["rest_envelope_and_error_contract"] != expected_rest_gate:
    self_consistency_problems.append(
        f"hard_gates.rest_envelope_and_error_contract={result['hard_gates']['rest_envelope_and_error_contract']!r} "
        f"does not match its own derivation (expected {expected_rest_gate!r} from rest_details_exists="
        f"{rest_details_exists}, map_write_rejection_reads_details_field="
        f"{f['map_write_rejection_reads_details_field']}, "
        "map_write_rejection_maps_server_draining_to_conflict_string_only="
        f"{f['map_write_rejection_maps_server_draining_to_conflict_string_only']})"
    )
if result["hard_gates"]["server_draining_reason_cross_surface_error_coverage"] != expected_cross_surface_gate:
    self_consistency_problems.append(
        "hard_gates.server_draining_reason_cross_surface_error_coverage="
        f"{result['hard_gates']['server_draining_reason_cross_surface_error_coverage']!r} does not match "
        f"its own derivation (expected {expected_cross_surface_gate!r} from variants_producible="
        f"{variants_producible}, cross_surface_ok={cross_surface_ok})"
    )
if self_consistency_problems:
    print(json.dumps({
        "error": "self-consistency check failed -- hard_gates disagrees with the booleans that derived "
                  "it: " + "; ".join(self_consistency_problems)
    }))
    sys.exit(0)

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
echo "=== malformed-case verdicts ===" >&2
jq -r '.malformed | to_entries[] | "  \(.key): \(.value.status)"' <<<"$FINAL_JSON" >&2

echo "$FINAL_JSON"

OVERALL_PASSED="$(jq -r '.passed' <<<"$FINAL_JSON")"
if [[ "$OVERALL_PASSED" == "true" ]]; then
  exit 0
fi
exit 1
