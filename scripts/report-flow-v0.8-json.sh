#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CONTRACTS_ROOT=/opt/working/sylvode-flow
EVIDENCE_ROOT=
GATE_YAML=
PREDECESSOR=
MANUAL_FROM=
while (($#)); do
  case "$1" in
    --repo-root) REPO_ROOT=${2:?}; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --gate-yaml) GATE_YAML=${2:?}; shift 2 ;;
    --predecessor-gate-result|--predecessor-evidence) PREDECESSOR=${2:?}; shift 2 ;;
    --manual-signoffs-from) MANUAL_FROM=${2:?}; shift 2 ;;
    --json) shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
for name in OPENPR_TEST_DATABASE_URL OPENPR_BACKUP_SOURCE_DATABASE_URL OPENPR_BACKUP_RESTORE_ADMIN_URL; do
  [[ -n ${!name:-} ]] || { echo "FAIL: $name is required" >&2; exit 2; }
done
[[ -n $EVIDENCE_ROOT ]] || EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.8"
[[ -n $GATE_YAML ]] || GATE_YAML="$CONTRACTS_ROOT/gates/v0.8-gate.yaml"
WORKSPACE_ROOT=$(cd "$REPO_ROOT/../.." && pwd)
[[ -n $PREDECESSOR ]] || PREDECESSOR="$WORKSPACE_ROOT/evidence/v0.7/gate-result.json"
[[ -n $MANUAL_FROM ]] || MANUAL_FROM="$EVIDENCE_ROOT/gate-result.json"
mkdir -p "$EVIDENCE_ROOT/logs"
ROWS=$(mktemp "$EVIDENCE_ROOT/.report-rows.XXXXXX")
trap 'rm -f "$ROWS"' EXIT

run() {
  local id=$1
  shift
  local log="$EVIDENCE_ROOT/logs/report-$id.log"
  set +e
  env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" \
    OPENPR_BACKUP_SOURCE_DATABASE_URL="$OPENPR_BACKUP_SOURCE_DATABASE_URL" \
    OPENPR_BACKUP_RESTORE_ADMIN_URL="$OPENPR_BACKUP_RESTORE_ADMIN_URL" \
    CARGO_BUILD_JOBS=4 "$@" >"$log" 2>&1
  local code=$?
  set -e
  printf '%s\t%s\t%s\t%s\n' "$id" "$code" "${log#"$EVIDENCE_ROOT/"}" "$*" >>"$ROWS"
}

# YAML-declared data producers. report/verify/gate/manual_signoff are orchestration commands and
# cannot recursively produce themselves; they are validated structurally below.
run surface "$REPO_ROOT/scripts/verify-flow-surface-coverage.sh" --release 0.8 \
  --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json
run cardinality "$REPO_ROOT/scripts/verify-flow-cardinality-v0.8.sh" \
  --adr "$CONTRACTS_ROOT/decisions/ADR-0013-multi-document-atomicity.md" --since-release 0.7 \
  --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json
run delivery "$REPO_ROOT/scripts/verify-flow-delivery-v0.8.sh" \
  --contract "$CONTRACTS_ROOT/contracts/events-v1.md" \
  --adr "$CONTRACTS_ROOT/decisions/ADR-0011-event-delivery-substrate.md" \
  --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json

# Artifact producers frozen by gate-commands.md.
run package_contract "$REPO_ROOT/scripts/verify-flow-package-v1.sh" \
  --fixtures "$REPO_ROOT/testing/fixtures/flow-package-v1" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json
run package_fault "$REPO_ROOT/scripts/verify-flow-package-faults.sh" \
  --all-promotion-points --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json
run backup_restore "$REPO_ROOT/scripts/verify-flow-backup-restore.sh" \
  --contracts-root "$CONTRACTS_ROOT" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json
run upgrade_rollback "$REPO_ROOT/scripts/verify-flow-upgrade-rollback.sh" --from 0.7 --to 0.8 \
  --evidence-root "$EVIDENCE_ROOT" --json
run fuzz "$REPO_ROOT/scripts/fuzz-flow-corpus.sh" --locked-corpus --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json
run security "$REPO_ROOT/scripts/verify-flow-security-v0.8.sh" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json
run fault_injection "$REPO_ROOT/scripts/verify-flow-fault-injection-v0.8.sh" --evidence-root "$EVIDENCE_ROOT" --repo-root "$REPO_ROOT" --json

