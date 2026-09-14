#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);CONTRACTS=/opt/working/sylvode-flow;EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";PRIOR=/opt/worker/evidence;JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--contracts-root) CONTRACTS=${2:?};shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--prior-evidence-root) PRIOR=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;};mkdir -p "$EVIDENCE"
python3 - "$ROOT" "$CONTRACTS" "$PRIOR" "$EVIDENCE" <<'PY'
import datetime as dt,hashlib,json,os,pathlib,sys,tempfile,yaml
repo,contracts,prior,evidence=map(pathlib.Path,sys.argv[1:])
sys.path.insert(0,str(repo/"scripts/lib"))
from flow_historical_acceptance import is_accepted_historical_status

expected_exclusions={"0.3":"artifact_never_produced"}
releases=[];files=[];exclusions=[];receipt_tamper_cases=[]
for minor in range(3,10):
 release=f"0.{minor}";gate=prior/f"v{release}"/"gate-result.json";contract=contracts/"gates"/f"v{release}-gate.yaml"
 try:
  contract_status=yaml.safe_load(contract.read_text()).get("status")
 except Exception:
  contract_status=None
 status_accepted=is_accepted_historical_status(contract_status)
 row={"release":release,"contract":{"path":str(contract),"status":contract_status,"accepted":status_accepted},"receipt":{"path":str(gate),"present":gate.is_file(),"status":"missing"}}
 if gate.is_file():
  raw=gate.read_bytes();digest=hashlib.sha256(raw).hexdigest();row["receipt"].update({"status":"verified","sha256":digest});files.append({"path":str(gate),"sha256":digest,"kind":"receipt"})
  receipt_tamper_cases.append({"release":release,"red":hashlib.sha256(raw+b" ").hexdigest()!=digest})
  for artifact in sorted(gate.parent.rglob("*")):
   if artifact.is_file() and artifact!=gate: files.append({"path":str(artifact),"sha256":hashlib.sha256(artifact.read_bytes()).hexdigest(),"kind":"artifact"})
 elif release in expected_exclusions:
  exclusion={"release":release,"artifact_path":str(gate),"reason_code":expected_exclusions[release],"acceptance_basis":{"kind":"contract_status","path":str(contract),"status":contract_status,"accepted":status_accepted}}
  exclusions.append(exclusion);row["receipt"].update({"status":"excluded","reason_code":expected_exclusions[release]})
 releases.append(row)

def exclusion_coverage(rows):
 return len(rows)==len(expected_exclusions) and {x.get("release"):x.get("reason_code") for x in rows}==expected_exclusions

coverage={"expected_count":len(expected_exclusions),"actual_count":len(exclusions),"expected":expected_exclusions,"actual":{x["release"]:x["reason_code"] for x in exclusions},"passed":exclusion_coverage(exclusions)}
mutations={
 "prior_receipt_tampered":{"red":bool(receipt_tamper_cases) and all(x["red"] for x in receipt_tamper_cases),"cases":receipt_tamper_cases},
 "active_contract_status":{"value":"active","red":not is_accepted_historical_status("active")},
 "unknown_contract_status":{"value":"accepted_typo","red":not is_accepted_historical_status("accepted_typo")},
 "unexpected_exclusion":{"red":not exclusion_coverage(exclusions+[{"release":"0.2","reason_code":"artifact_never_produced"}])},
 "exclusion_reason_changed":{"red":not exclusion_coverage([{**x,"reason_code":"missing"} for x in exclusions])},
}
receipts_accounted=all(r["receipt"]["status"] in ("verified","excluded") for r in releases)
contracts_accepted=all(r["contract"]["accepted"] for r in releases)
passed=contracts_accepted and receipts_accounted and coverage["passed"] and all(x["red"] for x in mutations.values())
result={"schema_version":"sylvode.flow.prior-receipts-manifest.v2","release":"1.0.0","acceptance_authority":"authoritative_contract_status_via_scripts/lib/flow_historical_acceptance.py","releases":releases,"excluded":exclusions,"exclusion_coverage":coverage,"files":files,"mutations":mutations,"executed_count":len(releases)+len(files)+1+len(mutations),"passed":passed,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".prior-receipts-manifest.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(result,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,evidence/"prior-receipts-manifest.json");print(json.dumps(result,sort_keys=True));raise SystemExit(0 if result["passed"] else 1)
PY
