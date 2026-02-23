#!/bin/bash
set -e

echo "🧹 OpenPR Clean Slate"
echo "===================="
echo ""
echo "⚠️  WARNING: This will remove:"
echo "  - All Docker containers"
echo "  - All Docker volumes (DATABASE DATA WILL BE LOST)"
echo "  - All Docker networks"
echo ""

read -p "Are you sure? (yes/no): " confirm

if [ "$confirm" != "yes" ]; then
  echo "❌ Clean cancelled"
  exit 0
fi

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_ROOT"

echo ""
echo "🗑️  Removing all services and data..."
docker-compose down -v --remove-orphans

echo ""
echo "✅ Clean complete"
echo ""
echo "🚀 To start fresh, run: bash scripts/start.sh"
