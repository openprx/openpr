#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;}
python3 - "$ROOT" "$EVIDENCE" <<'PY'
import datetime as dt,json,os,pathlib,subprocess,sys,tempfile
repo,evidence=map(pathlib.Path,sys.argv[1:]);p=evidence/"old-client-result.json"
try:old=json.loads(p.read_text())
except Exception as e:old={"passed":False,"executed_count":0,"error":str(e)}
head=subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip();is_current=lambda artifact_head: artifact_head==head;current=is_current(old.get("source_head"))
mutation_red=not is_current(head[:-1]+("0" if head[-1]!="0" else "1"))
r={"schema_version":"sylvode.flow.old-client-revalidation.v1","release":"1.0.0","source_head":head,"source_artifact":old,"artifact_current":current,"mutation":{"name":"stale_source_head","red":mutation_red},"executed_count":int(old.get("executed_count",0))+1,"passed":old.get("passed") is True and current and mutation_red,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".old-client-result.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(r,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,p);print(json.dumps(r,sort_keys=True));raise SystemExit(0 if r["passed"] else 1)
PY
