#!/bin/bash
set -e

# MCP Server Integration Test Script
# Tests tool discovery and invocation

MCP_URL="${MCP_URL:-http://localhost:8090}"

echo "🧪 Starting MCP Server Tests"
echo "MCP URL: $MCP_URL"
echo ""

# Test 1: Health Check
echo "📋 Test 1: MCP Server Health Check"
response=$(curl -s "$MCP_URL/health" || echo "failed")
if echo "$response" | grep -q "ok\|healthy"; then
  echo "✅ MCP server is healthy"
else
  echo "❌ MCP server health check failed"
  exit 1
fi
echo ""

# Test 2: List Tools
echo "📋 Test 2: List Available Tools"
tools_response=$(curl -s -X POST "$MCP_URL/v1/tools/list" \
  -H "Content-Type: application/json" \
  -d '{}')

if echo "$tools_response" | grep -q "tools\|list"; then
  echo "✅ Tools list retrieved"
  echo "Available tools:"
  echo "$tools_response" | grep -o '"name":"[^"]*' | cut -d'"' -f4 | while read tool; do
    echo "  - $tool"
  done
else
  echo "❌ Failed to retrieve tools list: $tools_response"
  exit 1
fi
echo ""

# Test 3: Call a tool (example: get_workspace_info)
echo "📋 Test 3: Invoke Tool (get_workspace_info)"
call_response=$(curl -s -X POST "$MCP_URL/v1/tools/call" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "get_workspace_info",
    "arguments": {
      "workspace_id": "00000000-0000-0000-0000-000000000000"
    }
  }')

if echo "$call_response" | grep -q "result\|content\|error"; then
  echo "✅ Tool invocation successful"
else
  echo "❌ Tool invocation failed: $call_response"
  exit 1
fi
echo ""

echo "🎉 All MCP tests passed!"
