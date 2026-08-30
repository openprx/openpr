#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 MCP three-transport contract verifier.
#
# Contract: /opt/working/sylvode-flow/contracts/mcp-surface-v1.md
# ("HTTP/stdio/SSE"; "Transport -> actor/origin"; "同一 semantic request 跨
# HTTP/stdio/SSE 不得改变 primary event type"; "只让 `tools/list` 出现名称而调用
# 返回 unknown/未授权假成功，gate 必须失败"), versions/v0.4-flow-alpha.md
# ("逐工具 PolicyScope、admin/migration policy、projectless 降级和三 transport
# 测试") and gates/v0.4-gate.yaml's `mcp_three_transport_contract`.
#
# This verifier reads NOTHING from a markdown surface table. It boots the real
# `api` binary against a scratch database, seeds a workspace with a real Flow
# page, then starts the shipped `mcp-server` binary three times -- once per
# transport -- and speaks real JSON-RPC to each:
#
#   HTTP   POST /mcp/rpc, bearer token per request
#   SSE    GET /sse for the stream, POST /messages for the call, result read
#          back off the stream (202 acceptance asserted)
#   stdio  the binary spawned with `serve --transport stdio`, newline-delimited
#          JSON-RPC on its stdin/stdout
#
# Asserted across the three:
#   T1  every transport comes up and answers `initialize`. A transport that
#       cannot be started is recorded as failed with its reason -- never
#       skipped, never assumed equal to the others.
#   T2  `tools/list` returns the SAME tool-name set on all three (compared as
#       a set and by sorted-name SHA-256), and the same count.
#   T3  the per-tool input schemas hash identically across the three -- a
#       transport that advertises a name but a different schema is a
#       divergence `tools/list` name comparison alone would miss.
#   T4  a real `tools/call` of a Flow read tool (`objects.get`) returns
#       equivalent content on all three. This is the assertion mcp-surface-v1.md
#       demands: a name in `tools/list` that answers "unknown tool" or a
#       fake success when actually called must fail the gate.
#   T5  the Flow tools v0.4 registers are present on all three.
#   T6  a negative case behaves the same on all three: calling a tool with an
#       object id from ANOTHER workspace is refused everywhere (`isError`),
#       with no transport leaking a success.
#   T7  an unregistered tool name is refused on all three.
#
# Exit codes: 0 = all three transports agree on every assertion, 1 = a
# divergence or a transport that would not start, 2 = usage/tool/environment
# error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-}"
TRANSPORTS="http,sse,stdio"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-mcp-transports-v0.4.sh --transports http,sse,stdio --json [OPTIONS]

Boots the real api binary and the shipped mcp-server binary once per transport,
speaks real JSON-RPC to each, and compares tools/list, per-tool schemas, a real
Flow tools/call, a cross-workspace refusal and an unknown-tool refusal across
all three. Writes evidence/v0.4/mcp-contract-result.json.

Options:
  --transports LIST      Comma-separated; must be exactly http,sse,stdio.
  --database-url URL     Postgres DSN this script may CREATE DATABASE on.
                         Default: $OPENPR_TEST_DATABASE_URL
  --repo-root DIR        Default: this checkout.
  --contracts-root DIR   Default: /opt/working/sylvode-flow
  --evidence-root DIR    Default: /opt/working/sylvode-flow/evidence/v0.4
  --json                 Required for CLI-contract compatibility.
  -h, --help             Show this help and exit 0.

Exit codes: 0 all three agree, 1 a divergence or a dead transport,
2 usage/tool/environment error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --transports) TRANSPORTS="${2:?--transports requires a value}"; shift 2 ;;
    --database-url) DATABASE_URL="${2:?--database-url requires a value}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done
: "$CONTRACTS_ROOT"

if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
if [[ "$TRANSPORTS" != "http,sse,stdio" ]]; then
  echo "FAIL: --transports must be exactly http,sse,stdio (the contract fixes all three)" >&2
  exit 2
fi
if [[ -z "$DATABASE_URL" ]]; then
  echo "FAIL: no database URL configured (set --database-url or OPENPR_TEST_DATABASE_URL)" >&2
  exit 2
