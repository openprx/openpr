#!/usr/bin/env bash
set -euo pipefail

# Focused counterexamples for the v0.4 receipt acceptance mechanism. This test
# uses synthetic receipt inputs only; it never signs the release evidence tree.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STATE_FILTER="$ROOT_DIR/scripts/lib/flow_gate_v0_4_receipt_state.jq"
RECORD_SCRIPT="$ROOT_DIR/scripts/record-flow-v0.4-manual-signoff.sh"
TEST_MCP_SCRIPT="$ROOT_DIR/scripts/test-mcp.sh"
TMP_DIR="$(mktemp -d /tmp/openpr-flow-v04-acceptance.XXXXXX)"

cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

assert_jq() {
  local description="$1"
  local expression="$2"
  local path="$3"
  jq -e "$expression" "$path" >/dev/null || fail "$description"
  echo "PASS: $description"
}

BASE="$TMP_DIR/base.json"
jq -n '{
  schema_version: "sylvode.flow.gate-result.v1",
  mode: "blocked",
  gate_passed: false,
  counts: {},
  checks: [{id:"generic.test_mcp",status:"passed",command:"bash scripts/test-mcp.sh",exit_code:0,duration_ms:1,executed_count:1,evidence:"test.log",sha256:("0"*64)}],
  required_commands: {
    mcp_transport_verify: {status:"passed"},
    tool_registry_verify: {status:"passed"}
  },
  hard_gates: {
    mcp_three_transport_contract: "passed",
    tool_registry_expected_107_or_rebased: "passed"
  },
  artifacts: {},
  manual_signoffs: {
    page_editor:{status:"deferred_to_frontend_track",reviewer:"",evidence:"ADR-0017"},
    navigator_a11y:{status:"deferred_to_frontend_track",reviewer:"",evidence:"ADR-0017"},
    restart_recovery:{status:"pending",reviewer:"",evidence:""},
    feature_flag:{status:"pending",reviewer:"",evidence:""},
    forms_regression:{status:"pending",reviewer:"",evidence:""}
  },
  blockers: []
}' | jq -f "$STATE_FILTER" > "$BASE"

assert_jq "pre-signoff receipt has the three real pending blockers after ADR-0017" \
  '.mode == "pre_signoff" and .gate_passed == false and .counts.manual_pending == 3 and .counts.unresolved == 3 and (.blockers | length) == 3' "$BASE"

# ADR-0017 must defer, not launder: the two moved rows stay visible in the
# artifact, are counted separately, and are never reported as passed.
assert_jq "deferred rows stay visible, are counted, and are not passed" \
  '.counts.manual_deferred_to_frontend_track == 2
   and (.deferred_signoffs | sort) == ["manual-signoff-deferred:navigator_a11y","manual-signoff-deferred:page_editor"]
   and ([.manual_signoffs[] | select(.status == "passed")] | length) == 0' "$BASE"

FAILED="$TMP_DIR/failed.json"
jq '.checks[0].status="failed" | .checks[0].exit_code=1' "$BASE" | jq -f "$STATE_FILTER" > "$FAILED"
assert_jq "an automated failure is named in blockers and blocks the receipt" \
  '.mode == "blocked" and .gate_passed == false and .counts.failed == 1 and .counts.unresolved == 4 and (.blockers | index("automated-check-failed:generic.test_mcp")) != null' "$FAILED"

ZERO_EXECUTION="$TMP_DIR/zero-execution.json"
jq '.checks[0].executed_count=0' "$BASE" | jq -f "$STATE_FILTER" > "$ZERO_EXECUTION"
assert_jq "a passed check with zero executions is a named automated failure" \
  '.mode == "blocked" and .gate_passed == false and .counts.passed == 0 and .counts.failed == 1
   and (.blockers | index("automated-check-not-executed:generic.test_mcp")) != null' "$ZERO_EXECUTION"

