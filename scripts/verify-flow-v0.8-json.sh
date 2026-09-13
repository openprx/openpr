#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
RESULT=
[[ $# -eq 0 || $1 == --* ]] || { RESULT=$1; shift; }
EVIDENCE="$ROOT/.flow-gate/evidence/v0.8"; CONTRACTS=/opt/working/sylvode-flow; REPO=$ROOT
while (($#)); do case "$1" in
  --evidence-root) EVIDENCE=${2:?}; shift 2;; --contracts-root) CONTRACTS=${2:?}; shift 2;;
  --repo-root) REPO=${2:?}; shift 2;; --json) shift;; *) echo "FAIL: unsupported argument: $1" >&2; exit 2;; esac; done
[[ -n $RESULT ]] || RESULT="$EVIDENCE/gate-result.json"
python3 - "$RESULT" "$EVIDENCE" "$REPO" "$CONTRACTS/gates/v0.8-gate.yaml" <<'PY'
import hashlib,json,pathlib,re,subprocess,sys,yaml
result,evidence,repo,gate_path=map(lambda p:pathlib.Path(p).resolve(),sys.argv[1:]); drift=[]
try: receipt=json.loads(result.read_text()); gate=yaml.safe_load(gate_path.read_text())
except Exception as exc: print(json.dumps({"receipt_consistent":False,"errors":[str(exc)]})); raise SystemExit(2)
def same(field,actual,expected):
    if actual!=expected: drift.append({"field":field,"observed":actual,"expected":expected})
head=subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip()
same("schema_version",receipt.get("schema_version"),"sylvode.flow.gate-result.v1"); same("release",receipt.get("release"),"0.8.0")
same("source.head",receipt.get("source",{}).get("head"),head)
same("gate_contract.sha256",receipt.get("gate_contract",{}).get("sha256"),hashlib.sha256(gate_path.read_bytes()).hexdigest())
same("hard_gates.keys",set(receipt.get("hard_gates",{})),set(gate.get("hard_gates",{})))
for row in receipt.get("checks",[]):
    if int(row.get("executed_count",0))<=0: drift.append({"field":f"checks.{row.get('id')}.executed_count","error":"must be nonzero"})
    if "log" in row:
        path=evidence/row["log"]
        try: same(f"checks.{row.get('id')}.sha256",row.get("sha256"),hashlib.sha256(path.read_bytes()).hexdigest())
        except Exception as exc: drift.append({"field":f"checks.{row.get('id')}.log","error":str(exc)})
required=list(gate.get("artifacts",{}).values())
artifacts={}
for relative in required:
    path=repo/relative if relative.startswith(".flow-gate/") else evidence/pathlib.Path(relative).name
    try:
        data=json.loads(path.read_text())
        executed=int(data.get("executed_count",data.get("counts",{}).get("matrix_rows",0)))
        artifacts[path.name]={"passed":data.get("passed"),"executed_count":executed}
        if executed<=0: drift.append({"field":f"artifact.{path.name}.executed_count","error":"must be nonzero"})
    except Exception as exc: drift.append({"field":f"artifact.{path.name}","error":str(exc)})
failed=[key for key,value in receipt.get("hard_gates",{}).items() if value!="passed"]
same("automated_gate_count",receipt.get("automated_gate_count"),len(gate.get("hard_gates",{})))
same("automated_failed",receipt.get("automated_failed"),len(failed))
same("automated_passed",receipt.get("automated_passed"),len(gate.get("hard_gates",{}))-len(failed))
candidate=(not failed and gate.get("status")=="active" and receipt.get("predecessor",{}).get("accepted") is True
           and receipt.get("source",{}).get("dirty") is False
           and all(value.get("status")!="unset" for value in gate.get("budgets",{}).values()))
same("candidate_ready",receipt.get("candidate_ready"),candidate)
accepted=candidate and all(value.get("status")=="passed" for value in receipt.get("manual_signoffs",{}).values())
same("accepted",receipt.get("accepted"),accepted)
out={"schema_version":"sylvode.flow.verification.v2","release":"0.8.0","receipt_consistent":not drift,
 "drift":drift,"artifacts":artifacts,"hard_gates":receipt.get("hard_gates",{}),"candidate_ready":candidate,
 "accepted":accepted,"blockers":receipt.get("blockers",[])}
print(json.dumps(out,sort_keys=True)); raise SystemExit(0 if not drift and candidate else 1)
PY
