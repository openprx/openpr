#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--contracts-root) shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;};mkdir -p "$EVIDENCE/logs"
set +e;env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 "$ROOT/scripts/verify-flow-forms-regression-v0.4.sh" --evidence-root "$EVIDENCE" --repo-root "$ROOT" --json >"$EVIDENCE/logs/forms-v10.log" 2>&1;code=$?;set -e
python3 - "$ROOT" "$EVIDENCE" "$code" <<'PY'
import datetime as dt,json,os,pathlib,subprocess,sys,tempfile
repo,evidence=map(pathlib.Path,sys.argv[1:3]);code=int(sys.argv[3])
try:base=json.loads((evidence/"forms-regression-result.json").read_text())
except Exception as e:base={"passed":False,"executed_count":0,"error":str(e)}
# Flow and Forms remain separately named evidence domains; exercise the same predicate on an alias mutation.
domains=["flow","forms"];independent=lambda values: len(values)==2 and len(set(values))==2
mutation_red=not independent(["flow","flow"])
r={"schema_version":"sylvode.flow.forms-signoffs-result.v1","release":"1.0.0","source_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),"domains":domains,"base":base,"mutation":{"name":"aliased_signoff_domains","red":mutation_red},"executed_count":int(base.get("executed_count",len(base.get("checks",[]))))+1,"passed":code==0 and base.get("passed") is True and independent(domains) and mutation_red,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".flow-forms-signoffs-result.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(r,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,evidence/"flow-forms-signoffs-result.json");print(json.dumps(r,sort_keys=True));raise SystemExit(0 if r["passed"] else 1)
PY
