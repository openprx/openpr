#!/usr/bin/env python3
"""Sylvode Flow v0.4 hard-gate recomputation.

Contract: /opt/working/sylvode-flow/gates/gate-commands.md ("verify" role)
and /opt/working/sylvode-flow/gates/v0.4-gate.yaml's 52 hard_gates.

This module NEVER trusts a gate-result.json's own self-reported
`hard_gates` block. It independently recomputes each hard gate's verdict
from the evidence artifact(s) that back it, re-validating the artifact's
own internal violation/passed fields rather than taking a top-level
`passed:true` at face value. A hard gate this module has no mapped
evidence artifact for is always "not_verified" -- never silently
defaulted to "passed".

Usage: python3 flow_gate_v0_4_recompute.py --evidence-root DIR --repo-root DIR
Prints {"hard_gates": {...52 keys...}, "reasons": {...}} as JSON.
"""
from __future__ import annotations

import glob
import hashlib
import json
import os
import re
import sys


ALL_HARD_GATES = [
    "rest_mcp_cli_ui_surface_parity",
    "mcp_default_rest_coverage_three_adr_threat_exceptions_only",
    "migration_forward_and_rollback_strategy",
    "document_row_lock_seq_unique",
    "collab_architecture_adr_accepted",
    "bounded_warm_cache_lock_hold_and_round_trip_budgets",
    "minimal_snapshot_advancement_bounds_tail",
    "snapshot_tail_restart_recovery",
    "bootstrap_repeatable_read_and_ws_parity",
    "accepted_egress_seq_monotonic_and_gap_resync",
    "rest_envelope_and_error_contract",
    "server_draining_reason_cross_surface_error_coverage",
    "ticket_single_use_origin_bot_exclusion",
    "secure_cookie_and_local_dev_guard",
    "cross_workspace_and_policy_bypass_negative",
    "unauthorized_update_rejected",
    "mcp_three_transport_contract",
    "tool_registry_expected_107_or_rebased",
    "cli_json_and_exit_code_contract",
    "web_ime_undo_selection_and_sync_state",
    "navigator_keyboard_drag_equivalence",
    "i18n_zh_en_flow_key_parity",
    "vite_wasm_static_build_and_deep_route",
    "feature_flag_navigation_and_direct_url",
    "feature_flag_mcp_read_admin_write_and_cli_equivalence",
    "forms_regression_no_degradation",
    "flow_limits_exact_boundary_and_plus_one_rejection",
    "isolated_decode_apply_cpu_wall_memory",
    "websocket_rate_connection_and_backpressure_limits",
    "deployed_chain_websocket_upgrade",
    "bootstrap_limits_web_server_parity",
    "limit_exceeded_kind_coverage",
    "flow_event_registry_payload_policy_complete",
    "business_event_dispatch_same_transaction",
    "dispatch_expansion_snapshot_semantics",
    "no_subscribers_terminalized_and_reaped",
    "dispatcher_liveness_and_backlog",
    "flow_content_delivery_coalescing",
    "coalescing_seal_and_source_first_expansion",
    "dispatch_numeric_budgets_locked",
    "command_contended_document_cardinality",
    "integrity_record_on_fail_closed",
    "flow_parent_authority_in_postgres",
    "member_baseline_no_behaviour_regression",
    "event_idempotency_audit_and_redaction",
    "legacy_pages_inventory_three_environments_complete",
    "legacy_pages_zero_or_importer_surface_available",
    "legacy_pages_mcp_admin_policy_and_semantic_equivalence",
    "legacy_pages_dry_run_and_rerun_idempotent",
    "legacy_pages_failure_source_immutable",
    "legacy_pages_lineage_complete",
    "legacy_pages_drop_requires_separate_adr",
]


def load_json(path: str):
    if not os.path.isfile(path):
        return None
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, json.JSONDecodeError):
        return None


class EvidenceFormatError(Exception):
    """Raised when an evidence artifact exists but does not conform to the
    shape this module's bridge for it requires. Per the "don't silently
    ignore a malformed artifact" contract, a caller catching this must
    exit non-zero -- never fall back to treating the gates it would have
    backed as not_verified."""


# The only three verdicts a per-gate verifier bridge may report for a hard
# gate. `not_covered` ("this verifier explicitly determined the requirement
# cannot be exercised in this repository, e.g. it needs a load-test harness
# that does not exist") is distinct from the module-wide default
# `not_verified` ("no verifier artifact is mapped/present for this gate at
# all") -- the schema (docs/schemas/sylvode-flow-gate-v0.4.schema.json)
# carries all four as valid `hard_gates.<gate>` values; a bridge must never
# collapse one into another.
BRIDGE_GATE_STATUSES = {"passed", "failed", "not_covered"}


