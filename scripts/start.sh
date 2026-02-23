#!/bin/bash
set -e

echo "🚀 OpenPR Quick Start"
echo "===================="
echo ""

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_ROOT"

# Check if Docker is running
if ! docker info > /dev/null 2>&1; then
  echo "❌ Docker is not running. Please start Docker first."
  exit 1
fi

# Check if .env exists
if [ ! -f .env ]; then
  echo "📝 Creating .env from .env.example"
  cp .env.example .env
  echo "⚠️  Please review .env and update passwords before production use!"
  echo ""
fi

# Pull latest images (optional)
if [ "$1" = "--pull" ]; then
  echo "📥 Pulling latest base images..."
  docker compose pull
  echo ""
fi

# Build and start services
echo "🔨 Building and starting services..."
docker compose up -d --build

# Wait for services to be healthy
echo ""
echo "⏳ Waiting for services to be ready..."
max_wait=120
elapsed=0
while [ $elapsed -lt $max_wait ]; do
  healthy_count=$(docker compose ps | grep -c "healthy" || echo "0")
  
  if [ "$healthy_count" -ge 3 ]; then
    echo "✅ All services are healthy!"
    break
  fi
  
  printf "."
  sleep 2
  elapsed=$((elapsed + 2))
done

echo ""
echo ""

if [ $elapsed -ge $max_wait ]; then
  echo "⚠️  Timeout waiting for services. Checking status..."
  docker compose ps
  echo ""
  echo "Check logs with: docker compose logs"
  exit 1
fi

# Display service URLs
echo "🎉 OpenPR is ready!"
echo ""
echo "📍 Service URLs:"
echo "  - Frontend:   http://localhost:3000"
echo "  - API:        http://localhost:8080"
echo "  - MCP Server: http://localhost:8090"
echo "  - PostgreSQL: localhost:5432"
echo ""
echo "📊 Service Status:"
docker compose ps
echo ""
echo "📝 Useful Commands:"
echo "  - View logs:       docker compose logs -f"
echo "  - Stop services:   docker compose down"
echo "  - Restart:         docker compose restart"
echo "  - Run tests:       bash scripts/e2e-test.sh"
echo ""
