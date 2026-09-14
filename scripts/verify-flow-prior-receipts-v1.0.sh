#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CONTRACTS=/opt/working/sylvode-flow
EVIDENCE="$ROOT/.flow-gate/evidence/v1.0"
PRIOR=/opt/worker/evidence
BASELINE="$ROOT/scripts/contracts/flow-v1.0-prior-evidence-baseline.json"
BASELINE_SHA256=d24911b25f84723baef66a4aaad4a2b687eeee059aa1d382e1232e95960fdc9f
JSON=0
SKIP_MUTATIONS=0

while (($#)); do
  case "$1" in
    --repo-root) ROOT=${2:?}; shift 2 ;;
    --contracts-root) CONTRACTS=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE=${2:?}; shift 2 ;;
    --prior-evidence-root) PRIOR=${2:?}; shift 2 ;;
    --baseline) BASELINE=${2:?}; shift 2 ;;
    --skip-mutations) SKIP_MUTATIONS=1; shift ;;
    --json) JSON=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $JSON -eq 1 ]] || { echo 'FAIL: --json required' >&2; exit 2; }
mkdir -p "$EVIDENCE"

run_check() {
  python3 - "$ROOT" "$CONTRACTS" "$PRIOR" "$EVIDENCE" "$BASELINE" "$BASELINE_SHA256" <<'PY'
import datetime as dt
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import tempfile

import yaml

repo, contracts, prior, evidence, baseline_path = map(pathlib.Path, sys.argv[1:6])
expected_baseline_sha256 = sys.argv[6]
sys.path.insert(0, str(repo / "scripts/lib"))
from flow_historical_acceptance import is_accepted_historical_status

errors = []
try:
    baseline_raw = baseline_path.read_bytes()
    baseline_sha256 = hashlib.sha256(baseline_raw).hexdigest()
    baseline = json.loads(baseline_raw)
except Exception as error:
    baseline_sha256 = None
    baseline = {}
    errors.append(f"baseline_unreadable:{error}")

baseline_hash_matches = baseline_sha256 == expected_baseline_sha256
if not baseline_hash_matches:
    errors.append("baseline_sha256_mismatch")

expected_rows = baseline.get("files", []) if isinstance(baseline.get("files", []), list) else []
expected_files = {}
for row in expected_rows:
    path = row.get("path") if isinstance(row, dict) else None
    digest = row.get("sha256") if isinstance(row, dict) else None
    if not isinstance(path, str) or not isinstance(digest, str) or path in expected_files:
        errors.append("baseline_file_inventory_invalid")
        continue
    expected_files[path] = digest

releases = baseline.get("releases", []) if isinstance(baseline.get("releases", []), list) else []
actual_files = {}
for release in releases:
    release_root = prior / f"v{release}"
    if release_root.is_dir():
        for path in sorted(item for item in release_root.rglob("*") if item.is_file()):
            relative = path.relative_to(prior).as_posix()
            actual_files[relative] = hashlib.sha256(path.read_bytes()).hexdigest()

file_rows = []
for path in sorted(set(expected_files) | set(actual_files)):
    expected = expected_files.get(path)
    actual = actual_files.get(path)
    status = "matched" if expected is not None and expected == actual else "missing" if actual is None else "unexpected" if expected is None else "hash_mismatch"
    file_rows.append({"path": path, "expected_sha256": expected, "actual_sha256": actual, "status": status})

expected_count = baseline.get("expected_file_count")
file_inventory = {
    "expected_count": expected_count,
    "actual_count": len(actual_files),
    "matched_count": sum(row["status"] == "matched" for row in file_rows),
    "missing_count": sum(row["status"] == "missing" for row in file_rows),
    "unexpected_count": sum(row["status"] == "unexpected" for row in file_rows),
    "hash_mismatch_count": sum(row["status"] == "hash_mismatch" for row in file_rows),
}
file_inventory["passed"] = (
    isinstance(expected_count, int)
    and expected_count == len(expected_files)
    and expected_count == len(actual_files)
    and all(row["status"] == "matched" for row in file_rows)
)

allowed_rows = baseline.get("allowed_artifact_exclusions", []) if isinstance(baseline.get("allowed_artifact_exclusions", []), list) else []
allowed_exclusions = {}
for row in allowed_rows:
    if not isinstance(row, dict):
        errors.append("baseline_exclusion_invalid")
        continue
    key = (row.get("release"), row.get("artifact_key"), row.get("artifact_path"))
    if not all(isinstance(value, str) and value for value in key) or key in allowed_exclusions or not isinstance(row.get("reason_code"), str):
        errors.append("baseline_exclusion_invalid")
        continue
    allowed_exclusions[key] = row["reason_code"]

release_rows = []
artifact_rows = []
used_exclusions = set()
for release in releases:
    contract_path = contracts / "gates" / f"v{release}-gate.yaml"
    try:
        contract = yaml.safe_load(contract_path.read_text())
    except Exception as error:
        contract = {}
        errors.append(f"contract_unreadable:v{release}:{error}")
    status = contract.get("status")
    accepted = is_accepted_historical_status(status)
    release_rows.append({"release": release, "contract_path": str(contract_path), "status": status, "accepted": accepted})
    artifacts = contract.get("artifacts", {}) if isinstance(contract.get("artifacts", {}), dict) else {}
    for artifact_key, raw_path in artifacts.items():
        raw_path = str(raw_path)
        if raw_path.startswith("evidence/"):
            resolved = prior / raw_path.removeprefix("evidence/")
        else:
            contract_owned = contracts / raw_path
            source_owned = repo / raw_path
            resolved = contract_owned if contract_owned.is_file() else source_owned
        exclusion_key = (release, str(artifact_key), raw_path)
        present = resolved.is_file()
        if present:
            artifact_status = "present"
            reason_code = None
        elif exclusion_key in allowed_exclusions:
            artifact_status = "excluded"
            reason_code = allowed_exclusions[exclusion_key]
            used_exclusions.add(exclusion_key)
        else:
            artifact_status = "missing"
            reason_code = None
        artifact_rows.append({
            "release": release,
            "artifact_key": str(artifact_key),
            "artifact_path": raw_path,
            "resolved_path": str(resolved),
            "present": present,
            "status": artifact_status,
            "reason_code": reason_code,
        })

unused_exclusions = [
    {"release": key[0], "artifact_key": key[1], "artifact_path": key[2], "reason_code": allowed_exclusions[key]}
    for key in sorted(set(allowed_exclusions) - used_exclusions)
]
artifact_mapping_passed = all(row["status"] in ("present", "excluded") for row in artifact_rows) and not unused_exclusions
contracts_accepted = bool(release_rows) and all(row["accepted"] for row in release_rows)
checks = {
    "baseline_hash_matches": baseline_hash_matches,
    "file_inventory_exact": file_inventory["passed"],
    "contracts_accepted": contracts_accepted,
    "declared_artifacts_accounted": artifact_mapping_passed,
    "allowed_exclusions_exact": not unused_exclusions and len(used_exclusions) == len(allowed_exclusions),
    "baseline_schema_valid": baseline.get("schema_version") == "sylvode.flow.prior-evidence-baseline.v1" and not errors,
}
passed = all(checks.values())
result = {
    "schema_version": "sylvode.flow.prior-receipts-manifest.v3",
    "release": "1.0.0",
    "source_head": subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip(),
    "acceptance_authority": "authoritative_contract_status_via_scripts/lib/flow_historical_acceptance.py",
    "baseline": {"path": str(baseline_path), "expected_sha256": expected_baseline_sha256, "actual_sha256": baseline_sha256},
    "checks": checks,
    "releases": release_rows,
    "file_inventory": file_inventory,
    "files": file_rows,
    "declared_artifacts": artifact_rows,
    "excluded": [row for row in artifact_rows if row["status"] == "excluded"],
    "unused_exclusions": unused_exclusions,
    "errors": errors,
    "mutations": {},
    "executed_count": len(release_rows) + len(file_rows) + len(artifact_rows) + len(checks),
    "passed": passed,
    "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
}
fd, temporary = tempfile.mkstemp(prefix=".prior-receipts-manifest.", dir=evidence)
with os.fdopen(fd, "w") as handle:
    json.dump(result, handle, sort_keys=True, indent=2)
    handle.write("\n")
os.replace(temporary, evidence / "prior-receipts-manifest.json")
print(json.dumps(result, sort_keys=True))
raise SystemExit(0 if passed else 1)
PY
}

