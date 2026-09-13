#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
JSON_MODE=0
EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.9"
while (($#)); do
  case "$1" in
    --json) JSON_MODE=1; shift ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $JSON_MODE -eq 1 ]] || { echo "FAIL: require --json" >&2; exit 2; }
: "${OPENPR_TEST_DATABASE_URL:?OPENPR_TEST_DATABASE_URL is required}"
for command_name in cargo curl jq psql python3 git sha256sum; do
  command -v "$command_name" >/dev/null || { echo "FAIL: missing $command_name" >&2; exit 2; }
done
if command -v bun >/dev/null; then BUN=$(command -v bun); elif [[ -x /home/ck/.bun/bin/bun ]]; then BUN=/home/ck/.bun/bin/bun; else echo "FAIL: missing bun" >&2; exit 2; fi

V029_HEAD=cac9e7c305c1c31c6f9bb6dbe164c955731b3fca
V08_HEAD=aa453de2a12911b4fe343c6ce52d67e4bf8e679e
CACHE_ROOT=/opt/worker/.cache/v09-install-upgrade
LOG_ROOT="$CACHE_ROOT/logs"
V029_WORKTREE="$CACHE_ROOT/v029"
V08_WORKTREE="$CACHE_ROOT/v08"
V029_TARGET=/opt/worker/.cache/v09-install-target-v029
V08_TARGET=/opt/worker/.cache/v09-install-target-v08
CLEAN_DB=v09_clean_install
UPGRADE_DB=v09_in_place_upgrade
ADMIN_URL=$OPENPR_TEST_DATABASE_URL
CLEAN_URL="${ADMIN_URL%/*}/$CLEAN_DB"
UPGRADE_URL="${ADMIN_URL%/*}/$UPGRADE_DB"
mkdir -p "$LOG_ROOT" "$EVIDENCE_ROOT" "$CACHE_ROOT/current-uploads" "$CACHE_ROOT/v029-uploads" "$CACHE_ROOT/v08-uploads"

API_PID=
WORKER_PID=
MCP_PID=
WEB_PID=
cleanup() {
  for process_id in "$WORKER_PID" "$MCP_PID" "$WEB_PID"; do
    [[ -z $process_id ]] || kill -TERM "$process_id" >/dev/null 2>&1 || true
  done
  WORKER_PID=''
  MCP_PID=''
  WEB_PID=''
  if [[ -n $API_PID ]]; then
    kill -TERM "$API_PID" >/dev/null 2>&1 || true
    wait "$API_PID" >/dev/null 2>&1 || true
    API_PID=
  fi
  psql "$ADMIN_URL" -X -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS \"$CLEAN_DB\" WITH (FORCE)" >/dev/null 2>&1 || true
  psql "$ADMIN_URL" -X -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS \"$UPGRADE_DB\" WITH (FORCE)" >/dev/null 2>&1 || true
  git -C "$REPO_ROOT" worktree remove --force "$V029_WORKTREE" >/dev/null 2>&1 || true
  git -C "$REPO_ROOT" worktree remove --force "$V08_WORKTREE" >/dev/null 2>&1 || true
}
trap cleanup EXIT
cleanup
[[ ! -e $V029_WORKTREE && ! -e $V08_WORKTREE ]] || { echo "FAIL: stale verifier worktree under $CACHE_ROOT" >&2; exit 2; }

git -C "$REPO_ROOT" worktree add --detach "$V029_WORKTREE" "$V029_HEAD" >"$LOG_ROOT/v029-worktree.log" 2>&1
git -C "$REPO_ROOT" worktree add --detach "$V08_WORKTREE" "$V08_HEAD" >"$LOG_ROOT/v08-worktree.log" 2>&1

env -u CARGO_TARGET_DIR -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 \
  cargo build --manifest-path "$REPO_ROOT/Cargo.toml" -p api -p worker -p mcp-server --bins \
  >"$LOG_ROOT/current-build.log" 2>&1
