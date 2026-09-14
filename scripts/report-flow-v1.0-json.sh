#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CONTRACTS=/opt/working/sylvode-flow
EVIDENCE="$ROOT/.flow-gate/evidence/v1.0"
GATE="$CONTRACTS/gates/v1.0-gate.yaml"
PREDECESSOR=/opt/worker/evidence/v0.9/gate-result.json
MANUAL_FROM=
while (($#)); do
 case "$1" in
  --repo-root) ROOT=${2:?}; shift 2;; --contracts-root) CONTRACTS=${2:?}; shift 2;;
  --evidence-root) EVIDENCE=${2:?}; shift 2;; --gate-yaml) GATE=${2:?}; shift 2;;
  --predecessor-gate-result|--predecessor-evidence) PREDECESSOR=${2:?}; shift 2;;
  --manual-signoffs-from) MANUAL_FROM=${2:?}; shift 2;; --json) shift;;
  *) echo "FAIL: unsupported argument: $1" >&2; exit 2;;
 esac
done
[[ -n $MANUAL_FROM ]] || MANUAL_FROM="$EVIDENCE/gate-result.json"
mkdir -p "$EVIDENCE/logs"
ROWS=$(mktemp "$EVIDENCE/.report-rows.XXXXXX"); trap 'rm -f "$ROWS"' EXIT
run() { local id=$1 artifact=$2; shift 2; local log="$EVIDENCE/logs/report-$id.log"; set +e
 env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 "$@" >"$log" 2>&1; local code=$?; set -e
 printf '%s\t%s\t%s\t%s\t%s\n' "$id" "$code" "${log#"$EVIDENCE/"}" "$artifact" "$*" >>"$ROWS"; }

# Every declared producer is executed. None uses serial test flags.
run prior_receipts prior-receipts-manifest.json "$ROOT/scripts/verify-flow-prior-receipts-v1.0.sh" --repo-root "$ROOT" --contracts-root "$CONTRACTS" --evidence-root "$EVIDENCE" --json
run stable_contracts stable-contract-manifest.json "$ROOT/scripts/verify-flow-stable-contracts-v1.0.sh" --contracts-root "$CONTRACTS" --evidence-root "$EVIDENCE" --json
run limits_events limits-events-result.json "$ROOT/scripts/verify-flow-limits-events-v1.0.sh" --contracts-root "$CONTRACTS" --evidence-root "$EVIDENCE" --json
run package export-package-result.json "$ROOT/scripts/verify-flow-package-v1.0.sh" --evidence-root "$EVIDENCE" --json
run release_build release-build-result.json "$ROOT/scripts/verify-flow-release-build-v1.0.sh" --evidence-root "$EVIDENCE" --json
run install install-upgrade-rollback-result.json "$ROOT/scripts/verify-flow-install-upgrade-rollback-v1.0.sh" --evidence-root "$EVIDENCE" --json
run backup backup-restore-result.json "$ROOT/scripts/verify-flow-backup-restore-v1.0.sh" --contracts-root "$CONTRACTS" --evidence-root "$EVIDENCE" --json
run old_client old-client-result.json "$ROOT/scripts/verify-flow-old-client-v1.0.sh" --evidence-root "$EVIDENCE" --json
run slo slo-result.json "$ROOT/scripts/verify-flow-slo-v1.0.sh" --evidence-root "$EVIDENCE" --json
run security security-result.json "$ROOT/scripts/verify-flow-security-v1.0.sh" --evidence-root "$EVIDENCE" --json
run forms flow-forms-signoffs-result.json "$ROOT/scripts/verify-flow-forms-signoffs-v1.0.sh" --contracts-root "$CONTRACTS" --evidence-root "$EVIDENCE" --json
run runbook runbook-result.json "$ROOT/scripts/verify-flow-runbook-v1.0.sh" --evidence-root "$EVIDENCE" --json
run surface surface-coverage-result.json "$ROOT/scripts/verify-flow-surface-coverage.sh" --release 1.0 --contracts-root "$CONTRACTS" --evidence-root "$EVIDENCE" --repo-root "$ROOT" --json
run cardinality cardinality-result.json "$ROOT/scripts/verify-flow-cardinality-v1.0.sh" --adr "$CONTRACTS/decisions/ADR-0013-multi-document-atomicity.md" --full-scan --contracts-root "$CONTRACTS" --evidence-root "$EVIDENCE" --repo-root "$ROOT" --json
run registry tool-registry-result.json "$ROOT/scripts/verify-flow-tool-registry-v0.4.sh" --baseline "$CONTRACTS/contracts/tool-count-baseline.md" --release 1.0 --contracts-root "$CONTRACTS" --evidence-root "$EVIDENCE" --repo-root "$ROOT" --json

python3 - "$ROOT" "$EVIDENCE" "$GATE" "$PREDECESSOR" "$MANUAL_FROM" "$ROWS" <<'PY'
import datetime as dt,hashlib,json,os,pathlib,re,subprocess,sys,tempfile,yaml
repo,evidence,gate_path,pred_path,manual_path,rows_path=map(pathlib.Path,sys.argv[1:])
gate=yaml.safe_load(gate_path.read_text()); head=subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip()
def load(name):
 try:return json.loads((evidence/name).read_text())
 except Exception as e:return {"passed":False,"executed_count":0,"error":str(e)}
