#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
EVIDENCE_ROOT=
FIXTURES=
JSON_MODE=0
while (($#)); do
  case "$1" in
    --repo-root) REPO_ROOT=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --fixtures) FIXTURES=${2:?}; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $JSON_MODE -eq 1 ]] || { echo 'FAIL: --json is required' >&2; exit 2; }
[[ -n ${OPENPR_TEST_DATABASE_URL:-} ]] || { echo 'FAIL: OPENPR_TEST_DATABASE_URL is required' >&2; exit 2; }
[[ -n $FIXTURES ]] || FIXTURES=testing/fixtures/flow-package-v1
[[ $FIXTURES = /* ]] || FIXTURES="$REPO_ROOT/$FIXTURES"
[[ -n $EVIDENCE_ROOT ]] || EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.8"
[[ -s $FIXTURES/package-fixture.json ]] || { echo "FAIL: locked package fixture missing: $FIXTURES" >&2; exit 2; }
mkdir -p "$EVIDENCE_ROOT/logs"
ROWS=$(mktemp "$EVIDENCE_ROOT/.package-rows.XXXXXX")
trap 'rm -f "$ROWS"' EXIT

run_test() {
  local id=$1 package=$2 filter=$3 mode=$4
  local log="$EVIDENCE_ROOT/logs/package-$id.log"
  local -a command=(cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p "$package")
  [[ $mode == test ]] && command+=(--test "$filter") || command+=(--lib "$filter")
  command+=(-- --nocapture)
  set +e
  env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" CARGO_BUILD_JOBS=4 \
    "${command[@]}" >"$log" 2>&1
  local status=$?
  set -e
  printf '%s\t%s\t%s\n' "$id" "$status" "${log#"$EVIDENCE_ROOT/"}" >>"$ROWS"
}

run_test codec api flow::package::tests:: lib
run_test export api flow::export::tests:: lib
run_test import api flow_package_import_ lib
run_test mcp mcp-server flow_package_roundtrip_e2e test

python3 - "$REPO_ROOT" "$FIXTURES" "$EVIDENCE_ROOT" "$ROWS" <<'PY'
import datetime as dt, hashlib, json, os, pathlib, re, subprocess, sys, tempfile
repo, fixtures, evidence, rows = map(lambda value: pathlib.Path(value).resolve(), sys.argv[1:])
checks=[]
for raw in rows.read_text().splitlines():
    cid, code, relative = raw.split("\t")
    path=evidence/relative; text=path.read_text(errors="replace")
    summaries=re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;",text,re.M)
    executed=sum(int(p)+int(f) for _,p,f,_ in summaries)
    ignored=sum(int(i) for *_,i in summaries)
    ok=int(code)==0 and executed>0 and all(state=="ok" and int(failed)==0 for state,_,failed,_ in summaries)
    checks.append({"id":cid,"status":"passed" if ok else "failed","exit_code":int(code),
      "executed_count":executed,"ignored_count":ignored,"log":relative,
      "sha256":hashlib.sha256(path.read_bytes()).hexdigest()})
fixture_files=sorted(path for path in fixtures.rglob("*") if path.is_file())
fixture_ok=bool(fixture_files) and all(path.stat().st_size>0 for path in fixture_files)
fixture_hash=hashlib.sha256(b"".join(path.relative_to(fixtures).as_posix().encode()+b"\0"+path.read_bytes()+b"\0" for path in fixture_files)).hexdigest()
checks.append({"id":"locked_fixture_nonempty","status":"passed" if fixture_ok else "failed",
 "executed_count":len(fixture_files),"fixture_tree_sha256":fixture_hash})
dirty=subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1","--","apps","crates","migrations","scripts","testing","Cargo.toml","Cargo.lock"],text=True).splitlines()
passed=all(check["status"]=="passed" for check in checks) and not dirty
result={"schema_version":"sylvode.flow.package-contract-result.v1","release":"0.8.0",
 "source_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),
 "fixtures":str(fixtures),"fixture_tree_sha256":fixture_hash,"checks":checks,
 "executed_count":sum(check["executed_count"] for check in checks),"source_dirty":bool(dirty),
 "dirty_entries":dirty,"passed":passed,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".package-contract.",dir=evidence)
with os.fdopen(fd,"w") as handle: json.dump(result,handle,sort_keys=True,indent=2); handle.write("\n")
os.replace(tmp,evidence/"package-contract-result.json")
print(json.dumps(result,sort_keys=True))
raise SystemExit(0 if passed else 1)
PY