def bridge_verifier_gates(evidence_root: str, filename: str, expected_schema_version: str, gates: dict, reasons: dict) -> bool:
    """Read a per-verifier evidence artifact (flow-events-result.json /
    collab-architecture-result.json shaped: a top-level `gates` object
    mapping hard_gate id -> {"status": "passed"|"failed"|"not_covered", ...})
    and copy each hard gate's own verdict into `gates`/`reasons` verbatim --
    never re-deriving or softening a verdict the verifier already computed.

    Returns False (no-op) if the artifact file is simply absent -- the gates
    it would have backed are left at whatever they already are (the
    ALL_HARD_GATES default of "not_verified" unless another bridge already
    set them), which is the correct "nobody verified this yet" state.

    Raises EvidenceFormatError -- never silently ignored, never downgraded
    to not_verified -- if the file exists but is not valid JSON, is not a
    JSON object, has the wrong schema_version, lacks a `gates` object, or
    any entry in that object that names a known hard gate is missing a
    `status` field or reports a status outside BRIDGE_GATE_STATUSES.

    Deliberately does NOT hardcode which hard gate ids this artifact backs:
    it iterates whatever keys the artifact's own `gates` object contains,
    so a future verifier revision that adds/drops a gate needs no change
    here.
    """
    path = os.path.join(evidence_root, filename)
    if not os.path.isfile(path):
        return False

    try:
        with open(path, encoding="utf-8") as f:
            raw = f.read()
    except OSError as e:
        raise EvidenceFormatError(f"{path}: could not read file ({e})") from e
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as e:
        raise EvidenceFormatError(f"{path}: not valid JSON ({e})") from e
    if not isinstance(data, dict):
        raise EvidenceFormatError(f"{path}: top-level JSON value is not an object")

    actual_schema_version = data.get("schema_version")
    if actual_schema_version != expected_schema_version:
        raise EvidenceFormatError(
            f"{path}: schema_version={actual_schema_version!r}, expected {expected_schema_version!r}"
        )

    gates_obj = data.get("gates")
    if not isinstance(gates_obj, dict):
        raise EvidenceFormatError(f"{path}: 'gates' field is missing or not a JSON object")

    for gate_name, gate_entry in gates_obj.items():
        if gate_name not in ALL_HARD_GATES:
            # Not one of the 52 hard_gates this contract tracks -- the
            # artifact may legitimately carry auxiliary detail under
            # `gates` that isn't itself a hard-gate id (none do today, but
            # nothing here requires the artifact's gate set to be a subset
            # of ALL_HARD_GATES a priori). Not an error; just not bridged.
            continue
        if not isinstance(gate_entry, dict) or "status" not in gate_entry:
            raise EvidenceFormatError(f"{path}: gates.{gate_name} is missing a 'status' field")
        status = gate_entry["status"]
        if status not in BRIDGE_GATE_STATUSES:
            raise EvidenceFormatError(
                f"{path}: gates.{gate_name}.status={status!r} is not one of {sorted(BRIDGE_GATE_STATUSES)}"
            )
        gates[gate_name] = status
        detail = gate_entry.get("reason") or gate_entry.get("note")
        reasons[gate_name] = f"{filename}: gates.{gate_name}.status={status}" + (f" -- {detail}" if detail else "")

    return True