"$BUN" run --cwd "$REPO_ROOT/frontend" build >"$LOG_ROOT/current-frontend-build.log" 2>&1
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$V029_TARGET" \
  cargo build --manifest-path "$V029_WORKTREE/Cargo.toml" -p api --bin api >"$LOG_ROOT/v029-build.log" 2>&1
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$V08_TARGET" \
  cargo build --manifest-path "$V08_WORKTREE/Cargo.toml" -p api --bin api -p mcp-server --bin mcp-server \
  >"$LOG_ROOT/v08-build.log" 2>&1

free_port() {
  python3 - <<'PY'
import socket
sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
}

write_api_config() {
  local path=$1 database_url=$2 port=$3 uploads=$4 name=$5
  python3 - "$path" "$database_url" "$port" "$uploads" "$name" <<'PY'
import pathlib
import sys
path, database_url, port, uploads, name = sys.argv[1:]
pathlib.Path(path).write_text(f'''[server]
app_name="{name}"
bind_addr="127.0.0.1:{port}"
[database]
url="{database_url}"
max_connections=8
min_connections=1
[auth]
jwt_secret="v09-install-upgrade-fixed-secret"
[logging]
filter="api=warn"
format="text"
output="stderr"
[storage]
backend="local"
dir="{uploads}"
''')
PY
}

wait_ready() {
  local pid=$1 port=$2 log=$3
  for _ in $(seq 1 240); do
    if curl --fail --silent --max-time 1 "http://127.0.0.1:$port/ready" | jq -e '.code == 0 and .data.status == "ready"' >/dev/null 2>&1; then
      return 0
    fi
    kill -0 "$pid" >/dev/null 2>&1 || { tail -100 "$log" >&2; return 1; }
    sleep 0.5
  done
  tail -100 "$log" >&2
  return 1
}

start_api() {
  local binary=$1 config=$2 port=$3 log=$4
  "$binary" --config "$config" >"$log" 2>&1 &
  API_PID=$!
  wait_ready "$API_PID" "$port" "$log"
}

stop_api() {
  kill -TERM "$API_PID"
  wait "$API_PID" || true
  API_PID=
}

psql "$ADMIN_URL" -X -v ON_ERROR_STOP=1 -c "CREATE DATABASE \"$CLEAN_DB\"" >/dev/null
CLEAN_PORT=$(free_port)
write_api_config "$CACHE_ROOT/current-clean.toml" "$CLEAN_URL" "$CLEAN_PORT" "$CACHE_ROOT/current-uploads" v09-clean-install
start_api "$REPO_ROOT/target/debug/api" "$CACHE_ROOT/current-clean.toml" "$CLEAN_PORT" "$LOG_ROOT/current-clean-api.log"
CLEAN_MIGRATIONS=$(psql "$CLEAN_URL" -X -At -v ON_ERROR_STOP=1 -c "SELECT count(*) FROM schema_migrations WHERE status='applied'")
CLEAN_TABLES=$(psql "$CLEAN_URL" -X -At -v ON_ERROR_STOP=1 -c "SELECT count(*) FROM (VALUES ('users'),('workspaces'),('projects'),('project_forms'),('form_records'),('flow_objects'),('flow_import_jobs')) AS wanted(name) WHERE to_regclass('public.'||name) IS NOT NULL")
[[ $CLEAN_MIGRATIONS -gt 0 && $CLEAN_TABLES -eq 7 ]]

"$REPO_ROOT/target/debug/worker" --config "$CACHE_ROOT/current-clean.toml" >"$LOG_ROOT/current-clean-worker.log" 2>&1 &
WORKER_PID=$!
sleep 2
kill -0 "$WORKER_PID"

CLEAN_MCP_PORT=$(free_port)
cat >"$CACHE_ROOT/current-clean-mcp.toml" <<EOF
[logging]
filter="error"
format="text"
output="stderr"
[mcp]
api_url="http://127.0.0.1:$CLEAN_PORT"
bot_token="opr_clean_install_fixture"
workspace_id="33333333-3333-4333-8333-333333333333"
transport="http"
bind_addr="127.0.0.1:$CLEAN_MCP_PORT"
EOF
"$REPO_ROOT/target/debug/mcp-server" serve --config "$CACHE_ROOT/current-clean-mcp.toml" \
  --transport http --bind-addr "127.0.0.1:$CLEAN_MCP_PORT" >"$LOG_ROOT/current-clean-mcp.log" 2>&1 &
