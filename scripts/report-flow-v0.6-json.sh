#!/usr/bin/env bash
set -euo pipefail

# Runs the v0.6 backend/REST/MCP/CLI producers and writes an honest gate receipt.
# UI rows moved by ADR-0017 are recorded as deferred, never passed. A stale source
# baseline or unaccepted v0.5 predecessor remains an explicit blocker.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT=""
GATE_YAML=""
MANUAL_SIGNOFFS_FROM=""
PREDECESSOR_GATE_RESULT=""
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/report-flow-v0.6-json.sh [OPTIONS]

Options:
  --evidence-root DIR   Default: <contracts-root>/evidence/v0.6
  --contracts-root DIR  Default: /opt/working/sylvode-flow
  --repo-root DIR       Default: this checkout
  --gate-yaml PATH      Default: <contracts-root>/gates/v0.6-gate.yaml
  --manual-signoffs-from PATH
                        Preserve the signed manual row from this receipt.
                        Default: <evidence-root>/gate-result.json
  --predecessor-gate-result PATH
                        v0.5 receipt. Default: sibling v0.5/gate-result.json
  --json                Accepted for command symmetry; output is always JSON
  -h, --help            Show help

Requires OPENPR_TEST_DATABASE_URL. Exit: 0 candidate-ready, 1 blocked/non-pass,
2 usage/tool/environment/malformed contract.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires DIR}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires DIR}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires DIR}"; shift 2 ;;
    --gate-yaml) GATE_YAML="${2:?--gate-yaml requires PATH}"; shift 2 ;;
    --manual-signoffs-from) MANUAL_SIGNOFFS_FROM="${2:?--manual-signoffs-from requires PATH}"; shift 2 ;;
    --predecessor-gate-result) PREDECESSOR_GATE_RESULT="${2:?--predecessor-gate-result requires PATH}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

for tool in cargo git jq python3 sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required command: $tool" >&2; exit 2; }
done
[[ -n "${OPENPR_TEST_DATABASE_URL:-}" ]] || { echo "FAIL: OPENPR_TEST_DATABASE_URL is required" >&2; exit 2; }
[[ -d "$REPO_ROOT/.git" ]] || { echo "FAIL: --repo-root is not a git checkout: $REPO_ROOT" >&2; exit 2; }
[[ -n "$EVIDENCE_ROOT" ]] || EVIDENCE_ROOT="$CONTRACTS_ROOT/evidence/v0.6"
[[ -n "$GATE_YAML" ]] || GATE_YAML="$CONTRACTS_ROOT/gates/v0.6-gate.yaml"
[[ -n "$MANUAL_SIGNOFFS_FROM" ]] || MANUAL_SIGNOFFS_FROM="$EVIDENCE_ROOT/gate-result.json"
[[ -n "$PREDECESSOR_GATE_RESULT" ]] || PREDECESSOR_GATE_RESULT="$(dirname "$EVIDENCE_ROOT")/v0.5/gate-result.json"
[[ -f "$GATE_YAML" ]] || { echo "FAIL: gate YAML not found: $GATE_YAML" >&2; exit 2; }
mkdir -p "$EVIDENCE_ROOT/logs"

python3 - "$REPO_ROOT" "$CONTRACTS_ROOT" "$EVIDENCE_ROOT" "$GATE_YAML" \
  "$MANUAL_SIGNOFFS_FROM" "$PREDECESSOR_GATE_RESULT" <<'PY'
import datetime as dt
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import time

repo, contracts, evidence, gate_yaml, manual_source, predecessor_path = map(
    lambda value: pathlib.Path(value).resolve(), sys.argv[1:]
)
head = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
gate_text = gate_yaml.read_text(encoding="utf-8")
repository_match = re.search(r"source_baseline:\s*\n\s+repository:\s*([^\s]+)", gate_text)
baseline_match = re.search(r"reviewed_head:\s*([0-9a-f]{40})", gate_text)
version_match = re.search(r"rust_workspace_version:\s*([^\s]+)", gate_text)
frontend_version_match = re.search(r"frontend_package_version:\s*([^\s]+)", gate_text)
gate_status_match = re.search(r"^status:\s*([^\s]+)", gate_text, re.M)
if not all([repository_match, baseline_match, version_match, frontend_version_match, gate_status_match]):
    raise SystemExit("FAIL: v0.6 gate source_baseline is malformed")
