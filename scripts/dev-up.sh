#!/usr/bin/env bash
set -euo pipefail

docker compose up -d postgres
echo "PostgreSQL started on 5432"