MCP_PID=$!
for _ in $(seq 1 120); do
  curl --fail --silent --max-time 1 "http://127.0.0.1:$CLEAN_MCP_PORT/health" >/dev/null 2>&1 && break
  kill -0 "$MCP_PID" >/dev/null 2>&1 || { tail -100 "$LOG_ROOT/current-clean-mcp.log" >&2; exit 1; }
  sleep 0.25
done
curl --fail --silent --max-time 2 "http://127.0.0.1:$CLEAN_MCP_PORT/health" >/dev/null

CLEAN_WEB_PORT=$(free_port)
"$BUN" run --cwd "$REPO_ROOT/frontend" preview -- --host 127.0.0.1 --port "$CLEAN_WEB_PORT" \
  >"$LOG_ROOT/current-clean-web.log" 2>&1 &
WEB_PID=$!
for _ in $(seq 1 120); do
  curl --fail --silent --max-time 1 "http://127.0.0.1:$CLEAN_WEB_PORT/" >/dev/null 2>&1 && break
  kill -0 "$WEB_PID" >/dev/null 2>&1 || { tail -100 "$LOG_ROOT/current-clean-web.log" >&2; exit 1; }
  sleep 0.25
done
curl --fail --silent --max-time 2 "http://127.0.0.1:$CLEAN_WEB_PORT/" >/dev/null
kill -TERM "$WORKER_PID" "$MCP_PID" "$WEB_PID"
wait "$WORKER_PID" || true; wait "$MCP_PID" || true; wait "$WEB_PID" || true
WORKER_PID=''
MCP_PID=''
WEB_PID=''
stop_api
psql "$ADMIN_URL" -X -v ON_ERROR_STOP=1 -c "DROP DATABASE \"$CLEAN_DB\" WITH (FORCE)" >/dev/null

psql "$ADMIN_URL" -X -v ON_ERROR_STOP=1 -c "CREATE DATABASE \"$UPGRADE_DB\"" >/dev/null
V029_PORT=$(free_port)
CURRENT_PORT=$(free_port)
V08_PORT=$(free_port)
write_api_config "$CACHE_ROOT/v029.toml" "$UPGRADE_URL" "$V029_PORT" "$CACHE_ROOT/v029-uploads" v029-upgrade-source
write_api_config "$CACHE_ROOT/current-upgrade.toml" "$UPGRADE_URL" "$CURRENT_PORT" "$CACHE_ROOT/current-uploads" v09-upgrade-target
write_api_config "$CACHE_ROOT/v08.toml" "$UPGRADE_URL" "$V08_PORT" "$CACHE_ROOT/v08-uploads" v08-rollback
start_api "$V029_TARGET/debug/api" "$CACHE_ROOT/v029.toml" "$V029_PORT" "$LOG_ROOT/v029-api.log"

OWNER_ID=11111111-1111-4111-8111-111111111111
BOT_ID=22222222-2222-4222-8222-222222222222
WORKSPACE_ID=33333333-3333-4333-8333-333333333333
PROJECT_ID=44444444-4444-4444-8444-444444444444
FORM_ID=55555555-5555-4555-8555-555555555555
RECORD_ID=66666666-6666-4666-8666-666666666666
BOT_TOKEN=opr_v09_old_client_token
BOT_HASH=$(printf '%s' "$BOT_TOKEN" | sha256sum | awk '{print $1}')
psql "$UPGRADE_URL" -X -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id,email,password_hash,name,role,is_active,entity_type,agent_type,created_at,updated_at)
VALUES ('$OWNER_ID','v029-owner@example.local','!','v0.2.9 Owner','admin',true,'human',NULL,now(),now()),
       ('$BOT_ID','$BOT_ID@bot.openpr.local','!','v0.8 Client Bot','user',true,'bot_mcp','mcp',now(),now());
