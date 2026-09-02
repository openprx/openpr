#!/usr/bin/env bash
set -euo pipefail

# Fast, side-effect-free query for the API binary currently named by a Flow
# deployment descriptor. It does not run the WebSocket chain and never touches
# a database. stdout is one JSON result; exit 0 means provenance passed, exit 1
# means a named fail-closed state, and exit 2 means CLI/tool misuse.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
DEPLOYMENT_PATH="${FLOW_DEPLOYMENT_DESCRIPTOR:-}"
PROBE_JSON=""
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-binary-provenance.sh --json [OPTIONS]

Queries the exact deployed API binary's sha256 and embedded --build-info, then
compares its commit and clean state with --repo-root HEAD. This is the quick
provenance-only check; it does not run the deployed WebSocket chain.

Options:
  --json               Required; emit one JSON result on stdout.
  --deployment PATH    Deployment descriptor. Default: $FLOW_DEPLOYMENT_DESCRIPTOR,
                       else <repo-root>/deploy/flow-deployed-websocket.json.
  --repo-root DIR      Git work tree whose HEAD the binary must match.
  --probe-json PATH    Test-only captured probe input; do not contact a container.
  -h, --help           Show this help and exit 0.

Exit codes: 0 passed; 1 a named provenance failure; 2 usage/tool error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --json) JSON_MODE=1; shift ;;
    --deployment) DEPLOYMENT_PATH="${2:?--deployment requires a PATH}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR}"; shift 2 ;;
    --probe-json) PROBE_JSON="${2:?--probe-json requires a PATH}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  exit 2
fi
for tool in git python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    exit 2
  fi
done
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi
if [[ -z "$DEPLOYMENT_PATH" ]]; then
  DEPLOYMENT_PATH="$REPO_ROOT/deploy/flow-deployed-websocket.json"
fi
if [[ -n "$PROBE_JSON" && ! -f "$PROBE_JSON" ]]; then
  echo "FAIL: --probe-json does not exist: $PROBE_JSON" >&2
  exit 2
fi

python3 - "$REPO_ROOT" "$DEPLOYMENT_PATH" "$PROBE_JSON" <<'PYEOF'
import json
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace

repo_root, deployment_path, probe_path = sys.argv[1:4]
commit_re = re.compile(r"^[0-9a-f]{40}$")
digest_re = re.compile(r"^[0-9a-f]{64}$")
committer_date_re = re.compile(
    r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$"
)


def run(argv):
    try:
        return subprocess.run(argv, text=True, capture_output=True, check=False, timeout=60)
    except (OSError, subprocess.TimeoutExpired) as exc:
        return SimpleNamespace(returncode=127, stdout="", stderr=str(exc))


def git(*args):
    result = run(["git", "-C", repo_root, *args])
    return result.stdout.strip() if result.returncode == 0 else None


def read_json(path):
    try:
        value = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    return value if isinstance(value, dict) else None


source_head = git("rev-parse", "HEAD")
source_status = git("status", "--porcelain")
source_dirty = None if source_status is None else bool(source_status)
probe = None
deployment_summary = {"descriptor": deployment_path}

if probe_path:
    probe = read_json(probe_path)
    deployment_summary["probe_mode"] = "captured_test_input"
