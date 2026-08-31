#!/usr/bin/env bash
set -euo pipefail

# Live verifier for the two v0.4 Flow feature-flag hard gates. The frozen
# contract names an integration-test target that does not exist in this
# repository; this verifier records that mismatch and exercises the described
# semantics directly against shipped processes instead of treating the missing
# target as skipped or passed.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-}"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-feature-flags-v0.4.sh --json [OPTIONS]

Boots a real API, shipped MCP servers over HTTP/SSE/stdio, the shipped sylvode
CLI and a real Chromium/Vite frontend. Verifies feature read, admin set,
non-admin refusal, exact workspace scope, disabled navigation/direct URL and
Forms non-regression. Writes feature-flag-result.json.

Options:
  --database-url URL    PostgreSQL DSN on which a scratch database may be created.
                        Default: $OPENPR_TEST_DATABASE_URL
  --repo-root DIR       Default: this checkout.
  --contracts-root DIR  Default: /opt/working/sylvode-flow
  --evidence-root DIR   Default: /opt/working/sylvode-flow/evidence/v0.4
  --json                Required.

Exit codes: 0 both gates passed, 1 observed failure/environment unavailable,
2 usage or malformed verifier dependencies.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --database-url) DATABASE_URL="${2:?--database-url requires a URL}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done
: "$CONTRACTS_ROOT"

[[ $JSON_MODE -eq 1 ]] || { echo "FAIL: --json is required" >&2; exit 2; }
for tool in jq git python3 psql curl cargo sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required command: $tool" >&2; exit 2; }
done
git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1 || { echo "FAIL: invalid --repo-root: $REPO_ROOT" >&2; exit 2; }
PROBE="$ROOT_DIR/scripts/lib/mcp_transport_probe.py"
UI_PROBE="$ROOT_DIR/scripts/lib/flow_feature_ui_probe.py"
REVERSE_PROXY="$ROOT_DIR/scripts/lib/flow_feature_reverse_proxy.py"
SCHEMA="$ROOT_DIR/docs/schemas/sylvode-flow-feature-flag-result-v1.schema.json"
[[ -f "$PROBE" && -f "$UI_PROBE" && -f "$REVERSE_PROXY" ]] || { echo "FAIL: verifier helper missing" >&2; exit 2; }
mkdir -p "$EVIDENCE_ROOT"

SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
OUT_PATH="$EVIDENCE_ROOT/feature-flag-result.json"

metadata_mismatch() {
  local targets target_exists
  targets="$(cargo metadata --manifest-path "$REPO_ROOT/Cargo.toml" --no-deps --format-version 1 2>/dev/null | jq -c '[.packages[] | select(.name=="mcp-server") | .targets[] | select(.kind|index("test")) | .name] | unique | sort')"
  if jq -e 'index("flow_feature_legacy_admin_e2e") != null' >/dev/null <<<"$targets"; then target_exists=true; else target_exists=false; fi
  jq -c -n --arg command "cargo test -p mcp-server --test flow_feature_legacy_admin_e2e" \
    --argjson target_exists "$target_exists" --argjson discovered "$targets" \
    '{contract_command:$command,expected_target:"flow_feature_legacy_admin_e2e",target_exists:$target_exists,discovered_integration_test_targets:$discovered,resolution:"execute the frozen semantic requirements through live shipped processes; never count the missing target as skipped or passed"}'
}
MISMATCH_JSON="$(metadata_mismatch)"

write_environment_failure() {
  local reason="$1" result tmp
  result="$(jq -n --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" --arg reason "$reason" \
    --argjson mismatch "$MISMATCH_JSON" '{
      schema_version:"sylvode.flow.feature-flag-result.v1",source_head:$head,generated_at:$generated_at,
      contract_command_mismatch:$mismatch,environment:{reachable:false,reason:$reason},
      transports:["http","sse","stdio"],observations:{},negative_controls:{},violations:[$reason],
      gates:{
        feature_flag_navigation_and_direct_url:{status:"failed",reason:$reason},
        feature_flag_mcp_read_admin_write_and_cli_equivalence:{status:"failed",reason:$reason}
      },passed:false
    }')"
  tmp="$OUT_PATH.tmp"
  printf '%s\n' "$result" | jq . > "$tmp"
  mv -f "$tmp" "$OUT_PATH"
  printf '%s\n' "$result"
  exit 1
}

