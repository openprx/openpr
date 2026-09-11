#!/usr/bin/env bash
set -euo pipefail

# v0.5 live-registry and shipped MCP/CLI semantic-equivalence verifier.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/flow_contract_path.sh
source "$ROOT_DIR/scripts/lib/flow_contract_path.sh"

REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
MCP_CONTRACT=""
SURFACE_CONTRACT=""
ADR_PATH=""
EVIDENCE_ROOT="${TMPDIR:-/tmp}/openpr-flow-evidence/v0.5"
DATABASE_URL="postgresql://flowtest:flowtest@127.0.0.1:25433/postgres"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-mcp-cli-equivalence-v0.5.sh --mcp-contract PATH --surface-contract PATH --adr PATH --json [OPTIONS]

Options:
  --mcp-contract PATH      mcp-surface-v1.md (required).
  --surface-contract PATH  surface-coverage-v1.md (required).
  --adr PATH               ADR-0009 Flow search decision (required; G6 cross-check).
  --contracts-root DIR     Contract checkout.
  --evidence-root DIR      Output directory.
  --repo-root DIR          Source checkout.
  --database-url URL       PostgreSQL test authority.
  --json                   Required; print the artifact.
  -h, --help               Show this help.

Exit codes: 0 all gates passed; 1 a gate failed; 2 malformed evidence.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mcp-contract) MCP_CONTRACT="${2:?--mcp-contract requires a path}"; shift 2 ;;
    --surface-contract) SURFACE_CONTRACT="${2:?--surface-contract requires a path}"; shift 2 ;;
    --adr) ADR_PATH="${2:?--adr requires a path}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a directory}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a directory}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a directory}"; shift 2 ;;
    --database-url) DATABASE_URL="${2:?--database-url requires a URL}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$MCP_CONTRACT" || -z "$SURFACE_CONTRACT" || -z "$ADR_PATH" || $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --mcp-contract, --surface-contract, --adr, and --json are required" >&2
  usage >&2
  exit 2
fi
for tool in cargo git jq python3 realpath sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required tool: $tool" >&2; exit 2; }
done
MCP_CONTRACT="$(flow_resolve_contract_path --mcp-contract "$MCP_CONTRACT" "$CONTRACTS_ROOT")" || exit 2
SURFACE_CONTRACT="$(flow_resolve_contract_path --surface-contract "$SURFACE_CONTRACT" "$CONTRACTS_ROOT")" || exit 2
ADR_PATH="$(flow_resolve_contract_path --adr "$ADR_PATH" "$CONTRACTS_ROOT")" || exit 2
BASELINE_CONTRACT="$CONTRACTS_ROOT/contracts/tool-count-baseline.md"
[[ -f "$BASELINE_CONTRACT" ]] || { echo "FAIL: missing tool count baseline" >&2; exit 2; }
git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1 || exit 2
REPO_ROOT="$(cd "$REPO_ROOT" && pwd)"

CONTRACTS_REAL="$(realpath -m "$CONTRACTS_ROOT")"
EVIDENCE_REAL="$(realpath -m "$EVIDENCE_ROOT")"
case "$EVIDENCE_REAL/" in
  "$CONTRACTS_REAL/"*) echo "FAIL: refusing to write evidence into the contract checkout" >&2; exit 2 ;;
esac
mkdir -p "$EVIDENCE_REAL/logs"

SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
SOURCE_DIRTY_STATUS="$(git -C "$REPO_ROOT" status --porcelain=v1 --untracked-files=all -- apps crates spikes migrations .cargo Cargo.toml Cargo.lock)"
SOURCE_DIRTY=$([[ -n "$SOURCE_DIRTY_STATUS" ]] && echo true || echo false)
SOURCE_DIRTY_ENTRIES="$(printf '%s\n' "$SOURCE_DIRTY_STATUS" | jq -R 'select(length > 0)' | jq -s '.')"