INSERT INTO workspaces (id,slug,name,created_by,created_at,updated_at)
VALUES ('$WORKSPACE_ID','v029-upgrade-fixture','v0.2.9 Upgrade Fixture','$OWNER_ID',now(),now());
INSERT INTO workspace_members (workspace_id,user_id,role,created_at)
VALUES ('$WORKSPACE_ID','$OWNER_ID','owner',now()),('$WORKSPACE_ID','$BOT_ID','admin',now());
INSERT INTO workspace_bots (id,workspace_id,name,token_hash,token_prefix,permissions,created_by,is_active,created_at,updated_at)
VALUES ('$BOT_ID','$WORKSPACE_ID','v0.8 Client Bot','$BOT_HASH',substring('$BOT_TOKEN' from 1 for 8),'["read","write","admin"]'::jsonb,'$OWNER_ID',true,now(),now());
INSERT INTO projects (id,workspace_id,key,name,description,created_by,type_key,type_settings,created_at,updated_at)
VALUES ('$PROJECT_ID','$WORKSPACE_ID','UP09','Upgrade Project','Created on v0.2.9','$OWNER_ID',NULL,'{"source":"v0.2.9"}'::jsonb,now(),now());
INSERT INTO project_forms (id,workspace_id,project_id,key,name,description,title_template,schema,detail_layout,created_by,created_at,updated_at)
VALUES ('$FORM_ID','$WORKSPACE_ID','$PROJECT_ID','upgrade_form','Upgrade Form','Created on v0.2.9','{name}',
        '{"version":"openpr.form.schema.v1","fields":[{"key":"name","label":"Name","type":"text","required":true}]}'::jsonb,
        '{}'::jsonb,'$OWNER_ID',now(),now());
INSERT INTO form_records (id,workspace_id,project_id,form_id,title,values,source,created_by,updated_by,created_at,updated_at)
VALUES ('$RECORD_ID','$WORKSPACE_ID','$PROJECT_ID','$FORM_ID','Preserved Record','{"name":"Preserved Record"}'::jsonb,
        '{"kind":"v0.2.9-fixture"}'::jsonb,'$OWNER_ID','$OWNER_ID',now(),now());
SQL

snapshot_data() {
  local output=$1
  psql "$UPGRADE_URL" -X -At -v ON_ERROR_STOP=1 -c "SELECT jsonb_build_object(
    'project',(SELECT jsonb_build_object('id',id,'key',key,'name',name,'description',description,'type_key',type_key,'type_settings',type_settings) FROM projects WHERE id='$PROJECT_ID'),
    'form',(SELECT jsonb_build_object('id',id,'key',key,'name',name,'schema',schema,'schema_version',schema_version) FROM project_forms WHERE id='$FORM_ID'),
    'record',(SELECT jsonb_build_object('id',id,'title',title,'values',values,'source',source,'schema_version',schema_version) FROM form_records WHERE id='$RECORD_ID'))::text" >"$output"
}
snapshot_data "$CACHE_ROOT/before-upgrade.json"
V029_MIGRATIONS=$(psql "$UPGRADE_URL" -X -At -v ON_ERROR_STOP=1 -c "SELECT count(*) FROM schema_migrations WHERE status='applied'")
stop_api

start_api "$REPO_ROOT/target/debug/api" "$CACHE_ROOT/current-upgrade.toml" "$CURRENT_PORT" "$LOG_ROOT/current-upgrade-api.log"
snapshot_data "$CACHE_ROOT/after-upgrade.json"
cmp "$CACHE_ROOT/before-upgrade.json" "$CACHE_ROOT/after-upgrade.json"
CURRENT_MIGRATIONS=$(psql "$UPGRADE_URL" -X -At -v ON_ERROR_STOP=1 -c "SELECT count(*) FROM schema_migrations WHERE status='applied'")
[[ $CURRENT_MIGRATIONS -gt $V029_MIGRATIONS ]]

