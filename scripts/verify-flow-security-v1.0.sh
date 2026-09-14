#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;};mkdir -p "$EVIDENCE/logs";ROWS=$(mktemp "$EVIDENCE/.security.XXXXXX");trap 'rm -f "$ROWS"' EXIT
run(){ local id=$1;shift;local log="$EVIDENCE/logs/security-v10-$id.log";set +e;env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-postgresql://flowtest:flowtest@127.0.0.1:25433/postgres}" CARGO_BUILD_JOBS=4 "$@" >"$log" 2>&1;local c=$?;set -e;printf '%s\t%s\t%s\n' "$id" "$c" "$log">>"$ROWS";}
AUDIT_JSON="$EVIDENCE/logs/security-v10-audit.json"
AUDIT_STDERR="$EVIDENCE/logs/security-v10-audit.stderr.log"
set +e
cargo audit --json >"$AUDIT_JSON" 2>"$AUDIT_STDERR"
AUDIT_EXIT=$?
set -e
printf '%s\t%s\t%s\n' audit "$AUDIT_EXIT" "$AUDIT_JSON">>"$ROWS"
run deny cargo deny --format json check advisories bans licenses sources
run export_authorization cargo test --manifest-path "$ROOT/Cargo.toml" -p api workspace_export_requires_admin_and_object_export_rechecks_effective_permission -- --nocapture
run revoked_reads cargo test --manifest-path "$ROOT/Cargo.toml" -p api object_reads_reauthorize_after_their_final_epoch_check_changes -- --nocapture
run authorized_search cargo test --manifest-path "$ROOT/Cargo.toml" -p api flow_search_filters_before_cardinality_cursor_snippet_and_frontier -- --nocapture
run field_secrecy cargo test --manifest-path "$ROOT/Cargo.toml" -p api field_secrecy_client_crdt_denied_and_server_query_redacts_restricted_fields -- --nocapture
run event_redaction cargo test --manifest-path "$ROOT/Cargo.toml" -p api flow::event_policy::tests:: -- --nocapture
run trace_redaction cargo test --manifest-path "$ROOT/Cargo.toml" -p api every_request_trace_uri_omits_query_strings -- --nocapture
AUDIT_PARSE_RESULT="$EVIDENCE/logs/security-v10-audit-parse.json"
AUDIT_INJECTED_JSON="$EVIDENCE/logs/security-v10-audit-injected.json"
AUDIT_INJECTED_PARSE_RESULT="$EVIDENCE/logs/security-v10-audit-injected-parse.json"
set +e
python3 "$ROOT/scripts/lib/parse_cargo_audit_json.py" "$AUDIT_JSON" >"$AUDIT_PARSE_RESULT"
AUDIT_PARSE_EXIT=$?
jq '.vulnerabilities.found=true | .vulnerabilities.count=1 | .vulnerabilities.list=[{"advisory":{"id":"RUSTSEC-MUTATION"}}]' "$AUDIT_JSON" >"$AUDIT_INJECTED_JSON"
python3 "$ROOT/scripts/lib/parse_cargo_audit_json.py" "$AUDIT_INJECTED_JSON" >"$AUDIT_INJECTED_PARSE_RESULT"
AUDIT_INJECTED_PARSE_EXIT=$?
python3 "$ROOT/scripts/lib/parse_cargo_audit_json.py" /dev/null >"$EVIDENCE/logs/security-v10-audit-unparseable-mutation.json"
AUDIT_UNPARSEABLE_EXIT=$?
set -e
python3 - "$ROOT" "$EVIDENCE" "$ROWS" "$AUDIT_PARSE_RESULT" "$AUDIT_PARSE_EXIT" "$AUDIT_INJECTED_PARSE_RESULT" "$AUDIT_INJECTED_PARSE_EXIT" "$AUDIT_UNPARSEABLE_EXIT" <<'PY'
import copy,datetime as dt,hashlib,json,os,pathlib,re,subprocess,sys,tempfile
repo,evidence,rows,audit_parse_path=map(pathlib.Path,sys.argv[1:5]);audit_parse_exit=int(sys.argv[5]);injected_parse_path=pathlib.Path(sys.argv[6]);injected_parse_exit=int(sys.argv[7]);unparseable_exit=int(sys.argv[8]);checks=[];deny_errors=0
for raw in rows.read_text().splitlines():
 cid,code,path=raw.split("\t");code=int(code);p=pathlib.Path(path);body=p.read_text(errors="replace");summaries=re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;",body,re.M);executed=1 if cid in ("audit","deny") else sum(int(a)+int(b) for _,a,b,_ in summaries);ok=code==0 and executed>0 and (not summaries or all(s=="ok" and int(f)==0 for s,_,f,_ in summaries));checks.append({"id":cid,"status":"passed" if ok else "failed","exit_code":code,"executed_count":executed,"log":str(p),"sha256":hashlib.sha256(p.read_bytes()).hexdigest()})
 if cid=="deny":
  for line in body.splitlines():
   try:item=json.loads(line)
   except Exception:continue
   if item.get("type")=="summary":deny_errors=sum(int(v.get("errors",0)) for v in item.get("fields",{}).values() if isinstance(v,dict))
try:audit_parse=json.loads(audit_parse_path.read_text())
except Exception as error:audit_parse={"status":"unparseable","vulnerability_count":None,"error":str(error)}
parse_ok=audit_parse_exit==0 and audit_parse.get("status")=="parsed" and isinstance(audit_parse.get("vulnerability_count"),int)
checks.append({"id":"audit_json_parse","status":"passed" if parse_ok else "failed","exit_code":audit_parse_exit,"executed_count":1,"log":str(audit_parse_path),"sha256":hashlib.sha256(audit_parse_path.read_bytes()).hexdigest()})
vulns=audit_parse.get("vulnerability_count")
parse_status=audit_parse.get("status","unparseable")
evaluate=lambda rows,v,status,d: all(x["status"]=="passed" and x["executed_count"]>0 for x in rows) and status=="parsed" and isinstance(v,int) and not isinstance(v,bool) and v==0 and d==0
green=evaluate(checks,vulns,parse_status,deny_errors);bad=copy.deepcopy(checks);bad[-2]["status"]="failed"
try:unparseable=json.loads((evidence/"logs/security-v10-audit-unparseable-mutation.json").read_text())
except Exception as error:unparseable={"status":"missing","error":str(error)}
try:injected=json.loads(injected_parse_path.read_text())
except Exception as error:injected={"status":"missing","vulnerability_count":None,"error":str(error)}
controls={"zero_vulnerabilities":{"green":green}}
mutations={
 "injected_vulnerability":{"parse_exit_code":injected_parse_exit,"status":injected.get("status"),"vulnerability_count":injected.get("vulnerability_count"),"red":injected_parse_exit==0 and injected.get("status")=="parsed" and isinstance(injected.get("vulnerability_count"),int) and injected.get("vulnerability_count")>0 and not evaluate(checks,injected.get("vulnerability_count"),"parsed",deny_errors)},
 "unparseable_audit_json":{"exit_code":unparseable_exit,"status":unparseable.get("status"),"error":unparseable.get("error"),"red":unparseable_exit!=0 and unparseable.get("status")=="unparseable" and not evaluate(checks,None,"unparseable",deny_errors)},
 "authorization_check_forced_failed":{"red":not evaluate(bad,vulns,parse_status,deny_errors)},
}
head=subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip();dirty=subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1"],text=True).splitlines();passed=green and all(v["red"] for v in mutations.values()) and not dirty
r={"schema_version":"sylvode.flow.security-result.v2","release":"1.0.0","source_head":head,"checks":checks,"vulnerability_count":vulns,"vulnerability_count_status":parse_status,"vulnerability_count_error":audit_parse.get("error"),"audit_stderr_log":str(evidence/"logs/security-v10-audit.stderr.log"),"deny_errors":deny_errors,"controls":controls,"mutations":mutations,"source_dirty":bool(dirty),"dirty_entries":dirty,"executed_count":sum(x["executed_count"] for x in checks)+len(controls)+len(mutations),"passed":passed,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".security-result.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(r,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,evidence/"security-result.json");print(json.dumps(r,sort_keys=True));raise SystemExit(0 if passed else 1)
PY
