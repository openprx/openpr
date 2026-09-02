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
import subprocess
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

# Ledger invariant: duplicate identifiers would be collapsed by the output
# dictionary and make a quoted denominator larger than the number of distinct
# decisions. report/verify additionally require this exact key set to equal the
# YAML and JSON-schema ledgers before accepting a receipt.
if len(ALL_HARD_GATES) != len(set(ALL_HARD_GATES)):
    raise RuntimeError("ALL_HARD_GATES contains duplicate identifiers")


def load_json(path: str):
    if not os.path.isfile(path):
        return None
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, json.JSONDecodeError):
        return None


def validate_deployed_binary_provenance(
    evidence_root: str, repo_root: str, gates: dict, reasons: dict
) -> bool:
    """Independently reject a deployed artifact whose binary identity is absent or stale."""
    path = os.path.join(evidence_root, "deployed-chain-websocket-result.json")
    data = load_json(path)
    if not isinstance(data, dict):
        return False

    failures = []
    self_test = data.get("self_test")
    if not isinstance(self_test, dict):
        failures.append("binary provenance self-test record missing")
    elif self_test.get("ran") is not True:
        failures.append("binary provenance self-test did not run")
    else:
        if self_test.get("exit") != 0:
            failures.append(f"binary provenance self-test exit is {self_test.get('exit')!r}")
        duration_ms = self_test.get("duration_ms")
        if not isinstance(duration_ms, int) or isinstance(duration_ms, bool) or duration_ms < 0:
            failures.append("binary provenance self-test duration is missing or malformed")
    provenance = data.get("binary_provenance")
    if not isinstance(provenance, dict):
        failures.append("binary_provenance object missing")
        provenance = {}
    binary = provenance.get("binary")
    metadata = provenance.get("build_metadata")
    source_head = data.get("source_head")
    current = subprocess.run(
        ["git", "-C", repo_root, "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        check=False,
    )
    current_head = current.stdout.strip() if current.returncode == 0 else None
    current_status = subprocess.run(
        ["git", "-C", repo_root, "status", "--porcelain"],
        capture_output=True,
        text=True,
        check=False,
    )

    if not isinstance(source_head, str) or re.fullmatch(r"[0-9a-f]{40}", source_head) is None:
        failures.append("artifact source_head missing or malformed")
    elif source_head != current_head:
        failures.append("artifact source_head does not match verifier repo HEAD")
    if current_status.returncode != 0 or current_status.stdout.strip():
        failures.append("verifier source tree is dirty or cleanliness is unproven")
    if data.get("source_dirty") is not False:
        failures.append("artifact does not prove a clean source tree")
    if provenance.get("schema_version") != "openpr.flow.binary-provenance.v1":
        failures.append("binary provenance schema missing or wrong")
    if provenance.get("status") != "passed" or provenance.get("passed") is not True:
        failures.append(f"binary provenance status is {provenance.get('status')!r}")
    if not isinstance(binary, dict) or binary.get("available") is not True:
        failures.append("deployed binary unavailable")
    elif not isinstance(binary.get("sha256"), str) or re.fullmatch(r"[0-9a-f]{64}", binary["sha256"]) is None:
        failures.append("deployed binary sha256 missing or malformed")
    if provenance.get("build_metadata_available") is not True or not isinstance(metadata, dict):
        failures.append("build metadata unavailable")
    else:
        if metadata.get("schema_version") != "openpr.build-info.v1":
            failures.append("build metadata schema missing or wrong")
        if metadata.get("source") not in ("git", "environment"):
            failures.append("build metadata source unknown")
        if (
            not isinstance(metadata.get("git_committer_date"), str)
            or re.fullmatch(
                r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})",
                metadata["git_committer_date"],
            )
            is None
        ):
            failures.append("build metadata committer date missing or malformed")
        if metadata.get("git_dirty") is not False:
            failures.append("binary clean build is unproven")
        if metadata.get("git_commit") != source_head:
            failures.append("binary commit does not match artifact source_head")
        if metadata.get("git_commit") != current_head:
            failures.append("binary commit does not match verifier repo HEAD")
    provenance_check = next(
        (
            item
            for item in data.get("checks", [])
            if isinstance(item, dict) and item.get("name") == "binary_provenance_matches_source_head"
        ),
        None,
    )
    if not isinstance(provenance_check, dict) or provenance_check.get("passed") is not True:
        failures.append("binary provenance check missing or failed")

    gate_name = "deployed_chain_websocket_upgrade"
    if failures:
        gates[gate_name] = "failed"
        reasons[gate_name] = f"{os.path.basename(path)}: " + "; ".join(failures)
        return False
    return True


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