checks=[]; artifacts={}
for raw in rows_path.read_text().splitlines():
 cid,code,log,artifact,command=raw.split("\t",4); code=int(code); body=(evidence/log).read_text(errors="replace"); value=load(artifact); artifacts[cid]=value
 summaries=re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;",body,re.M)
 if cid=="surface": executed=int(value.get("counts",{}).get("matrix_rows",0))
 elif cid=="registry": executed=int(value.get("live_registry",{}).get("enumerated_total",0))
 else: executed=int(value.get("executed_count",0))
 ignored=sum(int(x) for *_,x in summaries)
 ok=code==0 and executed>0 and value.get("passed") is True
 checks.append({"id":cid,"artifact":artifact,"status":"passed" if ok else "failed","exit_code":code,"executed_count":executed,"ignored_count":ignored,"command":command,"log":log,"sha256":hashlib.sha256((evidence/log).read_bytes()).hexdigest()})
ok={r["id"]:r["status"]=="passed" for r in checks}; surface=artifacts["surface"]; registry=artifacts["registry"]
hard_bool={
 "command_contended_document_cardinality":ok["cardinality"],"rest_mcp_cli_surface_parity":ok["surface"],
 "mcp_default_rest_coverage_three_adr_threat_exceptions_only":ok["surface"] and surface.get("counts",{}).get("not_exposed",{}).get("mcp")==3,
 "v0_3_through_v0_9_receipts_hash_valid":ok["prior_receipts"],"stable_contract_versions_frozen":ok["stable_contracts"],
 "flow_limits_and_event_contracts_reverified":ok["limits_events"],"export_package_v1_contract_reverified":ok["package"],
 "release_build_reproducible":ok["release_build"],"install_upgrade_rollback_reverified":ok["install"],
 "backup_restore_reverified":ok["backup"],"old_client_compatibility_reverified":ok["old_client"],
 "production_slo_budget_met":ok["slo"],"security_no_unresolved_high":ok["security"],
 "flow_forms_signoffs_independent":ok["forms"],"runbook_and_rollback_ownership_complete":ok["runbook"],
 "tool_registry_expected_139_or_rebased":ok["registry"] and registry.get("live_registry",{}).get("enumerated_total")==140 and registry.get("rebase_valid") is True}
hard={k:("pending" if k=="frontend_track_accepted" else "passed" if hard_bool.get(k,False) else "failed") for k in gate["hard_gates"]}
manual={k:{"status":"pending","signed_by":None,"signed_at":None,"note":None} for k in ("release_owner","rollback_owner","on_call_runbook","stable_contract_approval")}
try:
 old=json.loads(manual_path.read_text()).get("manual_signoffs",{})
 for k in manual:
  if old.get(k,{}).get("status") in ("passed","failed"):manual[k]=old[k]
except Exception:pass
rust=re.search(r'\[workspace\.package\].*?version\s*=\s*"([^"]+)"',(repo/"Cargo.toml").read_text(),re.S).group(1); frontend=json.loads((repo/"frontend/package.json").read_text())["version"]
dirty=subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1"],text=True).splitlines(); baseline=gate["source_baseline"]
baseline_ok=baseline.get("reviewed_head")==head and str(baseline.get("rust_workspace_version"))==rust and str(baseline.get("frontend_package_version"))==frontend
try:
 pred=json.loads(pred_path.read_text()); predecessor={"path":str(pred_path),"release":pred.get("release"),"accepted":pred.get("accepted") is True}
except Exception as e:predecessor={"path":str(pred_path),"accepted":False,"error":str(e)}
required=[r["id"] for r in checks]; violations=[r["id"] for r in checks if r["status"]!="passed" or r["executed_count"]<=0]
automated_ready=all(v=="passed" for k,v in hard.items() if k!="frontend_track_accepted") and not violations and not dirty
candidate=automated_ready and gate.get("status")=="accepted" and baseline_ok and predecessor["accepted"] and hard["frontend_track_accepted"]=="passed"
accepted=candidate and all(v["status"]=="passed" for v in manual.values())
blockers=[]
if gate.get("status")!="accepted":blockers.append("gate_contract_not_accepted")
if not baseline_ok:blockers.append("source_baseline_mismatch")
if not predecessor["accepted"]:blockers.append("predecessor_not_accepted")
if dirty:blockers.append("source_dirty")
blockers += ["artifact_missing_after_execution:"+x for x in violations]+["hard_gate_failed:"+k for k,v in hard.items() if v=="failed"]
if hard["frontend_track_accepted"]!="passed":blockers.append("hard_gate_pending:frontend_track_accepted")
blockers += ["manual_signoff_pending:"+k for k,v in manual.items() if v["status"]!="passed"]
result={"schema_version":"sylvode.flow.gate-result.v1","schema_path":"gates/v1.0-gate.yaml","release":"1.0.0","source_baseline":baseline,
 "source":{"head":head,"rust_workspace_version":rust,"frontend_package_version":frontend,"dirty":bool(dirty),"dirty_entries":dirty},
 "gate_contract":{"status":gate.get("status"),"sha256":hashlib.sha256(gate_path.read_bytes()).hexdigest()},"predecessor":predecessor,
 "checks":checks,"producer_execution":{"required":required,"violations":violations},"hard_gates":hard,"manual_signoffs":manual,
 "automated_gate_count":len(hard)-1,"automated_passed":sum(v=="passed" for k,v in hard.items() if k!="frontend_track_accepted"),
 "automated_failed":sum(v=="failed" for k,v in hard.items() if k!="frontend_track_accepted"),"automated_ready":automated_ready,
 "executed_count":sum(r["executed_count"] for r in checks),"candidate_ready":candidate,"accepted":accepted,"blockers":blockers,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".gate-result.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(result,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,evidence/"gate-result.json");print(json.dumps(result,sort_keys=True));raise SystemExit(0 if automated_ready else 1)
PY
