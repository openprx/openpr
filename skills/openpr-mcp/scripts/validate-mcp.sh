#!/bin/bash
# Quick smoke test for OpenPR MCP server connectivity
# Usage: ./validate-mcp.sh [http://localhost:8090] [project-uuid]
# Calls the server as a workspace bot. The token is read from mcp.bot_token in the TOML
# configuration file, by default config/openpr.compose.mcp.toml in the repository root; override
# the file with OPENPR_CONFIG_FILE, or the token itself with OPENPR_MCP_BOT_TOKEN.

MCP_URL="${1:-http://localhost:8090}"
PROJECT_ID="${2:-}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG_FILE="${OPENPR_CONFIG_FILE:-$SCRIPT_DIR/../../../config/openpr.compose.mcp.toml}"

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
# inbound secret, every request presents its own workspace bot token in `Authorization: Bearer
# opr_...`, and the MCP server forwards that token to the API, which authenticates it. /health is
# exempt and stays unauthenticated.
#
# mcp.bot_token is the natural source here: it is already a workspace bot token of this
# deployment — the identity the stdio transport runs as — so this smoke test can call the HTTP
# transport as somebody without credentials of its own, an API round trip, or a second account.
# OPENPR_MCP_BOT_TOKEN overrides it for whoever wants to validate as a different bot.
MCP_BOT_TOKEN="${OPENPR_MCP_BOT_TOKEN:-}"
if [ -z "$MCP_BOT_TOKEN" ]; then
  MCP_BOT_TOKEN="$(read_config_value mcp.bot_token "$CONFIG_FILE")"
fi
if [ -z "$MCP_BOT_TOKEN" ]; then
  echo "❌ No MCP caller bot token available"
  echo "   The MCP server rejects /mcp/rpc without 'Authorization: Bearer <opr_ bot token>'."
  echo "   Set mcp.bot_token in $CONFIG_FILE, or export OPENPR_MCP_BOT_TOKEN."
  echo "   Create one under Workspace → Members → Bot Tokens, or run"
  echo "   scripts/bootstrap-restaurant-demo.sh, which mints one and writes it into that file."
  exit 1
fi
case "$MCP_BOT_TOKEN" in
  # What scripts/start.sh generates so the container passes validation and starts. It names no
  # account, so every authenticated call below would come back 401 without an explanation.
  opr_local_*)
    echo "❌ mcp.bot_token in $CONFIG_FILE is still the bootstrap placeholder scripts/start.sh wrote"
    echo "   It belongs to no workspace, so the API rejects it. Run"
    echo "   scripts/bootstrap-restaurant-demo.sh to replace it with a real bot token, or export"
    echo "   OPENPR_MCP_BOT_TOKEN."
    exit 1
    ;;
esac

echo "Testing MCP at $MCP_URL ..."

