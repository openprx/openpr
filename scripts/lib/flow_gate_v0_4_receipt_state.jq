# Derive every mutable acceptance-state field in the v0.4 receipt from its
# authoritative inputs. report-flow-v0.4-json.sh applies this to a new receipt;
# record-flow-v0.4-manual-signoff.sh reapplies it after each manual decision.
# Keeping this as one jq program prevents the two writers from drifting.

def check_failures:
  [.checks[] |
    if .status == "failed" then
      "automated-check-failed:" + .id
    elif .status == "passed" and
         (((.executed_count | type) != "number") or .executed_count <= 0) then
      "automated-check-not-executed:" + .id
    else empty end];

def hard_gate_failures:
  [.hard_gates | to_entries[] | select(.value != "passed") |
    "hard-gate-not-passed:" + .key + ":" + .value];

def manual_pending:
  [.manual_signoffs | to_entries[] | select(.value.status == "pending") |
    "manual-signoff-pending:" + .key];

# ADR-0017: page_editor / navigator_a11y moved to the frontend track.  The rows
# stay in the artifact so the deferral is visible and auditable; they are not
# "passed" and must never be counted as such.  Only the two keys named by
# ADR-0017 may hold this status -- the schema enforces that structurally, so a
# bad criterion (feature_flag) cannot be laundered through it.
def manual_deferred:
  [.manual_signoffs | to_entries[] |
    select(.value.status == "deferred_to_frontend_track") |
    "manual-signoff-deferred:" + .key];

def manual_blocking:
  [.manual_signoffs | to_entries[] |
    select(.value.status == "failed" or .value.status == "needs_rework") |
    "manual-signoff-blocking:" + .key + ":" + .value.status];

def mcp_stronger_coverage_passed:
  .required_commands.mcp_transport_verify.status == "passed"
  and .required_commands.tool_registry_verify.status == "passed"
  and .hard_gates.mcp_three_transport_contract == "passed"
  and .hard_gates.tool_registry_expected_107_or_rebased == "passed";

def uncovered_environment:
  if mcp_stronger_coverage_passed then
    []
  else
    [.checks[] | select(.status == "environment_unavailable") |
      "environment-unavailable:" + .id]
  end;

def source_blocking:
  if (.source.dirty // false) then ["source-dirty"] else [] end;

(check_failures + hard_gate_failures + manual_blocking + uncovered_environment + source_blocking) as $blocking
| manual_pending as $pending
| manual_deferred as $deferred
| .counts.automated = (.checks | length)
| .counts.passed = ([.checks[] | select(.status == "passed" and (.executed_count | type) == "number" and .executed_count > 0)] | length)
| .counts.failed = ([.checks[] | select(.status == "failed" or (.status == "passed" and (((.executed_count | type) != "number") or .executed_count <= 0)))] | length)
| .counts.environment_unavailable = ([.checks[] | select(.status == "environment_unavailable")] | length)
| .counts.manual_pending = ($pending | length)
| .counts.manual_deferred_to_frontend_track = ($deferred | length)
| .deferred_signoffs = $deferred
| .blockers = ($blocking + $pending)
| .counts.unresolved = (.blockers | length)
| .gate_passed = (.blockers | length == 0)
| .mode = (
    if .gate_passed then "release"
    elif ($blocking | length) == 0 then "pre_signoff"
    else "blocked"
    end
  )
