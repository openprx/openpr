#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd);EVIDENCE="$ROOT/.flow-gate/evidence/v1.0";JSON=0
while (($#));do case "$1" in --repo-root) ROOT=${2:?};shift 2;;--evidence-root) EVIDENCE=${2:?};shift 2;;--json) JSON=1;shift;;*) echo "FAIL: unsupported argument: $1" >&2;exit 2;;esac;done
[[ $JSON -eq 1 ]]||{ echo 'FAIL: --json required' >&2;exit 2;};mkdir -p "$EVIDENCE/logs" /opt/worker/.cache
RUN=$(mktemp -d /opt/worker/.cache/v10-repro.XXXXXX);A="$RUN/a";B="$RUN/b";TA="$RUN/target-a";TB="$RUN/target-b"
cleanup(){ git -C "$ROOT" worktree remove --force "$A" >/dev/null 2>&1||true;git -C "$ROOT" worktree remove --force "$B" >/dev/null 2>&1||true;rm -rf -- "$RUN";};trap cleanup EXIT
git -C "$ROOT" worktree add --detach "$A" HEAD >/dev/null;git -C "$ROOT" worktree add --detach "$B" HEAD >/dev/null
EPOCH=$(git -C "$ROOT" show -s --format=%ct HEAD)
build(){ local src=$1 target=$2 log=$3;env -u RUST_TEST_THREADS SOURCE_DATE_EPOCH="$EPOCH" CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$target" RUSTFLAGS="--remap-path-prefix=$src=/workspace --remap-path-prefix=$target=/target" cargo build --manifest-path "$src/Cargo.toml" --locked --release -p api --bin api -p mcp-server --bin mcp-server --bin sylvode -p collab-core --bin collab-isolated-apply-worker >"$log" 2>&1;}
build "$A" "$TA" "$EVIDENCE/logs/repro-build-a.log";build "$B" "$TB" "$EVIDENCE/logs/repro-build-b.log"
python3 - "$ROOT" "$EVIDENCE" "$TA/release" "$TB/release" <<'PY'
import datetime as dt,hashlib,json,os,pathlib,shutil,subprocess,sys,tempfile
repo,evidence,a,b=map(pathlib.Path,sys.argv[1:]);names=["api","mcp-server","sylvode","collab-isolated-apply-worker"]
digest=lambda p:hashlib.sha256(p.read_bytes()).hexdigest();rows=[]
for name in names:rows.append({"artifact":name,"a_sha256":digest(a/name),"b_sha256":digest(b/name),"identical":digest(a/name)==digest(b/name)})
probe=tempfile.NamedTemporaryFile(dir=evidence,delete=False);probe.write((a/names[0]).read_bytes()+b"timestamp=1");probe.close();mutation_red=digest(pathlib.Path(probe.name))!=digest(a/names[0]);os.unlink(probe.name)
r={"schema_version":"sylvode.flow.release-build-result.v1","release":"1.0.0","source_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),"controls":{"source_date_epoch":"commit_timestamp","incremental":False,"source_path_remap":"/workspace","target_path_remap":"/target","locked":True},"artifacts":rows,"mutation":{"name":"timestamp_bytes_injected","red":mutation_red},"executed_count":len(rows)+1,"passed":all(x["identical"] for x in rows) and mutation_red,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".release-build-result.",dir=evidence)
with os.fdopen(fd,"w") as f:json.dump(r,f,sort_keys=True,indent=2);f.write("\n")
os.replace(tmp,evidence/"release-build-result.json");print(json.dumps(r,sort_keys=True));raise SystemExit(0 if r["passed"] else 1)
PY
