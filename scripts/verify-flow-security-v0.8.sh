#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
EVIDENCE_ROOT=
JSON_MODE=0
while (($#)); do
  case "$1" in
    --repo-root) REPO_ROOT=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $JSON_MODE -eq 1 ]] || { echo 'FAIL: --json is required' >&2; exit 2; }
[[ -n ${OPENPR_TEST_DATABASE_URL:-} ]] || { echo 'FAIL: OPENPR_TEST_DATABASE_URL is required' >&2; exit 2; }
[[ -n $EVIDENCE_ROOT ]] || EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.8"
mkdir -p "$EVIDENCE_ROOT/logs"
ROWS=$(mktemp "$EVIDENCE_ROOT/.security-rows.XXXXXX")
trap 'rm -f "$ROWS"' EXIT

run() {
  local id=$1
  shift
  local log="$EVIDENCE_ROOT/logs/security-$id.log"
  set +e
  env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" CARGO_BUILD_JOBS=4 \
    "$@" >"$log" 2>&1
  local code=$?
  set -e
  printf '%s\t%s\t%s\n' "$id" "$code" "${log#"$EVIDENCE_ROOT/"}" >>"$ROWS"
}

run audit cargo audit --json
run deny cargo deny --format json check advisories bans licenses sources
run metadata cargo metadata --locked --format-version 1 --no-deps
run frontend_tree /home/ck/.bun/bin/bun pm ls --cwd "$REPO_ROOT/frontend" --all
run export_authorization cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api \
  workspace_export_requires_admin_and_object_export_rechecks_effective_permission -- --nocapture
run revoked_reads cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api \
  object_reads_reauthorize_after_their_final_epoch_check_changes -- --nocapture
run authorized_search cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api \
  flow_search_filters_before_cardinality_cursor_snippet_and_frontier -- --nocapture
run field_secrecy cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api \
  field_secrecy_client_crdt_denied_and_server_query_redacts_restricted_fields -- --nocapture
run event_redaction cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api \
  flow::event_policy::tests:: -- --nocapture
run trace_redaction cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p api \
  every_request_trace_uri_omits_query_strings -- --nocapture

python3 - "$REPO_ROOT" "$EVIDENCE_ROOT" "$ROWS" <<'PY'
import datetime as dt, hashlib, json, os, pathlib, re, subprocess, sys, tempfile
repo, evidence, rows = map(lambda value: pathlib.Path(value).resolve(), sys.argv[1:])
checks=[]
raw={}
for line in rows.read_text().splitlines():
    cid, code, relative = line.split("\t")
    path=evidence/relative; text=path.read_text(errors="replace"); raw[cid]=(int(code),text,path)
    summaries=re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;",text,re.M)
    executed=sum(int(p)+int(f) for _,p,f,_ in summaries)
    if cid in {"audit","deny","metadata","frontend_tree"}: executed=1
    ok=int(code)==0 and executed>0 and all(state=="ok" and int(failed)==0 for state,_,failed,_ in summaries)
    checks.append({"id":cid,"status":"passed" if ok else "failed","exit_code":int(code),
      "executed_count":executed,"ignored_count":sum(int(i) for *_,i in summaries),"log":relative,
      "sha256":hashlib.sha256(path.read_bytes()).hexdigest()})

audit={}
try: audit=json.loads(raw["audit"][1])
except Exception: pass
vulnerability_count=audit.get("vulnerabilities",{}).get("count")
yanked=[{"name":item.get("package",{}).get("name"),"version":item.get("package",{}).get("version")}
        for item in audit.get("warnings",{}).get("yanked",[])]
ignored=sorted(audit.get("settings",{}).get("ignore",[]))

deny_summary={}
for line in raw["deny"][1].splitlines():
    try: item=json.loads(line)
    except Exception: continue
    if item.get("type")=="summary": deny_summary=item.get("fields",{})
deny_errors=sum(int(section.get("errors",0)) for section in deny_summary.values() if isinstance(section,dict))
deny_warnings=sum(int(section.get("warnings",0)) for section in deny_summary.values() if isinstance(section,dict))

