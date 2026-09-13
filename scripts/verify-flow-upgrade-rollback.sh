#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
FROM=
TO=
JSON_MODE=0
EVIDENCE_ROOT=
while (($#)); do
  case "$1" in
    --from) FROM=${2:?}; shift 2 ;;
    --to) TO=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $FROM == 0.7 && $TO == 0.8 && $JSON_MODE -eq 1 ]] || { echo 'FAIL: require --from 0.7 --to 0.8 --json' >&2; exit 2; }
: "${OPENPR_TEST_DATABASE_URL:?OPENPR_TEST_DATABASE_URL is required}"
command -v curl >/dev/null
command -v psql >/dev/null

OLD_HEAD=d881462957e3ffde4d6782cf578348de021208a8
CACHE_ROOT=/opt/worker/.cache/openpr-v08-upgrade-rollback
OLD_WORKTREE="$CACHE_ROOT/v07"
TARGET_DIR=/opt/worker/.cache/openpr-v08-shared-target
DB_NAME=v08_upgrade_rollback
ADMIN_URL=$OPENPR_TEST_DATABASE_URL
DB_URL="${ADMIN_URL%/*}/$DB_NAME"
[[ -n $EVIDENCE_ROOT ]] || EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.8"
mkdir -p "$CACHE_ROOT/logs" "$TARGET_DIR" "$EVIDENCE_ROOT"
CURRENT_PID=
OLD_PID=
cleanup() {
  [[ -z $CURRENT_PID ]] || kill -TERM "$CURRENT_PID" >/dev/null 2>&1 || true
  [[ -z $OLD_PID ]] || kill -TERM "$OLD_PID" >/dev/null 2>&1 || true
  psql "$ADMIN_URL" -X -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS \"$DB_NAME\" WITH (FORCE)" >/dev/null 2>&1 || true
  git -C "$REPO_ROOT" worktree remove --force "$OLD_WORKTREE" >/dev/null 2>&1 || true
}
trap cleanup EXIT
cleanup
git -C "$REPO_ROOT" worktree add --detach "$OLD_WORKTREE" "$OLD_HEAD" >/dev/null

free_port() {
  python3 - <<'PY'
import socket
s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()
PY
}
CURRENT_PORT=$(free_port)
OLD_PORT=$(free_port)
python3 - "$CACHE_ROOT/current.toml" "$DB_URL" "$CURRENT_PORT" "$CACHE_ROOT/current-uploads" <<'PY'
import pathlib,sys
path,url,port,uploads=sys.argv[1:]
pathlib.Path(path).write_text(f'''[server]\napp_name="v08-upgrade"\nbind_addr="127.0.0.1:{port}"\n[database]\nurl="{url}"\nmax_connections=5\nmin_connections=1\n[auth]\njwt_secret="0123456789abcdef0123456789abcdef"\n[logging]\nfilter="api=warn"\nformat="text"\noutput="stderr"\n[storage]\nbackend="local"\ndir="{uploads}"\n''')
PY
cp "$CACHE_ROOT/current.toml" "$CACHE_ROOT/old.toml"
python3 - "$CACHE_ROOT/old.toml" "$CURRENT_PORT" "$OLD_PORT" <<'PY'
import pathlib,sys
path,old,new=sys.argv[1:]; p=pathlib.Path(path); p.write_text(p.read_text().replace(f'127.0.0.1:{old}',f'127.0.0.1:{new}'))
PY

env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 cargo build --manifest-path "$REPO_ROOT/Cargo.toml" -p api --bin api \
  >"$CACHE_ROOT/logs/current-build.log" 2>&1
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
  cargo build --manifest-path "$OLD_WORKTREE/Cargo.toml" -p api --bin api \
  >"$CACHE_ROOT/logs/old-build.log" 2>&1

psql "$ADMIN_URL" -X -v ON_ERROR_STOP=1 -c "CREATE DATABASE \"$DB_NAME\"" >/dev/null

wait_ready() {
  local pid=$1 port=$2 log=$3
  for _ in $(seq 1 180); do
    if curl --fail --silent --max-time 1 "http://127.0.0.1:$port/ready" | grep -Fq '"status":"ready"'; then return 0; fi
    kill -0 "$pid" >/dev/null 2>&1 || { tail -100 "$log" >&2; return 1; }
    sleep 1
  done
  tail -100 "$log" >&2
  return 1
}

"$REPO_ROOT/target/debug/api" --config "$CACHE_ROOT/current.toml" >"$CACHE_ROOT/logs/current-api.log" 2>&1 &
CURRENT_PID=$!
wait_ready "$CURRENT_PID" "$CURRENT_PORT" "$CACHE_ROOT/logs/current-api.log"
kill -TERM "$CURRENT_PID"; wait "$CURRENT_PID" || true; CURRENT_PID=

psql "$DB_URL" -X -v ON_ERROR_STOP=1 -c \
  "UPDATE flow_v08_rollback_control SET compaction_paused=true,retention_paused=true,import_promotion_paused=true,reason='application rollback to v0.7',changed_at=now() WHERE singleton=true" >/dev/null

