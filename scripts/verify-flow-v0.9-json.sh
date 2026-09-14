#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
RESULT=
[[ $# -eq 0 || $1 == --* ]] || { RESULT=$1; shift; }
EVIDENCE="$ROOT/.flow-gate/evidence/v0.9"
CONTRACTS=/opt/working/sylvode-flow
REPO=$ROOT
PREDECESSOR=/opt/worker/evidence/v0.8/gate-result.json
while (($#)); do
  case "$1" in
    --evidence-root) EVIDENCE=${2:?}; shift 2 ;;
    --contracts-root) CONTRACTS=${2:?}; shift 2 ;;
    --repo-root) REPO=${2:?}; shift 2 ;;
    --predecessor-gate-result|--predecessor-evidence) PREDECESSOR=${2:?}; shift 2 ;;
    --json) shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ -n $RESULT ]] || RESULT="$EVIDENCE/gate-result.json"

python3 - "$RESULT" "$EVIDENCE" "$REPO" "$CONTRACTS/gates/v0.9-gate.yaml" "$PREDECESSOR" <<'PY'
import hashlib
import json
import pathlib
import re
import subprocess
import sys
import yaml

result_path, evidence, repo, gate_path, predecessor_path = map(lambda value: pathlib.Path(value).resolve(), sys.argv[1:])
sys.path.insert(0, str(repo / "scripts/lib"))
from flow_contract_status import is_accepted_contract_status

drift = []
def same(field, observed, expected):
    if observed != expected:
        drift.append({"field":field, "observed":observed, "expected":expected})

def load(name):
    path = evidence / name
    try:
        return json.loads(path.read_text())
    except Exception as error:
        drift.append({"field":f"artifact.{name}", "error":str(error)})
        return {}

try:
    receipt = json.loads(result_path.read_text())
    gate = yaml.safe_load(gate_path.read_text())
except Exception as error:
    print(json.dumps({"receipt_consistent":False, "errors":[str(error)]}, sort_keys=True))
    raise SystemExit(2)

head = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
rust = re.search(r'\[workspace\.package\].*?version\s*=\s*"([^"]+)"', (repo / "Cargo.toml").read_text(), re.S).group(1)
frontend = json.loads((repo / "frontend/package.json").read_text())["version"]
dirty = subprocess.check_output(
    ["git", "-C", str(repo), "status", "--porcelain=v1"], text=True
).splitlines()

same("schema_version", receipt.get("schema_version"), "sylvode.flow.gate-result.v1")
same("release", receipt.get("release"), "0.9.0")
same("source.head", receipt.get("source", {}).get("head"), head)
same("source.rust_workspace_version", receipt.get("source", {}).get("rust_workspace_version"), rust)
same("source.frontend_package_version", receipt.get("source", {}).get("frontend_package_version"), frontend)
same("source.dirty", receipt.get("source", {}).get("dirty"), bool(dirty))
same("gate_contract.status", receipt.get("gate_contract", {}).get("status"), gate.get("status"))
same("gate_contract.sha256", receipt.get("gate_contract", {}).get("sha256"), hashlib.sha256(gate_path.read_bytes()).hexdigest())
same("hard_gates.keys", sorted(receipt.get("hard_gates", {})), sorted(gate.get("hard_gates", {})))

artifacts = {name:load(name) for name in (
    "brand-compat-result.json", "mcp-uri-alias-result.json", "install-upgrade-result.json",
    "old-client-result.json", "export-roundtrip-result.json", "package-fixture-result.json",
    "event-consumer-fixture-result.json", "surface-coverage-result.json", "cardinality-result.json",
    "tool-registry-result.json", "no-undocumented-renames-result.json", "isolated-e2e-api-result.json",
)}
for name, artifact in artifacts.items():
    if artifact.get("source_head") is not None:
        same(f"artifact.{name}.source_head", artifact.get("source_head"), head)

def positive(value):
    return value.get("passed") is True and int(value.get("executed_count", 0)) > 0

