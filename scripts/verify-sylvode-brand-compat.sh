#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
JSON_MODE=0
EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.9"
while (($#)); do
  case "$1" in
    --json) JSON_MODE=1; shift ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $JSON_MODE -eq 1 ]] || { echo "FAIL: require --json" >&2; exit 2; }
for command_name in cargo python3; do command -v "$command_name" >/dev/null || { echo "FAIL: missing $command_name" >&2; exit 2; }; done
if command -v bun >/dev/null; then
  BUN=$(command -v bun)
elif [[ -x /home/ck/.bun/bin/bun ]]; then
  BUN=/home/ck/.bun/bin/bun
else
  echo "FAIL: missing bun" >&2
  exit 2
fi

CACHE_ROOT=/opt/worker/.cache/v09-brand-compat
LOG_ROOT="$CACHE_ROOT/logs"
mkdir -p "$LOG_ROOT" "$EVIDENCE_ROOT"

run_logged() {
  local name=$1
  shift
  "$@" >"$LOG_ROOT/$name.log" 2>&1
}

run_logged platform-config-test env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 \
  cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p platform \
  sylvode_ -- --nocapture
run_logged mcp-display-test env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 \
  cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p mcp-server \
  initialize_uses_sylvode_display_identity -- --nocapture
run_logged binary-build env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 \
  cargo build --manifest-path "$REPO_ROOT/Cargo.toml" -p api -p mcp-server --bins
run_logged api-help "$REPO_ROOT/target/debug/api" --help
run_logged sylvode-help "$REPO_ROOT/target/debug/sylvode" --help
run_logged legacy-mcp-help "$REPO_ROOT/target/debug/mcp-server" --help
run_logged frontend-build "$BUN" run --cwd "$REPO_ROOT/frontend" build

COMPAT_DIR=$(mktemp -d "$CACHE_ROOT/compat.XXXXXX")
mkdir -p "$COMPAT_DIR/config"
# shellcheck source=scripts/lib/sylvode_compat.sh
source "$REPO_ROOT/scripts/lib/sylvode_compat.sh"
[[ $(cd "$COMPAT_DIR" && sylvode_select_config config/sylvode.toml config/openpr.toml) == config/sylvode.toml ]]
: >"$COMPAT_DIR/config/openpr.toml"
[[ $(cd "$COMPAT_DIR" && sylvode_select_config config/sylvode.toml config/openpr.toml) == config/openpr.toml ]]
: >"$COMPAT_DIR/config/sylvode.toml"
set +e
(cd "$COMPAT_DIR" && sylvode_select_config config/sylvode.toml config/openpr.toml) >"$LOG_ROOT/config-conflict.log" 2>&1
CONFIG_CONFLICT_EXIT=$?
set -e
[[ $CONFIG_CONFLICT_EXIT -ne 0 ]]

ENV_FIXTURE="$COMPAT_DIR/environment"
printf 'OPENPR_API_PORT=18081\n' >"$ENV_FIXTURE"
[[ $(sylvode_resolve_env "$ENV_FIXTURE" SYLVODE_API_PORT OPENPR_API_PORT 8081) == 18081 ]]
printf 'SYLVODE_API_PORT=28081\nOPENPR_API_PORT=28081\n' >"$ENV_FIXTURE"
[[ $(sylvode_resolve_env "$ENV_FIXTURE" SYLVODE_API_PORT OPENPR_API_PORT 8081) == 28081 ]]
printf 'SYLVODE_API_PORT=28081\nOPENPR_API_PORT=18081\n' >"$ENV_FIXTURE"
set +e
sylvode_resolve_env "$ENV_FIXTURE" SYLVODE_API_PORT OPENPR_API_PORT 8081 >"$LOG_ROOT/env-conflict.log" 2>&1
ENV_CONFLICT_EXIT=$?
set -e
[[ $ENV_CONFLICT_EXIT -ne 0 ]]

python3 - "$REPO_ROOT" "$EVIDENCE_ROOT" "$LOG_ROOT" "$CONFIG_CONFLICT_EXIT" "$ENV_CONFLICT_EXIT" <<'PY'
import datetime as dt
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile

