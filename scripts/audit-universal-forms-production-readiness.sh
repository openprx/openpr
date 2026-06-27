#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PRODUCTION_DOC="$ROOT_DIR/docs/universal-forms-production.md"
COMPOSE_FILE="$ROOT_DIR/docker-compose.yml"
FRONTEND_NGINX="$ROOT_DIR/frontend/nginx.conf"
SOURCE_DOCKERFILE="$ROOT_DIR/Dockerfile"
PREBUILT_DOCKERFILE="$ROOT_DIR/Dockerfile.prebuilt"
FRONTEND_DOCKERFILE="$ROOT_DIR/frontend/Dockerfile"
WEBHOOK_EXAMPLE_CONFIG="$ROOT_DIR/config/openpr-webhook.example.toml"
START_SCRIPT="$ROOT_DIR/scripts/start.sh"
VERIFY_SCRIPT="$ROOT_DIR/scripts/verify.sh"
E2E_SCRIPT="$ROOT_DIR/scripts/e2e-test.sh"
BACKUP_SCRIPT="$ROOT_DIR/scripts/backup-db.sh"
RESTORE_SCRIPT="$ROOT_DIR/scripts/restore-db.sh"
STOP_SCRIPT="$ROOT_DIR/scripts/stop.sh"
CLEAN_SCRIPT="$ROOT_DIR/scripts/clean.sh"
TEST_API_SCRIPT="$ROOT_DIR/scripts/test-api.sh"
TEST_MCP_SCRIPT="$ROOT_DIR/scripts/test-mcp.sh"
BENCHMARK_SCRIPT="$ROOT_DIR/scripts/benchmark.sh"
PRODUCTION_AUTOMATION_SMOKE="$ROOT_DIR/scripts/smoke-universal-forms-production-automation.mjs"
PRODUCTION_OBJECT_STORAGE_SMOKE="$ROOT_DIR/scripts/smoke-universal-forms-production-object-storage.mjs"
PRODUCTION_ATTACHMENT_LIFECYCLE_SMOKE="$ROOT_DIR/scripts/smoke-universal-forms-production-attachment-lifecycle.mjs"
PRODUCTION_SIGNATURE_LIFECYCLE_SMOKE="$ROOT_DIR/scripts/smoke-universal-forms-production-signature-lifecycle.mjs"
CONTRIBUTING_DOC="$ROOT_DIR/CONTRIBUTING.md"
ROOT_README="$ROOT_DIR/README.md"
CI_WORKFLOW="$ROOT_DIR/.github/workflows/ci.yml"

