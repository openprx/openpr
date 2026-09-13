#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
EVIDENCE_ROOT=
ALL_POINTS=0
JSON_MODE=0
while (($#)); do
  case "$1" in
    --repo-root) REPO_ROOT=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --all-promotion-points) ALL_POINTS=1; shift ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $ALL_POINTS -eq 1 && $JSON_MODE -eq 1 ]] || { echo 'FAIL: require --all-promotion-points --json' >&2; exit 2; }
[[ -n ${OPENPR_TEST_DATABASE_URL:-} ]] || { echo 'FAIL: OPENPR_TEST_DATABASE_URL is required' >&2; exit 2; }
[[ -n $EVIDENCE_ROOT ]] || EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.8"
mkdir -p "$EVIDENCE_ROOT/logs"
ROWS=$(mktemp "$EVIDENCE_ROOT/.package-fault-rows.XXXXXX")
trap 'rm -f "$ROWS"' EXIT

run_mutations() {
  local id=$1 script=$2 log="$EVIDENCE_ROOT/logs/package-fault-$1.log"
  set +e
  env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" CARGO_BUILD_JOBS=4 \
    "$REPO_ROOT/scripts/$script" >"$log" 2>&1
  local status=$?
  set -e
  printf '%s\t%s\t%s\n' "$id" "$status" "${log#"$EVIDENCE_ROOT/"}" >>"$ROWS"
}

run_mutations codec verify-flow-package-mutations-v0.8.sh
run_mutations promotion verify-flow-package-import-mutations-v0.8.sh
run_mutations wire_limits verify-flow-import-limit-mutations-v0.8.sh

python3 - "$REPO_ROOT" "$EVIDENCE_ROOT" "$ROWS" <<'PY'
import datetime as dt, hashlib, json, os, pathlib, re, subprocess, sys, tempfile
repo,evidence,rows=map(lambda value:pathlib.Path(value).resolve(),sys.argv[1:])
expected={"codec":(1,10),"promotion":(1,6),"wire_limits":(1,4)}
checks=[]; cases=[]
for raw in rows.read_text().splitlines():
    cid,code,relative=raw.split("\t"); path=evidence/relative; text=path.read_text(errors="replace")
    parsed=re.findall(r"^([a-z0-9_]+) status=(\d+) expected=(green|red) log=(\S+)$",text,re.M)
    green=sum(expect=="green" for _,_,expect,_ in parsed); red=sum(expect=="red" for _,_,expect,_ in parsed)
    nested_executed=0; detected=True
    for label,status,expect,nested_raw in parsed:
        nested=pathlib.Path(nested_raw); nested_text=nested.read_text(errors="replace") if nested.is_file() else ""
        summaries=re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;",nested_text,re.M)
        executed=sum(int(p)+int(f) for _,p,f in summaries); nested_executed+=executed
        status=int(status); case_ok=executed>0 and ((expect=="green" and status==0) or (expect=="red" and status!=0))
        detected &= case_ok
        cases.append({"suite":cid,"id":label,"expected":expect,"exit_code":status,"executed_count":executed,"detected":case_ok,"log":str(nested)})
    ok=int(code)==0 and (green,red)==expected[cid] and detected
    checks.append({"id":cid,"status":"passed" if ok else "failed","exit_code":int(code),
      "green_controls":green,"red_mutations":red,"executed_count":nested_executed,
      "log":relative,"sha256":hashlib.sha256(path.read_bytes()).hexdigest()})
dirty=subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1","--","apps","crates","migrations","scripts","testing","Cargo.toml","Cargo.lock"],text=True).splitlines()
passed=all(check["status"]=="passed" for check in checks) and not dirty
result={"schema_version":"sylvode.flow.package-import-fault-result.v1","release":"0.8.0",
 "source_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),
 "all_promotion_points":["job_started","object_promoted","relations_promoted","event_inserted","report_finished","preview_linked"],
 "checks":checks,"mutation_cases":cases,"executed_count":sum(check["executed_count"] for check in checks),
 "source_dirty":bool(dirty),"dirty_entries":dirty,"passed":passed,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".package-fault.",dir=evidence)
with os.fdopen(fd,"w") as handle: json.dump(result,handle,sort_keys=True,indent=2); handle.write("\n")
os.replace(tmp,evidence/"package-import-fault-result.json")
print(json.dumps(result,sort_keys=True))
raise SystemExit(0 if passed else 1)
PY
