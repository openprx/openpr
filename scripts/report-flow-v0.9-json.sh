#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CONTRACTS_ROOT=/opt/working/sylvode-flow
EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.9"
GATE_YAML="$CONTRACTS_ROOT/gates/v0.9-gate.yaml"
PREDECESSOR=/opt/worker/evidence/v0.8/gate-result.json
MANUAL_FROM=
while (($#)); do
  case "$1" in
    --repo-root) REPO_ROOT=${2:?}; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --gate-yaml) GATE_YAML=${2:?}; shift 2 ;;
    --predecessor-gate-result|--predecessor-evidence) PREDECESSOR=${2:?}; shift 2 ;;
    --manual-signoffs-from) MANUAL_FROM=${2:?}; shift 2 ;;
    --json) shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ -n $MANUAL_FROM ]] || MANUAL_FROM="$EVIDENCE_ROOT/gate-result.json"
mkdir -p "$EVIDENCE_ROOT/logs"
ROWS=$(mktemp "$EVIDENCE_ROOT/.report-rows.XXXXXX")
trap 'rm -f "$ROWS"' EXIT

run() {
  local id=$1
  shift
  local log="$EVIDENCE_ROOT/logs/report-$id.log"
  set +e
  env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 \
    OPENPR_TEST_DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-postgresql://flowtest:flowtest@127.0.0.1:25433/postgres}" \
    "$@" >"$log" 2>&1
  local code=$?
  set -e
  printf '%s\t%s\t%s\t%s\n' "$id" "$code" "${log#"$EVIDENCE_ROOT/"}" "$*" >>"$ROWS"
}

# Every concrete producer declared by v0.9-gate.yaml and gate-commands.md is executed here.
# report/verify/gate/manual_signoff are orchestration commands and are structurally checked below.
run brand "$REPO_ROOT/scripts/verify-sylvode-brand-compat.sh" --evidence-root "$EVIDENCE_ROOT" --json
run aliases "$REPO_ROOT/scripts/verify-mcp-uri-aliases.sh" --all-resources --transports http,stdio,sse \
  --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json
run install "$REPO_ROOT/scripts/verify-sylvode-install-upgrade.sh" --evidence-root "$EVIDENCE_ROOT" --json
run roundtrip "$REPO_ROOT/scripts/verify-flow-export-roundtrip.sh" --package-schema v1 --source-workspace FIXTURE \
  --fresh-target-workspace --compare objects,documents,relations,lineage,semantic_hash,import_report \
  --evidence-root "$EVIDENCE_ROOT" --json
run fixtures "$REPO_ROOT/scripts/verify-flow-contract-fixtures-v0.9.sh" --evidence-root "$EVIDENCE_ROOT" --json
run surface "$REPO_ROOT/scripts/verify-flow-surface-coverage.sh" --release 0.9 --contracts-root "$CONTRACTS_ROOT" \
  --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json
run cardinality "$REPO_ROOT/scripts/verify-flow-cardinality-v0.9.sh" \
  --adr "$CONTRACTS_ROOT/decisions/ADR-0013-multi-document-atomicity.md" --since-release 0.8 \
  --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json
run registry "$REPO_ROOT/scripts/verify-flow-tool-registry-v0.4.sh" \
  --baseline "$CONTRACTS_ROOT/contracts/tool-count-baseline.md" --release 0.9 \
  --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json
run renames "$REPO_ROOT/scripts/verify-no-undocumented-renames-v0.9.sh" --evidence-root "$EVIDENCE_ROOT" --json
run e2e "$REPO_ROOT/scripts/verify-flow-e2e-api-v0.9.sh" --evidence-root "$EVIDENCE_ROOT" --json
run clippy_full cargo clippy --manifest-path "$REPO_ROOT/Cargo.toml" --workspace --all-targets -- -D warnings
run workspace_full cargo test --manifest-path "$REPO_ROOT/Cargo.toml" --workspace --no-fail-fast

python3 - "$REPO_ROOT" "$CONTRACTS_ROOT" "$EVIDENCE_ROOT" "$GATE_YAML" "$PREDECESSOR" "$MANUAL_FROM" "$ROWS" <<'PY'
import datetime as dt
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import yaml

repo, contracts, evidence, gate_path, predecessor_path, manual_path, rows_path = map(pathlib.Path, sys.argv[1:])
sys.path.insert(0, str(repo / "scripts/lib"))
from flow_contract_status import is_accepted_contract_status