set +e
run_check
BASE_EXIT=$?
set -e
if [[ $SKIP_MUTATIONS -eq 1 ]]; then
  exit "$BASE_EXIT"
fi

RUN=$(mktemp -d /opt/worker/.cache/v10-prior-receipts.XXXXXX)
trap 'rm -rf -- "$RUN"' EXIT
MUTATION_ROWS="$RUN/mutations.tsv"
: >"$MUTATION_ROWS"

copy_prior() {
  local destination=$1 release
  mkdir -p "$destination"
  for release in 0.3 0.4 0.5 0.6 0.7 0.8 0.9; do
    if [[ -d "$PRIOR/v$release" ]]; then
      cp -a "$PRIOR/v$release" "$destination/v$release"
    fi
  done
}

run_mutation() {
  local name=$1 prior_root=$2 baseline_path=$3 code command
  local out="$RUN/out-$name" log="$RUN/$name.log"
  command="$0 --repo-root $ROOT --contracts-root $CONTRACTS --evidence-root $out --prior-evidence-root $prior_root --baseline $baseline_path --json --skip-mutations"
  set +e
  "$0" --repo-root "$ROOT" --contracts-root "$CONTRACTS" --evidence-root "$out" --prior-evidence-root "$prior_root" --baseline "$baseline_path" --json --skip-mutations >"$log" 2>&1
  code=$?
  set -e
  mkdir -p "$EVIDENCE/logs"
  cp "$log" "$EVIDENCE/logs/prior-mutation-$name.log"
  cp "$out/prior-receipts-manifest.json" "$EVIDENCE/logs/prior-mutation-$name-result.json"
  printf '%s\t%s\t%s\t%s\n' "$name" "$code" "$command" "$EVIDENCE/logs/prior-mutation-$name-result.json" >>"$MUTATION_ROWS"
}

