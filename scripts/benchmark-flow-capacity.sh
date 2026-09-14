#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CLIENTS=
JSON_MODE=0
EVIDENCE_ROOT=
while (($#)); do
  case "$1" in
    --clients) CLIENTS=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $CLIENTS == 10,50 && $JSON_MODE -eq 1 ]] || { echo 'FAIL: require --clients 10,50 --json' >&2; exit 2; }
: "${OPENPR_CAPACITY_DATABASE_URL:?OPENPR_CAPACITY_DATABASE_URL is required}"
: "${OPENPR_CAPACITY_PG_CONTAINER:?OPENPR_CAPACITY_PG_CONTAINER is required}"
[[ -n $EVIDENCE_ROOT ]] || EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.8"
mkdir -p "$EVIDENCE_ROOT/logs"
ROWS=$(mktemp "$EVIDENCE_ROOT/.capacity-rows.XXXXXX")
trap 'rm -f "$ROWS"' EXIT

env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 \
  cargo build --manifest-path "$REPO_ROOT/Cargo.toml" --release -p collab-core \
    --bin collab-isolated-apply-worker >"$EVIDENCE_ROOT/logs/capacity-worker-build.log" 2>&1

for clients in 10 50; do
  raw="$EVIDENCE_ROOT/logs/capacity-$clients-raw.json"
  log="$EVIDENCE_ROOT/logs/capacity-$clients.log"
  set +e
  env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="$OPENPR_CAPACITY_DATABASE_URL" \
    OPENPR_FLOW_DEDICATED_PG_CONTAINER="$OPENPR_CAPACITY_PG_CONTAINER" \
    OPENPR_FLOW_PG_LOG_CONTAINER="$OPENPR_CAPACITY_PG_CONTAINER" \
    OPENPR_FLOW_PG_LOG_ENGINE=podman OPENPR_FLOW_QUIET_PG_QUALIFIED=1 \
    OPENPR_FLOW_CAPACITY_CLIENTS="$clients" OPENPR_FLOW_LOAD_HARNESS_OUT="$raw" \
    CARGO_BUILD_JOBS=4 cargo test --manifest-path "$REPO_ROOT/Cargo.toml" --release -p api \
      --test flow_collab_load_harness ten_client_load_harness_round_trip_p95_and_lock_hold_p95 \
      -- --exact --ignored --nocapture >"$log" 2>&1
  status=$?
  set -e
  printf '%s\t%s\t%s\t%s\n' "$clients" "$status" "$raw" "$log" >>"$ROWS"
done

python3 - "$REPO_ROOT" "$EVIDENCE_ROOT" "$ROWS" <<'PY'
import datetime as dt,hashlib,json,os,pathlib,re,subprocess,sys,tempfile
repo,evidence,rows=map(pathlib.Path,sys.argv[1:])
runs=[]
for raw_row in rows.read_text().splitlines():
    clients,code,raw,log=raw_row.split("\t"); raw=pathlib.Path(raw); log=pathlib.Path(log)
    body=log.read_text(errors="replace")
    summaries=re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;",body,re.M)
    executed=sum(int(p)+int(f) for _,p,f,_ in summaries)
    payload=json.loads(raw.read_text()) if raw.is_file() and raw.stat().st_size else None
    result_key=f"{clients}_client";tier_keys=sorted(k for k in payload or {} if re.fullmatch(r"\d+_client",k))
    actual=payload.get(result_key,{}).get("clients") if payload else None
    functional=int(code)==0 and executed>0 and tier_keys==[result_key] and actual==int(clients) and payload.get("passed") is True
    runs.append({"clients":int(clients),"exit_code":int(code),"executed_count":executed,
      "result_key":result_key,"observed_result_keys":tier_keys,"functional_status":"passed" if functional else "failed","result":payload,
      "raw":str(raw),"raw_sha256":hashlib.sha256(raw.read_bytes()).hexdigest() if raw.is_file() else None,
      "log":str(log)})
unset=["tested_websocket_connections_per_instance_min","tested_sustained_updates_per_second_min",
 "slow_consumer_disconnect_ms_p95_max","bootstrap_at_8mib_ms_p95_max","projection_lag_ms_p95_max",
 "search_lag_ms_p95_max","recovery_time_seconds_max"]
dirty=subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1","--","apps","crates","migrations","scripts","testing","Cargo.toml","Cargo.lock"],text=True).splitlines()
functional=all(r["functional_status"]=="passed" for r in runs)
result={"schema_version":"sylvode.flow.capacity-result.v1","release":"0.8.0",
 "source_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),
 "runs":runs,"functional_passed":functional,"approved_budgets_status":"unset","unset_budgets":unset,
 "executed_count":sum(r["executed_count"] for r in runs),"source_dirty":bool(dirty),"dirty_entries":dirty,
 "passed":False,"generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".capacity-result.",dir=evidence)
with os.fdopen(fd,"w") as f: json.dump(result,f,sort_keys=True,indent=2); f.write("\n")
os.replace(tmp,evidence/"capacity-result.json")
print(json.dumps(result,sort_keys=True))
raise SystemExit(1)
PY
