#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);CONTRACTS=/opt/working/sylvode-flow;EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--contracts-root) CONTRACTS=${2:?};shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;};mkdir -p "$EVIDENCE/logs"
set +e
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 "$ROOT/scripts/verify-flow-backup-restore.sh" --repo-root "$ROOT" --contracts-root "$CONTRACTS" --evidence-root "$EVIDENCE" --json >"$EVIDENCE/logs/backup-v10-base.log" 2>&1
base_exit=$?
set -e
python3 - "$ROOT" "$EVIDENCE" "$base_exit" <<'PY'
import copy,datetime as dt,json,os,pathlib,subprocess,sys,tempfile
repo,evidence=map(pathlib.Path,sys.argv[1:3]);base_exit=int(sys.argv[3]);path=evidence/"backup-restore-result.json"
try:base=json.loads(path.read_text())
except Exception as e:base={"functional_passed":False,"executed_count":0,"error":str(e)}
def functional(value):return value.get("functional_passed") is True and value.get("drill",{}).get("verification",{}).get("exact_match") is True and int(value.get("executed_count",0))>0
mutated=copy.deepcopy(base);mutated.setdefault("drill",{}).setdefault("verification",{})["exact_match"]=False;mutation_red=not functional(mutated)
r={"schema_version":"sylvode.flow.backup-restore-result.v2","release":"1.0.0","source_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),"base_exit_code":base_exit,"base":base,"rto_budget_status":"owned_by_production_slo_gate","mutation":{"name":"restored_fingerprint_changed","red":mutation_red},"executed_count":int(base.get("executed_count",0))+1,"passed":functional(base) and mutation_red,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".backup-restore-result.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(r,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,path);print(json.dumps(r,sort_keys=True));raise SystemExit(0 if r["passed"] else 1)
PY
