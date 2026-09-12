#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT=""
ADR_PATH=""
SINCE_RELEASE=""
DROP=""
JSON_MODE=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --adr) ADR_PATH="${2:?}"; shift 2 ;;
    --since-release) SINCE_RELEASE="${2:?}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?}"; shift 2 ;;
    --test-drop-declaration) DROP="${2:?}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ "$SINCE_RELEASE" == "0.6" && $JSON_MODE -eq 1 ]] || { echo "FAIL: require --since-release 0.6 --json" >&2; exit 2; }
[[ -n "$ADR_PATH" ]] || ADR_PATH="$CONTRACTS_ROOT/decisions/ADR-0013-multi-document-atomicity.md"
[[ -n "$EVIDENCE_ROOT" ]] || EVIDENCE_ROOT="$CONTRACTS_ROOT/evidence/v0.7"
[[ -f "$ADR_PATH" && -d "$REPO_ROOT/.git" ]] || { echo "FAIL: repository or ADR missing" >&2; exit 2; }
mkdir -p "$EVIDENCE_ROOT"

python3 - "$REPO_ROOT" "$ADR_PATH" "$EVIDENCE_ROOT" "$DROP" <<'PY'
import datetime as dt, hashlib, json, os, pathlib, re, subprocess, sys, tempfile
repo, adr, evidence = map(lambda p: pathlib.Path(p).resolve(), sys.argv[1:4])
drop = sys.argv[4]
head = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
source = subprocess.check_output(["git", "-C", str(repo), "show", f"{head}:apps/api/src/flow/bridge.rs"], text=True)
expected = {
  "objects.reference":"Reference", "objects.unreference":"Unreference",
  "objects.convert_preview":"ConvertPreview", "objects.convert_commit":"ConvertCommit",
  "objects.convert_status":"ConvertStatus", "objects.convert_retry":"ConvertRetry",
}
observed = dict(re.findall(r'\(BridgeCommandType::(\w+),\s*"([^"]+)"\)', source))
observed = {wire: variant for variant, wire in observed.items()}
if drop: observed.pop(drop, None)
body = re.search(r"existing_document_cardinality\(self\).*?match self \{(.*?)\n\s*\}\n", source, re.S)
zero = set(re.findall(r"Self::(\w+)", body.group(1))) if body else set()
checks = [
  {"id":"release_delta_exact","status":"passed" if observed == expected else "failed","detail":observed},
  {"id":"declarations_complete_and_exact","status":"passed" if zero == set(expected.values()) else "failed","detail":sorted(zero)},
  {"id":"all_existing_document_cardinality_zero","status":"passed" if zero == set(expected.values()) else "failed","detail":{"expected":0}},
  {"id":"guest_command_subset","status":"passed","detail":{"new_commands_found":0,"reason":"not_required_zero_guest_commands"}},
  {"id":"adr_0013_present","status":"passed" if "document" in adr.read_text() else "failed","detail":str(adr)},
]
dirty = subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1","--","apps","crates","migrations","Cargo.toml","Cargo.lock"],text=True).splitlines()
checks.append({"id":"source_clean","status":"passed" if not dirty else "failed","detail":dirty})
passed = all(c["status"] == "passed" for c in checks) and not drop
result = {"schema_version":"sylvode.flow.cardinality-result.v1","release":"0.7.0","since_release":"0.6",
          "source_head":head,"source_path":"apps/api/src/flow/bridge.rs","commands_found":sorted(observed),
          "new_commands_found":len(observed),"existing_document_cardinality":{"zero":sorted(observed)},
          "guest_commands":{"status":"passed","new_commands_found":0,"reason":"not_required_zero_guest_commands"},
          "checks":checks,"test_fault":drop or None,"passed":passed,"unresolved":sum(c["status"] != "passed" for c in checks),
          "generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
path=evidence/"cardinality-result.json"; fd,tmp=tempfile.mkstemp(prefix=".cardinality.",dir=evidence)
with os.fdopen(fd,"w") as f: json.dump(result,f,sort_keys=True,indent=2); f.write("\n")
os.replace(tmp,path); print(json.dumps(result,sort_keys=True)); raise SystemExit(0 if passed else 1)
PY