gate = yaml.safe_load(gate_path.read_text())
head = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
artifact_by_check = {
    "brand":"brand-compat-result.json", "aliases":"mcp-uri-alias-result.json",
    "install":"install-upgrade-result.json", "roundtrip":"export-roundtrip-result.json",
    "fixtures":"event-consumer-fixture-result.json", "surface":"surface-coverage-result.json",
    "cardinality":"cardinality-result.json", "registry":"tool-registry-result.json",
    "renames":"no-undocumented-renames-result.json", "e2e":"isolated-e2e-api-result.json",
}

def load(name):
    try:
        return json.loads((evidence / name).read_text())
    except Exception as error:
        return {"passed":False, "executed_count":0, "error":str(error)}

checks = []
for raw in rows_path.read_text().splitlines():
    cid, code, relative, command = raw.split("\t", 3)
    code = int(code)
    log_path = evidence / relative
    body = log_path.read_text(errors="replace")
    summaries = re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;", body, re.M)
    executed = sum(int(passed) + int(failed) for _, passed, failed, _ in summaries)
    ignored = sum(int(value) for *_, value in summaries)
    artifact = load(artifact_by_check[cid]) if cid in artifact_by_check else None
    if artifact is not None:
        if cid == "registry":
            executed = int(artifact.get("live_registry", {}).get("enumerated_total", 0))
        elif cid == "fixtures":
            package_fixture = load("package-fixture-result.json")
            executed = int(artifact.get("executed_count", 0)) + int(package_fixture.get("executed_count", 0))
            artifact = {"passed":artifact.get("passed") is True and package_fixture.get("passed") is True}
        elif cid == "surface":
            executed = int(artifact.get("counts", {}).get("matrix_rows", 0))
        else:
            executed = int(artifact.get("executed_count", 0))
    elif cid in {"clippy_full"}:
        executed = int(code == 0)
    ok = code == 0 and executed > 0 and (artifact is None or artifact.get("passed") is True)
    checks.append({"id":cid, "status":"passed" if ok else "failed", "exit_code":code,
                   "executed_count":executed, "ignored_count":ignored, "command":command,
                   "log":relative, "sha256":hashlib.sha256(log_path.read_bytes()).hexdigest()})

by = {row["id"]:row for row in checks}
brand, aliases = load("brand-compat-result.json"), load("mcp-uri-alias-result.json")
install, old_client = load("install-upgrade-result.json"), load("old-client-result.json")
roundtrip, package_fixture = load("export-roundtrip-result.json"), load("package-fixture-result.json")
event_fixture, surface = load("event-consumer-fixture-result.json"), load("surface-coverage-result.json")
cardinality, registry = load("cardinality-result.json"), load("tool-registry-result.json")
renames, e2e = load("no-undocumented-renames-result.json"), load("isolated-e2e-api-result.json")
passed = lambda value: value.get("passed") is True and int(value.get("executed_count", 0)) > 0
surface_ok = surface.get("passed") is True and int(surface.get("counts", {}).get("matrix_rows", 0)) > 0
three_exceptions = surface.get("counts", {}).get("not_exposed", {}).get("mcp") == 3
registry_mutations = registry.get("mutation_controls", {})
registry_ok = (registry.get("passed") is True and registry.get("live_registry", {}).get("enumerated_total") == 140
    and registry.get("rebase_valid") is True
    and set(registry_mutations) == {"latest_after_count_plus_one", "names_hash_changed"}
    and all(value.get("red") is True for value in registry_mutations.values()))
hard_bool = {
    "command_contended_document_cardinality": passed(cardinality) and cardinality.get("new_commands_found") == 0,
    "rest_mcp_cli_surface_parity": surface_ok,
    "mcp_default_rest_coverage_three_adr_threat_exceptions_only": surface_ok and three_exceptions,
    "sylvode_brand_compat_matrix": passed(brand),
    "openpr_uri_alias_all_resources_three_transports": passed(aliases) and aliases.get("registry_actual_count") == aliases.get("enumerated_resource_count"),
    "clean_install_and_in_place_upgrade": passed(install),
    "rollback_and_old_client_compatibility": passed(old_client),
    "workspace_export_import_roundtrip_complete": passed(roundtrip),
    "export_package_v1_fixture_frozen": passed(package_fixture),
    "flow_event_v1_consumer_fixture_frozen": passed(event_fixture),
    "isolated_full_e2e_api_leg": passed(e2e),
    "no_undocumented_breaking_rename": passed(renames),
    "tool_registry_expected_139_or_rebased": registry_ok,
}
hard = {key:"passed" if hard_bool.get(key, False) else "failed" for key in gate.get("hard_gates", {})}

