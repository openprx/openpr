#!/bin/bash
# Quick smoke test for OpenPR MCP server connectivity
# Usage: ./validate-mcp.sh [http://localhost:8090]

MCP_URL="${1:-http://localhost:8090}"

echo "Testing MCP at $MCP_URL ..."

# tools/list
TOOLS=$(curl -s -X POST "$MCP_URL/mcp/rpc" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | \
  python3 -c "import sys,json; print(len(json.load(sys.stdin)['result']['tools']))" 2>/dev/null)

if [ "$TOOLS" -gt 0 ] 2>/dev/null; then
  echo "✅ tools/list: $TOOLS tools available"
else
  echo "❌ tools/list failed"
  exit 1
fi

# projects.list
RESULT=$(curl -s -X POST "$MCP_URL/mcp/rpc" \
  -H "Content-Type: application/json" \
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