# Exact supplemental commands, plus one and only one unfiltered workspace test/clippy audit.
run worker_compaction cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p worker flow_compaction_ -- --nocapture
run bootstrap_compaction cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api flow_bootstrap_compaction_consistency_ -- --nocapture
run operations cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api flow_operations_ -- --nocapture
run package_import cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api flow_package_import_ -- --nocapture
run mcp_operations cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p mcp-server --test flow_admin_operations_e2e
run mcp_package cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p mcp-server --test flow_package_roundtrip_e2e
run frontend_package env PATH="/home/ck/.bun/bin:$PATH" bun run --cwd "$REPO_ROOT/frontend" test:flow-package-import
run frontend_check env PATH="/home/ck/.bun/bin:$PATH" bun run --cwd "$REPO_ROOT/frontend" check
run frontend_build env PATH="/home/ck/.bun/bin:$PATH" bun run --cwd "$REPO_ROOT/frontend" build
run clippy_full cargo clippy --manifest-path "$REPO_ROOT/Cargo.toml" --workspace --all-targets -- -D warnings
run workspace_full cargo test --manifest-path "$REPO_ROOT/Cargo.toml" --workspace --no-fail-fast

python3 - "$REPO_ROOT" "$CONTRACTS_ROOT" "$EVIDENCE_ROOT" "$GATE_YAML" "$PREDECESSOR" "$MANUAL_FROM" "$ROWS" <<'PY'
import datetime as dt, hashlib, json, os, pathlib, re, subprocess, sys, tempfile, yaml
repo,contracts,evidence,gate_path,predecessor_path,manual_path,rows=map(lambda p:pathlib.Path(p).resolve(),sys.argv[1:])
sys.path.insert(0,str(repo/"scripts"/"lib"))
from flow_contract_status import is_accepted_contract_status
gate=yaml.safe_load(gate_path.read_text()); head=subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip()

artifact_by_check={
 "surface":"surface-coverage-result.json","cardinality":"cardinality-result.json","delivery":"delivery-result.json",
 "package_contract":"package-contract-result.json","package_fault":"package-import-fault-result.json",
 "backup_restore":"backup-restore-result.json","upgrade_rollback":"upgrade-rollback-result.json",
 "fuzz":"fuzz-result.json","security":"security-review.json","fault_injection":"fault-injection-result.json"}
checks=[]
for raw in rows.read_text().splitlines():
    cid,code,relative,command=raw.split("\t",3); code=int(code); path=evidence/relative; body=path.read_text(errors="replace")
    summaries=re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;",body,re.M)
    executed=sum(int(p)+int(f) for _,p,f,_ in summaries); ignored=sum(int(i) for *_,i in summaries)
    artifact=None
    if cid in artifact_by_check:
        try:
            artifact=json.loads((evidence/artifact_by_check[cid]).read_text())
            executed=int(artifact.get("executed_count",artifact.get("counts",{}).get("matrix_rows",0)))
        except Exception: artifact=None; executed=0
    elif cid.startswith("frontend_") or cid=="clippy_full": executed=int(code==0)
    ok=code==0 and executed>0
    checks.append({"id":cid,"status":"passed" if ok else "failed","exit_code":code,"executed_count":executed,
      "ignored_count":ignored,"command":command,"log":relative,"sha256":hashlib.sha256(path.read_bytes()).hexdigest()})
by={row["id"]:row for row in checks}
def artifact(name):
    try: return json.loads((evidence/name).read_text())
    except Exception as exc: return {"passed":False,"executed_count":0,"error":str(exc)}
surface=artifact("surface-coverage-result.json"); cardinality=artifact("cardinality-result.json")
delivery=artifact("delivery-result.json"); package=artifact("package-contract-result.json")
package_fault=artifact("package-import-fault-result.json"); backup=artifact("backup-restore-result.json")
upgrade=artifact("upgrade-rollback-result.json"); fuzz=artifact("fuzz-result.json")
security=artifact("security-review.json"); faults=artifact("fault-injection-result.json")
capacity=artifact("capacity-result.json")