baseline_repository = repository_match.group(1)
baseline_head = baseline_match.group(1)
baseline_version = version_match.group(1)
baseline_frontend_version = frontend_version_match.group(1)
gate_status = gate_status_match.group(1)

def yaml_mapping_keys(text, section):
    keys = []
    inside = False
    for line in text.splitlines():
        if line == f"{section}:":
            inside = True
            continue
        if inside and line and not line[0].isspace():
            break
        if inside:
            match = re.match(r"^  ([A-Za-z0-9_]+):", line)
            if match:
                keys.append(match.group(1))
    return set(keys)

contract_hard_gate_keys = yaml_mapping_keys(gate_text, "hard_gates")
workspace_text = (repo / "Cargo.toml").read_text(encoding="utf-8")
workspace_version_match = re.search(r"\[workspace\.package\].*?\nversion\s*=\s*\"([^\"]+)\"", workspace_text, re.S)
if not workspace_version_match:
    raise SystemExit("FAIL: workspace package version not found")
workspace_version = workspace_version_match.group(1)
frontend_package = json.loads((repo / "frontend/package.json").read_text(encoding="utf-8"))
frontend_version = frontend_package.get("version")
if not isinstance(frontend_version, str) or not frontend_version:
    raise SystemExit("FAIL: frontend package version not found")
dirty_entries = subprocess.run(
    ["git", "-C", str(repo), "status", "--porcelain=v1", "--untracked-files=all", "--",
     "apps", "crates", "spikes", "migrations", ".cargo", "Cargo.toml", "Cargo.lock"],
    text=True, stdout=subprocess.PIPE, check=True,
).stdout.splitlines()

commands = {
    "collection_contract": ["cargo", "test", "-p", "api", "flow_collection_", "--", "--nocapture"],
    "atomic_embed": ["cargo", "test", "-p", "api", "flow_collection_atomic_create_", "--", "--nocapture"],
    "projection_rebuild": ["cargo", "test", "-p", "api", "typed_projection_rebuild_matches_canonical", "--", "--nocapture"],
    "forms_boundary": ["cargo", "test", "-p", "api", "flow_collection_forms_tables_untouched_scans_executable_sql_paths", "--", "--nocapture"],
    "schema_convergence": ["cargo", "test", "-p", "api", "schema_field_and_view_convergence_keeps_stable_ids", "--", "--nocapture"],
    "field_secrecy": ["cargo", "test", "-p", "api", "field_secrecy_client_crdt_denied_and_server_query_redacts_restricted_fields", "--", "--nocapture"],
    "collection_events": ["cargo", "test", "-p", "api", "collection_commands_emit_the_frozen_v0_6_semantic_event_family", "--", "--nocapture"],
    "metadata_redaction": ["cargo", "test", "-p", "api", "delivery_metadata_withholds_record_content_and_internal_replay_receipts", "--", "--nocapture"],
    "ticket_guard": ["cargo", "test", "-p", "api", "collection_and_record_documents_cannot_receive_collab_tickets", "--", "--nocapture"],
    "collection_archive_tier": ["cargo", "test", "-p", "api", "flow_collection_container_archive_tier_", "--", "--nocapture"],
    "mcp_cli": ["cargo", "test", "-p", "mcp-server", "--test", "flow_collections_e2e", "--", "--nocapture"],
    "tool_registry": ["cargo", "test", "-p", "mcp-server", "flow_v06_tools_match_the_repository_registry_baseline", "--", "--nocapture"],
    "capacity": [str(repo / "scripts/benchmark-flow-collections.sh"), "--records", "10000", "--evidence-root", str(evidence), "--repo-root", str(repo), "--json"],
    "cardinality": [str(repo / "scripts/verify-flow-cardinality-v0.6.sh"), "--adr", str(contracts / "decisions/ADR-0013-multi-document-atomicity.md"), "--since-release", "0.5", "--contracts-root", str(contracts), "--evidence-root", str(evidence), "--repo-root", str(repo), "--json"],
    "surface": [str(repo / "scripts/verify-flow-surface-coverage.sh"), "--release", "0.6", "--contracts-root", str(contracts), "--evidence-root", str(evidence), "--repo-root", str(repo), "--json"],
}

