#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 gate-result.json verifier.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md ("verify" role)
# -- "不运行产品动作，只校验 JSON schema、required keys、命令 exit、evidence
# path/checksum 与 source HEAD；成功 0，漂移 1."
#
# This script does NOT trust gate-result.json's own self-reported
# `hard_gates`/`gate_passed` fields. It independently recomputes every
# hard gate this round has a mapped evidence artifact for (see
# scripts/lib/flow_gate_v0_4_recompute.py), re-validating each artifact's
# OWN internal violation/passed fields rather than a bare top-level
# `passed:true`, and treats any hard gate it cannot map to an evidence
# artifact as "not_verified" (never silently "passed"). It also
# recomputes every artifact's sha256 from the file on disk and compares
# against the recorded value, and checks source.head against the actual
# repository HEAD.
#
# Exit codes: 0 = zero drift (every hard gate the file claims "passed"
# recomputes to "passed", every artifact checksum matches, and gate_passed
# is only true when the recomputation agrees -- i.e. automation is fully
# green). generic.test_mcp may retain environment_unavailable/69 only with
# integrity-checked proof plus passed live three-transport and registry gates.
# Exit 1 = drift found / automation not fully green, 2 = usage/tool/evidence
# malformed.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
REPO_ROOT="$ROOT_DIR"
SCHEMA_PATH="$ROOT_DIR/docs/schemas/sylvode-flow-gate-v0.4.schema.json"
RECEIPT_STATE_FILTER="$ROOT_DIR/scripts/lib/flow_gate_v0_4_receipt_state.jq"
JSON_MODE=0
GATE_RESULT_PATH=""

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-v0.4-json.sh GATE_RESULT_JSON --json [OPTIONS]

Independently recomputes every hard gate v0.4-gate.yaml names from the
evidence artifacts gate-result.json references (never trusting its
self-reported hard_gates/gate_passed), recomputes every artifact's
sha256, and checks source.head against the actual repository HEAD.

Arguments:
  GATE_RESULT_JSON   Path to gate-result.json to verify.

Options:
  --evidence-root DIR   Root the artifact relative paths resolve against.
                        Default: /opt/working/sylvode-flow/evidence/v0.4
  --repo-root DIR       Repository whose HEAD is compared against
                        source.head, and whose migrations/ directory is
                        scanned for legacy_pages_drop_requires_separate_adr.
                        Default: this checkout.
  --schema PATH         Path to the v0.4 gate schema (structural
                        required-key check only -- this script does not
                        implement a general JSON Schema validator).
                        Default: docs/schemas/sylvode-flow-gate-v0.4.schema.json
  --json                Required for CLI-contract compatibility.
  -h, --help            Show this help and exit 0.

Exit codes: 0 zero drift / automation fully green, 1 drift found,
2 usage/tool/evidence malformed.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --schema) SCHEMA_PATH="${2:?--schema requires a PATH argument}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)
      if [[ -n "$GATE_RESULT_PATH" ]]; then
        echo "Unexpected argument: $1" >&2; usage >&2; exit 2
      fi
      GATE_RESULT_PATH="$1"; shift ;;
  esac
done

if [[ -z "$GATE_RESULT_PATH" ]]; then
  echo "FAIL: GATE_RESULT_JSON argument is required" >&2
  usage >&2
  exit 2
fi
if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
for tool in jq sha256sum git python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    exit 2
  fi
done
if [[ ! -f "$GATE_RESULT_PATH" ]]; then
  echo "FAIL: gate-result.json not found: $GATE_RESULT_PATH" >&2
  exit 2
fi
if ! jq empty "$GATE_RESULT_PATH" >/dev/null 2>&1; then
  echo "FAIL: not valid JSON: $GATE_RESULT_PATH" >&2
  exit 2
fi
if [[ ! -f "$SCHEMA_PATH" ]]; then
  echo "FAIL: schema file not found: $SCHEMA_PATH" >&2
  exit 2