budgets={name:{"status":value.get("status"),"set_by":value.get("set_by"),"rule":value.get("rule")}
         for name,value in gate.get("budgets",{}).items()}
budget_result={"schema_version":"sylvode.flow.approved-budgets.v1","release":"0.8.0","budgets":budgets,
 "executed_count":len(budgets),"passed":bool(budgets) and all(row["status"] not in {"unset",None,0,"0"} for row in budgets.values()),
 "source_contract_sha256":hashlib.sha256(gate_path.read_bytes()).hexdigest(),"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
(evidence/"approved-budgets.json").write_text(json.dumps(budget_result,sort_keys=True,indent=2)+"\n")

suite_status={row.get("id"):row.get("status")=="passed" for row in faults.get("suites",[])}
supp=lambda *ids: all(by.get(cid,{}).get("status")=="passed" for cid in ids)
surface_ok=surface.get("passed") is True
package_ok=package.get("passed") is True; package_fault_ok=package_fault.get("passed") is True
delivery_functional=delivery.get("functional_passed") is True
hard={
 "rest_mcp_cli_surface_parity":surface_ok,
 "mcp_default_rest_coverage_three_adr_threat_exceptions_only":surface_ok,
 "approved_numeric_budgets_locked":budget_result["passed"],
 "object_retention_permanent_cleanup_archive_tier":suite_status.get("object-retention",False),
 "compaction_and_forced_resync":supp("worker_compaction","bootstrap_compaction") and suite_status.get("compaction",False),
 "bootstrap_compaction_repeatable_read_no_gap":supp("bootstrap_compaction") and suite_status.get("bootstrap-compaction",False),
 "multi_instance_egress_seq_monotonic":suite_status.get("sequence",False) and suite_status.get("fanout",False),
 "projection_and_search_rebuild_checksums":suite_status.get("projection-rebuild",False) and suite_status.get("search-rebuild",False),
 "backup_restore_rpo_rto":backup.get("passed") is True,
 "upgrade_and_rollback_wire_fixture":upgrade.get("passed") is True,
 "resource_limits_and_slow_consumer":capacity.get("functional_passed") is True and budget_result["passed"],
 "import_limit_kind_coverage":package_fault_ok and suite_status.get("import-limit",False),
 "deferred_retention_budgets_frozen":delivery.get("passed") is True,
 "operations_dry_run_and_execute_auth":supp("operations","mcp_operations") and suite_status.get("operations",False),
 "mcp_compact_rebuild_exact_scope_head_confirm_policy":supp("mcp_operations") and suite_status.get("operations",False),
 "fuzz_locked_corpus_no_regression":fuzz.get("passed") is True and suite_status.get("fuzz",False),
 "security_review_no_unresolved_high":security.get("automated_passed") is True and security.get("advisories",{}).get("unresolved_high")==0,
 "export_package_manifest_checksum_compatibility":package_ok,
 "import_preview_zero_canonical_writes":package_ok and package_fault_ok,
 "import_atomic_promotion_and_failure_rollback":package_fault_ok,
 "command_contended_document_cardinality":cardinality.get("passed") is True and suite_status.get("cardinality",False),
 "import_id_remap_policy_schema_lineage":package_ok and package_fault_ok,
 "import_mcp_cli_equivalence":surface_ok and supp("mcp_package"),
 "import_artifact_mcp_registration_preview_commit_status_chain":surface_ok and supp("package_import","mcp_package","frontend_package"),
 "tool_registry_expected_139_or_rebased":surface_ok,
 "delivery_at_least_once_dedupe_retry_and_retention":delivery_functional,
 "replay_across_retention_boundary_no_duplicate":delivery_functional,
}
hard={key:"passed" if hard.get(key,False) else "failed" for key in gate.get("hard_gates",{})}

# The first authoritative 10/50-client run is deliberately preserved after it falsified the
# 50-client target. Re-running until green would destroy evidence; report records it as consumed.
checks.append({"id":"capacity_preserved_first_run","status":"failed","exit_code":int(capacity.get("runs",[{},{}])[-1].get("exit_code",1)),
 "executed_count":int(capacity.get("executed_count",0)),"command":"scripts/benchmark-flow-capacity.sh --clients 10,50 --json",
 "artifact":"capacity-result.json","reason":"official run retained; functional failures and unset approved budgets are preserved in the artifact"})

bootstrap_result={"schema_version":"sylvode.flow.bootstrap-compaction-consistency-result.v1","release":"0.8.0","source_head":head,
 "checks":[by.get("worker_compaction",{}),by.get("bootstrap_compaction",{})],
 "mutation_suites":{k:suite_status.get(k,False) for k in ["compaction","bootstrap-compaction","sequence","fanout","worker-compaction"]},
 "executed_count":sum(by.get(k,{}).get("executed_count",0) for k in ["worker_compaction","bootstrap_compaction"])+5,
 "passed":hard["bootstrap_compaction_repeatable_read_no_gap"]=="passed" and hard["multi_instance_egress_seq_monotonic"]=="passed"}
(evidence/"bootstrap-compaction-consistency-result.json").write_text(json.dumps(bootstrap_result,sort_keys=True,indent=2)+"\n")

try:
    pred=json.loads(predecessor_path.read_text()); predecessor={"path":str(predecessor_path),"accepted":pred.get("accepted") is True,"release":pred.get("release")}
except Exception as exc: predecessor={"path":str(predecessor_path),"accepted":False,"error":str(exc)}
manual={key:{"status":"pending","signed_by":None,"signed_at":None,"note":None}
        for key in ["backup_restore","forced_resync","operations_dry_run","security_review"]}
try:
    old=json.loads(manual_path.read_text()).get("manual_signoffs",{})
    manual.update({key:value for key,value in old.items() if key in manual and value.get("status") in {"passed","failed"}})
except Exception: pass
rust=re.search(r'\[workspace\.package\].*?version\s*=\s*"([^"]+)"',(repo/"Cargo.toml").read_text(),re.S).group(1)
frontend=json.loads((repo/"frontend/package.json").read_text())["version"]
dirty=subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1","--",
 "apps","crates","frontend","migrations","scripts","testing","Cargo.toml","Cargo.lock"],text=True).splitlines()
