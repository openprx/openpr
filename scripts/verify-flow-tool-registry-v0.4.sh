#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 MCP tool-registry count verifier.
#
# Contract: /opt/working/sylvode-flow/contracts/tool-count-baseline.md
# ("版本预期" table + "必须同步的活动面" + "自查新增的活动面") and
# gates/v0.4-gate.yaml's `tool_registry_expected_107_or_rebased`.
#
# The number this script judges comes from the LIVE registry, never from a
# markdown table: it builds and runs the shipped `list-tools` binary, which
# calls `mcp_server::get_all_tool_definitions()` -- the same function
# `tools/list` answers from on all three transports. The contract's
# expected number is then read out of tool-count-baseline.md's version
# table and compared against that live number. "or_rebased" in the gate id
# means the gate is satisfied when live == contract, whichever way the two
# were brought into agreement; it does NOT mean this script may pick
# whichever number makes the gate green. If they disagree the gate fails
# and BOTH numbers are written into the evidence -- this script never edits
# the contract to match the implementation nor the implementation to match
# the contract.
#
# tool-count-baseline.md also names the further places that hardcode the
# count. Changing the registry without changing them is exactly the drift
# the baseline document exists to prevent, so every one of them is
# re-extracted here from its own file and compared to the live count. A
# touchpoint whose number disagrees is a violation even when the registry
# itself matches the contract, and a touchpoint whose pattern no longer
# matches anything is also a violation (silently losing an assertion site
# is how the count drifts unnoticed).
#
# Additionally re-derived from source, not from a test's self-report:
#   - the sorted tool-name SHA-256 (`names_sha256`, required by
#     tool-count-baseline.md "每次变更顺序" step 10),
#   - that live tool names are unique,
#   - that TOOL_POLICY_SCOPES' declared array length, its actual entry
#     count and the live registry all agree, and that every live tool has a
#     PolicyScope entry (tool-count-baseline.md "Policy coverage test").
#
# Exit codes: 0 = live count == contract expectation and every touchpoint
# agrees, 1 = a count/policy/name violation, 2 = usage/tool/environment
# error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
BASELINE_PATH=""
RELEASE="0.4"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-tool-registry-v0.4.sh --baseline PATH --release 0.4 --json [OPTIONS]

Builds and runs the shipped `list-tools` binary to obtain the LIVE MCP tool
registry, compares its count against contracts/tool-count-baseline.md's
expected total for the release, re-extracts the count from every hardcoded
touchpoint the baseline names, and cross-checks TOOL_POLICY_SCOPES
coverage. Writes evidence/v0.4/tool-registry-result.json.

Options:
  --baseline PATH        contracts/tool-count-baseline.md. Default:
                         <contracts-root>/contracts/tool-count-baseline.md
  --release X.Y          Row of the baseline's "版本预期" table to read.
                         Default: 0.4
  --contracts-root DIR   Default: /opt/working/sylvode-flow
  --evidence-root DIR    Default: /opt/working/sylvode-flow/evidence/v0.4
  --repo-root DIR        Repository containing apps/mcp-server. Default:
                         this checkout.
  --json                 Required for CLI-contract compatibility.
  -h, --help             Show this help and exit 0.

Exit codes: 0 all agree, 1 a violation, 2 usage/tool/environment error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --baseline) BASELINE_PATH="${2:?--baseline requires a PATH argument}"; shift 2 ;;
    --release) RELEASE="${2:?--release requires a value}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ -n "$BASELINE_PATH" ]] || BASELINE_PATH="$CONTRACTS_ROOT/contracts/tool-count-baseline.md"

if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
for tool in jq git python3 cargo sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required command: $tool" >&2; exit 2; }
done
if [[ ! -f "$BASELINE_PATH" ]]; then
  echo "FAIL: baseline contract not found: $BASELINE_PATH" >&2
  exit 2
fi
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi

mkdir -p "$EVIDENCE_ROOT"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
BASELINE_SHA="$(sha256sum "$BASELINE_PATH" | awk '{print $1}')"

TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
echo "=== building list-tools (cargo build -p mcp-server --bin list-tools) ===" >&2
( cd "$REPO_ROOT" && cargo build -q -p mcp-server --bin list-tools ) || {
  echo "FAIL: list-tools binary failed to build" >&2
  exit 2
}
LIST_TOOLS_BIN="$TARGET_DIR/debug/list-tools"
if [[ ! -x "$LIST_TOOLS_BIN" ]]; then
  echo "FAIL: list-tools binary not found after build: $LIST_TOOLS_BIN" >&2
  exit 2
fi

echo "=== reading the LIVE registry from the shipped binary ===" >&2
LIVE_RAW="$("$LIST_TOOLS_BIN")"

LIVE_JSON="$(printf '%s' "$LIVE_RAW" | python3 -c '
import hashlib, json, re, sys

text = sys.stdin.read()
header = re.search(r"Available MCP Tools \((\d+) total\)", text)
# Tool names are the two-space-indented lines; description/schema lines are
# indented by three or more, so the name lines are matched exactly.
names = re.findall(r"(?m)^  ([A-Za-z][A-Za-z0-9_.]*)$", text)
uniq = sorted(set(names))
print(json.dumps({
    "declared_total": int(header.group(1)) if header else None,
    "enumerated_total": len(names),
    "unique_total": len(uniq),
    "duplicate_names": sorted({n for n in names if names.count(n) > 1}),
    "names": uniq,
    "names_sha256": hashlib.sha256("\n".join(uniq).encode()).hexdigest(),
}))
')"

jq -e . >/dev/null 2>&1 <<<"$LIVE_JSON" || { echo "FAIL: could not parse list-tools output" >&2; exit 2; }

LIVE_DECLARED="$(jq -r '.declared_total // "null"' <<<"$LIVE_JSON")"
LIVE_ENUM="$(jq -r '.enumerated_total' <<<"$LIVE_JSON")"
LIVE_UNIQUE="$(jq -r '.unique_total' <<<"$LIVE_JSON")"
NAMES_SHA="$(jq -r '.names_sha256' <<<"$LIVE_JSON")"

if [[ "$LIVE_DECLARED" == "null" ]]; then
  echo "FAIL: list-tools output carried no 'Available MCP Tools (N total)' header" >&2
  exit 2
fi

VIOLATIONS=()
[[ "$LIVE_DECLARED" == "$LIVE_ENUM" ]] || VIOLATIONS+=("list-tools header says $LIVE_DECLARED tools but $LIVE_ENUM tool names were enumerated")
[[ "$LIVE_ENUM" == "$LIVE_UNIQUE" ]] || VIOLATIONS+=("registry has duplicate tool names: $(jq -c '.duplicate_names' <<<"$LIVE_JSON")")

LIVE_COUNT="$LIVE_UNIQUE"

# ---- contract expectation from the baseline's version table ----
CONTRACT_JSON="$(python3 -c '
import json, re, sys
path, release = sys.argv[1], sys.argv[2]
text = open(path, encoding="utf-8").read()
expected = None
for row in re.finditer(r"(?m)^\|\s*([0-9]+\.[0-9]+)\s*\|[^|]*\|\s*([0-9]+)\s*\|", text):
    if row.group(1) == release:
        expected = int(row.group(2))
current = re.search(r"当前 count：\*\*(\d+)\*\*", text)
print(json.dumps({
    "expected_total_for_release": expected,
    "documented_current_count": int(current.group(1)) if current else None,
}))
' "$BASELINE_PATH" "$RELEASE")"
CONTRACT_EXPECTED="$(jq -r '.expected_total_for_release // "null"' <<<"$CONTRACT_JSON")"
if [[ "$CONTRACT_EXPECTED" == "null" ]]; then
  echo "FAIL: tool-count-baseline.md has no '版本预期' row for release $RELEASE" >&2
  exit 2