fi
for tool in jq git python3 psql curl cargo sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required command: $tool" >&2; exit 2; }
done
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi
PROBE="$ROOT_DIR/scripts/lib/mcp_transport_probe.py"
[[ -f "$PROBE" ]] || { echo "FAIL: transport probe helper not found: $PROBE" >&2; exit 2; }
if ! psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atc "SELECT 1" >/dev/null 2>&1; then
  echo "FAIL: database is not reachable: $DATABASE_URL" >&2
  exit 2
fi

mkdir -p "$EVIDENCE_ROOT"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
echo "=== building api and mcp-server ===" >&2
( cd "$REPO_ROOT" && cargo build -q -p api --bin api -p mcp-server --bin mcp-server ) || {
  echo "FAIL: binaries failed to build" >&2; exit 2; }
API_BIN="$TARGET_DIR/debug/api"
MCP_BIN="$TARGET_DIR/debug/mcp-server"
[[ -x "$API_BIN" ]] || { echo "FAIL: api binary not found: $API_BIN" >&2; exit 2; }
[[ -x "$MCP_BIN" ]] || { echo "FAIL: mcp-server binary not found: $MCP_BIN" >&2; exit 2; }

RUN_ID="$(python3 -c 'import uuid; print(uuid.uuid4().hex[:8])')"
SCRATCH_DB="openpr_flow_mcp_verify_$RUN_ID"
DB_PREFIX="${DATABASE_URL%/*}"
SCRATCH_URL="$DB_PREFIX/$SCRATCH_DB"
TMP_DIR="$(mktemp -d "/tmp/openpr-flow-mcp-verify.XXXXXX")"
API_PORT=$((20000 + RANDOM % 10000))
HTTP_PORT=$((30001 + RANDOM % 10000))
SSE_PORT=$((40001 + RANDOM % 10000))
API_PID=""; HTTP_PID=""; SSE_PID=""

# shellcheck disable=SC2317  # invoked only via `trap ... EXIT`
cleanup() {
  local ec=$?
  for pid in "$SSE_PID" "$HTTP_PID" "$API_PID"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "DROP DATABASE IF EXISTS \"$SCRATCH_DB\" WITH (FORCE)" >/dev/null 2>&1 || true
  rm -rf "$TMP_DIR"
  exit "$ec"
}
trap cleanup EXIT

psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "DROP DATABASE IF EXISTS \"$SCRATCH_DB\" WITH (FORCE)" >/dev/null 2>&1 || true
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "CREATE DATABASE \"$SCRATCH_DB\"" >/dev/null

JWT_SECRET="flow-mcp-transport-verify-not-a-real-secret"
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

echo "=== starting api on 127.0.0.1:$API_PORT ===" >&2
"$API_BIN" --config "$API_CONFIG" > "$TMP_DIR/api.log" 2>&1 &
API_PID=$!
HEALTHY=0
for _ in $(seq 1 120); do
  if curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1; then HEALTHY=1; break; fi
  if ! kill -0 "$API_PID" 2>/dev/null; then break; fi
  sleep 0.5
done
[[ $HEALTHY -eq 1 ]] || { echo "FAIL: api did not become healthy" >&2; tail -40 "$TMP_DIR/api.log" >&2; exit 2; }

WORKSPACE="$(python3 -c 'import uuid; print(uuid.uuid4())')"
OTHER_WORKSPACE="$(python3 -c 'import uuid; print(uuid.uuid4())')"
OWNER_USER="$(python3 -c 'import uuid; print(uuid.uuid4())')"
BOT_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
BOT_TOKEN="opr_mcp_transport_verify_${RUN_ID}"
BOT_TOKEN_HASH="$(printf '%s' "$BOT_TOKEN" | sha256sum | awk '{print $1}')"
BOT_TOKEN_PREFIX="${BOT_TOKEN:0:8}"

# The bot is seeded exactly the way `POST /workspaces/{id}/bots` seeds one
# (apps/api/src/routes/bot.rs): a `users` row with the SAME id, a
# `workspace_members` row, and the `workspace_bots` token row. A fixture that
# writes only `workspace_bots` produces a bot whose actor id is not a user, and
# every Flow write then fails on `flow_objects.created_by`'s foreign key.
psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, created_at, updated_at)
VALUES ('$OWNER_USER', 'mcp-verify-$RUN_ID@example.local', '', 'MCP Verify Owner', 'user', true, 'human', now(), now());
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES ('$BOT_ID', '$BOT_ID@bot.openpr.local', '!', 'MCP Verify Bot', 'user', true, 'bot_mcp', 'mcp', now(), now());
INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE', 'mcp-verify-$RUN_ID', 'MCP Verify', '$OWNER_USER', now(), now()),
       ('$OTHER_WORKSPACE', 'mcp-verify-other-$RUN_ID', 'MCP Verify Other', '$OWNER_USER', now(), now());
INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES ('$WORKSPACE', '$OWNER_USER', 'owner', now()),
       ('$OTHER_WORKSPACE', '$OWNER_USER', 'owner', now()),
       ('$WORKSPACE', '$BOT_ID', 'admin', now());
INSERT INTO flow_workspace_settings (workspace_id, flow_enabled, default_member_level, authz_epoch, updated_at)
VALUES ('$WORKSPACE', true, 'edit', 0, now()),
       ('$OTHER_WORKSPACE', true, 'edit', 0, now());
INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by, is_active, created_at, updated_at)
VALUES ('$BOT_ID', '$WORKSPACE', 'MCP Verify Bot', '$BOT_TOKEN_HASH', '$BOT_TOKEN_PREFIX', '["read","write","admin"]'::jsonb, '$OWNER_USER', true, now(), now());
SQL

USER_JWT="$(python3 -c '
import base64, hashlib, hmac, json, sys, time
def b64(o):
    return base64.urlsafe_b64encode(json.dumps(o, separators=(",", ":")).encode()).rstrip(b"=").decode()
secret, sub, email = sys.argv[1], sys.argv[2], sys.argv[3]
now = int(time.time())
head = b64({"alg": "HS256", "typ": "JWT"})
body = b64({"sub": sub, "email": email, "token_type": "access", "iat": now, "exp": now + 3600})
sig = base64.urlsafe_b64encode(hmac.new(secret.encode(), f"{head}.{body}".encode(), hashlib.sha256).digest()).rstrip(b"=").decode()
print(f"{head}.{body}.{sig}")
' "$JWT_SECRET" "$OWNER_USER" "mcp-verify-$RUN_ID@example.local")"

API="http://127.0.0.1:$API_PORT"
UAUTH=(-H "Authorization: Bearer $USER_JWT" -H "Content-Type: application/json")
json_get() { jq -r "$2" <<<"$1" 2>/dev/null || printf ''; }

CREATE_RESP="$(curl -sS -X POST "$API/api/v1/workspaces/$WORKSPACE/flow/objects" "${UAUTH[@]}" \
  -d "$(jq -n --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" '{object_type:"page", title:"MCP transport fixture", idempotency_key:$key}')")"
OBJECT_ID="$(json_get "$CREATE_RESP" '.data.id // .data.object.id // empty')"
[[ -n "$OBJECT_ID" ]] || { echo "FAIL: could not create the Flow page the tools/call fixture reads (response: $CREATE_RESP)" >&2; exit 2; }

OTHER_RESP="$(curl -sS -X POST "$API/api/v1/workspaces/$OTHER_WORKSPACE/flow/objects" "${UAUTH[@]}" \
  -d "$(jq -n --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" '{object_type:"page", title:"Foreign workspace page", idempotency_key:$key}')")"
OTHER_OBJECT_ID="$(json_get "$OTHER_RESP" '.data.id // .data.object.id // empty')"
[[ -n "$OTHER_OBJECT_ID" ]] || { echo "FAIL: could not create the foreign-workspace page the negative fixture needs (response: $OTHER_RESP)" >&2; exit 2; }

# ---- mcp-server configs, one per transport ----
write_mcp_config() {
  local path="$1" transport="$2" bind="$3"
  {
    printf '[database]\nurl = "postgres://unused:unused@127.0.0.1:5432/unused"\n\n'
    printf '[auth]\njwt_secret = "unused-by-the-mcp-server"\n\n'
    printf '[logging]\nfilter = "error"\nformat = "text"\n\n'
    printf '[mcp]\napi_url = "%s"\nbot_token = "%s"\nworkspace_id = "%s"\ntransport = "%s"\n' \
      "$API" "$BOT_TOKEN" "$WORKSPACE" "$transport"
    if [[ -n "$bind" ]]; then printf 'bind_addr = "%s"\n' "$bind"; fi
  } > "$path"
}

