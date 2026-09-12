#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"; REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"; EVIDENCE_ROOT=""; GATE_YAML=""; PREDECESSOR=""; MANUAL_FROM=""
while [[ $# -gt 0 ]]; do case "$1" in
  --repo-root) REPO_ROOT="${2:?}"; shift 2;; --contracts-root) CONTRACTS_ROOT="${2:?}"; shift 2;;
  --evidence-root) EVIDENCE_ROOT="${2:?}"; shift 2;; --gate-yaml) GATE_YAML="${2:?}"; shift 2;;
  --predecessor-gate-result) PREDECESSOR="${2:?}"; shift 2;; --manual-signoffs-from) MANUAL_FROM="${2:?}"; shift 2;;
  --json) shift;; *) echo "FAIL: unsupported argument $1" >&2; exit 2;; esac; done
[[ -n "${OPENPR_TEST_DATABASE_URL:-}" ]] || { echo "FAIL: OPENPR_TEST_DATABASE_URL is required" >&2; exit 2; }
[[ -n "$EVIDENCE_ROOT" ]] || EVIDENCE_ROOT="$CONTRACTS_ROOT/evidence/v0.7"; [[ -n "$GATE_YAML" ]] || GATE_YAML="$CONTRACTS_ROOT/gates/v0.7-gate.yaml"
[[ -n "$PREDECESSOR" ]] || PREDECESSOR="$(dirname "$EVIDENCE_ROOT")/v0.6/gate-result.json"; [[ -n "$MANUAL_FROM" ]] || MANUAL_FROM="$EVIDENCE_ROOT/gate-result.json"
mkdir -p "$EVIDENCE_ROOT/logs"
python3 - "$REPO_ROOT" "$CONTRACTS_ROOT" "$EVIDENCE_ROOT" "$GATE_YAML" "$PREDECESSOR" "$MANUAL_FROM" <<'PY'
import datetime as dt, hashlib, json, os, pathlib, re, subprocess, sys, time
repo, contracts, evidence, gate_yaml, predecessor_path, manual_path = map(lambda p:pathlib.Path(p).resolve(),sys.argv[1:])
head=subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(); gate=gate_yaml.read_text()
def grab(pattern, default="missing"):
 m=re.search(pattern,gate,re.M); return m.group(1) if m else default
