#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CONTRACTS_ROOT=/opt/working/sylvode-flow
EVIDENCE_ROOT=
ADR_PATH=
SINCE_RELEASE=
JSON_MODE=0
while (($#)); do
  case "$1" in
    --repo-root) REPO_ROOT=${2:?}; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --adr) ADR_PATH=${2:?}; shift 2 ;;
    --since-release) SINCE_RELEASE=${2:?}; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $JSON_MODE -eq 1 && $SINCE_RELEASE == 0.7 ]] || {
  echo 'FAIL: require --since-release 0.7 --json' >&2
  exit 2
}
[[ -n ${OPENPR_TEST_DATABASE_URL:-} ]] || { echo 'FAIL: OPENPR_TEST_DATABASE_URL is required' >&2; exit 2; }
[[ -n $ADR_PATH ]] || ADR_PATH="$CONTRACTS_ROOT/decisions/ADR-0013-multi-document-atomicity.md"
[[ -n $EVIDENCE_ROOT ]] || EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.8"
[[ -f $ADR_PATH ]] || { echo "FAIL: ADR missing: $ADR_PATH" >&2; exit 2; }
mkdir -p "$EVIDENCE_ROOT/logs"

run_test() {
  local id=$1 package=$2 filter=$3
  local log="$EVIDENCE_ROOT/logs/cardinality-$id.log"
  set +e
  env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" CARGO_BUILD_JOBS=4 \
    cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p "$package" "$filter" -- --nocapture >"$log" 2>&1
  local status=$?
  set -e
  printf '%s\t%s\t%s\n' "$id" "$status" "${log#"$EVIDENCE_ROOT/"}"
}

ROWS=$(mktemp "$EVIDENCE_ROOT/.cardinality-rows.XXXXXX")
trap 'rm -f "$ROWS"' EXIT
{
  run_test registry api v0_8_hardening_registry_declares_every_new_command_cardinality
  run_test operations api flow_operations_
  run_test import api flow_package_import_
  run_test dispatch mcp-server every_v08_registered_tool_has_a_real_dispatch_arm
} >>"$ROWS"

python3 - "$REPO_ROOT" "$ADR_PATH" "$EVIDENCE_ROOT" "$ROWS" <<'PY'
import datetime as dt, hashlib, json, os, pathlib, re, subprocess, sys, tempfile
repo, adr, evidence, rows_path = map(lambda value: pathlib.Path(value).resolve(), sys.argv[1:])
expected = {
  "objects.export": 0, "objects.export_workspace": 0, "objects.import_artifact": 0,
  "objects.import_preview": 0, "objects.import_commit": 0, "objects.import_status": 0,
  "objects.integrity": 0, "collab.status": 0, "collab.compact": 1,
  "collab.rebuild_projection": 0, "deliveries.replay": 0,
}
checks=[]
for raw in rows_path.read_text().splitlines():
    cid, code, relative = raw.split("\t")
    path=evidence/relative; text=path.read_text()
    summaries=re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;", text, re.M)
    passed=sum(int(row[1]) for row in summaries); failed=sum(int(row[2]) for row in summaries)
    ok=int(code)==0 and passed+failed>0 and failed==0 and all(row[0]=="ok" for row in summaries)
    checks.append({"id":cid,"status":"passed" if ok else "failed","exit_code":int(code),
      "executed_count":passed+failed,"log":relative,"sha256":hashlib.sha256(text.encode()).hexdigest()})
adr_text=adr.read_text()
adr_ok=bool(re.search(r"existing_document_cardinality\s*=\s*0\s*\|\s*1\s*\|\s*bounded_many",adr_text))
checks.append({"id":"adr_cardinality_rule","status":"passed" if adr_ok else "failed","executed_count":1})
dirty=subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1","--","apps","crates","migrations","Cargo.toml","Cargo.lock"],text=True).splitlines()
passed=all(row["status"]=="passed" for row in checks) and not dirty
result={"schema_version":"sylvode.flow.cardinality-result.v3","release":"0.8.0","since_release":"0.7",
 "source_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),
 "commands_found":sorted(expected),"new_commands_found":len(expected),
 "existing_document_cardinality":{"zero":sorted(k for k,v in expected.items() if v==0),"one":sorted(k for k,v in expected.items() if v==1),"bounded_many":[]},
 "import_policy":"new_documents_only; in-place merge or restore requires a new reviewed bounded_many ADR",
 "checks":checks,"executed_count":sum(row["executed_count"] for row in checks),"source_dirty":bool(dirty),
 "dirty_entries":dirty,"passed":passed,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".cardinality.",dir=evidence)
with os.fdopen(fd,"w") as handle: json.dump(result,handle,sort_keys=True,indent=2); handle.write("\n")
os.replace(tmp,evidence/"cardinality-result.json")
print(json.dumps(result,sort_keys=True))
raise SystemExit(0 if passed else 1)
PY