baseline=gate.get("source_baseline",{}); contract_status=gate.get("status")
blockers=[]
if not is_accepted_contract_status(contract_status): blockers.append("gate_contract_not_active")
if baseline.get("reviewed_head")!=head or str(baseline.get("rust_workspace_version"))!=rust or str(baseline.get("frontend_package_version"))!=frontend: blockers.append("source_baseline_mismatch")
if not predecessor.get("accepted"): blockers.append("predecessor_not_accepted")
if dirty: blockers.append("source_dirty")
blockers += [f"budget_unset:{key}" for key,value in budgets.items() if value.get("status")=="unset"]
blockers += [f"hard_gate_failed:{key}" for key,value in hard.items() if value!="passed"]
required_producers=["surface","cardinality","delivery"]
for cid in required_producers:
    if by.get(cid,{}).get("executed_count",0)<=0: blockers.append(f"artifact_missing_after_execution:{cid}")
candidate=not blockers
accepted=candidate and all(value.get("status")=="passed" for value in manual.values())
result={"schema_version":"sylvode.flow.gate-result.v1","schema_path":"gates/v0.8-gate.yaml","release":"0.8.0",
 "source_baseline":baseline,"source":{"head":head,"rust_workspace_version":rust,"frontend_package_version":frontend,"dirty":bool(dirty),"dirty_entries":dirty},
 "gate_contract":{"status":contract_status,"sha256":hashlib.sha256(gate_path.read_bytes()).hexdigest()},"predecessor":predecessor,
 "checks":checks,"hard_gates":hard,"manual_signoffs":manual,"automated_gate_count":len(hard),
 "automated_passed":sum(value=="passed" for value in hard.values()),"automated_failed":sum(value!="passed" for value in hard.values()),
 "executed_count":sum(int(row.get("executed_count",0)) for row in checks),
 "candidate_ready":candidate,"accepted":accepted,"blockers":blockers,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".gate-result.",dir=evidence)
with os.fdopen(fd,"w") as handle: json.dump(result,handle,sort_keys=True,indent=2); handle.write("\n")
os.replace(tmp,evidence/"gate-result.json")
print(json.dumps(result,sort_keys=True))
raise SystemExit(0 if candidate else 1)
PY