BUILD_LOG="$EVIDENCE_REAL/logs/mcp-cli.binaries-build.log"
WORKER_BUILD_LOG="$EVIDENCE_REAL/logs/mcp-cli.worker-build.log"
LIVE_LOG="$EVIDENCE_REAL/logs/mcp-cli.live-registry.log"
PROBE_LOG="$EVIDENCE_REAL/logs/mcp-cli.binary-probe.json"
PROBE_STDERR="$EVIDENCE_REAL/logs/mcp-cli.binary-probe.stderr"
TEST_LOG="$EVIDENCE_REAL/logs/mcp-cli.projection-lag-tests.log"
MUTATION_LOG="$EVIDENCE_REAL/logs/mcp-cli.registry-mutation.log"

run_timed() {
  local log="$1"
  shift
  local start end status
  start="$(date +%s%N)"
  set +e
  (cd "$REPO_ROOT" && "$@") >"$log" 2>&1
  status=$?
  set -e
  end="$(date +%s%N)"
  printf '%s %s\n' "$status" "$(((end - start) / 1000000))"
}

echo "=== build shipped MCP/CLI/list-tools binaries ===" >&2
read -r BUILD_EXIT BUILD_MS < <(run_timed "$BUILD_LOG" cargo build --locked -p mcp-server --bins)
echo "  exit=$BUILD_EXIT duration_ms=$BUILD_MS log=$BUILD_LOG" >&2
echo "=== build collab-isolated-apply-worker ===" >&2
read -r WORKER_BUILD_EXIT WORKER_BUILD_MS < <(run_timed "$WORKER_BUILD_LOG" cargo build --locked -p collab-core --bin collab-isolated-apply-worker)
echo "  exit=$WORKER_BUILD_EXIT duration_ms=$WORKER_BUILD_MS log=$WORKER_BUILD_LOG" >&2

LIST_TOOLS_BIN="$REPO_ROOT/target/debug/list-tools"
MCP_BIN="$REPO_ROOT/target/debug/mcp-server"
CLI_BIN="$REPO_ROOT/target/debug/sylvode"
if [[ $BUILD_EXIT -eq 0 && -x "$LIST_TOOLS_BIN" && -x "$MCP_BIN" && -x "$CLI_BIN" ]]; then
  read -r LIVE_EXIT LIVE_MS < <(run_timed "$LIVE_LOG" "$LIST_TOOLS_BIN")
else
  LIVE_EXIT=127
  LIVE_MS=0
  printf 'binaries were not produced\n' >"$LIVE_LOG"
fi
echo "=== live registry exit=$LIVE_EXIT duration_ms=$LIVE_MS log=$LIVE_LOG ===" >&2

# Drive the two actual executables against one capture server. The server returns
# an already-policy-filtered projection-lag payload; equivalence is checked on
# the semantic data, while captured paths prove both surfaces made the same REST
# request. No in-process adapter is used.
PROBE_START="$(date +%s%N)"
set +e
python3 - "$MCP_BIN" "$CLI_BIN" >"$PROBE_LOG" 2>"$PROBE_STDERR" <<'PY'
import http.server
import json
import pathlib
import subprocess
import sys
import tempfile
import threading
import urllib.parse

mcp_bin, cli_bin = sys.argv[1:]
workspace = "11111111-1111-4111-8111-111111111111"
calls = []
data = {
    "max_lag": 7,
    "p95_lag": 3,
    "items": [{
        "object_id": "22222222-2222-4222-8222-222222222222",
        "head_seq": 9,
        "projection_seq": 2,
        "lag": 7,
    }],
    "next_cursor": "opaque-policy-filtered-cursor",
}

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        calls.append({
            "path": self.path,
            "authorization_present": bool(self.headers.get("authorization")),
            "surface": self.headers.get("x-openpr-mcp-surface"),
        })
        body = json.dumps({"code": 0, "data": data}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *_args):
        return

