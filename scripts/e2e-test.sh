#!/usr/bin/env bash
set -euo pipefail

# End-to-End Test Script
# Starts Docker environment, runs all tests, and cleans up

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_ROOT"

echo "🚀 Starting End-to-End Tests"
echo "Project Root: $PROJECT_ROOT"
echo ""

# Cleanup function
cleanup() {
  echo ""
  echo "🧹 Cleaning up..."
  docker compose down -v
  echo "✅ Cleanup complete"
}

# Set trap to cleanup on exit
trap cleanup EXIT

health_response_ok() {
  grep -Eiq "ok|healthy"
}

# Step 1: Start Docker environment
echo "📦 Step 1: Starting Docker Compose"
bash "$PROJECT_ROOT/scripts/start.sh"

# Wait for services to be healthy
echo "⏳ Waiting for services to be ready..."
max_wait=120
elapsed=0
while [ $elapsed -lt $max_wait ]; do
  if docker compose ps | grep -q "unhealthy"; then
    echo "⚠️  Some services are unhealthy, waiting..."
    sleep 5
    elapsed=$((elapsed + 5))
  else
    healthy_count=$(docker compose ps | grep -c "healthy" || echo "0")
    if [ "$healthy_count" -ge 4 ]; then
      echo "✅ All services are healthy"
      break
    fi
    sleep 5
    elapsed=$((elapsed + 5))
  fi
done

if [ $elapsed -ge $max_wait ]; then
  echo "❌ Timeout waiting for services to be healthy"
  docker compose ps
  docker compose logs
  exit 1
fi

echo ""

# Step 2: Verify database migrations
echo "📋 Step 2: Verify Database Migrations"
docker compose exec -T postgres psql -U openpr -d openpr -c "\dt" | grep -q "users\|workspaces\|projects"
if [ $? -eq 0 ]; then
  echo "✅ Database migrations applied successfully"
else
  echo "❌ Database migrations failed"
  exit 1
fi
echo ""

# Step 3: Run API tests
echo "📋 Step 3: Running API Integration Tests"
bash "$PROJECT_ROOT/scripts/test-api.sh"
echo ""

# Step 4: Run MCP tests
echo "📋 Step 4: Running MCP Server Tests"
bash "$PROJECT_ROOT/scripts/test-mcp.sh"
echo ""

# Step 5: Verify frontend is accessible
echo "📋 Step 5: Verify Frontend Accessibility"
frontend_response=$(curl -s -o /dev/null -w "%{http_code}" http://localhost:3000/)
if [ "$frontend_response" = "200" ]; then
  echo "✅ Frontend is accessible"
else
  echo "❌ Frontend is not accessible (HTTP $frontend_response)"
  exit 1
fi
echo ""

# Step 6: Verify all health endpoints
echo "📋 Step 6: Verify All Health Endpoints"

# API health
api_health=$(curl -s http://localhost:8081/health)
if echo "$api_health" | health_response_ok; then
  echo "✅ API health check passed"
else
  echo "❌ API health check failed"
  echo "Response: $api_health"
  exit 1
fi

# MCP health
mcp_health=$(curl -s http://localhost:8090/health)
if echo "$mcp_health" | health_response_ok; then
  echo "✅ MCP server health check passed"
else
  echo "❌ MCP server health check failed"
  echo "Response: $mcp_health"
  exit 1
fi

# Frontend health
frontend_health=$(curl -s http://localhost:3000/health)
if echo "$frontend_health" | grep -Eiq "healthy"; then
  echo "✅ Frontend health check passed"
else
  echo "❌ Frontend health check failed"
  echo "Response: $frontend_health"
  exit 1
fi

echo ""
echo "🎉 All End-to-End Tests Passed!"
echo ""
echo "📊 Test Summary:"
echo "  - Docker Compose: ✅"
echo "  - Database Migrations: ✅"
echo "  - API Integration: ✅"
echo "  - MCP Server: ✅"
echo "  - Frontend: ✅"
echo "  - Health Checks: ✅"
echo ""
