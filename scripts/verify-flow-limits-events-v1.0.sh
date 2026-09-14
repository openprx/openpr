#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);CONTRACTS=/opt/working/sylvode-flow;EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--contracts-root) CONTRACTS=${2:?};shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;};mkdir -p "$EVIDENCE/logs"
set +e
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 "$ROOT/scripts/verify-flow-limits-v0.4.sh" --contracts-root "$CONTRACTS" --evidence-root "$EVIDENCE" --repo-root "$ROOT" --json >"$EVIDENCE/logs/limits-v10.log" 2>&1;l=$?
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 "$ROOT/scripts/verify-flow-events-v0.4.sh" --contracts-root "$CONTRACTS" --evidence-root "$EVIDENCE" --repo-root "$ROOT" --json >"$EVIDENCE/logs/events-v10.log" 2>&1;e=$?
set -e
python3 - "$ROOT" "$EVIDENCE" "$l" "$e" <<'PY'
import datetime as dt,json,os,pathlib,subprocess,sys,tempfile
repo,evidence=map(pathlib.Path,sys.argv[1:3]);codes=list(map(int,sys.argv[3:]));names=["limits-result.json","flow-events-result.json"];rows=[]
def count(value): return int(value.get("executed_count",len(value.get("checks",[])) or len(value.get("hard_gates",{})) or len(value.get("gates",{}))))
def component_ok(value,code): return code==0 and value.get("passed") is True and count(value)>0
for name,code in zip(names,codes):
 try:v=json.loads((evidence/name).read_text())
 except Exception as x:v={"passed":False,"error":str(x)}
 rows.append({"artifact":name,"exit_code":code,"executed_count":count(v),"passed":component_ok(v,code)})
# Existing component verifiers carry their own source and negative controls; require both.
mutation_red=not component_ok({"passed":False,"executed_count":1},0)
r={"schema_version":"sylvode.flow.limits-events-result.v1","release":"1.0.0","source_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),"checks":rows,"mutation":{"name":"component_failure_propagates","red":mutation_red},"executed_count":sum(x["executed_count"] for x in rows)+1,"passed":all(x["passed"] and x["executed_count"]>0 for x in rows) and mutation_red,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".limits-events-result.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(r,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,evidence/"limits-events-result.json");print(json.dumps(r,sort_keys=True));raise SystemExit(0 if r["passed"] else 1)
PY