psql "$DB_URL" -X -At -v ON_ERROR_STOP=1 -c \
  "SELECT table_name||'.'||column_name FROM information_schema.columns WHERE table_schema='public' AND ((table_name='collab_documents' AND column_name IN ('snapshot_checksum','compaction_boundary_seq','compaction_boundary_frontier','last_compacted_at','compaction_generation','shallow_snapshot_enabled')) OR (table_name='flow_import_lineage' AND column_name IN ('package_sha256','target_kind','target_id'))) ORDER BY 1" \
  >"$CACHE_ROOT/metadata-before.txt"

"$TARGET_DIR/debug/api" --config "$CACHE_ROOT/old.toml" >"$CACHE_ROOT/logs/old-api.log" 2>&1 &
OLD_PID=$!
wait_ready "$OLD_PID" "$OLD_PORT" "$CACHE_ROOT/logs/old-api.log"
kill -TERM "$OLD_PID"; wait "$OLD_PID" || true; OLD_PID=

psql "$DB_URL" -X -At -v ON_ERROR_STOP=1 -c \
  "SELECT table_name||'.'||column_name FROM information_schema.columns WHERE table_schema='public' AND ((table_name='collab_documents' AND column_name IN ('snapshot_checksum','compaction_boundary_seq','compaction_boundary_frontier','last_compacted_at','compaction_generation','shallow_snapshot_enabled')) OR (table_name='flow_import_lineage' AND column_name IN ('package_sha256','target_kind','target_id'))) ORDER BY 1" \
  >"$CACHE_ROOT/metadata-after.txt"
cmp "$CACHE_ROOT/metadata-before.txt" "$CACHE_ROOT/metadata-after.txt"
[[ $(wc -l <"$CACHE_ROOT/metadata-after.txt") -eq 9 ]]
[[ $(psql "$DB_URL" -X -At -v ON_ERROR_STOP=1 -c "SELECT compaction_paused AND retention_paused AND import_promotion_paused FROM flow_v08_rollback_control WHERE singleton=true") == t ]]
[[ $(psql "$DB_URL" -X -At -v ON_ERROR_STOP=1 -c "SELECT count(*) FROM schema_migrations WHERE name BETWEEN '0063' AND '0069z' AND status='applied'") -eq 7 ]]

ROWS="$CACHE_ROOT/wire-rows.tsv"
: >"$ROWS"
run_wire() {
  local version=$1 root=$2 test_target=$3
  local log="$CACHE_ROOT/logs/wire-$version-$test_target.log"
  set +e
  env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
    cargo test --manifest-path "$root/Cargo.toml" -p mcp-server --test "$test_target" -- --nocapture >"$log" 2>&1
  local status=$?
  set -e
  printf '%s\t%s\t%s\t%s\n' "$version" "$test_target" "$status" "$log" >>"$ROWS"
}
for target in flow_bridge_e2e flow_collections_e2e flow_policy_bypass_stdio_e2e; do run_wire v07 "$OLD_WORKTREE" "$target"; done
for target in flow_bridge_e2e flow_collections_e2e flow_policy_bypass_stdio_e2e; do run_wire v08 "$REPO_ROOT" "$target"; done

python3 - "$REPO_ROOT" "$EVIDENCE_ROOT" "$ROWS" "$CACHE_ROOT/metadata-after.txt" "$OLD_HEAD" <<'PY'
import datetime as dt,hashlib,json,os,pathlib,re,subprocess,sys,tempfile
repo,evidence,rows,metadata=map(pathlib.Path,sys.argv[1:5]); old_head=sys.argv[5]
checks=[]
for raw in rows.read_text().splitlines():
    version,target,code,log=raw.split("\t"); path=pathlib.Path(log); body=path.read_text(errors="replace")
    summaries=re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;",body,re.M)
    executed=sum(int(p)+int(f) for _,p,f,_ in summaries); ignored=sum(int(i) for *_,i in summaries)
    ok=int(code)==0 and executed>0 and ignored==0 and all(s=="ok" and int(f)==0 for s,_,f,_ in summaries)
    checks.append({"version":version,"target":target,"exit_code":int(code),"executed_count":executed,
      "ignored_count":ignored,"status":"passed" if ok else "failed","log":log,"sha256":hashlib.sha256(path.read_bytes()).hexdigest()})
dirty=subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1","--","apps","crates","migrations","scripts","testing","Cargo.toml","Cargo.lock"],text=True).splitlines()
passed=all(c["status"]=="passed" for c in checks) and not dirty
result={"schema_version":"sylvode.flow.upgrade-rollback-result.v1","release":"0.8.0",
 "from_head":old_head,"to_head":subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip(),
 "upgrade":{"current_application_ready":True,"migrations_0063_through_0069_applied":True},
 "rollback":{"v07_application_ready_against_v08_database":True,
   "compaction_retention_import_promotion_paused":True,"new_metadata_columns_preserved":metadata.read_text().splitlines()},
 "wire_corpus_checks":checks,"executed_count":sum(c["executed_count"] for c in checks)+4,
 "source_dirty":bool(dirty),"dirty_entries":dirty,"passed":passed,
 "generated_at":dt.datetime.now(dt.timezone.utc).isoformat()}
fd,tmp=tempfile.mkstemp(prefix=".upgrade-rollback.",dir=evidence)
with os.fdopen(fd,"w") as f: json.dump(result,f,sort_keys=True,indent=2); f.write("\n")
os.replace(tmp,evidence/"upgrade-rollback-result.json")
print(json.dumps(result,sort_keys=True))
raise SystemExit(0 if passed else 1)
PY