HTTP_CONFIG="$TMP_DIR/mcp-http.toml"
SSE_CONFIG="$TMP_DIR/mcp-sse.toml"
STDIO_CONFIG="$TMP_DIR/mcp-stdio.toml"
write_mcp_config "$HTTP_CONFIG" http "127.0.0.1:$HTTP_PORT"
write_mcp_config "$SSE_CONFIG" sse "127.0.0.1:$SSE_PORT"
write_mcp_config "$STDIO_CONFIG" stdio ""

echo "=== starting mcp-server (http) on 127.0.0.1:$HTTP_PORT ===" >&2
"$MCP_BIN" --config "$HTTP_CONFIG" serve --transport http > "$TMP_DIR/mcp-http.log" 2>&1 &
HTTP_PID=$!
echo "=== starting mcp-server (sse) on 127.0.0.1:$SSE_PORT ===" >&2
"$MCP_BIN" --config "$SSE_CONFIG" serve --transport sse > "$TMP_DIR/mcp-sse.log" 2>&1 &
SSE_PID=$!

wait_port() {
  local port="$1" pid="$2"
  for _ in $(seq 1 60); do
    if curl -fsS -o /dev/null "http://127.0.0.1:$port/health" 2>/dev/null; then return 0; fi
    if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then exec 3<&- 3>&-; return 0; fi
    if ! kill -0 "$pid" 2>/dev/null; then return 1; fi
    sleep 0.5
  done
  return 1
}
HTTP_UP=1; SSE_UP=1
wait_port "$HTTP_PORT" "$HTTP_PID" || HTTP_UP=0
wait_port "$SSE_PORT" "$SSE_PID" || SSE_UP=0

# ---- the identical JSON-RPC conversation each transport is asked to hold ----
REQUESTS="$(jq -c -n --arg object_id "$OBJECT_ID" --arg other_id "$OTHER_OBJECT_ID" '[
  {jsonrpc:"2.0", id:1, method:"initialize", params:{protocolVersion:"2024-11-05", capabilities:{}, clientInfo:{name:"flow-v0.4-transport-verifier", version:"1"}}},
  {jsonrpc:"2.0", id:2, method:"tools/list", params:{}},
  {jsonrpc:"2.0", id:3, method:"tools/call", params:{name:"objects.get", arguments:{object_id:$object_id}}},
  {jsonrpc:"2.0", id:4, method:"tools/call", params:{name:"objects.get", arguments:{object_id:$other_id}}},
  {jsonrpc:"2.0", id:5, method:"tools/call", params:{name:"definitely.not.a.registered.tool", arguments:{}}}
]')"

run_probe() {
  local transport="$1"; shift
  python3 "$PROBE" --transport "$transport" --requests "$REQUESTS" --timeout 30 "$@" 2>/dev/null || \
    printf '{"transport":"%s","responses":[],"errors":["probe helper crashed"]}' "$transport"
}

echo "=== speaking JSON-RPC over http ===" >&2
if [[ $HTTP_UP -eq 1 ]]; then
  HTTP_JSON="$(run_probe http --host 127.0.0.1 --port "$HTTP_PORT" --token "$BOT_TOKEN")"
else
  HTTP_JSON="$(jq -c -n --arg log "$(tail -20 "$TMP_DIR/mcp-http.log" 2>/dev/null | tr -d '\000')" \
    '{transport:"http", responses:[], errors:[("the http transport never accepted a connection; server log tail: " + $log)]}')"
fi

echo "=== speaking JSON-RPC over sse ===" >&2
if [[ $SSE_UP -eq 1 ]]; then
  SSE_JSON="$(run_probe sse --host 127.0.0.1 --port "$SSE_PORT" --token "$BOT_TOKEN")"
else
  SSE_JSON="$(jq -c -n --arg log "$(tail -20 "$TMP_DIR/mcp-sse.log" 2>/dev/null | tr -d '\000')" \
    '{transport:"sse", responses:[], errors:[("the sse transport never accepted a connection; server log tail: " + $log)]}')"
fi

echo "=== speaking JSON-RPC over stdio ===" >&2
STDIO_JSON="$(run_probe stdio --binary "$MCP_BIN" --config "$STDIO_CONFIG")"