brand = artifacts["brand-compat-result.json"]
aliases = artifacts["mcp-uri-alias-result.json"]
install = artifacts["install-upgrade-result.json"]
old_client = artifacts["old-client-result.json"]
roundtrip = artifacts["export-roundtrip-result.json"]
package = artifacts["package-fixture-result.json"]
event = artifacts["event-consumer-fixture-result.json"]
surface = artifacts["surface-coverage-result.json"]
cardinality = artifacts["cardinality-result.json"]
registry = artifacts["tool-registry-result.json"]
renames = artifacts["no-undocumented-renames-result.json"]
e2e = artifacts["isolated-e2e-api-result.json"]

rows = aliases.get("rows", [])
alias_ok = (positive(aliases) and aliases.get("registry_actual_count", 0) > 0
    and aliases.get("registry_actual_count") == aliases.get("enumerated_resource_count")
    and aliases.get("transport_count") == 3 and len(rows) == aliases.get("registry_actual_count") * 3
    and aliases.get("executed_count") == len(rows) and {row.get("transport") for row in rows} == {"http", "stdio", "sse"}
    and all(row.get("passed") is True for row in rows)
    and all(value.get("red") is True for value in aliases.get("mutations", {}).values()))
comparison = roundtrip.get("roundtrip", {}).get("comparison", {})
branches = roundtrip.get("roundtrip", {}).get("branches", [])
roundtrip_ok = (positive(roundtrip) and roundtrip.get("roundtrip", {}).get("documents_compared") == roundtrip.get("roundtrip", {}).get("documents_enumerated")
    and roundtrip.get("roundtrip", {}).get("documents_compared", 0) > 0
    and all(comparison.get(key) is True for key in ("head_frontier", "head_seq", "semantic_hash", "projection_frontier", "projection_seq", "object_metadata", "relation_graph", "lineage", "import_report"))
    and roundtrip.get("roundtrip", {}).get("mcp_import_chain") == ["objects.import_artifact", "objects.import_preview", "objects.import_commit", "objects.import_status"]
    and len(branches) == 2 and branches[1].get("canonical_writes") == 0 and branches[1].get("head_changes") == 0
    and branches[1].get("before") == branches[1].get("after")
    and all(value.get("red") is True for value in roundtrip.get("mutation_controls", {}).values()))
surface_ok = surface.get("passed") is True and int(surface.get("counts", {}).get("matrix_rows", 0)) > 0
registry_live = registry.get("live_registry", {})
registry_mutations = registry.get("mutation_controls", {})
registry_ok = (registry.get("passed") is True and registry_live.get("header_declared_total") == 140
    and registry_live.get("enumerated_total") == 140 and registry_live.get("unique_total") == 140
    and not registry_live.get("duplicate_names") and registry.get("rebase_valid") is True
    and set(registry_mutations) == {"latest_after_count_plus_one", "names_hash_changed"}
    and all(value.get("red") is True for value in registry_mutations.values()))
e2e_ok = (positive(e2e) and all(e2e.get("checks", {}).values()) and e2e.get("container_prefix_ok") is True
    and e2e.get("cleanup_passed") is True and e2e.get("remaining_container_count") == 0
    and e2e.get("mutation", {}).get("red") is True)

hard_bool = {
    "command_contended_document_cardinality": positive(cardinality) and cardinality.get("new_commands_found") == 0 and cardinality.get("mutation", {}).get("red") is True,
    "rest_mcp_cli_surface_parity": surface_ok,
    "mcp_default_rest_coverage_three_adr_threat_exceptions_only": surface_ok and surface.get("counts", {}).get("not_exposed", {}).get("mcp") == 3,
    "sylvode_brand_compat_matrix": positive(brand) and all(item.get("status") == "passed" for item in brand.get("checks", [])),
    "openpr_uri_alias_all_resources_three_transports": alias_ok,
    "clean_install_and_in_place_upgrade": positive(install) and install.get("clean_install", {}).get("api_ready") is True and install.get("in_place_upgrade", {}).get("snapshot_exact_match") is True,
    "rollback_and_old_client_compatibility": positive(old_client) and old_client.get("old_mcp_client", {}).get("passed") is True and old_client.get("rollback", {}).get("data_snapshot_unchanged") is True,
    "workspace_export_import_roundtrip_complete": roundtrip_ok,
    "export_package_v1_fixture_frozen": positive(package) and package.get("fixture_count") == 1 and package.get("mutation", {}).get("red") is True,
    "flow_event_v1_consumer_fixture_frozen": positive(event) and event.get("fixture_count") == 3 and event.get("covered_shapes") == ["plain", "coalesced", "retry"] and event.get("mutation", {}).get("red") is True,
    "isolated_full_e2e_api_leg": e2e_ok,
    "no_undocumented_breaking_rename": positive(renames) and all(row.get("passed") is True for row in renames.get("checks", [])) and all(value.get("red") is True for value in renames.get("mutations", {}).values()),
    "tool_registry_expected_139_or_rebased": registry_ok,
}
hard = {key:"passed" if hard_bool.get(key, False) else "failed" for key in gate.get("hard_gates", {})}
same("hard_gates", receipt.get("hard_gates"), hard)