# tools/list
TOOLS_RESPONSE=$(curl -s -X POST "$MCP_URL/mcp/rpc" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $MCP_BOT_TOKEN" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}')

TOOLS=$(printf '%s' "$TOOLS_RESPONSE" | \
  python3 -c "import sys,json; print(len(json.load(sys.stdin)['result']['tools']))" 2>/dev/null)

if [ "${TOOLS:-0}" -eq 98 ] 2>/dev/null; then
  echo "✅ tools/list: $TOOLS tools available"
else
  echo "❌ tools/list expected exactly 98 tools, got ${TOOLS:-0}"
  exit 1
fi

MISSING=$(printf '%s' "$TOOLS_RESPONSE" | python3 -c 'import json, sys
required = {
    "bot_operation_logs.list",
    "project_types.list",
    "project_types.get",
    "scenario_templates.list",
    "scenario_templates.get",
    "scenario_templates.install",
    "project_resources.list",
    "project_resources.create",
    "project_resources.update",
    "project_resources.delete",
    "forms.list",
    "forms.get",
    "forms.create",
    "forms.create_from_template",
    "forms.update_schema",
    "forms.duplicate",
    "forms.schema_summary",
    "forms.field_usage",
    "forms.field_dependencies",
    "form_schema_versions.list",
    "form_schema_versions.get",
    "form_permissions.get",
    "form_permissions.update",
    "form_views.list",
    "form_attachments.list",
    "form_attachments.create",
    "form_attachments.archive",
    "form_attachments.restore",
    "form_records.list",
    "form_records.export",
    "form_records.import_preview",
    "form_records.import_commit",
    "form_records.get",
    "form_records.create",
    "form_records.update",
    "form_records.link",
    "form_records.relation_targets",
    "form_records.children",
    "form_records.child_create",
    "form_records.child_update",
    "form_records.child_archive",
    "form_records.child_restore",
    "form_records.aggregate",
    "events.tail",
    "plugins.list",
    "plugins.get",
    "plugins.install",
    "plugins.invoke",
    "plugin_invocations.list",
    "context.get_project",
    "context.get_governance",
    "context.get_agent_policy",
    "release.readiness.get",
    "check_results.create",
    "proposals.create_from_result",
    "code.resources.list",
    "code.directory.get",
    "code.task_context.get",
    "code.change_proposal.create",
    "documents.extract_summary",
    "documents.review_risk",
    "approval.request",
    "inspection.report",
    "corrective_action.propose",
}
payload = json.load(sys.stdin)
names = {tool["name"] for tool in payload["result"]["tools"]}
print(",".join(sorted(required - names)))' 2>/dev/null)

if [ -z "$MISSING" ]; then
  echo "✅ project type/resource, universal forms, plugin, connector, and invocation tools: present"
else
  echo "❌ missing project type/resource, universal forms, plugin, connector, or invocation tools: $MISSING"
  exit 1
fi

if [ -n "$PROJECT_ID" ]; then
  PROJECT_TOOLS_RESPONSE=$(curl -s -X POST "$MCP_URL/mcp/rpc" \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer $MCP_BOT_TOKEN" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{\"project_id\":\"$PROJECT_ID\"}}")

  PROJECT_TOOLS=$(printf '%s' "$PROJECT_TOOLS_RESPONSE" | \
    python3 -c "import sys,json; print(len(json.load(sys.stdin)['result']['tools']))" 2>/dev/null)

  PROJECT_MISSING=$(printf '%s' "$PROJECT_TOOLS_RESPONSE" | python3 -c 'import json, sys
required = {
    "context.get_project",
    "context.get_agent_policy",
    "release.readiness.get",
    "check_results.create",
    "proposals.create_from_result",
    "code.resources.list",
    "code.task_context.get",
}
payload = json.load(sys.stdin)
names = {tool["name"] for tool in payload["result"]["tools"]}
print(",".join(sorted(required - names)))' 2>/dev/null)

  if [ "$PROJECT_TOOLS" -gt 0 ] 2>/dev/null && [ -z "$PROJECT_MISSING" ]; then
    echo "✅ project-aware tools/list: $PROJECT_TOOLS tools enabled for $PROJECT_ID"
  else
    echo "❌ project-aware tools/list failed for $PROJECT_ID"
    echo "$PROJECT_TOOLS_RESPONSE"
    exit 1
  fi
fi

# projects.list
RESULT=$(curl -s -X POST "$MCP_URL/mcp/rpc" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $MCP_BOT_TOKEN" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"projects.list","arguments":{}}}')

# The JSON-RPC envelope is what says whether the call worked; the API payload it wraps is rendered
# as text and its shape is not this script's business. A failed tool call comes back as a normal
# result with isError set, which is also how a rejected caller bot token arrives here, so that flag
# is the check. Grepping the body for a pretty-printed '"code": 0' matched neither.
if RESULT_ERROR=$(printf '%s' "$RESULT" | python3 -c '
import json
import sys

payload = json.load(sys.stdin)
if payload.get("error"):
    print(json.dumps(payload["error"], ensure_ascii=False), file=sys.stderr)
    raise SystemExit(1)
result = payload.get("result")
if not isinstance(result, dict):
    print("tools/call response is missing result", file=sys.stderr)
    raise SystemExit(1)
if result.get("isError") is True or result.get("is_error") is True:
    print(json.dumps(result.get("content", []), ensure_ascii=False), file=sys.stderr)
    raise SystemExit(1)
' 2>&1); then
  echo "✅ projects.list: success"
else
  echo "❌ projects.list failed"
  echo "$RESULT_ERROR"
  exit 1
fi

# health
HEALTH=$(curl -s "$MCP_URL/health")
if [ -n "$HEALTH" ]; then
  echo "✅ health: $HEALTH"
else
  echo "⚠️  health endpoint not responding"
fi

echo "Done."
