#!/usr/bin/env bash
set -euo pipefail

# Independent v0.6 receipt verifier. It reruns no product action: it reloads
# source/contract/predecessor state, verifies every log and artifact digest,
# and independently recomputes the 16 hard gates and all derived receipt fields.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT=""
GATE_YAML=""
PREDECESSOR_GATE_RESULT=""
RESULT_PATH=""
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-v0.6-json.sh GATE_RESULT_JSON [OPTIONS]

Options:
  --evidence-root DIR   Default: directory containing GATE_RESULT_JSON
  --contracts-root DIR  Default: /opt/working/sylvode-flow
  --repo-root DIR       Default: this checkout
  --gate-yaml PATH      Default: <contracts-root>/gates/v0.6-gate.yaml
  --predecessor-gate-result PATH
                        v0.5 receipt. Default: sibling v0.5/gate-result.json
  --json                Accepted; output is always JSON
  -h, --help            Show help

Exit: 0 consistent candidate-ready receipt, 1 drift/non-pass, 2 malformed/tool/usage.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires DIR}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires DIR}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires DIR}"; shift 2 ;;
    --gate-yaml) GATE_YAML="${2:?--gate-yaml requires PATH}"; shift 2 ;;
    --predecessor-gate-result) PREDECESSOR_GATE_RESULT="${2:?--predecessor-gate-result requires PATH}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)
      [[ -z "$RESULT_PATH" ]] || { echo "FAIL: unexpected argument: $1" >&2; exit 2; }
      RESULT_PATH="$1"; shift ;;
  esac
done

[[ -n "$RESULT_PATH" ]] || { echo "FAIL: GATE_RESULT_JSON is required" >&2; exit 2; }
for tool in git python3; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required command: $tool" >&2; exit 2; }
done
[[ -f "$RESULT_PATH" ]] || { echo "FAIL: gate result missing: $RESULT_PATH" >&2; exit 2; }
[[ -n "$EVIDENCE_ROOT" ]] || EVIDENCE_ROOT="$(cd "$(dirname "$RESULT_PATH")" && pwd)"
[[ -n "$GATE_YAML" ]] || GATE_YAML="$CONTRACTS_ROOT/gates/v0.6-gate.yaml"
[[ -n "$PREDECESSOR_GATE_RESULT" ]] || PREDECESSOR_GATE_RESULT="$(dirname "$EVIDENCE_ROOT")/v0.5/gate-result.json"
[[ -f "$GATE_YAML" ]] || { echo "FAIL: gate YAML missing: $GATE_YAML" >&2; exit 2; }

python3 - "$RESULT_PATH" "$EVIDENCE_ROOT" "$REPO_ROOT" "$CONTRACTS_ROOT" "$GATE_YAML" \
  "$PREDECESSOR_GATE_RESULT" <<'PY'
import hashlib
import json
import pathlib
import re
import subprocess
import sys

result_path, evidence, repo, contracts, gate_yaml, predecessor_path = map(
    lambda value: pathlib.Path(value).resolve(), sys.argv[1:]
)
try:
    receipt = json.loads(result_path.read_text(encoding="utf-8"))
except (OSError, json.JSONDecodeError) as exc:
    print(json.dumps({"receipt_consistent": False, "malformed": True, "errors": [str(exc)]}))
    raise SystemExit(2)
if not isinstance(receipt, dict):
    print(json.dumps({"receipt_consistent": False, "malformed": True, "errors": ["top level must be object"]}))
    raise SystemExit(2)

drift = []
def same(label, observed, expected):
    if observed != expected:
        drift.append({"field": label, "observed": observed, "expected": expected})

same("schema_version", receipt.get("schema_version"), "sylvode.flow.gate-result.v1")
same("schema_path", receipt.get("schema_path"), "gates/v0.6-gate.yaml")
same("release", receipt.get("release"), "0.6.0")
gate_text = gate_yaml.read_text(encoding="utf-8")
baseline_repository_match = re.search(r"source_baseline:\s*\n\s+repository:\s*([^\s]+)", gate_text)
baseline_head_match = re.search(r"reviewed_head:\s*([0-9a-f]{40})", gate_text)
baseline_version_match = re.search(r"rust_workspace_version:\s*([^\s]+)", gate_text)
baseline_frontend_version_match = re.search(r"frontend_package_version:\s*([^\s]+)", gate_text)
gate_status_match = re.search(r"^status:\s*([^\s]+)", gate_text, re.M)
if not all([baseline_repository_match, baseline_head_match, baseline_version_match,
            baseline_frontend_version_match, gate_status_match]):
    print(json.dumps({"receipt_consistent": False, "malformed": True, "errors": ["gate YAML baseline/status malformed"]}))
    raise SystemExit(2)