server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
with tempfile.TemporaryDirectory(prefix="openpr-wp28-mcp-cli-") as temp:
    config = pathlib.Path(temp) / "openpr.toml"
    config.write_text(f'''[database]
url = "postgres://openpr:unused@127.0.0.1:5432/openpr"

[auth]
jwt_secret = "unused-wp28-jwt-secret"

[logging]
filter = "error"
format = "text"

[mcp]
api_url = "http://127.0.0.1:{server.server_port}"
bot_token = "opr_wp28_semantic_equivalence"
workspace_id = "{workspace}"
transport = "stdio"
''', encoding="utf-8")
    cli = subprocess.run(
        [cli_bin, "--config", str(config), "collab", "projection-lag",
         "--workspace", workspace, "--limit", "2"],
        cwd=temp, text=True, capture_output=True, timeout=30, check=False,
    )
    request = json.dumps({
        "jsonrpc": "2.0", "id": 28, "method": "tools/call",
        "params": {"name": "collab.projection_lag",
                   "arguments": {"workspace_id": workspace, "limit": 2}},
    }) + "\n"
    mcp = subprocess.run(
        [mcp_bin, "serve", "--config", str(config), "--transport", "stdio"],
        cwd=temp, input=request, text=True, capture_output=True, timeout=30, check=False,
    )
server.shutdown()
thread.join(timeout=5)

errors = []
try:
    cli_json = json.loads(cli.stdout)
except Exception as error:
    cli_json = None
    errors.append(f"CLI JSON parse failed: {error}")
try:
    rpc_json = json.loads(mcp.stdout.strip().splitlines()[-1])
except Exception as error:
    rpc_json = None
    errors.append(f"MCP JSON-RPC parse failed: {error}")
mcp_data = None
try:
    mcp_data = json.loads(rpc_json["result"]["content"][0]["text"])
except Exception as error:
    errors.append(f"MCP semantic payload parse failed: {error}")
cli_data = cli_json.get("data") if isinstance(cli_json, dict) else None
expected_path = f"/api/v1/workspaces/{workspace}/flow/projection-lag?limit=2"
paths = [urllib.parse.urlsplit(call["path"]).path + ("?" + urllib.parse.urlsplit(call["path"]).query if urllib.parse.urlsplit(call["path"]).query else "") for call in calls]
if cli.returncode != 0:
    errors.append(f"CLI exit {cli.returncode}: {cli.stderr}")
if mcp.returncode != 0:
    errors.append(f"MCP exit {mcp.returncode}: {mcp.stderr}")
if cli_data != data or mcp_data != data or cli_data != mcp_data:
    errors.append("CLI and MCP semantic data were not exactly equivalent to the captured API data")
if len(paths) != 2 or any(path != expected_path for path in paths):
    errors.append(f"captured REST paths differ: {paths}")
if len(calls) != 2 or not all(call["authorization_present"] for call in calls):
    errors.append("both binary calls did not present configured identity")

result = {
    "passed": not errors,
    "errors": errors,
    "cli_exit": cli.returncode,
    "mcp_exit": mcp.returncode,
    "cli_schema_version": cli_json.get("schema_version") if isinstance(cli_json, dict) else None,
    "cli_command": cli_json.get("command") if isinstance(cli_json, dict) else None,
    "semantic_equal": cli_data == mcp_data == data,
    "captured_calls": calls,
    "expected_path": expected_path,
}
print(json.dumps(result, sort_keys=True))
raise SystemExit(0 if not errors else 1)
PY
PROBE_EXIT=$?
set -e
PROBE_END="$(date +%s%N)"
PROBE_MS="$(((PROBE_END - PROBE_START) / 1000000))"
echo "=== shipped binary equivalence exit=$PROBE_EXIT duration_ms=$PROBE_MS log=$PROBE_LOG ===" >&2