[[ -n "$DATABASE_URL" ]] || write_environment_failure "no database URL configured"
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atc 'SELECT 1' >/dev/null 2>&1 || \
  write_environment_failure "configured PostgreSQL environment is unreachable"
python3 -c 'from playwright.sync_api import sync_playwright' >/dev/null 2>&1 || \
  write_environment_failure "Python Playwright is unavailable"
[[ -x /usr/bin/chromium ]] || write_environment_failure "Chromium is unavailable at /usr/bin/chromium"

TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
echo "=== building api, mcp-server and sylvode ===" >&2
if ! (cd "$REPO_ROOT" && cargo build -q -p api --bin api -p mcp-server --bin mcp-server --bin sylvode); then
  write_environment_failure "shipped feature-surface binaries did not build"
fi
API_BIN="$TARGET_DIR/debug/api"
MCP_BIN="$TARGET_DIR/debug/mcp-server"
CLI_BIN="$TARGET_DIR/debug/sylvode"
[[ -x "$API_BIN" && -x "$MCP_BIN" && -x "$CLI_BIN" ]] || write_environment_failure "one or more shipped binaries are missing"

RUN_ID="$(python3 -c 'import uuid; print(uuid.uuid4().hex[:8])')"
SCRATCH_DB="openpr_flow_feature_verify_$RUN_ID"
DB_PREFIX="${DATABASE_URL%/*}"
SCRATCH_URL="$DB_PREFIX/$SCRATCH_DB"
TMP_DIR="$(mktemp -d /tmp/openpr-flow-feature-verify.XXXXXX)"
API_PORT=$((21000 + RANDOM % 5000))
VITE_PORT=$((31000 + RANDOM % 5000))
PUBLIC_PORT=$((47000 + RANDOM % 5000))
API_PID=""; VITE_PID=""; PROXY_PID=""; MCP_PID=""

# shellcheck disable=SC2317  # invoked by the EXIT trap
cleanup() {
  local ec=$?
  for pid in "$MCP_PID" "$PROXY_PID" "$VITE_PID" "$API_PID"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  done
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "DROP DATABASE IF EXISTS \"$SCRATCH_DB\" WITH (FORCE)" >/dev/null 2>&1 || true
  rm -rf "$TMP_DIR"
  exit "$ec"
}
trap cleanup EXIT

psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "DROP DATABASE IF EXISTS \"$SCRATCH_DB\" WITH (FORCE)" >/dev/null 2>&1 || true
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "CREATE DATABASE \"$SCRATCH_DB\"" >/dev/null || write_environment_failure "could not create scratch database"

JWT_SECRET="flow-feature-verify-not-a-real-secret"
API_CONFIG="$TMP_DIR/openpr-api.toml"
cat > "$API_CONFIG" <<EOF
[server]
app_name = "api"
bind_addr = "127.0.0.1:$API_PORT"
[database]
url = "$SCRATCH_URL"
[auth]
jwt_secret = "$JWT_SECRET"
[logging]
filter = "api=warn,openpr=warn"
format = "text"
EOF
"$API_BIN" --config "$API_CONFIG" >"$TMP_DIR/api.log" 2>&1 & API_PID=$!
for _ in $(seq 1 120); do
  curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1 && break
  kill -0 "$API_PID" 2>/dev/null || write_environment_failure "live API process exited during startup"
  sleep 0.5
done
curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1 || write_environment_failure "live API did not become healthy"

API="http://127.0.0.1:$API_PORT"
WORKSPACE_A="$(python3 -c 'import uuid; print(uuid.uuid4())')"
WORKSPACE_B="$(python3 -c 'import uuid; print(uuid.uuid4())')"
OWNER_USER="$(python3 -c 'import uuid; print(uuid.uuid4())')"
MEMBER_USER="$(python3 -c 'import uuid; print(uuid.uuid4())')"
ADMIN_BOT="$(python3 -c 'import uuid; print(uuid.uuid4())')"
MEMBER_BOT="$(python3 -c 'import uuid; print(uuid.uuid4())')"
ADMIN_TOKEN="opr_feature_admin_$RUN_ID"
MEMBER_TOKEN="opr_feature_member_$RUN_ID"
ADMIN_HASH="$(printf '%s' "$ADMIN_TOKEN" | sha256sum | awk '{print $1}')"
MEMBER_HASH="$(printf '%s' "$MEMBER_TOKEN" | sha256sum | awk '{print $1}')"

psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id,email,password_hash,name,role,is_active,entity_type,created_at,updated_at) VALUES
('$OWNER_USER','owner-$RUN_ID@example.local','','Owner','user',true,'human',now(),now()),
('$MEMBER_USER','member-$RUN_ID@example.local','','Member','user',true,'human',now(),now());
INSERT INTO users (id,email,password_hash,name,role,is_active,entity_type,agent_type,created_at,updated_at) VALUES
('$ADMIN_BOT','$ADMIN_BOT@bot.openpr.local','!','Admin Bot','user',true,'bot_mcp','mcp',now(),now()),
('$MEMBER_BOT','$MEMBER_BOT@bot.openpr.local','!','Member Bot','user',true,'bot_mcp','mcp',now(),now());
INSERT INTO workspaces (id,slug,name,created_by,created_at,updated_at) VALUES
('$WORKSPACE_A','feature-a-$RUN_ID','Feature A','$OWNER_USER',now(),now()),
('$WORKSPACE_B','feature-b-$RUN_ID','Feature B','$OWNER_USER',now(),now());
INSERT INTO workspace_members (workspace_id,user_id,role,created_at) VALUES
('$WORKSPACE_A','$OWNER_USER','owner',now()),('$WORKSPACE_B','$OWNER_USER','owner',now()),
('$WORKSPACE_A','$MEMBER_USER','member',now()),('$WORKSPACE_A','$ADMIN_BOT','admin',now()),
('$WORKSPACE_A','$MEMBER_BOT','member',now());
INSERT INTO flow_workspace_settings (workspace_id,flow_enabled,default_member_level,authz_epoch,updated_at) VALUES
('$WORKSPACE_A',false,'edit',0,now()),('$WORKSPACE_B',false,'edit',0,now());
INSERT INTO workspace_bots (id,workspace_id,name,token_hash,token_prefix,permissions,created_by,is_active,created_at,updated_at) VALUES
('$ADMIN_BOT','$WORKSPACE_A','Admin Bot','$ADMIN_HASH','${ADMIN_TOKEN:0:8}','["read","write","admin"]'::jsonb,'$OWNER_USER',true,now(),now()),
('$MEMBER_BOT','$WORKSPACE_A','Member Bot','$MEMBER_HASH','${MEMBER_TOKEN:0:8}','["read","write"]'::jsonb,'$OWNER_USER',true,now(),now());
SQL

make_jwt() {
  python3 - "$JWT_SECRET" "$1" "$2" <<'PY'
import base64,hashlib,hmac,json,sys,time
def b64(v): return base64.urlsafe_b64encode(json.dumps(v,separators=(",",":")).encode()).rstrip(b"=").decode()
secret,sub,email=sys.argv[1:]
now=int(time.time()); head=b64({"alg":"HS256","typ":"JWT"}); body=b64({"sub":sub,"email":email,"token_type":"access","iat":now,"exp":now+3600})
sig=base64.urlsafe_b64encode(hmac.new(secret.encode(),f"{head}.{body}".encode(),hashlib.sha256).digest()).rstrip(b"=").decode()
print(f"{head}.{body}.{sig}")
PY
}
OWNER_JWT="$(make_jwt "$OWNER_USER" "owner-$RUN_ID@example.local")"
MEMBER_JWT="$(make_jwt "$MEMBER_USER" "member-$RUN_ID@example.local")"

json_field() { jq -r "$2" <<<"$1" 2>/dev/null || true; }
rest() { local token="$1" method="$2" path="$3" body="${4:-}"; if [[ -n "$body" ]]; then curl -sS -X "$method" "$API$path" -H "Authorization: Bearer $token" -H 'Content-Type: application/json' -d "$body"; else curl -sS -X "$method" "$API$path" -H "Authorization: Bearer $token" -H 'Content-Type: application/json'; fi; }

VIOLATIONS='[]'
add_violation() { VIOLATIONS="$(jq -c --arg v "$1" '. + [$v]' <<<"$VIOLATIONS")"; }