# ---- comparison ----
COMPARE_JSON="$(python3 -c '
import hashlib, json, sys

per = {}
for raw in sys.argv[1:4]:
    d = json.loads(raw)
    per[d["transport"]] = d

violations = []
summary = {}

def response_for(t, rid):
    for r in per[t]["responses"]:
        if r.get("id") == rid:
            return r
    return None

def content_text(resp):
    if not resp:
        return None
    result = resp.get("result")
    if not isinstance(result, dict):
        return None
    parts = []
    for item in result.get("content", []) or []:
        if isinstance(item, dict) and "text" in item:
            parts.append(item["text"])
    return "\n".join(parts) if parts else None

def is_error(resp):
    if not resp:
        return None
    if "error" in resp:
        return True
    result = resp.get("result")
    if isinstance(result, dict) and "isError" in result:
        return bool(result["isError"])
    return False

for t in ("http", "sse", "stdio"):
    errs = per[t].get("errors") or []
    if errs:
        violations.append(f"T1 {t}: transport reported {len(errs)} error(s): {errs}")
    init = response_for(t, 1)
    if init is None or "result" not in init:
        violations.append(f"T1 {t}: no usable answer to initialize (got {json.dumps(init)[:300]})")

# --- T2/T3: tools/list ---
name_sets, name_hashes, schema_hashes, counts = {}, {}, {}, {}
for t in ("http", "sse", "stdio"):
    resp = response_for(t, 2)
    tools = ((resp or {}).get("result") or {}).get("tools")
    if not isinstance(tools, list):
        violations.append(f"T2 {t}: tools/list returned no tools array (got {json.dumps(resp)[:300]})")
        continue
    names = sorted(tool.get("name", "") for tool in tools)
    name_sets[t] = names
    counts[t] = len(names)
    name_hashes[t] = hashlib.sha256("\n".join(names).encode()).hexdigest()
    canonical = json.dumps(
        {tool.get("name"): tool.get("inputSchema") for tool in tools},
        sort_keys=True, separators=(",", ":"),
    )
    schema_hashes[t] = hashlib.sha256(canonical.encode()).hexdigest()

if len(name_hashes) == 3 and len(set(name_hashes.values())) != 1:
    for a in ("sse", "stdio"):
        only_here = sorted(set(name_sets[a]) - set(name_sets["http"]))
        only_http = sorted(set(name_sets["http"]) - set(name_sets[a]))
        if only_here or only_http:
            violations.append(
                f"T2: tools/list differs between http and {a} -- only on {a}: {only_here}; only on http: {only_http}"
            )
if len(counts) == 3 and len(set(counts.values())) != 1:
    violations.append(f"T2: tools/list counts differ across transports: {counts}")
if len(schema_hashes) == 3 and len(set(schema_hashes.values())) != 1:
    violations.append(f"T3: per-tool input schemas hash differently across transports: {schema_hashes}")

# --- T5: the Flow tools v0.4 registers ---
required = ["flow.feature_get", "flow.feature_set", "objects.get", "objects.query", "objects.history",
            "legacy_pages.inventory", "legacy_pages.import_preview", "legacy_pages.import_commit",
            "legacy_pages.import_status"]
for t, names in name_sets.items():
    missing = [n for n in required if n not in names]
    if missing:
        violations.append(f"T5 {t}: tools/list is missing v0.4 Flow tools {missing}")

# --- T4: the real call ---
call_texts = {}
for t in ("http", "sse", "stdio"):
    resp = response_for(t, 3)
    if is_error(resp) is not False:
        violations.append(f"T4 {t}: objects.get on an in-workspace page did not succeed (got {json.dumps(resp)[:400]})")
        continue
    text = content_text(resp)
    if text is None:
        violations.append(f"T4 {t}: objects.get returned no textual content (got {json.dumps(resp)[:400]})")
        continue
    call_texts[t] = text
if len(call_texts) == 3:
    normalised = {}
    for t, text in call_texts.items():
        try:
            normalised[t] = json.dumps(json.loads(text), sort_keys=True, separators=(",", ":"))
        except json.JSONDecodeError:
            normalised[t] = text
    if len(set(normalised.values())) != 1:
        violations.append("T4: objects.get returned different content across transports: " + json.dumps({t: v[:200] for t, v in normalised.items()}))
    summary["objects_get_sha256"] = {t: hashlib.sha256(v.encode()).hexdigest() for t, v in normalised.items()}