readarray -t DB_TESTS <<'EOF'
routes::flow::flow_database_tests::projection_lag_filters_before_aggregating_and_cursors_from_returned_rows
routes::flow::flow_database_tests::projection_lag_large_scope_remains_usable_and_pageable
EOF
: >"$TEST_LOG"
TEST_START="$(date +%s%N)"
TEST_EXIT=0
for test_name in "${DB_TESTS[@]}"; do
  printf '=== %s ===\n' "$test_name" >>"$TEST_LOG"
  set +e
  (cd "$REPO_ROOT" && OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
    cargo test --locked -p api --lib --no-fail-fast "$test_name" -- --exact --nocapture --test-threads=1) >>"$TEST_LOG" 2>&1
  one_exit=$?
  set -e
  printf '=== exit=%s test=%s ===\n' "$one_exit" "$test_name" >>"$TEST_LOG"
  [[ $one_exit -eq 0 ]] || TEST_EXIT=$one_exit
done
TEST_END="$(date +%s%N)"
TEST_MS="$(((TEST_END - TEST_START) / 1000000))"
echo "=== projection-lag policy tests exit=$TEST_EXIT duration_ms=$TEST_MS log=$TEST_LOG ===" >&2

MUTATION_START="$(date +%s%N)"
set +e
python3 - "$MCP_CONTRACT" "$LIVE_LOG" >"$MUTATION_LOG" 2>&1 <<'PY'
import pathlib
import re
import sys

contract = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
live = pathlib.Path(sys.argv[2]).read_text(encoding="utf-8", errors="replace")
tool_section = contract.split("## Tools", 1)[1].split("## Resources", 1)[0]
required = []
for line in tool_section.splitlines():
    match = re.match(r"^\| `([^`]+)` \| (0\.[45](?: conditional)?) \|", line)
    if match:
        required.append(match.group(1))
live_names = re.findall(r"^  ([A-Za-z0-9_.-]+)\n   [^\n]", live, re.M)
if not required or not live_names:
    raise SystemExit("MUTATION_SETUP_FAILED_NONEMPTY_PARSE")
removed = next((name for name in required if name in live_names), None)
if removed is None:
    raise SystemExit("MUTATION_SETUP_FAILED_NO_INTERSECTION")
mutated = [name for name in live_names if name != removed]
missing = sorted(set(required) - set(mutated))
print(f"WP28_MUTATION_REGISTRY_REMOVE_ACTIVE removed={removed}")
if missing:
    print("REGISTRY_GATE_RED missing=" + ",".join(missing))
    raise SystemExit(1)
print("REGISTRY_GATE_FALSE_GREEN")
raise SystemExit(0)
PY
MUTATION_EXIT=$?
set -e
MUTATION_END="$(date +%s%N)"
MUTATION_MS="$(((MUTATION_END - MUTATION_START) / 1000000))"
echo "=== registry mutation exit=$MUTATION_EXIT duration_ms=$MUTATION_MS log=$MUTATION_LOG ===" >&2

ANALYSIS_JSON="$(python3 - \
  "$MCP_CONTRACT" "$SURFACE_CONTRACT" "$ADR_PATH" "$BASELINE_CONTRACT" \
  "$LIVE_LOG" "$LIVE_EXIT" "$LIVE_MS" "$BUILD_LOG" "$BUILD_EXIT" "$BUILD_MS" \
  "$WORKER_BUILD_LOG" "$WORKER_BUILD_EXIT" "$WORKER_BUILD_MS" \
  "$PROBE_LOG" "$PROBE_STDERR" "$PROBE_EXIT" "$PROBE_MS" \
  "$TEST_LOG" "$TEST_EXIT" "$TEST_MS" "$DATABASE_URL" \
  "$MUTATION_LOG" "$MUTATION_EXIT" "$MUTATION_MS" "${DB_TESTS[0]}" "${DB_TESTS[1]}" <<'PY'
import hashlib
import json
import pathlib
import re
import sys

