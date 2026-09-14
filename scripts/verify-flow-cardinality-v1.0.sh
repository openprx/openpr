#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CONTRACTS=/opt/working/sylvode-flow
EVIDENCE="$ROOT/.flow-gate/evidence/v1.0"
ADR=
FULL_SCAN=0
JSON_MODE=0
while (($#)); do
  case "$1" in
    --repo-root) ROOT=${2:?}; shift 2 ;;
    --contracts-root) CONTRACTS=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE=${2:?}; shift 2 ;;
    --adr) ADR=${2:?}; shift 2 ;;
    --full-scan) FULL_SCAN=1; shift ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $FULL_SCAN -eq 1 && $JSON_MODE -eq 1 ]] || { echo 'FAIL: require --full-scan --json' >&2; exit 2; }
[[ -n $ADR ]] || ADR="$CONTRACTS/decisions/ADR-0013-multi-document-atomicity.md"
[[ -f $ADR ]] || { echo "FAIL: ADR missing: $ADR" >&2; exit 2; }
mkdir -p "$EVIDENCE/logs" /opt/worker/.cache
GREEN="$EVIDENCE/logs/cardinality-v10-green.log"
MISSING="$EVIDENCE/logs/cardinality-v10-missing-red.log"
LOCK="$EVIDENCE/logs/cardinality-v10-lock-order-red.log"
TEST=tools::tests::live_v08_dispatch_has_exact_cardinality_coverage_and_lock_order_support

run_test() {
  local root=$1 log=$2
  env -u RUST_TEST_THREADS -u CARGO_TARGET_DIR CARGO_BUILD_JOBS=4 \
    cargo test --manifest-path "$root/Cargo.toml" -p mcp-server --lib "$TEST" \
      -- --exact --nocapture >"$log" 2>&1
}

run_test "$ROOT" "$GREEN"
MUTATION=$(mktemp -d /opt/worker/.cache/v10-cardinality.XXXXXX)
cleanup() { git -C "$ROOT" worktree remove --force "$MUTATION" >/dev/null 2>&1 || true; rm -rf -- "$MUTATION"; }
trap cleanup EXIT
rm -rf -- "$MUTATION"
git -C "$ROOT" worktree add --detach "$MUTATION" HEAD >/dev/null
TARGET="$MUTATION/apps/mcp-server/src/tools/mod.rs"
python3 - "$TARGET" <<'PY'
import pathlib,sys
p=pathlib.Path(sys.argv[1]); s=p.read_text()
n="        objects::rebuild_flow_projection_tool(),\n    ]\n}\n\nfn flow_v08_tool_definitions"
r="        objects::rebuild_flow_projection_tool(),\n        objects::get_flow_object_tool(),\n    ]\n}\n\nfn flow_v08_tool_definitions"
if s.count(n)!=1: raise SystemExit("missing-declaration mutation anchor drifted")
p.write_text(s.replace(n,r))
PY
if run_test "$MUTATION" "$MISSING"; then echo 'FAIL: undeclared production command stayed green' >&2; exit 1; fi
git -C "$MUTATION" checkout -- apps/mcp-server/src/tools/mod.rs
python3 - "$TARGET" <<'PY'
import pathlib,sys
p=pathlib.Path(sys.argv[1]); s=p.read_text(); n='                "collab.compact" => One,'
r='                "collab.compact" => api::flow::command::ExistingDocumentCardinality::BoundedMany(4),'
if s.count(n)!=1: raise SystemExit("lock-order mutation anchor drifted")
p.write_text(s.replace(n,r))
PY
if run_test "$MUTATION" "$LOCK"; then echo 'FAIL: BoundedMany without lock order stayed green' >&2; exit 1; fi

python3 - "$ROOT" "$ADR" "$EVIDENCE" "$GREEN" "$MISSING" "$LOCK" <<'PY'
import datetime as dt,hashlib,json,os,pathlib,re,subprocess,sys,tempfile
repo,adr,evidence,green,missing,lock=map(pathlib.Path,sys.argv[1:])
def count(p):
 s=p.read_text(errors="replace"); rows=re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;",s,re.M)
 return sum(int(a)+int(b) for _,a,b,_ in rows)
g=count(green)==1 and "test result: ok. 1 passed; 0 failed; 0 ignored;" in green.read_text(errors="replace")
m=count(missing)==1 and "has no cardinality declaration" in missing.read_text(errors="replace")
l=count(lock)==1 and "declares BoundedMany without an ADR-0013 section 2 lock-order mechanism" in lock.read_text(errors="replace")
a="existing_document_cardinality = 0 | 1 | bounded_many" in adr.read_text()
head=subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip()
result={"schema_version":"sylvode.flow.cardinality-result.v6","release":"1.0.0","scope":"full_scan",
 "source_head":head,"checks":[{"id":"all_live_commands_declared","status":"passed" if g else "failed","executed_count":count(green)},
 {"id":"adr_rule","status":"passed" if a else "failed","executed_count":1}],
 "mutations":{"production_command_without_declaration":{"red":m,"executed_count":count(missing)},
 "bounded_many_without_lock_order":{"red":l,"executed_count":count(lock)}},
 "executed_count":count(green)+count(missing)+count(lock)+1,"passed":g and m and l and a,
 "logs_sha256":{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in (green,missing,lock)},
 "generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".cardinality-result.",dir=evidence)
with os.fdopen(fd,"w") as f: json.dump(result,f,sort_keys=True,indent=2); f.write("\n")
os.replace(tmp,evidence/"cardinality-result.json"); print(json.dumps(result,sort_keys=True)); raise SystemExit(0 if result["passed"] else 1)
PY
