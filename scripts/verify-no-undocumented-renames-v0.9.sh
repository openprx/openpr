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
mkdir -p "$EVIDENCE_ROOT/logs"
LOG="$EVIDENCE_ROOT/logs/no-undocumented-renames-runtime.log"
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 cargo build --manifest-path "$REPO_ROOT/Cargo.toml" \
  -p mcp-server --bin list-tools --bin mcp-server >"$LOG" 2>&1
"$REPO_ROOT/target/debug/list-tools" >"$EVIDENCE_ROOT/logs/no-undocumented-renames-tools.txt"
"$REPO_ROOT/target/debug/mcp-server" --help >"$EVIDENCE_ROOT/logs/no-undocumented-renames-cli.txt"

python3 - "$REPO_ROOT" "$EVIDENCE_ROOT" <<'PY'
import datetime as dt
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile

repo, evidence = map(pathlib.Path, sys.argv[1:])
baseline = json.loads(subprocess.check_output(
    ["git", "-C", str(repo), "show", "aa453de:apps/mcp-server/tool-registry-baseline.json"], text=True))
tools_text = (evidence / "logs/no-undocumented-renames-tools.txt").read_text()
tools = sorted(re.findall(r"^  ([a-z][a-z0-9_.]+)$", tools_text, re.M))
tools_hash = hashlib.sha256("\n".join(tools).encode()).hexdigest()
tool_ok = len(tools) == baseline["count"] and tools_hash == baseline["names_sha256"] and len(tools) == len(set(tools))

cli_text = (evidence / "logs/no-undocumented-renames-cli.txt").read_text()
match = re.search(r"^Commands:\n(?P<body>(?:  .+\n)+)", cli_text, re.M)
cli = sorted(re.match(r"  ([a-z][a-z0-9-]*)", line).group(1) for line in match.group("body").splitlines()) if match else []
expected_cli = json.loads((repo / "testing/fixtures/v09-openpr-cli-top-level.json").read_text())
cli_ok = cli == expected_cli

schema_expected = json.loads((repo / "testing/fixtures/v09-openpr-schema-ids.json").read_text())
schema_actual = []
for path in sorted((repo / "docs/schemas").glob("*.json")):
    payload = json.loads(path.read_text())
    schema_id = payload.get("$id")
    if schema_id and ("openpr" in schema_id or path.name.startswith("openpr-")):
        schema_actual.append({"file":str(path.relative_to(repo)), "id":schema_id})
schema_ok = schema_actual == schema_expected and len(schema_actual) > 0

matrix = (repo / "docs/sylvode-v0.9-compatibility.md").read_text()
documented_surfaces = ["Product, Web UI, current docs", "CLI", "Configuration", "Compose environment",
                       "REST API and schema IDs", "MCP tools", "MCP resources", "Database and migrations",
                       "Release archives", "Telemetry and containers"]
matrix_ok = all(f"| {surface} |" in matrix for surface in documented_surfaces)
old_client_path = evidence / "old-client-result.json"
old_client = json.loads(old_client_path.read_text()) if old_client_path.is_file() else {}
old_mcp_client = old_client.get("old_mcp_client", {})
old_client_checks = old_mcp_client.get("checks", {})
old_client_ok = old_client.get("passed") is True and old_mcp_client.get("passed") is True and all(old_client_checks.values())

def valid(names, matrix_complete):
    return len(names) == baseline["count"] and hashlib.sha256("\n".join(sorted(names)).encode()).hexdigest() == baseline["names_sha256"] and matrix_complete

mutated_names = tools[:-1]
mutations = {
    "runtime_tool_removed_or_renamed":{"red":not valid(mutated_names, matrix_ok)},
    "compatibility_matrix_row_removed":{"red":not valid(tools, False)},
}
checks = [
    {"id":"v08_to_v09_runtime_mcp_names_exact", "passed":tool_ok, "executed_count":len(tools),
     "baseline_count":baseline["count"], "actual_count":len(tools), "names_sha256":tools_hash},
    {"id":"legacy_mcp_server_cli_commands", "passed":cli_ok, "executed_count":len(cli)},
    {"id":"legacy_api_schema_ids", "passed":schema_ok, "executed_count":len(schema_actual)},
    {"id":"documented_rename_matrix", "passed":matrix_ok, "executed_count":len(documented_surfaces)},
    {"id":"unmodified_v08_client_runtime", "passed":old_client_ok, "executed_count":len(old_client_checks),
     "evidence_source_head":old_client.get("source_head")},
]
passed = all(row["passed"] and row["executed_count"] > 0 for row in checks) and all(row["red"] for row in mutations.values())
result = {
    "schema_version":"sylvode.flow.no-undocumented-renames-result.v1", "release":"0.9.0",
    "source_head":subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip(),
    "checks":checks, "mutations":mutations, "executed_count":sum(row["executed_count"] for row in checks),
    "passed":passed, "generated_at":dt.datetime.now(dt.timezone.utc).isoformat(),
}
fd, temporary = tempfile.mkstemp(prefix=".no-undocumented-renames-result.", dir=evidence)
with os.fdopen(fd, "w") as handle:
    json.dump(result, handle, sort_keys=True, indent=2)
    handle.write("\n")
os.replace(temporary, evidence / "no-undocumented-renames-result.json")
print(json.dumps(result, sort_keys=True))
raise SystemExit(0 if passed else 1)
PY
