#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;};mkdir -p "$EVIDENCE/logs"
set +e
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 "$ROOT/scripts/verify-sylvode-install-upgrade.sh" --evidence-root "$EVIDENCE" --json >"$EVIDENCE/logs/install-upgrade-v10-base.log" 2>&1
base_exit=$?
set -e
python3 - "$ROOT" "$EVIDENCE" "$base_exit" <<'PY'
import copy,datetime as dt,json,os,pathlib,subprocess,sys,tempfile
repo,evidence=map(pathlib.Path,sys.argv[1:3]);code=int(sys.argv[3])
def load(name):
 try:return json.loads((evidence/name).read_text())
 except Exception as e:return {"passed":False,"executed_count":0,"error":str(e)}
install=load("install-upgrade-result.json");old=load("old-client-result.json")
def valid(i,o):return i.get("passed") is True and o.get("rollback",{}).get("data_snapshot_unchanged") is True and int(i.get("executed_count",0))>0
mutated=copy.deepcopy(old);mutated.setdefault("rollback",{})["data_snapshot_unchanged"]=False;mutation_red=not valid(install,mutated)
r={"schema_version":"sylvode.flow.install-upgrade-rollback-result.v1","release":"1.0.0","source_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),"base_exit_code":code,"install_upgrade":install,"rollback":old.get("rollback",{}),"mutation":{"name":"rollback_snapshot_changed","red":mutation_red},"executed_count":int(install.get("executed_count",0))+5+1,"passed":code==0 and valid(install,old) and mutation_red,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".install-upgrade-rollback-result.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(r,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,evidence/"install-upgrade-rollback-result.json");print(json.dumps(r,sort_keys=True));raise SystemExit(0 if r["passed"] else 1)
PY
