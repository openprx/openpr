#!/usr/bin/env bash
set -euo pipefail

# OpenPR System Verification Script
# Quickly verify all components are working

echo "🔍 OpenPR System Verification"
echo "=============================="
echo ""

failed_checks=0

# Helper function for checks
check() {
  local name=$1
  local command=$2
  local expected=$3
  
  printf "%-40s " "$name..."
  
  if output=$(eval "$command" 2>&1); then
    if [ -n "$expected" ]; then
      if echo "$output" | grep -Eiq "$expected"; then
        echo "✅ PASS"
      else
        echo "❌ FAIL (output: $output)"
        ((failed_checks++))
      fi
    else
      echo "✅ PASS"
    fi
  else
    echo "❌ FAIL (error: $output)"
    ((failed_checks++))
  fi
}

compose_service_up() {
  local service=$1
  local container="openpr_${service}_1"
  podman inspect "$container" --format '{{.State.Status}}' 2>/dev/null | grep -q '^running$'
}

# Docker checks
echo "🐳 Docker Environment"
echo "---------------------"
check "Docker daemon running" "docker info > /dev/null" ""
check "Docker Compose available" "docker compose version" "version"
echo ""

# Service checks
echo "📦 Service Status"
echo "-----------------"
check "PostgreSQL container running" "compose_service_up postgres" ""
check "API container running" "compose_service_up api" ""
check "MCP Server container running" "compose_service_up mcp-server" ""
check "Frontend container running" "compose_service_up frontend" ""
echo ""

# Health checks
echo "🏥 Health Endpoints"
echo "-------------------"
check "API health check" "curl -s http://localhost:8081/health" "ok|healthy"
check "MCP server health check" "curl -s http://localhost:8090/health" "ok|healthy"
check "Frontend health check" "curl -s http://localhost:3000/health" "healthy"
check "Frontend accessible" "curl -s -o /dev/null -w '%{http_code}' http://localhost:3000" "200"
echo ""

# Database checks
echo "🗄️  Database Verification"
echo "-------------------------"
check "Database connection" "docker compose exec -T postgres psql -U openpr -d openpr -c '\q'" ""
check "Users table exists" "docker compose exec -T postgres psql -U openpr -d openpr -c '\dt' | grep -q users" ""
check "Workspaces table exists" "docker compose exec -T postgres psql -U openpr -d openpr -c '\dt' | grep -q workspaces" ""
check "Projects table exists" "docker compose exec -T postgres psql -U openpr -d openpr -c '\dt' | grep -q projects" ""
check "Work items table exists" "docker compose exec -T postgres psql -U openpr -d openpr -c '\dt' | grep -q work_items" ""
echo ""

# File checks
echo "📁 Configuration Files"
echo "----------------------"
check ".env file exists" "test -f .env" ""
check "docker-compose.yml exists" "test -f docker-compose.yml" ""
check "Migrations directory exists" "test -d migrations" ""
check "Scripts directory exists" "test -d scripts" ""
echo ""

# Port checks
echo "🔌 Port Availability"
echo "--------------------"
check "Port 8081 responding (API)" "curl -s -o /dev/null -w '%{http_code}' http://localhost:8081/health" "200"
check "Port 8090 responding (MCP)" "curl -s -o /dev/null -w '%{http_code}' http://localhost:8090/health" "200"
check "Port 3000 responding (Frontend)" "curl -s -o /dev/null -w '%{http_code}' http://localhost:3000" "200"
echo ""

# Summary
echo "=============================="
if [ $failed_checks -eq 0 ]; then
  echo "✅ All checks passed! System is healthy."
  echo ""
  echo "🚀 Your OpenPR installation is ready!"
  echo ""
  echo "📍 Access the application:"
  echo "  - Frontend:   http://localhost:3000"
  echo "  - API:        http://localhost:8081"
  echo "  - MCP Server: http://localhost:8090"
  exit 0
else
  echo "❌ $failed_checks check(s) failed."
  echo ""
  echo "🔧 Troubleshooting:"
  echo "  - Check logs: docker compose logs"
  echo "  - Restart services: docker compose restart"
  echo "  - See DEPLOYMENT.md for detailed troubleshooting"
  exit 1
fi
