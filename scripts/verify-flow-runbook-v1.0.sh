#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;};mkdir -p "$EVIDENCE"
python3 - "$ROOT" "$EVIDENCE" <<'PY'
import datetime as dt,hashlib,json,os,pathlib,subprocess,sys,tempfile
repo,evidence=map(pathlib.Path,sys.argv[1:]);path=repo/"docs/sylvode-v1-stable-runbook.md";body=path.read_text()
required={"staged_rollout":"Enable the Flow feature flag for one internal workspace","stop_conditions":"Stop rollout on any data-integrity alarm","hot_document":"coordinator throughput/queueing","collector_rotation":"Capture all PostgreSQL collector files","projection_rebuild":"Never repair canonical document state from a projection","corruption_quarantine":"quarantine its snapshot and update tail","forced_resync":"Force clients to resync only after checksum","backup_restore":"restore into a fresh database name","permission_incident":"bump the authorization epoch","rollback_owner":"externally signed rollback owner decides rollback","forward_only":"Forward-only migrations are not deleted","external_signoff":"external on-call reviewer"}
evaluate=lambda text:{k:(needle in text) for k,needle in required.items()};checks=evaluate(body);victim=required["forward_only"];mutated=evaluate(body.replace(victim,"",1));mutation_red=not all(mutated.values()) and mutated["forward_only"] is False
r={"schema_version":"sylvode.flow.runbook-result.v1","release":"1.0.0","source_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),"document":str(path),"sha256":hashlib.sha256(path.read_bytes()).hexdigest(),"checks":checks,"mutation":{"name":"critical_forward_only_step_removed","red":mutation_red},"executed_count":len(checks)+1,"passed":all(checks.values()) and mutation_red,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".runbook-result.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(r,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,evidence/"runbook-result.json");print(json.dumps(r,sort_keys=True));raise SystemExit(0 if r["passed"] else 1)
PY
