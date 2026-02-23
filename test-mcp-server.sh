#!/bin/bash
# MCP Server 测试脚本

set -e

cd /opt/worker/code/openpr

# 检查 .env 文件是否存在
if [ ! -f .env ]; then
    echo "Error: .env file not found"
    echo "Please create .env file with DATABASE_URL and JWT_SECRET"
    exit 1
fi

# 加载环境变量
export $(cat .env | xargs)

# 测试 1: 启动服务器并测试 tools/list
echo "Test 1: List all tools"
echo '{"jsonrpc": "2.0", "id": 1, "method": "tools/list"}' | timeout 5 cargo run -q -p mcp-server -- --transport stdio 2>&1 | grep -v "warning:" | tail -1 | jq '.'

echo ""
echo "Test 2: Initialize"
echo '{"jsonrpc": "2.0", "id": 2, "method": "initialize"}' | timeout 5 cargo run -q -p mcp-server -- --transport stdio 2>&1 | grep -v "warning:" | tail -1 | jq '.'

echo ""
echo "✅ MCP Server tests completed!"
