#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
EVIDENCE_ROOT=
JSON_MODE=0
while (($#)); do
  case "$1" in
    --repo-root) REPO_ROOT=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $JSON_MODE -eq 1 ]] || { echo 'FAIL: --json is required' >&2; exit 2; }
[[ -n ${OPENPR_TEST_DATABASE_URL:-} ]] || { echo 'FAIL: OPENPR_TEST_DATABASE_URL is required' >&2; exit 2; }
[[ -n $EVIDENCE_ROOT ]] || EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.8"
mkdir -p "$EVIDENCE_ROOT/logs"
ROWS=$(mktemp "$EVIDENCE_ROOT/.fault-suite-rows.XXXXXX")
trap 'rm -f "$ROWS"' EXIT

suites=(
  authz-broadcast
  bootstrap-compaction
  cardinality
  compaction
  fanout
  fuzz
  import-limit
  object-retention
  operations
  projection-rebuild
  search-rebuild
  sequence
  worker-compaction
)
for suite in "${suites[@]}"; do
  script="$REPO_ROOT/scripts/verify-flow-${suite}-mutations-v0.8.sh"
  [[ -x $script ]] || script="$REPO_ROOT/scripts/verify-flow-${suite}-mutation-v0.8.sh"
  [[ -x $script ]] || { echo "FAIL: mutation producer missing: $suite" >&2; exit 2; }
  log="$EVIDENCE_ROOT/logs/fault-$suite.log"
  set +e
  env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" CARGO_BUILD_JOBS=4 \
    "$script" >"$log" 2>&1
  code=$?
  set -e
  printf '%s\t%s\t%s\n' "$suite" "$code" "${log#"$EVIDENCE_ROOT/"}" >>"$ROWS"
done

python3 - "$REPO_ROOT" "$EVIDENCE_ROOT" "$ROWS" <<'PY'
import datetime as dt, hashlib, json, os, pathlib, re, subprocess, sys, tempfile
repo,evidence,rows=map(lambda value:pathlib.Path(value).resolve(),sys.argv[1:])
suites=[]; total=0
for raw in rows.read_text().splitlines():
    suite,code,relative=raw.split("\t"); code=int(code); path=evidence/relative
    text=path.read_text(errors="replace")
    cases=[]
    for label,status,expected,case_path in re.findall(r"^([a-z0-9_]+) status=(\d+) expected=(green|red) log=(\S+)$",text,re.M):
        status=int(status); child=pathlib.Path(case_path)
        body=child.read_text(errors="replace") if child.is_file() else ""
        summaries=re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;",body,re.M)
        executed=sum(int(p)+int(f) for _,p,f in summaries)
        # Some process/CLI controls are not libtest binaries. A non-empty log is still one real
        # execution, while the expected exit direction remains the deciding oracle.
        executed=max(executed,int(bool(body.strip())))
        detected=executed>0 and ((expected=="green" and status==0) or (expected=="red" and status!=0))
        cases.append({"id":label,"expected":expected,"exit_code":status,"executed_count":executed,
                      "detected":detected,"log":str(child),
                      "sha256":hashlib.sha256(body.encode()).hexdigest() if body else None})
    ok=code==0 and bool(cases) and all(row["detected"] for row in cases) and bool(re.search(r"^PASS:",text,re.M))
    total+=len(cases)
    suites.append({"id":suite,"status":"passed" if ok else "failed","exit_code":code,
                   "executed_count":len(cases),"cases":cases,"log":relative,
                   "sha256":hashlib.sha256(path.read_bytes()).hexdigest()})
dirty=subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1","--",
 "apps","crates","frontend","migrations","scripts","testing","Cargo.toml","Cargo.lock"],text=True).splitlines()
passed=all(row["status"]=="passed" for row in suites) and not dirty
result={"schema_version":"sylvode.flow.fault-injection-result.v1","release":"0.8.0",
 "source_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),
 "suites":suites,"executed_count":total,"source_dirty":bool(dirty),"dirty_entries":dirty,
 "passed":passed,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".fault-injection.",dir=evidence)
with os.fdopen(fd,"w") as handle: json.dump(result,handle,sort_keys=True,indent=2); handle.write("\n")
os.replace(tmp,evidence/"fault-injection-result.json")
print(json.dumps(result,sort_keys=True))
raise SystemExit(0 if passed else 1)
PY