baseline_repository = baseline_repository_match.group(1)
baseline_head = baseline_head_match.group(1)
baseline_version = baseline_version_match.group(1)
baseline_frontend_version = baseline_frontend_version_match.group(1)
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
same("source_baseline", receipt.get("source_baseline"),
     {"repository": baseline_repository, "reviewed_head": baseline_head,
      "rust_workspace_version": baseline_version,
      "frontend_package_version": baseline_frontend_version})
same("gate_contract.sha256", receipt.get("gate_contract", {}).get("sha256"), hashlib.sha256(gate_yaml.read_bytes()).hexdigest())
same("gate_contract.status", receipt.get("gate_contract", {}).get("status"), gate_status)

head = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
workspace = (repo / "Cargo.toml").read_text(encoding="utf-8")
version_match = re.search(r"\[workspace\.package\].*?\nversion\s*=\s*\"([^\"]+)\"", workspace, re.S)
if not version_match:
    print(json.dumps({"receipt_consistent": False, "malformed": True, "errors": ["workspace version missing"]}))
    raise SystemExit(2)
workspace_version = version_match.group(1)
frontend_package = json.loads((repo / "frontend/package.json").read_text(encoding="utf-8"))
frontend_version = frontend_package.get("version")
if not isinstance(frontend_version, str) or not frontend_version:
    print(json.dumps({"receipt_consistent": False, "malformed": True, "errors": ["frontend version missing"]}))
    raise SystemExit(2)
dirty_entries = subprocess.run(
    ["git", "-C", str(repo), "status", "--porcelain=v1", "--untracked-files=all", "--",
     "apps", "crates", "spikes", "migrations", ".cargo", "Cargo.toml", "Cargo.lock"],
    text=True, stdout=subprocess.PIPE, check=True,
).stdout.splitlines()
same("source", receipt.get("source"), {
    "head": head, "rust_workspace_version": workspace_version,
    "frontend_package_version": frontend_version,
    "dirty": bool(dirty_entries), "dirty_entries": dirty_entries,
})

expected_check_ids = {
    "collection_contract", "atomic_embed", "projection_rebuild", "forms_boundary",
    "schema_convergence", "field_secrecy", "collection_events", "metadata_redaction",
    "ticket_guard", "collection_archive_tier", "mcp_cli", "tool_registry", "capacity", "cardinality", "surface",
}
checks = receipt.get("checks")
if not isinstance(checks, list) or any(not isinstance(item, dict) for item in checks):
    print(json.dumps({"receipt_consistent": False, "malformed": True, "errors": ["checks must be object array"]}))
    raise SystemExit(2)
by_id = {item.get("id"): item for item in checks}
same("checks.keys", set(by_id), expected_check_ids)
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

for check_id, item in by_id.items():
    log = evidence / str(item.get("log", ""))
    if not log.is_file():
        drift.append({"field": f"checks.{check_id}.log", "observed": "missing", "expected": "present"})
        continue
    log_bytes = log.read_bytes()
    same(f"checks.{check_id}.sha256", item.get("sha256"), hashlib.sha256(log_bytes).hexdigest())
    log_text = log_bytes.decode("utf-8", errors="replace")
    executed_count = (
        cargo_executed_tests(log_text)
        if check_id in cargo_test_checks
        else artifact_execution_count(check_id)
    )
    same(f"checks.{check_id}.executed_count", item.get("executed_count"), executed_count)
    expected_status = "passed" if item.get("exit_code") == 0 and executed_count > 0 else "failed"
    if b"skipped: OPENPR_TEST_DATABASE_URL is not set" in log_bytes:
        expected_status = "environment_unavailable"
    same(f"checks.{check_id}.status", item.get("status"), expected_status)

def passed(*ids):
    return all(by_id.get(item, {}).get("status") == "passed" for item in ids)
def verdict(*ids):
    return "passed" if passed(*ids) else "failed"
def gate_execution_count(*ids):
    return min(by_id.get(item, {}).get("executed_count", 0) for item in ids)
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
same("hard_gates.keys", set(hard_gates), contract_hard_gate_keys)
if len(hard_gates) != 16:
    drift.append({"field": "hard_gates.count", "observed": len(hard_gates), "expected": 16})
