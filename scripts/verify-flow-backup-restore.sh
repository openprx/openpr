#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CONTRACTS_ROOT=/opt/working/sylvode-flow
EVIDENCE_ROOT=
JSON_MODE=0
while (($#)); do
  case "$1" in
    --repo-root) REPO_ROOT=${2:?}; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $JSON_MODE -eq 1 ]] || { echo 'FAIL: --json is required' >&2; exit 2; }
for name in OPENPR_TEST_DATABASE_URL OPENPR_BACKUP_SOURCE_DATABASE_URL OPENPR_BACKUP_RESTORE_ADMIN_URL; do
  [[ -n ${!name:-} ]] || { echo "FAIL: $name is required" >&2; exit 2; }
done
[[ -n $EVIDENCE_ROOT ]] || EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.8"
GATE_YAML="$CONTRACTS_ROOT/gates/v0.8-gate.yaml"
[[ -s $GATE_YAML ]] || { echo "FAIL: authoritative v0.8 gate missing: $GATE_YAML" >&2; exit 2; }
mkdir -p "$EVIDENCE_ROOT/logs"
DRILL="$EVIDENCE_ROOT/backup-restore-drill.json"
EXPORT_LOG="$EVIDENCE_ROOT/logs/backup-object-export.log"
MUTATION_LOG="$EVIDENCE_ROOT/logs/backup-restore-mutations.log"

set +e
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 \
  OPENPR_BACKUP_SOURCE_DATABASE_URL="$OPENPR_BACKUP_SOURCE_DATABASE_URL" \
  OPENPR_BACKUP_RESTORE_ADMIN_URL="$OPENPR_BACKUP_RESTORE_ADMIN_URL" \
  OPENPR_BACKUP_RESTORE_DATABASE_NAME=v08_restore_official \
  "$REPO_ROOT/scripts/verify-flow-backup-restore-v0.8.sh" "$DRILL" \
  >"$EVIDENCE_ROOT/logs/backup-restore-drill.log" 2>&1
DRILL_STATUS=$?
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" \
  cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api --lib \
    flow::export::tests::workspace_and_object_exports_reconstruct_exact_head_and_freeze_idempotency \
    -- --exact --nocapture >"$EXPORT_LOG" 2>&1
EXPORT_STATUS=$?
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 \
  OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" \
  OPENPR_BACKUP_SOURCE_DATABASE_URL="$OPENPR_BACKUP_SOURCE_DATABASE_URL" \
  OPENPR_BACKUP_RESTORE_ADMIN_URL="$OPENPR_BACKUP_RESTORE_ADMIN_URL" \
  "$REPO_ROOT/scripts/verify-flow-backup-restore-mutations-v0.8.sh" >"$MUTATION_LOG" 2>&1
MUTATION_STATUS=$?
set -e

python3 - "$REPO_ROOT" "$GATE_YAML" "$EVIDENCE_ROOT" "$DRILL_STATUS" "$EXPORT_STATUS" "$MUTATION_STATUS" <<'PY'
import datetime as dt, hashlib, json, os, pathlib, re, subprocess, sys, tempfile
repo,gate,evidence=map(lambda value:pathlib.Path(value).resolve(),sys.argv[1:4])
drill_status,export_status,mutation_status=map(int,sys.argv[4:7])
drill_path=evidence/"backup-restore-drill.json"; export_log=evidence/"logs/backup-object-export.log"; mutation_log=evidence/"logs/backup-restore-mutations.log"
drill=json.loads(drill_path.read_text()) if drill_path.is_file() and drill_path.stat().st_size else {}
summaries=re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;",export_log.read_text(errors="replace"),re.M)
export_executed=sum(int(p)+int(f) for _,p,f in summaries)
export_ok=export_status==0 and export_executed>0 and all(state=="ok" and int(f)==0 for state,_,f in summaries)
rows=re.findall(r"^([a-z0-9_]+) status=(\d+) expected=(green|red) log=(\S+)$",mutation_log.read_text(errors="replace"),re.M)
expected={"checksum_green_control":("green",0),"corrupted_snapshot_accepted":("red",None),"restore_green_control":("green",0),"retained_updates_omitted_from_backup":("red",None)}
mutation_executed=0; mutation_ok=mutation_status==0 and {row[0] for row in rows}==set(expected)
cases=[]
for label,code,kind,raw_path in rows:
    path=pathlib.Path(raw_path); text=path.read_text(errors="replace") if path.is_file() else ""
    nested=re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;",text,re.M)
    executed=sum(int(p)+int(f) for _,p,f in nested)
    if label.startswith("restore_") or label.startswith("retained_"):
        executed=max(executed, int(bool(text.strip())))
    mutation_executed+=executed; code=int(code)
    detected=executed>0 and ((kind=="green" and code==0) or (kind=="red" and code!=0))
    mutation_ok &= detected
    cases.append({"id":label,"expected":kind,"exit_code":code,"executed_count":executed,"detected":detected,"log":str(path)})
drill_ok=drill_status==0 and drill.get("status")=="passed" and drill.get("verification",{}).get("document_count",0)>0 and drill.get("verification",{}).get("exact_match") is True
gate_text=gate.read_text(); budget_unset=bool(re.search(r"recovery_time_seconds_max:\s*\n\s*status: unset",gate_text))
dirty=subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1","--","apps","crates","migrations","scripts","testing","Cargo.toml","Cargo.lock"],text=True).splitlines()
functional=drill_ok and export_ok and mutation_ok and not dirty
passed=functional and not budget_unset
checks=[
 {"id":"postgres_restore_exact_document_fingerprints","status":"passed" if drill_ok else "failed","exit_code":drill_status,"executed_count":drill.get("verification",{}).get("executed_count",0)},
 {"id":"object_package_export","status":"passed" if export_ok else "failed","exit_code":export_status,"executed_count":export_executed},
 {"id":"backup_restore_mutations","status":"passed" if mutation_ok else "failed","exit_code":mutation_status,"executed_count":mutation_executed},
 {"id":"recovery_time_budget_locked","status":"failed" if budget_unset else "passed","executed_count":1,"reason":"authoritative v0.8 recovery_time_seconds_max is unset" if budget_unset else None},
]
result={"schema_version":"sylvode.flow.backup-restore-result.v1","release":"0.8.0",
 "source_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),
 "drill":drill,"mutation_cases":cases,"checks":checks,"functional_passed":functional,
 "approved_recovery_budget_locked":not budget_unset,"executed_count":sum(c["executed_count"] for c in checks),
 "source_dirty":bool(dirty),"dirty_entries":dirty,"passed":passed,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".backup-restore.",dir=evidence)
with os.fdopen(fd,"w") as handle: json.dump(result,handle,sort_keys=True,indent=2); handle.write("\n")
os.replace(tmp,evidence/"backup-restore-result.json")
print(json.dumps(result,sort_keys=True))
raise SystemExit(0 if passed else 1)
PY