elif Path(deployment_path).is_file():
    descriptor = read_json(deployment_path)
    if descriptor is not None:
        hops = descriptor.get("hops")
        api_hop = next(
            (hop for hop in hops if isinstance(hop, dict) and hop.get("name") == "api"),
            None,
        ) if isinstance(hops, list) else None
        binary_path = api_hop.get("binary_path") if isinstance(api_hop, dict) else None
        binary_probe = api_hop.get("binary_probe") if isinstance(api_hop, dict) else None
        if not isinstance(binary_path, str) and (
            isinstance(binary_probe, list)
            and len(binary_probe) == 2
            and binary_probe[0] == "sha256sum"
            and isinstance(binary_probe[1], str)
        ):
            binary_path = binary_probe[1]
        container = api_hop.get("container") if isinstance(api_hop, dict) else None
        container_cli = descriptor.get("container_cli", "docker")
        deployment_summary.update({
            "api_container": container,
            "binary_path": binary_path,
            "container_cli": container_cli,
        })
        if all(isinstance(value, str) and value for value in (container_cli, container, binary_path)):
            inspect = run([container_cli, "inspect", container])
            digest = run([container_cli, "exec", container, "sha256sum", binary_path])
            metadata = run([container_cli, "exec", container, binary_path, "--build-info"])
            inspect_value = None
            if inspect.returncode == 0:
                try:
                    inspect_json = json.loads(inspect.stdout)
                    item = inspect_json[0] if isinstance(inspect_json, list) and inspect_json else {}
                    inspect_value = {
                        "id": item.get("Id"),
                        "image": item.get("Image"),
                        "name": item.get("Name"),
                    }
                except json.JSONDecodeError:
                    pass
            build_metadata = None
            if metadata.returncode == 0:
                try:
                    build_metadata = json.loads(metadata.stdout)
                except json.JSONDecodeError:
                    pass
            probe = {
                "producer_specified": True,
                "binary_available": digest.returncode == 0,
                "binary_sha256": digest.stdout.split()[0] if digest.returncode == 0 and digest.stdout.split() else None,
                "binary_error": digest.stderr.strip() or None,
                "build_metadata_available": metadata.returncode == 0 and bool(metadata.stdout.strip()),
                "build_metadata": build_metadata,
                "build_metadata_error": metadata.stderr.strip() or None,
                "container_identity": inspect_value,
            }

if probe is None:
    probe = {"producer_specified": False}

binary_available = probe.get("binary_available") is True
binary_sha256 = probe.get("binary_sha256")
metadata_available = probe.get("build_metadata_available") is True
metadata = probe.get("build_metadata")

status = "passed"
reason = "deployed binary digest and embedded clean commit match source HEAD"
if not commit_re.fullmatch(source_head or ""):
    status, reason = "source_head_missing", "source repository HEAD is unavailable or malformed"
elif source_dirty is None:
    status, reason = "source_clean_unproven", "source repository cleanliness could not be queried"
elif source_dirty:
    status, reason = "source_dirty", "source repository has uncommitted changes"
elif probe.get("producer_specified") is not True:
    status, reason = "producer_unspecified", "deployment descriptor does not identify an API binary producer"
elif not binary_available:
    status, reason = "binary_unavailable", "deployed API binary could not be read and hashed"
elif not isinstance(binary_sha256, str) or not digest_re.fullmatch(binary_sha256):
    status, reason = "binary_digest_malformed", "deployed API binary sha256 is missing or malformed"
elif not metadata_available:
    status, reason = "build_metadata_unavailable", "deployed API binary --build-info was unavailable"
elif not isinstance(metadata, dict) or metadata.get("schema_version") != "openpr.build-info.v1":
    status, reason = "build_metadata_malformed", "deployed API binary returned malformed build metadata"
elif metadata.get("source") not in ("git", "environment") or metadata.get("git_commit") is None:
    status, reason = "build_source_unknown", "deployed API binary build source or commit is unknown"
elif (
    not commit_re.fullmatch(metadata.get("git_commit", ""))
    or not isinstance(metadata.get("git_dirty"), bool)
    or not committer_date_re.fullmatch(metadata.get("git_committer_date", ""))
):
    status, reason = "build_metadata_malformed", "embedded commit, dirty, or committer-date field is malformed"
elif metadata["git_dirty"]:
    status, reason = "source_dirty", "deployed API binary was built from a dirty source tree"
elif metadata["git_commit"] != source_head:
    status, reason = "source_head_mismatch", "deployed API binary commit does not match source HEAD"

result = {
    "schema_version": "openpr.flow.binary-provenance.v1",
    "checked_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
    "source_head": source_head,
    "source_dirty": source_dirty,
    "deployment": deployment_summary,
    "binary": {
        "available": binary_available,
        "sha256": binary_sha256 if isinstance(binary_sha256, str) else None,
        "error": probe.get("binary_error"),
    },
    "build_metadata_available": metadata_available,
    "build_metadata": metadata if isinstance(metadata, dict) else None,
    "build_metadata_error": probe.get("build_metadata_error"),
    "container_identity": probe.get("container_identity"),
    "status": status,
    "reason": reason,
    "passed": status == "passed",
}
json.dump(result, sys.stdout, sort_keys=True, separators=(",", ":"))
sys.stdout.write("\n")
sys.exit(0 if result["passed"] else 1)
PYEOF