repo, evidence, logs = map(pathlib.Path, sys.argv[1:4])
config_conflict_exit, env_conflict_exit = map(int, sys.argv[4:6])

def log_check(name, required):
    path = logs / f"{name}.log"
    body = path.read_text(errors="replace")
    missing = [value for value in required if value not in body]
    return {
        "name": name,
        "status": "passed" if not missing else "failed",
        "missing": missing,
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
    }

checks = [
    log_check("api-help", ["Sylvode API server", "config/sylvode.toml"]),
    log_check("sylvode-help", ["Sylvode Flow CLI", "config/sylvode.toml"]),
    log_check("legacy-mcp-help", ["Sylvode MCP server", "config/sylvode.toml"]),
]
for name in ("platform-config-test", "mcp-display-test"):
    path = logs / f"{name}.log"
    body = path.read_text(errors="replace")
    summaries = re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;", body, re.M)
    executed = sum(int(passed) + int(failed) for _, passed, failed, _ in summaries)
    ignored = sum(int(value) for *_, value in summaries)
    ok = executed > 0 and ignored == 0 and all(state == "ok" and int(failed) == 0 for state, _, failed, _ in summaries)
    checks.append({"name": name, "status": "passed" if ok else "failed", "executed_count": executed,
                   "ignored_count": ignored, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})

build_files = [path for path in (repo / "frontend/build").rglob("*") if path.is_file()]
build_has_sylvode = any(b"Sylvode" in path.read_bytes() for path in build_files)
checks.append({"name": "frontend-built-product-brand", "status": "passed" if build_has_sylvode else "failed",
               "files_examined": len(build_files)})

fixture_path = repo / "testing/fixtures/v09-openpr-schema-ids.json"
expected = json.loads(fixture_path.read_text())
actual = []
for path in sorted((repo / "docs/schemas").glob("*.json")):
    payload = json.loads(path.read_text())
    schema_id = payload.get("$id")
    relative = str(path.relative_to(repo))
    if schema_id and ("openpr" in schema_id or path.name.startswith("openpr-")):
        actual.append({"file": relative, "id": schema_id})
schema_ok = actual == expected and len(actual) > 0
checks.append({"name": "legacy-api-schema-ids-frozen", "status": "passed" if schema_ok else "failed",
               "expected_count": len(expected), "actual_count": len(actual)})

matrix = (repo / "docs/sylvode-v0.9-compatibility.md").read_text()
required_surfaces = ["Product, Web UI, current docs", "CLI", "Configuration", "Compose environment",
                     "REST API and schema IDs", "MCP tools", "MCP resources", "Database and migrations",
                     "Release archives", "Telemetry and containers"]
missing_surfaces = [surface for surface in required_surfaces if f"| {surface} |" not in matrix]
checks.append({"name": "compatibility-matrix-complete", "status": "passed" if not missing_surfaces else "failed",
               "surface_count": len(required_surfaces), "missing": missing_surfaces})

mutations = [
    {"name": "both-config-names", "exit_code": config_conflict_exit, "detected": config_conflict_exit != 0},
    {"name": "conflicting-env-aliases", "exit_code": env_conflict_exit, "detected": env_conflict_exit != 0},
]
passed = all(check["status"] == "passed" for check in checks) and all(row["detected"] for row in mutations)
result = {
    "schema_version": "sylvode.flow.brand-compat-result.v1",
    "release": "0.9.0",
    "source_head": subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip(),
    "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
    "checks": checks,
    "mutation_controls": mutations,
    "executed_count": len(checks) + len(mutations),
    "passed": passed,
}
fd, temporary = tempfile.mkstemp(prefix=".brand-compat.", dir=evidence)
with os.fdopen(fd, "w") as handle:
    json.dump(result, handle, sort_keys=True, indent=2)
    handle.write("\n")
os.replace(temporary, evidence / "brand-compat-result.json")
print(json.dumps(result, sort_keys=True))
raise SystemExit(0 if passed else 1)
PY