OWNER_JWT=$(python3 - "$OWNER_ID" <<'PY'
import base64, hashlib, hmac, json, sys, time
def encoded(value):
    return base64.urlsafe_b64encode(json.dumps(value,separators=(",",":")).encode()).rstrip(b"=").decode()
now=int(time.time()); secret=b"v09-install-upgrade-fixed-secret"
header=encoded({"alg":"HS256","typ":"JWT"})
body=encoded({"sub":sys.argv[1],"email":"v029-owner@example.local","token_type":"access","iat":now,"exp":now+3600})
signature=base64.urlsafe_b64encode(hmac.new(secret,f"{header}.{body}".encode(),hashlib.sha256).digest()).rstrip(b"=").decode()
print(f"{header}.{body}.{signature}")
PY
)
api_get() { curl -sS -H "Authorization: Bearer $OWNER_JWT" "http://127.0.0.1:$1$2"; }
PROJECT_RESPONSE=$(api_get "$CURRENT_PORT" "/api/v1/projects/$PROJECT_ID")
FORMS_RESPONSE=$(api_get "$CURRENT_PORT" "/api/v1/projects/$PROJECT_ID/forms")
jq -e --arg id "$PROJECT_ID" '.code == 0 and .data.id == $id and .data.name == "Upgrade Project"' <<<"$PROJECT_RESPONSE" >/dev/null
jq -e --arg id "$FORM_ID" '.code == 0 and any(.data.items[]; .id == $id and .name == "Upgrade Form")' <<<"$FORMS_RESPONSE" >/dev/null

[[ $(psql "$UPGRADE_URL" -X -At -v ON_ERROR_STOP=1 -c "SELECT count(*) FROM flow_workspace_settings WHERE workspace_id='$WORKSPACE_ID'") -eq 0 ]]
FLOW_DEFAULT=$(api_get "$CURRENT_PORT" "/api/v1/workspaces/$WORKSPACE_ID/features/flow")
jq -e '.code == 0 and .data.flow_enabled == false' <<<"$FLOW_DEFAULT" >/dev/null
[[ $(psql "$UPGRADE_URL" -X -At -v ON_ERROR_STOP=1 -c "SELECT count(*) FROM flow_workspace_settings WHERE workspace_id='$WORKSPACE_ID'") -eq 0 ]]
FLOW_ENABLED=$(curl -sS -X PUT -H "Authorization: Bearer $OWNER_JWT" -H 'Content-Type: application/json' \
  "http://127.0.0.1:$CURRENT_PORT/api/v1/workspaces/$WORKSPACE_ID/features/flow" \
  -d '{"enabled":true,"idempotency_key":"v09-gradual-enable"}')
jq -e '.code == 0 and .data.flow_enabled == true' <<<"$FLOW_ENABLED" >/dev/null

LEGACY_REST_TOKEN_RESPONSE=$(curl -sS -H "Authorization: Bearer $BOT_TOKEN" \
  "http://127.0.0.1:$CURRENT_PORT/api/v1/workspaces/$WORKSPACE_ID/projects")
jq -e --arg id "$PROJECT_ID" '.code == 0 and any(.data.items[]; .id == $id)' <<<"$LEGACY_REST_TOKEN_RESPONSE" >/dev/null
OLD_CLIENT_BOT=$(curl -sS -X POST -H "Authorization: Bearer $OWNER_JWT" -H 'Content-Type: application/json' \
  "http://127.0.0.1:$CURRENT_PORT/api/v1/workspaces/$WORKSPACE_ID/bots" \
  -d '{"name":"v0.8 stdio compatibility","permissions":["read","write","admin"],"transport_surface":"mcp_stdio"}')
MCP_BOT_TOKEN=$(jq -er '.data.token' <<<"$OLD_CLIENT_BOT")

cat >"$CACHE_ROOT/v08-mcp.toml" <<'TOML'
[logging]
filter="error"
format="text"
output="stderr"
TOML
python3 - "$V08_TARGET/debug/mcp-server" "$CACHE_ROOT/v08-mcp.toml" "$CURRENT_PORT" "$MCP_BOT_TOKEN" "$WORKSPACE_ID" \
  >"$CACHE_ROOT/old-client-summary.json" 2>"$LOG_ROOT/old-client-mcp.log" <<'PY'
