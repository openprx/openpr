#!/usr/bin/env bash
set -euo pipefail

# Database Restore Script

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BACKUP_DIR="$PROJECT_ROOT/backups"

echo "♻️  OpenPR Database Restore"
echo "=========================="
echo ""

# Check for backup file argument
if [ -z "${1:-}" ]; then
  echo "📁 Available backups:"
  ls -lh "$BACKUP_DIR"/openpr_backup_*.sql.gz 2>/dev/null || echo "  (none)"
  echo ""
  echo "Usage: $0 <backup_file>"
  echo "Example: $0 backups/openpr_backup_20240115_103000.sql.gz"
  exit 1
fi

BACKUP_FILE="$1"

# Check if backup file exists
if [ ! -f "$BACKUP_FILE" ]; then
  echo "❌ Backup file not found: $BACKUP_FILE"
  exit 1
fi

# Check if PostgreSQL container is running
if ! docker compose ps postgres | grep -q "Up"; then
  echo "❌ PostgreSQL container is not running"
  echo "Start it with: docker compose up -d postgres"
  exit 1
fi

echo "⚠️  WARNING: This will replace all data in the database!"
echo "Backup file: $BACKUP_FILE"
echo ""
read -p "Are you sure? (yes/no): " confirm

if [ "$confirm" != "yes" ]; then
  echo "❌ Restore cancelled"
  exit 0
fi

echo ""
echo "📦 Restoring database..."

# Drop and recreate database
docker compose exec -T postgres psql -U openpr -d postgres -c "DROP DATABASE IF EXISTS openpr;"
docker compose exec -T postgres psql -U openpr -d postgres -c "CREATE DATABASE openpr;"

# Restore from backup
gunzip -c "$BACKUP_FILE" | docker compose exec -T postgres psql -U openpr -d openpr

echo "✅ Restore completed successfully"
echo ""
echo "🔄 Restart services to apply changes:"
echo "  docker compose restart api worker mcp-server"
