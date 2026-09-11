# Shared v0.5 gate wiring and receipt-state derivation.
#
# report-flow-v0.5-json.sh and record-flow-v0.5-manual-signoff.sh are the two
# writers of gate-result.json.  Both apply this program so blockers, counts,
# mode, candidate readiness and final acceptance cannot drift between writers.
# verify-flow-v0.5-json.sh also uses the same 31-entry wiring table while
# independently rebuilding artifact states from files on disk.

def flow_gate_wiring:
  {
    # ADR-0017: the UI face moved to the frontend track
    # (gates/vF-frontend-gate.yaml#ui_surface_parity_v0_5). This asserts
    # REST/MCP/CLI parity only -- three-face parity does not imply four-face.
    rest_mcp_cli_surface_parity:
      {artifact:"surface_coverage_result", top_level_fallback:true},
    mcp_default_rest_coverage_three_adr_threat_exceptions_only:
      {artifact:"surface_coverage_result", top_level_fallback:true},
    ten_client_convergence:
      {artifact:"convergence_result", top_level_fallback:true},
    ten_client_round_trip_and_lock_budget_no_regression:
      {artifact:"collab_architecture_result", top_level_fallback:false},
    accepted_egress_duplicate_reorder_gap_resync:
      {artifact:"collab_architecture_result", top_level_fallback:false},
    warm_cache_eviction_restart_semantic_equivalence:
      {artifact:"collab_architecture_result", top_level_fallback:false},
    # ADR-0017: offline_accepted_zero_loss_and_recovery_draft moved to the
    # frontend track with its offline_recovery_result artifact. Its evidence is
    # the IndexedDB outbox and the recovery draft, both browser-only.
    token_expiry_and_permission_revocation:
      {artifact:"authz_result", top_level_fallback:false},
    permission_inheritance_and_break:
      {artifact:"authz_result", top_level_fallback:false},
    authz_linearization_no_escalation:
      {artifact:"authz_result", top_level_fallback:false},
    permission_cache_is_not_authority:
      {artifact:"authz_result", top_level_fallback:false},
    authz_numeric_budgets_locked:
      {artifact:"authz_result", top_level_fallback:false},
    move_subtree_nodes_max_locked:
      {artifact:"multi_document_result", top_level_fallback:false},
    permission_revocation_closes_subtree_sessions:
      {artifact:"authz_result", top_level_fallback:false},
    search_and_relation_use_effective_permission:
      {artifact:"authz_result", top_level_fallback:false},
    authz_boundary_self_lockout_guarded:
      {artifact:"authz_result", top_level_fallback:false},
    admin_bot_does_not_bypass_object_boundary:
      {artifact:"authz_result", top_level_fallback:false},
    archive_tier_by_object_scope:
      {artifact:"authz_result", top_level_fallback:false},
    multi_document_lock_order_and_atomicity:
      {artifact:"multi_document_result", top_level_fallback:false},
    navigator_tombstone_growth_measured:
      {artifact:"collab_architecture_result", top_level_fallback:false},
    invalid_update_named_reason_producer_consumer_contract:
      {artifact:"invalid_update_reason_result", top_level_fallback:true},
    command_contended_document_cardinality:
      {artifact:"cardinality_result", top_level_fallback:true},
    relation_pagination_reauthorization_no_leak:
      {artifact:"relation_policy_result", top_level_fallback:false},
    cross_workspace_relation_fail_closed:
      {artifact:"relation_policy_result", top_level_fallback:false},
    flow_search_accepted_index_frontier_and_stale:
      {artifact:"search_contract_result", top_level_fallback:false},
    flow_search_policy_cardinality_no_leak:
      {artifact:"search_contract_result", top_level_fallback:false},
    projection_lag_mcp_cli_policy_filtered_equivalence:
      {artifact:"mcp_cli_equivalence_result", top_level_fallback:false},
    legacy_search_contract_unchanged:
      {artifact:"search_contract_result", top_level_fallback:false},
    # ADR-0017: web face -> gates/vF-frontend-gate.yaml#web_semantic_equivalence_v0_5
    mcp_cli_semantic_equivalence:
      {artifact:"mcp_cli_equivalence_result", top_level_fallback:false},
    tool_registry_expected_119_or_rebased:
      {artifact:"mcp_cli_equivalence_result", top_level_fallback:false},
    audit_actor_origin_causation:
      {artifact:"audit_causation_result", top_level_fallback:false},
    flow_relation_move_event_registry_and_transport_causation:
      {artifact:"audit_causation_result", top_level_fallback:false}
  };

