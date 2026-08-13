#!/bin/bash
set -e

# MCP Server Integration Test Script
# Tests tool discovery and invocation

MCP_URL="${MCP_URL:-http://localhost:8090}"
EXPECTED_TOOL_COUNT="${EXPECTED_TOOL_COUNT:-105}"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_FILE="${OPENPR_CONFIG_FILE:-$PROJECT_ROOT/config/openpr.compose.mcp.toml}"

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

# /mcp/rpc, /sse and /messages require a bearer token when the server was configured with
# mcp.auth_token; /health is exempt. OPENPR_MCP_AUTH_TOKEN is an operator override for whoever
# runs this script by hand; it is not application config, the binaries only read the TOML file.
MCP_AUTH_TOKEN="${OPENPR_MCP_AUTH_TOKEN:-}"
if [ -z "$MCP_AUTH_TOKEN" ]; then
  MCP_AUTH_TOKEN="$(read_config_value mcp.auth_token "$CONFIG_FILE")"
fi
if [ -z "$MCP_AUTH_TOKEN" ]; then
  echo "❌ No MCP bearer token available"
  echo "   The MCP server rejects /mcp/rpc without 'Authorization: Bearer <token>'."
  echo "   Set mcp.auth_token in $CONFIG_FILE, or export OPENPR_MCP_AUTH_TOKEN"
  echo "   (bash scripts/start.sh generates the configuration on first run)."
  exit 1
fi

echo "🧪 Starting MCP Server Tests"
echo "MCP URL: $MCP_URL"
echo ""

# Test 1: Health Check
echo "📋 Test 1: MCP Server Health Check"
response=$(curl -s "$MCP_URL/health" || echo "failed")
if echo "$response" | grep -Eiq "^(ok|healthy)$"; then
  echo "✅ MCP server is healthy"
else
  echo "❌ MCP server health check failed"
  echo "Response: $response"
  exit 1
fi
echo ""

# Test 2: List Tools
echo "📋 Test 2: List Available Tools"
tools_response=$(curl -sS -X POST "$MCP_URL/mcp/rpc" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $MCP_AUTH_TOKEN" \
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
  "connectors.list" \
  "release.readiness.get" \
  "scenario_templates.list" \
  "scenario_templates.get" \
  "scenario_templates.install"; do
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
  -H "Authorization: Bearer $MCP_AUTH_TOKEN" \
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

if result.get("is_error") is True:
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
