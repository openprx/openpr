#!/bin/bash
set -e

# Database initialization script
# Runs all migration files in order

PGHOST="${PGHOST:-127.0.0.1}"
PGPORT="${PGPORT:-5432}"
PGDATABASE="${PGDATABASE:-openpr}"
PGUSER="${PGUSER:-openpr}"
PGPASSWORD="${PGPASSWORD:-openpr_dev_password}"

export PGPASSWORD

echo "🔍 Checking database connection..."
until psql -h "$PGHOST" -p "$PGPORT" -U "$PGUSER" -d "$PGDATABASE" -c '\q' 2>/dev/null; do
  echo "⏳ Waiting for PostgreSQL to be ready..."
  sleep 2
done

echo "✅ Database connection established"
echo ""

MIGRATION_DIR="$(dirname "$0")/../migrations"

# Run migrations in order
for migration in $(ls "$MIGRATION_DIR"/*.sql | sort); do
  filename=$(basename "$migration")
  echo "📋 Executing migration: $filename"
  psql -h "$PGHOST" -p "$PGPORT" -U "$PGUSER" -d "$PGDATABASE" -f "$migration"
  echo "✅ $filename completed"
  echo ""
done

echo "🎉 All migrations completed successfully!"
