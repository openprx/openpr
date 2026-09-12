#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"; RESULT="${1:-}"; shift || true
REPO_ROOT="$ROOT_DIR"; CONTRACTS_ROOT="/opt/working/sylvode-flow"; EVIDENCE_ROOT=""; GATE_YAML=""
while [[ $# -gt 0 ]]; do case "$1" in --repo-root) REPO_ROOT="$2";shift 2;; --contracts-root) CONTRACTS_ROOT="$2";shift 2;; --evidence-root) EVIDENCE_ROOT="$2";shift 2;; --gate-yaml) GATE_YAML="$2";shift 2;; --json)shift;; *)echo "FAIL: unsupported argument $1" >&2;exit 2;; esac;done
[[ -f "$RESULT" ]] || { echo "FAIL: gate result missing" >&2; exit 2; }; [[ -n "$EVIDENCE_ROOT" ]] || EVIDENCE_ROOT="$(dirname "$RESULT")"; [[ -n "$GATE_YAML" ]] || GATE_YAML="$CONTRACTS_ROOT/gates/v0.7-gate.yaml"
python3 - "$RESULT" "$EVIDENCE_ROOT" "$REPO_ROOT" "$GATE_YAML" <<'PY'
import hashlib,json,pathlib,re,subprocess,sys
result,evidence,repo,gate=map(lambda p:pathlib.Path(p).resolve(),sys.argv[1:]); drift=[]
try:r=json.loads(result.read_text())
except Exception as e: print(json.dumps({"receipt_consistent":False,"malformed":True,"errors":[str(e)]}));raise SystemExit(2)
def same(field,a,b):
 if a!=b: drift.append({"field":field,"observed":a,"expected":b})
same('schema_version',r.get('schema_version'),'sylvode.flow.gate-result.v1');same('release',r.get('release'),'0.7.0')
head=subprocess.check_output(['git','-C',str(repo),'rev-parse','HEAD'],text=True).strip();same('source.head',r.get('source',{}).get('head'),head)
same('gate_contract.sha256',r.get('gate_contract',{}).get('sha256'),hashlib.sha256(gate.read_bytes()).hexdigest())
checks=r.get('checks',[]); by={x.get('id'):x for x in checks if isinstance(x,dict)}
expected={'credential_binding','bridge_permission','reference_embed','conversion_fault_lineage','mcp_registry','mcp_policy','cli_bridge','forms_full','flow_full','cardinality','surface'}
same('checks.keys',set(by),expected)
for cid,item in by.items():
 p=evidence/item.get('log','');
 try: text=p.read_text(); same(f'checks.{cid}.sha256',item.get('sha256'),hashlib.sha256(text.encode()).hexdigest())
 except Exception as e: drift.append({'field':f'checks.{cid}.log','error':str(e)});continue
 passed=item.get('exit_code')==0 and isinstance(item.get('executed_count'),int) and item['executed_count']>0
 same(f'checks.{cid}.status',item.get('status'),'passed' if passed else 'failed')
gate_keys=set(re.findall(r'^  ([a-z0-9_]+): pending$',gate.read_text(),re.M)); same('hard_gates.keys',set(r.get('hard_gates',{})),gate_keys)
failed=[k for k,v in r.get('hard_gates',{}).items() if v!='passed']; same('automated_failed',r.get('automated_failed'),len(failed));same('automated_passed',r.get('automated_passed'),len(gate_keys)-len(failed))
required_artifacts=['bridge-contract-result.json','embed-permission-result.json','conversion-fault-result.json','lineage-result.json','forms-regression-result.json','cardinality-result.json','surface-coverage-result.json']
for name in required_artifacts:
 try:
  a=json.loads((evidence/name).read_text());
  if not isinstance(a,dict): raise ValueError('top level is not object')
 except Exception as e: drift.append({'field':f'artifact.{name}','error':str(e)})
contract_active=r.get('gate_contract',{}).get('status')=='active'; baseline=r.get('source_baseline',{}); src=r.get('source',{})
baseline_match=baseline.get('reviewed_head')==head and baseline.get('rust_workspace_version')==src.get('rust_workspace_version') and baseline.get('frontend_package_version')==src.get('frontend_package_version')
pred=bool(r.get('predecessor',{}).get('accepted')); source_clean=not src.get('dirty'); candidate=not failed and contract_active and baseline_match and pred and source_clean
same('candidate_ready',r.get('candidate_ready'),candidate); accepted=candidate and all(v.get('status')=='passed' for v in r.get('manual_signoffs',{}).values());same('accepted',r.get('accepted'),accepted)
out={"schema_version":"sylvode.flow.verification.v1","release":"0.7.0","receipt_consistent":not drift,"drift":drift,"hard_gates":r.get('hard_gates'),"candidate_ready":candidate,"accepted":accepted,"blockers":r.get('blockers',[])}
print(json.dumps(out,sort_keys=True));raise SystemExit(0 if not drift and candidate else 1)
PY