required_checks = ["brand", "aliases", "install", "roundtrip", "fixtures", "surface", "cardinality", "registry", "renames", "e2e", "clippy_full", "workspace_full"]
checks = {row.get("id"):row for row in receipt.get("checks", [])}
same("checks.ids", sorted(checks), sorted(required_checks))
producer_violations = []
for check_id in required_checks:
    row = checks.get(check_id, {})
    if row.get("status") != "passed" or int(row.get("executed_count", 0)) <= 0 or row.get("exit_code") != 0:
        producer_violations.append(check_id)
    if row.get("log"):
        try:
            same(f"checks.{check_id}.sha256", row.get("sha256"), hashlib.sha256((evidence / row["log"]).read_bytes()).hexdigest())
        except Exception as error:
            drift.append({"field":f"checks.{check_id}.log", "error":str(error)})
same("producer_execution.violations", receipt.get("producer_execution", {}).get("violations"), producer_violations)

try:
    predecessor_doc = json.loads(predecessor_path.read_text())
    predecessor_status = predecessor_doc.get("gate_contract", {}).get("status")
    predecessor = {"path":str(predecessor_path), "release":predecessor_doc.get("release"),
        "accepted":predecessor_doc.get("accepted") is True or is_accepted_contract_status(predecessor_status),
        "acceptance_basis":"accepted_boolean" if predecessor_doc.get("accepted") is True else predecessor_status}
except Exception as error:
    predecessor = {"path":str(predecessor_path), "accepted":False, "error":str(error)}
same("predecessor", receipt.get("predecessor"), predecessor)

orchestration = receipt.get("orchestration_commands", {})
automated_ready = (all(value == "passed" for value in hard.values()) and not producer_violations
    and all(orchestration.get(name) is True for name in ("report-flow-v0.9-json.sh", "verify-flow-v0.9-json.sh", "gate-flow-v0.9.sh", "record-flow-v0.9-manual-signoff.sh"))
    and not dirty)
baseline = gate.get("source_baseline", {})
baseline_matches = baseline.get("reviewed_head") == head and str(baseline.get("rust_workspace_version")) == rust and str(baseline.get("frontend_package_version")) == frontend
candidate = automated_ready and is_accepted_contract_status(gate.get("status")) and baseline_matches and predecessor.get("accepted") is True
manual = receipt.get("manual_signoffs", {})
accepted = candidate and manual.get("deprecation_matrix", {}).get("status") == "passed"
same("automated_gate_count", receipt.get("automated_gate_count"), len(hard))
same("automated_passed", receipt.get("automated_passed"), sum(value == "passed" for value in hard.values()))
same("automated_failed", receipt.get("automated_failed"), sum(value != "passed" for value in hard.values()))
same("automated_ready", receipt.get("automated_ready"), automated_ready)
same("candidate_ready", receipt.get("candidate_ready"), candidate)
same("accepted", receipt.get("accepted"), accepted)

out = {"schema_version":"sylvode.flow.verification.v2", "release":"0.9.0",
    "receipt_consistent":not drift, "drift":drift, "hard_gates":hard,
    "automated_ready":automated_ready, "candidate_ready":candidate, "accepted":accepted,
    "predecessor":predecessor, "producer_violations":producer_violations,
    "blockers":receipt.get("blockers", [])}
print(json.dumps(out, sort_keys=True))
raise SystemExit(0 if not drift and automated_ready else 1)
PY
