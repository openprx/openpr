#!/bin/bash
set -euo pipefail

# MCP Server Integration Test Script
# Tests tool discovery and invocation
#
# Environment:
#   MCP_URL                MCP base URL. Default: http://localhost:8090
#   OPENPR_CONFIG_FILE     MCP configuration file the caller token is read from.
#                          Default: canonical config/sylvode.compose.mcp.toml, with
#                          config/openpr.compose.mcp.toml as a legacy fallback.
#   OPENPR_MCP_BOT_TOKEN   Call as this workspace bot instead (opr_ prefix).
#
# Exit codes:
#   0  live MCP discovery and invocation passed
#   1  a reachable MCP implementation failed a behavioral assertion
#   2  local usage/tool error
#   69 the configured external MCP environment is unavailable (machine-readable
#      MCP_TEST_RESULT=environment_unavailable is emitted). Callers must not
#      reinterpret this as product success without stronger live coverage.

MCP_URL="${MCP_URL:-http://localhost:8090}"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/sylvode_compat.sh
source "$PROJECT_ROOT/scripts/lib/sylvode_compat.sh"
if [[ -n "${OPENPR_CONFIG_FILE:-}" ]]; then
  CONFIG_FILE=$OPENPR_CONFIG_FILE
else
  CONFIG_FILE=$(sylvode_select_config \
    "$PROJECT_ROOT/config/sylvode.compose.mcp.toml" \
    "$PROJECT_ROOT/config/openpr.compose.mcp.toml")
fi

environment_unavailable() {
  local reason="$1"
  echo "MCP_TEST_RESULT=environment_unavailable reason=$reason url=$MCP_URL"
  exit 69
}

for required_tool in curl python3 grep sed wc tr; do
  if ! command -v "$required_tool" >/dev/null 2>&1; then
    echo "MCP_TEST_RESULT=tool_error missing=$required_tool" >&2
    exit 2
  fi
done

# Keep the default coupled to the repository-owned registry snapshot. An explicit override is
# still useful when testing a deliberately rebased external deployment, but stale source defaults
# must fail the same exact-count comparison below instead of silently weakening it.
EXPECTED_TOOL_COUNT="${EXPECTED_TOOL_COUNT:-$(python3 "$PROJECT_ROOT/skills/openpr-mcp/scripts/expected-tool-count.py")}"

# Probe the external runtime before reading credentials. A closed compose port
# is an environment precondition failure, not evidence that MCP product code
# failed. Conversely, an HTTP response from a reachable service with a bad
# health payload is a real behavioral failure and remains exit 1.
HEALTH_FILE="$(mktemp)"
trap 'rm -f "$HEALTH_FILE"' EXIT
set +e
HEALTH_HTTP_CODE="$(curl -sS --connect-timeout 2 --max-time 5 -o "$HEALTH_FILE" -w '%{http_code}' "$MCP_URL/health")"
HEALTH_CURL_EXIT=$?
set -e
if [[ $HEALTH_CURL_EXIT -ne 0 ]]; then
  environment_unavailable "health_endpoint_unreachable_curl_exit_${HEALTH_CURL_EXIT}"
fi
HEALTH_RESPONSE="$(cat "$HEALTH_FILE")"
if [[ "$HEALTH_HTTP_CODE" != "200" ]] || ! grep -Eiq "^(ok|healthy)$" <<<"$HEALTH_RESPONSE"; then
  echo "MCP_TEST_RESULT=implementation_failed reason=health_contract http_status=$HEALTH_HTTP_CODE"
  echo "Response: $HEALTH_RESPONSE"
  exit 1
fi

# Reads one dotted key out of the TOML configuration file, using the tomllib parser in python3
# (3.11+) rather than a grep that would mis-handle quoting and section scoping. Prints nothing
# when the file or the key is absent, so the caller decides whether that is fatal. The value goes
# to stdout only, never to the log.
read_config_value() {
  local key="$1"
  local file="$2"
  [ -f "$file" ] || return 0
  OPENPR_CONFIG_KEY="$key" OPENPR_CONFIG_PATH="$file" python3 -c '
import os
import sys
import tomllib

try:
    with open(os.environ["OPENPR_CONFIG_PATH"], "rb") as handle:
        data = tomllib.load(handle)
except (OSError, tomllib.TOMLDecodeError):
    raise SystemExit(0)

node = data
for part in os.environ["OPENPR_CONFIG_KEY"].split("."):
    if not isinstance(node, dict) or part not in node:
        raise SystemExit(0)
    node = node[part]
if isinstance(node, str):
    sys.stdout.write(node)
'
}

# /mcp/rpc, /sse and /messages are served as the caller that made the request: there is no shared
# inbound secret any more, every request presents a workspace bot token in `Authorization: Bearer
# opr_...`, and the MCP server forwards it to the API, which authenticates it. /health is exempt.
#
# The identity has to be a bot of the workspace this MCP server is bound to -- it scopes every API
# call to mcp.workspace_id, and a bot from anywhere else is answered "bot not authorized for this
# workspace". mcp.bot_token in the same file is exactly that bot, which makes it the one source
# that cannot disagree with mcp.workspace_id. Minting a fresh bot here instead would put it in
# some other workspace and fail. scripts/bootstrap-restaurant-demo.sh is what puts a real pair in
# the file; before it runs, both values are placeholders that name nothing.
MCP_BOT_TOKEN="${OPENPR_MCP_BOT_TOKEN:-}"
if [ -z "$MCP_BOT_TOKEN" ]; then
  MCP_BOT_TOKEN="$(read_config_value mcp.bot_token "$CONFIG_FILE")"