baseline={"repository":grab(r"repository:\s*(\S+)"),"reviewed_head":grab(r"reviewed_head:\s*([0-9a-f]{40})"),"rust_workspace_version":grab(r"rust_workspace_version:\s*(\S+)"),"frontend_package_version":grab(r"frontend_package_version:\s*(\S+)")}
version=grab(r'^version\s*=\s*"([^"]+)"',"",) if False else re.search(r'\[workspace\.package\].*?version\s*=\s*"([^"]+)"',(repo/'Cargo.toml').read_text(),re.S).group(1)
frontend=json.loads((repo/'frontend/package.json').read_text())["version"]
commands={
 "credential_binding":["cargo","test","-p","api","a_bot_cannot_forge_a_transport_different_from_its_credential","--","--nocapture"],
 "bridge_permission":["cargo","test","-p","api","flow::bridge::tests::","--","--nocapture"],
 "bridge_mutations":[str(repo/'scripts/verify-flow-bridge-mutations-v0.7.sh'),"--repo-root",str(repo),"--evidence-root",str(evidence),"--json"],
 "reference_embed":["cargo","test","-p","api","flow_bridge_reference_embed_reauthorizes_forms_policy_and_missing_policy_is_read_only","--","--nocapture"],
 "conversion_fault_lineage":["cargo","test","-p","api","conversion_commit_rechecks","--","--nocapture"],
 "event_policy":["cargo","test","-p","api","flow::event_policy::tests::","--","--nocapture"],
 "mcp_registry":["cargo","test","-p","mcp-server","flow_v07_tools_match_the_repository_registry_baseline","--","--nocapture"],
 "mcp_policy":["cargo","test","-p","mcp-server","tool_policy_scopes_match_the_registered_tool_schemas","--","--nocapture"],
 "cli_bridge":["cargo","test","-p","mcp-server","parses_every_v07_bridge_command_line","--","--nocapture"],
 "bridge_smoke":[str(repo/'scripts/smoke-flow-forms-bridge.sh'),"--evidence-root",str(evidence),"--json"],
 "migration_replay":[str(repo/'scripts/verify-flow-migration-replay-v0.7.sh'),"--repo-root",str(repo),"--evidence-root",str(evidence),"--json"],
 "forms_full":["bash","scripts/ci-universal-forms-gates.sh"],
 "flow_full":["cargo","test","-p","api","--lib","--","--nocapture"],
 "cardinality":[str(repo/'scripts/verify-flow-cardinality-v0.7.sh'),"--adr",str(contracts/'decisions/ADR-0013-multi-document-atomicity.md'),"--since-release","0.6","--contracts-root",str(contracts),"--evidence-root",str(evidence),"--repo-root",str(repo),"--json"],
 "surface":[str(repo/'scripts/verify-flow-surface-coverage.sh'),"--release","0.7","--contracts-root",str(contracts),"--evidence-root",str(evidence),"--repo-root",str(repo),"--json"],
}
def test_count(out): return sum(int(a)+int(b) for a,b in re.findall(r'^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed;',out,re.M))
checks=[]
for cid,cmd in commands.items():
 t=time.monotonic(); p=subprocess.run(cmd,cwd=repo,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT); elapsed=round((time.monotonic()-t)*1000)
 log=evidence/'logs'/f'{cid}.log'; log.write_text(p.stdout)
 count=test_count(p.stdout)
 if cid=='bridge_mutations':
  try: count=json.loads((evidence/'bridge-mutation-result.json').read_text()).get('executed_count',0)
  except Exception: count=0
 if cid=='bridge_smoke':
  try: count=json.loads((evidence/'bridge-smoke-result.json').read_text()).get('executed_count',0)
  except Exception: count=0
 if cid=='migration_replay':
  try: count=json.loads((evidence/'migration-replay-result.json').read_text()).get('executed_count',0)
  except Exception: count=0
 if cid=='forms_full':
  if 'Universal Forms static and Rust regression gates passed.' not in p.stdout or re.search(r'^FAIL: ',p.stdout,re.M): count=0
 if cid=='cardinality':
  try: count=json.loads((evidence/'cardinality-result.json').read_text()).get('new_commands_found',0)
  except Exception: count=0
 if cid=='surface':
  try: count=json.loads((evidence/'surface-coverage-result.json').read_text()).get('counts',{}).get('matrix_rows',0)
  except Exception: count=0
 status='passed' if p.returncode==0 and count>0 else 'failed'
 checks.append({"id":cid,"status":status,"exit_code":p.returncode,"executed_count":count,"duration_ms":elapsed,"command":" ".join(cmd),"log":f"logs/{cid}.log","sha256":hashlib.sha256(p.stdout.encode()).hexdigest()})
by={x['id']:x for x in checks}; ok=lambda *ids: all(by[x]['status']=='passed' for x in ids); verdict=lambda *ids:'passed' if ok(*ids) else 'failed'
gates={
 "command_contended_document_cardinality":verdict('cardinality'),"audit_actor_origin_credential_bound":verdict('credential_binding'),
 "rest_mcp_cli_surface_parity":verdict('surface','mcp_registry','cli_bridge'),"mcp_default_rest_coverage_three_adr_threat_exceptions_only":verdict('surface'),
 "reference_and_unreference_policy":verdict('reference_embed','bridge_permission','bridge_mutations'),"embed_request_time_permission":verdict('reference_embed','bridge_permission','bridge_mutations'),
 "preview_commit_frontier_and_schema_freeze":verdict('conversion_fault_lineage'),"conversion_retry_idempotent":verdict('conversion_fault_lineage'),
 "fault_injection_no_partial_bridge":verdict('conversion_fault_lineage'),"lineage_complete_no_double_write":verdict('conversion_fault_lineage','bridge_smoke'),
 "forms_gate_full_regression":verdict('forms_full','migration_replay'),"mcp_cli_bridge_equivalence":verdict('mcp_registry','mcp_policy','cli_bridge','bridge_smoke'),
 "tool_registry_expected_128_or_rebased":verdict('mcp_registry'),"bridge_event_registry_causation_and_redaction":verdict('reference_embed','conversion_fault_lineage','event_policy')}
