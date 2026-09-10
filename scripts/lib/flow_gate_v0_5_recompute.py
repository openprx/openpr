#!/usr/bin/env python3
"""Independently recompute the Sylvode Flow v0.5 evidence ledger.

This intentionally does not import or execute flow_gate_v0_5_receipt_state.jq.
The report and verifier therefore have separate implementations of YAML
enumeration, producer wiring, artifact provenance, and hard-gate verdict
normalization.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from typing import Any


WIRING: dict[str, tuple[str, bool]] = {
    "rest_mcp_cli_surface_parity": ("surface_coverage_result", True),
    "mcp_default_rest_coverage_three_adr_threat_exceptions_only": ("surface_coverage_result", True),
    "ten_client_convergence": ("convergence_result", True),
    "ten_client_round_trip_and_lock_budget_no_regression": ("collab_architecture_result", False),
    "accepted_egress_duplicate_reorder_gap_resync": ("collab_architecture_result", False),
    "warm_cache_eviction_restart_semantic_equivalence": ("collab_architecture_result", False),
    "token_expiry_and_permission_revocation": ("authz_result", False),
    "permission_inheritance_and_break": ("authz_result", False),
    "authz_linearization_no_escalation": ("authz_result", False),
    "permission_cache_is_not_authority": ("authz_result", False),
    "authz_numeric_budgets_locked": ("authz_result", False),
    "move_subtree_nodes_max_locked": ("multi_document_result", False),
    "permission_revocation_closes_subtree_sessions": ("authz_result", False),
    "search_and_relation_use_effective_permission": ("authz_result", False),
    "authz_boundary_self_lockout_guarded": ("authz_result", False),
    "admin_bot_does_not_bypass_object_boundary": ("authz_result", False),
    "archive_tier_by_object_scope": ("authz_result", False),
    "multi_document_lock_order_and_atomicity": ("multi_document_result", False),
    "navigator_tombstone_growth_measured": ("collab_architecture_result", False),
    "invalid_update_named_reason_producer_consumer_contract": ("invalid_update_reason_result", True),
    "command_contended_document_cardinality": ("cardinality_result", True),
    "relation_pagination_reauthorization_no_leak": ("relation_policy_result", False),
    "cross_workspace_relation_fail_closed": ("relation_policy_result", False),
    "flow_search_accepted_index_frontier_and_stale": ("search_contract_result", False),
    "flow_search_policy_cardinality_no_leak": ("search_contract_result", False),
    "projection_lag_mcp_cli_policy_filtered_equivalence": ("mcp_cli_equivalence_result", False),
    "legacy_search_contract_unchanged": ("search_contract_result", False),
    "mcp_cli_semantic_equivalence": ("mcp_cli_equivalence_result", False),
    "tool_registry_expected_119_or_rebased": ("mcp_cli_equivalence_result", False),
    "audit_actor_origin_causation": ("audit_causation_result", False),
    "flow_relation_move_event_registry_and_transport_causation": ("audit_causation_result", False),
}

COMMAND_FOR_ARTIFACT = {
    "surface_coverage_result": "surface_parity",
    "collab_architecture_result": "collab_architecture_verify",
    "authz_result": "authz_verify",
    "multi_document_result": "multi_document_verify",
    "cardinality_result": "cardinality_verify",
    "invalid_update_reason_result": "invalid_update_reason_verify",
    "audit_causation_result": "audit_causation_verify",
}


def yaml_map(path: str, section: str) -> dict[str, str]:
    result: dict[str, str] = {}
    inside = False
    with open(path, encoding="utf-8") as handle:
        for raw in handle:
            line = raw.rstrip("\n")
            if line == f"{section}:":
                inside = True
                continue
            if inside and line and not line[0].isspace() and not line.startswith("#"):
                break
            match = re.match(r"^  ([A-Za-z0-9_]+):\s*(.*?)\s*(?:#.*)?$", line)
            if inside and match:
                result[match.group(1)] = match.group(2).strip()
    return result


def load_json(path: str) -> tuple[bool, Any]:
    if not os.path.isfile(path):
        return False, None
    try:
        with open(path, encoding="utf-8") as handle:
            value = json.load(handle)
        return isinstance(value, dict), value
    except (OSError, json.JSONDecodeError):
        return False, None


def sha256(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def normalize(value: Any) -> str:
    if value is True or value == "passed":
        return "passed"
    if value is False or value == "failed":
        return "failed"
    if value in ("not_covered", "not_implemented", "not_verified", "not_run"):
        return "not_covered"
    if isinstance(value, dict):
        if value.get("status") == "passed" and value.get("passed") is not False:
            return "passed"
        if "status" not in value and value.get("passed") is True:
            return "passed"
        if value.get("status") in ("failed", "not_covered"):
            return value["status"]
        if value.get("passed") is False:
            return "failed"
    return "not_covered"


def raw_gate(document: dict[str, Any], gate: str, fallback: bool) -> Any:
    for container_name in ("hard_gates", "gates"):
        container = document.get(container_name)
        if isinstance(container, dict) and gate in container:
            return container[gate]
    if gate in document:
        return document[gate]
    return document.get("passed") if fallback else None


def reason_codes(document: dict[str, Any], gate: str, verdict: str) -> list[str]:
    values: list[str] = []
    detail = document.get("gate_details", {}).get(gate) if isinstance(document.get("gate_details"), dict) else None
    entry = document.get("gates", {}).get(gate) if isinstance(document.get("gates"), dict) else None
    hard = document.get("hard_gates", {}).get(gate) if isinstance(document.get("hard_gates"), dict) else None
    candidates = [detail, entry, hard]
    for candidate in candidates:
        if not isinstance(candidate, dict):
            continue
        for key in ("reason_code", "reason"):
            value = candidate.get(key)
            if isinstance(value, str) and value and value not in values:
                values.append(value)
        list_key = "not_covered_reason_codes" if verdict == "not_covered" else "reason_codes"
        value = candidate.get(list_key)
        if isinstance(value, list):
            for item in value:
                if isinstance(item, str) and item and item not in values:
                    values.append(item)
    return values


def git_output(repo_root: str, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", repo_root, *args], capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or "git command failed")
    return result.stdout.strip()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--evidence-root", required=True)
    parser.add_argument("--repo-root", required=True)
    parser.add_argument("--gate-yaml", required=True)
    args = parser.parse_args()

    artifacts = yaml_map(args.gate_yaml, "artifacts")
    commands = yaml_map(args.gate_yaml, "required_commands")
    yaml_gates = yaml_map(args.gate_yaml, "hard_gates")
    if set(yaml_gates) != set(WIRING) or len(WIRING) != 31:
        raise RuntimeError("independent 31-gate wiring does not match v0.5-gate.yaml")

    head = git_output(args.repo_root, "rev-parse", "HEAD")
    states: dict[str, Any] = {}
    gates: dict[str, str] = {}
    reasons: dict[str, list[str]] = {}

    # jq's `keys[]` traversal is lexicographic; preserve that order so the
    # independently derived blocker arrays are byte-for-byte reproducible.
    for artifact in sorted(artifacts):
        canonical = artifacts[artifact]
        if artifact == "gate_result":
            continue
        command_key = COMMAND_FOR_ARTIFACT.get(artifact)
        command = commands.get(command_key, "") if command_key else ""
        if artifact == "convergence_result":
            command = "scripts/verify-flow-convergence-v0.5.sh --clients 10 --json"
        script = command.split(maxsplit=1)[0] if command else ""
        if not command:
            producer_status = "producer_unspecified"
        elif not os.path.isfile(os.path.join(args.repo_root, script)):
            producer_status = "producer_missing"
        else:
            producer_status = "available"

        path = os.path.join(args.evidence_root, os.path.basename(canonical))
        exists = os.path.isfile(path)
        valid, document = load_json(path)
        artifact_sha = sha256(path) if exists else None
        document = document if isinstance(document, dict) else {}
        source_head = document.get("source_head")
        if not isinstance(source_head, str):
            source = document.get("source")
            source_head = source.get("head") if isinstance(source, dict) else None
        if "source_dirty" in document:
            source_dirty = document.get("source_dirty")
        elif "source_tree_dirty" in document:
            source_dirty = document.get("source_tree_dirty")
        else:
            source = document.get("source")
            source_dirty = source.get("dirty") if isinstance(source, dict) else None

        owned = [(gate, fallback) for gate, (owner, fallback) in WIRING.items() if owner == artifact]
        explicit = {
            gate: normalize(raw_gate(document, gate, fallback)) for gate, fallback in owned
        }
        if not exists:
            status = producer_status if producer_status != "available" else "artifact_missing"
        elif not valid:
            status = "artifact_malformed"
        elif producer_status != "available":
            status = producer_status
        elif source_head is None:
            status = "source_head_missing"
        elif source_head != head:
            status = "source_head_mismatch"
        elif source_dirty is None:
            status = "source_clean_unproven"
        elif source_dirty is not False:
            status = "source_dirty"
        elif any(verdict not in ("passed", "failed", "not_covered") for verdict in explicit.values()):
            status = "artifact_verdict_missing"
        else:
            # Evidence completeness is independent from whether the observations passed.
            status = "passed_evidence"

        state_verdicts: dict[str, str] = {}
        for gate, _fallback in owned:
            if status == "passed_evidence":
                verdict = explicit[gate]
                why = reason_codes(document, gate, verdict)
                if not why and verdict != "passed":
                    why = ["artifact_reported_failure" if verdict == "failed" else "artifact_did_not_cover_gate"]
            else:
                verdict = "not_covered"
                why = [status]
            state_verdicts[gate] = verdict
            gates[gate] = verdict
            reasons[gate] = why

        states[artifact] = {
            "status": status,
            "path": canonical,
            "sha256": artifact_sha,
            "producer_status": producer_status,
            "producer_command": command or None,
            "source_head": source_head,
            "source_dirty": source_dirty,
            "passed": status == "passed_evidence",
            "gate_verdicts": state_verdicts,
        }

    output = {
        "source_head": head,
        "artifact_states": states,
        "hard_gates": {gate: gates[gate] for gate in WIRING},
        "reasons": {gate: reasons[gate] for gate in WIRING},
    }
    print(json.dumps(output, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError, ValueError) as error:
        print(f"FAIL: v0.5 independent recomputation failed: {error}", file=sys.stderr)
        sys.exit(2)