DIRTY="$TMP_DIR/dirty.json"
jq '.source={dirty:true}' "$BASE" | jq -f "$STATE_FILTER" > "$DIRTY"
assert_jq "a dirty source is explicit and cannot claim release after signoff" \
  '.mode == "blocked" and .gate_passed == false and (.blockers | index("source-dirty")) != null and .counts.unresolved == 4' "$DIRTY"

ENV_COVERED="$TMP_DIR/environment-covered.json"
jq '.checks[0].status="environment_unavailable" | .checks[0].exit_code=69' "$BASE" | jq -f "$STATE_FILTER" > "$ENV_COVERED"
assert_jq "environment-unavailable remains visible but stronger MCP evidence prevents a false implementation failure" \
  '.mode == "pre_signoff" and .counts.failed == 0 and .counts.environment_unavailable == 1 and ([.blockers[] | select(startswith("environment-unavailable:"))] | length) == 0' "$ENV_COVERED"

ENV_UNCOVERED="$TMP_DIR/environment-uncovered.json"
jq '.required_commands.mcp_transport_verify.status="failed"' "$ENV_COVERED" | jq -f "$STATE_FILTER" > "$ENV_UNCOVERED"
assert_jq "environment-unavailable blocks when stronger MCP evidence is absent" \
  '.mode == "blocked" and (.blockers | index("environment-unavailable:generic.test_mcp")) != null' "$ENV_UNCOVERED"

PENDING="$TMP_DIR/pending.json"
cp "$BASE" "$PENDING"
RELEASE="$TMP_DIR/release.json"
cp "$BASE" "$RELEASE"
for key in restart_recovery feature_flag forms_regression; do
  "$RECORD_SCRIPT" --gate-result "$RELEASE" --key "$key" --status passed \
    --reviewer "acceptance-test" --evidence "synthetic:$key" >/dev/null
done
assert_jq "three real record transitions reach release and clear every derived blocker" \
  '.mode == "release" and .gate_passed == true and .counts.manual_pending == 0 and .counts.unresolved == 0 and .blockers == []
   and ([.manual_signoffs | to_entries[] | select(.key != "page_editor" and .key != "navigator_a11y") | .value.status] | all(. == "passed"))
   and .counts.manual_deferred_to_frontend_track == 2' "$RELEASE"

# The deferral must not become a way to retire a criterion that is merely
# broken. feature_flag is exactly that case (ADR-0017 section 4), so the writer
# must refuse it.
set +e
# Point at a real receipt so a refusal can only come from the key/status rule,
# not from a missing file (that would exit 2, not 1).
LAUNDER="$TMP_DIR/launder.json"
cp "$BASE" "$LAUNDER"
"$RECORD_SCRIPT" --gate-result "$LAUNDER" --key feature_flag \
  --status deferred_to_frontend_track --reviewer "acceptance-test" \
  --evidence "synthetic" > "$TMP_DIR/launder.log" 2>&1
LAUNDER_EXIT=$?
set -e
[[ $LAUNDER_EXIT -eq 1 ]] || fail "deferring feature_flag exit=$LAUNDER_EXIT, expected 1"
jq -e '.manual_signoffs.feature_flag.status == "pending"' "$LAUNDER" >/dev/null \
  || fail "refused deferral still mutated the receipt"
grep -Fq "only valid for: page_editor navigator_a11y" "$TMP_DIR/launder.log" \
  || fail "deferring feature_flag was refused for the wrong reason"
echo "PASS: deferred_to_frontend_track is refused for every key ADR-0017 did not name"

# Run the actual gate aggregator with only its automated verifier replaced by a
# deterministic green stub. This isolates and proves the manual-state/exit-code
# transition without pretending synthetic artifacts passed the real verifier.
mkdir -p "$TMP_DIR/gate-root/scripts"
cp "$ROOT_DIR/scripts/gate-flow-v0.4.sh" "$TMP_DIR/gate-root/scripts/gate-flow-v0.4.sh"
printf '%s\n' '#!/usr/bin/env bash' 'exit 0' > "$TMP_DIR/gate-root/scripts/verify-flow-v0.4-json.sh"
chmod +x "$TMP_DIR/gate-root/scripts/verify-flow-v0.4-json.sh"