MUTATED="$RUN/prior-forged"
copy_prior "$MUTATED"
python3 - "$MUTATED/v0.6/gate-result.json" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
value = json.loads(path.read_text())
value["gate_passed"] = False
value["accepted"] = False
value["blockers"] = ["FORGED BY V1.0 PRIOR RECEIPT MUTATION"]
path.write_text(json.dumps(value, sort_keys=True, indent=2) + "\n")
PY
run_mutation prior_receipt_forged "$MUTATED" "$BASELINE"

MUTATED="$RUN/prior-artifact-deleted"
copy_prior "$MUTATED"
rm -f -- "$MUTATED/v0.8/delivery-result.json"
run_mutation declared_artifact_deleted "$MUTATED" "$BASELINE"

MUTATED="$RUN/prior-spurious-exclusion"
copy_prior "$MUTATED"
MUTATED_BASELINE="$RUN/baseline-spurious-exclusion.json"
cp "$BASELINE" "$MUTATED_BASELINE"
python3 - "$MUTATED_BASELINE" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
value = json.loads(path.read_text())
value["allowed_artifact_exclusions"].append({"release":"0.7","artifact_key":"gate_result","artifact_path":"evidence/v0.7/gate-result.json","reason_code":"spurious_exclusion"})
path.write_text(json.dumps(value, sort_keys=True, indent=2) + "\n")
PY
run_mutation spurious_exclusion_added "$MUTATED" "$MUTATED_BASELINE"

MUTATED="$RUN/prior-file-count-shrunk"
copy_prior "$MUTATED"
VICTIM=$(python3 - "$BASELINE" <<'PY'
import json, pathlib, sys
value=json.loads(pathlib.Path(sys.argv[1]).read_text())
print(next(row["path"] for row in value["files"] if "/logs/" in row["path"]))
PY
)
rm -f -- "$MUTATED/$VICTIM"
run_mutation frozen_file_count_shrunk "$MUTATED" "$BASELINE"

python3 - "$EVIDENCE/prior-receipts-manifest.json" "$MUTATION_ROWS" "$BASE_EXIT" <<'PY'
import datetime as dt
import json
import os
import pathlib
import sys
import tempfile

manifest_path, rows_path = map(pathlib.Path, sys.argv[1:3])
base_exit = int(sys.argv[3])
result = json.loads(manifest_path.read_text())
mutations = {}
for raw in rows_path.read_text().splitlines():
    name, code, command, child_manifest_path = raw.split("\t", 3)
    code = int(code)
    try:
        child = json.loads(pathlib.Path(child_manifest_path).read_text())
        failed_checks = sorted(key for key, passed in child.get("checks", {}).items() if passed is not True)
        inventory = child.get("file_inventory", {})
    except Exception as error:
        failed_checks = [f"child_manifest_unreadable:{error}"]
        inventory = {}
    mutations[name] = {"command": command, "exit_code": code, "red": code != 0, "failed_checks": failed_checks, "file_inventory": inventory}
result["mutations"] = {
    "positive_control": {"command": "primary verifier invocation", "exit_code": base_exit, "green": base_exit == 0},
    **mutations,
}
result["executed_count"] += len(result["mutations"])
result["passed"] = result["passed"] is True and base_exit == 0 and all(row.get("red") is True for row in mutations.values())
result["generated_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
fd, temporary = tempfile.mkstemp(prefix=".prior-receipts-manifest.", dir=manifest_path.parent)
with os.fdopen(fd, "w") as handle:
    json.dump(result, handle, sort_keys=True, indent=2)
    handle.write("\n")
os.replace(temporary, manifest_path)
print(json.dumps(result, sort_keys=True))
raise SystemExit(0 if result["passed"] else 1)
PY