(mcp_s, surface_s, adr_s, baseline_s, live_log_s, live_exit_s, live_ms_s,
 build_log_s, build_exit_s, build_ms_s, worker_log_s, worker_exit_s, worker_ms_s,
 probe_log_s, probe_stderr_s, probe_exit_s, probe_ms_s, test_log_s, test_exit_s,
 test_ms_s, database_url, mutation_log_s, mutation_exit_s, mutation_ms_s,
 lag_test, large_test) = sys.argv[1:]

def read(path):
    return pathlib.Path(path).read_text(encoding="utf-8", errors="replace")

mcp = read(mcp_s)
surface = read(surface_s)
adr = read(adr_s)
baseline = read(baseline_s)
live = read(live_log_s)
test_log = read(test_log_s)
mutation_log = read(mutation_log_s)

base_match = re.search(r"当前源码基线：([0-9]+) tools", mcp)
tool_section = mcp.split("## Tools", 1)[1].split("## Resources", 1)[0]
rows = []
for line in tool_section.splitlines():
    match = re.match(r"^\| `([^`]+)` \| (0\.[0-9]+)(?: conditional)? \|", line)
    if match:
        rows.append((match.group(1), match.group(2)))
if not base_match or not rows:
    raise SystemExit("MCP contract parse failed: non-empty base and tool table required")
base_count = int(base_match.group(1))
required_names = sorted({name for name, version in rows if version in {"0.4", "0.5"}})
if not required_names:
    raise SystemExit("MCP contract parse failed: v0.4/v0.5 required tool set is empty")
expected_from_table = base_count + len(required_names)

total_match = re.search(r"v0\.5 `([0-9]+)`", mcp)
baseline_match = re.search(r"^\| 0\.5 \|.*?\| ([0-9]+) \|$", baseline, re.M)
baseline_totals = {
    int(match.group(1))
    for match in re.finditer(r"^\| 0\.[5-9] \|.*?\| ([0-9]+) \|$", baseline, re.M)
}
adr_delegates_to_table = "逐工具表为准" in adr and "不得硬编码" in adr
adr_match = re.search(r"v0\.5 累计总数.*?\*{0,2}([0-9]+)\*{0,2}", adr)
if not total_match or not baseline_match or not baseline_totals or not adr_delegates_to_table:
    raise SystemExit("G6 count parse failed: every source must be non-empty")
mcp_declared = int(total_match.group(1))
baseline_declared = int(baseline_match.group(1))
adr_declared = int(adr_match.group(1)) if adr_match else None

header_match = re.search(r"Available MCP Tools \(([0-9]+) total\):", live)
live_names = re.findall(r"^  ([A-Za-z0-9_.-]+)\n   [^\n]", live, re.M)
if not header_match or not live_names or len(live_names) != len(set(live_names)):
    raise SystemExit("live registry parse failed: non-empty unique registry and header required")
live_header_count = int(header_match.group(1))
live_count = len(live_names)
missing_required = sorted(set(required_names) - set(live_names))
names_hash = hashlib.sha256(("\n".join(sorted(live_names)) + "\n").encode()).hexdigest()
registry_pass = (
    int(build_exit_s) == 0 and int(live_exit_s) == 0 and live_header_count == live_count
    and expected_from_table == mcp_declared == baseline_declared
    and live_count in baseline_totals and live_count >= expected_from_table
    and not missing_required
)

allowed = set()
for line in surface.split("## 49-endpoint coverage matrix", 1)[0].splitlines():
    match = re.match(r"^\| `([^`]+)` \| (.*) \|$", line)
    if match and "MCP" in match.group(2) and "CLI-only" not in match.group(2):
        allowed.add(match.group(1))
matrix = surface.split("## 49-endpoint coverage matrix", 1)[1].split("## 本轮对账", 1)[0]
exceptions = []
for line in matrix.splitlines():
    cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
    if len(cells) != 6 or not re.fullmatch(r"0\.[45](?: conditional)?", cells[1]):
        continue
    if cells[2].startswith("`not_exposed:"):
        reason = cells[2].split("not_exposed:", 1)[1].split("`", 1)[0]
        exceptions.append({"endpoint": cells[0].strip("`"), "reason": reason})
