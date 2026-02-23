#!/bin/bash
set -e

echo "🛑 Stopping OpenPR Services"
echo "==========================="
echo ""

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_ROOT"

# Stop services
echo "📦 Stopping Docker Compose services..."
docker-compose down

echo ""
echo "✅ All services stopped"
echo ""
echo "📝 Note: Data is preserved in Docker volumes"
echo "To remove all data, use: bash scripts/clean.sh"
