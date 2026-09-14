#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);CONTRACTS=/opt/working/sylvode-flow;EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";CAPACITY=${OPENPR_V10_CAPACITY_RESULT:-/opt/worker/evidence/v1.0-w1-final-f01d344/capacity-result.json};BACKUP=;DECISION=;JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--contracts-root) CONTRACTS=${2:?};shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--capacity-result) CAPACITY=${2:?};shift 2;;--backup-restore-result) BACKUP=${2:?};shift 2;;--target-environment-decision) DECISION=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ -n $BACKUP ]] || BACKUP="$EVIDENCE/backup-restore-result.json"
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;};mkdir -p "$EVIDENCE"
python3 - "$ROOT" "$CONTRACTS" "$EVIDENCE" "$CAPACITY" "$BACKUP" "$DECISION" <<'PY'
import copy,datetime as dt,json,os,pathlib,sys,tempfile,yaml
repo,contracts,evidence,capacity_path,backup_path=map(pathlib.Path,sys.argv[1:6]);decision_path=pathlib.Path(sys.argv[6]) if sys.argv[6] else None
try:capacity=json.loads(capacity_path.read_text())
except Exception as e:capacity={"runs":[],"error":str(e)}
try:backup=json.loads(backup_path.read_text())
except Exception as e:backup={"error":str(e)}
try:rto_budget=yaml.safe_load((contracts/"gates/v0.8-gate.yaml").read_text()).get("budgets",{}).get("recovery_time_seconds_max",{})
except Exception as e:rto_budget={"status":"invalid","error":str(e)}
restore=backup.get("rto_measurement") or backup.get("base",{}).get("drill",{}).get("restore",{}) or backup.get("drill",{}).get("restore",{});elapsed=restore.get("elapsed_seconds_ceiling");budget_status=rto_budget.get("status") if isinstance(rto_budget,dict) else None
budget_max=rto_budget.get("value") if isinstance(rto_budget,dict) else None
if budget_status=="unset":
 rto={"status":"not_frozen","measurement_path":str(backup_path),"measurement_seconds_ceiling":elapsed,"measurement_status":restore.get("measurement_status"),"budget_contract":str(contracts/"gates/v0.8-gate.yaml")+"#budgets.recovery_time_seconds_max","budget_status":"unset","budget_max_seconds":None,"passed":False}
else:
 try:rto_passed=restore.get("measurement_status")=="measured" and float(elapsed)<=float(budget_max)
 except (TypeError,ValueError):rto_passed=False
 rto={"status":"passed" if rto_passed else "failed","measurement_path":str(backup_path),"measurement_seconds_ceiling":elapsed,"measurement_status":restore.get("measurement_status"),"budget_contract":str(contracts/"gates/v0.8-gate.yaml")+"#budgets.recovery_time_seconds_max","budget_status":budget_status,"budget_max_seconds":budget_max,"passed":rto_passed}
def evaluate(doc):
 runs={r.get("clients"):r for r in doc.get("runs",[])};checks={"tiers_exact":set(runs)=={10,50},"tier_key_matches_clients":True,"budget_frozen":True,"reconstruction_complete":True,"functional":True,"round_trip":True,"lock":True}
 for tier in (10,50):
  r=runs.get(tier,{});p=r.get("result") or {};key=f"{tier}_client";tier_keys=sorted(k for k in p if __import__("re").fullmatch(r"\d+_client",k));load=p.get(key,{});lock=p.get("lock",{});recon=lock.get("reconstruction",{});hold=lock.get("lock_hold_p95",{});rt=load.get("round_trip_p95",{})
  checks["budget_frozen"] &= p.get("budgets",{}).get("round_trip_p95_ms_max")==250.0 and p.get("budgets",{}).get("lock_hold_p95_ms_max")==25.0
  checks["reconstruction_complete"] &= lock.get("committed_write_transactions")==load.get("accepted_total") and recon.get("unresolved_statements")==0 and bool(recon.get("harvested_log_files"))
  checks["tier_key_matches_clients"] &= tier_keys==[key] and load.get("clients")==tier
  checks["functional"] &= not p.get("rejections") and load.get("accepted_total")==tier*15
  checks["round_trip"] &= float(rt.get("p95_ms",1e99))<=250.0
  checks["lock"] &= float(hold.get("p95_ms",1e99))<=25.0 and float(hold.get("max_ms",1e99))<=100.0
 return checks
checks=evaluate(capacity);known_green=copy.deepcopy(capacity)
for r in known_green.get("runs",[]):
 p=r.get("result") or {};p["violations"]=[];p["passed"]=True;p.get(f'{r.get("clients")}_client',{}).get("round_trip_p95",{})["p95_ms"]=100.0
green_ok=all(evaluate(known_green).values());tier_mut=copy.deepcopy(known_green)
for r in tier_mut.get("runs",[]):r["clients"]=10
key_mut=copy.deepcopy(known_green);key_result=key_mut.get("runs",[{}])[-1].get("result") or {};key_payload=key_result.pop("50_client",{});key_result["10_client"]=key_payload
latency_mut=copy.deepcopy(known_green);latency_result=latency_mut.get("runs",[{}])[-1].get("result") or {};latency_load=latency_result.get("50_client",{}).get("round_trip_p95",{});latency_load["p95_ms"]=251.0
mutations={"same_client_label_for_both_tiers":{"red":not all(evaluate(tier_mut).values())},"same_result_key_for_both_tiers":{"red":not all(evaluate(key_mut).values())},"round_trip_p95_over_budget":{"red":not all(evaluate(latency_mut).values())}}
decision={"approved":False,"status":"pending_target_environment_adjudication"}
if decision_path:
 try:
  d=json.loads(decision_path.read_text());decision={"approved":d.get("approved") is True and bool(d.get("signed_by")) and bool(d.get("environment_id")),"status":"approved" if d.get("approved") is True else "rejected","signed_by":d.get("signed_by"),"environment_id":d.get("environment_id")}
 except Exception as e:decision={"approved":False,"status":"invalid","error":str(e)}
checks["rto_budget_frozen"]=rto["budget_status"]!="unset";checks["rto_budget_met"]=rto["passed"]
performance=all(checks.values());passed=performance and decision["approved"] and green_ok and all(v["red"] for v in mutations.values())
r={"schema_version":"sylvode.flow.slo-result.v2","release":"1.0.0","measurement":str(capacity_path),"measurement_source_head":capacity.get("source_head"),"rto":rto,"checks":checks,"target_environment_decision":decision,"mutations":mutations,"executed_count":len(capacity.get("runs",[]))+len(mutations)+1,"performance_passed":performance,"passed":passed,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".slo-result.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(r,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,evidence/"slo-result.json");print(json.dumps(r,sort_keys=True));raise SystemExit(0 if passed else 1)
PY