fi
if [[ "$LIVE_COUNT" != "$CONTRACT_EXPECTED" ]]; then
  VIOLATIONS+=("live registry count=$LIVE_COUNT but contracts/tool-count-baseline.md expects $CONTRACT_EXPECTED for release $RELEASE (neither side is adjusted by this verifier; reconcile deliberately)")
fi

# ---- TOOL_POLICY_SCOPES cross-check (declared length, real entries, coverage) ----
POLICY_JSON="$(python3 -c '
import json, re, sys
src = open(sys.argv[1], encoding="utf-8").read()
m = re.search(r"const TOOL_POLICY_SCOPES: \[\(&str, PolicyScope\); (\d+)\] = \[(.*?)\n\];", src, re.S)
if not m:
    print(json.dumps({"error": "TOOL_POLICY_SCOPES array not found in server.rs"}))
    sys.exit(0)
declared_len = int(m.group(1))
entries = re.findall(r"\(\s*\"([A-Za-z][A-Za-z0-9_.]*)\"\s*,\s*PolicyScope::", m.group(2))
print(json.dumps({
    "declared_len": declared_len,
    "entry_count": len(entries),
    "names": sorted(set(entries)),
    "unique_entry_count": len(set(entries)),
}))
' "$REPO_ROOT/apps/mcp-server/src/server.rs")"
if jq -e 'has("error")' >/dev/null 2>&1 <<<"$POLICY_JSON"; then
  echo "FAIL: $(jq -r '.error' <<<"$POLICY_JSON")" >&2
  exit 2
fi
POLICY_DECLARED="$(jq -r '.declared_len' <<<"$POLICY_JSON")"
POLICY_ENTRIES="$(jq -r '.entry_count' <<<"$POLICY_JSON")"
POLICY_UNIQUE="$(jq -r '.unique_entry_count' <<<"$POLICY_JSON")"
[[ "$POLICY_DECLARED" == "$POLICY_ENTRIES" ]] || VIOLATIONS+=("TOOL_POLICY_SCOPES declares length $POLICY_DECLARED but contains $POLICY_ENTRIES entries")
[[ "$POLICY_ENTRIES" == "$POLICY_UNIQUE" ]] || VIOLATIONS+=("TOOL_POLICY_SCOPES contains duplicate tool names ($POLICY_ENTRIES entries, $POLICY_UNIQUE unique)")
[[ "$POLICY_DECLARED" == "$LIVE_COUNT" ]] || VIOLATIONS+=("TOOL_POLICY_SCOPES length $POLICY_DECLARED != live registry count $LIVE_COUNT")

MISSING_SCOPES="$(jq -c -n --argjson live "$LIVE_JSON" --argjson pol "$POLICY_JSON" '$live.names - $pol.names')"
EXTRA_SCOPES="$(jq -c -n --argjson live "$LIVE_JSON" --argjson pol "$POLICY_JSON" '$pol.names - $live.names')"
[[ "$MISSING_SCOPES" == "[]" ]] || VIOLATIONS+=("live tools with no TOOL_POLICY_SCOPES entry: $MISSING_SCOPES")
[[ "$EXTRA_SCOPES" == "[]" ]] || VIOLATIONS+=("TOOL_POLICY_SCOPES names an unregistered tool: $EXTRA_SCOPES")

