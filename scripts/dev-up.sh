#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
POSTGRES_PORT="${OPENPR_DEV_POSTGRES_PORT:-5432}"
POSTGRES_PASSWORD="${OPENPR_DEV_POSTGRES_PASSWORD:-openpr_dev_password}"
override_file="$(mktemp)"
trap 'rm -f "$override_file"' EXIT

cat >"$override_file" <<YAML
services:
  postgres:
    ports:
      - "127.0.0.1:${POSTGRES_PORT}:5432"
YAML

POSTGRES_PASSWORD="$POSTGRES_PASSWORD" \
  docker compose -f "$ROOT_DIR/docker-compose.yml" -f "$override_file" up -d postgres

echo "PostgreSQL started for local development."
echo "  Host: 127.0.0.1"
echo "  Port: ${POSTGRES_PORT}"
echo "  DATABASE_URL=postgres://openpr:${POSTGRES_PASSWORD}@127.0.0.1:${POSTGRES_PORT}/openpr"
echo "  PGPASSWORD=${POSTGRES_PASSWORD} PGHOST=127.0.0.1 PGPORT=${POSTGRES_PORT} bash scripts/init-db.sh"