required_producers = ["brand","aliases","install","roundtrip","fixtures","surface","cardinality","registry","renames","e2e","clippy_full","workspace_full"]
producer_violations = [cid for cid in required_producers if by.get(cid, {}).get("executed_count", 0) <= 0]
command_text = json.dumps(gate.get("required_commands", {}), sort_keys=True)
orchestration = {name:(name in command_text) for name in ("report-flow-v0.9-json.sh", "verify-flow-v0.9-json.sh", "gate-flow-v0.9.sh", "record-flow-v0.9-manual-signoff.sh")}

try:
    predecessor_doc = json.loads(predecessor_path.read_text())
    predecessor_status = predecessor_doc.get("gate_contract", {}).get("status")
    predecessor = {"path":str(predecessor_path), "release":predecessor_doc.get("release"),
                   "accepted":predecessor_doc.get("accepted") is True or is_accepted_contract_status(predecessor_status),
                   "acceptance_basis":"accepted_boolean" if predecessor_doc.get("accepted") is True else predecessor_status}
except Exception as error:
    predecessor = {"path":str(predecessor_path), "accepted":False, "error":str(error)}

manual = {"deprecation_matrix":{"status":"pending", "signed_by":None, "signed_at":None, "note":None}}
try:
    old = json.loads(manual_path.read_text()).get("manual_signoffs", {})
    if old.get("deprecation_matrix", {}).get("status") in {"passed","failed"}:
        manual["deprecation_matrix"] = old["deprecation_matrix"]
except Exception:
    pass

rust = re.search(r'\[workspace\.package\].*?version\s*=\s*"([^"]+)"', (repo / "Cargo.toml").read_text(), re.S).group(1)
frontend = json.loads((repo / "frontend/package.json").read_text())["version"]
dirty = subprocess.check_output(["git", "-C", str(repo), "status", "--porcelain=v1", "--",
    "apps","crates","frontend","migrations","scripts","testing","Cargo.toml","Cargo.lock"], text=True).splitlines()
baseline = gate.get("source_baseline", {})
baseline_matches = baseline.get("reviewed_head") == head and str(baseline.get("rust_workspace_version")) == rust and str(baseline.get("frontend_package_version")) == frontend
contract_status = gate.get("status")
automated_ready = all(value == "passed" for value in hard.values()) and not producer_violations and all(orchestration.values()) and not dirty
candidate = automated_ready and is_accepted_contract_status(contract_status) and baseline_matches and predecessor.get("accepted") is True
accepted = candidate and manual["deprecation_matrix"]["status"] == "passed"
blockers = []
if not is_accepted_contract_status(contract_status): blockers.append("gate_contract_not_active")
if not baseline_matches: blockers.append("source_baseline_mismatch")
if predecessor.get("accepted") is not True: blockers.append("predecessor_not_accepted")
if dirty: blockers.append("source_dirty")
blockers += [f"artifact_missing_after_execution:{cid}" for cid in producer_violations]
blockers += [f"orchestration_command_missing:{name}" for name, present in orchestration.items() if not present]
blockers += [f"hard_gate_failed:{key}" for key, value in hard.items() if value != "passed"]
if manual["deprecation_matrix"]["status"] != "passed": blockers.append("manual_signoff_pending:deprecation_matrix")

result = {
    "schema_version":"sylvode.flow.gate-result.v1", "schema_path":"gates/v0.9-gate.yaml", "release":"0.9.0",
    "source_baseline":baseline, "source":{"head":head,"rust_workspace_version":rust,"frontend_package_version":frontend,
        "dirty":bool(dirty),"dirty_entries":dirty},
    "gate_contract":{"status":contract_status,"sha256":hashlib.sha256(gate_path.read_bytes()).hexdigest()},
    "predecessor":predecessor, "checks":checks, "producer_execution":{"required":required_producers,"violations":producer_violations},
    "orchestration_commands":orchestration, "hard_gates":hard, "manual_signoffs":manual,
    "automated_gate_count":len(hard), "automated_passed":sum(value == "passed" for value in hard.values()),
    "automated_failed":sum(value != "passed" for value in hard.values()), "automated_ready":automated_ready,
    "executed_count":sum(int(row.get("executed_count", 0)) for row in checks),
    "candidate_ready":candidate, "accepted":accepted, "blockers":blockers,
    "generated_at":dt.datetime.now(dt.timezone.utc).isoformat(),
}
fd, temporary = tempfile.mkstemp(prefix=".gate-result.", dir=evidence)
with os.fdopen(fd, "w") as handle:
    json.dump(result, handle, sort_keys=True, indent=2)
    handle.write("\n")
os.replace(temporary, evidence / "gate-result.json")
print(json.dumps(result, sort_keys=True))
raise SystemExit(0 if automated_ready else 1)
PY
