#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";CAPACITY=${OPENPR_V10_CAPACITY_RESULT:-/opt/worker/evidence/v1.0-w1-final-cb8ee53/capacity-result.json};DECISION=;JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--capacity-result) CAPACITY=${2:?};shift 2;;--target-environment-decision) DECISION=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;};mkdir -p "$EVIDENCE"
python3 - "$ROOT" "$EVIDENCE" "$CAPACITY" "$DECISION" <<'PY'
import copy,datetime as dt,json,os,pathlib,sys,tempfile
repo,evidence,capacity_path=map(pathlib.Path,sys.argv[1:4]);decision_path=pathlib.Path(sys.argv[4]) if sys.argv[4] else None
try:capacity=json.loads(capacity_path.read_text())
except Exception as e:capacity={"runs":[],"error":str(e)}
def evaluate(doc):
 runs={r.get("clients"):r for r in doc.get("runs",[])};checks={"tiers_exact":set(runs)=={10,50},"budget_frozen":True,"reconstruction_complete":True,"functional":True,"round_trip":True,"lock":True}
 for tier in (10,50):
  r=runs.get(tier,{});p=r.get("result") or {};load=p.get("10_client",{});lock=p.get("lock",{});recon=lock.get("reconstruction",{});hold=lock.get("lock_hold_p95",{});rt=load.get("round_trip_p95",{})
  checks["budget_frozen"] &= p.get("budgets",{}).get("round_trip_p95_ms_max")==250.0 and p.get("budgets",{}).get("lock_hold_p95_ms_max")==25.0
  checks["reconstruction_complete"] &= lock.get("committed_write_transactions")==load.get("accepted_total") and recon.get("unresolved_statements")==0 and bool(recon.get("harvested_log_files"))
  checks["functional"] &= load.get("clients")==tier and not p.get("rejections") and load.get("accepted_total")==tier*15
  checks["round_trip"] &= float(rt.get("p95_ms",1e99))<=250.0
  checks["lock"] &= float(hold.get("p95_ms",1e99))<=25.0 and float(hold.get("max_ms",1e99))<=100.0
 return checks
checks=evaluate(capacity);known_green=copy.deepcopy(capacity)
for r in known_green.get("runs",[]):
 p=r.get("result") or {};p["violations"]=[];p["passed"]=True;p.get("10_client",{}).get("round_trip_p95",{})["p95_ms"]=100.0
green_ok=all(evaluate(known_green).values());tier_mut=copy.deepcopy(known_green)
for r in tier_mut.get("runs",[]):r["clients"]=10
latency_mut=copy.deepcopy(known_green);latency_mut["runs"][-1]["result"]["10_client"]["round_trip_p95"]["p95_ms"]=251.0
mutations={"same_client_label_for_both_tiers":{"red":not all(evaluate(tier_mut).values())},"round_trip_p95_over_budget":{"red":not all(evaluate(latency_mut).values())}}
decision={"approved":False,"status":"pending_target_environment_adjudication"}
if decision_path:
 try:
  d=json.loads(decision_path.read_text());decision={"approved":d.get("approved") is True and bool(d.get("signed_by")) and bool(d.get("environment_id")),"status":"approved" if d.get("approved") is True else "rejected","signed_by":d.get("signed_by"),"environment_id":d.get("environment_id")}
 except Exception as e:decision={"approved":False,"status":"invalid","error":str(e)}
performance=all(checks.values());passed=performance and decision["approved"] and green_ok and all(v["red"] for v in mutations.values())
r={"schema_version":"sylvode.flow.slo-result.v1","release":"1.0.0","measurement":str(capacity_path),"measurement_source_head":capacity.get("source_head"),"checks":checks,"target_environment_decision":decision,"mutations":mutations,"executed_count":len(capacity.get("runs",[]))+len(mutations),"performance_passed":performance,"passed":passed,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".slo-result.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(r,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,evidence/"slo-result.json");print(json.dumps(r,sort_keys=True));raise SystemExit(0 if passed else 1)
PY
