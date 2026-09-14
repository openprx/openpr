#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";DOCUMENT=;JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--document) DOCUMENT=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ -n $DOCUMENT ]] || DOCUMENT="$ROOT/docs/sylvode-v1-stable-runbook.md"
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;};mkdir -p "$EVIDENCE"
python3 - "$ROOT" "$EVIDENCE" "$DOCUMENT" <<'PY'
import datetime as dt,hashlib,json,os,pathlib,subprocess,sys,tempfile
repo,evidence,path=map(pathlib.Path,sys.argv[1:]);body=path.read_text()
required={"staged_rollout":("Release preparation and staged rollout","Enable the Flow feature flag for one internal workspace"),"stop_conditions":("Release preparation and staged rollout","Stop rollout on any data-integrity alarm"),"hot_document":("Sync degraded or hot document","coordinator throughput/queueing"),"collector_rotation":("Sync degraded or hot document","Capture all PostgreSQL collector files"),"projection_rebuild":("Projection or search lag","Never repair canonical document state from a projection"),"corruption_quarantine":("Corruption quarantine and forced resync","quarantine its snapshot and update tail"),"forced_resync":("Corruption quarantine and forced resync","Force clients to resync only after checksum"),"backup_restore":("Backup restore","restore into a fresh database name"),"permission_incident":("Token or permission incident","bump the authorization epoch"),"rollback_owner":("Rollback procedure","externally signed rollback owner decides rollback"),"forward_only":("Rollback procedure","Forward-only migrations are not deleted"),"external_signoff":("Exit and evidence","external on-call reviewer")}
def sections(text):
 out={};current=None
 for line in text.splitlines():
  if line.startswith("## "):current=line[3:].strip();out[current]=[]
  elif current is not None:out[current].append(line)
 return {key:"\n".join(lines) for key,lines in out.items()}
def evaluate(text):
 scoped=sections(text)
 return {key:section in scoped and needle in scoped[section] for key,(section,needle) in required.items()}
checks=evaluate(body);victim=required["forward_only"][1];mutated=evaluate(body.replace(victim,"",1));mutation_red=not all(mutated.values()) and mutated["forward_only"] is False
r={"schema_version":"sylvode.flow.runbook-result.v2","release":"1.0.0","source_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),"document":str(path),"sha256":hashlib.sha256(path.read_bytes()).hexdigest(),"check_kind":"required_phrase_presence_within_named_section","requirements":{key:{"section":section,"phrase":phrase} for key,(section,phrase) in required.items()},"checks":checks,"mutation":{"name":"critical_forward_only_step_removed_from_rollback_section","red":mutation_red},"executed_count":len(checks)+1,"passed":all(checks.values()) and mutation_red,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".runbook-result.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(r,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,evidence/"runbook-result.json");print(json.dumps(r,sort_keys=True));raise SystemExit(0 if r["passed"] else 1)
PY