fi
if [ -z "$MCP_BOT_TOKEN" ]; then
  echo "❌ No MCP caller bot token available"
  echo "   The MCP server rejects /mcp/rpc without 'Authorization: Bearer <opr_ bot token>'."
  echo "   Set mcp.bot_token in $CONFIG_FILE, or export OPENPR_MCP_BOT_TOKEN."
  echo "   bash scripts/bootstrap-restaurant-demo.sh creates a workspace bot and writes it there."
  environment_unavailable "caller_bot_token_missing"
fi
case "$MCP_BOT_TOKEN" in
  # The bootstrap placeholder scripts/start.sh writes so the container passes validation and
  # starts. It names no account, so every authenticated call below would come back 401.
  opr_local_*)
    echo "❌ mcp.bot_token in $CONFIG_FILE is still the placeholder scripts/start.sh generated"
    echo "   It belongs to no workspace, so the API rejects it. Run"
    echo "   bash scripts/bootstrap-restaurant-demo.sh to replace it, and mcp.workspace_id with"
    echo "   it, or export OPENPR_MCP_BOT_TOKEN for a bot of the configured workspace."
    environment_unavailable "caller_bot_token_placeholder"
    ;;
esac

echo "🧪 Starting MCP Server Tests"
echo "MCP URL: $MCP_URL"
echo ""

# Test 1: Health Check (the environment gate above already performed the live
# request; repeat its recorded result here so the human test transcript remains
# in the familiar three-test order without issuing a weaker second probe).
echo "📋 Test 1: MCP Server Health Check"
echo "✅ MCP server is healthy (HTTP $HEALTH_HTTP_CODE)"
echo ""

# Test 2: List Tools
echo "📋 Test 2: List Available Tools"
tools_response=$(curl -sS -X POST "$MCP_URL/mcp/rpc" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $MCP_BOT_TOKEN" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}')

tool_names=$(python3 -c '
import json
import sys

payload = json.load(sys.stdin)
if payload.get("error"):
    print(payload["error"], file=sys.stderr)
    raise SystemExit(1)

tools = payload.get("result", {}).get("tools")
if not isinstance(tools, list):
    print("JSON-RPC tools/list response is missing result.tools", file=sys.stderr)
    raise SystemExit(1)

for tool in tools:
    name = tool.get("name")
    if name:
        print(name)
' <<<"$tools_response") || {
  echo "❌ Failed to retrieve tools list: $tools_response"
  exit 1
}

tool_count=$(printf '%s\n' "$tool_names" | sed '/^$/d' | wc -l | tr -d ' ')
if (( tool_count != EXPECTED_TOOL_COUNT )); then
  echo "❌ MCP tool count mismatch: expected exactly $EXPECTED_TOOL_COUNT, got $tool_count"
  echo "$tool_names"
  exit 1
fi

for required_tool in \
  "forms.list" \
  "forms.create_from_template" \
  "forms.duplicate" \
  "forms.schema_summary" \
  "forms.field_usage" \
  "forms.field_dependencies" \
  "form_schema_versions.list" \
  "form_schema_versions.get" \
  "form_permissions.get" \
  "form_permissions.update" \
  "form_attachments.list" \
  "form_attachments.create" \
  "form_attachments.archive" \
  "form_attachments.restore" \
  "form_records.export" \
  "form_records.import_preview" \
  "form_records.import_commit" \
  "form_records.create" \
  "form_records.relation_targets" \
  "form_records.children" \
  "form_records.child_create" \
  "form_records.child_update" \
  "form_records.child_archive" \
  "form_records.child_restore" \
  "form_records.aggregate" \
  "events.tail" \
  "plugins.list" \
  "plugins.invoke" \
  "plugin_invocations.list" \
  "bot_operation_logs.list" \
  "release.readiness.get" \
  "scenario_templates.list" \
  "scenario_templates.get" \
  "scenario_templates.install" \
  "flow.feature_get" \
  "flow.feature_set" \
  "objects.get" \
  "objects.query" \
  "objects.history" \
  "legacy_pages.inventory" \
  "legacy_pages.import_preview" \
  "legacy_pages.import_commit" \
  "legacy_pages.import_status"; do
  if ! printf '%s\n' "$tool_names" | grep -Fxq "$required_tool"; then
    echo "❌ Missing required MCP tool: $required_tool"
    echo "$tool_names"
    exit 1
  fi
done

echo "✅ Tools list retrieved ($tool_count tools)"
echo "Required universal forms and plugin tools are present"
echo ""

# Test 3: Call a stable read-only tool.
echo "📋 Test 3: Invoke Tool (projects.list)"
call_response=$(curl -sS -X POST "$MCP_URL/mcp/rpc" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $MCP_BOT_TOKEN" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"projects.list","arguments":{}}}')

python3 -c '
import json
import sys

payload = json.load(sys.stdin)
if payload.get("error"):
    print(payload["error"], file=sys.stderr)
    raise SystemExit(1)

result = payload.get("result")
if not isinstance(result, dict):
    print("JSON-RPC tools/call response is missing result", file=sys.stderr)
    raise SystemExit(1)

# The wire field is isError; is_error is accepted too so the check cannot be silently defeated by
# a serialisation change. Reading only the snake_case name is what used to let an API-side 401 --
# the whole failure mode of a caller token that names nothing -- pass as a successful invocation.
if result.get("isError") is True or result.get("is_error") is True:
    print(json.dumps(result.get("content", []), ensure_ascii=False), file=sys.stderr)
    raise SystemExit(1)

content = result.get("content")
if not isinstance(content, list):
    print("JSON-RPC tools/call result is missing content", file=sys.stderr)
    raise SystemExit(1)
' <<<"$call_response" || {
  echo "❌ Tool invocation failed: $call_response"
  exit 1
}

echo "✅ Tool invocation successful"
echo ""

echo "🎉 All MCP tests passed!"
