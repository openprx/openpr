#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;};mkdir -p "$EVIDENCE/logs" /opt/worker/.cache
RUN=$(mktemp -d /opt/worker/.cache/v10-repro.XXXXXX);A="$RUN/a";B="$RUN/b";TA="$RUN/target-a";TB="$RUN/target-b"
cleanup(){ git -C "$ROOT" worktree remove --force "$A" >/dev/null 2>&1||true;git -C "$ROOT" worktree remove --force "$B" >/dev/null 2>&1||true;rm -rf -- "$RUN";};trap cleanup EXIT
git -C "$ROOT" worktree add --detach "$A" HEAD >/dev/null;git -C "$ROOT" worktree add --detach "$B" HEAD >/dev/null
EPOCH=$(git -C "$ROOT" show -s --format=%ct HEAD)
build_all(){ local src=$1 target=$2 log=$3;env -u RUST_TEST_THREADS SOURCE_DATE_EPOCH="$EPOCH" CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$target" RUSTFLAGS="--remap-path-prefix=$src=/workspace --remap-path-prefix=$target=/target" cargo build --manifest-path "$src/Cargo.toml" --locked --release -p api --bin api -p mcp-server --bin mcp-server --bin sylvode -p collab-core --bin collab-isolated-apply-worker >"$log" 2>&1;}
build_api(){ local src=$1 target=$2 log=$3;env -u RUST_TEST_THREADS SOURCE_DATE_EPOCH="$EPOCH" CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$target" RUSTFLAGS="--remap-path-prefix=$src=/workspace --remap-path-prefix=$target=/target" cargo build --manifest-path "$src/Cargo.toml" --locked --release -p api --bin api >"$log" 2>&1;}
build_all "$A" "$TA" "$EVIDENCE/logs/repro-build-a.log";build_all "$B" "$TB" "$EVIDENCE/logs/repro-build-b.log"
BASELINE_B_API_SHA=$(sha256sum "$TB/release/api" | awk '{print $1}')
MUTATION_TOKEN=$(date +%s%N)
python3 - "$B/apps/api/build.rs" "$MUTATION_TOKEN" <<'PY'
import pathlib,sys
path=pathlib.Path(sys.argv[1]);token=sys.argv[2];body=path.read_text()
needle='println!("cargo:rustc-env=OPENPR_EMBEDDED_PROVENANCE_SOURCE={source}");'
replacement=f'println!("cargo:rustc-env=OPENPR_EMBEDDED_PROVENANCE_SOURCE={{source}}:time={token}");'
if body.count(needle)!=1:raise SystemExit("provenance injection seam not found exactly once")
path.write_text(body.replace(needle,replacement))
PY
set +e
build_api "$B" "$TB" "$EVIDENCE/logs/repro-build-time-dependency-mutation.log"
MUTATION_BUILD_EXIT=$?
set -e
MUTATED_B_API_SHA=$(sha256sum "$TB/release/api" | awk '{print $1}')
python3 - "$ROOT" "$EVIDENCE" "$TA/release" "$TB/release" "$BASELINE_B_API_SHA" "$MUTATED_B_API_SHA" "$MUTATION_BUILD_EXIT" <<'PY'
import datetime as dt,hashlib,json,os,pathlib,subprocess,sys,tempfile
repo,evidence,a,b=map(pathlib.Path,sys.argv[1:5]);baseline_b_api_sha,mutated_b_api_sha=sys.argv[5:7];mutation_build_exit=int(sys.argv[7]);names=["api","mcp-server","sylvode","collab-isolated-apply-worker"]
digest=lambda p:hashlib.sha256(p.read_bytes()).hexdigest();cargo_home=pathlib.Path(os.environ.get("CARGO_HOME",pathlib.Path.home()/".cargo"))
rows=[]
for name in names:
 a_sha=digest(a/name);b_sha=baseline_b_api_sha if name=="api" else digest(b/name)
 rows.append({"artifact":name,"a_sha256":a_sha,"b_sha256":b_sha,"identical":a_sha==b_sha,"embedded_cargo_home_occurrences":(a/name).read_bytes().count(str(cargo_home).encode())})
mutation_red=mutation_build_exit==0 and mutated_b_api_sha!=baseline_b_api_sha
r={"schema_version":"sylvode.flow.release-build-result.v2","release":"1.0.0","source_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),"claim_scope":"same_machine_same_cargo_home_path_normalized","conditions":{"same_machine":True,"same_cargo_home":str(cargo_home),"source_and_target_paths_normalized":True,"cross_machine_verified":False,"different_cargo_home_verified":False,"cargo_home_path_remapped":False,"rustflags_source":"environment_override","cargo_config_target_rustflags_inherited":False},"controls":{"source_date_epoch":"commit_timestamp","incremental":False,"source_path_remap":"/workspace","target_path_remap":"/target","locked":True},"artifacts":rows,"mutation":{"name":"time_dependency_rebuilt","build_exit_code":mutation_build_exit,"baseline_sha256":baseline_b_api_sha,"mutated_sha256":mutated_b_api_sha,"red":mutation_red},"executed_count":len(rows)+1,"passed":all(x["identical"] for x in rows) and mutation_red,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".release-build-result.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(r,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,evidence/"release-build-result.json");print(json.dumps(r,sort_keys=True));raise SystemExit(0 if r["passed"] else 1)
PY