# Direct REST controls: read is member-visible, write is admin-only, and setting A never mutates B.
REST_MEMBER_READ="$(rest "$MEMBER_JWT" GET "/api/v1/workspaces/$WORKSPACE_A/features/flow")"
[[ "$(json_field "$REST_MEMBER_READ" '.code')" == 0 && "$(json_field "$REST_MEMBER_READ" '.data.flow_enabled')" == false ]] || add_violation "REST member feature read failed"
REST_MEMBER_DENY="$(rest "$MEMBER_JWT" PUT "/api/v1/workspaces/$WORKSPACE_A/features/flow" "$(jq -n '{enabled:true,idempotency_key:"rest-member-deny"}')")"
[[ "$(json_field "$REST_MEMBER_DENY" '.code')" == 403 ]] || add_violation "REST non-admin feature set was not refused with code 403"
REST_ADMIN_SET="$(rest "$OWNER_JWT" PUT "/api/v1/workspaces/$WORKSPACE_A/features/flow" "$(jq -n '{enabled:true,idempotency_key:"rest-admin-set"}')")"
[[ "$(json_field "$REST_ADMIN_SET" '.code')" == 0 ]] || add_violation "REST admin feature set failed"
REST_B_READ="$(rest "$OWNER_JWT" GET "/api/v1/workspaces/$WORKSPACE_B/features/flow")"
[[ "$(json_field "$REST_B_READ" '.data.flow_enabled')" == false ]] || add_violation "REST feature set leaked from workspace A into B"

write_mcp_config() {
  local path="$1" token="$2" transport="$3" bind="$4"
  cat > "$path" <<EOF
[database]
url = "postgres://unused:unused@127.0.0.1:5432/unused"
[auth]
jwt_secret = "unused-by-feature-verifier"
[logging]
filter = "error"
format = "text"
[mcp]
api_url = "$API"
bot_token = "$token"
workspace_id = "$WORKSPACE_A"
transport = "$transport"
$( [[ -n "$bind" ]] && printf 'bind_addr = "%s"' "$bind" )
EOF
}

wait_port() { local port="$1" pid="$2"; for _ in $(seq 1 80); do (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null && { exec 3<&- 3>&-; return 0; }; kill -0 "$pid" 2>/dev/null || return 1; sleep 0.25; done; return 1; }

run_mcp_probe() {
  local identity="$1" transport="$2" token="$3" requests="$4" port config
  port=$((36000 + RANDOM % 10000)); config="$TMP_DIR/mcp-$identity-$transport.toml"
  if [[ "$transport" == stdio ]]; then
    write_mcp_config "$config" "$token" stdio ""
    python3 "$PROBE" --transport stdio --requests "$requests" --timeout 30 --binary "$MCP_BIN" --config "$config" 2>/dev/null
    return
  fi
  write_mcp_config "$config" "$token" "$transport" "127.0.0.1:$port"
  "$MCP_BIN" --config "$config" serve --transport "$transport" >"$TMP_DIR/mcp-$identity-$transport.log" 2>&1 & MCP_PID=$!
  if ! wait_port "$port" "$MCP_PID"; then
    jq -n -c --arg t "$transport" '{transport:$t,responses:[],errors:["server did not accept a connection"]}'
  elif [[ "$transport" == http ]]; then
    python3 "$PROBE" --transport http --requests "$requests" --timeout 30 --host 127.0.0.1 --port "$port" --token "$token" 2>/dev/null
  else
    python3 "$PROBE" --transport sse --requests "$requests" --timeout 30 --host 127.0.0.1 --port "$port" --token "$token" 2>/dev/null
  fi
  kill "$MCP_PID" 2>/dev/null || true; wait "$MCP_PID" 2>/dev/null || true; MCP_PID=""
}

MCP_RESULTS='{}'
for transport in http sse stdio; do
  psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -q -c "UPDATE flow_workspace_settings SET flow_enabled=false WHERE workspace_id IN ('$WORKSPACE_A','$WORKSPACE_B')" >/dev/null
  ADMIN_REQUESTS="$(jq -c -n --arg a "$WORKSPACE_A" --arg b "$WORKSPACE_B" --arg key "mcp-$transport-admin-set" '[
    {jsonrpc:"2.0",id:1,method:"initialize",params:{protocolVersion:"2024-11-05",capabilities:{},clientInfo:{name:"feature-verifier",version:"1"}}},
    {jsonrpc:"2.0",id:2,method:"tools/call",params:{name:"flow.feature_get",arguments:{workspace_id:$a}}},
    {jsonrpc:"2.0",id:3,method:"tools/call",params:{name:"flow.feature_set",arguments:{workspace_id:$a,enabled:true,idempotency_key:$key}}},
    {jsonrpc:"2.0",id:4,method:"tools/call",params:{name:"flow.feature_get",arguments:{workspace_id:$a}}},
    {jsonrpc:"2.0",id:5,method:"tools/call",params:{name:"flow.feature_get",arguments:{workspace_id:$b}}},
    {jsonrpc:"2.0",id:6,method:"tools/call",params:{name:"flow.feature_set",arguments:{workspace_id:$b,enabled:true,idempotency_key:("foreign-"+$key)}}}
  ]')"
  ADMIN_RESULT="$(run_mcp_probe admin "$transport" "$ADMIN_TOKEN" "$ADMIN_REQUESTS" || jq -n -c --arg t "$transport" '{transport:$t,responses:[],errors:["probe crashed"]}')"
  MEMBER_REQUESTS="$(jq -c -n --arg a "$WORKSPACE_A" --arg key "mcp-$transport-member-deny" '[
    {jsonrpc:"2.0",id:1,method:"initialize",params:{protocolVersion:"2024-11-05",capabilities:{},clientInfo:{name:"feature-verifier",version:"1"}}},
    {jsonrpc:"2.0",id:2,method:"tools/call",params:{name:"flow.feature_get",arguments:{workspace_id:$a}}},
    {jsonrpc:"2.0",id:3,method:"tools/call",params:{name:"flow.feature_set",arguments:{workspace_id:$a,enabled:false,idempotency_key:$key}}},
    {jsonrpc:"2.0",id:4,method:"tools/call",params:{name:"flow.feature_get",arguments:{workspace_id:$a}}}
  ]')"
  MEMBER_RESULT="$(run_mcp_probe member "$transport" "$MEMBER_TOKEN" "$MEMBER_REQUESTS" || jq -n -c --arg t "$transport" '{transport:$t,responses:[],errors:["probe crashed"]}')"
  MCP_RESULTS="$(jq -c --arg t "$transport" --argjson admin "$ADMIN_RESULT" --argjson member "$MEMBER_RESULT" '.[$t]={admin:$admin,member:$member}' <<<"$MCP_RESULTS")"
