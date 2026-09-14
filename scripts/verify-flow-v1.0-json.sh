#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd); EVIDENCE="$ROOT/.flow-gate/evidence/v1.0"; CONTRACTS=/opt/working/sylvode-flow; RESULT=
[[ $# -eq 0 || $1 == --* ]] || { RESULT=$1; shift; }
while (($#)); do case "$1" in
 --repo-root) ROOT=${2:?};shift 2;; --evidence-root) EVIDENCE=${2:?};shift 2;; --contracts-root) CONTRACTS=${2:?};shift 2;; --json) shift;;
 --predecessor-gate-result|--predecessor-evidence) shift 2;; *) echo "FAIL: unsupported argument: $1" >&2;exit 2;; esac; done
[[ -n $RESULT ]] || RESULT="$EVIDENCE/gate-result.json"
python3 - "$RESULT" "$EVIDENCE" "$ROOT" "$CONTRACTS/gates/v1.0-gate.yaml" <<'PY'
import hashlib,json,pathlib,re,subprocess,sys,yaml
result,evidence,repo,gate_path=map(pathlib.Path,sys.argv[1:]); drift=[]
def same(k,a,b):
 if a!=b:drift.append({"field":k,"observed":a,"expected":b})
try:receipt=json.loads(result.read_text());gate=yaml.safe_load(gate_path.read_text())
except Exception as e:print(json.dumps({"receipt_consistent":False,"errors":[str(e)]},sort_keys=True));raise SystemExit(2)
head=subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip();rust=re.search(r'\[workspace\.package\].*?version\s*=\s*"([^"]+)"',(repo/"Cargo.toml").read_text(),re.S).group(1);frontend=json.loads((repo/"frontend/package.json").read_text())["version"]
dirty=subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1"],text=True).splitlines()
same("schema_version",receipt.get("schema_version"),"sylvode.flow.gate-result.v1");same("release",receipt.get("release"),"1.0.0");same("source.head",receipt.get("source",{}).get("head"),head);same("source.rust_workspace_version",receipt.get("source",{}).get("rust_workspace_version"),rust);same("source.frontend_package_version",receipt.get("source",{}).get("frontend_package_version"),frontend);same("source.dirty",receipt.get("source",{}).get("dirty"),bool(dirty));same("gate_contract.status",receipt.get("gate_contract",{}).get("status"),gate.get("status"));same("gate_contract.sha256",receipt.get("gate_contract",{}).get("sha256"),hashlib.sha256(gate_path.read_bytes()).hexdigest());same("hard_gates.keys",sorted(receipt.get("hard_gates",{})),sorted(gate.get("hard_gates",{})))
checks={r.get("id"):r for r in receipt.get("checks",[])}; violations=[]
for cid,row in checks.items():
 try:same("checks."+cid+".sha256",row.get("sha256"),hashlib.sha256((evidence/row["log"]).read_bytes()).hexdigest())
 except Exception as e:drift.append({"field":"checks."+cid+".log","error":str(e)})
 if row.get("status")!="passed" or int(row.get("executed_count",0))<=0 or row.get("exit_code")!=0:violations.append(cid)
same("producer_execution.violations",receipt.get("producer_execution",{}).get("violations"),violations)
hard=receipt.get("hard_gates",{});same("frontend_track_accepted",hard.get("frontend_track_accepted"),"pending")
auto=all(v=="passed" for k,v in hard.items() if k!="frontend_track_accepted") and not violations and not dirty
same("automated_ready",receipt.get("automated_ready"),auto);same("automated_gate_count",receipt.get("automated_gate_count"),16);same("automated_passed",receipt.get("automated_passed"),sum(v=="passed" for k,v in hard.items() if k!="frontend_track_accepted"));same("automated_failed",receipt.get("automated_failed"),sum(v=="failed" for k,v in hard.items() if k!="frontend_track_accepted"))
baseline=gate["source_baseline"];baseline_ok=baseline.get("reviewed_head")==head and str(baseline.get("rust_workspace_version"))==rust and str(baseline.get("frontend_package_version"))==frontend
candidate=auto and gate.get("status")=="accepted" and baseline_ok and receipt.get("predecessor",{}).get("accepted") is True and hard.get("frontend_track_accepted")=="passed";accepted=candidate and all(v.get("status")=="passed" for v in receipt.get("manual_signoffs",{}).values())
same("candidate_ready",receipt.get("candidate_ready"),candidate);same("accepted",receipt.get("accepted"),accepted)
out={"schema_version":"sylvode.flow.verification.v2","release":"1.0.0","receipt_consistent":not drift,"drift":drift,"hard_gates":hard,"automated_ready":auto,"candidate_ready":candidate,"accepted":accepted,"producer_violations":violations,"blockers":receipt.get("blockers",[])}
print(json.dumps(out,sort_keys=True));raise SystemExit(0 if not drift and auto else 1)
PY