fi
if [[ ! -f "$RECEIPT_STATE_FILTER" ]]; then
  echo "FAIL: receipt-state filter not found: $RECEIPT_STATE_FILTER" >&2
  exit 2
fi
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi
REPO_ROOT="$(cd "$REPO_ROOT" && pwd)"

DRIFT=()

# ---- structural pre-flight: verify the document's SHAPE before any jq call that
# assumes it, so a malformed/incomplete gate-result.json produces a clear exit-2
# message instead of a raw jq crash (jq's own builtins -- `keys`, `keys[]`, `[]`
# iteration, `has()` -- throw on null/wrong-typed input, and under `set -e` that
# exit code would otherwise leak straight out of this script). Any problem found
# here is "evidence malformed" (exit 2), never folded into the drift list (exit 1)
# -- a file that is not even shaped right cannot have its claims meaningfully
# compared against a recomputation.
STRUCT_ERRORS=()

if [[ "$(jq -r 'type' "$GATE_RESULT_PATH")" != "object" ]]; then
  echo "FAIL: gate-result.json top level is not a JSON object: $GATE_RESULT_PATH" >&2
  exit 2
fi

SCHEMA_REQUIRED_KEYS="$(jq -r '.required[]' "$SCHEMA_PATH")"
while IFS= read -r key; do
  if [[ "$(jq --arg k "$key" 'has($k)' "$GATE_RESULT_PATH")" != "true" ]]; then
    STRUCT_ERRORS+=("missing required top-level key: $key")
  fi
done <<<"$SCHEMA_REQUIRED_KEYS"

# Container fields the rest of this script indexes into or iterates over must be
# the JSON type it expects, checked only when the key is present (a missing key
# was already reported above and would make this check redundant/misleading).
declare -A REQUIRED_TOP_LEVEL_TYPES=(
  [source]=object
  [artifacts]=object
  [hard_gates]=object
  [required_commands]=object
  [checks]=array
  [manual_signoffs]=object
  [counts]=object
  [blockers]=array
)
for key in "${!REQUIRED_TOP_LEVEL_TYPES[@]}"; do
  expected="${REQUIRED_TOP_LEVEL_TYPES[$key]}"
  actual="$(jq -r --arg k "$key" 'if has($k) then (.[$k] | type) else "missing" end' "$GATE_RESULT_PATH")"
  [[ "$actual" == "missing" ]] && continue
  if [[ "$actual" != "$expected" ]]; then
    STRUCT_ERRORS+=("top-level key '$key' must be a JSON $expected, found $actual")
  fi
done