cargo_test_checks = {
    "collection_contract", "atomic_embed", "projection_rebuild", "forms_boundary",
    "schema_convergence", "field_secrecy", "collection_events", "metadata_redaction",
    "ticket_guard", "collection_archive_tier", "mcp_cli", "tool_registry",
}

def cargo_executed_tests(output):
    return sum(
        int(match.group(1)) + int(match.group(2))
        for match in re.finditer(
            r"^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed;",
            output,
            re.M,
        )
    )

def artifact_execution_count(check_id):
    artifact_fields = {
        "capacity": ("collection-10k-result.json", ("executed_tests",)),
        "cardinality": ("cardinality-result.json", ("new_commands_found",)),
        "surface": ("surface-coverage-result.json", ("counts", "matrix_rows")),
    }
    filename, fields = artifact_fields[check_id]
    try:
        value = json.loads((evidence / filename).read_text(encoding="utf-8"))
        for field in fields:
            value = value[field]
        return value if isinstance(value, int) and not isinstance(value, bool) and value >= 0 else 0
    except (OSError, KeyError, TypeError, json.JSONDecodeError):
        return 0

checks = []
for check_id, command in commands.items():
    started = time.monotonic()
    completed = subprocess.run(command, cwd=repo, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    duration_ms = round((time.monotonic() - started) * 1000)
    log_path = evidence / "logs" / f"{check_id}.log"
    log_path.write_text(completed.stdout, encoding="utf-8")
    skipped = "skipped: OPENPR_TEST_DATABASE_URL is not set" in completed.stdout
    executed_count = (
        cargo_executed_tests(completed.stdout)
        if check_id in cargo_test_checks
        else artifact_execution_count(check_id)
    )
    status = (
        "environment_unavailable"
        if skipped
        else ("passed" if completed.returncode == 0 and executed_count > 0 else "failed")
    )
    checks.append({
        "id": check_id,
        "status": status,
        "command": " ".join(command),
        "exit_code": completed.returncode,
        "duration_ms": duration_ms,
        "executed_count": executed_count,
        "log": f"logs/{check_id}.log",
        "sha256": hashlib.sha256(completed.stdout.encode()).hexdigest(),
    })

by_id = {item["id"]: item for item in checks}
def passed(*ids):
    return all(by_id[item]["status"] == "passed" for item in ids)
def verdict(*ids):
    return "passed" if passed(*ids) else "failed"

def gate_execution_count(*ids):
    return min(by_id[item]["executed_count"] for item in ids)

artifact_checks = {
    "collection-contract-result.json": "collection_contract",
    "atomic-embed-fault-result.json": "atomic_embed",
    "projection-rebuild-result.json": "projection_rebuild",
    "forms-boundary-result.json": "forms_boundary",
}
for filename, check_id in artifact_checks.items():
    item = by_id[check_id]
    artifact = {
        "schema_version": "sylvode.flow.check-result.v1", "release": "0.6.0", "source_head": head,
        "check": check_id, "status": item["status"], "passed": item["status"] == "passed",
        "command": item["command"], "duration_ms": item["duration_ms"], "log": item["log"],
        "executed_count": item["executed_count"], "log_sha256": item["sha256"],
    }
    (evidence / filename).write_text(json.dumps(artifact, sort_keys=True, indent=2) + "\n", encoding="utf-8")

hard_gates = {
    "rest_mcp_cli_surface_parity": verdict("surface", "mcp_cli"),
    "mcp_default_rest_coverage_three_adr_threat_exceptions_only": verdict("surface"),
    "standalone_collection_create_idempotent": verdict("collection_contract"),
    "embedded_collection_single_transaction": verdict("atomic_embed"),
    "embedded_collection_fault_injection_no_orphan": verdict("atomic_embed"),
    "command_contended_document_cardinality": verdict("cardinality"),
    "generic_record_create_rejected": verdict("collection_contract"),
    "schema_field_and_view_convergence": verdict("schema_convergence"),
    "typed_projection_rebuild_matches_canonical": verdict("projection_rebuild"),
    "ten_thousand_record_query_uses_index": verdict("capacity"),
    "field_secrecy_client_crdt_denied": verdict("field_secrecy"),
    "forms_tables_untouched": verdict("forms_boundary"),
    "mcp_cli_create_equivalence": verdict("mcp_cli"),
    "tool_registry_expected_122_or_rebased": verdict("tool_registry"),
    "collection_record_event_registry_and_redaction": verdict("collection_events", "metadata_redaction"),
    "collection_container_archive_tier": verdict("collection_archive_tier"),
}
if set(hard_gates) != contract_hard_gate_keys or len(hard_gates) != 16:
    raise SystemExit("FAIL: report hard-gate wiring does not exactly match the 16-key v0.6 contract")
hard_gate_execution_counts = {
    "rest_mcp_cli_surface_parity": gate_execution_count("surface", "mcp_cli"),
    "mcp_default_rest_coverage_three_adr_threat_exceptions_only": gate_execution_count("surface"),
    "standalone_collection_create_idempotent": gate_execution_count("collection_contract"),
    "embedded_collection_single_transaction": gate_execution_count("atomic_embed"),
    "embedded_collection_fault_injection_no_orphan": gate_execution_count("atomic_embed"),
    "command_contended_document_cardinality": gate_execution_count("cardinality"),
    "generic_record_create_rejected": gate_execution_count("collection_contract"),
    "schema_field_and_view_convergence": gate_execution_count("schema_convergence"),
    "typed_projection_rebuild_matches_canonical": gate_execution_count("projection_rebuild"),
    "ten_thousand_record_query_uses_index": gate_execution_count("capacity"),
    "field_secrecy_client_crdt_denied": gate_execution_count("field_secrecy"),
    "forms_tables_untouched": gate_execution_count("forms_boundary"),
    "mcp_cli_create_equivalence": gate_execution_count("mcp_cli"),
    "tool_registry_expected_122_or_rebased": gate_execution_count("tool_registry"),
    "collection_record_event_registry_and_redaction": gate_execution_count("collection_events", "metadata_redaction"),
    "collection_container_archive_tier": gate_execution_count("collection_archive_tier"),
}

predecessor_status = "missing"
if predecessor_path.is_file():
    try:
        predecessor = json.loads(predecessor_path.read_text(encoding="utf-8"))
        predecessor_status = "accepted" if predecessor.get("gate_passed") is True else "not_accepted"
    except (OSError, json.JSONDecodeError):
        predecessor_status = "malformed"

source_clean = not dirty_entries
source_baseline_matches = (
    str(repo) == baseline_repository
    and head == baseline_head
    and workspace_version == baseline_version
    and frontend_version == baseline_frontend_version
)
automation_passed = all(value == "passed" for value in hard_gates.values())
candidate_ready = (automation_passed and source_clean and source_baseline_matches
                   and predecessor_status == "accepted" and gate_status != "planned")
manual = {"field_secrecy_denial": {"status": "pending", "reviewer": "", "evidence": ""}}
if manual_source.is_file():
    try:
        prior = json.loads(manual_source.read_text(encoding="utf-8"))
        candidate_manual = prior.get("manual_signoffs")
    except (OSError, json.JSONDecodeError) as exc:
        raise SystemExit(f"FAIL: manual-signoff source is malformed: {exc}")
    if not isinstance(candidate_manual, dict) or set(candidate_manual) != {"field_secrecy_denial"}:
        raise SystemExit("FAIL: manual-signoff source has the wrong v0.6 keys")
    row = candidate_manual["field_secrecy_denial"]
    if (
        not isinstance(row, dict)
        or row.get("status") not in {"pending", "passed", "failed", "needs_rework"}
        or not isinstance(row.get("reviewer"), str)
        or not isinstance(row.get("evidence"), str)
        or ("signed_at" in row and not isinstance(row["signed_at"], str))
    ):
        raise SystemExit("FAIL: manual-signoff source contains a malformed row")
    manual = candidate_manual
accepted = candidate_ready and manual["field_secrecy_denial"]["status"] == "passed"
blockers = []
if not automation_passed: blockers.append("automated_hard_gate_failed")
if not source_clean: blockers.append("source_dirty")
if not source_baseline_matches: blockers.append("source_baseline_mismatch")
if predecessor_status != "accepted": blockers.append(f"predecessor_{predecessor_status}")
if gate_status == "planned": blockers.append("gate_contract_planned")
if manual["field_secrecy_denial"]["status"] != "passed":
    blockers.append(f"manual_field_secrecy_denial_{manual['field_secrecy_denial']['status']}")

artifacts = {}
for key, relative in {
    "collection_contract_result": "collection-contract-result.json",
    "atomic_embed_fault_result": "atomic-embed-fault-result.json",
    "projection_rebuild_result": "projection-rebuild-result.json",
    "capacity_result": "collection-10k-result.json",
    "forms_boundary_result": "forms-boundary-result.json",
    "cardinality_result": "cardinality-result.json",
    "surface_coverage_result": "surface-coverage-result.json",
}.items():
    path = evidence / relative
    artifacts[key] = {
        "path": f"evidence/v0.6/{relative}",
        "exists": path.is_file(),
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None,
    }

counts = {
    "automated": len(hard_gates),
    "passed": sum(value == "passed" for value in hard_gates.values()),
    "failed": sum(value != "passed" for value in hard_gates.values()),
    "manual_pending": 1 if manual["field_secrecy_denial"]["status"] == "pending" else 0,
    "manual_deferred_to_frontend_track": 3,
    "unresolved": len(blockers),
}
result = {
    "schema_version": "sylvode.flow.gate-result.v1",
    "schema_path": "gates/v0.6-gate.yaml",
    "release": "0.6.0",
    "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
    "source_baseline": {"repository": baseline_repository, "reviewed_head": baseline_head,
                        "rust_workspace_version": baseline_version,
                        "frontend_package_version": baseline_frontend_version},
    "source": {"head": head, "rust_workspace_version": workspace_version,
               "frontend_package_version": frontend_version, "dirty": not source_clean,
               "dirty_entries": dirty_entries},
    "gate_contract": {"path": str(gate_yaml), "sha256": hashlib.sha256(gate_yaml.read_bytes()).hexdigest(),
                      "status": gate_status},
    "checks": checks,
    "artifacts": artifacts,
    "hard_gates": hard_gates,
    "hard_gate_execution_counts": hard_gate_execution_counts,
    "predecessor": {"release": "0.5.0", "status": predecessor_status, "path": str(predecessor_path)},
    "manual_signoffs": manual,
    "deferred_to_frontend_track": ["table_board", "schema_mode", "large_collection"],
    "automation_passed": automation_passed,
    "candidate_ready": candidate_ready,
    "accepted": accepted,
    "gate_passed": accepted,
    "mode": "accepted" if accepted else "blocked",
    "counts": counts,
    "blockers": blockers,
}
fd, temp_name = tempfile.mkstemp(prefix=".gate-result.", suffix=".json", dir=evidence)
with os.fdopen(fd, "w", encoding="utf-8") as handle:
    json.dump(result, handle, ensure_ascii=False, sort_keys=True, indent=2)
    handle.write("\n")
os.replace(temp_name, evidence / "gate-result.json")
print(json.dumps(result, ensure_ascii=False, sort_keys=True))
raise SystemExit(0 if candidate_ready else 1)
PY
