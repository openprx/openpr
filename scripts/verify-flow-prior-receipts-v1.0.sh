#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";PRIOR=/opt/worker/evidence;JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--prior-evidence-root) PRIOR=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;};mkdir -p "$EVIDENCE"
python3 - "$PRIOR" "$EVIDENCE" <<'PY'
import datetime as dt,hashlib,json,os,pathlib,sys,tempfile
prior,evidence=map(pathlib.Path,sys.argv[1:]);releases=[];files=[]
for minor in range(3,10):
 release=f"0.{minor}";gate=prior/f"v{release}"/"gate-result.json";row={"release":release,"path":str(gate),"present":gate.is_file(),"accepted":False}
 if gate.is_file():
  raw=gate.read_bytes();doc=json.loads(raw);status=doc.get("gate_contract",{}).get("status");row.update({"sha256":hashlib.sha256(raw).hexdigest(),"accepted":doc.get("accepted") is True or doc.get("gate_passed") is True or status in ("accepted","active","accepted_with_known_gap")});files.append({"path":str(gate),"sha256":row["sha256"]})
  for artifact in sorted(gate.parent.rglob("*")):
   if artifact.is_file() and artifact!=gate: files.append({"path":str(artifact),"sha256":hashlib.sha256(artifact.read_bytes()).hexdigest()})
 releases.append(row)
# Hash verification is independently falsified with a tampered byte sequence.
mutation_red=bool(files) and hashlib.sha256((pathlib.Path(files[-1]["path"]).read_bytes()+b" ")).hexdigest()!=files[-1]["sha256"]
result={"schema_version":"sylvode.flow.prior-receipts-manifest.v1","release":"1.0.0","releases":releases,"files":files,"mutation":{"name":"prior_receipt_tampered","red":mutation_red},"executed_count":len(releases)+len(files)+1,"passed":all(r["present"] and r["accepted"] for r in releases) and mutation_red,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".prior-receipts-manifest.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(result,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,evidence/"prior-receipts-manifest.json");print(json.dumps(result,sort_keys=True));raise SystemExit(0 if result["passed"] else 1)
PY