import json, subprocess, sys
binary, config, port, token, workspace = sys.argv[1:]
requests = [
  {"jsonrpc":"2.0","id":1,"method":"initialize","params":{}},
  {"jsonrpc":"2.0","id":2,"method":"resources/templates/list","params":{}},
  {"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":"openpr://scenario-templates"}},
  {"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"projects.list","arguments":{}}},
]
payload="".join(json.dumps(row,separators=(",",":"))+"\n" for row in requests)
result=subprocess.run([binary,"serve","--config",config,"--transport","stdio","--api-url",f"http://127.0.0.1:{port}",
                       "--bot-token",token,"--workspace-id",workspace],input=payload,text=True,capture_output=True,check=False)
sys.stderr.write(result.stderr)
responses=[json.loads(line) for line in result.stdout.splitlines() if line.strip()]
sys.stderr.write(json.dumps(responses,sort_keys=True)+"\n")
by_id={row.get("id"):row for row in responses}
templates=by_id.get(2,{}).get("result",{}).get("resourceTemplates",[])
checks={
  "process_exit_zero": result.returncode == 0,
  "v08_server_identity": by_id.get(1,{}).get("result",{}).get("serverInfo",{}).get("name") == "openpr-mcp-server",
  "v08_openpr_registry": any(row.get("uriTemplate") == "openpr://scenario-templates/{key}" for row in templates),
  "openpr_resource_read": "error" not in by_id.get(3,{}) and bool(by_id.get(3,{}).get("result",{}).get("contents")),
  "old_projects_client": "error" not in by_id.get(4,{}) and not by_id.get(4,{}).get("result",{}).get("isError",False),
}
summary={"checks":checks,"response_count":len(responses),"response_sha256":__import__("hashlib").sha256(result.stdout.encode()).hexdigest(),
         "template_count":len(templates),"passed":all(checks.values())}
print(json.dumps(summary,sort_keys=True))
raise SystemExit(0 if summary["passed"] else 1)
PY
stop_api

V08_METADATA_BEFORE=$(psql "$UPGRADE_URL" -X -At -v ON_ERROR_STOP=1 -c "SELECT count(*) FROM information_schema.columns WHERE table_schema='public'")
start_api "$V08_TARGET/debug/api" "$CACHE_ROOT/v08.toml" "$V08_PORT" "$LOG_ROOT/v08-rollback-api.log"
ROLLBACK_PROJECT=$(api_get "$V08_PORT" "/api/v1/projects/$PROJECT_ID")
ROLLBACK_FORMS=$(api_get "$V08_PORT" "/api/v1/projects/$PROJECT_ID/forms")
jq -e --arg id "$PROJECT_ID" '.code == 0 and .data.id == $id and .data.name == "Upgrade Project"' <<<"$ROLLBACK_PROJECT" >/dev/null
jq -e --arg id "$FORM_ID" '.code == 0 and any(.data.items[]; .id == $id and .name == "Upgrade Form")' <<<"$ROLLBACK_FORMS" >/dev/null
V08_METADATA_AFTER=$(psql "$UPGRADE_URL" -X -At -v ON_ERROR_STOP=1 -c "SELECT count(*) FROM information_schema.columns WHERE table_schema='public'")
[[ $V08_METADATA_AFTER -eq $V08_METADATA_BEFORE ]]
snapshot_data "$CACHE_ROOT/after-rollback.json"
cmp "$CACHE_ROOT/after-upgrade.json" "$CACHE_ROOT/after-rollback.json"
stop_api

jq '.form.name="mutation-that-must-not-pass"' "$CACHE_ROOT/after-upgrade.json" >"$CACHE_ROOT/mutated-upgrade.json"
set +e
cmp "$CACHE_ROOT/before-upgrade.json" "$CACHE_ROOT/mutated-upgrade.json" >"$LOG_ROOT/snapshot-mutation.log" 2>&1
MUTATION_EXIT=$?
set -e
[[ $MUTATION_EXIT -ne 0 ]]