done

MCP_B_ENABLED="$(psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -Atc "SELECT flow_enabled FROM flow_workspace_settings WHERE workspace_id='$WORKSPACE_B'")"
[[ "$MCP_B_ENABLED" == f ]] || add_violation "MCP foreign-workspace attempts changed workspace B"

MCP_COMPARE="$(python3 - "$MCP_RESULTS" <<'PY'
import json,sys
d=json.loads(sys.argv[1]); violations=[]; summary={}
def response(run,rid): return next((r for r in run.get("responses",[]) if r.get("id")==rid),None)
def is_error(r):
 if not r: return None
 if "error" in r: return True
 return bool((r.get("result") or {}).get("isError",False))
def text(r):
 out=[]
 for p in ((r or {}).get("result") or {}).get("content",[]) or []:
  if isinstance(p,dict) and isinstance(p.get("text"),str): out.append(p["text"])
 return "\n".join(out)
for t in ("http","sse","stdio"):
 a=d.get(t,{}).get("admin",{}); m=d.get(t,{}).get("member",{})
 if a.get("errors"): violations.append(f"{t} admin transport errors: {a['errors']}")
 if m.get("errors"): violations.append(f"{t} member transport errors: {m['errors']}")
 checks={
  "initial_read":is_error(response(a,2)) is False and '"flow_enabled": false' in text(response(a,2)),
  "admin_set":is_error(response(a,3)) is False,
  "post_set_read":is_error(response(a,4)) is False and '"flow_enabled": true' in text(response(a,4)),
  "foreign_read_denied":is_error(response(a,5)) is True,
  "foreign_set_denied":is_error(response(a,6)) is True,
  "member_read":is_error(response(m,2)) is False and '"flow_enabled": true' in text(response(m,2)),
  "member_set_denied":is_error(response(m,3)) is True,
  "member_denial_no_mutation":is_error(response(m,4)) is False and '"flow_enabled": true' in text(response(m,4)),
 }
 summary[t]=checks
 for k,v in checks.items():
  if not v: violations.append(f"{t}: {k} failed")
print(json.dumps({"summary":summary,"violations":violations},separators=(",",":")))
PY
)"
while IFS= read -r violation; do [[ -n "$violation" ]] && add_violation "$violation"; done < <(jq -r '.violations[]' <<<"$MCP_COMPARE")

