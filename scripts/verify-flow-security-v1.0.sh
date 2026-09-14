#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;};mkdir -p "$EVIDENCE/logs";ROWS=$(mktemp "$EVIDENCE/.security.XXXXXX");trap 'rm -f "$ROWS"' EXIT
run(){ local id=$1;shift;local log="$EVIDENCE/logs/security-v10-$id.log";set +e;env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-postgresql://flowtest:flowtest@127.0.0.1:25433/postgres}" CARGO_BUILD_JOBS=4 "$@" >"$log" 2>&1;local c=$?;set -e;printf '%s\t%s\t%s\n' "$id" "$c" "$log">>"$ROWS";}
run audit cargo audit --json
run deny cargo deny --format json check advisories bans licenses sources
run export_authorization cargo test --manifest-path "$ROOT/Cargo.toml" -p api workspace_export_requires_admin_and_object_export_rechecks_effective_permission -- --nocapture
run revoked_reads cargo test --manifest-path "$ROOT/Cargo.toml" -p api object_reads_reauthorize_after_their_final_epoch_check_changes -- --nocapture
run authorized_search cargo test --manifest-path "$ROOT/Cargo.toml" -p api flow_search_filters_before_cardinality_cursor_snippet_and_frontier -- --nocapture
run field_secrecy cargo test --manifest-path "$ROOT/Cargo.toml" -p api field_secrecy_client_crdt_denied_and_server_query_redacts_restricted_fields -- --nocapture
run event_redaction cargo test --manifest-path "$ROOT/Cargo.toml" -p api flow::event_policy::tests:: -- --nocapture
run trace_redaction cargo test --manifest-path "$ROOT/Cargo.toml" -p api every_request_trace_uri_omits_query_strings -- --nocapture
python3 - "$ROOT" "$EVIDENCE" "$ROWS" <<'PY'
import copy,datetime as dt,hashlib,json,os,pathlib,re,subprocess,sys,tempfile
repo,evidence,rows=map(pathlib.Path,sys.argv[1:]);checks=[];audit={};deny_errors=0
for raw in rows.read_text().splitlines():
 cid,code,path=raw.split("\t");code=int(code);p=pathlib.Path(path);body=p.read_text(errors="replace");summaries=re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;",body,re.M);executed=1 if cid in ("audit","deny") else sum(int(a)+int(b) for _,a,b,_ in summaries);ok=code==0 and executed>0 and (not summaries or all(s=="ok" and int(f)==0 for s,_,f,_ in summaries));checks.append({"id":cid,"status":"passed" if ok else "failed","exit_code":code,"executed_count":executed,"log":str(p),"sha256":hashlib.sha256(p.read_bytes()).hexdigest()})
 if cid=="audit":
  try:audit=json.loads(body)
  except Exception:pass
 if cid=="deny":
  for line in body.splitlines():
   try:item=json.loads(line)
   except Exception:continue
   if item.get("type")=="summary":deny_errors=sum(int(v.get("errors",0)) for v in item.get("fields",{}).values() if isinstance(v,dict))
vulns=int(audit.get("vulnerabilities",{}).get("count",-1));evaluate=lambda rows,v,d: all(x["status"]=="passed" and x["executed_count"]>0 for x in rows) and v==0 and d==0
green=evaluate(checks,vulns,deny_errors);bad=copy.deepcopy(checks);bad[-1]["status"]="failed";mutations={"vulnerability_count_plus_one":{"red":not evaluate(checks,vulns+1,deny_errors)},"authorization_check_forced_failed":{"red":not evaluate(bad,vulns,deny_errors)}}
head=subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip();dirty=subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1"],text=True).splitlines();passed=green and all(v["red"] for v in mutations.values()) and not dirty
r={"schema_version":"sylvode.flow.security-result.v1","release":"1.0.0","source_head":head,"checks":checks,"vulnerability_count":vulns,"deny_errors":deny_errors,"mutations":mutations,"source_dirty":bool(dirty),"dirty_entries":dirty,"executed_count":sum(x["executed_count"] for x in checks)+len(mutations),"passed":passed,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".security-result.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(r,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,evidence/"security-result.json");print(json.dumps(r,sort_keys=True));raise SystemExit(0 if passed else 1)
PY
