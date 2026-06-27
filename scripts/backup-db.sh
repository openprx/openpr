#!/usr/bin/env bash
set -euo pipefail

# Database Backup Script

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BACKUP_DIR="$PROJECT_ROOT/backups"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BACKUP_FILE="$BACKUP_DIR/openpr_backup_$TIMESTAMP.sql.gz"

# Create backup directory
mkdir -p "$BACKUP_DIR"

echo "💾 OpenPR Database Backup"
echo "========================"
echo ""
echo "Backup file: $BACKUP_FILE"
echo ""

# Check if PostgreSQL container is running
if ! docker compose ps postgres | grep -q "Up"; then
  echo "❌ PostgreSQL container is not running"
  exit 1
fi

# Create backup
echo "📦 Creating backup..."
docker compose exec -T postgres pg_dump -U openpr openpr | gzip > "$BACKUP_FILE"

# Check if backup was successful
if [ -f "$BACKUP_FILE" ] && [ -s "$BACKUP_FILE" ]; then
  size=$(du -h "$BACKUP_FILE" | cut -f1)
  echo "✅ Backup completed successfully"
  echo "📊 Backup size: $size"
  echo ""
  
  # Keep only last 7 backups
  echo "🧹 Cleaning old backups (keeping last 7)..."
  cd "$BACKUP_DIR"
  ls -t openpr_backup_*.sql.gz | tail -n +8 | xargs -r rm
  
  echo "✅ Cleanup complete"
  echo ""
  echo "📁 Available backups:"
  ls -lh openpr_backup_*.sql.gz 2>/dev/null || echo "  (none)"
else
  echo "❌ Backup failed"
  exit 1
fi
