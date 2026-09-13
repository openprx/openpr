#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
EVIDENCE_ROOT="$ROOT/.flow-gate/evidence/v0.9"
ALL_RESOURCES=0
TRANSPORTS=
JSON=0
while (($#)); do
  case "$1" in
    --all-resources) ALL_RESOURCES=1; shift ;;
    --transports) TRANSPORTS=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --repo-root) ROOT=${2:?}; shift 2 ;;
    --json) JSON=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $ALL_RESOURCES -eq 1 ]] || { echo 'FAIL: --all-resources is required' >&2; exit 2; }
[[ $TRANSPORTS == http,stdio,sse ]] || { echo 'FAIL: --transports must be exactly http,stdio,sse' >&2; exit 2; }
[[ $JSON -eq 1 ]] || { echo 'FAIL: --json is required' >&2; exit 2; }

mkdir -p "$EVIDENCE_ROOT/logs"
GREEN_LOG="$EVIDENCE_ROOT/logs/mcp-uri-alias-green.log"
DROP_LOG="$EVIDENCE_ROOT/logs/mcp-uri-alias-drop-registry-mutation.log"
BREAK_LOG="$EVIDENCE_ROOT/logs/mcp-uri-alias-break-alias-mutation.log"
TEST=(cargo test --manifest-path "$ROOT/Cargo.toml" -p mcp-server --test flow_policy_bypass_stdio_e2e all_resource_aliases_are_identical_over_stdio_http_and_sse -- --nocapture)

env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 "${TEST[@]}" >"$GREEN_LOG" 2>&1
set +e
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 OPENPR_TEST_URI_ALIAS_MUTATION=drop-last-registry \
  "${TEST[@]}" >"$DROP_LOG" 2>&1
DROP_EXIT=$?
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 OPENPR_TEST_URI_ALIAS_MUTATION=break-one-alias \
  "${TEST[@]}" >"$BREAK_LOG" 2>&1
BREAK_EXIT=$?
set -e

python3 - "$ROOT" "$EVIDENCE_ROOT" "$GREEN_LOG" "$DROP_LOG" "$BREAK_LOG" "$DROP_EXIT" "$BREAK_EXIT" <<'PY'
import datetime as dt
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import tempfile

root, evidence, green_path, drop_path, break_path = map(pathlib.Path, sys.argv[1:6])
drop_exit, break_exit = map(int, sys.argv[6:8])
green = green_path.read_text(errors="replace")
rows = [line.split("V09_URI_ALIAS_RESULT ", 1)[1] for line in green.splitlines() if "V09_URI_ALIAS_RESULT " in line]
parsed = json.loads(rows[0]) if len(rows) == 1 else {}
drop_text = drop_path.read_text(errors="replace")
break_text = break_path.read_text(errors="replace")
drop_red = drop_exit != 0 and "enumerated count != live registry count" in drop_text
break_red = break_exit != 0 and "Unknown resource URI: openpr-broken://" in break_text
registry_count = parsed.get("registry_actual_count", 0)
enumerated_count = parsed.get("enumerated_resource_count", 0)
transport_count = parsed.get("transport_count", 0)
matrix = parsed.get("rows", [])
passed = bool(
    parsed.get("passed") is True
    and registry_count > 0
    and enumerated_count == registry_count
    and transport_count == 3
    and len(matrix) == registry_count * transport_count
    and parsed.get("executed_count") == len(matrix)
    and all(row.get("passed") is True for row in matrix)
    and {row.get("transport") for row in matrix} == {"http", "stdio", "sse"}
    and drop_red
    and break_red
)
result = {
    "schema_version": "sylvode.flow.mcp-uri-alias-result.v1",
    "release": "0.9.0",
    "source_head": subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip(),
    "source_dirty": bool(subprocess.check_output(["git", "-C", str(root), "status", "--porcelain=v1"], text=True).splitlines()),
    "registry_actual_count": registry_count,
    "enumerated_resource_count": enumerated_count,
    "transport_count": transport_count,
    "executed_count": len(matrix),
    "rows": matrix,
    "mutations": {
        "drop_last_registry": {"exit_code": drop_exit, "red": drop_red, "log": str(drop_path.relative_to(evidence))},
        "break_one_alias": {"exit_code": break_exit, "red": break_red, "log": str(break_path.relative_to(evidence))},
    },
    "green_log": str(green_path.relative_to(evidence)),
    "green_log_sha256": hashlib.sha256(green_path.read_bytes()).hexdigest(),
    "passed": passed,
    "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
}
fd, temporary = tempfile.mkstemp(prefix=".mcp-uri-alias-result.", dir=evidence)
with os.fdopen(fd, "w") as handle:
    json.dump(result, handle, sort_keys=True, indent=2)
    handle.write("\n")
os.replace(temporary, evidence / "mcp-uri-alias-result.json")
print(json.dumps(result, sort_keys=True))
raise SystemExit(0 if passed else 1)
PY
