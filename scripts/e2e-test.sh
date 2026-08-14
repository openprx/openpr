#!/usr/bin/env bash
set -euo pipefail

# End-to-End Test Script
# Rebuilds the compose environment from scratch, runs all tests, and tears it
# down again.
#
# THIS SCRIPT DESTROYS AND RECREATES THE LOCAL TEST ENVIRONMENT.
# It runs `docker compose down -v` both before and after the test run, which
# deletes the `pgdata` volume of THIS compose project (the openpr stack defined
# by ./docker-compose.yml). Volumes belonging to any other project are never
# touched. The up-front reset is what makes the script repeatable: test-api.sh
# registers the very first account of an empty database, and registration is
# admin-only once a user exists, so a leftover database from a previous run
# would fail the second run.
#
# Environment variables:
#   OPENPR_E2E_ASSUME_YES=1  skip the interactive confirmation of the reset
#   OPENPR_E2E_KEEP_STACK=1  leave the stack running afterwards (for debugging);
#                            the next run still starts from a clean database

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_ROOT"

ASSUME_YES="${OPENPR_E2E_ASSUME_YES:-0}"
KEEP_STACK="${OPENPR_E2E_KEEP_STACK:-0}"

echo "🚀 Starting End-to-End Tests"
echo "Project Root: $PROJECT_ROOT"
echo ""

health_response_ok() {
  grep -Eiq "ok|healthy"
}

# Number of compose services currently reporting a healthy healthcheck.
# A bare `grep -c healthy` also matches "unhealthy", so match the parenthesised
# marker compose prints in the status column: "(unhealthy)" does not contain
# the literal "(healthy)".
# `|| echo 0` would be wrong here: on zero matches grep already prints "0" and
# then exits 1, so the fallback would append a second line and the comparison
# below would be handed "0\n0".
healthy_service_count() {
  local count
  count="$(docker compose ps | grep -cF '(healthy)' || true)"
  printf '%s' "${count:-0}"
}

unhealthy_service_present() {
  docker compose ps | grep -qF '(unhealthy)'
}

# Step 0: Reset the environment before starting, so the run does not depend on
# the previous run having reached its cleanup.
echo "🧨 Step 0: Resetting the test environment"
echo "   This removes the containers and the database volume of the openpr compose project."
if [ "$ASSUME_YES" != "1" ] && [ -t 0 ]; then
  read -r -p "   Type 'yes' to erase the local openpr compose data and continue: " reply
  if [ "$reply" != "yes" ]; then
    echo "❌ Aborted; nothing was changed."
    exit 1
  fi
fi
docker compose down -v --remove-orphans
echo "✅ Environment reset"
echo ""

# Cleanup function. Registered only after the reset was confirmed, so aborting
# at the prompt above cannot delete anything.
cleanup() {
  if [ "$KEEP_STACK" = "1" ]; then
    echo ""
    echo "ℹ️  OPENPR_E2E_KEEP_STACK=1; leaving the stack running."
    echo "   Tear it down with: docker compose down -v"
    return
  fi
  echo ""
  echo "🧹 Cleaning up..."
  docker compose down -v --remove-orphans
  echo "✅ Cleanup complete"
}

trap cleanup EXIT

# Step 1: Start Docker environment
echo "📦 Step 1: Starting Docker Compose"
bash "$PROJECT_ROOT/scripts/start.sh"

# Wait for services to be healthy
echo "⏳ Waiting for services to be ready..."
max_wait=120
elapsed=0
services_ready=0
while [ $elapsed -lt $max_wait ]; do
  if unhealthy_service_present; then
    echo "⚠️  Some services are unhealthy, waiting..."
  elif [ "$(healthy_service_count)" -ge 4 ]; then
    echo "✅ All services are healthy"
    services_ready=1
    break
  fi
  sleep 5
  elapsed=$((elapsed + 5))
done

if [ "$services_ready" -ne 1 ]; then
  echo "❌ Timeout waiting for services to be healthy"
  docker compose ps
  docker compose logs
  exit 1
fi

echo ""

# Step 2: Verify database migrations
echo "📋 Step 2: Verify Database Migrations"
if docker compose exec -T postgres psql -U openpr -d openpr -c "\dt" | grep -Eq "users|workspaces|projects"; then
  echo "✅ Database migrations applied successfully"
else
  echo "❌ Database migrations failed"
  exit 1
fi
echo ""

# Step 3: Run API tests
# The account is pinned rather than left random because step 3b needs to log in as it: the
# database was reset in step 0, so this registration is the first one and the account it creates
# is the admin, and registration is admin-only from here on.
E2E_ACCOUNT_EMAIL="e2e-$(date +%s)-$$@example.com"
E2E_ACCOUNT_PASSWORD="TestPassword123!"

echo "📋 Step 3: Running API Integration Tests"
OPENPR_TEST_EMAIL="$E2E_ACCOUNT_EMAIL" \
OPENPR_TEST_PASSWORD="$E2E_ACCOUNT_PASSWORD" \
bash "$PROJECT_ROOT/scripts/test-api.sh"
echo ""

# Step 3b: Give the MCP server a real workspace and a real bot to be reached as.
# There is no shared inbound secret any more: an MCP HTTP request is served as the bot whose token
# it presents, and that bot has to belong to the workspace the server is bound to through
# mcp.workspace_id. Step 1 wrote placeholders for both, which name nothing. This bootstrap creates
# the workspace and the bot through the API, writes the pair into
# config/openpr.compose.mcp.toml, and recreates the mcp-server container so it picks them up —
# after which step 4 can authenticate as that bot.
echo "📋 Step 3b: Seeding a workspace and the MCP bot the server is reached as"
OPENPR_DEMO_EMAIL="$E2E_ACCOUNT_EMAIL" \
OPENPR_DEMO_PASSWORD="$E2E_ACCOUNT_PASSWORD" \
OPENPR_DEMO_VERIFY_MCP_HTTP=1 \
bash "$PROJECT_ROOT/scripts/bootstrap-restaurant-demo.sh"
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