# --- T6: cross-workspace refusal ---
cross = {}
for t in ("http", "sse", "stdio"):
    resp = response_for(t, 4)
    err = is_error(resp)
    cross[t] = err
    if err is not True:
        violations.append(f"T6 {t}: an object from another workspace was NOT refused (isError={err}, got {json.dumps(resp)[:400]})")
if len(set(v for v in cross.values() if v is not None)) > 1:
    violations.append(f"T6: the cross-workspace refusal differs across transports: {cross}")

# --- T7: unknown tool refusal ---
unknown = {}
for t in ("http", "sse", "stdio"):
    resp = response_for(t, 5)
    err = is_error(resp)
    unknown[t] = err
    if err is not True:
        violations.append(f"T7 {t}: an unregistered tool name was not refused (isError={err}, got {json.dumps(resp)[:300]})")

summary.update({
    "tool_counts": counts,
    "tool_name_sha256": name_hashes,
    "tool_schema_sha256": schema_hashes,
    "cross_workspace_is_error": cross,
    "unknown_tool_is_error": unknown,
    "transport_errors": {t: per[t].get("errors") or [] for t in per},
})
print(json.dumps({"summary": summary, "violations": violations}))
' "$HTTP_JSON" "$SSE_JSON" "$STDIO_JSON")"

jq -e . >/dev/null 2>&1 <<<"$COMPARE_JSON" || { echo "FAIL: transport comparison did not produce valid JSON" >&2; echo "$COMPARE_JSON" >&2; exit 2; }

VIOLATIONS_JSON="$(jq -c '.violations' <<<"$COMPARE_JSON")"
SUMMARY_JSON="$(jq -c '.summary' <<<"$COMPARE_JSON")"
NUM_VIOLATIONS="$(jq 'length' <<<"$VIOLATIONS_JSON")"
PASSED=$([[ "$NUM_VIOLATIONS" -eq 0 ]] && echo true || echo false)
GATE_STATUS=$([[ "$PASSED" == "true" ]] && echo passed || echo failed)
REASON="live JSON-RPC over http/sse/stdio against the shipped mcp-server: tool counts $(jq -c '.tool_counts' <<<"$SUMMARY_JSON"), name hashes agree=$(jq '[.tool_name_sha256[]]|unique|length==1' <<<"$SUMMARY_JSON"), schema hashes agree=$(jq '[.tool_schema_sha256[]]|unique|length==1' <<<"$SUMMARY_JSON"); $NUM_VIOLATIONS violation(s)"

RESULT="$(jq -n \
  --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" \
  --arg object_id "$OBJECT_ID" --arg other_object_id "$OTHER_OBJECT_ID" \
  --argjson requests "$REQUESTS" \
  --argjson http "$HTTP_JSON" --argjson sse "$SSE_JSON" --argjson stdio "$STDIO_JSON" \
  --argjson summary "$SUMMARY_JSON" --argjson violations "$VIOLATIONS_JSON" \
  --argjson passed "$PASSED" --arg gate_status "$GATE_STATUS" --arg reason "$REASON" \
  '{
    schema_version: "sylvode.flow.mcp-contract-result.v1",
    source_head: $head,
    generated_at: $generated_at,
    fixture: {object_id: $object_id, foreign_workspace_object_id: $other_object_id},
    transports: ["http", "sse", "stdio"],
    jsonrpc_conversation: $requests,
    raw: {http: $http, sse: $sse, stdio: $stdio},
    comparison: $summary,
    violations: $violations,
    passed: $passed,
    gates: {
      mcp_three_transport_contract: {status: $gate_status, reason: $reason}
    }
  }')"

OUT_PATH="$EVIDENCE_ROOT/mcp-contract-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$RESULT" | jq . > "$OUT_TMP"
sync "$OUT_TMP" 2>/dev/null || true
mv -f "$OUT_TMP" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2

if [[ "$PASSED" != "true" ]]; then
  jq -r '.violations[] | "  VIOLATION: " + .' <<<"$RESULT" >&2
fi

echo "$RESULT"
[[ "$PASSED" == "true" ]] && exit 0
exit 1
