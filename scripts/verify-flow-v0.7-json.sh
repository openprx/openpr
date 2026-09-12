#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RESULT=""
if [[ $# -gt 0 && "$1" != --* ]]; then RESULT="$1"; shift; fi
REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT=""
GATE_YAML=""
while (($#)); do
  case "$1" in
    --repo-root) REPO_ROOT="$2"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="$2"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="$2"; shift 2 ;;
    --gate-yaml) GATE_YAML="$2"; shift 2 ;;
    --json) shift ;;
    *) echo "FAIL: unsupported argument $1" >&2; exit 2 ;;
  esac
done
[[ -n "$EVIDENCE_ROOT" ]] || EVIDENCE_ROOT="$CONTRACTS_ROOT/evidence/v0.7"
[[ -n "$RESULT" ]] || RESULT="$EVIDENCE_ROOT/gate-result.json"
[[ -n "$GATE_YAML" ]] || GATE_YAML="$CONTRACTS_ROOT/gates/v0.7-gate.yaml"
[[ -f "$RESULT" ]] || { echo "FAIL: gate result missing: $RESULT" >&2; exit 2; }

python3 - "$RESULT" "$EVIDENCE_ROOT" "$REPO_ROOT" "$GATE_YAML" <<'PY'
import hashlib, json, pathlib, re, subprocess, sys
result, evidence, repo, gate = map(lambda p: pathlib.Path(p).resolve(), sys.argv[1:])
drift = []
try:
    receipt = json.loads(result.read_text())
except Exception as error:
    print(json.dumps({"receipt_consistent": False, "malformed": True, "errors": [str(error)]}))
    raise SystemExit(2)

def same(field, observed, expected):
    if observed != expected:
        drift.append({"field": field, "observed": observed, "expected": expected})

def libtest(log_text):
    rows = re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;", log_text, re.M)
    passed = sum(int(row[1]) for row in rows)
    failed = sum(int(row[2]) for row in rows)
    return {"executed_count": passed + failed, "passed": bool(rows) and failed == 0 and all(row[0] == "ok" for row in rows)}

def producer_json(log_text):
    decoder = json.JSONDecoder()
    values = []
    for match in re.finditer(r"\{", log_text):
        try:
            value, _ = decoder.raw_decode(log_text[match.start():])
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            values.append(value)
    return values[-1] if values else None

def nested_libtest(path, expected):
    try:
        parsed = libtest(path.read_text())
    except Exception as error:
        drift.append({"field": f"nested_log.{path.name}", "error": str(error)})
        return False
    return parsed["executed_count"] > 0 and parsed["passed"] is expected

same("schema_version", receipt.get("schema_version"), "sylvode.flow.gate-result.v1")
same("release", receipt.get("release"), "0.7.0")
head = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
same("source.head", receipt.get("source", {}).get("head"), head)
same("gate_contract.sha256", receipt.get("gate_contract", {}).get("sha256"), hashlib.sha256(gate.read_bytes()).hexdigest())
rust_version = re.search(r"\[workspace\.package\].*?version\s*=\s*\"([^\"]+)\"", (repo / "Cargo.toml").read_text(), re.S).group(1)
frontend_version = json.loads((repo / "frontend/package.json").read_text())["version"]
same("source.rust_workspace_version", receipt.get("source", {}).get("rust_workspace_version"), rust_version)
same("source.frontend_package_version", receipt.get("source", {}).get("frontend_package_version"), frontend_version)

checks = receipt.get("checks", [])
if not isinstance(checks, list) or any(not isinstance(item, dict) for item in checks):
    drift.append({"field": "checks", "error": "must be an array of objects"})
    checks = []
ids = [item.get("id") for item in checks]
if len(ids) != len(set(ids)):
    drift.append({"field": "checks.ids", "error": "duplicate producer id"})

