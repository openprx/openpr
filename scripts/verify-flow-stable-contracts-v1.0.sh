#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);CONTRACTS=/opt/working/sylvode-flow;EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--contracts-root) CONTRACTS=${2:?};shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;};mkdir -p "$EVIDENCE"
python3 - "$ROOT/contracts/v1.0-stable-contract-sha256.txt" "$CONTRACTS" "$EVIDENCE" <<'PY'
import datetime as dt,hashlib,json,os,pathlib,sys,tempfile
manifest,contracts,evidence=map(pathlib.Path,sys.argv[1:]);rows=[]
for line in manifest.read_text().splitlines():
 expected,name=line.split(None,1);p=contracts/name.strip();actual=hashlib.sha256(p.read_bytes()).hexdigest() if p.is_file() else None;rows.append({"path":name.strip(),"expected_sha256":expected,"actual_sha256":actual,"passed":actual==expected})
# The same comparator must reject a one-byte contract mutation.
sample=(contracts/rows[0]["path"]).read_bytes();mutation_red=hashlib.sha256(sample+b"\nmutation").hexdigest()!=rows[0]["expected_sha256"]
result={"schema_version":"sylvode.flow.stable-contract-manifest.v1","release":"1.0.0","contracts":rows,"mutation":{"name":"contract_byte_changed","red":mutation_red},"executed_count":len(rows)+1,"passed":all(r["passed"] for r in rows) and mutation_red,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".stable-contract-manifest.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(result,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,evidence/"stable-contract-manifest.json");print(json.dumps(result,sort_keys=True));raise SystemExit(0 if result["passed"] else 1)
PY