if not allowed or not exceptions:
    raise SystemExit("surface exception parse failed: allowlist and actual set must be non-empty")
coverage_pass = {item["reason"] for item in exceptions} == allowed and len(exceptions) == len(allowed)

try:
    probe = json.loads(read(probe_log_s))
except Exception:
    probe = {"passed": False, "errors": [read(probe_stderr_s)]}
probe_pass = int(probe_exit_s) == 0 and probe.get("passed") is True and probe.get("semantic_equal") is True

tests = [lag_test, large_test]
passed_tests = [name for name in tests if f"test {name} ... ok" in test_log and f"=== exit=0 test={name} ===" in test_log]
summary_count = len(re.findall(r"^test result: ok\. 1 passed; 0 failed;", test_log, re.M))
db_pass = (
    int(worker_exit_s) == 0 and int(test_exit_s) == 0 and "skipped:" not in test_log.lower()
    and len(passed_tests) == len(tests) and summary_count == len(tests)
)

mutation_red = (
    int(mutation_exit_s) != 0
    and "WP28_MUTATION_REGISTRY_REMOVE_ACTIVE" in mutation_log
    and "REGISTRY_GATE_RED missing=" in mutation_log
)

observed = [
    {"kind": "contract_registry_parse", "base_count": base_count,
     "v04_v05_table_rows": len(required_names), "expected_from_table": expected_from_table,
     "mcp_declared_v05": mcp_declared, "baseline_declared_v05": baseline_declared,
     "allowed_rebased_totals": sorted(baseline_totals),
     "parse_nonempty": True},
    {"kind": "live_registry", "header_count": live_header_count, "parsed_count": live_count,
     "missing_required": missing_required, "sorted_names_sha256": names_hash,
     "exit": int(live_exit_s), "duration_ms": int(live_ms_s), "log": live_log_s},
    {"kind": "contract_conflict", "id": "G6", "mcp_table_authoritative_total": expected_from_table,
     "mcp_declared_total": mcp_declared, "adr_0009_total": adr_declared,
     "delegates_to_tool_table": adr_delegates_to_table,
     "conflict_present": adr_declared is not None and adr_declared != expected_from_table,
     "resolution": "per-tool table plus live registry"},
    {"kind": "cargo_build", "targets": ["mcp-server", "sylvode", "list-tools"],
     "exit": int(build_exit_s), "duration_ms": int(build_ms_s), "log": build_log_s},
    {"kind": "cargo_build", "target": "collab-isolated-apply-worker",
     "exit": int(worker_exit_s), "duration_ms": int(worker_ms_s), "log": worker_log_s},
    {"kind": "shipped_binary_probe", "command": "collab.projection_lag", "probe": probe,
     "exit": int(probe_exit_s), "duration_ms": int(probe_ms_s), "log": probe_log_s},
    {"kind": "cargo_test", "database_url": database_url, "tests": tests,
     "passed_count": len(passed_tests), "strict_ok_summary_count": summary_count,
     "exit": int(test_exit_s), "duration_ms": int(test_ms_s),
     "skipped_marker_seen": "skipped:" in test_log.lower(), "log": test_log_s},
    {"kind": "surface_exceptions", "allowed_reasons": sorted(allowed),
     "v05_not_exposed": exceptions, "exact_match": coverage_pass},
    {"kind": "mutation", "criterion": "live registry contains every parsed v0.4/v0.5 tool",
     "mutation": "remove first parsed required live tool", "exit": int(mutation_exit_s),
     "duration_ms": int(mutation_ms_s), "red": mutation_red, "log": mutation_log_s},
]
print(json.dumps({
    "registry_pass": registry_pass, "probe_pass": probe_pass, "db_pass": db_pass,
    "coverage_pass": coverage_pass, "mutation_red": mutation_red, "observed": observed,
}, separators=(",", ":")))
PY
)" || { echo "FAIL: MCP/CLI evidence parsing failed" >&2; exit 2; }

