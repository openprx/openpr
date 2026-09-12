#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER="$ROOT_DIR/scripts/lib/flow_v0_4_verifier_source_checks.py"
TMP_DIR="$(mktemp -d /tmp/openpr-flow-v04-current-head.XXXXXX)"
trap 'rm -rf "$TMP_DIR"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

python3 - "$ROOT_DIR" "$HELPER" <<'PY'
import importlib.util
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
path = pathlib.Path(sys.argv[2])
spec = importlib.util.spec_from_file_location("flow_v04_checks", path)
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)

repository = (root / "apps/api/src/flow/repository.rs").read_text()
good = module.projection_parent_alias_result(repository)
assert good["passed"], good
mutant = repository + '''\nconst MUTANT: &str = r"\nSELECT projection.parent_id\nFROM flow_object_projections projection\n";\n'''
bad = module.projection_parent_alias_result(mutant)
assert not bad["passed"] and bad["violations"], bad
print("PASS: SQL-literal-scoped projection alias check is green and a real projection.parent_id mutation is red")

server = (root / "apps/mcp-server/src/server.rs").read_text()
policy = module.tool_policy_scopes_result(server)
assert policy["representation"] == "slice"
assert policy["entry_count"] == policy["unique_entry_count"] > 0
try:
    module.tool_policy_scopes_result(server.replace("const TOOL_POLICY_SCOPES", "const MUTATED_POLICY_SCOPES", 1))
except ValueError:
    pass
else:
    raise AssertionError("renaming the actual policy-scope declaration did not turn the parser red")
print("PASS: current TOOL_POLICY_SCOPES slice parses and renaming its declaration is mutation red")
PY

bare='{"id":"object-1","title":"bare"}'
wrapped='{"data":{"id":"object-1","title":"bare"}}'
normalizer='if type=="object" and (.data|type)=="object" then .data elif type=="object" then . else empty end'
[[ "$(jq -c "$normalizer" <<<"$bare")" == "$(jq -c "$normalizer" <<<"$wrapped")" ]] \
  || fail "MCP bare-object and REST-style envelope did not normalize identically"
if jq -e "$normalizer" >/dev/null 2>&1 <<< '[]'; then
  fail "non-object MCP payload was accepted"
fi
echo "PASS: MCP bare-object and wrapped-object shapes normalize identically; a non-object is red"

grep -Fq 'reason_code:"owned_by_v0_5_relation_gate"' "$ROOT_DIR/scripts/verify-flow-rest-contract-v0.4.sh" \
  || fail "v0.5 relation criterion has no paired v0.4 exclusion reason"
grep -q '^  relation_pagination_reauthorization_no_leak:' /opt/working/sylvode-flow/gates/v0.5-gate.yaml \
  || fail "the paired v0.5 relation gate anchor does not exist"
if grep -q 'INSERT INTO flow_objects' "$ROOT_DIR/scripts/verify-flow-integrity-records-v0.4.sh"; then
  fail "integrity fixture still inserts a second navigator root"
fi
grep -Fq 'FLOW_OBJECTS_A_BEFORE=' "$ROOT_DIR/scripts/verify-flow-integrity-records-v0.4.sh" \
  || fail "integrity fixture does not compare request side effects against the migrated baseline"
echo "PASS: later-version relation coverage is paired and the integrity fixture reuses the canonical root"

grep -Fq "root.governance_metadata->>'system_role'='workspace_navigator_root'" \
  "$ROOT_DIR/scripts/lib/flow_cardinality_live_probe.py" \
  || fail "cardinality collision fixture does not satisfy the current navigator-root parent invariant"
echo "PASS: cardinality collision fixture targets the duplicate key after satisfying current parent invariants"
