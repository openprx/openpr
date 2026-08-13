#!/usr/bin/env bash
set -euo pipefail

echo "🚀 OpenPR Quick Start"
echo "===================="
echo ""

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_ROOT"
MODE="${1:-}"

random_hex() {
  if command -v openssl >/dev/null 2>&1; then
    openssl rand -hex "$1"
  else
    od -An -N "$1" -tx1 /dev/urandom | tr -d ' \n'
  fi
}

random_uuid() {
  if command -v uuidgen >/dev/null 2>&1; then
    uuidgen | tr '[:upper:]' '[:lower:]'
  else
    printf '%s-%s-%s-%s-%s\n' \
      "$(random_hex 4)" \
      "$(random_hex 2)" \
      "$(random_hex 2)" \
      "$(random_hex 2)" \
      "$(random_hex 6)"
  fi
}

upsert_env() {
  local key="$1"
  local value="$2"
  if grep -qE "^#?${key}=" .env; then
    sed -i.bak -E "s|^#?${key}=.*|${key}=${value}|" .env
    rm -f .env.bak
  else
    printf '%s=%s\n' "$key" "$value" >> .env
  fi
}

env_value() {
  local key="$1"
  grep -E "^${key}=" .env | tail -n 1 | cut -d= -f2-
}

is_placeholder_value() {
  local value="$1"
  [[ -z "$value" || "$value" == *'${'* || "$value" == *replace_with* || "$value" == "change-me-in-production" ]]
}

validate_concrete_value() {
  local key="$1"
  local value="$2"
  if is_placeholder_value "$value"; then
    echo "❌ .env contains a placeholder or empty value for $key"
    return 1
  fi
}

validate_uuid_value() {
  local key="$1"
  local value="$2"
  validate_concrete_value "$key" "$value" || return 1
  if [[ ! "$value" =~ ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$ ]]; then
    echo "❌ .env value for $key must be a UUID"
    return 1
  fi
  if [[ "$value" == "00000000-0000-0000-0000-000000000000" ]]; then
    echo "❌ .env value for $key must not be the nil UUID placeholder"
    return 1
  fi
}

# Check if .env exists
if [ ! -f .env ]; then
  echo "📝 Creating .env from .env.example"
  cp .env.example .env
  upsert_env "POSTGRES_PASSWORD" "local_$(random_hex 16)"
  upsert_env "JWT_SECRET" "local_$(random_hex 32)"
  upsert_env "OPENPR_BOT_TOKEN" "opr_local_$(random_hex 24)"
  upsert_env "OPENPR_WORKSPACE_ID" "$(random_uuid)"
  upsert_env "DEFAULT_AUTHOR_ID" "$(random_uuid)"
  upsert_env "OPENPR_MCP_AUTH_TOKEN" "$(random_hex 32)"
  echo "⚠️  Local bootstrap secrets were generated in .env. Replace them before production use."
  echo ""
fi

# An .env written before the MCP server gained inbound authentication has no token, and compose
# now refuses to start mcp-server without one. Only mcp-server reads this value, so generating it
# here cannot invalidate an already provisioned database or bot token.
if ! grep -qE "^OPENPR_MCP_AUTH_TOKEN=.+" .env; then
  upsert_env "OPENPR_MCP_AUTH_TOKEN" "$(random_hex 32)"
  echo "🔐 Generated OPENPR_MCP_AUTH_TOKEN in .env (value not printed)."
  echo ""
fi

required_vars=(
  POSTGRES_PASSWORD
  JWT_SECRET
  OPENPR_BOT_TOKEN
  OPENPR_WORKSPACE_ID
  OPENPR_MCP_AUTH_TOKEN
)

missing_vars=()
for key in "${required_vars[@]}"; do
  if ! grep -qE "^${key}=.+" .env; then
    missing_vars+=("$key")
  fi
done

if [ "${#missing_vars[@]}" -ne 0 ]; then
  echo "❌ .env is missing required compose values: ${missing_vars[*]}"
  echo "Set them in .env or remove .env and rerun this script to generate local bootstrap values."
  exit 1