def bridge_verifier_gates(
    evidence_root: str,
    filename: str,
    expected_schema_version: str,
    gates: dict,
    reasons: dict,
    *,
    conjoin: bool = False,
) -> bool:
    """Read a per-verifier evidence artifact and copy each hard gate's own
    verdict into `gates`/`reasons` verbatim -- never re-deriving or softening
    a verdict the verifier already computed.

    Two artifact shapes are accepted, because the verifiers were written
    against two conventions and neither is wrong:

    - `gates`: hard_gate id -> {"status": "passed"|"failed"|"not_covered", ...}
      (flow-events-result.json, collab-architecture-result.json). An entry may
      carry a `reason`/`note` used as the recorded detail.
    - `hard_gates`: hard_gate id -> "passed"|"failed"|"not_covered"
      (limits-result.json, error-contract-result.json). Detail, when present,
      comes from a sibling `hard_gate_reasons` object keyed by the same ids.

    Exactly one of the two keys must be present; an artifact carrying both is
    rejected rather than silently preferring one, since the two could disagree.

    Returns False (no-op) if the artifact file is simply absent -- the gates
    it would have backed are left at whatever they already are (the
    ALL_HARD_GATES default of "not_verified" unless another bridge already
    set them), which is the correct "nobody verified this yet" state.

    Raises EvidenceFormatError -- never silently ignored, never downgraded
    to not_verified -- if the file exists but is not valid JSON, is not a
    JSON object, has the wrong schema_version, carries neither or both of
    the `gates`/`hard_gates` objects, or any entry in that object that names
    a known hard gate is missing a `status` field or reports a status outside
    BRIDGE_GATE_STATUSES.

    Deliberately does NOT hardcode which hard gate ids this artifact backs:
    it iterates whatever keys the artifact's own gate object contains,
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

    has_gates = isinstance(data.get("gates"), dict)
    has_hard_gates = isinstance(data.get("hard_gates"), dict)
    if has_gates and has_hard_gates:
        raise EvidenceFormatError(
            f"{path}: carries both a 'gates' and a 'hard_gates' object; "
            "which one is authoritative is undefined, so this is refused rather than guessed"
        )
    if not has_gates and not has_hard_gates:
        raise EvidenceFormatError(f"{path}: neither a 'gates' nor a 'hard_gates' JSON object is present")

    gate_key = "gates" if has_gates else "hard_gates"
    gates_obj = data[gate_key]
    sibling_reasons = data.get("hard_gate_reasons") if gate_key == "hard_gates" else None
    if sibling_reasons is not None and not isinstance(sibling_reasons, dict):
        raise EvidenceFormatError(f"{path}: 'hard_gate_reasons' is present but is not a JSON object")

    for gate_name, gate_entry in gates_obj.items():
        if gate_name not in ALL_HARD_GATES:
            # Not one of the 52 hard_gates this contract tracks -- the
            # artifact may legitimately carry auxiliary detail under
            # `gates` that isn't itself a hard-gate id (none do today, but
            # nothing here requires the artifact's gate set to be a subset
            # of ALL_HARD_GATES a priori). Not an error; just not bridged.
            continue
        if gate_key == "gates":
            if not isinstance(gate_entry, dict) or "status" not in gate_entry:
                raise EvidenceFormatError(f"{path}: gates.{gate_name} is missing a 'status' field")
            status = gate_entry["status"]
            detail = gate_entry.get("reason") or gate_entry.get("note")
        else:
            if not isinstance(gate_entry, str):
                raise EvidenceFormatError(
                    f"{path}: hard_gates.{gate_name} is {type(gate_entry).__name__}, expected a status string"
                )
            status = gate_entry
            detail = sibling_reasons.get(gate_name) if sibling_reasons else None
        if status not in BRIDGE_GATE_STATUSES:
            raise EvidenceFormatError(
                f"{path}: {gate_key}.{gate_name} status={status!r} is not one of {sorted(BRIDGE_GATE_STATUSES)}"
            )
        artifact_reason = f"{filename}: {gate_key}.{gate_name}={status}" + (f" -- {detail}" if detail else "")
        if conjoin and gates.get(gate_name) != "not_verified":
            previous = gates[gate_name]
            if "failed" in (previous, status):
                combined = "failed"
            elif "not_covered" in (previous, status):
                combined = "not_covered"
            else:
                combined = "passed"
            previous_reason = reasons.get(gate_name, f"prior evidence={previous}")
            gates[gate_name] = combined
            reasons[gate_name] = f"{previous_reason}; AND {artifact_reason}"
        else:
            gates[gate_name] = status
            reasons[gate_name] = artifact_reason

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
        implementation_parity = sc.get("implementation_parity", {})
        version_scope_issues = []
        scope = implementation_parity.get("scope", {}) if isinstance(implementation_parity, dict) else {}
        diagnostic = implementation_parity.get("version_scope_diagnostic", {}) if isinstance(implementation_parity, dict) else {}
        if scope.get("release") != sc.get("release"):
            version_scope_issues.append("implementation parity did not bind its release filter to artifact.release")
        if diagnostic.get("code") != "future_contract_entries_excluded_from_current_release_failure":
            version_scope_issues.append("missing verifier_criterion_version_scope diagnostic")
        for surface in ("mcp", "rest", "cli"):
            dimension = implementation_parity.get(surface, {}) if isinstance(implementation_parity, dict) else {}
            if not isinstance(dimension, dict):
                version_scope_issues.append(f"{surface} implementation parity dimension is malformed")
                continue
            declared = set(dimension.get("contract_declared", []))
            required = set(dimension.get("contract_required", []))
            future = {item.get("name") for item in dimension.get("not_yet_in_release", []) if isinstance(item, dict)}
            conditional = {item.get("name") for item in dimension.get("conditional_not_applicable", []) if isinstance(item, dict)}
            implementation = set(dimension.get("implementation", []))
            missing = set(dimension.get("contract_missing_in_implementation", []))
            if declared != required | future | conditional or required & (future | conditional):
                version_scope_issues.append(f"{surface} release classification is incomplete or overlapping")
            if not required or dimension.get("required_set_non_empty") is not True:
                version_scope_issues.append(f"{surface} current-release required set is empty")
            if missing != required - implementation:
                version_scope_issues.append(f"{surface} missing set is not current-release-required minus implementation")
        parity_keys = [
            "missing_rest_rows", "duplicate_rest_rows", "unknown_matrix_rest_rows",
            "orphan_mcp_tools", "orphan_mcp_resources", "orphan_cli_commands",
            "unknown_mcp_refs", "unknown_cli_refs", "unknown_ui_consumers",
            "blank_cells", "version_inversions", "future_exposure_counted_as_shipped",
            "empty_contract_required_surfaces",
            "contract_mcp_missing_live", "contract_rest_missing_implementation",
            "contract_cli_missing_implementation",
        ]
        parity_violations = sum(len(v.get(k, [])) for k in parity_keys) + len(version_scope_issues)
        set_gate(
            "rest_mcp_cli_ui_surface_parity",
            parity_violations == 0,
            (
                f"{parity_violations} surface-coverage violations across parity-relevant categories; "
                f"version_scope_issues={version_scope_issues}"
                if parity_violations
                else "0 violations across all parity-relevant categories; future contract entries classified not_yet_in_release and non-failing"
            ),
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
        concurrency = card.get("concurrency_fixtures", {})
        fixtures = concurrency.get("fixtures", {}) if isinstance(concurrency, dict) else {}
        required_fixtures = {
            "concurrent_same_idempotency_key",
            "different_keys_same_lineage",
            "preallocated_uuid_conflict",
            "lost_response_retry",
        }
        fixture_statuses_ok = required_fixtures == set(fixtures) and all(
            isinstance(fixtures.get(name), dict) and fixtures[name].get("status") == "passed"
            for name in required_fixtures
        )
        negative_reuse_ok = (
            fixtures.get("concurrent_same_idempotency_key", {})
            .get("negative_different_body", {})
            .get("code")
            == 409
        )
        response_convergence_ok = fixtures.get("concurrent_same_idempotency_key", {}).get(
            "all_response_object_ids_canonical"
        ) is True
        preallocated = fixtures.get("preallocated_uuid_conflict", {})
        preallocated_rollback_ok = (
            preallocated.get("collision_exit_code") not in (None, 0)
            and preallocated.get("constraint_error_observed") is True
            and preallocated.get("marker_epoch_before") == preallocated.get("marker_epoch_after")
            and preallocated.get("canonical_row_count") == 1
        )
        concurrency_ok = (
            isinstance(concurrency, dict)
            and concurrency.get("status") == "passed"
            and len(concurrency.get("violations", [])) == 0
            and fixture_statuses_ok
            and negative_reuse_ok
            and response_convergence_ok
            and preallocated_rollback_ok
        )
        ok = static_ok and static_violation_count == 0 and dynamic_ok and concurrency_ok
        set_gate(
            "command_contended_document_cardinality",
            ok,
            f"static_ok={static_ok} dynamic_ok={dynamic_ok} concurrency_status={concurrency.get('status')} "
            f"four_fixtures={fixture_statuses_ok} negative_reuse={negative_reuse_ok} "
            f"response_convergence={response_convergence_ok} preallocated_rollback={preallocated_rollback_ok}",
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
        collection_status = inv.get("collection_status")
        if collection_status == "failed":
            failures = [
                f"{entry.get('kind')}:{entry.get('reason_code')}"
                for entry in inv.get("environments", [])
                if entry.get("status") == "failed"
            ]
            set_gate(
                "legacy_pages_inventory_three_environments_complete",
                False,
                f"collector produced durable failure evidence; failed environments={failures}",
            )
            for g in (
                "legacy_pages_zero_or_importer_surface_available",
                "legacy_pages_mcp_admin_policy_and_semantic_equivalence",
                "legacy_pages_dry_run_and_rerun_idempotent",
                "legacy_pages_failure_source_immutable",
                "legacy_pages_lineage_complete",
            ):
                reasons[g] = "inventory collection failed; zero/nonzero branch is intentionally unknown"
        else:
            kinds = [e.get("kind") for e in inv.get("environments", [])]
            complete = (
                collection_status == "complete"
                and sorted(kinds) == ["development", "target_deployment", "test"]
                and all(e.get("status") == "collected" for e in inv.get("environments", []))
            )
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
                f"collection_status={collection_status} kinds={sorted(kinds)} dist_sums_ok={dist_ok} total_rows={total_rows} computed_total={computed_total}",
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

    # ---- authz-baseline: backs both of its gates ----
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
        # The artifact carries its own verdict for this gate. Copying only its
        # `reason` and leaving `gates` untouched silently collapsed an explicit
        # `not_covered` into the module-wide `not_verified` default -- exactly
        # what BRIDGE_GATE_STATUSES says a bridge must never do.
        member_baseline = authz.get("member_baseline_no_behaviour_regression", {})
        if not isinstance(member_baseline, dict) or "status" not in member_baseline:
            raise EvidenceFormatError(
                f"{authz_path}: member_baseline_no_behaviour_regression is missing a 'status' field"
            )
        mb_status = member_baseline["status"]
        if mb_status not in BRIDGE_GATE_STATUSES:
            raise EvidenceFormatError(
                f"{authz_path}: member_baseline_no_behaviour_regression status={mb_status!r} "
                f"is not one of {sorted(BRIDGE_GATE_STATUSES)}"
            )
        if mb_status == "passed":
            timeline = member_baseline.get("timeline", {})
            writes = member_baseline.get("writes", {})
            negatives = member_baseline.get("negative_controls", {})
            fixture = member_baseline.get("fixture", {})
            required_phases = {"before", "after_edit", "after_archive", "after_restore"}
            surface_keys = {"rest", "mcp_http", "mcp_sse", "mcp_stdio", "cli"}
            timeline_ok = required_phases == set(timeline) and all(
                surface_keys <= set((timeline.get(phase, {}).get("normalized_objects", {})))
                for phase in required_phases
            )
            lifecycle_writes_ok = all((writes.get(name) or {}).get("code") == 0 for name in ("archive", "restore"))
            negatives_ok = (
                (negatives.get("view_archive_denied") or {}).get("code") == 403
                and negatives.get("view_denial_no_success_event") is True
                and negatives.get("view_denial_status") == "active"
                and (negatives.get("archive_expected_frontier_denied") or {}).get("code") != 0
                and negatives.get("frontier_denial_status") == "active"
            )
            member_pass_evidence_ok = (
                fixture.get("default_member_level") == "edit"
                and fixture.get("inherit_from_parent_values") == "true"
                and len(member_baseline.get("violations", [])) == 0
                and timeline_ok
                and lifecycle_writes_ok
                and negatives_ok
            )
            if not member_pass_evidence_ok:
                mb_status = "failed"
        gates["member_baseline_no_behaviour_regression"] = mb_status
        mb_detail = member_baseline.get("reason")
        reasons["member_baseline_no_behaviour_regression"] = (
            f"authz-baseline-result.json: member_baseline_no_behaviour_regression={mb_status}"
            + (f" -- {mb_detail}" if mb_detail else "")
        )

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

    # ---- limits / error-contract: both emit their verdicts under `hard_gates`
    # (flat id -> status strings) rather than `gates`. They were previously
    # unmapped here, so the 8 hard gates they decide sat at "not_verified"
    # regardless of what the verifiers actually found. Bridged verbatim, same
    # as the two above -- see bridge_verifier_gates() docstring.
    bridge_verifier_gates(evidence_root, "limits-result.json", "sylvode.flow.limits-result.v1", gates, reasons)
    bridge_verifier_gates(evidence_root, "error-contract-result.json", "sylvode.flow.error-contract-result.v1", gates, reasons)

    # The success-path REST artifact is independently required in addition to
    # the surface matrix and error mapping artifact.  It strengthens (never
    # overwrites) both relevant gates: a markdown-complete matrix cannot stand
    # in for one live REST/MCP/CLI object, and error-shape coverage cannot stand
    # in for successful ApiResponse envelopes.  Absence is not allowed to
    # inherit an earlier pass from either weaker artifact.
    if not bridge_verifier_gates(
        evidence_root,
        "rest-contract-result.json",
        "sylvode.flow.rest-contract-result.v1",
        gates,
        reasons,
        conjoin=True,
    ):
        missing = os.path.join(evidence_root, "rest-contract-result.json")
        for gate_name in ("rest_mcp_cli_ui_surface_parity", "rest_envelope_and_error_contract"):
            gates[gate_name] = "not_verified"
            reasons[gate_name] = f"missing/unreadable required live success-path artifact {missing}"

    # Likewise, architecture's in-process restart test is necessary but not
    # sufficient for the frozen document-integrity criterion.  Require the
    # independent PostgreSQL replay/projection/API-process-restart artifact as
    # a conjunct of snapshot_tail_restart_recovery.
    if not bridge_verifier_gates(
        evidence_root,
        "document-integrity-result.json",
        "sylvode.flow.document-integrity-result.v1",
        gates,
        reasons,
        conjoin=True,
    ):
        missing = os.path.join(evidence_root, "document-integrity-result.json")
        gates["snapshot_tail_restart_recovery"] = "not_verified"
        reasons["snapshot_tail_restart_recovery"] = (
            f"missing/unreadable required live canonical replay/restart artifact {missing}"
        )

    # ---- deployed three-hop WebSocket verifier: backs 1 gate ----
    # A missing/unreachable real deployment is written as an explicit failed
    # verdict. The bridge copies it verbatim; wiring can never make the gate
    # green without all 14 live checks passing. Then independently re-check
    # the binary digest/build metadata/source-HEAD relation so hand-editing the
    # producer's gate status cannot turn stale deployment evidence green.
    bridge_verifier_gates(
        evidence_root,
        "deployed-chain-websocket-result.json",
        "sylvode.flow.deployed-chain-websocket-result.v1",
        gates,
        reasons,
    )
    validate_deployed_binary_provenance(evidence_root, repo_root, gates, reasons)

    # ---- frontend v0.4 aggregate: backs 4 web gates ----
    # Per-gate status is based only on failed automated checks. Named skips
    # remain in the artifact as manual-signoff work and are never counted as
    # passed checks by the producer.
    bridge_verifier_gates(
        evidence_root,
        "ui-e2e-result.json",
        "sylvode.flow.ui-e2e-result.v1",
        gates,
        reasons,
    )

    # ---- live feature-flag verifier: backs both rollout gates ----
    # The contract's historical cargo integration-test target does not exist;
    # feature-flag-result.json records that mismatch and derives both verdicts
    # from live API/MCP HTTP+SSE+stdio/CLI/browser observations. A transport or
    # environment failure is an explicit failed verdict, never a skip/pass.
    bridge_verifier_gates(
        evidence_root,
        "feature-flag-result.json",
        "sylvode.flow.feature-flag-result.v1",
        gates,
        reasons,
    )

    # ---- forms regression: backs exactly 1 gate ----
    # scripts/verify-flow-forms-regression-v0.4.sh runs the repository's
    # existing Universal Forms CI gate bundle (scripts/ci-universal-forms-
    # gates.sh -- the same entrypoint CI uses) and writes forms-regression-
    # result.json carrying that run's exact command, exit code, duration,
    # assertion counts, log checksum and its own passed/failed verdict for
    # forms_regression_no_degradation under `hard_gates`. The report used to
    # execute the same bundle once in its generic section and once through this
    # artifact producer; that was duplicate execution, not a second hard-gate
    # key. Only this artifact-producing run remains canonical. The bundle had been
    # passing every round while nobody recorded it, which is why this gate sat
    # at not_verified. Bridged verbatim -- see bridge_verifier_gates()
    # docstring; absent artifact still means not_verified, never passed.
    bridge_verifier_gates(evidence_root, "forms-regression-result.json", "sylvode.flow.forms-regression-result.v1", gates, reasons)
    # ---- contract-surface verifier bridges (W8): 5 gates, one artifact each ----
    # Each of these scripts recomputes its own hard gate's verdict from a live
    # observation -- the shipped binary's registry, a real three-transport
    # JSON-RPC conversation, a real concurrent write race, a real forward
    # migration run, real CLI process exits -- and writes it under its own
    # top-level `gates` object. Bridged verbatim by bridge_verifier_gates():
    # a malformed artifact raises EvidenceFormatError, an absent one leaves the
    # gate at not_verified, and a verifier-reported `failed` is never softened.
    #
    #   migration-result.json     -> migration_forward_and_rollback_strategy
    #   document-seq-result.json  -> document_row_lock_seq_unique
    #   mcp-contract-result.json  -> mcp_three_transport_contract
    #   tool-registry-result.json -> tool_registry_expected_107_or_rebased
    #   cli-contract-result.json  -> cli_json_and_exit_code_contract
    bridge_verifier_gates(evidence_root, "migration-result.json", "sylvode.flow.migration-result.v1", gates, reasons)
    bridge_verifier_gates(evidence_root, "document-seq-result.json", "sylvode.flow.document-seq-result.v1", gates, reasons)
    bridge_verifier_gates(evidence_root, "mcp-contract-result.json", "sylvode.flow.mcp-contract-result.v1", gates, reasons)
    bridge_verifier_gates(evidence_root, "tool-registry-result.json", "sylvode.flow.tool-registry-result.v1", gates, reasons)
    bridge_verifier_gates(evidence_root, "cli-contract-result.json", "sylvode.flow.cli-contract-result.v1", gates, reasons)
    # ---- transport-auth verifier bridge: backs 3 gates ----
    # scripts/verify-flow-transport-auth-v0.4.sh writes transport-auth-
    # result.json with its own per-gate verdict for
    # ticket_single_use_origin_bot_exclusion, secure_cookie_and_local_dev_
    # guard and unauthorized_update_rejected. Every one of those verdicts is
    # decided by live negative fixtures (bot issuance, replay, wrong Origin,
    # wrong client_id, expiry, a mid-session revocation, a non-loopback
    # insecure-cookie config) each paired with a positive control, so the
    # artifact's `passed`/`failed` is a real observation, not a self-report
    # about coverage. Bridged verbatim -- see bridge_verifier_gates().
    bridge_verifier_gates(evidence_root, "transport-auth-result.json", "sylvode.flow.transport-auth-result.v1", gates, reasons)

    # ---- cross-workspace / policy-bypass verifier bridge: backs 1 gate ----
    # scripts/verify-flow-cross-workspace-v0.4.sh writes cross-workspace-
    # negative-result.json with its verdict for
    # cross_workspace_and_policy_bypass_negative, decided by live
    # cross-workspace get/update/link/ticket refusals and the policy-bypass
    # negatives (bot scoping, read-only bot, bot on user-only CRDT surfaces,
    # disabled feature flag, absent/forged credentials), each with a
    # same-workspace control. Bridged verbatim -- see bridge_verifier_gates().
    bridge_verifier_gates(evidence_root, "cross-workspace-negative-result.json", "sylvode.flow.cross-workspace-result.v1", gates, reasons)

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