if [[ ${#STRUCT_ERRORS[@]} -gt 0 ]]; then
  echo "FAIL: gate-result.json is structurally malformed -- cannot recompute drift against it:" >&2
  for e in "${STRUCT_ERRORS[@]}"; do
    echo "  - $e" >&2
  done
  exit 2
fi

if [[ "$(jq -r '.schema_version // empty' "$GATE_RESULT_PATH")" != "sylvode.flow.gate-result.v1" ]]; then
  DRIFT+=("schema_version is not sylvode.flow.gate-result.v1")
fi
if [[ "$(jq -r '.release // empty' "$GATE_RESULT_PATH")" != "0.4.0" ]]; then
  DRIFT+=("release is not 0.4.0")
fi

# ---- source/source_baseline vs actual checked-out repository ----
ACTUAL_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
ACTUAL_RUST_VERSION="$(sed -n '/^\[workspace.package\]$/,/^\[/s/^version = "\([^"]*\)"/\1/p' "$REPO_ROOT/Cargo.toml" | head -1)"
ACTUAL_FRONTEND_VERSION="$(jq -r '.version // empty' "$REPO_ROOT/frontend/package.json")"
RECORDED_HEAD="$(jq -r '.source.head // empty' "$GATE_RESULT_PATH")"
if [[ "$RECORDED_HEAD" != "$ACTUAL_HEAD" ]]; then
  DRIFT+=("source.head=$RECORDED_HEAD does not match actual repo HEAD=$ACTUAL_HEAD")
fi
BASELINE_REPOSITORY="$(jq -r '.source_baseline.repository // empty' "$GATE_RESULT_PATH")"
BASELINE_RUST_VERSION="$(jq -r '.source_baseline.rust_workspace_version // empty' "$GATE_RESULT_PATH")"
BASELINE_FRONTEND_VERSION="$(jq -r '.source_baseline.frontend_package_version // empty' "$GATE_RESULT_PATH")"
BASELINE_HEAD="$(jq -r '.source_baseline.reviewed_head // empty' "$GATE_RESULT_PATH")"
[[ "$BASELINE_REPOSITORY" == "$REPO_ROOT" ]] || DRIFT+=("source_baseline.repository=$BASELINE_REPOSITORY does not match repo root=$REPO_ROOT")
[[ "$BASELINE_RUST_VERSION" == "$ACTUAL_RUST_VERSION" ]] || DRIFT+=("source_baseline.rust_workspace_version=$BASELINE_RUST_VERSION does not match Cargo.toml=$ACTUAL_RUST_VERSION")
[[ "$BASELINE_FRONTEND_VERSION" == "$ACTUAL_FRONTEND_VERSION" ]] || DRIFT+=("source_baseline.frontend_package_version=$BASELINE_FRONTEND_VERSION does not match frontend/package.json=$ACTUAL_FRONTEND_VERSION")
[[ "$BASELINE_HEAD" == "$ACTUAL_HEAD" ]] || DRIFT+=("source_baseline.reviewed_head=$BASELINE_HEAD does not match actual repo HEAD=$ACTUAL_HEAD")
RECORDED_DIRTY="$(jq -r 'if has("source") and (.source | has("dirty")) then (.source.dirty | tostring) else "" end' "$GATE_RESULT_PATH")"
if [[ -n "$(git -C "$REPO_ROOT" status --porcelain)" ]]; then
  ACTUAL_DIRTY=true
else
  ACTUAL_DIRTY=false
fi
if [[ "$RECORDED_DIRTY" != "$ACTUAL_DIRTY" ]]; then
  DRIFT+=("source.dirty=$RECORDED_DIRTY does not match actual working tree state=$ACTUAL_DIRTY")
fi
if [[ "$ACTUAL_DIRTY" == "true" ]]; then
  DRIFT+=("working tree is dirty; strict gate must fail on a dirty source tree")
fi

# union_keys SECTION -- prints the sorted union of the schema's declared required
# keys for top-level object SECTION and the keys actually present in
# GATE_RESULT_PATH's SECTION object. Missing-from-the-file keys must still be
# visited (as an explicit drift finding), not silently skipped, or a sparse
# object could dodge every check just by omitting entries.
union_keys() {
  local section="$1"
  jq -r --arg s "$section" '.properties[$s].required[]?' "$SCHEMA_PATH" > "$TMP_KEYS_A"
  jq -r --arg s "$section" '.[$s] // {} | keys[]?' "$GATE_RESULT_PATH" > "$TMP_KEYS_B"
  sort -u "$TMP_KEYS_A" "$TMP_KEYS_B"
}
TMP_KEYS_A="$(mktemp)"
TMP_KEYS_B="$(mktemp)"
trap 'rm -f "$TMP_KEYS_A" "$TMP_KEYS_B"' EXIT

# ---- artifact checksums: recompute, compare ----
while IFS= read -r key; do
  [[ -z "$key" ]] && continue
  [[ "$key" == "gate_result" ]] && continue  # self-referential, see report script comment
  if [[ "$(jq --arg k "$key" '.artifacts | has($k)' "$GATE_RESULT_PATH")" != "true" ]]; then
    DRIFT+=("artifacts.$key is missing (required by $SCHEMA_PATH)")
    continue
  fi
  rel_path="$(jq -r --arg k "$key" '.artifacts[$k].path // empty' "$GATE_RESULT_PATH")"
  recorded_sha="$(jq -r --arg k "$key" '.artifacts[$k].sha256 // empty' "$GATE_RESULT_PATH")"
  if [[ -z "$rel_path" || -z "$recorded_sha" ]]; then
    DRIFT+=("artifacts.$key is missing 'path' or 'sha256'")
    continue
  fi
  abs_path="$REPO_ROOT/$rel_path"
  if [[ ! -f "$abs_path" ]]; then
    # legacy_pages_inventory / surface_coverage_result / cardinality_result live
    # under evidence-root, which may not be REPO_ROOT-relative.
    abs_path="$EVIDENCE_ROOT/$(basename "$rel_path")"
  fi
  if [[ ! -f "$abs_path" ]]; then
    DRIFT+=("artifact '$key': file not found at $rel_path (nor $abs_path)")
    continue
  fi
  actual_sha="$(sha256sum "$abs_path" | awk '{print $1}')"
  if [[ "$actual_sha" != "$recorded_sha" ]]; then
    DRIFT+=("artifact '$key': recorded sha256=$recorded_sha does not match actual sha256=$actual_sha of $abs_path")
  fi
done < <(union_keys artifacts)

# ---- checks[]: every recorded check's log evidence must exist with matching sha256 ----
CHECKS_COUNT="$(jq '.checks | length' "$GATE_RESULT_PATH")"
MCP_ENVIRONMENT_UNAVAILABLE=0
for ((i = 0; i < CHECKS_COUNT; i++)); do
  entry_type="$(jq -r ".checks[$i] | type" "$GATE_RESULT_PATH")"
  if [[ "$entry_type" != "object" ]]; then
    DRIFT+=("checks[$i] is not an object (found $entry_type)")
    continue
  fi
  cid="$(jq -r ".checks[$i].id // empty" "$GATE_RESULT_PATH")"
  cstatus="$(jq -r ".checks[$i].status // empty" "$GATE_RESULT_PATH")"
  cexit="$(jq -r ".checks[$i].exit_code // empty" "$GATE_RESULT_PATH")"
  crel="$(jq -r ".checks[$i].evidence // empty" "$GATE_RESULT_PATH")"
  csha="$(jq -r ".checks[$i].sha256 // empty" "$GATE_RESULT_PATH")"
  if [[ -z "$cid" || -z "$crel" || -z "$csha" ]]; then
    DRIFT+=("checks[$i] is missing 'id', 'evidence' or 'sha256'")
    continue
  fi
  cabs="$REPO_ROOT/$crel"
  if [[ ! -f "$cabs" ]]; then
    # report-flow-v0.4-json.sh supports an arbitrary --evidence-root while
    # retaining the contract's canonical evidence/v0.4/logs/... paths in the
    # portable result document. Resolve those logs the same way artifacts are
    # resolved above instead of falsely treating a non-default evidence root
    # as missing evidence.
    cabs="$EVIDENCE_ROOT/logs/$(basename "$crel")"
  fi
  if [[ ! -f "$cabs" ]]; then
    DRIFT+=("check '$cid': evidence log not found at $crel (nor $cabs)")
    continue
  fi
  actual_sha="$(sha256sum "$cabs" | awk '{print $1}')"
  if [[ "$actual_sha" != "$csha" ]]; then
    DRIFT+=("check '$cid': recorded sha256 does not match actual log at $crel")
  fi
  if [[ "$cstatus" == "environment_unavailable" ]]; then
    if [[ "$cid" != "generic.test_mcp" ]]; then
      DRIFT+=("check '$cid' uses environment_unavailable, which is only defined for generic.test_mcp")
    elif [[ "$cexit" != "69" ]]; then
      DRIFT+=("check '$cid' status='environment_unavailable' must carry exit_code=69, found '$cexit'")
    elif ! grep -Fq 'MCP_TEST_RESULT=environment_unavailable' "$cabs"; then
      DRIFT+=("check '$cid' claims environment_unavailable but its integrity-checked log has no environment gate result")
    else
      MCP_ENVIRONMENT_UNAVAILABLE=$((MCP_ENVIRONMENT_UNAVAILABLE + 1))
    fi
  elif [[ "$cstatus" != "passed" ]]; then
    DRIFT+=("check '$cid' status='$cstatus' (must be 'passed' for a green gate)")
  fi
done

# ---- hard_gates: independently recomputed, never trusted from self-report ----
RECOMPUTE_JSON="$(python3 "$ROOT_DIR/scripts/lib/flow_gate_v0_4_recompute.py" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT")"
if ! jq -e 'type == "object" and has("hard_gates")' >/dev/null 2>&1 <<<"$RECOMPUTE_JSON"; then
  echo "FAIL: scripts/lib/flow_gate_v0_4_recompute.py did not produce the expected JSON shape" >&2
  echo "$RECOMPUTE_JSON" >&2
  exit 2
fi
RECOMPUTED_GATES="$(jq '.hard_gates' <<<"$RECOMPUTE_JSON")"
CLAIMED_GATES="$(jq '.hard_gates' "$GATE_RESULT_PATH")"

ANY_HARD_GATE_NOT_PASSED=0
while IFS= read -r key; do
  [[ -z "$key" ]] && continue
  claimed="$(jq -r --arg k "$key" 'if has($k) then .[$k] else "MISSING" end' <<<"$CLAIMED_GATES")"
  recomputed="$(jq -r --arg k "$key" '.[$k] // "not_verified"' <<<"$RECOMPUTED_GATES")"
  if [[ "$claimed" != "$recomputed" ]]; then
    DRIFT+=("hard_gate '$key': gate-result.json claims '$claimed' but independent recomputation says '$recomputed'")
  fi
  if [[ "$recomputed" != "passed" ]]; then
    ANY_HARD_GATE_NOT_PASSED=1
  fi
done < <(union_keys hard_gates)

if [[ $ANY_HARD_GATE_NOT_PASSED -eq 1 ]]; then
  DRIFT+=("at least one hard gate does not recompute to 'passed' -- automation is not fully green")
fi

# ---- required_commands: producer/report commands must have passed. The three
# commands that are ordered after report may truthfully be not_run with no exit
# code in report's initial artifact. A recorded failure is never equivalent to
# that state and remains drift.
while IFS= read -r key; do
  [[ -z "$key" ]] && continue
  if [[ "$(jq --arg k "$key" '.required_commands | has($k)' "$GATE_RESULT_PATH")" != "true" ]]; then
    DRIFT+=("required_commands.$key is missing (required by $SCHEMA_PATH)")
    continue
  fi
  status="$(jq -r --arg k "$key" '.required_commands[$k].status // empty' "$GATE_RESULT_PATH")"
  has_exit_code="$(jq --arg k "$key" '.required_commands[$k] | has("exit_code")' "$GATE_RESULT_PATH")"
  exit_code_type="$(jq -r --arg k "$key" '.required_commands[$k].exit_code | type' "$GATE_RESULT_PATH")"
  if [[ "$has_exit_code" != "true" ]]; then
    DRIFT+=("required_commands.$key is missing exit_code")
  elif [[ "$status" == "not_run" ]] && [[ "$key" == "verify" || "$key" == "gate" || "$key" == "manual_signoff" ]]; then
    if [[ "$exit_code_type" != "null" ]]; then
      DRIFT+=("required_commands.$key status='not_run' must carry exit_code=null")
    fi
  elif [[ "$status" != "passed" ]]; then
    DRIFT+=("required_commands.$key status='$status' (must be 'passed')")
  fi
done < <(union_keys required_commands)

# generic.test_mcp is a compose-environment integration check. Its dedicated
# exit 69 remains visible in checks/counts and is non-blocking only when two
# stronger required verifiers independently passed: the shipped server spoke
# real JSON-RPC over HTTP/SSE/stdio and the live registry enumerated all tools.
if [[ $MCP_ENVIRONMENT_UNAVAILABLE -gt 0 ]]; then
  MCP_TRANSPORT_COMMAND_STATUS="$(jq -r '.required_commands.mcp_transport_verify.status // empty' "$GATE_RESULT_PATH")"
  MCP_REGISTRY_COMMAND_STATUS="$(jq -r '.required_commands.tool_registry_verify.status // empty' "$GATE_RESULT_PATH")"
  MCP_TRANSPORT_GATE_STATUS="$(jq -r '.mcp_three_transport_contract // "not_verified"' <<<"$RECOMPUTED_GATES")"
  MCP_REGISTRY_GATE_STATUS="$(jq -r '.tool_registry_expected_107_or_rebased // "not_verified"' <<<"$RECOMPUTED_GATES")"
  if [[ $MCP_ENVIRONMENT_UNAVAILABLE -ne 1 ]]; then
    DRIFT+=("generic.test_mcp environment_unavailable appears $MCP_ENVIRONMENT_UNAVAILABLE times; expected at most once")
  fi
  if [[ "$MCP_TRANSPORT_COMMAND_STATUS" != "passed" || "$MCP_REGISTRY_COMMAND_STATUS" != "passed" ||
        "$MCP_TRANSPORT_GATE_STATUS" != "passed" || "$MCP_REGISTRY_GATE_STATUS" != "passed" ]]; then
    DRIFT+=("generic.test_mcp environment is unavailable and stronger MCP coverage is not fully green (transport command=$MCP_TRANSPORT_COMMAND_STATUS gate=$MCP_TRANSPORT_GATE_STATUS; registry command=$MCP_REGISTRY_COMMAND_STATUS gate=$MCP_REGISTRY_GATE_STATUS)")
  fi
fi

# ---- derived receipt-state consistency ----
# Recompute against independently derived hard gates, not the receipt's claims,
# then require every writer-maintained field to match the single shared state
# transition. This proves mode=release is reachable only through real green
# automation plus five passed manual rows.
set +e
EXPECTED_RECEIPT="$(jq --argjson gates "$RECOMPUTED_GATES" '.hard_gates = $gates' "$GATE_RESULT_PATH" | jq -f "$RECEIPT_STATE_FILTER")"
EXPECTED_RECEIPT_EXIT=$?
set -e
if [[ $EXPECTED_RECEIPT_EXIT -ne 0 ]]; then
  echo "FAIL: gate-result.json cannot be evaluated by the receipt-state derivation" >&2
  exit 2
fi
for state_path in mode gate_passed counts blockers; do
  actual_state="$(jq -c ".$state_path" "$GATE_RESULT_PATH")"
  expected_state="$(jq -c ".$state_path" <<<"$EXPECTED_RECEIPT")"
  if [[ "$actual_state" != "$expected_state" ]]; then
    DRIFT+=("derived $state_path drift: receipt=$actual_state recomputed=$expected_state")
  fi
done

# ---- gate_passed consistency: must only be true when there is zero drift so far ----
CLAIMED_GATE_PASSED="$(jq -r '.gate_passed' "$GATE_RESULT_PATH")"
if [[ "$CLAIMED_GATE_PASSED" == "true" && ${#DRIFT[@]} -gt 0 ]]; then
  DRIFT+=("gate_passed=true but ${#DRIFT[@]} drift finding(s) were recomputed")
fi

PASSED=$([[ ${#DRIFT[@]} -eq 0 ]] && echo true || echo false)
DRIFT_JSON="$(printf '%s\n' "${DRIFT[@]:-}" | jq -R 'select(length>0)' | jq -s '.')"

RESULT="$(jq -n \
  --arg gate_result "$GATE_RESULT_PATH" \
  --argjson recomputed_hard_gates "$RECOMPUTED_GATES" \
  --argjson recompute_reasons "$(jq '.reasons' <<<"$RECOMPUTE_JSON")" \
  --argjson drift "$DRIFT_JSON" \
  --argjson passed "$PASSED" \
  '{gate_result:$gate_result, recomputed_hard_gates:$recomputed_hard_gates, recompute_reasons:$recompute_reasons, drift:$drift, passed:$passed}')"

echo "$RESULT" | jq .

if [[ "$PASSED" == "true" ]]; then
  exit 0
fi
exit 1