set +e
"$TMP_DIR/gate-root/scripts/gate-flow-v0.4.sh" --json --gate-result "$PENDING" \
  --evidence-root "$TMP_DIR" --repo-root "$ROOT_DIR" --schema "$ROOT_DIR/docs/schemas/sylvode-flow-gate-v0.4.schema.json" >/dev/null 2>&1
PENDING_STRICT_EXIT=$?
"$TMP_DIR/gate-root/scripts/gate-flow-v0.4.sh" --json --allow-pending --gate-result "$PENDING" \
  --evidence-root "$TMP_DIR" --repo-root "$ROOT_DIR" --schema "$ROOT_DIR/docs/schemas/sylvode-flow-gate-v0.4.schema.json" >/dev/null 2>&1
PENDING_ALLOW_EXIT=$?
"$TMP_DIR/gate-root/scripts/gate-flow-v0.4.sh" --json --gate-result "$RELEASE" \
  --evidence-root "$TMP_DIR" --repo-root "$ROOT_DIR" --schema "$ROOT_DIR/docs/schemas/sylvode-flow-gate-v0.4.schema.json" >/dev/null 2>&1
RELEASE_STRICT_EXIT=$?
set -e
[[ $PENDING_STRICT_EXIT -eq 1 ]] || fail "pending receipt strict gate exit=$PENDING_STRICT_EXIT, expected 1"
[[ $PENDING_ALLOW_EXIT -eq 0 ]] || fail "pending receipt allow-pending gate exit=$PENDING_ALLOW_EXIT, expected 0"
[[ $RELEASE_STRICT_EXIT -eq 0 ]] || fail "release receipt strict gate exit=$RELEASE_STRICT_EXIT, expected 0"
echo "PASS: gate exits agree with receipt state (pending strict=1, pending allow=0, release strict=0)"

set +e
MCP_URL=http://127.0.0.1:1 "$TEST_MCP_SCRIPT" > "$TMP_DIR/mcp-unavailable.log" 2>&1
MCP_UNAVAILABLE_EXIT=$?
set -e
[[ $MCP_UNAVAILABLE_EXIT -eq 69 ]] || fail "closed MCP endpoint exit=$MCP_UNAVAILABLE_EXIT, expected 69"
grep -Fq 'MCP_TEST_RESULT=environment_unavailable' "$TMP_DIR/mcp-unavailable.log" || fail "environment gate marker missing"
echo "PASS: closed MCP environment exits 69 with a machine-readable environment result"

PROBE_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
python3 -m http.server "$PROBE_PORT" --bind 127.0.0.1 > "$TMP_DIR/wrong-server.log" 2>&1 &
PROBE_PID=$!
for _ in $(seq 1 20); do
  curl -fsS "http://127.0.0.1:$PROBE_PORT/" >/dev/null 2>&1 && break
  sleep 0.1
done
set +e
MCP_URL="http://127.0.0.1:$PROBE_PORT" "$TEST_MCP_SCRIPT" > "$TMP_DIR/mcp-implementation.log" 2>&1
MCP_IMPLEMENTATION_EXIT=$?
set -e
kill "$PROBE_PID" 2>/dev/null || true
wait "$PROBE_PID" 2>/dev/null || true
[[ $MCP_IMPLEMENTATION_EXIT -eq 1 ]] || fail "reachable wrong MCP implementation exit=$MCP_IMPLEMENTATION_EXIT, expected 1"
grep -Fq 'MCP_TEST_RESULT=implementation_failed' "$TMP_DIR/mcp-implementation.log" || fail "implementation failure marker missing"
echo "PASS: reachable wrong MCP implementation remains a real exit-1 failure"

echo "All v0.4 acceptance-mechanism counterexamples passed."