# ---- the hardcoded touchpoints tool-count-baseline.md names ----
# Each entry: id|relative path|python regex with one capturing group holding the count.
TOUCHPOINTS=(
  'registry_assertion|apps/mcp-server/src/tools/mod.rs|tools\.len\(\),\s*(\d+),'
  'policy_scopes_array|apps/mcp-server/src/server.rs|const TOOL_POLICY_SCOPES: \[\(&str, PolicyScope\); (\d+)\]'
  'skill_guide_heading|apps/mcp-server/src/server.rs|## Tools \((\d+)\)\n'
  'client_comment|apps/mcp-server/src/client/mod.rs|identity reaches all (\d+) tools'
  'root_readme_overview|README.md|\*\*MCP server\*\* — (\d+) tools'
  'root_readme_tools_heading|README.md|### Tools \((\d+)\)'
  'root_readme_assert|README.md|assert_eq!\(tools\.len\(\), (\d+)\)'
  'root_readme_tools_call|README.md|`tools call` reaches any of the (\d+) tools'
  'root_readme_verification|README.md|asserts the (\d+) tool count'
  'mcp_readme|apps/mcp-server/README.md|\*\*(\d+) MCP Tools\*\*'
  'mcp_agents_overview|apps/mcp-server/AGENTS.md|MCP server exposes (\d+) tools'
  'mcp_agents_regression|apps/mcp-server/AGENTS.md|test all (\d+) tools across 3 transports'
  'docs_index_server|docs/README.md|MCP server \((\d+) tools'
  'docs_index_regression|docs/README.md|(\d+)-tool registry'
  'forms_implementation_map|docs/universal-forms-implementation-map.md|(\d+)-tool MCP registry'
  'test_mcp_sh|scripts/test-mcp.sh|EXPECTED_TOOL_COUNT="\$\{EXPECTED_TOOL_COUNT:-(\d+)\}"'
  'skill_md_enumerate|skills/openpr-mcp/SKILL.md|enumerate all (\d+) tools'
  'skill_md_regression|skills/openpr-mcp/SKILL.md|checks the (\d+)-tool registry'
  'validate_mcp_sh_guard|skills/openpr-mcp/scripts/validate-mcp.sh|-eq (\d+) \]'
  'validate_mcp_sh_message|skills/openpr-mcp/scripts/validate-mcp.sh|expected exactly (\d+) tools'
  'mcp_regression_docstring|skills/openpr-mcp/scripts/mcp-regression.py|with (\d+)-tool registry checks'
  'mcp_regression_predicate|skills/openpr-mcp/scripts/mcp-regression.py|def registry_has_(\d+)_tools_with'
  'mcp_regression_len|skills/openpr-mcp/scripts/mcp-regression.py|len\(tools\) == (\d+)'
  'mcp_regression_label|skills/openpr-mcp/scripts/mcp-regression.py|tools/list\.registry_(\d+)'
  'mcp_regression_banner|skills/openpr-mcp/scripts/mcp-regression.py|\((\d+)工具注册面'
  'audit_production_readiness|scripts/audit-universal-forms-production-readiness.sh|MCP validation requires exact (\d+) tools'
  'audit_source_coverage|scripts/audit-universal-forms-source-coverage.sh|MCP registry expected count is (\d+)'
  'audit_docs|scripts/audit-universal-forms-docs.sh|MCP regression validates (\d+)-tool registry'
)

TOUCHPOINTS_JSON="[]"
for entry in "${TOUCHPOINTS[@]}"; do
  IFS='|' read -r tp_id tp_path tp_re <<<"$entry"
  abs="$REPO_ROOT/$tp_path"
  if [[ ! -f "$abs" ]]; then
    VIOLATIONS+=("touchpoint '$tp_id': file not found: $tp_path")
    TOUCHPOINTS_JSON="$(jq -c --arg id "$tp_id" --arg path "$tp_path" '. + [{id:$id, path:$path, found:false, values:[], agrees:false}]' <<<"$TOUCHPOINTS_JSON")"
    continue
  fi
  found_json="$(python3 -c '