python3 - "$REPO_ROOT" "$EVIDENCE_ROOT" "$CACHE_ROOT" "$LOG_ROOT" "$V029_HEAD" "$V08_HEAD" \
  "$CLEAN_MIGRATIONS" "$V029_MIGRATIONS" "$CURRENT_MIGRATIONS" "$CLEAN_TABLES" "$V08_METADATA_BEFORE" "$V08_METADATA_AFTER" "$MUTATION_EXIT" <<'PY'
import datetime as dt, hashlib, json, os, pathlib, re, subprocess, sys, tempfile
repo,evidence,cache,logs=map(pathlib.Path,sys.argv[1:5])
v029_head,v08_head=sys.argv[5:7]
clean_migrations,v029_migrations,current_migrations,clean_tables,metadata_before,metadata_after,mutation_exit=map(int,sys.argv[7:14])
before=(cache/"before-upgrade.json").read_bytes(); after=(cache/"after-upgrade.json").read_bytes()
old_client=json.loads((cache/"old-client-summary.json").read_text())
workspace=re.search(r'\[workspace\.package\].*?\nversion\s*=\s*"([^"]+)"',(repo/"Cargo.toml").read_text(),re.S).group(1)
frontend=json.loads((repo/"frontend/package.json").read_text())["version"]
head=subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip()
generated=dt.datetime.now(dt.timezone.utc).isoformat()
log_hashes={path.name:hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(logs.glob("*.log"))}
install={
  "schema_version":"sylvode.flow.install-upgrade-result.v1","release":"0.9.0","source_head":head,"generated_at":generated,
  "clean_install":{"api_ready":True,"worker_running":True,"mcp_health":True,"web_health":True,
    "applied_migration_count":clean_migrations,"required_table_count":clean_tables,"required_table_expected":7},
  "in_place_upgrade":{"from_version":"0.2.9","from_head":v029_head,"before_migration_count":v029_migrations,
    "after_migration_count":current_migrations,"project_preserved_via_api":True,"form_preserved_via_api":True,
    "data_snapshot_sha256_before":hashlib.sha256(before).hexdigest(),"data_snapshot_sha256_after":hashlib.sha256(after).hexdigest(),
    "snapshot_exact_match":before==after,"flow_default_disabled_without_write":True,"flow_enabled_explicitly":True},
  "version_source":{"rust_workspace_version":workspace,"frontend_package_version":frontend,"aligned":workspace==frontend},
  "mutation_controls":[{"name":"changed_preserved_form","exit_code":mutation_exit,"detected":mutation_exit!=0}],
  "log_sha256":log_hashes,"executed_count":14,
}
install["passed"]=(install["clean_install"]["api_ready"] and clean_migrations>0 and clean_tables==7 and current_migrations>v029_migrations
  and before==after and workspace==frontend and mutation_exit!=0)
old={
  "schema_version":"sylvode.flow.old-client-result.v1","release":"0.9.0","source_head":head,"generated_at":generated,
  "rollback":{"to_release":"0.8","to_head":v08_head,"v08_api_ready":True,"project_preserved_via_api":True,
    "form_preserved_via_api":True,"metadata_columns_before":metadata_before,"metadata_columns_after":metadata_after,
    "no_contract_migration_removed":metadata_before==metadata_after,"data_snapshot_unchanged":(cache/"after-upgrade.json").read_bytes()==(cache/"after-rollback.json").read_bytes(),
    "migrated_rest_credential_on_original_surface":True},
  "old_mcp_client":old_client,"executed_count":len(old_client["checks"])+5,
}
old["passed"]=(old["rollback"]["no_contract_migration_removed"] and old["rollback"]["data_snapshot_unchanged"] and old_client["passed"])
for name,payload in (("install-upgrade-result.json",install),("old-client-result.json",old)):
  fd,tmp=tempfile.mkstemp(prefix=f".{name}.",dir=evidence)
  with os.fdopen(fd,"w") as handle: json.dump(payload,handle,sort_keys=True,indent=2); handle.write("\n")
  os.replace(tmp,evidence/name)
print(json.dumps({"install_upgrade":install,"old_client":old},sort_keys=True))
raise SystemExit(0 if install["passed"] and old["passed"] else 1)
PY