def flow_normalize_verdict($value):
  if $value == true or $value == "passed" then "passed"
  elif $value == false then "failed"
  elif ($value | type) == "object" then
    if $value.status == "passed" and $value.passed != false then "passed"
    elif (($value | has("status")) | not) and $value.passed == true then "passed"
    elif ($value.status | type) == "string" then
      if $value.status == "not_implemented" or $value.status == "not_verified" or $value.status == "not_run"
      then "not_covered"
      elif $value.status == "passed" or $value.status == "failed" or $value.status == "not_covered"
      then $value.status
      else "not_covered"
      end
    elif $value.passed == false then "failed"
    else "not_covered"
    end
  elif ($value | type) == "string" then
    if $value == "not_implemented" or $value == "not_verified" or $value == "not_run"
    then "not_covered"
    else $value
    end
  else "not_covered"
  end;

def flow_object_value($object; $key):
  if ($object | type) == "object" and ($object | has($key)) then $object[$key]
  else null
  end;

def flow_document_gate_verdict($document; $gate; $allow_top_level):
  (flow_object_value($document.hard_gates; $gate)) as $hard_gate
  | (flow_object_value($document.gates; $gate)) as $gate_entry
  | (flow_object_value($document; $gate)) as $top_gate
  | if $hard_gate != null then flow_normalize_verdict($hard_gate)
    elif $gate_entry != null then flow_normalize_verdict($gate_entry)
    elif $top_gate != null then flow_normalize_verdict($top_gate)
    elif $allow_top_level then flow_normalize_verdict($document.passed)
    else "not_covered"
    end;