REGISTRY_PASS="$(jq -r '.registry_pass' <<<"$ANALYSIS_JSON")"
PROBE_PASS="$(jq -r '.probe_pass' <<<"$ANALYSIS_JSON")"
DB_PASS="$(jq -r '.db_pass' <<<"$ANALYSIS_JSON")"
COVERAGE_PASS="$(jq -r '.coverage_pass' <<<"$ANALYSIS_JSON")"
MUTATION_RED="$(jq -r '.mutation_red' <<<"$ANALYSIS_JSON")"
EQUIVALENCE_PASS=false
[[ "$PROBE_PASS" == true && "$MUTATION_RED" == true ]] && EQUIVALENCE_PASS=true
LAG_PASS=false
[[ "$PROBE_PASS" == true && "$DB_PASS" == true ]] && LAG_PASS=true
[[ "$MUTATION_RED" == true ]] || REGISTRY_PASS=false
PASSED=false
[[ "$EQUIVALENCE_PASS" == true && "$REGISTRY_PASS" == true && "$LAG_PASS" == true && "$COVERAGE_PASS" == true && "$SOURCE_DIRTY" == false ]] && PASSED=true

gate_entry() {
  local value="$1" reason="$2"
  if [[ "$value" == true ]]; then jq -nc '{status:"passed",passed:true,reason_code:null}'
  else jq -nc --arg reason "$reason" '{status:"failed",passed:false,reason_code:$reason}'; fi
}
EQUIVALENCE_GATE="$(gate_entry "$EQUIVALENCE_PASS" mcp_cli_shipped_binary_equivalence_failed)"
REGISTRY_GATE="$(gate_entry "$REGISTRY_PASS" tool_registry_contract_or_live_count_failed)"
LAG_GATE="$(gate_entry "$LAG_PASS" projection_lag_policy_equivalence_failed)"
COVERAGE_GATE="$(gate_entry "$COVERAGE_PASS" mcp_default_rest_exception_set_drifted)"

ARTIFACT="$EVIDENCE_REAL/mcp-cli-equivalence-result.json"
TMP_ARTIFACT="$ARTIFACT.tmp.$$"
jq -n --arg schema "openpr.flow.mcp-cli-equivalence.v0.5" --arg generated_at "$GENERATED_AT" \
  --arg source_head "$SOURCE_HEAD" --argjson source_dirty "$SOURCE_DIRTY" \
  --argjson source_dirty_entries "$SOURCE_DIRTY_ENTRIES" --argjson passed "$PASSED" \
  --argjson mutation_red "$MUTATION_RED" --argjson equivalence_gate "$EQUIVALENCE_GATE" \
  --argjson registry_gate "$REGISTRY_GATE" --argjson lag_gate "$LAG_GATE" \
  --argjson coverage_gate "$COVERAGE_GATE" --argjson analysis "$ANALYSIS_JSON" \
  '{schema:$schema,generated_at:$generated_at,source_head:$source_head,
    source_dirty:$source_dirty,source_dirty_entries:$source_dirty_entries,passed:$passed,
    gates:{mcp_cli_semantic_equivalence:$equivalence_gate,
      tool_registry_expected_119_or_rebased:$registry_gate,
      projection_lag_mcp_cli_policy_filtered_equivalence:$lag_gate,
      mcp_default_rest_coverage_three_adr_threat_exceptions_only:$coverage_gate},
    mutation:{required:true,red:$mutation_red},observed:$analysis.observed}' >"$TMP_ARTIFACT"
mv "$TMP_ARTIFACT" "$ARTIFACT"
jq empty "$ARTIFACT" || exit 2
cat "$ARTIFACT"
[[ "$PASSED" == true ]] && exit 0
exit 1
