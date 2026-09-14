#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.9"
JSON_MODE=0
while (($#)); do
  case "$1" in
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $JSON_MODE -eq 1 ]] || { echo 'FAIL: --json is required' >&2; exit 2; }
CACHE_ROOT=/opt/worker/.cache/openpr-v09-e2e
mkdir -p "$CACHE_ROOT" "$EVIDENCE_ROOT/logs"
LOG="$EVIDENCE_ROOT/logs/isolated-e2e-api.log"
CONTAINERS="$CACHE_ROOT/container-snapshot.txt"
: >"$CONTAINERS"
env OPENPR_E2E_ASSUME_YES=1 OPENPR_E2E_CONTAINER_SNAPSHOT="$CONTAINERS" \
  COMPOSE_PROJECT_NAME=v09-openpr-rc "$REPO_ROOT/scripts/e2e-test.sh" >"$LOG" 2>&1

python3 - "$REPO_ROOT" "$EVIDENCE_ROOT" "$LOG" "$CONTAINERS" <<'PY'
import copy
import datetime as dt
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import tempfile

repo, evidence, log_path, containers_path = map(pathlib.Path, sys.argv[1:])
log = log_path.read_text(errors="replace")
required = {
    "database_migrations":"Database migrations applied successfully",
    "api_integration":"Step 3: Running API Integration Tests",
    "mcp_integration":"Step 4: Running MCP Server Tests",
    "api_health":"API health check passed",
    "mcp_health":"MCP server health check passed",
    "completed":"All End-to-End Tests Passed!",
    "cleanup":"Cleanup complete",
}
checks = {name: marker in log for name, marker in required.items()}
containers = [line for line in containers_path.read_text().splitlines() if line]
container_prefix_ok = bool(containers) and all(name.startswith("v09-") for name in containers)
remaining = subprocess.run(
    ["docker", "compose", "-p", "v09-openpr-rc", "ps", "-q"], cwd=repo, text=True,
    stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False).stdout.splitlines()
cleanup_ok = not remaining

def valid(values, prefix_ok, cleaned):
    return all(values.values()) and prefix_ok and cleaned

mutated = copy.deepcopy(checks)
mutated["mcp_integration"] = False
mutation_red = not valid(mutated, container_prefix_ok, cleanup_ok)
passed = valid(checks, container_prefix_ok, cleanup_ok) and mutation_red
result = {
    "schema_version":"sylvode.flow.isolated-e2e-api-result.v1", "release":"0.9.0",
    "source_head":subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip(),
    "checks":checks, "containers":containers, "container_prefix":"v09-",
    "container_prefix_ok":container_prefix_ok, "remaining_container_count":len(remaining), "cleanup_passed":cleanup_ok,
    "mutation":{"name":"mcp-leg-result-removed", "red":mutation_red},
    "executed_count":len(checks) + len(containers) + 1, "log_sha256":hashlib.sha256(log_path.read_bytes()).hexdigest(),
    "passed":passed, "generated_at":dt.datetime.now(dt.timezone.utc).isoformat(),
}
fd, temporary = tempfile.mkstemp(prefix=".isolated-e2e-api-result.", dir=evidence)
with os.fdopen(fd, "w") as handle:
    json.dump(result, handle, sort_keys=True, indent=2)
    handle.write("\n")
os.replace(temporary, evidence / "isolated-e2e-api-result.json")
print(json.dumps(result, sort_keys=True))
raise SystemExit(0 if passed else 1)
PY