usage() {
  cat <<'EOF'
Usage: scripts/audit-universal-forms-production-readiness.sh

Fast structural audit for production readiness of the universal forms business
platform path. This does not start containers; it verifies that production
documentation and runtime packaging still expose the required services,
health checks, proxies, and acceptance gates.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

failures=0
MCP_COMPOSE_BLOCK="$(mktemp)"
trap 'rm -f "$MCP_COMPOSE_BLOCK"' EXIT

check_file() {
  local path="$1"
  if [[ -f "$path" ]]; then
    printf 'PASS: file exists: %s\n' "${path#$ROOT_DIR/}"
  else
    printf 'FAIL: file missing: %s\n' "${path#$ROOT_DIR/}" >&2
    failures=$((failures + 1))
  fi
}

check_executable() {
  local path="$1"
  if [[ -x "$path" ]]; then
    printf 'PASS: executable file: %s\n' "${path#$ROOT_DIR/}"
  else
    printf 'FAIL: file not executable: %s\n' "${path#$ROOT_DIR/}" >&2
    failures=$((failures + 1))
  fi
}

contains() {
  local description="$1"
  local path="$2"
  local needle="$3"
  if rg -q --fixed-strings -- "$needle" "$path"; then
    printf 'PASS: %s\n' "$description"
  else
    printf 'FAIL: %s\n' "$description" >&2
    printf '  missing in %s: %s\n' "${path#$ROOT_DIR/}" "$needle" >&2
    failures=$((failures + 1))
  fi
}

not_contains() {
  local description="$1"
  local path="$2"
  local needle="$3"
  if rg -q --fixed-strings -- "$needle" "$path"; then
    printf 'FAIL: %s\n' "$description" >&2
    printf '  forbidden in %s: %s\n' "${path#$ROOT_DIR/}" "$needle" >&2
    failures=$((failures + 1))
  else
    printf 'PASS: %s\n' "$description"
  fi
}

not_contains_regex() {
  local description="$1"
  local path="$2"
  local pattern="$3"
  if rg -q -- "$pattern" "$path"; then
    printf 'FAIL: %s\n' "$description" >&2
    printf '  forbidden pattern in %s: %s\n' "${path#$ROOT_DIR/}" "$pattern" >&2
    failures=$((failures + 1))
  else
    printf 'PASS: %s\n' "$description"
  fi
}

workflow_step_contains() {
  local description="$1"
  local path="$2"
  local step_name="$3"
  local needle="$4"
  if awk -v step="      - name: ${step_name}" -v needle="$needle" '
    $0 == step { in_block = 1 }
    in_block && index($0, needle) { found = 1 }
    in_block && $0 ~ /^      - name: / && $0 != step { exit found ? 0 : 1 }
    END { exit found ? 0 : 1 }
  ' "$path"; then
    printf 'PASS: %s\n' "$description"
  else
    printf 'FAIL: %s\n' "$description" >&2
    printf '  missing in %s step %s: %s\n' "${path#$ROOT_DIR/}" "$step_name" "$needle" >&2
    failures=$((failures + 1))
  fi
}

write_compose_service_block() {
  local service="$1"
  local output_path="$2"
  awk -v service="  ${service}:" '
    $0 == service {
      in_block = 1
      print
      next
    }
    in_block && $0 ~ /^  [A-Za-z0-9_-]+:/ {
      exit
    }
    in_block {
      print
    }
  ' "$COMPOSE_FILE" > "$output_path"
}

printf 'Universal forms production readiness audit\n'
printf '  repo: %s\n' "$ROOT_DIR"
printf '\n'

for path in \
  "$PRODUCTION_DOC" \
  "$COMPOSE_FILE" \
  "$FRONTEND_NGINX" \
  "$SOURCE_DOCKERFILE" \
  "$PREBUILT_DOCKERFILE" \
  "$FRONTEND_DOCKERFILE" \
  "$WEBHOOK_EXAMPLE_CONFIG" \
  "$START_SCRIPT" \
  "$VERIFY_SCRIPT" \
  "$E2E_SCRIPT" \
  "$BACKUP_SCRIPT" \
  "$RESTORE_SCRIPT" \
  "$STOP_SCRIPT" \
  "$CLEAN_SCRIPT" \
  "$TEST_API_SCRIPT" \
  "$TEST_MCP_SCRIPT" \
  "$BENCHMARK_SCRIPT" \
  "$PRODUCTION_AUTOMATION_SMOKE" \
  "$PRODUCTION_OBJECT_STORAGE_SMOKE" \
  "$PRODUCTION_ATTACHMENT_LIFECYCLE_SMOKE" \
  "$PRODUCTION_SIGNATURE_LIFECYCLE_SMOKE" \
  "$CONTRIBUTING_DOC" \
  "$ROOT_README" \
  "$CI_WORKFLOW" \
  "$ROOT_DIR/scripts/ci-universal-forms-gates.sh"; do
  check_file "$path"
done
check_executable "$PRODUCTION_AUTOMATION_SMOKE"
check_executable "$PRODUCTION_OBJECT_STORAGE_SMOKE"
check_executable "$PRODUCTION_ATTACHMENT_LIFECYCLE_SMOKE"
check_executable "$PRODUCTION_SIGNATURE_LIFECYCLE_SMOKE"
check_executable "$ROOT_DIR/scripts/ci-universal-forms-gates.sh"
write_compose_service_block "mcp-server" "$MCP_COMPOSE_BLOCK"

printf '\nCompose service coverage:\n'
contains "compose defines PostgreSQL service" "$COMPOSE_FILE" "postgres:"
contains "compose defines API service" "$COMPOSE_FILE" "api:"
contains "compose defines worker service" "$COMPOSE_FILE" "worker:"
contains "compose defines MCP service" "$COMPOSE_FILE" "mcp-server:"
contains "compose defines frontend service" "$COMPOSE_FILE" "frontend:"
contains "compose defines optional webhook receiver" "$COMPOSE_FILE" "webhook:"
contains "compose uses PostgreSQL 16 image" "$COMPOSE_FILE" "image: postgres:16"
contains "compose mounts migrations into postgres" "$COMPOSE_FILE" "./migrations:/docker-entrypoint-initdb.d"
contains "compose keeps pgdata volume" "$COMPOSE_FILE" "pgdata:"
contains "PostgreSQL is exposed only to compose network" "$COMPOSE_FILE" "expose:"
not_contains "compose does not publish PostgreSQL to host" "$COMPOSE_FILE" '"5432:5432"'
contains "API waits for PostgreSQL health" "$COMPOSE_FILE" "condition: service_healthy"
contains "worker uses shared database URL" "$COMPOSE_FILE" 'DATABASE_URL: postgres://openpr:${POSTGRES_PASSWORD:?set POSTGRES_PASSWORD for PostgreSQL}@postgres:5432/openpr'
contains "API service passes object-storage backend env" "$COMPOSE_FILE" 'OPENPR_OBJECT_STORAGE_BACKEND: ${OPENPR_OBJECT_STORAGE_BACKEND:-local}'
contains "API service passes object-storage S3 endpoint env" "$COMPOSE_FILE" 'OPENPR_OBJECT_STORAGE_S3_ENDPOINT: ${OPENPR_OBJECT_STORAGE_S3_ENDPOINT:-}'
contains "API service passes object-storage S3 bucket env" "$COMPOSE_FILE" 'OPENPR_OBJECT_STORAGE_S3_BUCKET: ${OPENPR_OBJECT_STORAGE_S3_BUCKET:-}'
contains "worker service passes object-storage backend env" "$COMPOSE_FILE" 'OPENPR_OBJECT_STORAGE_BACKEND: ${OPENPR_OBJECT_STORAGE_BACKEND:-local}'
contains "worker service passes object-storage S3 endpoint env" "$COMPOSE_FILE" 'OPENPR_OBJECT_STORAGE_S3_ENDPOINT: ${OPENPR_OBJECT_STORAGE_S3_ENDPOINT:-}'
contains "worker service passes object-storage S3 bucket env" "$COMPOSE_FILE" 'OPENPR_OBJECT_STORAGE_S3_BUCKET: ${OPENPR_OBJECT_STORAGE_S3_BUCKET:-}'
contains "API host port binds localhost by default" "$COMPOSE_FILE" '"${OPENPR_BIND_HOST:-127.0.0.1}:${OPENPR_API_PORT:-8081}:8080"'
contains "MCP host port binds localhost by default" "$COMPOSE_FILE" '"${OPENPR_BIND_HOST:-127.0.0.1}:${MCP_SERVER_PORT:-8090}:8090"'
contains "frontend host port binds localhost by default" "$COMPOSE_FILE" '"${OPENPR_BIND_HOST:-127.0.0.1}:${OPENPR_FRONTEND_PORT:-3000}:80"'
not_contains "compose does not publish API on all interfaces" "$COMPOSE_FILE" '"8081:8080"'
not_contains "compose does not publish MCP on all interfaces" "$COMPOSE_FILE" '"8090:8090"'
not_contains "compose does not publish frontend on all interfaces" "$COMPOSE_FILE" '"3000:80"'
contains "frontend waits for API service health" "$COMPOSE_FILE" "condition: service_healthy"
not_contains "compose does not hardcode container names" "$COMPOSE_FILE" "container_name:"

printf '\nDatabase secret configuration coverage:\n'
contains "compose requires PostgreSQL password explicitly" "$COMPOSE_FILE" 'POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:?set POSTGRES_PASSWORD for PostgreSQL}'
contains "compose injects PostgreSQL password into service database URLs" "$COMPOSE_FILE" 'DATABASE_URL: postgres://openpr:${POSTGRES_PASSWORD:?set POSTGRES_PASSWORD for PostgreSQL}@postgres:5432/openpr'
contains "platform rejects placeholder database URL" "$ROOT_DIR/crates/platform/src/config.rs" "test_from_env_rejects_placeholder_database_url"
contains "env example documents PostgreSQL password placeholder" "$ROOT_DIR/.env.example" "POSTGRES_PASSWORD=replace_with_postgres_password"
not_contains "compose does not hardcode PostgreSQL password" "$COMPOSE_FILE" "POSTGRES_PASSWORD: openpr"
not_contains "compose does not hardcode service database password" "$COMPOSE_FILE" "DATABASE_URL: postgres://openpr:openpr@postgres:5432/openpr"
not_contains "env example does not enable default PostgreSQL password" "$ROOT_DIR/.env.example" "POSTGRES_PASSWORD=openpr"
not_contains "env example does not enable default database URL password" "$ROOT_DIR/.env.example" "DATABASE_URL=postgres://openpr:openpr@localhost:5432/openpr"

printf '\nOptional connector receiver coverage:\n'
contains "webhook receiver uses connectors profile" "$COMPOSE_FILE" "profiles:"
contains "webhook receiver image is configurable" "$COMPOSE_FILE" 'image: ${OPENPR_WEBHOOK_IMAGE:-ghcr.io/openprx/openpr-webhook:latest}'
contains "webhook receiver config path is configurable" "$COMPOSE_FILE" '${OPENPR_WEBHOOK_CONFIG:-./config/openpr-webhook.example.toml}:/etc/openpr-webhook/config.toml:ro'
contains "webhook receiver binds localhost by default" "$COMPOSE_FILE" '"${OPENPR_BIND_HOST:-127.0.0.1}:${OPENPR_WEBHOOK_PORT:-9090}:9090"'
contains "env example documents webhook receiver image" "$ROOT_DIR/.env.example" "OPENPR_WEBHOOK_IMAGE=ghcr.io/openprx/openpr-webhook:latest"
contains "env example documents webhook receiver config" "$ROOT_DIR/.env.example" "OPENPR_WEBHOOK_CONFIG=./config/openpr-webhook.example.toml"
contains "webhook example listens on compose service port" "$WEBHOOK_EXAMPLE_CONFIG" 'listen = "0.0.0.0:9090"'
contains "webhook example keeps unsigned webhooks disabled" "$WEBHOOK_EXAMPLE_CONFIG" "allow_unsigned = false"
contains "webhook example documents secret placeholder" "$WEBHOOK_EXAMPLE_CONFIG" "replace_with_openpr_webhook_secret"
contains "webhook example routes connector events" "$WEBHOOK_EXAMPLE_CONFIG" 'connector_kinds = ["webhook", "rest", "print", "device"]'
not_contains "compose does not hardcode local openpr-webhook binary path" "$COMPOSE_FILE" "/opt/opsx/openpr-webhook"
not_contains "webhook example does not use machine-specific paths" "$WEBHOOK_EXAMPLE_CONFIG" "/opt/"

printf '\nApplication secret configuration coverage:\n'
contains "compose requires JWT secret explicitly" "$COMPOSE_FILE" 'JWT_SECRET: ${JWT_SECRET:?set JWT_SECRET for OpenPR services}'
contains "platform rejects placeholder JWT secret" "$ROOT_DIR/crates/platform/src/config.rs" "test_from_env_rejects_placeholder_jwt_secret"
contains "env example documents JWT secret placeholder" "$ROOT_DIR/.env.example" "JWT_SECRET=replace_with_long_random_secret"
not_contains "compose does not default JWT secret to change-me" "$COMPOSE_FILE" 'JWT_SECRET: ${JWT_SECRET:-change-me-in-production}'
not_contains "env example does not enable weak JWT secret" "$ROOT_DIR/.env.example" "JWT_SECRET=change-me-in-production"

printf '\nFrontend API configuration coverage:\n'
contains "root env example uses current frontend API variable" "$ROOT_DIR/.env.example" "VITE_API_BASE_URL=http://localhost:8081"
contains "frontend env example points to compose API host port" "$ROOT_DIR/frontend/.env.example" "VITE_API_BASE_URL=http://localhost:8081"
contains "frontend client reads VITE_API_BASE_URL" "$ROOT_DIR/frontend/src/lib/api/client.ts" "import.meta.env.VITE_API_BASE_URL"
contains "frontend README documents compose API host port" "$ROOT_DIR/frontend/README.md" "VITE_API_BASE_URL=http://localhost:8081"
contains "frontend quickstart documents compose API host port" "$ROOT_DIR/frontend/QUICKSTART.md" "VITE_API_BASE_URL=http://localhost:8081"
not_contains "root env example does not use retired VITE_API_URL" "$ROOT_DIR/.env.example" "VITE_API_URL="
not_contains "root env example does not point frontend at retired API port" "$ROOT_DIR/.env.example" "VITE_API_BASE_URL=http://localhost:8080"
not_contains "frontend env example does not point browser at frontend port as API" "$ROOT_DIR/frontend/.env.example" "VITE_API_BASE_URL=http://localhost:3000"
not_contains "frontend README does not claim missing Vite proxy" "$ROOT_DIR/frontend/README.md" "Vite 的 proxy 功能"

printf '\nMCP production configuration coverage:\n'
contains "MCP API URL points to compose API service" "$COMPOSE_FILE" "OPENPR_API_URL: http://api:8080"
contains "MCP bot token must be supplied explicitly" "$COMPOSE_FILE" 'OPENPR_BOT_TOKEN: ${OPENPR_BOT_TOKEN:?set OPENPR_BOT_TOKEN for the MCP server}'
contains "MCP workspace must be supplied explicitly" "$COMPOSE_FILE" 'OPENPR_WORKSPACE_ID: ${OPENPR_WORKSPACE_ID:?set OPENPR_WORKSPACE_ID for the MCP server}'
contains "MCP server rejects placeholder config literals" "$ROOT_DIR/apps/mcp-server/src/main.rs" "rejects_compose_placeholder_literals"
contains "MCP server rejects demo token and nil workspace" "$ROOT_DIR/apps/mcp-server/src/main.rs" "rejects_demo_token_and_nil_workspace"
contains "MCP server default API URL targets compose host API port" "$ROOT_DIR/apps/mcp-server/src/main.rs" 'DEFAULT_OPENPR_API_URL: &str = "http://localhost:8081"'
not_contains "MCP server default API URL does not target frontend port" "$ROOT_DIR/apps/mcp-server/src/main.rs" '"http://localhost:3000"'
contains "MCP local HTTP default bind uses localhost" "$ROOT_DIR/apps/mcp-server/src/cli.rs" 'default_value = "127.0.0.1:8090"'
contains "MCP CLI tests pin localhost HTTP bind default" "$ROOT_DIR/apps/mcp-server/src/cli.rs" "http_transport_defaults_to_localhost_bind"
not_contains "README local MCP HTTP example does not bind all interfaces" "$ROOT_README" "mcp-server serve --transport http --bind-addr 0.0.0.0:8090"
not_contains "MCP app README local HTTP example does not bind all interfaces" "$ROOT_DIR/apps/mcp-server/README.md" "mcp-server serve --transport http --bind-addr 0.0.0.0:8090"
contains "API container default bind uses all interfaces" "$ROOT_DIR/apps/api/src/main.rs" 'AppConfig::from_env("api", "0.0.0.0:8081")'
not_contains "MCP compose service does not require direct database URL" "$MCP_COMPOSE_BLOCK" "DATABASE_URL:"
not_contains "MCP compose service does not require JWT secret" "$MCP_COMPOSE_BLOCK" "JWT_SECRET:"
not_contains "MCP compose service does not inherit default author id" "$MCP_COMPOSE_BLOCK" "DEFAULT_AUTHOR_ID:"
not_contains "MCP compose service does not depend directly on PostgreSQL" "$MCP_COMPOSE_BLOCK" "postgres:"
contains "env example documents MCP API URL" "$ROOT_DIR/.env.example" "OPENPR_API_URL=http://api:8080"
contains "env example documents MCP bot token placeholder" "$ROOT_DIR/.env.example" "OPENPR_BOT_TOKEN=opr_replace_with_workspace_bot_token"
contains "env example documents MCP workspace placeholder" "$ROOT_DIR/.env.example" "OPENPR_WORKSPACE_ID=00000000-0000-0000-0000-000000000000"
contains "README stdio MCP config targets API host port" "$ROOT_DIR/README.md" '"OPENPR_API_URL": "http://localhost:8081"'
contains "README MCP stdio config uses serve subcommand" "$ROOT_DIR/README.md" '"args": ["serve", "--transport", "stdio"]'
contains "MCP app README local examples target API host port" "$ROOT_DIR/apps/mcp-server/README.md" "OPENPR_API_URL=http://localhost:8081"
contains "MCP app README documents current tool count" "$ROOT_DIR/apps/mcp-server/README.md" "105 MCP Tools"
contains "MCP app README documents three transports" "$ROOT_DIR/apps/mcp-server/README.md" "Three Transport Modes"
contains "MCP app README documents SSE transport" "$ROOT_DIR/apps/mcp-server/README.md" "serve --transport sse"
contains "MCP app README documents universal forms tools" "$ROOT_DIR/apps/mcp-server/README.md" "Universal Forms and Events"
contains "MCP app README documents plugin tools" "$ROOT_DIR/apps/mcp-server/README.md" "WASM Plugins"
contains "MCP app README compose example uses prebuilt Dockerfile" "$ROOT_DIR/apps/mcp-server/README.md" "dockerfile: Dockerfile.prebuilt"
contains "MCP app README manual test starts API stack" "$ROOT_DIR/apps/mcp-server/README.md" "bash scripts/start.sh"
contains "MCP app README development guide uses API client helpers" "$ROOT_DIR/apps/mcp-server/README.md" "OpenPR API client helper"
contains "MCP app README performance section uses API request wording" "$ROOT_DIR/apps/mcp-server/README.md" "API Requests"
not_contains "MCP app README does not retain stale 65-tool count" "$ROOT_DIR/apps/mcp-server/README.md" "65 MCP Tools"
not_contains "MCP app README does not retain stale two-transport wording" "$ROOT_DIR/apps/mcp-server/README.md" "Two Transport Modes"
not_contains "MCP app README does not document hardcoded database password" "$ROOT_DIR/apps/mcp-server/README.md" "postgres://openpr:openpr"
not_contains "MCP app README quick start does not require database URL" "$ROOT_DIR/apps/mcp-server/README.md" "export DATABASE_URL"
not_contains "MCP app README quick start does not require JWT secret" "$ROOT_DIR/apps/mcp-server/README.md" "export JWT_SECRET"
not_contains "MCP app README does not describe MCP as PostgreSQL backend" "$ROOT_DIR/apps/mcp-server/README.md" "PostgreSQL Backend"
not_contains "MCP app README manual test does not start only PostgreSQL" "$ROOT_DIR/apps/mcp-server/README.md" "Start PostgreSQL"
not_contains "MCP app README project structure does not include direct db module" "$ROOT_DIR/apps/mcp-server/README.md" "src/db"
not_contains "MCP app README development guide does not add database modules" "$ROOT_DIR/apps/mcp-server/README.md" "Add database function"
not_contains "MCP app README performance section does not claim raw database queries" "$ROOT_DIR/apps/mcp-server/README.md" "Database Queries"
contains "MCP app README states MCP talks to API not database" "$ROOT_DIR/apps/mcp-server/README.md" "MCP talks to the OpenPR API, not directly to PostgreSQL"
contains "MCP app README documents bot token hash auth" "$ROOT_DIR/apps/mcp-server/README.md" "SHA-256 token hash"
contains "MCP app README documents workspace-scoped bot access" "$ROOT_DIR/apps/mcp-server/README.md" "a bot token can only act inside its workspace"
not_contains "MCP app README does not claim auth is unenforced" "$ROOT_DIR/apps/mcp-server/README.md" "Authentication infrastructure exists but is not enforced"
not_contains "MCP app README does not list stale JWT TODO" "$ROOT_DIR/apps/mcp-server/README.md" "Implement JWT token validation"
contains "MCP regression stdio path targets API host port" "$ROOT_DIR/skills/openpr-mcp/scripts/mcp-regression.py" '"OPENPR_API_URL":"http://localhost:8081"'
contains "MCP regression stdio path uses serve subcommand" "$ROOT_DIR/skills/openpr-mcp/scripts/mcp-regression.py" '[MCP_BIN,"serve","--transport","stdio"]'
contains "MCP regression checks current 105-tool registry" "$ROOT_DIR/skills/openpr-mcp/scripts/mcp-regression.py" "tools/list.registry_105"
contains "MCP validation requires exact 105 tools" "$ROOT_DIR/skills/openpr-mcp/scripts/validate-mcp.sh" "expected exactly 105 tools"
contains "MCP skill guide documents current tool count" "$ROOT_DIR/skills/openpr-mcp/SKILL.md" "enumerate all 105 tools"
not_contains "MCP skill guide does not retain stale 65-tool count" "$ROOT_DIR/skills/openpr-mcp/SKILL.md" "65 tools"
not_contains "MCP validation does not retain stale 65-tool minimum" "$ROOT_DIR/skills/openpr-mcp/scripts/validate-mcp.sh" "-ge 65"
contains "docs index documents current MCP server count" "$ROOT_DIR/docs/README.md" "MCP server (105 tools"
contains "docs index documents current MCP regression count" "$ROOT_DIR/docs/README.md" "105-tool registry"
contains "docs index links implementation map" "$ROOT_DIR/docs/README.md" "universal-forms-implementation-map.md"
contains "readiness summary generator links implementation map" "$ROOT_DIR/scripts/report-universal-forms-readiness-summary.sh" "universal-forms-implementation-map.md"
contains "user acceptance packet generator links implementation map" "$ROOT_DIR/scripts/prepare-universal-forms-user-acceptance-packet.sh" "universal-forms-implementation-map.md"
contains "delivery bundle audit runs implementation map verifier" "$ROOT_DIR/scripts/audit-universal-forms-delivery-bundle.sh" "implementation map verifier passes"
contains "delivery bundle audit runs implementation map contract smoke" "$ROOT_DIR/scripts/audit-universal-forms-delivery-bundle.sh" "implementation map contract smoke passes"
not_contains "docs index does not retain stale 64-tool count" "$ROOT_DIR/docs/README.md" "64-tool"
not_contains "docs index does not retain stale 64 MCP server count" "$ROOT_DIR/docs/README.md" "64 tools"
contains "README MCP HTTP example supplies API URL" "$ROOT_README" "OPENPR_API_URL=http://localhost:8081"
contains "README MCP HTTP example supplies bot token" "$ROOT_README" "OPENPR_BOT_TOKEN=opr_your_token_here"
contains "README MCP HTTP example uses serve subcommand" "$ROOT_README" "./target/release/mcp-server serve --transport http"
contains "MCP integration test uses current JSON-RPC endpoint" "$TEST_MCP_SCRIPT" "/mcp/rpc"
contains "MCP integration test lists tools through JSON-RPC" "$TEST_MCP_SCRIPT" '"method":"tools/list"'
contains "MCP integration test invokes tools through JSON-RPC" "$TEST_MCP_SCRIPT" '"method":"tools/call"'
contains "MCP integration test expects exact current tool count" "$TEST_MCP_SCRIPT" 'EXPECTED_TOOL_COUNT="${EXPECTED_TOOL_COUNT:-105}"'
contains "MCP integration test rejects tool count drift" "$TEST_MCP_SCRIPT" 'expected exactly $EXPECTED_TOOL_COUNT'
contains "MCP integration test checks form template tool" "$TEST_MCP_SCRIPT" "forms.create_from_template"
contains "MCP integration test checks scenario template install tool" "$TEST_MCP_SCRIPT" "scenario_templates.install"
contains "MCP integration test checks form schema version tools" "$TEST_MCP_SCRIPT" "form_schema_versions.list"
contains "MCP integration test checks form child lifecycle tools" "$TEST_MCP_SCRIPT" "form_records.child_archive"
contains "MCP integration test checks form import/export tools" "$TEST_MCP_SCRIPT" "form_records.import_commit"
contains "MCP integration test invokes stable projects.list tool" "$TEST_MCP_SCRIPT" "projects.list"
contains "MCP integration test checks plugin tools" "$TEST_MCP_SCRIPT" "plugin_invocations.list"
contains "MCP integration test accepts uppercase health OK" "$TEST_MCP_SCRIPT" "grep -Eiq"
not_contains "MCP integration test does not use retired tools/list endpoint" "$TEST_MCP_SCRIPT" "/v1/tools/list"
not_contains "MCP integration test does not use retired tools/call endpoint" "$TEST_MCP_SCRIPT" "/v1/tools/call"
not_contains "MCP integration test does not call removed workspace info tool" "$TEST_MCP_SCRIPT" "get_workspace_info"
not_contains "compose does not include old hardcoded MCP workspace UUID" "$COMPOSE_FILE" "e5166fd1-3bb7-46d9-b907-273b1eef3f44"
not_contains "compose does not route MCP to host frontend dev port" "$COMPOSE_FILE" "http://host.containers.internal:3000"
not_contains_regex "compose does not embed a literal MCP bot token" "$COMPOSE_FILE" 'OPENPR_BOT_TOKEN:[[:space:]]+opr_[[:alnum:]_]+'

printf '\nHealth and packaging coverage:\n'
contains "postgres has healthcheck" "$COMPOSE_FILE" "pg_isready -U openpr -d openpr"
contains "API has healthcheck" "$COMPOSE_FILE" "http://localhost:8080/health"
contains "MCP has healthcheck" "$COMPOSE_FILE" "http://localhost:8090/health"
contains "frontend has healthcheck" "$COMPOSE_FILE" "http://127.0.0.1/health"
contains "prebuilt image installs libpq" "$PREBUILT_DOCKERFILE" "libpq5"
contains "prebuilt image uses non-root user" "$PREBUILT_DOCKERFILE" "USER appuser"
contains "source-build image persists APP_BIN for runtime" "$SOURCE_DOCKERFILE" 'ENV APP_BIN=${APP_BIN}'
contains "source-build image installs libpq" "$SOURCE_DOCKERFILE" "libpq5"
contains "source-build image creates uploads directory" "$SOURCE_DOCKERFILE" "mkdir -p /app/uploads"
contains "source-build image marks binary executable" "$SOURCE_DOCKERFILE" 'chmod +x /app/${APP_BIN}'
contains "source-build image execs selected binary" "$SOURCE_DOCKERFILE" 'CMD ["exec /app/${APP_BIN}"]'
contains "source-build image uses non-root user" "$SOURCE_DOCKERFILE" "USER appuser"
contains "frontend image builds with frozen lockfile" "$FRONTEND_DOCKERFILE" "bun install --frozen-lockfile"
contains "frontend image runs nginx" "$FRONTEND_DOCKERFILE" "nginx"
contains "acceptance script uses resolved Bun for frontend check" "$ROOT_DIR/scripts/acceptance-universal-forms.sh" 'cd frontend && $FRONTEND_BUN run check'
contains "acceptance script uses resolved Bun for frontend smoke" "$ROOT_DIR/scripts/acceptance-universal-forms.sh" 'cd frontend && $FRONTEND_BUN run smoke:forms-ui'
contains "acceptance script resolves common Bun install path" "$ROOT_DIR/scripts/acceptance-universal-forms.sh" '$HOME/.bun/bin'
contains "UI artifact collector uses Bun for frontend build" "$ROOT_DIR/scripts/collect-universal-forms-ui-artifacts.sh" "FRONTEND_BUN"
contains "UI artifact collector uses Bun for frontend smoke" "$ROOT_DIR/scripts/collect-universal-forms-ui-artifacts.sh" "FRONTEND_BUN"
contains "UI artifact collector captures project template screenshots" "$ROOT_DIR/scripts/collect-universal-forms-ui-artifacts.sh" "OPENPR_PROJECT_TEMPLATE_SCREENSHOT_DIR"
contains "UI artifact collector captures template work item screenshots" "$ROOT_DIR/scripts/collect-universal-forms-ui-artifacts.sh" "OPENPR_TEMPLATE_WORK_ITEMS_SCREENSHOT_DIR"
contains "UI artifact collector prepares review gallery" "$ROOT_DIR/scripts/collect-universal-forms-ui-artifacts.sh" "prepare-universal-forms-ui-review-gallery.sh"
contains "UI artifact collector verifies review gallery" "$ROOT_DIR/scripts/collect-universal-forms-ui-artifacts.sh" "verify-universal-forms-ui-review-gallery.sh"
contains "UI artifact collector renders review gallery" "$ROOT_DIR/scripts/collect-universal-forms-ui-artifacts.sh" "smoke-universal-forms-ui-review-gallery-render.sh"
contains "UI artifact collector resolves common Bun install path" "$ROOT_DIR/scripts/collect-universal-forms-ui-artifacts.sh" '$HOME/.bun/bin'
not_contains "acceptance script does not use npm for frontend commands" "$ROOT_DIR/scripts/acceptance-universal-forms.sh" "npm run"
not_contains "UI artifact collector does not use npm for frontend commands" "$ROOT_DIR/scripts/collect-universal-forms-ui-artifacts.sh" "npm run"
contains "auth API exposes current profile update" "$ROOT_DIR/apps/api/src/routes/auth.rs" "update_profile"
contains "auth API exposes current password update" "$ROOT_DIR/apps/api/src/routes/auth.rs" "update_password"
contains "auth API exposes current preference update" "$ROOT_DIR/apps/api/src/routes/auth.rs" "update_preferences"
contains "frontend settings uses profile API" "$ROOT_DIR/frontend/src/routes/(app)/settings/+page.svelte" "authApi.updateProfile"
contains "frontend settings uses password API" "$ROOT_DIR/frontend/src/routes/(app)/settings/+page.svelte" "authApi.updatePassword"
contains "frontend settings uses preference API" "$ROOT_DIR/frontend/src/routes/(app)/settings/+page.svelte" "authApi.updatePreferences"
not_contains "frontend settings does not fake saves with timeout" "$ROOT_DIR/frontend/src/routes/(app)/settings/+page.svelte" "setTimeout"
not_contains "frontend English settings copy does not expose placeholder saves" "$ROOT_DIR/frontend/src/lib/i18n/en.json" "placeholder)"
not_contains "frontend Chinese settings copy does not expose placeholder saves" "$ROOT_DIR/frontend/src/lib/i18n/zh.json" "占位实现"
contains "start script builds release binaries before compose" "$START_SCRIPT" "cargo build --workspace --release"
contains "start script uses docker compose build" "$START_SCRIPT" "docker compose up -d --build"
contains "start script generates local PostgreSQL password" "$START_SCRIPT" "POSTGRES_PASSWORD"
contains "start script generates local JWT secret" "$START_SCRIPT" "JWT_SECRET"
contains "start script generates local MCP token" "$START_SCRIPT" "OPENPR_BOT_TOKEN"
contains "start script generates local workspace id" "$START_SCRIPT" "OPENPR_WORKSPACE_ID"
contains "start script documents API localhost port" "$START_SCRIPT" "http://localhost:8081"
contains "start script supports config-only validation" "$START_SCRIPT" "--check-config"
contains "start script rejects placeholder env values" "$START_SCRIPT" "is_placeholder_value"
contains "start script rejects nil workspace UUID" "$START_SCRIPT" "must not be the nil UUID placeholder"
contains "start script requires opr token prefix" "$START_SCRIPT" "must start with opr_"
contains "dev-up uses localhost-only PostgreSQL port override" "$ROOT_DIR/scripts/dev-up.sh" "127.0.0.1:\${POSTGRES_PORT}:5432"
contains "dev-up provides host DATABASE_URL" "$ROOT_DIR/scripts/dev-up.sh" "DATABASE_URL=postgres://openpr:\${POSTGRES_PASSWORD}@127.0.0.1:\${POSTGRES_PORT}/openpr"
contains "dev-up documents init-db command" "$ROOT_DIR/scripts/dev-up.sh" "bash scripts/init-db.sh"
contains "init-db defaults to localhost development database" "$ROOT_DIR/scripts/init-db.sh" 'PGHOST="${PGHOST:-127.0.0.1}"'
contains "init-db uses development password default" "$ROOT_DIR/scripts/init-db.sh" 'PGPASSWORD="${PGPASSWORD:-openpr_dev_password}"'
contains "restaurant demo bootstrap exists" "$ROOT_DIR/scripts/bootstrap-restaurant-demo.sh" "restaurant_ordering_default"
contains "restaurant demo bootstrap refuses remote API by default" "$ROOT_DIR/scripts/bootstrap-restaurant-demo.sh" "Refusing to seed a non-local API URL"
contains "restaurant demo bootstrap is documented as local-only" "$ROOT_DIR/scripts/bootstrap-restaurant-demo.sh" "not a production seeding tool"
contains "restaurant demo bootstrap creates MCP bot token" "$ROOT_DIR/scripts/bootstrap-restaurant-demo.sh" "Local Restaurant Demo MCP Bot"
contains "restaurant demo bootstrap writes MCP workspace env" "$ROOT_DIR/scripts/bootstrap-restaurant-demo.sh" "OPENPR_WORKSPACE_ID"
contains "restaurant demo bootstrap recreates running MCP compose service" "$ROOT_DIR/scripts/bootstrap-restaurant-demo.sh" "--force-recreate mcp-server"
contains "restaurant demo bootstrap verifies MCP HTTP projects.list" "$ROOT_DIR/scripts/bootstrap-restaurant-demo.sh" "projects.list"
contains "restaurant demo bootstrap supports required MCP HTTP verification" "$ROOT_DIR/scripts/bootstrap-restaurant-demo.sh" "OPENPR_DEMO_VERIFY_MCP_HTTP=1"
contains "restaurant demo MCP HTTP smoke uses temporary env file" "$ROOT_DIR/scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh" "OPENPR_DEMO_ENV_PATH"
contains "restaurant demo MCP HTTP smoke starts local MCP HTTP" "$ROOT_DIR/scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh" "serve --transport http"
contains "restaurant demo MCP HTTP smoke requires RESTDEMO through MCP" "$ROOT_DIR/scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh" "projects.list includes RESTDEMO"
contains "verify script uses docker compose v2" "$VERIFY_SCRIPT" "docker compose version"
contains "verify script checks API localhost port" "$VERIFY_SCRIPT" "http://localhost:8081/health"
contains "e2e script starts through bootstrap script" "$E2E_SCRIPT" 'bash "$PROJECT_ROOT/scripts/start.sh"'
contains "e2e script checks API localhost port" "$E2E_SCRIPT" "http://localhost:8081/health"
contains "e2e script accepts uppercase service health responses" "$E2E_SCRIPT" "health_response_ok"
contains "verify script accepts uppercase service health responses" "$VERIFY_SCRIPT" "grep -Eiq"
contains "backup script uses docker compose v2" "$BACKUP_SCRIPT" "docker compose exec -T postgres pg_dump"
contains "restore script uses docker compose v2" "$RESTORE_SCRIPT" "docker compose exec -T postgres psql"
contains "stop script uses docker compose v2" "$STOP_SCRIPT" "docker compose down"
contains "clean script uses docker compose v2" "$CLEAN_SCRIPT" "docker compose down -v --remove-orphans"
contains "API integration test defaults to compose API port" "$TEST_API_SCRIPT" 'API_URL="${API_URL:-http://localhost:8081}"'
contains "API integration test accepts uppercase service health responses" "$TEST_API_SCRIPT" "grep -Eiq"
contains "benchmark defaults to compose API port" "$BENCHMARK_SCRIPT" 'API_URL="${API_URL:-http://localhost:8081}"'

printf '\nContributor documentation coverage:\n'
contains "README uses Bun for frontend install" "$ROOT_README" "cd frontend && bun install && bun run dev"
contains "README uses Bun for frontend smoke commands" "$ROOT_README" "bun run smoke:forms-ui"
contains "README prerequisites mention Bun" "$ROOT_README" "Bun 1.3+"
contains "README exposes delivery acceptance state" "$ROOT_README" "## Delivery Acceptance State"
contains "README exposes current automated check count" "$ROOT_README" "Total automated checks: 27"
contains "README exposes pending manual signoff count" "$ROOT_README" "Manual signoff rows pending: 7"
contains "README exposes security scope audit" "$ROOT_README" "scripts/audit-universal-forms-security-scope.sh"
contains "README exposes signoff status JSON verifier" "$ROOT_README" "scripts/verify-universal-forms-signoff-status-json.sh"
contains "README exposes signoff dashboard generator" "$ROOT_README" "scripts/prepare-universal-forms-signoff-dashboard.sh"
contains "README exposes signoff dashboard verifier" "$ROOT_README" "scripts/verify-universal-forms-signoff-dashboard.sh"
contains "README exposes signoff dashboard render smoke" "$ROOT_README" "scripts/smoke-universal-forms-signoff-dashboard-render.sh"
contains "README exposes signoff dashboard progression smoke" "$ROOT_README" "scripts/smoke-universal-forms-signoff-dashboard-progression.sh"
contains "README exposes signoff status output smoke" "$ROOT_README" "scripts/smoke-universal-forms-signoff-status-output.sh"
contains "README explains signoff JSON evidence map fields" "$ROOT_README" "automated evidence and reviewer check"
contains "README explains signoff pending queue" "$ROOT_README" "pending_queue"
contains "README exposes next signoff review verifier" "$ROOT_README" "scripts/verify-universal-forms-next-signoff-review.sh"
contains "README exposes next signoff review contract smoke" "$ROOT_README" "scripts/smoke-universal-forms-next-signoff-review-contract.sh"
contains "README exposes next signoff command smoke" "$ROOT_README" "scripts/smoke-universal-forms-next-signoff-command.sh"
contains "README exposes manual signoff progression smoke" "$ROOT_README" "scripts/smoke-universal-forms-manual-signoff-progression.sh"
contains "README exposes all manual signoff command smoke" "$ROOT_README" "scripts/smoke-universal-forms-manual-signoff-commands.sh"
contains "README exposes delivery status command" "$ROOT_README" "scripts/status-universal-forms-delivery.sh"
contains "README exposes delivery status JSON verifier" "$ROOT_README" "scripts/verify-universal-forms-delivery-status-json.sh"
contains "README exposes delivery status output smoke" "$ROOT_README" "scripts/smoke-universal-forms-delivery-status-output.sh"
contains "README exposes delivery status JSON schema" "$ROOT_README" "docs/schemas/openpr-universal-forms-delivery-status.schema.json"
contains "README exposes delivery status completion summary" "$ROOT_README" "completion_summary"
contains "README exposes delivery status completion breakdown" "$ROOT_README" "completion_breakdown"
contains "README exposes delivery status release blockers" "$ROOT_README" "release_blockers"
contains "README exposes delivery status next actions" "$ROOT_README" "next_actions"
contains "README exposes delivery status next reviewer check" "$ROOT_README" "next row's automated evidence, reviewer check"
contains "README exposes total handoff progress fields" "$ROOT_README" "total handoff items completed/total/remaining/percent"
contains "README exposes delivery status manual signoff queue" "$ROOT_README" "manual_signoff_queue"
contains "README exposes delivery status review surfaces" "$ROOT_README" "review_surfaces"
contains "README exposes completion audit JSON verifier" "$ROOT_README" "scripts/verify-universal-forms-completion-audit-json.sh"
contains "README exposes completion audit JSON contract smoke" "$ROOT_README" "scripts/smoke-universal-forms-completion-audit-json-contract.sh"
contains "README exposes completion audit JSON schema" "$ROOT_README" "docs/schemas/openpr-universal-forms-completion-audit.schema.json"
contains "README exposes implementation map JSON verifier" "$ROOT_README" "scripts/verify-universal-forms-implementation-map-json.sh"
contains "README exposes implementation map JSON contract smoke" "$ROOT_README" "scripts/smoke-universal-forms-implementation-map-json-contract.sh"
contains "README exposes implementation map JSON schema" "$ROOT_README" "docs/schemas/openpr-universal-forms-implementation-map.schema.json"
contains "README exposes release gate handoff command" "$ROOT_README" "scripts/gate-universal-forms-release.sh --allow-pending"
contains "README exposes release gate JSON verifier" "$ROOT_README" "scripts/verify-universal-forms-release-gate-json.sh"
contains "README exposes release gate JSON contract smoke" "$ROOT_README" "scripts/smoke-universal-forms-release-gate-json-contract.sh"
contains "README exposes release gate output smoke" "$ROOT_README" "scripts/smoke-universal-forms-release-gate-output.sh"
contains "README exposes release gate JSON schema" "$ROOT_README" "docs/schemas/openpr-universal-forms-release-gate.schema.json"
contains "README exposes delivery bundle audit" "$ROOT_README" "scripts/audit-universal-forms-delivery-bundle.sh"
contains "README exposes finalizer boundary" "$ROOT_README" "scripts/finalize-universal-forms-acceptance.sh"
contains "README documents universal forms CI gates" "$ROOT_README" "Universal Forms Gates"
contains "README exposes local universal forms CI wrapper" "$ROOT_README" "scripts/ci-universal-forms-gates.sh"
contains "README documents CI cargo-audit warning boundary" "$ROOT_README" "workspace warning-as-error policy"
contains "CI defines universal forms gate job" "$CI_WORKFLOW" "universal-forms:"
contains "CI names universal forms gates" "$CI_WORKFLOW" "Universal Forms Gates"
contains "CI installs cargo machete" "$CI_WORKFLOW" "cargo install cargo-machete --locked"
contains "CI clears warning-as-error boundary for cargo-machete install" "$CI_WORKFLOW" 'RUSTFLAGS: ""'
workflow_step_contains "CI clears RUSTFLAGS inside cargo-machete install step" "$CI_WORKFLOW" "Unused deps" 'RUSTFLAGS: ""'
contains "CI installs cargo audit" "$CI_WORKFLOW" "cargo install cargo-audit --locked"
contains "CI clears warning-as-error boundary for cargo-audit install" "$CI_WORKFLOW" 'RUSTFLAGS: ""'
workflow_step_contains "CI clears RUSTFLAGS inside cargo-audit install step" "$CI_WORKFLOW" "Install audit tools" 'RUSTFLAGS: ""'
contains "CI installs jq and ripgrep" "$CI_WORKFLOW" "sudo apt-get install -y jq ripgrep"
contains "CI uses universal forms gate wrapper" "$CI_WORKFLOW" "bash scripts/ci-universal-forms-gates.sh"
contains "CI wrapper runs PostgreSQL-only security scope audit" "$ROOT_DIR/scripts/ci-universal-forms-gates.sh" "scripts/audit-universal-forms-security-scope.sh"
contains "CI wrapper runs universal forms source coverage audit" "$ROOT_DIR/scripts/ci-universal-forms-gates.sh" "scripts/audit-universal-forms-source-coverage.sh"
contains "CI wrapper runs universal forms production readiness audit" "$ROOT_DIR/scripts/ci-universal-forms-gates.sh" "scripts/audit-universal-forms-production-readiness.sh"
contains "CONTRIBUTING uses docker compose v2" "$CONTRIBUTING_DOC" "docker compose exec postgres"
contains "CONTRIBUTING uses dev-up for database-only startup" "$CONTRIBUTING_DOC" "bash scripts/dev-up.sh"
contains "CONTRIBUTING explains dev-up localhost database" "$CONTRIBUTING_DOC" "temporary localhost-only port override"
contains "CONTRIBUTING frontend checks use available scripts" "$CONTRIBUTING_DOC" "bun run check && bun run build"
not_contains "CONTRIBUTING does not use retired docker-compose command" "$CONTRIBUTING_DOC" "docker-compose"
not_contains "CONTRIBUTING does not suggest nonexistent frontend test script" "$CONTRIBUTING_DOC" "bun run test"
not_contains "CONTRIBUTING does not publish PostgreSQL with default openpr password" "$CONTRIBUTING_DOC" "POSTGRES_PASSWORD=openpr"

printf '\nFrontend proxy coverage:\n'
contains "nginx uses Podman DNS resolver for service discovery" "$FRONTEND_NGINX" "resolver 10.89.3.1"
contains "nginx defines API upstream alias" "$FRONTEND_NGINX" 'set $api_upstream api:8080'
contains "nginx defines MCP upstream alias" "$FRONTEND_NGINX" 'set $mcp_upstream mcp-server:8090'
contains "nginx proxies API through resolved upstream alias" "$FRONTEND_NGINX" 'proxy_pass http://$api_upstream'
contains "nginx proxies MCP through resolved upstream alias" "$FRONTEND_NGINX" 'proxy_pass http://$mcp_upstream'
not_contains "nginx does not depend on API container_name alias" "$FRONTEND_NGINX" "openpr-api:8080"
not_contains "nginx does not depend on MCP container_name alias" "$FRONTEND_NGINX" "openpr-mcp-server:8090"
contains "nginx exposes health endpoint" "$FRONTEND_NGINX" "return 200 \"healthy"
contains "nginx supports SvelteKit fallback" "$FRONTEND_NGINX" 'try_files $uri $uri/ /index.html'

printf '\nProduction runbook coverage:\n'
contains "runbook states minimum production services" "$PRODUCTION_DOC" "Minimum production services:"
contains "runbook states worker is required for connector delivery" "$PRODUCTION_DOC" "If the worker is not running"
contains "runbook states runtime configuration section" "$PRODUCTION_DOC" "## Runtime Configuration"
contains "runbook states PostgreSQL password must be concrete" "$PRODUCTION_DOC" '`POSTGRES_PASSWORD` and `DATABASE_URL` must use a concrete database password'
contains "runbook states PostgreSQL is internal-only" "$PRODUCTION_DOC" "PostgreSQL is exposed only inside the compose network"
contains "runbook states app ports bind localhost by default" "$PRODUCTION_DOC" "host ports bind"
contains "runbook states reverse proxy or tunnel requirement" "$PRODUCTION_DOC" "reverse proxy or tunnel"
contains "runbook states compose avoids fixed container names" "$PRODUCTION_DOC" 'avoids fixed `container_name` values'
contains "runbook states webhook example config" "$PRODUCTION_DOC" "config/openpr-webhook.example.toml"
contains "runbook states webhook production secret replacement" "$PRODUCTION_DOC" 'set a concrete `webhook_secrets` value'
contains "runbook states local compose bootstrap script" "$PRODUCTION_DOC" "bash scripts/start.sh"
contains "runbook states local restaurant demo bootstrap" "$PRODUCTION_DOC" "scripts/bootstrap-restaurant-demo.sh"
contains "runbook states local demo writes MCP credentials" "$PRODUCTION_DOC" 'writes `OPENPR_BOT_TOKEN` and'
contains "runbook states local demo recreates MCP service" "$PRODUCTION_DOC" 'recreates a running compose `mcp-server`'
contains "runbook states local demo verifies MCP HTTP" "$PRODUCTION_DOC" 'verifies `/mcp/rpc` with `projects.list`'
contains "runbook states restaurant demo is not production seeding" "$PRODUCTION_DOC" "not a production data seeding path"
contains "runbook includes security scope audit" "$PRODUCTION_DOC" "scripts/audit-universal-forms-security-scope.sh"
contains "runbook documents universal forms CI gates" "$PRODUCTION_DOC" "Universal Forms Gates"
contains "runbook exposes local universal forms CI wrapper" "$PRODUCTION_DOC" "scripts/ci-universal-forms-gates.sh"
contains "runbook documents CI cargo-audit warning boundary" "$PRODUCTION_DOC" "workspace warning-as-error policy"
contains "runbook documents CI source coverage gate" "$PRODUCTION_DOC" "scripts/audit-universal-forms-source-coverage.sh"
contains "runbook documents CI production readiness gate" "$PRODUCTION_DOC" "scripts/audit-universal-forms-production-readiness.sh"
contains "runbook documents SQLx inactive MySQL boundary" "$PRODUCTION_DOC" "inactive MySQL backend scope"
contains "runbook states prebuilt compose needs release build" "$PRODUCTION_DOC" "Dockerfile.prebuilt"
contains "runbook states webhook receiver uses connectors profile" "$PRODUCTION_DOC" '`connectors` profile'
contains "runbook states JWT secret must be concrete" "$PRODUCTION_DOC" '`JWT_SECRET` must be a concrete deployment secret'
contains "runbook states MCP compose API URL" "$PRODUCTION_DOC" "OPENPR_API_URL=http://api:8080"
contains "runbook states MCP credentials are production workspace scoped" "$PRODUCTION_DOC" "token and workspace ID must be issued for the production workspace"
contains "runbook states PostgreSQL-only path" "$PRODUCTION_DOC" "Production is PostgreSQL-only for this delivery path"
contains "runbook includes event outbox audit requirement" "$PRODUCTION_DOC" 'Verify `event_outbox` has no growing backlog'
contains "runbook includes restaurant acceptance scenario" "$PRODUCTION_DOC" 'Use `restaurant_ordering_default`'
contains "runbook includes MCP smoke" "$PRODUCTION_DOC" "scripts/smoke-forms-mcp.sh"
contains "runbook includes production automation smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-production-automation.mjs"
contains "runbook documents real external automation endpoint" "$PRODUCTION_DOC" "OPENPR_AUTOMATION_ENDPOINT"
contains "runbook documents automation receipt callback" "$PRODUCTION_DOC" "POST /api/v1/invocations/{invocation_id}/receipt"
contains "runbook documents automation inbox diagnostics" "$PRODUCTION_DOC" "invocation-scoped and form-scoped inbox diagnostics"
contains "runbook documents automation receiver 2xx-before-receipt ordering" "$PRODUCTION_DOC" "return a 2xx response to the worker dispatch before it"
contains "runbook includes production object-storage smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-production-object-storage.mjs"
contains "runbook documents expected object-storage backend assertion" "$PRODUCTION_DOC" "OPENPR_EXPECT_OBJECT_STORAGE_BACKEND"
contains "runbook documents object-storage S3 endpoint env" "$PRODUCTION_DOC" "OPENPR_OBJECT_STORAGE_S3_ENDPOINT"
contains "runbook documents object-storage S3 bucket env" "$PRODUCTION_DOC" "OPENPR_OBJECT_STORAGE_S3_BUCKET"
contains "runbook documents upload storage backend field" "$PRODUCTION_DOC" "storage_backend"
contains "runbook documents upload object key field" "$PRODUCTION_DOC" "object_key"
contains "runbook documents upload thumbnail field" "$PRODUCTION_DOC" "thumbnail_url"
contains "runbook documents object-storage import-file acceptance" "$PRODUCTION_DOC" "import-file"
contains "runbook documents attachment package job acceptance" "$PRODUCTION_DOC" "attachment package job"
contains "runbook includes production attachment lifecycle smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-production-attachment-lifecycle.mjs"
contains "runbook documents attachment archive restore lifecycle" "$PRODUCTION_DOC" "archives the attachment"
contains "runbook documents archived attachment signed download rejection" "$PRODUCTION_DOC" "signed download rejects archived attachments"
contains "runbook includes production signature lifecycle smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-production-signature-lifecycle.mjs"
contains "runbook documents signature audit verification endpoint" "$PRODUCTION_DOC" "/api/v1/form-records/{record_id}/signatures/audit-verification"
contains "runbook documents signature lifecycle summary" "$PRODUCTION_DOC" "signature_lifecycle"
contains "runbook documents signature workflow verification decision" "$PRODUCTION_DOC" "signature_workflow_verification.status=verified"
contains "runbook includes connector smoke" "$PRODUCTION_DOC" "scripts/smoke-webhook-generic-consumer.sh"
contains "runbook includes WASM smoke" "$PRODUCTION_DOC" "scripts/smoke-wasm-plugin-runtime.sh"
contains "runbook includes frontend smoke" "$PRODUCTION_DOC" "bun run smoke:forms-ui"
contains "runbook includes project template frontend smoke" "$PRODUCTION_DOC" "bun run smoke:project-template"
contains "runbook includes template work item frontend smoke" "$PRODUCTION_DOC" "bun run smoke:template-work-items"
contains "runbook includes UI review gallery verification" "$PRODUCTION_DOC" "scripts/verify-universal-forms-ui-review-gallery.sh"
contains "runbook includes UI review gallery render smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-ui-review-gallery-render.sh"
contains "runbook includes readiness summary generation" "$PRODUCTION_DOC" "scripts/report-universal-forms-readiness-summary.sh"
contains "runbook includes delivery manifest generation" "$PRODUCTION_DOC" "scripts/prepare-universal-forms-delivery-manifest.sh"
contains "runbook includes delivery manifest verifier" "$PRODUCTION_DOC" "scripts/verify-universal-forms-delivery-manifest.sh"
contains "runbook includes readiness JSON generation" "$PRODUCTION_DOC" "scripts/report-universal-forms-readiness-json.sh"
contains "runbook includes readiness JSON verifier" "$PRODUCTION_DOC" "scripts/verify-universal-forms-readiness-json.sh"
contains "runbook documents readiness JSON signoff status JSON link" "$PRODUCTION_DOC" "reports.signoff_status_json"
contains "runbook includes signoff status JSON generation" "$PRODUCTION_DOC" "scripts/report-universal-forms-signoff-status-json.sh"
contains "runbook includes signoff status JSON verifier" "$PRODUCTION_DOC" "scripts/verify-universal-forms-signoff-status-json.sh"
contains "runbook includes signoff status JSON contract smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-signoff-status-json-contract.sh"
contains "runbook includes signoff dashboard generator" "$PRODUCTION_DOC" "scripts/prepare-universal-forms-signoff-dashboard.sh"
contains "runbook includes signoff dashboard verifier" "$PRODUCTION_DOC" "scripts/verify-universal-forms-signoff-dashboard.sh"
contains "runbook includes signoff dashboard render smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-signoff-dashboard-render.sh"
contains "runbook includes signoff dashboard progression smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-signoff-dashboard-progression.sh"
contains "runbook includes signoff status output smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-signoff-status-output.sh"
contains "runbook includes next signoff review verifier" "$PRODUCTION_DOC" "scripts/verify-universal-forms-next-signoff-review.sh"
contains "runbook includes next signoff review contract smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-next-signoff-review-contract.sh"
contains "runbook includes next signoff command smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-next-signoff-command.sh"
contains "runbook includes manual signoff progression smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-manual-signoff-progression.sh"
contains "runbook includes all manual signoff command smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-manual-signoff-commands.sh"
contains "runbook includes delivery status command" "$PRODUCTION_DOC" "scripts/status-universal-forms-delivery.sh"
contains "runbook includes delivery status JSON verifier" "$PRODUCTION_DOC" "scripts/verify-universal-forms-delivery-status-json.sh"
contains "runbook includes delivery status JSON contract smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-delivery-status-json-contract.sh"
contains "runbook includes delivery status output smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-delivery-status-output.sh"
contains "runbook links signoff status JSON schema" "$PRODUCTION_DOC" "docs/schemas/openpr-universal-forms-signoff-status.schema.json"
contains "runbook documents signoff status JSON final flag" "$PRODUCTION_DOC" "final_signoff_allowed"
contains "runbook explains signoff JSON evidence map fields" "$PRODUCTION_DOC" "automated evidence and reviewer check"
contains "runbook explains signoff pending queue" "$PRODUCTION_DOC" "pending_queue"
contains "runbook includes development status JSON generation" "$PRODUCTION_DOC" "scripts/report-universal-forms-development-status-json.sh"
contains "runbook includes development status JSON verifier" "$PRODUCTION_DOC" "scripts/verify-universal-forms-development-status-json.sh"
contains "runbook includes development status JSON contract smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-development-status-json-contract.sh"
contains "runbook links development status JSON schema" "$PRODUCTION_DOC" "docs/schemas/openpr-universal-forms-development-status.schema.json"
contains "runbook includes scenario catalog JSON generation" "$PRODUCTION_DOC" "scripts/report-universal-forms-scenario-catalog-json.sh"
contains "runbook includes scenario catalog JSON verifier" "$PRODUCTION_DOC" "scripts/verify-universal-forms-scenario-catalog-json.sh"
contains "runbook includes scenario catalog JSON contract smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-scenario-catalog-json-contract.sh"
contains "runbook links scenario catalog JSON schema" "$PRODUCTION_DOC" "docs/schemas/openpr-universal-forms-scenario-catalog.schema.json"
contains "runbook documents scenario catalog operator entrypoints" "$PRODUCTION_DOC" "operator entrypoints"
contains "runbook documents scenario catalog connector kinds" "$PRODUCTION_DOC" "connector kinds"
contains "runbook documents runtime scenario usage guide" "$PRODUCTION_DOC" "usage_guide"
contains "runbook documents scenario template MCP resource" "$PRODUCTION_DOC" "openpr://scenario-templates"
contains "runbook includes implementation map JSON generation" "$PRODUCTION_DOC" "scripts/report-universal-forms-implementation-map-json.sh"
contains "runbook includes implementation map JSON verifier" "$PRODUCTION_DOC" "scripts/verify-universal-forms-implementation-map-json.sh"
contains "runbook includes implementation map JSON contract smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-implementation-map-json-contract.sh"
contains "runbook links implementation map JSON schema" "$PRODUCTION_DOC" "docs/schemas/openpr-universal-forms-implementation-map.schema.json"
contains "runbook includes delivery manifest JSON verifier" "$PRODUCTION_DOC" "scripts/verify-universal-forms-delivery-manifest-json.sh"
contains "runbook includes delivery manifest JSON contract smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-delivery-manifest-json-contract.sh"
contains "runbook includes report output boundary smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-report-output-boundaries.sh"
contains "runbook explains report output boundary smoke" "$PRODUCTION_DOC" "The report output boundary smoke is the production guard for generated handoff"
contains "runbook explains signoff status JSON stdout mode" "$PRODUCTION_DOC" "report-universal-forms-signoff-status-json.sh --output -"
contains "runbook explains signoff status JSON stdout alias" "$PRODUCTION_DOC" "equivalent \`--stdout\` alias"
contains "runbook explains stdout mode does not leave dash file" "$PRODUCTION_DOC" 'without leaving a repository-root `-`'
contains "runbook includes delivery bundle audit" "$PRODUCTION_DOC" "scripts/audit-universal-forms-delivery-bundle.sh"
not_contains "production runbook does not use npm frontend commands" "$PRODUCTION_DOC" "npm run"
contains "runbook includes finalizer" "$PRODUCTION_DOC" "scripts/finalize-universal-forms-acceptance.sh"
contains "runbook includes completion audit JSON verifier" "$PRODUCTION_DOC" "scripts/verify-universal-forms-completion-audit-json.sh"
contains "runbook includes completion audit JSON contract smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-completion-audit-json-contract.sh"
contains "runbook links completion audit JSON schema" "$PRODUCTION_DOC" "docs/schemas/openpr-universal-forms-completion-audit.schema.json"
contains "runbook includes release gate" "$PRODUCTION_DOC" "scripts/gate-universal-forms-release.sh"
contains "runbook includes release gate JSON verifier" "$PRODUCTION_DOC" "scripts/verify-universal-forms-release-gate-json.sh"
contains "runbook includes release gate JSON contract smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-release-gate-json-contract.sh"
contains "runbook includes release gate smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-release-gate.sh"
contains "runbook includes release gate output smoke" "$PRODUCTION_DOC" "scripts/smoke-universal-forms-release-gate-output.sh"
contains "runbook links release gate JSON schema" "$PRODUCTION_DOC" "docs/schemas/openpr-universal-forms-release-gate.schema.json"
contains "runbook explains delivery status JSON mode" "$PRODUCTION_DOC" "scripts/status-universal-forms-delivery.sh --json"
contains "runbook explains delivery status JSON verifier" "$PRODUCTION_DOC" "Verify it with"
contains "runbook explains delivery status completion summary" "$PRODUCTION_DOC" "completion_summary"
contains "runbook explains delivery status completion breakdown" "$PRODUCTION_DOC" "completion_breakdown"
contains "runbook explains delivery status release blockers" "$PRODUCTION_DOC" "release_blockers"
contains "runbook explains delivery status next actions" "$PRODUCTION_DOC" "next_actions"
contains "runbook explains delivery status next reviewer check" "$PRODUCTION_DOC" "current row's automated evidence, reviewer check"
contains "runbook explains total handoff progress fields" "$PRODUCTION_DOC" "Overall handoff progress: 27 / 34 complete, 7 remaining"
contains "runbook explains delivery status manual signoff queue" "$PRODUCTION_DOC" "manual_signoff_queue"
contains "runbook explains delivery status review surfaces" "$PRODUCTION_DOC" "review_surfaces"

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms production readiness audit failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms production readiness audit passed.\n'