same("hard_gates", receipt.get("hard_gates"), hard_gates)
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
same("hard_gate_execution_counts", receipt.get("hard_gate_execution_counts"), hard_gate_execution_counts)

artifact_relatives = {
    "collection_contract_result": "collection-contract-result.json",
    "atomic_embed_fault_result": "atomic-embed-fault-result.json",
    "projection_rebuild_result": "projection-rebuild-result.json",
    "capacity_result": "collection-10k-result.json",
    "forms_boundary_result": "forms-boundary-result.json",
    "cardinality_result": "cardinality-result.json",
    "surface_coverage_result": "surface-coverage-result.json",
}
artifacts = receipt.get("artifacts", {})
same("artifacts.keys", set(artifacts) if isinstance(artifacts, dict) else set(), set(artifact_relatives))
for key, relative in artifact_relatives.items():
    path = evidence / relative
    recorded = artifacts.get(key, {}) if isinstance(artifacts, dict) else {}
    same(f"artifacts.{key}.exists", recorded.get("exists"), path.is_file())
    same(f"artifacts.{key}.sha256", recorded.get("sha256"), hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None)

predecessor_status = "missing"
if predecessor_path.is_file():
    try:
        predecessor_doc = json.loads(predecessor_path.read_text(encoding="utf-8"))
        predecessor_status = "accepted" if predecessor_doc.get("gate_passed") is True else "not_accepted"
    except (OSError, json.JSONDecodeError):
        predecessor_status = "malformed"
same("predecessor", receipt.get("predecessor"),
     {"release": "0.5.0", "status": predecessor_status, "path": str(predecessor_path)})

manual = receipt.get("manual_signoffs")
if not isinstance(manual, dict) or set(manual) != {"field_secrecy_denial"}:
    print(json.dumps({"receipt_consistent": False, "malformed": True, "errors": ["manual_signoffs key mismatch"]}))
    raise SystemExit(2)
manual_status = manual["field_secrecy_denial"].get("status")
if manual_status not in {"pending", "passed", "failed", "needs_rework"}:
    print(json.dumps({"receipt_consistent": False, "malformed": True, "errors": ["invalid manual status"]}))
    raise SystemExit(2)

automation_passed = all(value == "passed" for value in hard_gates.values())
source_clean = not dirty_entries
source_baseline_matches = (
    str(repo) == baseline_repository
    and head == baseline_head
    and workspace_version == baseline_version
    and frontend_version == baseline_frontend_version
)
candidate_ready = (automation_passed and source_clean and source_baseline_matches
                   and predecessor_status == "accepted" and gate_status != "planned")
accepted = candidate_ready and manual_status == "passed"
blockers = []
if not automation_passed: blockers.append("automated_hard_gate_failed")
if not source_clean: blockers.append("source_dirty")
if not source_baseline_matches: blockers.append("source_baseline_mismatch")
if predecessor_status != "accepted": blockers.append(f"predecessor_{predecessor_status}")
if gate_status == "planned": blockers.append("gate_contract_planned")
if manual_status != "passed": blockers.append(f"manual_field_secrecy_denial_{manual_status}")
counts = {
    "automated": len(hard_gates),
    "passed": sum(value == "passed" for value in hard_gates.values()),
    "failed": sum(value != "passed" for value in hard_gates.values()),
    "manual_pending": 1 if manual_status == "pending" else 0,
    "manual_deferred_to_frontend_track": 3,
    "unresolved": len(blockers),
}
same("deferred_to_frontend_track", receipt.get("deferred_to_frontend_track"), ["table_board", "schema_mode", "large_collection"])
same("automation_passed", receipt.get("automation_passed"), automation_passed)
same("candidate_ready", receipt.get("candidate_ready"), candidate_ready)
same("accepted", receipt.get("accepted"), accepted)
same("gate_passed", receipt.get("gate_passed"), accepted)
same("mode", receipt.get("mode"), "accepted" if accepted else "blocked")
same("counts", receipt.get("counts"), counts)
same("blockers", receipt.get("blockers"), blockers)

consistent = not drift
output = {
    "schema_version": "sylvode.flow.gate-verification.v1",
    "receipt_consistent": consistent,
    "candidate_ready": candidate_ready,
    "accepted": accepted,
    "drift": drift,
    "blockers": blockers,
}
print(json.dumps(output, ensure_ascii=False, sort_keys=True))
raise SystemExit(0 if consistent and candidate_ready else 1)
PY