def recompute(evidence_root: str, repo_root: str) -> dict:
    gates: dict[str, str] = {g: "not_verified" for g in ALL_HARD_GATES}
    reasons: dict[str, str] = {}

    def set_gate(name: str, ok: bool, reason: str) -> None:
        gates[name] = "passed" if ok else "failed"
        reasons[name] = reason

    # ---- surface coverage: backs 2 gates ----
    sc_path = os.path.join(evidence_root, "surface-coverage-result.json")
    sc = load_json(sc_path)
    if sc is None:
        reasons["rest_mcp_cli_ui_surface_parity"] = f"missing/unreadable {sc_path}"
        reasons["mcp_default_rest_coverage_three_adr_threat_exceptions_only"] = f"missing/unreadable {sc_path}"
    else:
        v = sc.get("violations", {})
        parity_keys = [
            "missing_rest_rows", "duplicate_rest_rows", "unknown_matrix_rest_rows",
            "orphan_mcp_tools", "orphan_mcp_resources", "orphan_cli_commands",
            "unknown_mcp_refs", "unknown_cli_refs", "unknown_ui_consumers",
            "blank_cells", "version_inversions", "future_exposure_counted_as_shipped",
        ]
        parity_violations = sum(len(v.get(k, [])) for k in parity_keys)
        set_gate(
            "rest_mcp_cli_ui_surface_parity",
            parity_violations == 0,
            f"{parity_violations} surface-coverage violations across parity-relevant categories" if parity_violations else "0 violations across all parity-relevant categories",
        )
        exception_keys = ["invalid_reason_codes", "mcp_exception_endpoint_mismatches", "mcp_exception_without_authority"]
        exception_violations = sum(len(v.get(k, [])) for k in exception_keys)
        mcp_not_exposed = sc.get("counts", {}).get("not_exposed", {}).get("mcp")
        exception_ok = exception_violations == 0 and mcp_not_exposed == 3
        set_gate(
            "mcp_default_rest_coverage_three_adr_threat_exceptions_only",
            exception_ok,
            f"exception_violations={exception_violations} mcp_not_exposed_count={mcp_not_exposed} (want 0 and 3)",
        )

    # ---- cardinality: backs 1 gate ----
    card_path = os.path.join(evidence_root, "cardinality-result.json")
    card = load_json(card_path)
    if card is None:
        reasons["command_contended_document_cardinality"] = f"missing/unreadable {card_path}"
    else:
        static_ok = card.get("static_check", {}).get("passed") is True
        static_violation_count = len(card.get("static_check", {}).get("violations", []))
        dynamic_ok = card.get("dynamic_check", {}).get("status") == "passed"
        concurrency_ok = card.get("concurrency_fixtures", {}).get("status") == "passed"
        ok = static_ok and static_violation_count == 0 and dynamic_ok and concurrency_ok
        set_gate(
            "command_contended_document_cardinality",
            ok,
            f"static_ok={static_ok} dynamic_ok={dynamic_ok} concurrency_status={card.get('concurrency_fixtures', {}).get('status')}",
        )

    # ---- legacy pages: inventory completeness + zero/nonzero branch + 4 conditional gates ----
    inv_path = os.path.join(evidence_root, "legacy-pages-inventory.json")
    inv = load_json(inv_path)
    if inv is None:
        reasons["legacy_pages_inventory_three_environments_complete"] = f"missing/unreadable {inv_path}"
        reasons["legacy_pages_zero_or_importer_surface_available"] = f"missing/unreadable {inv_path}"
        for g in (
            "legacy_pages_mcp_admin_policy_and_semantic_equivalence",
            "legacy_pages_dry_run_and_rerun_idempotent",
            "legacy_pages_failure_source_immutable",
            "legacy_pages_lineage_complete",
        ):
            reasons[g] = f"missing/unreadable {inv_path}"
    else:
        kinds = [e.get("kind") for e in inv.get("environments", [])]
        complete = sorted(kinds) == ["development", "target_deployment", "test"]
        total_rows = inv.get("total_rows")
        dist_ok = True
        for e in inv.get("environments", []):
            dist_sum = sum(d.get("row_count", 0) for d in e.get("workspace_distribution", []))
            if dist_sum != e.get("row_count"):
                dist_ok = False
        computed_total = sum(e.get("row_count", 0) for e in inv.get("environments", []))
        total_ok = total_rows == computed_total
        set_gate(
            "legacy_pages_inventory_three_environments_complete",
            complete and dist_ok and total_ok,
            f"kinds={sorted(kinds)} dist_sums_ok={dist_ok} total_rows={total_rows} computed_total={computed_total}",
        )

        if total_rows == 0:
            set_gate("legacy_pages_zero_or_importer_surface_available", True, "total_rows=0: zero branch, no importer surface required")
            for g in (
                "legacy_pages_mcp_admin_policy_and_semantic_equivalence",
                "legacy_pages_dry_run_and_rerun_idempotent",
                "legacy_pages_failure_source_immutable",
                "legacy_pages_lineage_complete",
            ):
                set_gate(g, True, "total_rows=0: legacy-pages-import-v1.md permits auto-pass with detail not_required_zero_inventory")
        elif total_rows is not None and total_rows > 0:
            import_result_path = os.path.join(evidence_root, "legacy-pages-import-result.json")
            import_result = load_json(import_result_path)
            set_gate(
                "legacy_pages_zero_or_importer_surface_available",
                import_result is not None,
                f"total_rows={total_rows} > 0 (nonzero branch): importer artifact {'present' if import_result else 'MISSING: ' + import_result_path}",
            )
            for g in (
                "legacy_pages_mcp_admin_policy_and_semantic_equivalence",
                "legacy_pages_dry_run_and_rerun_idempotent",
                "legacy_pages_failure_source_immutable",
                "legacy_pages_lineage_complete",
            ):
                set_gate(g, False, "nonzero branch requires the importer implementation + evidence, not present this round")
        else:
            reasons["legacy_pages_zero_or_importer_surface_available"] = "total_rows field missing/invalid"

    # ---- integrity-records: backs 1 gate ----
    ir_path = os.path.join(evidence_root, "integrity-records-result.json")
    ir = load_json(ir_path)
    if ir is None:
        reasons["integrity_record_on_fail_closed"] = f"missing/unreadable {ir_path}"
    else:
        set_gate(
            "integrity_record_on_fail_closed",
            ir.get("passed") is True and len(ir.get("violations", [])) == 0,
            f"live cross_workspace_relation fixture: passed={ir.get('passed')} violations={len(ir.get('violations', []))}",
        )

    # ---- authz-baseline: backs 1 of its 2 gates (member_baseline is explicitly not_covered by the artifact itself) ----
    authz_path = os.path.join(evidence_root, "authz-baseline-result.json")
    authz = load_json(authz_path)
    if authz is None:
        reasons["flow_parent_authority_in_postgres"] = f"missing/unreadable {authz_path}"
    else:
        parent_authority = authz.get("flow_parent_authority_in_postgres", {})
        set_gate(
            "flow_parent_authority_in_postgres",
            parent_authority.get("passed") is True and len(parent_authority.get("violations", [])) == 0,
            f"static+live checks: passed={parent_authority.get('passed')} violations={len(parent_authority.get('violations', []))}",
        )
        member_baseline = authz.get("member_baseline_no_behaviour_regression", {})
        reasons["member_baseline_no_behaviour_regression"] = member_baseline.get("reason", "not covered by any verifier this round")

    # ---- events/dispatch verifier bridge: backs 8 gates ----
    # scripts/verify-flow-events-v0.4.sh writes flow-events-result.json with
    # its own independently-recomputed per-gate verdict (passed/failed) for
    # business_event_dispatch_same_transaction, dispatch_expansion_snapshot_
    # semantics, no_subscribers_terminalized_and_reaped, dispatcher_liveness_
    # and_backlog, flow_content_delivery_coalescing, coalescing_seal_and_
    # source_first_expansion, flow_event_registry_payload_policy_complete,
    # event_idempotency_audit_and_redaction. Bridged verbatim -- see
    # bridge_verifier_gates() docstring.
    bridge_verifier_gates(evidence_root, "flow-events-result.json", "sylvode.flow.events-result.v1", gates, reasons)

    # ---- collab-architecture verifier bridge: backs 6 gates ----
    # scripts/verify-flow-collab-architecture.sh writes collab-architecture-
    # result.json with its own independently-recomputed per-gate verdict
    # (passed/failed/not_covered) for collab_architecture_adr_accepted,
    # bounded_warm_cache_lock_hold_and_round_trip_budgets, minimal_snapshot_
    # advancement_bounds_tail, snapshot_tail_restart_recovery, bootstrap_
    # repeatable_read_and_ws_parity, accepted_egress_seq_monotonic_and_gap_
    # resync (the last of which it itself reports not_covered -- bridged
    # as not_covered here too, never rounded up to passed or down to
    # not_verified). Bridged verbatim -- see bridge_verifier_gates() docstring.
    bridge_verifier_gates(evidence_root, "collab-architecture-result.json", "sylvode.flow.collab-architecture-result.v1", gates, reasons)

    # ---- legacy_pages_drop_requires_separate_adr: static migration scan ----
    migrations_dir = os.path.join(repo_root, "migrations")
    drop_found = []
    if os.path.isdir(migrations_dir):
        for path in sorted(glob.glob(os.path.join(migrations_dir, "*.sql"))):
            with open(path, encoding="utf-8", errors="replace") as f:
                text = f.read()
            for m in re.finditer(r"DROP\s+TABLE[^;]*\bpages\b", text, re.IGNORECASE):
                drop_found.append(f"{os.path.basename(path)}: {m.group(0)[:80]}")
        set_gate(
            "legacy_pages_drop_requires_separate_adr",
            len(drop_found) == 0,
            "no migration drops `pages`" if not drop_found else f"pages dropped without a separate ADR: {drop_found}",
        )
    else:
        reasons["legacy_pages_drop_requires_separate_adr"] = f"migrations dir not found: {migrations_dir}"

    for g in ALL_HARD_GATES:
        if g not in reasons:
            reasons[g] = "no verifier artifact mapped/produced this round" if gates[g] == "not_verified" else reasons.get(g, "")

    return {"hard_gates": gates, "reasons": reasons}


def main() -> int:
    import argparse

    ap = argparse.ArgumentParser()
    ap.add_argument("--evidence-root", required=True)
    ap.add_argument("--repo-root", required=True)
    args = ap.parse_args()
    try:
        result = recompute(args.evidence_root, args.repo_root)
    except EvidenceFormatError as e:
        print(f"FAIL: evidence artifact does not conform to its expected shape: {e}", file=sys.stderr)
        print("Fix: regenerate the artifact with its producing verify-flow-*.sh script, or correct it by hand; "
              "this module refuses to guess a gate verdict from a malformed artifact.", file=sys.stderr)
        return 2
    json.dump(result, sys.stdout, indent=2, sort_keys=True)
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