# Shipped CLI equivalence and negative controls.
psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -q -c "UPDATE flow_workspace_settings SET flow_enabled=false WHERE workspace_id IN ('$WORKSPACE_A','$WORKSPACE_B')" >/dev/null
CLI_CONFIG="$TMP_DIR/mcp-admin-http.toml"
run_cli() { set +e; CLI_OUTPUT="$("$CLI_BIN" --config "$CLI_CONFIG" --api-url "$API" --bot-token "$1" --format json "${@:2}" 2>&1)"; CLI_EXIT=$?; set -e; }
run_cli "$ADMIN_TOKEN" features flow get --workspace "$WORKSPACE_A"; CLI_GET_BEFORE="$CLI_OUTPUT"; CLI_GET_BEFORE_EXIT=$CLI_EXIT
run_cli "$ADMIN_TOKEN" features flow set --workspace "$WORKSPACE_A" --enabled true --idempotency-key cli-admin-set; CLI_SET="$CLI_OUTPUT"; CLI_SET_EXIT=$CLI_EXIT
run_cli "$ADMIN_TOKEN" features flow get --workspace "$WORKSPACE_A"; CLI_GET_AFTER="$CLI_OUTPUT"; CLI_GET_AFTER_EXIT=$CLI_EXIT
run_cli "$MEMBER_TOKEN" features flow set --workspace "$WORKSPACE_A" --enabled false --idempotency-key cli-member-deny; CLI_DENY="$CLI_OUTPUT"; CLI_DENY_EXIT=$CLI_EXIT
run_cli "$ADMIN_TOKEN" features flow get --workspace "$WORKSPACE_B"; CLI_B="$CLI_OUTPUT"; CLI_B_EXIT=$CLI_EXIT
[[ $CLI_GET_BEFORE_EXIT -eq 0 && "$(jq -r '.data.flow_enabled' <<<"$CLI_GET_BEFORE" 2>/dev/null)" == false ]] || add_violation "CLI initial feature read failed"
[[ $CLI_SET_EXIT -eq 0 && $CLI_GET_AFTER_EXIT -eq 0 && "$(jq -r '.data.flow_enabled' <<<"$CLI_GET_AFTER" 2>/dev/null)" == true ]] || add_violation "CLI admin set/read equivalence failed"
[[ $CLI_DENY_EXIT -eq 4 ]] || add_violation "CLI non-admin set did not exit 4"
[[ $CLI_B_EXIT -ne 0 || "$(jq -r '.data.flow_enabled // empty' <<<"$CLI_B" 2>/dev/null)" == false ]] || add_violation "CLI workspace A set changed workspace B"

# Create one real project so Forms has a positive, empty-list live control.
PROJECT_RESP="$(rest "$OWNER_JWT" POST "/api/v1/workspaces/$WORKSPACE_A/projects" "$(jq -n --arg key "FF${RUN_ID^^}" '{key:$key,name:"Feature Flag Forms Control",description:"live verifier"}')")"
PROJECT_ID="$(json_field "$PROJECT_RESP" '.data.id')"
[[ -n "$PROJECT_ID" && "$PROJECT_ID" != null ]] || add_violation "could not create the Forms live-control project: $PROJECT_RESP"
OBJECT_RESP="$(rest "$OWNER_JWT" POST "/api/v1/workspaces/$WORKSPACE_A/flow/objects" "$(jq -n '{object_type:"page",title:"Feature UI control",idempotency_key:"feature-ui-object"}')")"
OBJECT_ID="$(json_field "$OBJECT_RESP" '.data.id // .data.object.id')"
[[ -n "$OBJECT_ID" && "$OBJECT_ID" != null ]] || add_violation "could not create the enabled direct-URL positive-control object: $OBJECT_RESP"