cargo=(repo/"Cargo.toml").read_text(); collab=(repo/"crates/collab-core/Cargo.toml").read_text()
frontend=json.loads((repo/"frontend/package.json").read_text())
lock=(repo/"Cargo.lock").read_text()
def locked(crate):
    match=re.search(rf'\[\[package\]\]\nname = "{re.escape(crate)}"\nversion = "([^"]+)"',lock)
    return match.group(1) if match else None
pins={
 "loro":{"manifest":"=1.13.9","resolved":locked("loro"),"exact":bool(re.search(r'loro\s*=\s*"=1\.13\.9"',collab))},
 "wasmtime":{"manifest":"47","resolved":locked("wasmtime"),"locked":bool(re.search(r'wasmtime\s*=\s*"47"',cargo))},
 "loro-crdt":{"manifest":frontend.get("dependencies",{}).get("loro-crdt")},
 "loro-prosemirror":{"manifest":frontend.get("dependencies",{}).get("loro-prosemirror")},
 "prosemirror":{name:version for name,version in frontend.get("dependencies",{}).items() if name.startswith("prosemirror-")},
}
pins_ok=(pins["loro"]["exact"] and pins["loro"]["resolved"]=="1.13.9"
         and pins["wasmtime"]["locked"] and pins["wasmtime"]["resolved"] is not None
         and all(isinstance(frontend["dependencies"].get(name),str)
                 and not frontend["dependencies"][name].startswith(("^","~",">","<","*"))
                 for name in ["loro-crdt","loro-prosemirror","prosemirror-commands","prosemirror-keymap",
                              "prosemirror-model","prosemirror-state","prosemirror-view"]))
checks.append({"id":"dependency_versions_pinned","status":"passed" if pins_ok else "failed",
               "executed_count":len(pins),"pins":pins})

def evidence_result(name):
    try: return json.loads((evidence/name).read_text())
    except Exception as exc: return {"passed":False,"executed_count":0,"error":str(exc)}
fuzz=evidence_result("fuzz-result.json"); upgrade=evidence_result("upgrade-rollback-result.json")
wire_count=sum(int(row.get("executed_count",0)) for row in upgrade.get("wire_corpus_checks",[]))
wire_ok=bool(fuzz.get("passed")) and int(fuzz.get("executed_count",0))>0 and bool(upgrade.get("passed")) and wire_count>0
checks.append({"id":"wire_corpus_replayed_for_upgrade","status":"passed" if wire_ok else "failed",
 "executed_count":int(fuzz.get("executed_count",0))+wire_count,"fuzz_source_head":fuzz.get("source_head"),
 "upgrade_from_head":upgrade.get("from_head"),"upgrade_to_head":upgrade.get("to_head")})

automated_passed=(all(row["status"]=="passed" for row in checks) and vulnerability_count==0 and deny_errors==0)
head=subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip()
dirty=subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1","--",
 "apps","crates","frontend","migrations","scripts","testing","Cargo.toml","Cargo.lock"],text=True).splitlines()
result={"schema_version":"sylvode.flow.security-review.v1","release":"0.8.0","source_head":head,
 "automated_passed":automated_passed and not dirty,"status":"passed" if automated_passed and not dirty else "failed",
 "passed":automated_passed and not dirty,"manual_signoff":{"status":"pending","note":"must be signed by an authorized reviewer"},
 "advisories":{"active_vulnerabilities":vulnerability_count,"ignored_reviewed_ids":ignored,
   "warnings":{"yanked":yanked},"unresolved_high":0 if vulnerability_count==0 else None},
 "dependency_policy":{"deny_errors":deny_errors,"deny_warnings":deny_warnings,"summary":deny_summary},
 "checks":checks,"executed_count":sum(int(row.get("executed_count",0)) for row in checks),
 "source_dirty":bool(dirty),"dirty_entries":dirty,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".security-review.",dir=evidence)
with os.fdopen(fd,"w") as handle: json.dump(result,handle,sort_keys=True,indent=2); handle.write("\n")
os.replace(tmp,evidence/"security-review.json")
print(json.dumps(result,sort_keys=True))
raise SystemExit(0 if result["passed"] else 1)
PY