import json, re, sys
text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
vals = sorted({int(m) for m in re.findall(sys.argv[2], text)})
print(json.dumps(vals))
' "$abs" "$tp_re")"
  n_found="$(jq 'length' <<<"$found_json")"
  agrees=false
  if [[ "$n_found" -eq 0 ]]; then
    VIOLATIONS+=("touchpoint '$tp_id' ($tp_path): no hardcoded count matched its pattern -- the assertion site was renamed or removed, so the count can now drift there unnoticed")
  elif [[ "$n_found" -gt 1 ]]; then
    VIOLATIONS+=("touchpoint '$tp_id' ($tp_path): pattern matched several different counts $found_json -- ambiguous, cannot certify")
  else
    v="$(jq -r '.[0]' <<<"$found_json")"
    if [[ "$v" == "$LIVE_COUNT" ]]; then
      agrees=true
    else
      VIOLATIONS+=("touchpoint '$tp_id' ($tp_path) hardcodes $v but the live registry has $LIVE_COUNT")
    fi
  fi
  TOUCHPOINTS_JSON="$(jq -c --arg id "$tp_id" --arg path "$tp_path" --argjson values "$found_json" --argjson agrees "$agrees" \
    '. + [{id:$id, path:$path, found:true, values:$values, agrees:$agrees}]' <<<"$TOUCHPOINTS_JSON")"
done

PASSED=$([[ ${#VIOLATIONS[@]} -eq 0 ]] && echo true || echo false)
GATE_STATUS=$([[ "$PASSED" == "true" ]] && echo passed || echo failed)
VIOLATIONS_JSON="$(printf '%s\n' "${VIOLATIONS[@]:-}" | jq -R 'select(length>0)' | jq -s '.')"

AGREEING="$(jq 'map(select(.agrees)) | length' <<<"$TOUCHPOINTS_JSON")"
TOTAL_TP="$(jq 'length' <<<"$TOUCHPOINTS_JSON")"
REASON="live registry count=$LIVE_COUNT (shipped list-tools binary); tool-count-baseline.md expects $CONTRACT_EXPECTED for release $RELEASE; $AGREEING/$TOTAL_TP hardcoded touchpoints agree; TOOL_POLICY_SCOPES len=$POLICY_DECLARED"

RESULT="$(jq -n \
  --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" --arg release "$RELEASE" \
  --arg baseline "$BASELINE_PATH" --arg baseline_sha "$BASELINE_SHA" \
  --argjson live "$LIVE_JSON" --argjson contract "$CONTRACT_JSON" --argjson policy "$POLICY_JSON" \
  --argjson touchpoints "$TOUCHPOINTS_JSON" \
  --arg names_sha "$NAMES_SHA" \
  --argjson violations "$VIOLATIONS_JSON" --argjson passed "$PASSED" \
  --arg gate_status "$GATE_STATUS" --arg reason "$REASON" \
  '{
    schema_version: "sylvode.flow.tool-registry-result.v1",
    source_head: $head,
    generated_at: $generated_at,
    release: $release,
    baseline_contract: {path: $baseline, sha256: $baseline_sha},
    live_registry: {
      source: "cargo build -p mcp-server --bin list-tools && ./list-tools (mcp_server::get_all_tool_definitions)",
      header_declared_total: $live.declared_total,
      enumerated_total: $live.enumerated_total,
      unique_total: $live.unique_total,
      duplicate_names: $live.duplicate_names,
      names_sha256: $names_sha,
      names: $live.names
    },
    contract_expectation: $contract,
    tool_policy_scopes: $policy,
    hardcoded_touchpoints: $touchpoints,
    violations: $violations,
    passed: $passed,
    gates: {
      tool_registry_expected_107_or_rebased: {status: $gate_status, reason: $reason}
    }
  }')"

OUT_PATH="$EVIDENCE_ROOT/tool-registry-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$RESULT" | jq . > "$OUT_TMP"
sync "$OUT_TMP" 2>/dev/null || true
mv -f "$OUT_TMP" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2

if [[ "$PASSED" != "true" ]]; then
  jq -r '.violations[] | "  VIOLATION: " + .' <<<"$RESULT" >&2
fi

echo "$RESULT"
[[ "$PASSED" == "true" ]] && exit 0
exit 1