UI_JSON='{"observations":{},"violations":["UI probe was not run"]}'
VITE_REACHABLE=false
UI_PROBE_RAN=false
if [[ -n "$PROJECT_ID" && "$PROJECT_ID" != null && -n "$OBJECT_ID" && "$OBJECT_ID" != null ]]; then
  echo "=== starting live Vite frontend ===" >&2
  PATH="/home/ck/.bun/bin:$PATH" VITE_API_BASE_URL="" bun run --cwd "$REPO_ROOT/frontend" dev -- --host 127.0.0.1 --port "$VITE_PORT" >"$TMP_DIR/vite.log" 2>&1 & VITE_PID=$!
  for _ in $(seq 1 120); do curl -fsS "http://127.0.0.1:$VITE_PORT/" >/dev/null 2>&1 && break; kill -0 "$VITE_PID" 2>/dev/null || break; sleep 0.5; done
  if curl -fsS "http://127.0.0.1:$VITE_PORT/" >/dev/null 2>&1; then
    VITE_REACHABLE=true
    python3 "$REVERSE_PROXY" --bind "127.0.0.1:$PUBLIC_PORT" --api-upstream "$API" --frontend-upstream "http://127.0.0.1:$VITE_PORT" >"$TMP_DIR/proxy.log" 2>&1 & PROXY_PID=$!
    for _ in $(seq 1 80); do curl -fsS "http://127.0.0.1:$PUBLIC_PORT/" >/dev/null 2>&1 && break; kill -0 "$PROXY_PID" 2>/dev/null || break; sleep 0.25; done
    if curl -fsS "http://127.0.0.1:$PUBLIC_PORT/" >/dev/null 2>&1; then
      set +e
      UI_JSON="$(python3 "$UI_PROBE" --frontend-url "http://127.0.0.1:$PUBLIC_PORT" --api-url "http://127.0.0.1:$PUBLIC_PORT" --workspace-id "$WORKSPACE_A" --project-id "$PROJECT_ID" --object-id "$OBJECT_ID" --token "$OWNER_JWT" 2>"$TMP_DIR/ui-probe.log")"
      UI_EXIT=$?
      set -e
      UI_PROBE_RAN=true
      if ! jq -e . >/dev/null 2>&1 <<<"$UI_JSON"; then UI_JSON="$(jq -n -c --arg log "$(tail -30 "$TMP_DIR/ui-probe.log" | tr '\n' ' ')" '{observations:{},violations:[("UI probe produced invalid JSON: "+$log)]}')"; UI_EXIT=1; fi
      [[ $UI_EXIT -eq 0 ]] || true
    else
      UI_JSON='{"observations":{},"violations":["same-origin browser reverse proxy was unreachable"]}'
    fi
  else
    UI_JSON='{"observations":{},"violations":["live Vite frontend was unreachable"]}'
  fi
fi
while IFS= read -r violation; do [[ -n "$violation" ]] && add_violation "$violation"; done < <(jq -r '.violations[]' <<<"$UI_JSON")

NAV_VIOLATIONS="$(jq -c '[.[] | select(test("navigation|direct URL|Forms|frontend|Vite|\\bUI\\b";"i"))]' <<<"$VIOLATIONS")"
SURFACE_VIOLATIONS="$(jq -c '[.[] | select(test("navigation|direct URL|Forms|frontend|Vite|\\bUI\\b";"i")|not)]' <<<"$VIOLATIONS")"
NAV_STATUS="$([[ "$(jq 'length' <<<"$NAV_VIOLATIONS")" -eq 0 ]] && echo passed || echo failed)"
SURFACE_STATUS="$([[ "$(jq 'length' <<<"$SURFACE_VIOLATIONS")" -eq 0 ]] && echo passed || echo failed)"
PASSED="$([[ "$NAV_STATUS" == passed && "$SURFACE_STATUS" == passed ]] && echo true || echo false)"
REST_DENIED_BOOL="$([[ "$(json_field "$REST_MEMBER_DENY" '.code')" == 403 ]] && echo true || echo false)"
FOREIGN_UNCHANGED_BOOL="$([[ "$(json_field "$REST_B_READ" '.data.flow_enabled')" == false && "$MCP_B_ENABLED" == f ]] && echo true || echo false)"
MCP_MEMBER_DENIALS="$(jq -c '.summary | with_entries(.value = .value.member_set_denied)' <<<"$MCP_COMPARE")"