fi

config_errors=0
for key in POSTGRES_PASSWORD JWT_SECRET; do
  if ! validate_concrete_value "$key" "$(env_value "$key")"; then
    config_errors=$((config_errors + 1))
  fi
done

bot_token="$(env_value "OPENPR_BOT_TOKEN")"
if ! validate_concrete_value "OPENPR_BOT_TOKEN" "$bot_token"; then
  config_errors=$((config_errors + 1))
elif [[ "$bot_token" != opr_* ]]; then
  echo "❌ .env value for OPENPR_BOT_TOKEN must start with opr_"
  config_errors=$((config_errors + 1))
fi

mcp_auth_token="$(env_value "OPENPR_MCP_AUTH_TOKEN")"
if ! validate_concrete_value "OPENPR_MCP_AUTH_TOKEN" "$mcp_auth_token"; then
  config_errors=$((config_errors + 1))
elif [ "${#mcp_auth_token}" -lt 16 ]; then
  echo "❌ .env value for OPENPR_MCP_AUTH_TOKEN must be at least 16 characters"
  config_errors=$((config_errors + 1))
fi

if ! validate_uuid_value "OPENPR_WORKSPACE_ID" "$(env_value "OPENPR_WORKSPACE_ID")"; then
  config_errors=$((config_errors + 1))
fi

if grep -qE "^DEFAULT_AUTHOR_ID=" .env; then
  if ! validate_uuid_value "DEFAULT_AUTHOR_ID" "$(env_value "DEFAULT_AUTHOR_ID")"; then
    config_errors=$((config_errors + 1))
  fi
fi

if [ "$config_errors" -ne 0 ]; then
  echo "Remove .env and rerun this script to regenerate local bootstrap values, or replace the invalid values manually."
  exit 1
fi

if [ "$MODE" = "--check-config" ]; then
  echo "✅ .env bootstrap configuration is valid."
  exit 0
fi

# Check if Docker is running
if ! docker info > /dev/null 2>&1; then
  echo "❌ Docker is not running. Please start Docker first."
  exit 1
fi

# Pull latest images (optional)
if [ "$MODE" = "--pull" ]; then
  echo "📥 Pulling latest base images..."
  docker compose pull
  echo ""
fi

# Build and start services
echo "🔨 Building release binaries for Dockerfile.prebuilt..."
cargo build --workspace --release

# The binaries are linked against the host glibc, so the runtime image has to be
# at least as new. Derive it from the host release unless the caller pinned one.
if [ -z "${OPENPR_RUNTIME_BASE:-}" ] && [ -r /etc/os-release ]; then
  host_id=$(. /etc/os-release && echo "${ID:-}")
  host_codename=$(. /etc/os-release && echo "${VERSION_CODENAME:-}")
  if [ "$host_id" = "debian" ] && [ -n "$host_codename" ]; then
    OPENPR_RUNTIME_BASE="debian:${host_codename}-slim"
    export OPENPR_RUNTIME_BASE
    echo "   runtime base image: $OPENPR_RUNTIME_BASE (matched to host)"
    # Persist it so a plain `docker compose up --build` keeps the same base.
    if ! grep -q '^OPENPR_RUNTIME_BASE=' .env 2>/dev/null; then
      printf '\n# Runtime base image for Dockerfile.prebuilt; must ship a glibc at least as new as the host.\nOPENPR_RUNTIME_BASE=%s\n' "$OPENPR_RUNTIME_BASE" >> .env
    fi
  fi
fi

echo "🔨 Building and starting services..."
docker compose up -d --build

# Wait for services to be healthy
echo ""
echo "⏳ Waiting for services to be ready..."
max_wait=120
elapsed=0
while [ $elapsed -lt $max_wait ]; do
  healthy_count=$(docker compose ps | grep -c "healthy" || echo "0")
  
  if [ "$healthy_count" -ge 4 ]; then
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
echo "  - API:        http://localhost:8081"
echo "  - MCP Server: http://localhost:8090"
echo "  - PostgreSQL: compose network only"
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
