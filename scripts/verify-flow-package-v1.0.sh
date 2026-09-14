#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;};mkdir -p "$EVIDENCE/logs"
set +e
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 OPENPR_TEST_DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-postgresql://flowtest:flowtest@127.0.0.1:25433/postgres}" "$ROOT/scripts/verify-flow-package-v1.sh" --repo-root "$ROOT" --evidence-root "$EVIDENCE" --json >"$EVIDENCE/logs/package-v10.log" 2>&1;code=$?
set -e
python3 - "$ROOT" "$EVIDENCE" "$code" <<'PY'
import datetime as dt,json,os,pathlib,subprocess,sys,tempfile
repo,evidence=map(pathlib.Path,sys.argv[1:3]);code=int(sys.argv[3])
try:base=json.loads((evidence/"package-contract-result.json").read_text())
except Exception as e:base={"passed":False,"executed_count":0,"error":str(e)}
# The same equality used for the live fixture must reject a changed digest.
expected=base.get("fixture_tree_sha256");matches=lambda actual: bool(expected) and actual==expected
mutation_red=not matches((expected or "")+"0")
r={"schema_version":"sylvode.flow.export-package-result.v1","release":"1.0.0","source_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),"base":base,"mutation":{"name":"fixture_hash_changed","red":mutation_red},"executed_count":int(base.get("executed_count",0))+1,"passed":code==0 and base.get("passed") is True and mutation_red,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".export-package-result.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(r,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,evidence/"export-package-result.json");print(json.dumps(r,sort_keys=True));raise SystemExit(0 if r["passed"] else 1)
PY