artifact_map={"bridge-contract-result.json":['reference_embed'],"embed-permission-result.json":['bridge_permission','bridge_mutations','reference_embed'],"conversion-fault-result.json":['conversion_fault_lineage'],"lineage-result.json":['conversion_fault_lineage'],"forms-regression-result.json":['forms_full','migration_replay']}
for name,ids in artifact_map.items():
 value={"schema_version":"sylvode.flow.check-result.v1","release":"0.7.0","source_head":head,"status":verdict(*ids),"passed":ok(*ids),"checks":[by[x] for x in ids],"executed_count":sum(by[x]['executed_count'] for x in ids)}
 (evidence/name).write_text(json.dumps(value,sort_keys=True,indent=2)+'\n')
try: pred=json.loads(predecessor_path.read_text()); predecessor={"path":str(predecessor_path),"accepted":bool(pred.get('accepted')),"release":pred.get('release')}
except Exception as e: predecessor={"path":str(predecessor_path),"accepted":False,"error":str(e)}
manual={k:{"status":"pending","signed_by":None,"signed_at":None,"note":None} for k in ['reference','embed_permission','lineage_no_double_write']}
try:
 old=json.loads(manual_path.read_text()).get('manual_signoffs',{}); manual.update({k:v for k,v in old.items() if k in manual})
except Exception: pass
dirty=subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1","--","apps","crates","migrations","Cargo.toml","Cargo.lock"],text=True).splitlines()
blockers=[]; contract_status=grab(r'^status:\s*(\S+)')
if contract_status!='active': blockers.append('gate_contract_not_active')
if baseline['reviewed_head']!=head or baseline['rust_workspace_version']!=version or baseline['frontend_package_version']!=frontend: blockers.append('source_baseline_mismatch')
if not predecessor.get('accepted'): blockers.append('predecessor_not_accepted')
if dirty: blockers.append('source_dirty')
failed=[k for k,v in gates.items() if v!='passed']; blockers += [f'hard_gate_failed:{x}' for x in failed]
failed_checks=[item['id'] for item in checks if item['status']!='passed']
blockers += [f'producer_failed:{x}' for x in failed_checks]
candidate=not blockers
accepted=candidate and all(v.get('status')=='passed' for v in manual.values())
receipt={"schema_version":"sylvode.flow.gate-result.v1","schema_path":"gates/v0.7-gate.yaml","release":"0.7.0","source_baseline":baseline,
 "source":{"head":head,"rust_workspace_version":version,"frontend_package_version":frontend,"dirty":bool(dirty),"dirty_entries":dirty},
 "gate_contract":{"status":contract_status,"sha256":hashlib.sha256(gate_yaml.read_bytes()).hexdigest()},"predecessor":predecessor,
 "checks":checks,"hard_gates":gates,"manual_signoffs":manual,"automated_gate_count":len(gates),"automated_passed":len(gates)-len(failed),
 "automated_failed":len(failed),"candidate_ready":candidate,"accepted":accepted,"blockers":blockers,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
(evidence/'gate-result.json').write_text(json.dumps(receipt,sort_keys=True,indent=2)+'\n'); print(json.dumps(receipt,sort_keys=True)); raise SystemExit(0 if candidate else 1)
PY
