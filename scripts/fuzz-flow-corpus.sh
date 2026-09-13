#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
EVIDENCE_ROOT=
CORPUS=
JSON_MODE=0
LOCKED_MODE=0
while (($#)); do
  case "$1" in
    --repo-root) REPO_ROOT=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --corpus) CORPUS=${2:?}; shift 2 ;;
    --locked-corpus) LOCKED_MODE=1; shift ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $LOCKED_MODE -eq 1 ]] || { echo 'FAIL: --locked-corpus is required' >&2; exit 2; }
[[ $JSON_MODE -eq 1 ]] || { echo 'FAIL: --json is required' >&2; exit 2; }
[[ -n $CORPUS ]] || CORPUS=testing/fixtures/flow-wire-v08/corpus.json
[[ $CORPUS = /* ]] || CORPUS="$REPO_ROOT/$CORPUS"
[[ -n $EVIDENCE_ROOT ]] || EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.8"
[[ -s $CORPUS ]] || { echo "FAIL: locked corpus missing or empty: $CORPUS" >&2; exit 2; }
mkdir -p "$EVIDENCE_ROOT/logs"
ROWS=$(mktemp "$EVIDENCE_ROOT/.fuzz-rows.XXXXXX")
trap 'rm -f "$ROWS"' EXIT

run_test() {
  local id=$1 target=$2 filter=$3
  local log="$EVIDENCE_ROOT/logs/fuzz-$id.log"
  local -a command=(cargo test --manifest-path "$REPO_ROOT/Cargo.toml" -p collab-core)
  if [[ $target == test:* ]]; then
    command+=(--test "${target#test:}")
  else
    command+=(--lib "$filter")
  fi
  [[ $target == test:* ]] || command+=(-- --exact --nocapture)
  [[ $target == test:* ]] && command+=(-- --nocapture)
  set +e
  env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 "${command[@]}" >"$log" 2>&1
  local status=$?
  set -e
  printf '%s\t%s\t%s\n' "$id" "$status" "${log#"$EVIDENCE_ROOT/"}" >>"$ROWS"
}

env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 \
  cargo build --manifest-path "$REPO_ROOT/Cargo.toml" -p collab-core \
    --bin collab-isolated-apply-worker >"$EVIDENCE_ROOT/logs/fuzz-worker-build.log" 2>&1
run_test update_bytes test:flow_v08_locked_corpus unused
run_test response_roundtrip lib isolation::host::tests::response_frame_round_trips
run_test response_corrupt lib isolation::host::tests::response_frame_rejects_a_corrupted_crc
run_test response_truncated lib isolation::host::tests::response_frame_rejects_a_truncated_frame
run_test cpu_boundary lib isolation::host::tests::decode_apply_cpu_ms_ceiling_accepts_just_under_and_kills_with_cpu_ceiling_just_over_the_boundary
run_test wall_boundary lib isolation::host::tests::decode_apply_wall_ms_ceiling_accepts_just_under_and_kills_with_wall_ceiling_just_over_the_boundary
run_test memory_boundary lib isolation::host::tests::isolated_apply_memory_bytes_ceiling_accepts_the_exact_byte_and_rejects_the_next_byte_over

python3 - "$REPO_ROOT" "$CORPUS" "$EVIDENCE_ROOT" "$ROWS" <<'PY'
import datetime as dt, hashlib, json, os, pathlib, re, subprocess, sys, tempfile
repo, corpus, evidence, rows = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3]), pathlib.Path(sys.argv[4])
manifest=json.loads(corpus.read_text())
required={
 "corrupt_update_bytes","truncated_update_bytes","duplicate_update_bytes","out_of_order_update_bytes",
 "corrupt_response_crc","truncated_response_frame","cpu_under_and_over_ceiling",
 "wall_under_and_over_ceiling","memory_exact_and_plus_one",
}
ids=[case.get("id") for case in manifest.get("cases",[])]
manifest_ok=manifest.get("schema")=="openpr.flow.wire-corpus.v0.8" and len(ids)==len(set(ids)) and required==set(ids)
checks=[]
for raw in rows.read_text().splitlines():
    cid, code, relative = raw.split("\t")
    path=evidence/relative; body=path.read_text(errors="replace")
    summaries=re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;",body,re.M)
    executed=sum(int(p)+int(f) for _,p,f,_ in summaries)
    ignored=sum(int(i) for *_,i in summaries)
    ok=int(code)==0 and executed>0 and ignored==0 and all(state=="ok" and int(failed)==0 for state,_,failed,_ in summaries)
    checks.append({"id":cid,"status":"passed" if ok else "failed","exit_code":int(code),
      "executed_count":executed,"ignored_count":ignored,"log":relative,
      "sha256":hashlib.sha256(path.read_bytes()).hexdigest()})
checks.append({"id":"locked_corpus_manifest","status":"passed" if manifest_ok else "failed",
 "executed_count":len(ids),"case_ids":ids,"sha256":hashlib.sha256(corpus.read_bytes()).hexdigest()})
dirty=subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1","--","apps","crates","migrations","scripts","testing","Cargo.toml","Cargo.lock"],text=True).splitlines()
passed=all(c["status"]=="passed" for c in checks) and not dirty
result={"schema_version":"sylvode.flow.fuzz-result.v1","release":"0.8.0",
 "source_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),
 "locked_corpus":str(corpus.resolve()),"checks":checks,"executed_count":sum(c["executed_count"] for c in checks),
 "source_dirty":bool(dirty),"dirty_entries":dirty,"passed":passed,
 "generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".fuzz-result.",dir=evidence)
with os.fdopen(fd,"w") as handle: json.dump(result,handle,sort_keys=True,indent=2); handle.write("\n")
os.replace(tmp,evidence/"fuzz-result.json")
print(json.dumps(result,sort_keys=True))
raise SystemExit(0 if passed else 1)
PY