RESULT="$(jq -n --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" --arg nav_status "$NAV_STATUS" --arg surface_status "$SURFACE_STATUS" \
  --argjson mismatch "$MISMATCH_JSON" --argjson rest_member_read "$REST_MEMBER_READ" --argjson rest_member_deny "$REST_MEMBER_DENY" \
  --argjson rest_admin_set "$REST_ADMIN_SET" --argjson rest_b_read "$REST_B_READ" --argjson mcp "$MCP_RESULTS" --argjson mcp_compare "$MCP_COMPARE" \
  --arg cli_get_before "$CLI_GET_BEFORE" --arg cli_set "$CLI_SET" --arg cli_get_after "$CLI_GET_AFTER" --arg cli_deny "$CLI_DENY" --arg cli_b "$CLI_B" \
  --argjson cli_get_before_exit "$CLI_GET_BEFORE_EXIT" --argjson cli_set_exit "$CLI_SET_EXIT" --argjson cli_get_after_exit "$CLI_GET_AFTER_EXIT" \
  --argjson cli_deny_exit "$CLI_DENY_EXIT" --argjson cli_b_exit "$CLI_B_EXIT" --argjson ui "$UI_JSON" --argjson violations "$VIOLATIONS" \
  --argjson nav_violations "$NAV_VIOLATIONS" --argjson surface_violations "$SURFACE_VIOLATIONS" --argjson rest_denied "$REST_DENIED_BOOL" \
  --argjson foreign_unchanged "$FOREIGN_UNCHANGED_BOOL" --argjson mcp_member_denials "$MCP_MEMBER_DENIALS" \
  --argjson vite_reachable "$VITE_REACHABLE" --argjson ui_probe_ran "$UI_PROBE_RAN" --argjson passed "$PASSED" '{
    schema_version:"sylvode.flow.feature-flag-result.v1",source_head:$head,generated_at:$generated_at,
    contract_command_mismatch:$mismatch,environment:{reachable:true,api_process:true,mcp_processes:true,cli_process:true,vite_process:$vite_reachable,browser_probe:$ui_probe_ran,browser:"chromium"},
    transports:["http","sse","stdio"],
    observations:{rest:{member_read:$rest_member_read,member_set_denied:$rest_member_deny,admin_set:$rest_admin_set,other_workspace_after_set:$rest_b_read},mcp:{raw:$mcp,comparison:$mcp_compare},cli:{get_before:{exit_code:$cli_get_before_exit,output:$cli_get_before},admin_set:{exit_code:$cli_set_exit,output:$cli_set},get_after:{exit_code:$cli_get_after_exit,output:$cli_get_after},member_set_denied:{exit_code:$cli_deny_exit,output:$cli_deny},other_workspace:{exit_code:$cli_b_exit,output:$cli_b}},ui:$ui},
    negative_controls:{rest_non_admin_denied:$rest_denied,mcp_non_admin_denied_by_transport:$mcp_member_denials,cli_non_admin_exit:$cli_deny_exit,foreign_workspace_unchanged:$foreign_unchanged,disabled_ui:$ui.observations.disabled,forms_unchanged:$ui.observations.forms.identical_across_toggle},
    violations:$violations,
    gates:{
      feature_flag_navigation_and_direct_url:{status:$nav_status,reason:(if ($nav_violations|length)==0 then "live Chromium observed enabled navigation/direct-route positive controls, disabled navigation/direct-route refusals and unchanged live Forms behavior" else ($nav_violations|join("; ")) end)},
      feature_flag_mcp_read_admin_write_and_cli_equivalence:{status:$surface_status,reason:(if ($surface_violations|length)==0 then "real HTTP/SSE/stdio MCP calls plus REST and shipped CLI proved read, admin set, non-admin denial and exact workspace scope" else ($surface_violations|join("; ")) end)}
    },passed:$passed
  }')"

OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$RESULT" | jq . > "$OUT_TMP"
if [[ -f "$SCHEMA" ]]; then
  jq -e . "$SCHEMA" >/dev/null || { echo "FAIL: invalid JSON schema: $SCHEMA" >&2; rm -f "$OUT_TMP"; exit 2; }
  jq -e '
    .schema_version == "sylvode.flow.feature-flag-result.v1" and
    (.source_head | test("^[0-9a-f]{40}$")) and
    .contract_command_mismatch.target_exists == false and
    .transports == ["http","sse","stdio"] and
    ([.gates[].status] | all(. == "passed" or . == "failed")) and
    (if .passed then ((.violations|length)==0 and ([.gates[].status]|all(.=="passed"))) else true end)
  ' "$OUT_TMP" >/dev/null || { echo "FAIL: generated feature evidence violates its required schema invariants" >&2; rm -f "$OUT_TMP"; exit 2; }
fi
mv -f "$OUT_TMP" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2
printf '%s\n' "$RESULT"
[[ "$PASSED" == true ]] && exit 0
exit 1