recomputed = {}
for item in checks:
    cid = item.get("id")
    path = evidence / str(item.get("log", ""))
    try:
        text = path.read_text()
    except Exception as error:
        drift.append({"field": f"checks.{cid}.log", "error": str(error)})
        continue
    same(f"checks.{cid}.sha256", item.get("sha256"), hashlib.sha256(text.encode()).hexdigest())
    parsed = libtest(text)
    if cid == "forms_full":
        parsed["passed"] = parsed["passed"] and "Universal Forms static and Rust regression gates passed." in text
    elif cid == "bridge_mutations":
        data = json.loads((evidence / "bridge-mutation-result.json").read_text())
        outcomes = []
        for case in data.get("cases", []):
            case_id = case.get("id", "")
            if case.get("expected") == "green":
                prefix = case_id.removesuffix("_production_green")
                outcomes.append(nested_libtest(evidence / f"bridge-{prefix}-green.log", True))
            else:
                outcomes.append(nested_libtest(evidence / f"bridge-{case_id}-red.log", False))
        parsed = {"executed_count": len(outcomes), "passed": bool(outcomes) and all(outcomes)}
    elif cid == "migration_replay":
        outcomes = [
            nested_libtest(evidence / "migration-replay-green.log", True),
            nested_libtest(evidence / "migration-replay-no-if-not-exists-red.log", False),
        ]
        parsed = {"executed_count": 2, "passed": all(outcomes)}
    elif cid == "bridge_smoke":
        data = json.loads((evidence / "bridge-smoke-result.json").read_text())
        outcomes = []
        for child in data.get("checks", []):
            child_text = (evidence / child.get("log", "")).read_text()
            child_parsed = libtest(child_text)
            if child.get("id") == "frontend":
                child_ok = bool(re.search(r"\b\d+\s+pass(?:ed)?\b", child_text, re.I)) and not re.search(
                    r"\b\d+\s+fail(?:ed)?\b", child_text, re.I
                )
            else:
                child_ok = child_parsed["executed_count"] > 0 and child_parsed["passed"]
            outcomes.append(child_ok)
        parsed = {"executed_count": len(outcomes), "passed": len(outcomes) == 4 and all(outcomes)}
    elif cid == "cardinality":
        data = json.loads((evidence / "cardinality-result.json").read_text())
        outcomes = [nested_libtest(evidence / row["log"], True) for row in data.get("tests", [])]
        routes = data.get("commands_found", [])
        parsed = {"executed_count": len(routes), "passed": len(routes) == 6 and len(outcomes) == 3 and all(outcomes)}
    elif cid == "surface":
        data = json.loads((evidence / "surface-coverage-result.json").read_text())
        violations = data.get("violations", {})
        violation_count = sum(len(value) for value in violations.values() if isinstance(value, list))
        counts = data.get("counts", {})
        executed = counts.get("matrix_rows", 0)
        parsed = {
            "executed_count": executed,
            "passed": executed > 0 and violation_count == 0 and not data.get("source_dirty", True)
                      and data.get("implementation_parity", {}).get("passed") is True,
        }
    elif parsed["executed_count"] == 0:
        data = producer_json(text)
        parsed = {
            "executed_count": int(data.get("executed_count", 0)) if data else 0,
            "passed": bool(data and data.get("passed") is True and int(data.get("executed_count", 0)) > 0),
        }
    recomputed[cid] = parsed
    same(f"checks.{cid}.executed_count", item.get("executed_count"), parsed["executed_count"])
    same(f"checks.{cid}.status", item.get("status"), "passed" if parsed["passed"] else "failed")

required_artifacts = [
    "bridge-contract-result.json", "embed-permission-result.json", "conversion-fault-result.json",
    "lineage-result.json", "forms-regression-result.json", "migration-replay-result.json",
    "bridge-mutation-result.json", "bridge-smoke-result.json", "cardinality-result.json",
    "surface-coverage-result.json",
]
artifact_ok = True
for name in required_artifacts:
    try:
        artifact = json.loads((evidence / name).read_text())
        if not isinstance(artifact, dict):
            raise ValueError("top level is not object")
        same(f"artifact.{name}.source_head", artifact.get("source_head"), head)
    except Exception as error:
        artifact_ok = False
        drift.append({"field": f"artifact.{name}", "error": str(error)})

gate_keys = set(re.findall(r"^  ([a-z0-9_]+): pending$", gate.read_text(), re.M))
hard_gates = receipt.get("hard_gates", {})
same("hard_gates.keys", set(hard_gates), gate_keys)
invalid_gate_values = {key: value for key, value in hard_gates.items() if value not in {"passed", "failed"}}
if invalid_gate_values:
    drift.append({"field": "hard_gates.values", "observed": invalid_gate_values, "expected": "passed or failed"})
all_producers = bool(recomputed) and len(recomputed) == len(checks) and all(value["passed"] for value in recomputed.values())
if all_producers:
    same("hard_gates.all_when_all_producers_pass", set(key for key, value in hard_gates.items() if value == "passed"), gate_keys)
failed_gates = [key for key, value in hard_gates.items() if value != "passed"]
same("automated_failed", receipt.get("automated_failed"), len(failed_gates))
same("automated_passed", receipt.get("automated_passed"), len(gate_keys) - len(failed_gates))

contract_active = receipt.get("gate_contract", {}).get("status") == "active"
baseline = receipt.get("source_baseline", {})
source = receipt.get("source", {})
baseline_match = (
    baseline.get("reviewed_head") == head
    and baseline.get("rust_workspace_version") == rust_version
    and baseline.get("frontend_package_version") == frontend_version
)
predecessor = bool(receipt.get("predecessor", {}).get("accepted"))
source_clean = not source.get("dirty")
candidate = not failed_gates and all_producers and artifact_ok and contract_active and baseline_match and predecessor and source_clean
same("candidate_ready", receipt.get("candidate_ready"), candidate)
manual = receipt.get("manual_signoffs", {})
accepted = candidate and bool(manual) and all(value.get("status") == "passed" for value in manual.values())
same("accepted", receipt.get("accepted"), accepted)
out = {
    "schema_version": "sylvode.flow.verification.v2", "release": "0.7.0",
    "receipt_consistent": not drift, "drift": drift, "recomputed_checks": recomputed,
    "hard_gates": hard_gates, "candidate_ready": candidate, "accepted": accepted,
    "blockers": receipt.get("blockers", []),
}
print(json.dumps(out, sort_keys=True))
raise SystemExit(0 if not drift and candidate else 1)
PY