# $input is produced independently by report/verify and contains file facts:
# {exists,valid_json,sha256,document,producer_status,producer_command,path}.
def flow_artifact_state($artifact; $input; $actual_head):
  ($input.document // {}) as $document
  | ($document.source_head // $document.source.head // "") as $source_head
  | (if ($document | has("source_dirty")) then $document.source_dirty
     elif ($document | has("source_tree_dirty")) then $document.source_tree_dirty
     elif (($document.source // null) | type) == "object" and ($document.source | has("dirty")) then $document.source.dirty
     else null
     end) as $source_dirty
  | (if ($input.exists | not) then
       if $input.producer_status == "available" and ($input.producer_executed_count // 0) > 0
       then "artifact_missing_after_execution"
       elif $input.producer_status == "producer_missing" then "producer_missing"
       elif $input.producer_status == "producer_unspecified" then "producer_unspecified"
       else "artifact_missing"
       end
     elif ($input.valid_json | not) then "artifact_malformed"
     elif $input.producer_status == "producer_missing" then "producer_missing"
     elif $input.producer_status == "producer_unspecified" then "producer_unspecified"
     elif $source_head == "" then "source_head_missing"
     elif $source_head != $actual_head then "source_head_mismatch"
     elif $source_dirty == null then "source_clean_unproven"
     elif $source_dirty != false then "source_dirty"
     # An artifact can be complete and honest while one of its observations is
     # failed/not_covered. Evidence integrity and gate success are separate.
     # The producer command's non-zero exit remains visible in checks[].
     else "passed_evidence"
     end) as $status
  | (flow_gate_wiring | to_entries | map(select(.value.artifact == $artifact))) as $owned
  | {
      status: $status,
      path: $input.path,
      sha256: ($input.sha256 // null),
      producer_status: $input.producer_status,
      producer_command: ($input.producer_command // null),
      producer_executed_count: ($input.producer_executed_count // 0),
      producer_execution_status: ($input.producer_execution_status // null),
      source_head: (if $source_head == "" then null else $source_head end),
      source_dirty: $source_dirty,
      passed: ($status == "passed_evidence"),
      gate_verdicts: (
        reduce $owned[] as $entry ({};
          .[$entry.key] = (
            if $status == "passed_evidence" then
              flow_document_gate_verdict($document; $entry.key; $entry.value.top_level_fallback)
            elif $status == "artifact_missing_after_execution" then "failed"
            else "not_covered"
            end
          )
        )
      )
    };

def flow_compute_artifact_states($inputs; $actual_head):
  reduce ($inputs | keys[]) as $artifact ({};
    .[$artifact] = flow_artifact_state($artifact; $inputs[$artifact]; $actual_head)
  );

def flow_compute_hard_gates($artifact_states):
  flow_gate_wiring
  | to_entries
  | map(. as $entry | {
      key: $entry.key,
      value: ($artifact_states[$entry.value.artifact].gate_verdicts[$entry.key] // "not_implemented")
    })
  | from_entries;

def flow_counts_by_status($values):
  reduce ($values | sort | group_by(.)[]) as $group ({}; .[$group[0]] = ($group | length));

def flow_required_boolean($object; $key; $path):
  if ($object | type) != "object" or (($object | has($key)) | not) then
    error($path + " is required")
  elif ($object[$key] | type) != "boolean" then
    error($path + " must be boolean")
  else
    $object[$key]
  end;

def flow_derive_receipt:
  ([.checks[]? | select(.status != "passed") |
      "check-not-passed:" + .id + ":" + .status]) as $check_blocking
  | ([.required_commands | to_entries[] |
      select(
        (.key != "verify" and .key != "gate" and .key != "manual_signoff")
        and .value.status != "passed"
      ) |
      "required-command-not-passed:" + .key + ":" + .value.status]) as $command_blocking
  | ([.artifact_states | to_entries[] | select(.value.status != "passed_evidence") |
      "artifact-not-passed:" + .key + ":" + .value.status]) as $artifact_blocking
  | ([.hard_gates | to_entries[] | select(.value != "passed") |
      "hard-gate-not-passed:" + .key + ":" + .value]) as $hard_gate_blocking
  | (if .predecessor.status == "accepted" then []
     else ["predecessor-not-accepted:" + .predecessor.status]
     end) as $predecessor_blocking
  | ([.budgets | to_entries[] | select(.value.frozen != true) |
      "budget-not-frozen:" + .key + ":" + .value.status]) as $budget_blocking
  | (flow_required_boolean(.source; "dirty"; "source.dirty")) as $source_dirty
  | (if $source_dirty then ["source-dirty"] else [] end) as $source_blocking
  | ([.manual_signoffs | to_entries[] | select(.value.status == "failed" or .value.status == "needs_rework") |
      "manual-signoff-blocking:" + .key + ":" + .value.status]) as $manual_blocking
  | ([.manual_signoffs | to_entries[] | select(.value.status == "pending") |
      "manual-signoff-pending:" + .key]) as $manual_pending
  | ($check_blocking + $command_blocking + $artifact_blocking + $hard_gate_blocking
     + $predecessor_blocking + $budget_blocking + $source_blocking + $manual_blocking) as $blocking
  | .counts.automated = (.checks | length)
  | .counts.passed = ([.checks[]? | select(.status == "passed")] | length)
  | .counts.failed = ([.checks[]? | select(.status == "failed")] | length)
  | .counts.checks_by_status = flow_counts_by_status([.checks[]? | .status])
  | .counts.artifacts_by_status = flow_counts_by_status([.artifact_states[] | .status])
  | .counts.hard_gates_by_status = flow_counts_by_status([.hard_gates[]])
  | .counts.hard_gates_total = (.hard_gates | length)
  | .counts.hard_gates_passed = ([.hard_gates[] | select(. == "passed")] | length)
  | .counts.manual_pending = ($manual_pending | length)
  | .counts.unresolved = (($blocking + $manual_pending) | length)
  | .blocking_reasons = $blocking
  | .pending_signoffs = $manual_pending
  | .blockers = ($blocking + $manual_pending)
  | .automation_passed = (($blocking | length) == 0)
  | .candidate_ready = .automation_passed
  | .gate_passed = ((($blocking + $manual_pending) | length) == 0)
  | .mode = (
      if .gate_passed then "accepted"
      elif .candidate_ready then "candidate"
      else "blocked"
      end
    );
