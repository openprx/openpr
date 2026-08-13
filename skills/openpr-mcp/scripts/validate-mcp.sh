#!/bin/bash
# Quick smoke test for OpenPR MCP server connectivity
# Usage: ./validate-mcp.sh [http://localhost:8090] [project-uuid]
# Reads the MCP bearer token from the TOML configuration file (mcp.auth_token), by default
# config/openpr.compose.mcp.toml in the repository root; override the file with
# OPENPR_CONFIG_FILE, or the token itself with OPENPR_MCP_AUTH_TOKEN.

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

# /mcp/rpc, /sse and /messages require a bearer token when the server was configured with
# mcp.auth_token; /health is exempt. The token is never passed as an argument so it does not end
# up in the process list. OPENPR_MCP_AUTH_TOKEN is an operator override for whoever runs this
# script by hand; it is not application config, the binaries only read the TOML file.
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

echo "Testing MCP at $MCP_URL ..."

# tools/list
TOOLS_RESPONSE=$(curl -s -X POST "$MCP_URL/mcp/rpc" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $MCP_AUTH_TOKEN" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}')

TOOLS=$(printf '%s' "$TOOLS_RESPONSE" | \
  python3 -c "import sys,json; print(len(json.load(sys.stdin)['result']['tools']))" 2>/dev/null)

if [ "${TOOLS:-0}" -eq 105 ] 2>/dev/null; then
  echo "✅ tools/list: $TOOLS tools available"
else
  echo "❌ tools/list expected exactly 105 tools, got ${TOOLS:-0}"
  exit 1
fi

MISSING=$(printf '%s' "$TOOLS_RESPONSE" | python3 -c 'import json, sys
required = {
    "project_types.list",
    "project_types.get",
    "scenario_templates.list",
    "scenario_templates.get",
    "scenario_templates.install",
    "project_resources.list",
    "project_resources.create",
    "project_resources.update",
    "project_resources.delete",
    "connectors.list",
    "connectors.get",
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
    "invocations.list",
    "invocations.get",
    "invocations.create",
    "invocations.report_progress",
    "invocations.complete",
    "invocations.fail",
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
    -H "Authorization: Bearer $MCP_AUTH_TOKEN" \
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
  -H "Authorization: Bearer $MCP_AUTH_TOKEN" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"projects.list","arguments":{}}}')

if echo "$RESULT" | grep -q '"code": 0'; then
  echo "✅ projects.list: success"
else
  echo "❌ projects.list failed"
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
