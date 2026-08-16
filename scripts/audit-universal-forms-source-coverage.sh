#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'EOF'
Usage: scripts/audit-universal-forms-source-coverage.sh

Audits that the universal forms delivery surface claimed by the tracker still
has concrete source-code, migration, frontend, MCP, worker, and smoke-test
entrypoints. This is a fast structural gate; behavior is covered by the smoke
and acceptance scripts.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

failures=0

check_file() {
  local path="$1"
  if [[ -f "$ROOT_DIR/$path" ]]; then
    printf 'PASS: file exists: %s\n' "$path"
  else
    printf 'FAIL: file missing: %s\n' "$path" >&2
    failures=$((failures + 1))
  fi
}

check_executable() {
  local path="$1"
  if [[ -x "$ROOT_DIR/$path" ]]; then
    printf 'PASS: executable file: %s\n' "$path"
  else
    printf 'FAIL: file not executable: %s\n' "$path" >&2
    failures=$((failures + 1))
  fi
}

contains() {
  local description="$1"
  local path="$2"
  local needle="$3"
  if rg -q --fixed-strings -- "$needle" "$ROOT_DIR/$path"; then
    printf 'PASS: %s\n' "$description"
  else
    printf 'FAIL: %s\n' "$description" >&2
    printf '  missing in %s: %s\n' "$path" "$needle" >&2
    failures=$((failures + 1))
  fi
}

contains_regex() {
  local description="$1"
  local path="$2"
  local pattern="$3"
  if rg -q -- "$pattern" "$ROOT_DIR/$path"; then
    printf 'PASS: %s\n' "$description"
  else
    printf 'FAIL: %s\n' "$description" >&2
    printf '  missing pattern in %s: %s\n' "$path" "$pattern" >&2
    failures=$((failures + 1))
  fi
}

not_contains() {
  local description="$1"
  local path="$2"
  local needle="$3"
  if rg -q --fixed-strings -- "$needle" "$ROOT_DIR/$path"; then
    printf 'FAIL: %s\n' "$description" >&2
    printf '  forbidden in %s: %s\n' "$path" "$needle" >&2
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
  ' "$ROOT_DIR/$path"; then
    printf 'PASS: %s\n' "$description"
  else
    printf 'FAIL: %s\n' "$description" >&2
    printf '  missing in %s step %s: %s\n' "$path" "$step_name" "$needle" >&2
    failures=$((failures + 1))
  fi
}

printf 'Universal forms source coverage audit\n'
printf '  repo: %s\n' "$ROOT_DIR"
printf '\n'

printf 'CI gate coverage:\n'
check_file ".github/workflows/ci.yml"
check_file "scripts/ci-universal-forms-gates.sh"
check_executable "scripts/ci-universal-forms-gates.sh"
contains "CI defines universal forms gate job" ".github/workflows/ci.yml" "universal-forms:"
contains "CI installs cargo machete" ".github/workflows/ci.yml" "cargo install cargo-machete --locked"
contains "CI clears warning-as-error boundary for cargo-machete install" ".github/workflows/ci.yml" 'RUSTFLAGS: ""'
workflow_step_contains "CI clears RUSTFLAGS inside cargo-machete install step" ".github/workflows/ci.yml" "Unused deps" 'RUSTFLAGS: ""'
contains "CI clears warning-as-error boundary for cargo-audit install" ".github/workflows/ci.yml" 'RUSTFLAGS: ""'
workflow_step_contains "CI clears RUSTFLAGS inside cargo-audit install step" ".github/workflows/ci.yml" "Install audit tools" 'RUSTFLAGS: ""'
contains "CI uses universal forms gate wrapper" ".github/workflows/ci.yml" "bash scripts/ci-universal-forms-gates.sh"
contains "CI wrapper runs PostgreSQL-only security scope audit" "scripts/ci-universal-forms-gates.sh" "scripts/audit-universal-forms-security-scope.sh"
contains "CI wrapper runs universal forms source coverage audit" "scripts/ci-universal-forms-gates.sh" "scripts/audit-universal-forms-source-coverage.sh"
contains "CI wrapper runs universal forms production readiness audit" "scripts/ci-universal-forms-gates.sh" "scripts/audit-universal-forms-production-readiness.sh"

printf 'Migration and backend source files:\n'
for path in \
  migrations/0030_universal_forms.sql \
  migrations/0031_business_events_outbox_inbox.sql \
  migrations/0032_print_device_connector_kinds.sql \
  migrations/0033_plugins_wasm.sql \
  migrations/0034_user_profile_preferences.sql \
  migrations/0038_form_permissions.sql \
  apps/api/src/forms/mod.rs \
  apps/api/src/forms/schema.rs \
  apps/api/src/forms/values.rs \
  apps/api/src/forms/decimal.rs \
  apps/api/src/forms/validation.rs \
  apps/api/src/forms/projections.rs \
  apps/api/src/plugins/manifest.rs \
  apps/api/src/plugins/runtime.rs \
  apps/api/src/plugins/hooks.rs \
  apps/api/src/events/mod.rs \
  apps/api/src/routes/form.rs \
  apps/api/src/routes/plugin.rs \
  apps/api/src/routes/connector.rs \
  apps/api/src/routes/webhook.rs \
  apps/api/src/routes/check_result.rs \
  apps/api/src/routes/project_type.rs \
  apps/api/src/routes/scenario_template.rs \
  apps/api/src/services/invocation_service.rs \
  apps/worker/src/main.rs; do
  check_file "$path"
done

printf '\nDatabase contract coverage:\n'
contains "universal forms migration creates project_forms" "migrations/0030_universal_forms.sql" "CREATE TABLE IF NOT EXISTS project_forms"
contains "universal forms migration creates form_records" "migrations/0030_universal_forms.sql" "CREATE TABLE IF NOT EXISTS form_records"
contains "universal forms migration creates form_record_links" "migrations/0030_universal_forms.sql" "CREATE TABLE IF NOT EXISTS form_record_links"
contains "universal forms migration creates decimal projection index" "migrations/0030_universal_forms.sql" "value_decimal"
contains "event migration creates business_events" "migrations/0031_business_events_outbox_inbox.sql" "CREATE TABLE IF NOT EXISTS business_events"
contains "event migration creates event_outbox" "migrations/0031_business_events_outbox_inbox.sql" "CREATE TABLE IF NOT EXISTS event_outbox"
contains "event migration creates event_inbox" "migrations/0031_business_events_outbox_inbox.sql" "CREATE TABLE IF NOT EXISTS event_inbox"
contains "print/device migration includes print connector kind" "migrations/0032_print_device_connector_kinds.sql" "print"
contains "print/device migration includes device connector kind" "migrations/0032_print_device_connector_kinds.sql" "device"
contains "plugin migration creates plugins table" "migrations/0033_plugins_wasm.sql" "CREATE TABLE IF NOT EXISTS plugins"
contains "plugin migration creates plugin invocations table" "migrations/0033_plugins_wasm.sql" "CREATE TABLE IF NOT EXISTS plugin_invocations"
contains "user profile migration adds avatar URL" "migrations/0034_user_profile_preferences.sql" "avatar_url"
contains "user profile migration adds notification preferences" "migrations/0034_user_profile_preferences.sql" "notification_prefs"
contains "form permission migration creates form_permissions" "migrations/0038_form_permissions.sql" "CREATE TABLE IF NOT EXISTS form_permissions"
contains "form permission migration supports role subjects" "migrations/0038_form_permissions.sql" "subject_type IN ('role')"
contains "API runtime registers form permission migration" "apps/api/src/main.rs" "0038_form_permissions.sql"

printf '\nBackend behavior coverage:\n'
contains "decimal normalization uses rust_decimal" "apps/api/src/forms/decimal.rs" "rust_decimal"
contains "record values reject amount JSON number" "apps/api/src/forms/values.rs" "must be a decimal string"
contains "forms projection writes decimal index" "apps/api/src/forms/projections.rs" "value_decimal"
contains "form route writes business events" "apps/api/src/routes/form.rs" "business_events"
contains "business event helper writes event outbox" "apps/api/src/events/mod.rs" "INSERT INTO event_outbox"
contains "form route emits form created events" "apps/api/src/routes/form.rs" "form.created"
contains "form route exposes duplicate handler" "apps/api/src/routes/form.rs" "duplicate_form"
contains "form route emits duplicated events" "apps/api/src/routes/form.rs" "form.duplicated"
contains "form route emits form updated events" "apps/api/src/routes/form.rs" "form.updated"
contains "form route emits form archived events" "apps/api/src/routes/form.rs" "form.archived"
contains "form route emits view created events" "apps/api/src/routes/form.rs" "form.view.created"
contains "form route emits view archived events" "apps/api/src/routes/form.rs" "form.view.archived"
contains "form route handles record links" "apps/api/src/routes/form.rs" "form_record_links"
contains "form route emits print job created" "apps/api/src/routes/form.rs" "print_job.created"
contains "form route exposes import preview handler" "apps/api/src/routes/form.rs" "preview_import_form_records"
contains "form route exposes import commit handler" "apps/api/src/routes/form.rs" "import_form_records"
contains "form import reuses shared record creation" "apps/api/src/routes/form.rs" "create_record_for_form"
contains "form route normalizes write idempotency keys" "apps/api/src/routes/form.rs" "normalize_optional_idempotency_key"
contains "form route resolves existing idempotency receipts" "apps/api/src/routes/form.rs" "find_idempotent_record"
contains "form record create writes idempotency receipts" "apps/api/src/routes/form.rs" "form.record.created"
contains "form event helper forwards idempotency keys" "apps/api/src/routes/form.rs" "insert_form_event_with_idempotency"
contains "API registers form import preview route" "apps/api/src/main.rs" "/api/v1/forms/{form_id}/records/import-preview"
contains "API registers form import commit route" "apps/api/src/main.rs" "/api/v1/forms/{form_id}/records/import"
contains "API registers form duplicate route" "apps/api/src/main.rs" "/api/v1/forms/{form_id}/duplicate"
contains "form route exposes permissions getter" "apps/api/src/routes/form.rs" "get_form_permissions"
contains "form route exposes permissions updater" "apps/api/src/routes/form.rs" "update_form_permissions"
contains "form route enforces role permissions" "apps/api/src/routes/form.rs" "require_form_action"
contains "form route denies permission actions" "apps/api/src/routes/form.rs" "form permission denied"
contains "API registers form permission route" "apps/api/src/main.rs" "/api/v1/forms/{form_id}/permissions"
contains "plugin runtime uses wasmtime" "apps/api/src/plugins/runtime.rs" "wasmtime"
contains "plugin hooks support field validators" "apps/api/src/plugins/hooks.rs" "field_validator"
contains "plugin hooks support formula hooks" "apps/api/src/plugins/hooks.rs" "formula"
contains "plugin hooks support event handlers" "apps/api/src/plugins/hooks.rs" "event_handler"
contains "plugin route emits installed business events" "apps/api/src/routes/plugin.rs" "plugin.installed"
contains "plugin route emits updated business events" "apps/api/src/routes/plugin.rs" "plugin.updated"
contains "plugin route emits invoked business events" "apps/api/src/routes/plugin.rs" "plugin.invoked"
contains "plugin hooks emit invoked business events" "apps/api/src/plugins/hooks.rs" "plugin.invoked"
contains "plugin hooks mark automatic hook metadata" "apps/api/src/plugins/hooks.rs" "automatic_hook"
contains "scenario template installs restaurant calc plugin" "apps/api/src/routes/project.rs" "restaurant_calc"
contains "scenario template creates restaurant ordering default" "apps/api/src/routes/project.rs" "restaurant_ordering_default"
contains "scenario template API emits runtime usage guide" "apps/api/src/routes/scenario_template.rs" "usage_guide"
contains "scenario template API emits usage guide schema version" "apps/api/src/routes/scenario_template.rs" "openpr.scenario_template.usage_guide.v1"
contains "scenario template API usage guide includes operator entrypoints" "apps/api/src/routes/scenario_template.rs" "operator_entrypoints"
contains "scenario template API usage guide includes MCP tools" "apps/api/src/routes/scenario_template.rs" "primary_mcp_tools"
contains "scenario template API usage guide includes connector kinds" "apps/api/src/routes/scenario_template.rs" "connector_kinds"
contains "scenario template API usage guide includes plugin keys" "apps/api/src/routes/scenario_template.rs" "plugin_keys"
contains "scenario template API tests restaurant usage guide" "apps/api/src/routes/scenario_template.rs" "restaurant_usage_guide_exposes_runtime_integration_contract"
contains "MCP scenario template tool documents usage guide" "apps/mcp-server/src/tools/scenario_templates.rs" "usage_guide"
contains "MCP scenario template tool tests usage guide docs" "apps/mcp-server/src/tools/scenario_templates.rs" "scenario_template_tools_document_usage_guide_contract"
contains "project route emits project created business events" "apps/api/src/routes/project.rs" "project.created"
contains "project route emits project updated business events" "apps/api/src/routes/project.rs" "project.updated"
contains "project route emits project deleted business events" "apps/api/src/routes/project.rs" "project.deleted"
contains "scenario template initialization emits business events" "apps/api/src/routes/project.rs" "scenario_template_initialized"
contains "scenario template initialization writes form events" "apps/api/src/routes/project.rs" "insert_scenario_form_event"
contains "API registers scenario template install route" "apps/api/src/main.rs" "/api/v1/projects/{project_id}/scenario-templates/{template_key}/install"
contains "project route installs scenario templates into existing projects" "apps/api/src/routes/project.rs" "install_project_scenario_template"
contains "project route install emits scenario template business event" "apps/api/src/routes/project.rs" "project.scenario_template.installed"
contains "project route scenario install reuses full template application" "apps/api/src/routes/project.rs" "apply_scenario_template("
contains "project type route emits created business events" "apps/api/src/routes/project_type.rs" "project_type.created"
contains "project type route emits updated business events" "apps/api/src/routes/project_type.rs" "project_type.updated"
contains "project resource route emits created business events" "apps/api/src/routes/project_type.rs" "project_resource.created"
contains "project resource route emits updated business events" "apps/api/src/routes/project_type.rs" "project_resource.updated"
contains "project resource route emits deleted business events" "apps/api/src/routes/project_type.rs" "project_resource.deleted"
contains "auth route updates current user profile" "apps/api/src/routes/auth.rs" "update_profile"
contains "auth route verifies current password before update" "apps/api/src/routes/auth.rs" "invalid current password"
contains "auth route persists notification preferences" "apps/api/src/routes/auth.rs" "update_preferences"

printf '\nWorker and connector coverage:\n'
contains "worker consumes event outbox" "apps/worker/src/main.rs" "pickup_pending_event_outbox"
contains "worker marks outbox dispatched" "apps/worker/src/main.rs" "mark_event_outbox_dispatched"
contains "worker marks outbox failed for retry" "apps/worker/src/main.rs" "mark_event_outbox_failed"
contains "worker routes webhook connector kind" "apps/worker/src/main.rs" "'webhook'"
contains "worker routes print connector kind" "apps/worker/src/main.rs" "'print'"
contains "worker routes device connector kind" "apps/worker/src/main.rs" "'device'"
contains "worker emits AI task business events" "apps/worker/src/main.rs" "ai_task.picked_up"
contains "worker emits invocation dispatch business events" "apps/worker/src/main.rs" "invocation.dispatched"
contains "worker emits workflow invocation created events" "apps/worker/src/main.rs" "invocation.created"
contains "worker does not downgrade terminal invocation status" "apps/worker/src/main.rs" "status NOT IN ('completed', 'failed', 'cancelled')"
contains "API smoke covers workflow invocation business events" "scripts/smoke-universal-forms-api.sh" "workflow invocation business events and outbox smoke passed"
contains "worker requires explicit invocation lifecycle subscription" "apps/worker/src/main.rs" "LIKE 'invocation.%'"
contains "API smoke blocks wildcard lifecycle connector fanout" "scripts/smoke-universal-forms-api.sh" "invocation lifecycle fanout guard smoke passed"
contains "connector route handles receipts" "apps/api/src/routes/connector.rs" "receipt"
contains "connector route writes event inbox" "apps/api/src/routes/connector.rs" "event_inbox"
contains "connector route emits created business events" "apps/api/src/routes/connector.rs" "connector.created"
contains "connector route emits updated business events" "apps/api/src/routes/connector.rs" "connector.updated"
contains "connector route emits deleted business events" "apps/api/src/routes/connector.rs" "connector.deleted"
contains "legacy webhook route emits created business events" "apps/api/src/routes/webhook.rs" "webhook.created"
contains "legacy webhook route emits updated business events" "apps/api/src/routes/webhook.rs" "webhook.updated"
contains "legacy webhook route emits deleted business events" "apps/api/src/routes/webhook.rs" "webhook.deleted"
contains "legacy webhook connector sync marks metadata" "apps/api/src/routes/webhook.rs" "legacy_webhook"
contains "check result route emits created business events" "apps/api/src/routes/check_result.rs" "check_result.created"
contains "check result route emits proposed business events" "apps/api/src/routes/check_result.rs" "check_result.proposed"
contains "check result route emits proposal business events" "apps/api/src/routes/check_result.rs" "proposal.created_from_check_result"
contains "API registers form schema version list route" "apps/api/src/main.rs" "/api/v1/forms/{form_id}/schema-versions"
contains "API registers form schema version get route" "apps/api/src/main.rs" "/api/v1/forms/{form_id}/schema-versions/{version}"
contains "API registers child create route" "apps/api/src/main.rs" "post(routes::form::create_child_record)"
contains "API registers child update route" "apps/api/src/main.rs" "patch(routes::form::update_child_record)"
contains "API registers child archive route" "apps/api/src/main.rs" "delete(routes::form::archive_child_record)"
contains "API registers child restore route" "apps/api/src/main.rs" "post(routes::form::restore_child_record)"
contains "form route lists schema versions" "apps/api/src/routes/form.rs" "list_form_schema_versions"
contains "form route gets one schema version" "apps/api/src/routes/form.rs" "get_form_schema_version"
contains "form route creates child records" "apps/api/src/routes/form.rs" "create_child_record"
contains "form route updates child records" "apps/api/src/routes/form.rs" "update_child_record"
contains "form route archives child records" "apps/api/src/routes/form.rs" "archive_child_record"
contains "form route restores child records" "apps/api/src/routes/form.rs" "restore_child_record"
contains "forms MCP smoke calls schema version list" "scripts/smoke-forms-mcp.sh" "form_schema_versions.list"
contains "forms MCP smoke calls schema version get" "scripts/smoke-forms-mcp.sh" "form_schema_versions.get"
contains "forms MCP smoke calls scenario template install" "scripts/smoke-forms-mcp.sh" "scenario_templates.install"
contains "forms MCP smoke calls child create" "scripts/smoke-forms-mcp.sh" "form_records.child_create"
contains "forms MCP smoke calls child update" "scripts/smoke-forms-mcp.sh" "form_records.child_update"
contains "forms MCP smoke calls child archive" "scripts/smoke-forms-mcp.sh" "form_records.child_archive"
contains "forms MCP smoke calls child restore" "scripts/smoke-forms-mcp.sh" "form_records.child_restore"
contains "forms MCP smoke verifies record create idempotency" "scripts/smoke-forms-mcp.sh" "MCP create idempotency should return the original record on retry"
contains "forms MCP smoke verifies import idempotency" "scripts/smoke-forms-mcp.sh" "MCP import idempotency should not write duplicate retry rows"

printf '\nMCP and CLI coverage:\n'
for path in \
  apps/mcp-server/src/tools/forms.rs \
  apps/mcp-server/src/tools/plugins.rs \
  apps/mcp-server/src/tools/connectors.rs \
  apps/mcp-server/src/tools/project_types.rs \
  apps/mcp-server/src/tools/scenario_templates.rs \
  apps/mcp-server/src/cli.rs; do
  check_file "$path"
done
contains "MCP exposes forms.list" "apps/mcp-server/src/tools/forms.rs" "forms.list"
contains "MCP exposes scenario template install" "apps/mcp-server/src/tools/scenario_templates.rs" "scenario_templates.install"
contains "MCP exposes forms.create_from_template" "apps/mcp-server/src/tools/forms.rs" "forms.create_from_template"
contains "MCP exposes forms.duplicate" "apps/mcp-server/src/tools/forms.rs" "forms.duplicate"
contains "MCP exposes forms.schema_summary" "apps/mcp-server/src/tools/forms.rs" "forms.schema_summary"
contains "MCP exposes forms.field_usage" "apps/mcp-server/src/tools/forms.rs" "forms.field_usage"
contains "MCP exposes forms.field_dependencies" "apps/mcp-server/src/tools/forms.rs" "forms.field_dependencies"
contains "MCP exposes form schema version list" "apps/mcp-server/src/tools/forms.rs" "form_schema_versions.list"
contains "MCP exposes form schema version get" "apps/mcp-server/src/tools/forms.rs" "form_schema_versions.get"
contains "MCP exposes form_permissions.get" "apps/mcp-server/src/tools/forms.rs" "form_permissions.get"
contains "MCP exposes form_permissions.update" "apps/mcp-server/src/tools/forms.rs" "form_permissions.update"
contains "MCP exposes form_attachments.list" "apps/mcp-server/src/tools/forms.rs" "form_attachments.list"
contains "MCP exposes form_attachments.create" "apps/mcp-server/src/tools/forms.rs" "form_attachments.create"
contains "MCP exposes form_attachments.archive" "apps/mcp-server/src/tools/forms.rs" "form_attachments.archive"
contains "MCP exposes form_attachments.restore" "apps/mcp-server/src/tools/forms.rs" "form_attachments.restore"
contains "MCP exposes form_records.create" "apps/mcp-server/src/tools/forms.rs" "form_records.create"
contains "MCP form write tools expose idempotency keys" "apps/mcp-server/src/tools/forms.rs" "Optional write receipt key for retry-safe automation"
contains "MCP exposes form_records.export" "apps/mcp-server/src/tools/forms.rs" "form_records.export"
contains "MCP exposes form_records.import_preview" "apps/mcp-server/src/tools/forms.rs" "form_records.import_preview"
contains "MCP exposes form_records.import_commit" "apps/mcp-server/src/tools/forms.rs" "form_records.import_commit"
contains "MCP exposes form_records.relation_targets" "apps/mcp-server/src/tools/forms.rs" "form_records.relation_targets"
contains "MCP exposes form_records.children" "apps/mcp-server/src/tools/forms.rs" "form_records.children"
contains "MCP exposes form_records.child_create" "apps/mcp-server/src/tools/forms.rs" "form_records.child_create"
contains "MCP exposes form_records.child_update" "apps/mcp-server/src/tools/forms.rs" "form_records.child_update"
contains "MCP exposes form_records.child_archive" "apps/mcp-server/src/tools/forms.rs" "form_records.child_archive"
contains "MCP exposes form_records.child_restore" "apps/mcp-server/src/tools/forms.rs" "form_records.child_restore"
contains "MCP exposes form_records.aggregate" "apps/mcp-server/src/tools/forms.rs" "form_records.aggregate"
contains "MCP exposes events.tail" "apps/mcp-server/src/tools/forms.rs" "events.tail"
contains "MCP exposes plugin install" "apps/mcp-server/src/tools/plugins.rs" "plugins.install"
contains "MCP exposes plugin invoke" "apps/mcp-server/src/tools/plugins.rs" "plugins.invoke"
contains "MCP exposes connector list" "apps/mcp-server/src/tools/connectors.rs" "connectors.list"
contains "MCP registry pins current tool count" "apps/mcp-server/src/tools/mod.rs" "tools.len(),"
contains "MCP registry expected count is 106" "apps/mcp-server/src/tools/mod.rs" "106,"
contains "MCP embedded skill guide exposes 106 tools" "apps/mcp-server/src/server.rs" "## Tools (106)"
contains "MCP embedded skill guide exposes universal forms" "apps/mcp-server/src/server.rs" "### Universal Forms:"
contains "MCP embedded skill guide lists scenario install" "apps/mcp-server/src/server.rs" "scenario_templates.install"
contains "MCP embedded skill guide lists forms.duplicate" "apps/mcp-server/src/server.rs" "forms.duplicate"
contains "MCP embedded skill guide lists forms.create_from_template" "apps/mcp-server/src/server.rs" "forms.create_from_template"
contains "MCP embedded skill guide lists form metadata tools" "apps/mcp-server/src/server.rs" "forms.schema_summary"
contains "MCP embedded skill guide lists schema version tools" "apps/mcp-server/src/server.rs" "form_schema_versions.list"
contains "MCP embedded skill guide lists form permission tools" "apps/mcp-server/src/server.rs" "form_permissions.get"
contains "MCP embedded skill guide lists form attachment tools" "apps/mcp-server/src/server.rs" "form_attachments.create"
contains "MCP embedded skill guide lists relation tools" "apps/mcp-server/src/server.rs" "form_records.relation_targets"
contains "MCP embedded skill guide lists child lifecycle tools" "apps/mcp-server/src/server.rs" "form_records.child_archive"
contains "MCP embedded skill guide lists import/export tools" "apps/mcp-server/src/server.rs" "form_records.import_commit"
contains "MCP embedded skill guide exposes plugins" "apps/mcp-server/src/server.rs" "### Plugins:"
contains "MCP AGENTS guide exposes 106 tools" "apps/mcp-server/AGENTS.md" "106 tools"
contains "MCP AGENTS guide lists universal forms tools" "apps/mcp-server/AGENTS.md" "form_records.aggregate"
contains "MCP AGENTS guide lists scenario install" "apps/mcp-server/AGENTS.md" "scenario_templates.install"
contains "MCP AGENTS guide lists forms.duplicate" "apps/mcp-server/AGENTS.md" "forms.duplicate"
contains "MCP AGENTS guide lists forms.create_from_template" "apps/mcp-server/AGENTS.md" "forms.create_from_template"
contains "MCP AGENTS guide lists form metadata tools" "apps/mcp-server/AGENTS.md" "forms.schema_summary"
contains "MCP AGENTS guide lists schema version tools" "apps/mcp-server/AGENTS.md" "form_schema_versions.list"
contains "MCP AGENTS guide lists form permission tools" "apps/mcp-server/AGENTS.md" "form_permissions.get"
contains "MCP AGENTS guide lists form attachment tools" "apps/mcp-server/AGENTS.md" "form_attachments.create"
contains "MCP AGENTS guide lists relation tools" "apps/mcp-server/AGENTS.md" "form_records.relation_targets"
contains "MCP AGENTS guide lists child lifecycle tools" "apps/mcp-server/AGENTS.md" "form_records.child_archive"
contains "MCP AGENTS guide lists import/export tools" "apps/mcp-server/AGENTS.md" "form_records.import_commit"
contains "MCP AGENTS guide lists plugin tools" "apps/mcp-server/AGENTS.md" "plugin_invocations.list"
contains "MCP app README exposes 106 tools" "apps/mcp-server/README.md" "106 MCP Tools"
contains "MCP app README exposes three transports" "apps/mcp-server/README.md" "Three Transport Modes"
contains "MCP app README lists universal forms tools" "apps/mcp-server/README.md" "form_records.aggregate"
contains "MCP app README lists scenario install" "apps/mcp-server/README.md" "scenario_templates.install"
contains "MCP app README lists forms.duplicate" "apps/mcp-server/README.md" "forms.duplicate"
contains "MCP app README lists forms.create_from_template" "apps/mcp-server/README.md" "forms.create_from_template"
contains "MCP app README lists form metadata tools" "apps/mcp-server/README.md" "forms.schema_summary"
contains "MCP app README lists schema version tools" "apps/mcp-server/README.md" "form_schema_versions.list"
contains "MCP app README lists form permission tools" "apps/mcp-server/README.md" "form_permissions.get"
contains "MCP app README lists form attachment tools" "apps/mcp-server/README.md" "form_attachments.create"
contains "MCP app README lists relation tools" "apps/mcp-server/README.md" "form_records.relation_targets"
contains "MCP app README lists child lifecycle tools" "apps/mcp-server/README.md" "form_records.child_archive"
contains "MCP app README lists import/export tools" "apps/mcp-server/README.md" "form_records.import_commit"
contains "MCP app README lists plugin tools" "apps/mcp-server/README.md" "plugin_invocations.list"
contains "MCP app README uses API client structure" "apps/mcp-server/README.md" "client/           # OpenPR API client helpers"
contains "MCP skill guide exposes 106 tools" "skills/openpr-mcp/SKILL.md" "enumerate all 106 tools"
contains "MCP skill guide lists universal forms tools" "skills/openpr-mcp/SKILL.md" "form_records.aggregate"
contains "MCP skill guide lists scenario install" "skills/openpr-mcp/SKILL.md" "scenario_templates.install"
contains "MCP skill guide lists forms.duplicate" "skills/openpr-mcp/SKILL.md" "forms.duplicate"
contains "MCP skill guide lists forms.create_from_template" "skills/openpr-mcp/SKILL.md" "forms.create_from_template"
contains "MCP skill guide lists form metadata tools" "skills/openpr-mcp/SKILL.md" "forms.schema_summary"
contains "MCP skill guide lists schema version tools" "skills/openpr-mcp/SKILL.md" "form_schema_versions.list"
contains "MCP skill guide lists form permission tools" "skills/openpr-mcp/SKILL.md" "form_permissions.get"
contains "MCP skill guide lists form attachment tools" "skills/openpr-mcp/SKILL.md" "form_attachments.create"
contains "MCP skill guide lists relation tools" "skills/openpr-mcp/SKILL.md" "form_records.relation_targets"
contains "MCP skill guide lists child lifecycle tools" "skills/openpr-mcp/SKILL.md" "form_records.child_archive"
contains "MCP skill guide lists import/export tools" "skills/openpr-mcp/SKILL.md" "form_records.import_commit"
contains "MCP skill guide lists plugin tools" "skills/openpr-mcp/SKILL.md" "plugins.install"
contains "MCP skill validation requires exact 106 tools" "skills/openpr-mcp/scripts/validate-mcp.sh" "expected exactly 106 tools"
contains "MCP regression checks 106-tool registry" "skills/openpr-mcp/scripts/mcp-regression.py" "registry_has_106_tools_with_forms_and_plugins"
contains "MCP regression checks universal forms registry tools" "skills/openpr-mcp/scripts/mcp-regression.py" "form_records.aggregate"
contains "MCP regression checks scenario install" "skills/openpr-mcp/scripts/mcp-regression.py" "scenario_templates.install"
contains "MCP regression checks forms.duplicate" "skills/openpr-mcp/scripts/mcp-regression.py" "forms.duplicate"
contains "MCP regression checks forms.create_from_template" "skills/openpr-mcp/scripts/mcp-regression.py" "forms.create_from_template"
contains "MCP regression checks form metadata tools" "skills/openpr-mcp/scripts/mcp-regression.py" "forms.schema_summary"
contains "MCP regression checks schema version tools" "skills/openpr-mcp/scripts/mcp-regression.py" "form_schema_versions.list"
contains "MCP regression checks form permission tools" "skills/openpr-mcp/scripts/mcp-regression.py" "form_permissions.get"
contains "MCP regression checks form attachment tools" "skills/openpr-mcp/scripts/mcp-regression.py" "form_attachments.create"
contains "MCP regression checks relation tools" "skills/openpr-mcp/scripts/mcp-regression.py" "form_records.relation_targets"
contains "MCP regression checks child lifecycle tools" "skills/openpr-mcp/scripts/mcp-regression.py" "form_records.child_archive"
contains "MCP regression checks import/export tools" "skills/openpr-mcp/scripts/mcp-regression.py" "form_records.import_commit"
contains "MCP regression checks plugin registry tools" "skills/openpr-mcp/scripts/mcp-regression.py" "plugin_invocations.list"
contains "API agent policy enables form template MCP tool" "apps/api/src/routes/context.rs" "forms.create_from_template"
contains "API agent policy enables scenario template install tool" "apps/api/src/routes/context.rs" "scenario_templates.install"
contains "API agent policy enables form schema version MCP tools" "apps/api/src/routes/context.rs" "form_schema_versions.list"
contains "API agent policy enables form permission MCP tools" "apps/api/src/routes/context.rs" "form_permissions.get"
contains "API agent policy enables form attachment MCP tools" "apps/api/src/routes/context.rs" "form_attachments.create"
contains "API agent policy enables form import/export MCP tools" "apps/api/src/routes/context.rs" "form_records.import_commit"
contains "API agent policy enables relation MCP tools" "apps/api/src/routes/context.rs" "form_records.relation_targets"
contains "API agent policy enables child lifecycle MCP tools" "apps/api/src/routes/context.rs" "form_records.child_archive"
not_contains "MCP guides do not retain stale 82-tool heading" "apps/mcp-server/src/server.rs" "## Tools (82)"
not_contains "MCP AGENTS guide does not retain stale 82-tool count" "apps/mcp-server/AGENTS.md" "82 tools"
not_contains "MCP app README does not retain stale 82-tool count" "apps/mcp-server/README.md" "82 MCP Tools"
not_contains "MCP skill guide does not retain stale 82-tool count" "skills/openpr-mcp/SKILL.md" "82 tools"
not_contains "MCP guides do not retain stale 65-tool heading" "apps/mcp-server/src/server.rs" "## Tools (65)"
not_contains "MCP AGENTS guide does not retain stale 65-tool count" "apps/mcp-server/AGENTS.md" "65 tools"
not_contains "MCP app README does not retain stale 65-tool count" "apps/mcp-server/README.md" "65 MCP Tools"
not_contains "MCP app README does not retain stale two-transport wording" "apps/mcp-server/README.md" "Two Transport Modes"
not_contains "MCP app README does not expose direct db module" "apps/mcp-server/README.md" "src/db"
not_contains "MCP skill guide does not retain stale 65-tool count" "skills/openpr-mcp/SKILL.md" "65 tools"
not_contains "MCP validation does not accept stale 65-tool minimum" "skills/openpr-mcp/scripts/validate-mcp.sh" "-ge 65"
contains "docs index records current MCP tool count" "docs/README.md" "MCP server (106 tools"
contains "docs index records current MCP regression count" "docs/README.md" "106-tool registry"
contains "docs index links implementation map" "docs/README.md" "universal-forms-implementation-map.md"
not_contains "docs index does not retain stale 64-tool count" "docs/README.md" "64-tool"
not_contains "docs index does not retain stale 64 MCP server count" "docs/README.md" "64 tools"
contains "CLI has generic MCP tool caller" "apps/mcp-server/src/cli.rs" "ToolsAction::Call"
contains "CLI examples can call forms.list" "apps/mcp-server/src/cli.rs" "forms.list"

printf '\nFrontend and browser smoke coverage:\n'
for path in \
  frontend/src/lib/api/forms.ts \
  frontend/src/lib/api/connectors.ts \
  frontend/src/lib/api/auth.ts \
  frontend/src/lib/utils/scenario-template.ts \
  'frontend/src/routes/(app)/settings/+page.svelte' \
  'frontend/src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/+page.svelte' \
  frontend/scripts/smoke-forms-ui.mjs \
  frontend/scripts/smoke-project-template-wizard.mjs \
  frontend/scripts/smoke-template-work-items.mjs \
  frontend/scripts/smoke-restaurant-ordering.mjs; do
  check_file "$path"
done
contains "frontend forms API creates records" "frontend/src/lib/api/forms.ts" "createRecord"
contains "frontend forms API updates records" "frontend/src/lib/api/forms.ts" "updateRecord"
contains "frontend forms API aggregates records" "frontend/src/lib/api/forms.ts" "aggregate"
contains "frontend forms API previews record imports" "frontend/src/lib/api/forms.ts" "previewImportRecords"
contains "frontend forms API commits record imports" "frontend/src/lib/api/forms.ts" "importRecords"
contains "frontend forms API gets permissions" "frontend/src/lib/api/forms.ts" "getPermissions"
contains "frontend forms API updates permissions" "frontend/src/lib/api/forms.ts" "updatePermissions"
contains "frontend forms route loads form records" 'frontend/src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/+page.svelte' "loadRecords"
contains "frontend forms route supports record edit" 'frontend/src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/+page.svelte' "editingRecordId"
contains "frontend forms route supports record links" 'frontend/src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/+page.svelte' "createRecordLink"
contains "frontend forms route shows print jobs" 'frontend/src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/+page.svelte' "print_job"
contains "frontend forms route supports import modal" 'frontend/src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/+page.svelte' "showImportRecords"
contains "frontend forms route parses CSV imports" 'frontend/src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/+page.svelte' "parseCsvImport"
contains "frontend forms route supports permission panel" 'frontend/src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/+page.svelte' "saveMemberPermissions"
contains "frontend forms route renders permission actions" 'frontend/src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/+page.svelte' "formPermissionActions"
contains "frontend settings saves profile through API" 'frontend/src/routes/(app)/settings/+page.svelte' "authApi.updateProfile"
contains "frontend settings updates password through API" 'frontend/src/routes/(app)/settings/+page.svelte' "authApi.updatePassword"
contains "frontend settings saves notification prefs through API" 'frontend/src/routes/(app)/settings/+page.svelte' "authApi.updatePreferences"
not_contains "frontend settings does not fake save with timeout" 'frontend/src/routes/(app)/settings/+page.svelte' "setTimeout"
not_contains "frontend i18n does not expose placeholder settings copy" "frontend/src/lib/i18n/en.json" "placeholder)"
not_contains "frontend zh i18n does not expose placeholder settings copy" "frontend/src/lib/i18n/zh.json" "占位实现"
contains "forms browser smoke covers mobile overflow" "frontend/scripts/smoke-forms-ui.mjs" "horizontal overflow"
contains "project template wizard smoke covers template cards" "frontend/scripts/smoke-project-template-wizard.mjs" "data-template-card-seen"
contains "project template wizard smoke covers scenario setup" "frontend/scripts/smoke-project-template-wizard.mjs" "data-scenario-dashboard-seen"
contains "project template wizard smoke covers connection config" "frontend/scripts/smoke-project-template-wizard.mjs" "data-scenario-connection-configured"
contains "project template wizard smoke captures screenshots" "frontend/scripts/smoke-project-template-wizard.mjs" "OPENPR_PROJECT_TEMPLATE_SCREENSHOT_DIR"
contains "template work items smoke covers all non-restaurant templates" "frontend/scripts/smoke-template-work-items.mjs" "quality_corrective_action_default"
contains "template work items smoke covers scenario field issue creation" "frontend/scripts/smoke-template-work-items.mjs" "data-template-work-item-smoke"
contains "template work items smoke captures screenshots" "frontend/scripts/smoke-template-work-items.mjs" "OPENPR_TEMPLATE_WORK_ITEMS_SCREENSHOT_DIR"
contains "restaurant browser smoke covers table change" "frontend/scripts/smoke-restaurant-ordering.mjs" "table"
contains "restaurant browser smoke captures screenshots" "frontend/scripts/smoke-restaurant-ordering.mjs" "OPENPR_RESTAURANT_SCREENSHOT_DIR"
contains "acceptance uses resolved Bun frontend check" "scripts/acceptance-universal-forms.sh" 'cd frontend && $FRONTEND_BUN run check'
contains "acceptance includes project template wizard smoke" "scripts/acceptance-universal-forms.sh" "smoke:project-template"
contains "acceptance includes template work items smoke" "scripts/acceptance-universal-forms.sh" "smoke:template-work-items"
contains "acceptance uses resolved Bun frontend smoke" "scripts/acceptance-universal-forms.sh" 'cd frontend && $FRONTEND_BUN run smoke:forms-ui'
contains "acceptance resolves common Bun install path" "scripts/acceptance-universal-forms.sh" '$HOME/.bun/bin'
contains "UI artifact collector uses Bun frontend build" "scripts/collect-universal-forms-ui-artifacts.sh" "FRONTEND_BUN"
contains "UI artifact collector captures project template screenshots" "scripts/collect-universal-forms-ui-artifacts.sh" "OPENPR_PROJECT_TEMPLATE_SCREENSHOT_DIR"
contains "UI artifact collector captures template work item screenshots" "scripts/collect-universal-forms-ui-artifacts.sh" "OPENPR_TEMPLATE_WORK_ITEMS_SCREENSHOT_DIR"
contains "UI artifact collector resolves common Bun install path" "scripts/collect-universal-forms-ui-artifacts.sh" '$HOME/.bun/bin'
contains "UI artifact collector writes same-directory manifest candidate" "scripts/collect-universal-forms-ui-artifacts.sh" 'mktemp "$(dirname "$MANIFEST_PATH")/'
contains "UI artifact collector verifies candidate manifest before publish" "scripts/collect-universal-forms-ui-artifacts.sh" '--manifest "$manifest_tmp"'
contains "UI artifact collector preserves existing manifest permissions" "scripts/collect-universal-forms-ui-artifacts.sh" 'chmod --reference="$MANIFEST_PATH" "$manifest_tmp"'
contains "UI artifact collector atomically publishes manifest" "scripts/collect-universal-forms-ui-artifacts.sh" 'mv -f "$manifest_tmp" "$MANIFEST_PATH"'
not_contains "acceptance does not use npm frontend commands" "scripts/acceptance-universal-forms.sh" "npm run"

printf '\nAcceptance smoke coverage:\n'
for path in \
  scripts/smoke-universal-forms-api.sh \
  scripts/smoke-forms-mcp.sh \
  scripts/smoke-phase2-connectors-invocations.sh \
  scripts/smoke-phase3-mcp-governance-acceptance.sh \
  scripts/smoke-phase5-release-readiness.sh \
  scripts/smoke-plugins-mcp.sh \
  scripts/smoke-phase1-project-types-resources.sh \
  scripts/smoke-user-settings-api.sh \
  scripts/smoke-webhook-generic-consumer.sh \
  scripts/smoke-scenario-template-forms.sh \
  scripts/smoke-restaurant-ordering.sh \
  scripts/bootstrap-restaurant-demo.sh \
  scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh \
  scripts/smoke-wasm-plugin-runtime.sh \
  scripts/smoke-universal-forms-deployed-import.mjs \
  scripts/smoke-universal-forms-deployed-duplicate.mjs \
  scripts/audit-universal-forms-delivery-bundle.sh \
  scripts/gate-universal-forms-release.sh \
  scripts/smoke-universal-forms-release-gate.sh \
  scripts/smoke-universal-forms-release-gate-output.sh \
  scripts/status-universal-forms-delivery.sh \
  scripts/verify-universal-forms-delivery-status-json.sh \
  scripts/verify-universal-forms-implementation-map.sh \
  scripts/smoke-universal-forms-implementation-map-contract.sh \
  scripts/smoke-universal-forms-delivery-status-json-contract.sh \
  scripts/smoke-universal-forms-delivery-status-output.sh \
  scripts/smoke-universal-forms-manual-signoff-progression.sh \
  scripts/smoke-universal-forms-manual-signoff-commands.sh \
  scripts/refresh-universal-forms-delivery-bundle.sh \
  scripts/prepare-universal-forms-delivery-manifest.sh \
  scripts/verify-universal-forms-delivery-manifest.sh \
  scripts/report-universal-forms-delivery-manifest-json.sh \
  scripts/verify-universal-forms-delivery-manifest-json.sh \
  scripts/smoke-universal-forms-delivery-manifest-json-contract.sh \
  scripts/prepare-universal-forms-ui-review-gallery.sh \
  scripts/verify-universal-forms-ui-review-gallery.sh \
  scripts/smoke-universal-forms-ui-review-gallery-render.sh \
  scripts/prepare-universal-forms-signoff-dashboard.sh \
  scripts/verify-universal-forms-signoff-dashboard.sh \
  scripts/smoke-universal-forms-signoff-dashboard-render.sh \
  scripts/smoke-universal-forms-signoff-dashboard-progression.sh \
  scripts/smoke-universal-forms-report-output-boundaries.sh \
  scripts/report-universal-forms-completion-audit-json.sh \
  scripts/verify-universal-forms-completion-audit-json.sh \
  scripts/smoke-universal-forms-completion-audit-json-contract.sh \
  scripts/report-universal-forms-signoff-status.sh \
  scripts/report-universal-forms-signoff-status-json.sh \
  scripts/verify-universal-forms-signoff-status-json.sh \
  scripts/smoke-universal-forms-signoff-status-json-contract.sh \
  scripts/verify-universal-forms-next-signoff-review.sh \
  scripts/smoke-universal-forms-next-signoff-review-contract.sh \
  scripts/smoke-universal-forms-next-signoff-command.sh \
  scripts/smoke-universal-forms-manual-signoff-progression.sh \
  scripts/report-universal-forms-readiness-json.sh \
  scripts/verify-universal-forms-readiness-json.sh \
  scripts/report-universal-forms-development-status-json.sh \
  scripts/verify-universal-forms-development-status-json.sh \
  scripts/smoke-universal-forms-development-status-json-contract.sh \
  scripts/report-universal-forms-scenario-catalog-json.sh \
  scripts/verify-universal-forms-scenario-catalog-json.sh \
  scripts/smoke-universal-forms-scenario-catalog-json-contract.sh \
  scripts/report-universal-forms-implementation-map-json.sh \
  scripts/verify-universal-forms-implementation-map-json.sh \
  scripts/smoke-universal-forms-implementation-map-json-contract.sh \
  scripts/acceptance-universal-forms.sh; do
  check_file "$path"
done
check_executable "scripts/smoke-universal-forms-deployed-import.mjs"
check_executable "scripts/smoke-universal-forms-deployed-permissions.mjs"
check_executable "scripts/smoke-universal-forms-deployed-duplicate.mjs"
contains "API smoke covers retry and idempotency" "scripts/smoke-universal-forms-api.sh" "retry, and idempotency"
contains "deployed import smoke covers import preview endpoint" "scripts/smoke-universal-forms-deployed-import.mjs" "/records/import-preview"
contains "deployed import smoke covers import commit endpoint" "scripts/smoke-universal-forms-deployed-import.mjs" "/records/import"
contains "deployed import smoke covers browser import modal" "scripts/smoke-universal-forms-deployed-import.mjs" "导入记录"
contains "deployed duplicate smoke covers duplicate endpoint" "scripts/smoke-universal-forms-deployed-duplicate.mjs" "/duplicate"
contains "deployed duplicate smoke covers MCP duplicate tool" "scripts/smoke-universal-forms-deployed-duplicate.mjs" "forms.duplicate"
contains "deployed duplicate smoke covers browser duplicate action" "scripts/smoke-universal-forms-deployed-duplicate.mjs" "复制表单"
contains "deployed duplicate smoke covers records not copied" "scripts/smoke-universal-forms-deployed-duplicate.mjs" "records should not be copied"
contains "deployed permissions smoke covers permissions endpoint" "scripts/smoke-universal-forms-deployed-permissions.mjs" "/permissions"
contains "deployed permissions smoke covers member denial" "scripts/smoke-universal-forms-deployed-permissions.mjs" "member create"
contains "deployed permissions smoke covers form list hiding" "scripts/smoke-universal-forms-deployed-permissions.mjs" "member list should hide form when form.view is false"
contains "deployed permissions smoke covers browser permissions panel" "scripts/smoke-universal-forms-deployed-permissions.mjs" "保存权限"
contains "API smoke covers form definition write events" "scripts/smoke-universal-forms-api.sh" "form definition/view write business events"
contains "API smoke covers connector write events" "scripts/smoke-universal-forms-api.sh" "connector write business events"
contains "API smoke covers legacy webhook write events" "scripts/smoke-universal-forms-api.sh" "legacy webhook business events"
contains "API smoke covers legacy webhook connector outbox" "scripts/smoke-universal-forms-api.sh" "legacy webhook connector outbox rows"
contains "user settings smoke verifies profile update" "scripts/smoke-user-settings-api.sh" "/api/v1/auth/me/profile"
contains "user settings smoke verifies preference persistence" "scripts/smoke-user-settings-api.sh" "/api/v1/auth/me/preferences"
contains "user settings smoke verifies password rotation" "scripts/smoke-user-settings-api.sh" "old password login should fail"
contains "acceptance includes user settings smoke" "scripts/acceptance-universal-forms.sh" "User settings profile password preferences smoke"
contains "acceptance includes Phase 2 invocation smoke" "scripts/acceptance-universal-forms.sh" "Phase 2 connectors and invocation lifecycle smoke"
contains "Phase 2 invocation smoke covers lifecycle business events" "scripts/smoke-phase2-connectors-invocations.sh" "invocation lifecycle business events and outbox smoke passed"
contains "Phase 2 invocation smoke covers AI task invocation business events" "scripts/smoke-phase2-connectors-invocations.sh" "AI task invocation business events and outbox smoke passed"
contains "AI task invocation service emits business events" "apps/api/src/services/invocation_service.rs" "ai_task_invocation"
contains "Phase 2 invocation smoke covers AI task business events" "scripts/smoke-phase2-connectors-invocations.sh" "AI task business events and outbox smoke passed"
contains "AI task service emits business events" "apps/api/src/services/ai_task_service.rs" "ai_task.created"
contains "Phase 2 invocation smoke covers worker dispatch events" "scripts/smoke-phase2-connectors-invocations.sh" "worker AI task dispatch business events and outbox smoke passed"
contains "acceptance includes Phase 3 MCP governance smoke" "scripts/acceptance-universal-forms.sh" "Phase 3 MCP governance acceptance smoke"
contains "Phase 3 MCP governance smoke covers check results" "scripts/smoke-phase3-mcp-governance-acceptance.sh" "check_results.create"
contains "Phase 3 MCP governance smoke covers check result business events" "scripts/smoke-phase3-mcp-governance-acceptance.sh" "check result business events and outbox smoke passed"
contains "MCP smoke covers generic CLI" "scripts/smoke-forms-mcp.sh" "tools call"
contains "MCP smoke covers forms.duplicate" "scripts/smoke-forms-mcp.sh" "forms.duplicate"
contains "acceptance includes Phase 5 release readiness smoke" "scripts/acceptance-universal-forms.sh" "Phase 5 release readiness live API and MCP smoke"
contains "Phase 5 release readiness smoke covers MCP readiness" "scripts/smoke-phase5-release-readiness.sh" "release.readiness.get"
contains "project release readiness schema exists" "docs/schemas/openpr-project-release-readiness.schema.json" "openpr.project.release_readiness.v1"
contains "project release readiness schema pins next action shape" "docs/schemas/openpr-project-release-readiness.schema.json" '"next_actions"'
contains "project release readiness schema pins gate order" "docs/schemas/openpr-project-release-readiness.schema.json" '"no_pending_ai_tasks"'
contains "project release readiness API emits schema version" "apps/api/src/routes/release_readiness.rs" "RELEASE_READINESS_SCHEMA_VERSION"
contains "project release readiness API emits schema path" "apps/api/src/routes/release_readiness.rs" "RELEASE_READINESS_SCHEMA_PATH"
contains "Phase 5 release readiness API emits next actions" "apps/api/src/routes/release_readiness.rs" "next_actions_for_gates"
contains "Phase 5 release readiness next actions expose order" "apps/api/src/routes/release_readiness.rs" "review_order"
contains "Phase 5 release readiness next actions expose blocking flag" "apps/api/src/routes/release_readiness.rs" "blocking"
contains "Phase 5 release readiness unit tests ready next action" "apps/api/src/routes/release_readiness.rs" "ready_release_readiness_returns_review_action"
contains "Phase 5 release readiness unit tests blocked next actions" "apps/api/src/routes/release_readiness.rs" "blocked_release_readiness_returns_gate_remediation_actions"
contains "MCP release readiness tool description documents next actions" "apps/mcp-server/src/tools/release.rs" "next actions"
contains "MCP release readiness tool unit tests next actions" "apps/mcp-server/src/tools/release.rs" "release_readiness_tool_documents_next_actions"
contains "embedded MCP skill guide documents release next actions" "apps/mcp-server/src/server.rs" "gates, blockers, next actions"
contains "Phase 5 release readiness smoke covers REST next actions" "scripts/smoke-phase5-release-readiness.sh" "ready release readiness should include reviewer next action"
contains "Phase 5 release readiness smoke covers schema version" "scripts/smoke-phase5-release-readiness.sh" "release readiness schema version should match schema"
contains "Phase 5 release readiness smoke covers schema path" "scripts/smoke-phase5-release-readiness.sh" "release readiness schema path should be stable"
contains "Phase 5 release readiness smoke covers MCP schema version" "scripts/smoke-phase5-release-readiness.sh" "MCP release.readiness.get should expose schema version"
contains "Phase 5 release readiness smoke covers MCP next actions" "scripts/smoke-phase5-release-readiness.sh" "MCP release.readiness.get should expose release next actions"
contains "Phase 5 release readiness smoke covers next action order" "scripts/smoke-phase5-release-readiness.sh" "action.review_order === 1"
contains "Phase 5 release readiness smoke covers next action blocking flag" "scripts/smoke-phase5-release-readiness.sh" "action.blocking === true"
contains "Phase 5 release readiness smoke covers blocked remediation action" "scripts/smoke-phase5-release-readiness.sh" "blocked release readiness should expose actionable pending AI task remediation"
contains "delivery manifest includes project release readiness schema" "scripts/prepare-universal-forms-delivery-manifest.sh" "project release readiness JSON schema"
contains "plugin MCP smoke covers formula patch" "scripts/smoke-plugins-mcp.sh" "formula"
contains "plugin MCP smoke covers plugin business events" "scripts/smoke-plugins-mcp.sh" "Plugin business events and outbox smoke passed"
contains "plugin MCP smoke covers automatic hook business events" "scripts/smoke-plugins-mcp.sh" "automatic hook plugin.invoked events"
contains "project type/resource smoke covers business events" "scripts/smoke-phase1-project-types-resources.sh" "Project type/resource business events and outbox smoke passed"
contains "project type/resource smoke covers project lifecycle events" "scripts/smoke-phase1-project-types-resources.sh" "Project lifecycle business events and outbox smoke passed"
contains "webhook smoke covers generic consumer" "scripts/smoke-webhook-generic-consumer.sh" "webhook generic consumer"
contains "scenario template smoke covers restaurant plugin" "scripts/smoke-scenario-template-forms.sh" "restaurant_calc plugin"
contains "scenario template smoke covers runtime usage guide schema" "scripts/smoke-scenario-template-forms.sh" "usage guide schema version"
contains "scenario template smoke covers runtime usage MCP tools" "scripts/smoke-scenario-template-forms.sh" "aggregate MCP tool"
contains "scenario template smoke covers runtime usage connectors" "scripts/smoke-scenario-template-forms.sh" "print connector kind"
contains "scenario template smoke covers initialization business events" "scripts/smoke-scenario-template-forms.sh" "scenario template initialization business events and outbox smoke passed"
contains "restaurant smoke covers print receipt" "scripts/smoke-restaurant-ordering.sh" "print receipt"
contains "restaurant demo bootstrap uses public API" "scripts/bootstrap-restaurant-demo.sh" "/api/v1/workspaces"
contains "restaurant demo bootstrap creates restaurant template project" "scripts/bootstrap-restaurant-demo.sh" "restaurant_ordering_default"
contains "restaurant demo bootstrap refuses remote API by default" "scripts/bootstrap-restaurant-demo.sh" "OPENPR_DEMO_ALLOW_REMOTE"
contains "restaurant demo bootstrap verifies formula output" "scripts/bootstrap-restaurant-demo.sh" "restaurant_calc should calculate order_line.line_total"
contains "restaurant demo bootstrap creates MCP bot token" "scripts/bootstrap-restaurant-demo.sh" "/bots"
contains "restaurant demo bootstrap writes MCP config credentials" "scripts/bootstrap-restaurant-demo.sh" "mcp.workspace_id"
contains "restaurant demo bootstrap can recreate compose MCP server" "scripts/bootstrap-restaurant-demo.sh" "--force-recreate mcp-server"
contains "restaurant demo bootstrap verifies MCP HTTP projects.list" "scripts/bootstrap-restaurant-demo.sh" "projects.list"
contains "restaurant demo bootstrap checks demo project through MCP HTTP" "scripts/bootstrap-restaurant-demo.sh" "MCP HTTP verification passed"
contains "restaurant demo MCP HTTP smoke starts real MCP server" "scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh" "/target/debug/mcp-server\" --config"
contains "restaurant demo MCP HTTP smoke forces JSON-RPC verification" "scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh" "OPENPR_DEMO_VERIFY_MCP_HTTP=1"
contains "restaurant demo MCP HTTP smoke proves RESTDEMO over MCP" "scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh" "projects.list includes RESTDEMO"
contains "acceptance includes restaurant demo MCP HTTP smoke" "scripts/acceptance-universal-forms.sh" "Restaurant demo bootstrap MCP HTTP smoke"
contains "acceptance includes PostgreSQL-only security scope audit" "scripts/acceptance-universal-forms.sh" "PostgreSQL-only security scope audit"
contains "security scope audit exists" "scripts/audit-universal-forms-security-scope.sh" "Universal forms security scope audit"
contains "security scope audit runs cargo audit JSON" "scripts/audit-universal-forms-security-scope.sh" "cargo audit --json"
contains "security scope audit proves PostgreSQL SQLx backend" "scripts/audit-universal-forms-security-scope.sh" "sqlx-postgres"
contains "security scope audit blocks active sqlx-mysql tree" "scripts/audit-universal-forms-security-scope.sh" "workspace has no active sqlx-mysql dependency tree"
contains "security scope audit blocks active rsa tree" "scripts/audit-universal-forms-security-scope.sh" "workspace has no active rsa dependency tree"
contains "acceptance evidence preserves key pass fail lines" "scripts/acceptance-universal-forms.sh" "Key PASS/FAIL lines"
contains "acceptance evidence extracts full pass fail summary" "scripts/acceptance-universal-forms.sh" "rg '^(PASS|FAIL):'"
contains "acceptance evidence writes automated check index" "scripts/acceptance-universal-forms.sh" "## Automated Check Index"
contains "acceptance evidence stores check index rows" "scripts/acceptance-universal-forms.sh" "CHECK_INDEX_PATH"
contains "acceptance evidence filters Vite npm preview hint" "scripts/acceptance-universal-forms.sh" "vite_preview_hint"
contains "WASM runtime smoke runs plugin runtime tests" "scripts/smoke-wasm-plugin-runtime.sh" "plugins::runtime::tests::"
contains "WASM runtime tests cover fuel trap" "apps/api/src/plugins/runtime.rs" "fuel_limit_traps_runaway_plugins"
contains "delivery bundle audit covers generated evidence consistency" "scripts/audit-universal-forms-delivery-bundle.sh" "Generated evidence consistency"
contains "delivery bundle audit covers temporary finalizer drill" "scripts/audit-universal-forms-delivery-bundle.sh" "temporary signed-copy finalizer succeeds"
contains "release gate has pre-signoff mode" "scripts/gate-universal-forms-release.sh" "--allow-pending"
contains "release gate reads readiness JSON" "scripts/gate-universal-forms-release.sh" "openpr-universal-form-readiness-2026-05-31.json"
contains "release gate reads development status JSON" "scripts/gate-universal-forms-release.sh" "openpr-universal-form-development-status-2026-05-31.json"
contains "release gate checks final release flag" "scripts/gate-universal-forms-release.sh" "final_release_allowed"
contains "release gate emits schema version" "scripts/gate-universal-forms-release.sh" "openpr.universal_forms.release_gate.v1"
contains "release gate emits schema path" "scripts/gate-universal-forms-release.sh" "schema_path"
contains "release gate JSON schema exists" "docs/schemas/openpr-universal-forms-release-gate.schema.json" "openpr.universal_forms.release_gate.v1"
contains "release gate JSON schema defines reusable manual key enum" "docs/schemas/openpr-universal-forms-release-gate.schema.json" '"manual_key"'
contains "release gate JSON schema constrains next manual signoff key" "docs/schemas/openpr-universal-forms-release-gate.schema.json" '#/$defs/manual_key'
contains "release gate JSON verifier checks readiness parity" "scripts/verify-universal-forms-release-gate-json.sh" "stage matches readiness JSON"
contains "release gate JSON verifier checks final release flag" "scripts/verify-universal-forms-release-gate-json.sh" "development final flag matches development status JSON"
contains "release gate JSON verifier checks next manual key enum" "scripts/verify-universal-forms-release-gate-json.sh" "JSON next manual key is allowed by schema"
contains "release gate JSON verifier checks nested schema additional properties" "scripts/verify-universal-forms-release-gate-json.sh" "schema file disallows tracker status additional properties"
contains "release gate JSON verifier rejects nested extra fields" "scripts/verify-universal-forms-release-gate-json.sh" "JSON tracker status object has no extra keys beyond schema"
contains "release gate JSON contract smoke checks release flag drift" "scripts/smoke-universal-forms-release-gate-json-contract.sh" "release allowed drift"
contains "release gate JSON contract smoke checks nested missing fields" "scripts/smoke-universal-forms-release-gate-json-contract.sh" "missing tracker status required key"
contains "release gate JSON contract smoke checks nested extra fields" "scripts/smoke-universal-forms-release-gate-json-contract.sh" "extra tracker status property"
contains "release gate JSON contract smoke checks unknown next manual key" "scripts/smoke-universal-forms-release-gate-json-contract.sh" "unknown next manual key"
contains "release gate smoke checks JSON output" "scripts/smoke-universal-forms-release-gate.sh" "--allow-pending --json"
contains "release gate smoke checks strict pending rejection" "scripts/smoke-universal-forms-release-gate.sh" "strict release gate rejects pending manual signoff"
contains "release gate smoke runs JSON verifier" "scripts/smoke-universal-forms-release-gate.sh" "verify-universal-forms-release-gate-json.sh"
contains "release gate smoke runs JSON contract smoke" "scripts/smoke-universal-forms-release-gate.sh" "smoke-universal-forms-release-gate-json-contract.sh"
contains "release gate output smoke checks allow-pending text" "scripts/smoke-universal-forms-release-gate-output.sh" "allow-pending release gate output"
contains "release gate output smoke checks strict text" "scripts/smoke-universal-forms-release-gate-output.sh" "strict release gate output"
contains "release gate output smoke checks JSON mirror" "scripts/smoke-universal-forms-release-gate-output.sh" "mirrors release flag"
contains "release gate output smoke rejects null leakage" "scripts/smoke-universal-forms-release-gate-output.sh" "does not leak JSON nulls"
contains "delivery status command verifies readiness JSON" "scripts/status-universal-forms-delivery.sh" "verify-universal-forms-readiness-json.sh"
contains "delivery status command verifies development status JSON" "scripts/status-universal-forms-delivery.sh" "verify-universal-forms-development-status-json.sh"
contains "delivery status command verifies signoff status JSON" "scripts/status-universal-forms-delivery.sh" "verify-universal-forms-signoff-status-json.sh"
contains "delivery status command verifies delivery manifest JSON" "scripts/status-universal-forms-delivery.sh" "verify-universal-forms-delivery-manifest-json.sh"
contains "delivery status command runs next signoff smoke" "scripts/status-universal-forms-delivery.sh" "smoke-universal-forms-next-signoff-command.sh"
contains "delivery status command runs manual signoff progression smoke" "scripts/status-universal-forms-delivery.sh" "smoke-universal-forms-manual-signoff-progression.sh"
contains "delivery status command runs all manual signoff smoke" "scripts/status-universal-forms-delivery.sh" "smoke-universal-forms-manual-signoff-commands.sh"
contains "delivery status command suppresses successful prerequisite chatter" "scripts/status-universal-forms-delivery.sh" "run_quiet"
contains "delivery status command preserves failed prerequisite output" "scripts/status-universal-forms-delivery.sh" "Delivery status prerequisite failed"
contains "delivery status command runs pre-signoff release gate JSON" "scripts/status-universal-forms-delivery.sh" "--allow-pending --json"
contains "delivery status command supports JSON output" "scripts/status-universal-forms-delivery.sh" "--json"
contains "delivery status command emits generation time" "scripts/status-universal-forms-delivery.sh" "generated_at"
contains "delivery status command emits verified prerequisites" "scripts/status-universal-forms-delivery.sh" "verified_prerequisites"
contains "delivery status command emits completion summary" "scripts/status-universal-forms-delivery.sh" "completion_summary"
contains "delivery status command emits completion breakdown" "scripts/status-universal-forms-delivery.sh" "completion_breakdown"
contains "delivery status command emits release blockers" "scripts/status-universal-forms-delivery.sh" "release_blockers"
contains "delivery status command emits next actions" "scripts/status-universal-forms-delivery.sh" "next_actions"
contains "delivery status command emits next reviewer check" "scripts/status-universal-forms-delivery.sh" "next_reviewer_check"
contains "delivery status command emits next automated evidence" "scripts/status-universal-forms-delivery.sh" "next_automated_evidence"
contains "delivery status command emits manual accepted count" "scripts/status-universal-forms-delivery.sh" "manual_signoff_accepted_rows"
contains "delivery status command emits overall completion count" "scripts/status-universal-forms-delivery.sh" "overall_items_completed"
contains "delivery status command emits overall completion percent" "scripts/status-universal-forms-delivery.sh" "overall_completion_percent"
contains "delivery status command emits manual signoff queue" "scripts/status-universal-forms-delivery.sh" "manual_signoff_queue"
contains "delivery status command emits review surfaces" "scripts/status-universal-forms-delivery.sh" "review_surfaces"
contains "delivery status command emits signoff dashboard review surface" "scripts/status-universal-forms-delivery.sh" "signoff_dashboard"
contains "delivery status JSON schema exists" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" "openpr.universal_forms.delivery_status.v1"
contains "delivery status command emits schema version" "scripts/status-universal-forms-delivery.sh" "openpr.universal_forms.delivery_status.v1"
contains "delivery status command emits schema path" "scripts/status-universal-forms-delivery.sh" "schema_path"
contains "delivery status JSON schema pins verified prerequisites" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" "verified_prerequisites"
contains "delivery status JSON schema pins completion summary" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" "completion_summary"
contains "delivery status JSON schema pins completion breakdown" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" "completion_breakdown"
contains "delivery status JSON schema defines completion breakdown row" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" "completion_breakdown_row"
contains "delivery status JSON schema pins release blockers" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" "release_blockers"
contains "delivery status JSON schema defines release blocker row" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" "release_blocker"
contains "delivery status JSON schema pins next actions" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" "next_actions"
contains "delivery status JSON schema defines next action row" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" "next_action"
contains "delivery status JSON schema requires next reviewer check" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" '"reviewer_check"'
contains "delivery status JSON schema requires next automated evidence" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" '"automated_evidence"'
contains "delivery status JSON schema pins overall completion fields" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" "overall_items_remaining"
contains "delivery status JSON schema pins manual signoff queue" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" "manual_signoff_queue"
contains "delivery status JSON schema pins review surfaces" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" "review_surfaces"
contains "delivery status JSON schema defines manual queue row" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" "manual_queue_row"
contains "delivery status JSON schema pins delivery state enum" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" "engineering_complete_pending_manual_signoff"
contains "delivery status JSON schema pins progression prerequisite" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" "manual_signoff_progression_smoke"
contains "delivery status JSON schema defines reusable manual key enum" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" '"manual_key"'
contains "delivery status JSON schema constrains next manual signoff key" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" '#/$defs/manual_key'
contains "delivery status JSON schema uses release gate release mode" "docs/schemas/openpr-universal-forms-delivery-status.schema.json" '"release"'
contains "delivery status JSON verifier checks readiness parity" "scripts/verify-universal-forms-delivery-status-json.sh" "stage matches readiness JSON"
contains "delivery status JSON verifier checks manifest parity" "scripts/verify-universal-forms-delivery-status-json.sh" "manifest file count matches delivery manifest JSON"
contains "delivery status JSON verifier checks release gate parity" "scripts/verify-universal-forms-delivery-status-json.sh" "release gate mode matches release gate JSON"
contains "delivery status JSON verifier checks prerequisite order" "scripts/verify-universal-forms-delivery-status-json.sh" "JSON verified prerequisites match schema order"
contains "delivery status JSON verifier checks completion summary counters" "scripts/verify-universal-forms-delivery-status-json.sh" "completion summary engineering completed subtracts failures"
contains "delivery status JSON verifier checks completion breakdown counters" "scripts/verify-universal-forms-delivery-status-json.sh" "completion breakdown overall completed mirrors summary"
contains "delivery status JSON verifier checks completion breakdown status markers" "scripts/verify-universal-forms-delivery-status-json.sh" "completion breakdown manual status matches counters"
contains "delivery status JSON verifier checks release blocker order" "scripts/verify-universal-forms-delivery-status-json.sh" "JSON release blocker order is exact"
contains "delivery status JSON verifier checks release blocker counts" "scripts/verify-universal-forms-delivery-status-json.sh" "release blocker manual count matches pending rows"
contains "delivery status JSON verifier checks next action order" "scripts/verify-universal-forms-delivery-status-json.sh" "JSON next action order is exact"
contains "delivery status JSON verifier checks next action commands" "scripts/verify-universal-forms-delivery-status-json.sh" "next action record next signoff command mirrors recorder"
contains "delivery status JSON verifier checks next action blockers" "scripts/verify-universal-forms-delivery-status-json.sh" "next action verify manual signoff blocked_by matches blockers"
contains "delivery status JSON verifier checks review surface paths" "scripts/verify-universal-forms-delivery-status-json.sh" "signoff dashboard path matches current report"
contains "delivery status JSON verifier checks manual signoff queue mirror" "scripts/verify-universal-forms-delivery-status-json.sh" "manual signoff queue mirrors signoff status JSON"
contains "delivery status JSON verifier checks manual signoff queue next marker" "scripts/verify-universal-forms-delivery-status-json.sh" "manual signoff queue next marker mirrors next manual key"
contains "delivery status JSON verifier checks delivery state" "scripts/verify-universal-forms-delivery-status-json.sh" "completion summary delivery state matches counters"
contains "delivery status JSON verifier checks release mode schema alignment" "scripts/verify-universal-forms-delivery-status-json.sh" "schema file aligns release gate modes with release gate schema"
contains "delivery status JSON verifier checks nested schema additional properties" "scripts/verify-universal-forms-delivery-status-json.sh" "schema file disallows release gate additional properties"
contains "delivery status JSON verifier checks release mode enum" "scripts/verify-universal-forms-delivery-status-json.sh" "JSON release gate mode is allowed by schema"
contains "delivery status JSON verifier checks next manual key enum" "scripts/verify-universal-forms-delivery-status-json.sh" "JSON next manual key is allowed by schema"
contains "delivery status JSON verifier checks next manual reviewer check" "scripts/verify-universal-forms-delivery-status-json.sh" "next manual reviewer check matches signoff status JSON"
contains "delivery status JSON verifier checks next manual automated evidence" "scripts/verify-universal-forms-delivery-status-json.sh" "next manual automated evidence matches signoff status JSON"
contains "delivery status JSON verifier checks nested required keys" "scripts/verify-universal-forms-delivery-status-json.sh" "JSON next manual signoff object matches schema required keys"
contains "delivery status JSON verifier rejects nested extra fields" "scripts/verify-universal-forms-delivery-status-json.sh" "JSON release gate object has no extra keys beyond schema"
contains "delivery status JSON contract smoke checks extra fields" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "extra top-level property"
contains "delivery status JSON contract smoke checks nested missing fields" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "missing release gate required key"
contains "delivery status JSON contract smoke checks missing review surfaces" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "missing review surfaces required key"
contains "delivery status JSON contract smoke checks review surface drift" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "review surface path drift"
contains "delivery status JSON contract smoke checks missing prerequisites" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "missing verified prerequisites"
contains "delivery status JSON contract smoke checks missing completion summary" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "missing completion summary required key"
contains "delivery status JSON contract smoke checks missing completion breakdown" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "missing completion breakdown required key"
contains "delivery status JSON contract smoke checks completion breakdown drift" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "completion breakdown overall status drift"
contains "delivery status JSON contract smoke checks missing release blockers" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "missing release blockers"
contains "delivery status JSON contract smoke checks release blocker drift" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "release blocker count drift"
contains "delivery status JSON contract smoke checks missing next actions" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "missing next actions"
contains "delivery status JSON contract smoke checks next action drift" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "next action blocked_by drift"
contains "delivery status JSON contract smoke checks missing manual signoff queue" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "missing manual signoff queue"
contains "delivery status JSON contract smoke checks manual signoff queue drift" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "manual signoff queue order drift"
contains "delivery status JSON contract smoke checks delivery state drift" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "completion summary delivery state drift"
contains "delivery status JSON contract smoke checks prerequisite order drift" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "verified prerequisite order drift"
contains "delivery status JSON contract smoke checks nested extra fields" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "extra release gate property"
contains "delivery status JSON contract smoke checks release flag drift" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "release gate allowed flag drift"
contains "delivery status JSON contract smoke checks unknown next manual key" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "unknown next manual signoff key"
contains "delivery status JSON contract smoke checks next reviewer drift" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "next manual reviewer check drift"
contains "delivery status JSON contract smoke checks next evidence drift" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "next manual automated evidence drift"
contains "delivery status JSON contract smoke checks unknown release gate mode" "scripts/smoke-universal-forms-delivery-status-json-contract.sh" "unknown release gate mode"
contains "delivery status output smoke checks text mirrors JSON" "scripts/smoke-universal-forms-delivery-status-output.sh" "text output mirrors stage"
contains "delivery status output smoke checks manual accepted count" "scripts/smoke-universal-forms-delivery-status-output.sh" "text output mirrors manual accepted rows"
contains "delivery status output smoke checks overall completed count" "scripts/smoke-universal-forms-delivery-status-output.sh" "text output mirrors overall completed items"
contains "delivery status output smoke checks overall percent" "scripts/smoke-universal-forms-delivery-status-output.sh" "text output mirrors overall completion percent"
contains "delivery status output smoke checks progress breakdown" "scripts/smoke-universal-forms-delivery-status-output.sh" "text output mirrors overall handoff progress breakdown"
contains "delivery status output smoke checks release blockers" "scripts/smoke-universal-forms-delivery-status-output.sh" "text output mirrors manual signoff release blocker"
contains "delivery status output smoke checks next actions" "scripts/smoke-universal-forms-delivery-status-output.sh" "text output mirrors verify manual signoff action"
contains "delivery status output smoke checks signoff dashboard path" "scripts/smoke-universal-forms-delivery-status-output.sh" "text output mirrors signoff dashboard path"
contains "delivery status output smoke checks next reviewer check" "scripts/smoke-universal-forms-delivery-status-output.sh" "text output mirrors next manual reviewer check"
contains "delivery status output smoke checks next automated evidence" "scripts/smoke-universal-forms-delivery-status-output.sh" "text output mirrors next manual automated evidence"
contains "delivery status output smoke checks next recorder command" "scripts/smoke-universal-forms-delivery-status-output.sh" "text output mirrors recorder command"
contains "delivery status output smoke rejects null leakage" "scripts/smoke-universal-forms-delivery-status-output.sh" "text output does not leak JSON nulls"
contains "delivery status output smoke checks JSON stderr cleanliness" "scripts/smoke-universal-forms-delivery-status-output.sh" "delivery status JSON output leaves stderr empty on success"
contains "delivery status output smoke checks text stderr cleanliness" "scripts/smoke-universal-forms-delivery-status-output.sh" "delivery status text output leaves stderr empty on success"
contains "README documents delivery status JSON is on-demand" "README.md" "intentionally generated on demand"
contains "README documents delivery status JSON manifest count dependency" "README.md" "manifest file count"
contains "README documents delivery status completion summary" "README.md" "completion_summary"
contains "README documents delivery status completion breakdown" "README.md" "completion_breakdown"
contains "README documents delivery status release blockers" "README.md" "release_blockers"
contains "README documents delivery status next actions" "README.md" "next_actions"
contains "README documents delivery status next reviewer check" "README.md" "next row's automated evidence, reviewer check"
contains "README documents delivery status manual signoff queue" "README.md" "manual_signoff_queue"
contains "README documents delivery status review surfaces" "README.md" "review_surfaces"
contains "delivery bundle audit checks release gate" "scripts/audit-universal-forms-delivery-bundle.sh" "Release gate"
contains "delivery bundle audit runs release gate smoke" "scripts/audit-universal-forms-delivery-bundle.sh" "release gate smoke passes"
contains "delivery bundle audit runs release gate output smoke" "scripts/audit-universal-forms-delivery-bundle.sh" "release gate output smoke passes"
contains "delivery bundle audit runs release gate JSON verifier" "scripts/audit-universal-forms-delivery-bundle.sh" "release gate JSON verifier passes"
contains "delivery bundle audit runs release gate JSON contract smoke" "scripts/audit-universal-forms-delivery-bundle.sh" "release gate JSON contract smoke passes"
contains "delivery bundle refresh regenerates acceptance evidence" "scripts/refresh-universal-forms-delivery-bundle.sh" "acceptance-universal-forms.sh"
contains "delivery bundle refresh runs bundle audit" "scripts/refresh-universal-forms-delivery-bundle.sh" "audit-universal-forms-delivery-bundle.sh"
contains "delivery bundle refresh quick mode uses temporary report" "scripts/refresh-universal-forms-delivery-bundle.sh" "quick_report="
contains "acceptance evidence refresh preserves manual signoff rows" "scripts/acceptance-universal-forms.sh" "capture_existing_manual_signoff_rows"
contains "acceptance evidence refresh reuses preserved manual signoff rows" "scripts/acceptance-universal-forms.sh" "PRESERVED_MANUAL_SIGNOFF_ROWS"
contains "acceptance evidence writes a same-directory temporary output" "scripts/acceptance-universal-forms.sh" 'mktemp "$(dirname "$REPORT_TARGET_PATH")/'
contains "acceptance evidence preserves existing output permissions" "scripts/acceptance-universal-forms.sh" 'chmod --reference="$REPORT_TARGET_PATH" "$output_tmp"'
contains "acceptance evidence atomically publishes output" "scripts/acceptance-universal-forms.sh" 'mv -f "$output_tmp" "$REPORT_TARGET_PATH"'
contains "delivery-state audit requires runbook consistency" "scripts/audit-universal-forms-delivery-state.sh" "runbook/evidence manual signoff consistency passes"
contains "delivery manifest records checksums" "scripts/prepare-universal-forms-delivery-manifest.sh" "sha256sum"
contains "delivery manifest JSON schema exists" "docs/schemas/openpr-universal-forms-delivery-manifest.schema.json" "openpr.universal_forms.delivery_manifest.v1"
contains "delivery manifest JSON reporter emits schema version" "scripts/report-universal-forms-delivery-manifest-json.sh" "openpr.universal_forms.delivery_manifest.v1"
contains "delivery manifest JSON reporter mirrors markdown rows" "scripts/report-universal-forms-delivery-manifest-json.sh" "files_json"
contains "delivery manifest JSON verifier checks schema version" "scripts/verify-universal-forms-delivery-manifest-json.sh" "schema file pins delivery manifest JSON v1"
contains "delivery manifest JSON verifier checks schema additional properties" "scripts/verify-universal-forms-delivery-manifest-json.sh" "schema file disallows top-level additional properties"
contains "delivery manifest JSON verifier checks nested schema additional properties" "scripts/verify-universal-forms-delivery-manifest-json.sh" "schema file disallows file row additional properties"
contains "delivery manifest JSON verifier checks schema file checksum requirement" "scripts/verify-universal-forms-delivery-manifest-json.sh" "schema file requires file row sha256"
contains "delivery manifest JSON verifier checks top-level keys" "scripts/verify-universal-forms-delivery-manifest-json.sh" "top-level keys are pinned"
contains "delivery manifest JSON verifier checks markdown mirror" "scripts/verify-universal-forms-delivery-manifest-json.sh" "JSON file rows mirror Markdown manifest exactly"
contains "delivery manifest JSON verifier checks live files" "scripts/verify-universal-forms-delivery-manifest-json.sh" "sha256 matches live file"
contains "delivery manifest JSON contract smoke exists" "scripts/smoke-universal-forms-delivery-manifest-json-contract.sh" "missing schema path"
contains "delivery manifest JSON contract smoke checks extra fields" "scripts/smoke-universal-forms-delivery-manifest-json-contract.sh" "extra top-level property"
contains "delivery manifest JSON contract smoke checks nested extra fields" "scripts/smoke-universal-forms-delivery-manifest-json-contract.sh" "extra file row property"
contains "delivery manifest JSON contract smoke checks typed counters" "scripts/smoke-universal-forms-delivery-manifest-json-contract.sh" "string automated check counter"
contains "delivery manifest JSON contract smoke checks file count drift" "scripts/smoke-universal-forms-delivery-manifest-json-contract.sh" "file count drift"
contains "delivery manifest JSON contract smoke checks checksum drift" "scripts/smoke-universal-forms-delivery-manifest-json-contract.sh" "file row checksum drift"
contains "delivery manifest JSON contract smoke checks row order drift" "scripts/smoke-universal-forms-delivery-manifest-json-contract.sh" "file row order drift"
contains "readiness JSON schema exists" "docs/schemas/openpr-universal-forms-readiness.schema.json" "openpr.universal_forms.readiness.v1"
contains "readiness JSON schema requires schema path" "docs/schemas/openpr-universal-forms-readiness.schema.json" '"schema_path"'
contains "readiness JSON schema defines next row recorder command" "docs/schemas/openpr-universal-forms-readiness.schema.json" '"recorder_command"'
contains "readiness JSON schema requires signoff status JSON path" "docs/schemas/openpr-universal-forms-readiness.schema.json" '"signoff_status_json"'
contains "readiness JSON schema enumerates manual signoff keys" "docs/schemas/openpr-universal-forms-readiness.schema.json" '"restaurant_template"'
contains "readiness JSON schema defines reusable manual key enum" "docs/schemas/openpr-universal-forms-readiness.schema.json" '"manual_key"'
contains "readiness JSON schema constrains next row key" "docs/schemas/openpr-universal-forms-readiness.schema.json" '#/$defs/manual_key'
contains "readiness JSON schema pins manual row order" "docs/schemas/openpr-universal-forms-readiness.schema.json" '"prefixItems"'
contains "manual signoff status JSON schema exists" "docs/schemas/openpr-universal-forms-signoff-status.schema.json" "openpr.universal_forms.signoff_status.v1"
contains "manual signoff status JSON schema requires final signoff flag" "docs/schemas/openpr-universal-forms-signoff-status.schema.json" '"final_signoff_allowed"'
contains "manual signoff status JSON schema requires row recorder command" "docs/schemas/openpr-universal-forms-signoff-status.schema.json" '"recorder_command"'
contains "manual signoff status JSON schema requires automated evidence" "docs/schemas/openpr-universal-forms-signoff-status.schema.json" '"automated_evidence"'
contains "manual signoff status JSON schema requires reviewer check" "docs/schemas/openpr-universal-forms-signoff-status.schema.json" '"reviewer_check"'
contains "manual signoff status JSON schema requires pending queue" "docs/schemas/openpr-universal-forms-signoff-status.schema.json" '"pending_queue"'
contains "manual signoff status JSON schema defines pending queue row" "docs/schemas/openpr-universal-forms-signoff-status.schema.json" '"pending_queue_row"'
contains "manual signoff status JSON schema enumerates manual signoff keys" "docs/schemas/openpr-universal-forms-signoff-status.schema.json" '"restaurant_template"'
contains "manual signoff status JSON schema defines reusable manual key enum" "docs/schemas/openpr-universal-forms-signoff-status.schema.json" '"manual_key"'
contains "manual signoff status JSON schema constrains next row key" "docs/schemas/openpr-universal-forms-signoff-status.schema.json" '#/$defs/manual_key'
contains "manual signoff status JSON schema pins manual row order" "docs/schemas/openpr-universal-forms-signoff-status.schema.json" '"prefixItems"'
contains "readiness JSON reporter emits schema version" "scripts/report-universal-forms-readiness-json.sh" "openpr.universal_forms.readiness.v1"
contains "readiness JSON reporter emits schema path" "scripts/report-universal-forms-readiness-json.sh" "schema_path"
contains "readiness JSON reporter emits next manual row" "scripts/report-universal-forms-readiness-json.sh" "next_row"
contains "readiness JSON reporter emits suggested evidence note" "scripts/report-universal-forms-readiness-json.sh" "suggested_evidence_note"
contains "readiness JSON reporter emits recorder command" "scripts/report-universal-forms-readiness-json.sh" "recorder_command"
contains "readiness JSON reporter emits signoff status JSON path" "scripts/report-universal-forms-readiness-json.sh" "signoff_status_json"
contains "readiness JSON verifier checks schema version" "scripts/verify-universal-forms-readiness-json.sh" "schema version is v1"
contains "readiness JSON verifier checks schema path" "scripts/verify-universal-forms-readiness-json.sh" "JSON schema path matches repository schema"
contains "readiness JSON verifier checks schema next row contract" "scripts/verify-universal-forms-readiness-json.sh" "schema file requires next row recorder command"
contains "readiness JSON verifier checks required keys" "scripts/verify-universal-forms-readiness-json.sh" "JSON has every schema top-level required key"
contains "readiness JSON verifier rejects top-level extras" "scripts/verify-universal-forms-readiness-json.sh" "JSON has no extra top-level keys beyond schema"
contains "readiness JSON verifier rejects nested extra fields" "scripts/verify-universal-forms-readiness-json.sh" "JSON manual row objects have no extra keys beyond schema"
contains "readiness JSON verifier checks manual key enum" "scripts/verify-universal-forms-readiness-json.sh" "JSON manual row keys are allowed by schema"
contains "readiness JSON verifier checks manual row order" "scripts/verify-universal-forms-readiness-json.sh" "JSON manual row keys match schema order"
contains "readiness JSON verifier checks next row key enum" "scripts/verify-universal-forms-readiness-json.sh" "JSON next row key is allowed by schema"
contains "readiness JSON verifier checks stage enum" "scripts/verify-universal-forms-readiness-json.sh" "JSON stage is allowed by schema enum"
contains "readiness JSON verifier checks integer counters" "scripts/verify-universal-forms-readiness-json.sh" "JSON numeric gates are integers"
contains "readiness JSON verifier checks gate status enum" "scripts/verify-universal-forms-readiness-json.sh" "JSON gate status values are allowed by schema"
contains "readiness JSON verifier checks release constants" "scripts/verify-universal-forms-readiness-json.sh" "JSON release requirement constants match schema"
contains "readiness JSON verifier checks pending rows" "scripts/verify-universal-forms-readiness-json.sh" "JSON manual pending rows match evidence"
contains "readiness JSON verifier checks suggested evidence note" "scripts/verify-universal-forms-readiness-json.sh" "JSON next manual evidence note matches signoff status"
contains "readiness JSON verifier checks recorder command" "scripts/verify-universal-forms-readiness-json.sh" "JSON next manual recorder command matches signoff status"
contains "readiness JSON verifier checks signoff status JSON path" "scripts/verify-universal-forms-readiness-json.sh" "JSON signoff status JSON path matches default report"
contains "readiness JSON verifier checks signoff status JSON next command" "scripts/verify-universal-forms-readiness-json.sh" "JSON next manual recorder command matches signoff status JSON"
contains "readiness JSON contract smoke exists" "scripts/smoke-universal-forms-readiness-json-contract.sh" "missing next-row recorder command"
contains "readiness JSON contract smoke checks missing signoff JSON path" "scripts/smoke-universal-forms-readiness-json-contract.sh" "missing signoff status JSON report path"
contains "readiness JSON contract smoke checks extra fields" "scripts/smoke-universal-forms-readiness-json-contract.sh" "extra top-level property"
contains "readiness JSON contract smoke checks nested extra fields" "scripts/smoke-universal-forms-readiness-json-contract.sh" "extra manual row property"
contains "readiness JSON contract smoke checks typed counters" "scripts/smoke-universal-forms-readiness-json-contract.sh" "string automated check counter"
contains "readiness JSON contract smoke checks row order drift" "scripts/smoke-universal-forms-readiness-json-contract.sh" "manual row order drift"
contains "readiness JSON contract smoke checks unknown next row key" "scripts/smoke-universal-forms-readiness-json-contract.sh" "unknown next manual row key"
contains "readiness JSON contract smoke checks release constants" "scripts/smoke-universal-forms-readiness-json-contract.sh" "release requirement constant drift"
contains "development status JSON schema exists" "docs/schemas/openpr-universal-forms-development-status.schema.json" "openpr.universal_forms.development_status.v1"
contains "development status JSON schema enumerates matrix keys" "docs/schemas/openpr-universal-forms-development-status.schema.json" "user_manual_acceptance"
contains "development status JSON schema pins row order" "docs/schemas/openpr-universal-forms-development-status.schema.json" '"prefixItems"'
contains "development status JSON schema requires non-empty phase" "docs/schemas/openpr-universal-forms-development-status.schema.json" '"phase": { "type": "string", "minLength": 1 }'
contains "development status JSON schema requires non-empty completion rule" "docs/schemas/openpr-universal-forms-development-status.schema.json" '"completion_rule": { "type": "string", "minLength": 1 }'
contains "development status JSON reporter emits schema version" "scripts/report-universal-forms-development-status-json.sh" "openpr.universal_forms.development_status.v1"
contains "development status JSON reporter reads tracker matrix" "scripts/report-universal-forms-development-status-json.sh" "开发阶段准入与交付判定矩阵"
contains "development status JSON reporter emits final release flag" "scripts/report-universal-forms-development-status-json.sh" "final_release_allowed"
contains "development status JSON verifier checks schema version" "scripts/verify-universal-forms-development-status-json.sh" "schema version is v1"
contains "development status JSON verifier checks top-level extras" "scripts/verify-universal-forms-development-status-json.sh" "JSON has no extra top-level keys beyond schema"
contains "development status JSON verifier checks nested extras" "scripts/verify-universal-forms-development-status-json.sh" "JSON rows have no extra keys beyond schema"
contains "development status JSON verifier checks row keys" "scripts/verify-universal-forms-development-status-json.sh" "JSON row keys exactly match schema enum"
contains "development status JSON verifier checks row order" "scripts/verify-universal-forms-development-status-json.sh" "JSON row keys match schema order"
contains "development status JSON verifier checks tracker mirror" "scripts/verify-universal-forms-development-status-json.sh" "JSON status mirrors tracker matrix"
contains "development status JSON verifier checks phase mirror" "scripts/verify-universal-forms-development-status-json.sh" "JSON phase mirrors tracker matrix"
contains "development status JSON verifier checks requirement mirror" "scripts/verify-universal-forms-development-status-json.sh" "JSON engineering requirement mirrors tracker matrix"
contains "development status JSON verifier checks completion mirror" "scripts/verify-universal-forms-development-status-json.sh" "JSON completion rule mirrors tracker matrix"
contains "development status JSON verifier checks readiness pending count" "scripts/verify-universal-forms-development-status-json.sh" "JSON manual pending count matches readiness JSON"
contains "development status JSON contract smoke exists" "scripts/smoke-universal-forms-development-status-json-contract.sh" "missing schema path"
contains "development status JSON contract smoke checks extra fields" "scripts/smoke-universal-forms-development-status-json-contract.sh" "extra top-level property"
contains "development status JSON contract smoke checks nested extra fields" "scripts/smoke-universal-forms-development-status-json-contract.sh" "extra row property"
contains "development status JSON contract smoke checks row order drift" "scripts/smoke-universal-forms-development-status-json-contract.sh" "row order drift"
contains "development status JSON contract smoke checks phase drift" "scripts/smoke-universal-forms-development-status-json-contract.sh" "row phase drift"
contains "development status JSON contract smoke checks completion drift" "scripts/smoke-universal-forms-development-status-json-contract.sh" "completion rule drift"
contains "development status JSON contract smoke checks row drift" "scripts/smoke-universal-forms-development-status-json-contract.sh" "row count drift"
contains "completion audit JSON schema exists" "docs/schemas/openpr-universal-forms-completion-audit.schema.json" "openpr.universal_forms.completion_audit.v1"
contains "completion audit JSON reporter emits schema version" "scripts/report-universal-forms-completion-audit-json.sh" "openpr.universal_forms.completion_audit.v1"
contains "completion audit JSON reporter emits automated completion flag" "scripts/report-universal-forms-completion-audit-json.sh" "automated_delivery_complete"
contains "completion audit JSON verifier checks evidence parity" "scripts/verify-universal-forms-completion-audit-json.sh" "automated check count matches evidence"
contains "completion audit JSON verifier checks signoff parity" "scripts/verify-universal-forms-completion-audit-json.sh" "manual pending rows match signoff status JSON"
contains "completion audit JSON verifier rejects nested extra fields" "scripts/verify-universal-forms-completion-audit-json.sh" "JSON finalization object has no extra keys beyond schema"
contains "completion audit JSON contract smoke checks nested extra fields" "scripts/smoke-universal-forms-completion-audit-json-contract.sh" "extra finalization property"
contains "completion audit JSON contract smoke checks conclusion drift" "scripts/smoke-universal-forms-completion-audit-json-contract.sh" "conclusion drift"
contains "scenario catalog JSON schema exists" "docs/schemas/openpr-universal-forms-scenario-catalog.schema.json" "openpr.universal_forms.scenario_catalog.v1"
contains "scenario catalog JSON schema enumerates templates" "docs/schemas/openpr-universal-forms-scenario-catalog.schema.json" "restaurant_ordering_default"
contains "scenario catalog JSON schema pins operator entrypoints" "docs/schemas/openpr-universal-forms-scenario-catalog.schema.json" "operator_entrypoints"
contains "scenario catalog JSON schema requires operator steps" "docs/schemas/openpr-universal-forms-scenario-catalog.schema.json" "operator_steps"
contains "scenario catalog JSON schema requires connector kinds" "docs/schemas/openpr-universal-forms-scenario-catalog.schema.json" "connector_kinds"
contains "scenario catalog JSON schema pins template order" "docs/schemas/openpr-universal-forms-scenario-catalog.schema.json" '"prefixItems"'
contains "scenario catalog JSON reporter emits schema version" "scripts/report-universal-forms-scenario-catalog-json.sh" "openpr.universal_forms.scenario_catalog.v1"
contains "scenario catalog JSON reporter reads built-in template table" "scripts/report-universal-forms-scenario-catalog-json.sh" "Built-In Templates"
contains "scenario catalog JSON reporter emits operator entrypoints" "scripts/report-universal-forms-scenario-catalog-json.sh" "operator_entrypoints"
contains "scenario catalog JSON reporter emits template usage" "scripts/report-universal-forms-scenario-catalog-json.sh" "usage_for"
contains "scenario catalog JSON verifier checks markdown parity" "scripts/verify-universal-forms-scenario-catalog-json.sh" "JSON project type mirrors Markdown"
contains "scenario catalog JSON verifier checks operator entrypoints" "scripts/verify-universal-forms-scenario-catalog-json.sh" "JSON operator entrypoint keys match schema order"
contains "scenario catalog JSON verifier checks MCP creation tools" "scripts/verify-universal-forms-scenario-catalog-json.sh" "JSON template primary MCP tools include project creation"
contains "scenario catalog JSON verifier checks connector kinds" "scripts/verify-universal-forms-scenario-catalog-json.sh" "JSON template connector kinds include MCP"
contains "scenario catalog JSON verifier checks nested extras" "scripts/verify-universal-forms-scenario-catalog-json.sh" "JSON templates have no extra keys beyond schema"
contains "scenario catalog JSON verifier checks template order" "scripts/verify-universal-forms-scenario-catalog-json.sh" "JSON template keys match schema order"
contains "scenario catalog JSON verifier checks smoke coverage" "scripts/verify-universal-forms-scenario-catalog-json.sh" "scenario smoke covers template"
contains "scenario catalog JSON verifier checks restaurant plugin" "scripts/verify-universal-forms-scenario-catalog-json.sh" "restaurant template includes restaurant_calc integration"
contains "scenario catalog JSON verifier checks restaurant print connector" "scripts/verify-universal-forms-scenario-catalog-json.sh" "restaurant template includes print connector kind"
contains "scenario catalog JSON contract smoke exists" "scripts/smoke-universal-forms-scenario-catalog-json-contract.sh" "missing schema path"
contains "scenario catalog JSON contract smoke checks operator entrypoint order" "scripts/smoke-universal-forms-scenario-catalog-json-contract.sh" "operator entrypoint order drift"
contains "scenario catalog JSON contract smoke checks operator steps" "scripts/smoke-universal-forms-scenario-catalog-json-contract.sh" "missing operator steps"
contains "scenario catalog JSON contract smoke checks unknown key" "scripts/smoke-universal-forms-scenario-catalog-json-contract.sh" "unknown template key"
contains "scenario catalog JSON contract smoke checks nested extras" "scripts/smoke-universal-forms-scenario-catalog-json-contract.sh" "extra template property"
contains "scenario catalog JSON contract smoke checks template order drift" "scripts/smoke-universal-forms-scenario-catalog-json-contract.sh" "template order drift"
contains "scenario catalog JSON contract smoke checks restaurant plugin" "scripts/smoke-universal-forms-scenario-catalog-json-contract.sh" "missing restaurant_calc integration"
contains "scenario catalog JSON contract smoke checks restaurant print connector" "scripts/smoke-universal-forms-scenario-catalog-json-contract.sh" "missing restaurant print connector kind"
contains "implementation map JSON schema exists" "docs/schemas/openpr-universal-forms-implementation-map.schema.json" "openpr.universal_forms.implementation_map.v1"
contains "implementation map JSON schema enumerates module keys" "docs/schemas/openpr-universal-forms-implementation-map.schema.json" "user_side_manual_acceptance"
contains "implementation map JSON schema pins module order" "docs/schemas/openpr-universal-forms-implementation-map.schema.json" '"prefixItems"'
contains "implementation map JSON reporter emits schema version" "scripts/report-universal-forms-implementation-map-json.sh" "openpr.universal_forms.implementation_map.v1"
contains "implementation map JSON reporter reads module map" "scripts/report-universal-forms-implementation-map-json.sh" "Module Map"
contains "implementation map JSON verifier checks Markdown parity" "scripts/verify-universal-forms-implementation-map-json.sh" "JSON delivery area mirrors Markdown"
contains "implementation map JSON verifier checks nested extras" "scripts/verify-universal-forms-implementation-map-json.sh" "JSON release boundary has no extra keys beyond schema"
contains "implementation map JSON verifier checks module order" "scripts/verify-universal-forms-implementation-map-json.sh" "JSON module keys match schema order"
contains "implementation map JSON verifier checks implementation paths" "scripts/verify-universal-forms-implementation-map-json.sh" "implementation path exists"
contains "implementation map JSON verifier checks manual pending row" "scripts/verify-universal-forms-implementation-map-json.sh" "manual acceptance module remains pending"
contains "implementation map JSON contract smoke exists" "scripts/smoke-universal-forms-implementation-map-json-contract.sh" "missing schema path"
contains "implementation map JSON contract smoke checks nested extras" "scripts/smoke-universal-forms-implementation-map-json-contract.sh" "extra module property"
contains "implementation map JSON contract smoke checks module order drift" "scripts/smoke-universal-forms-implementation-map-json-contract.sh" "module order drift"
contains "implementation map JSON contract smoke checks module count drift" "scripts/smoke-universal-forms-implementation-map-json-contract.sh" "module count drift"
contains "implementation map JSON contract smoke checks manual marker drift" "scripts/smoke-universal-forms-implementation-map-json-contract.sh" "manual marker drift"
contains "implementation map JSON contract smoke checks path drift" "scripts/smoke-universal-forms-implementation-map-json-contract.sh" "missing implementation path"

printf '\nMachine-readable reporter atomicity coverage:\n'
for path in \
  scripts/report-universal-forms-completion-audit-json.sh \
  scripts/report-universal-forms-signoff-status-json.sh \
  scripts/report-universal-forms-readiness-json.sh \
  scripts/report-universal-forms-development-status-json.sh \
  scripts/report-universal-forms-scenario-catalog-json.sh \
  scripts/report-universal-forms-implementation-map-json.sh \
  scripts/report-universal-forms-delivery-manifest-json.sh; do
  contains "JSON reporter writes a same-directory temporary output: $path" "$path" 'mktemp "$(dirname "$OUTPUT_PATH")/'
  contains "JSON reporter preserves existing output permissions: $path" "$path" 'chmod --reference="$OUTPUT_PATH" "$output_tmp"'
  contains "JSON reporter atomically publishes output: $path" "$path" 'mv -f "$output_tmp" "$OUTPUT_PATH"'
done

printf '\nGenerated handoff report atomicity coverage:\n'
for path in \
  scripts/report-universal-forms-completion-audit.sh \
  scripts/prepare-universal-forms-manual-evidence-map.sh \
  scripts/report-universal-forms-signoff-status.sh \
  scripts/prepare-universal-forms-next-signoff-review.sh \
  scripts/prepare-universal-forms-user-acceptance-packet.sh \
  scripts/report-universal-forms-readiness-summary.sh \
  scripts/prepare-universal-forms-ui-review-gallery.sh \
  scripts/prepare-universal-forms-signoff-dashboard.sh \
  scripts/prepare-universal-forms-delivery-manifest.sh; do
  contains "handoff reporter writes a same-directory temporary output: $path" "$path" 'mktemp "$(dirname "$OUTPUT_PATH")/'
  contains "handoff reporter preserves existing output permissions: $path" "$path" 'chmod --reference="$OUTPUT_PATH" "$output_tmp"'
  contains "handoff reporter atomically publishes output: $path" "$path" 'mv -f "$output_tmp" "$OUTPUT_PATH"'
done

contains "delivery bundle refresh runs readiness JSON" "scripts/refresh-universal-forms-delivery-bundle.sh" "report-universal-forms-readiness-json.sh"
contains "delivery bundle refresh verifies readiness JSON" "scripts/refresh-universal-forms-delivery-bundle.sh" "verify-universal-forms-readiness-json.sh"
contains "delivery bundle refresh runs readiness JSON contract smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-readiness-json-contract.sh"
contains "delivery bundle refresh runs development status JSON" "scripts/refresh-universal-forms-delivery-bundle.sh" "report-universal-forms-development-status-json.sh"
contains "delivery bundle refresh verifies development status JSON" "scripts/refresh-universal-forms-delivery-bundle.sh" "verify-universal-forms-development-status-json.sh"
contains "delivery bundle refresh runs development status JSON contract smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-development-status-json-contract.sh"
contains "delivery bundle refresh runs completion audit JSON" "scripts/refresh-universal-forms-delivery-bundle.sh" "report-universal-forms-completion-audit-json.sh"
contains "delivery bundle refresh verifies completion audit JSON" "scripts/refresh-universal-forms-delivery-bundle.sh" "verify-universal-forms-completion-audit-json.sh"
contains "delivery bundle refresh runs completion audit JSON contract smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-completion-audit-json-contract.sh"
contains "delivery bundle refresh runs scenario catalog JSON" "scripts/refresh-universal-forms-delivery-bundle.sh" "report-universal-forms-scenario-catalog-json.sh"
contains "delivery bundle refresh verifies scenario catalog JSON" "scripts/refresh-universal-forms-delivery-bundle.sh" "verify-universal-forms-scenario-catalog-json.sh"
contains "delivery bundle refresh runs scenario catalog JSON contract smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-scenario-catalog-json-contract.sh"
contains "delivery bundle refresh runs implementation map JSON" "scripts/refresh-universal-forms-delivery-bundle.sh" "report-universal-forms-implementation-map-json.sh"
contains "delivery bundle refresh verifies implementation map JSON" "scripts/refresh-universal-forms-delivery-bundle.sh" "verify-universal-forms-implementation-map-json.sh"
contains "delivery bundle refresh runs implementation map JSON contract smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-implementation-map-json-contract.sh"
contains "delivery bundle refresh runs report output boundary smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-report-output-boundaries.sh"
contains "delivery bundle refresh runs delivery manifest JSON" "scripts/refresh-universal-forms-delivery-bundle.sh" "report-universal-forms-delivery-manifest-json.sh"
contains "delivery bundle refresh verifies delivery manifest JSON" "scripts/refresh-universal-forms-delivery-bundle.sh" "verify-universal-forms-delivery-manifest-json.sh"
contains "delivery bundle refresh runs delivery manifest JSON contract smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-delivery-manifest-json-contract.sh"
contains "delivery bundle refresh runs next signoff review" "scripts/refresh-universal-forms-delivery-bundle.sh" "prepare-universal-forms-next-signoff-review.sh"
contains "delivery bundle refresh runs signoff dashboard" "scripts/refresh-universal-forms-delivery-bundle.sh" "prepare-universal-forms-signoff-dashboard.sh"
contains "delivery bundle refresh verifies signoff dashboard" "scripts/refresh-universal-forms-delivery-bundle.sh" "verify-universal-forms-signoff-dashboard.sh"
contains "delivery bundle refresh renders signoff dashboard" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-signoff-dashboard-render.sh"
contains "delivery bundle refresh runs signoff dashboard progression smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-signoff-dashboard-progression.sh"
contains "signoff dashboard exposes Start Here handoff" "scripts/prepare-universal-forms-signoff-dashboard.sh" "Start Here"
contains "signoff dashboard verifier checks Start Here handoff" "scripts/verify-universal-forms-signoff-dashboard.sh" "dashboard exposes Start Here handoff section"
contains "signoff dashboard verifier checks status command" "scripts/verify-universal-forms-signoff-dashboard.sh" "scripts/status-universal-forms-delivery.sh"
contains "signoff dashboard render smoke reads current status JSON" "scripts/smoke-universal-forms-signoff-dashboard-render.sh" "OPENPR_SIGNOFF_STATUS_JSON"
contains "signoff dashboard render smoke derives queue count" "scripts/smoke-universal-forms-signoff-dashboard-render.sh" "expected_queue_count"
contains "signoff dashboard render smoke derives next key" "scripts/smoke-universal-forms-signoff-dashboard-render.sh" "expected_next_key"
contains "signoff dashboard render smoke checks Start Here key" "scripts/smoke-universal-forms-signoff-dashboard-render.sh" "data-signoff-start-key"
contains "signoff dashboard render smoke suppresses Chromium screenshot chatter" "scripts/smoke-universal-forms-signoff-dashboard-render.sh" "stderr_path"
contains "signoff dashboard render smoke replays Chromium failures" "scripts/smoke-universal-forms-signoff-dashboard-render.sh" "Chromium render failed"
contains "signoff dashboard exposes finalization action" "scripts/prepare-universal-forms-signoff-dashboard.sh" "Finalization Action"
contains "signoff dashboard exposes finalizer command" "scripts/prepare-universal-forms-signoff-dashboard.sh" "scripts/finalize-universal-forms-acceptance.sh"
contains "signoff dashboard verifier checks finalizer command" "scripts/verify-universal-forms-signoff-dashboard.sh" "dashboard exposes finalizer command"
contains "signoff dashboard render smoke checks finalizer command" "scripts/smoke-universal-forms-signoff-dashboard-render.sh" "Rendered signoff dashboard did not expose finalizer command"
contains "signoff dashboard progression smoke covers first-row advancement" "scripts/smoke-universal-forms-signoff-dashboard-progression.sh" "frontend_usability"
contains "signoff dashboard progression smoke covers finalization action" "scripts/smoke-universal-forms-signoff-dashboard-progression.sh" "Finalization Action"
contains "signoff dashboard progression smoke keeps status markdown temporary" "scripts/smoke-universal-forms-signoff-dashboard-progression.sh" '--markdown "$tmp_markdown"'
contains "signoff dashboard progression smoke verifies official files unchanged" "scripts/smoke-universal-forms-signoff-dashboard-progression.sh" "official file remains unchanged"
contains "signoff dashboard progression smoke renders temporary dashboards" "scripts/smoke-universal-forms-signoff-dashboard-progression.sh" "smoke-universal-forms-signoff-dashboard-render.sh"
contains "signoff dashboard progression smoke suppresses successful subcommand chatter" "scripts/smoke-universal-forms-signoff-dashboard-progression.sh" "run_quiet"
contains "signoff dashboard progression smoke replays failed subcommand output" "scripts/smoke-universal-forms-signoff-dashboard-progression.sh" "stderr:"
contains "delivery bundle refresh runs signoff status output smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-signoff-status-output.sh"
contains "delivery bundle refresh verifies next signoff review" "scripts/refresh-universal-forms-delivery-bundle.sh" "verify-universal-forms-next-signoff-review.sh"
contains "delivery bundle refresh runs next signoff review contract smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-next-signoff-review-contract.sh"
contains "delivery bundle refresh runs next signoff command smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-next-signoff-command.sh"
contains "delivery bundle refresh runs manual signoff progression smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-manual-signoff-progression.sh"
contains "delivery bundle refresh runs all manual signoff command smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-manual-signoff-commands.sh"
contains "delivery bundle refresh runs release gate smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-release-gate.sh"
contains "delivery bundle refresh runs release gate JSON verifier" "scripts/refresh-universal-forms-delivery-bundle.sh" "verify-universal-forms-release-gate-json.sh"
contains "delivery bundle refresh runs release gate JSON contract smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-release-gate-json-contract.sh"
contains "delivery bundle refresh runs release gate output smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-release-gate-output.sh"
contains "delivery bundle refresh runs delivery status command" "scripts/refresh-universal-forms-delivery-bundle.sh" "status-universal-forms-delivery.sh"
contains "delivery bundle refresh verifies delivery status JSON" "scripts/refresh-universal-forms-delivery-bundle.sh" "verify-universal-forms-delivery-status-json.sh"
contains "delivery bundle refresh runs delivery status JSON contract smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-delivery-status-json-contract.sh"
contains "delivery bundle refresh runs delivery status output smoke" "scripts/refresh-universal-forms-delivery-bundle.sh" "smoke-universal-forms-delivery-status-output.sh"
contains "delivery manifest includes readiness JSON" "scripts/prepare-universal-forms-delivery-manifest.sh" "readiness JSON"
contains "delivery manifest includes development status JSON" "scripts/prepare-universal-forms-delivery-manifest.sh" "development status JSON"
contains "delivery manifest includes completion audit JSON" "scripts/prepare-universal-forms-delivery-manifest.sh" "completion audit JSON"
contains "delivery manifest includes scenario catalog JSON" "scripts/prepare-universal-forms-delivery-manifest.sh" "scenario catalog JSON"
contains "delivery manifest includes implementation map JSON" "scripts/prepare-universal-forms-delivery-manifest.sh" "implementation map JSON"
contains "delivery manifest includes implementation map" "scripts/prepare-universal-forms-delivery-manifest.sh" "implementation map"
contains "delivery manifest includes implementation map verifier" "scripts/prepare-universal-forms-delivery-manifest.sh" "implementation map verifier"
contains "delivery manifest includes implementation map contract smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "implementation map contract smoke"
contains "delivery bundle audit runs implementation map verifier" "scripts/audit-universal-forms-delivery-bundle.sh" "implementation map verifier passes"
contains "delivery bundle audit runs implementation map contract smoke" "scripts/audit-universal-forms-delivery-bundle.sh" "implementation map contract smoke passes"
contains "implementation map verifier checks module rows" "scripts/verify-universal-forms-implementation-map.sh" "module map has 12 delivery rows"
contains "implementation map verifier checks source paths" "scripts/verify-universal-forms-implementation-map.sh" "implementation reference exists"
contains "implementation map verifier checks verification commands" "scripts/verify-universal-forms-implementation-map.sh" "verification command exists"
contains "implementation map contract smoke rejects missing row" "scripts/smoke-universal-forms-implementation-map-contract.sh" "missing delivery row"
contains "implementation map contract smoke rejects marker drift" "scripts/smoke-universal-forms-implementation-map-contract.sh" "manual marker drift"
contains "implementation map contract smoke rejects path drift" "scripts/smoke-universal-forms-implementation-map-contract.sh" "missing implementation path"
contains "readiness summary generator links implementation map" "scripts/report-universal-forms-readiness-summary.sh" "universal-forms-implementation-map.md"
contains "user acceptance packet generator links implementation map" "scripts/prepare-universal-forms-user-acceptance-packet.sh" "universal-forms-implementation-map.md"
contains "delivery manifest includes readiness JSON verifier" "scripts/prepare-universal-forms-delivery-manifest.sh" "readiness JSON verifier"
contains "delivery manifest includes development status JSON verifier" "scripts/prepare-universal-forms-delivery-manifest.sh" "development status JSON verifier"
contains "delivery manifest includes development status JSON contract smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "development status JSON contract smoke"
contains "delivery manifest includes completion audit JSON verifier" "scripts/prepare-universal-forms-delivery-manifest.sh" "completion audit JSON verifier"
contains "delivery manifest includes completion audit JSON contract smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "completion audit JSON contract smoke"
contains "delivery manifest includes completion audit JSON schema" "scripts/prepare-universal-forms-delivery-manifest.sh" "completion audit JSON schema"
contains "delivery manifest includes scenario catalog JSON verifier" "scripts/prepare-universal-forms-delivery-manifest.sh" "scenario catalog JSON verifier"
contains "delivery manifest includes scenario catalog JSON contract smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "scenario catalog JSON contract smoke"
contains "delivery manifest includes implementation map JSON verifier" "scripts/prepare-universal-forms-delivery-manifest.sh" "implementation map JSON verifier"
contains "delivery manifest includes implementation map JSON contract smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "implementation map JSON contract smoke"
contains "delivery manifest includes implementation map JSON schema" "scripts/prepare-universal-forms-delivery-manifest.sh" "implementation map JSON schema"
contains "delivery manifest includes readiness JSON contract smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "readiness JSON contract smoke"
contains "delivery manifest includes delivery manifest JSON verifier" "scripts/prepare-universal-forms-delivery-manifest.sh" "delivery manifest JSON verifier"
contains "delivery manifest includes delivery manifest JSON contract smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "delivery manifest JSON contract smoke"
contains "delivery manifest includes delivery manifest JSON schema" "scripts/prepare-universal-forms-delivery-manifest.sh" "delivery manifest JSON schema"
contains "delivery manifest includes CI universal forms gates" "scripts/prepare-universal-forms-delivery-manifest.sh" "CI universal forms gates"
contains "delivery manifest includes security scope audit" "scripts/prepare-universal-forms-delivery-manifest.sh" "security scope audit"
contains "delivery manifest includes MCP skill guide" "scripts/prepare-universal-forms-delivery-manifest.sh" "MCP skill guide"
contains "delivery manifest includes MCP skill validator" "scripts/prepare-universal-forms-delivery-manifest.sh" "MCP skill validator"
contains "delivery manifest includes MCP skill regression" "scripts/prepare-universal-forms-delivery-manifest.sh" "MCP skill regression"
contains "delivery manifest includes project template UI screenshots" "scripts/prepare-universal-forms-delivery-manifest.sh" "project-template-wizard-mobile.png"
contains "delivery manifest includes template work item UI screenshots" "scripts/prepare-universal-forms-delivery-manifest.sh" "template-work-items-mobile.png"
contains "delivery manifest includes UI artifact collector" "scripts/prepare-universal-forms-delivery-manifest.sh" "UI artifact collector"
contains "delivery manifest includes UI review gallery" "scripts/prepare-universal-forms-delivery-manifest.sh" "UI review gallery"
contains "delivery manifest includes signoff dashboard" "scripts/prepare-universal-forms-delivery-manifest.sh" "signoff dashboard"
contains "delivery manifest includes signoff dashboard render smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "signoff dashboard browser render smoke"
contains "delivery manifest includes signoff dashboard progression smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "signoff dashboard progression smoke"
contains "delivery manifest includes manual signoff status report" "scripts/prepare-universal-forms-delivery-manifest.sh" "manual signoff status report"
contains "delivery manifest includes manual signoff status JSON" "scripts/prepare-universal-forms-delivery-manifest.sh" "manual signoff status JSON"
contains "delivery manifest includes manual signoff status JSON schema" "scripts/prepare-universal-forms-delivery-manifest.sh" "manual signoff status JSON schema"
contains "delivery manifest includes signoff status output smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "manual signoff status output smoke"
contains "delivery manifest includes next signoff review" "scripts/prepare-universal-forms-delivery-manifest.sh" "next signoff review"
contains "delivery manifest includes next signoff review generator" "scripts/prepare-universal-forms-delivery-manifest.sh" "next signoff review generator"
contains "delivery manifest includes next signoff review verifier" "scripts/prepare-universal-forms-delivery-manifest.sh" "next signoff review verifier"
contains "delivery manifest includes next signoff review contract smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "next signoff review contract smoke"
contains "delivery manifest includes next signoff command smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "next signoff command smoke"
contains "delivery manifest includes manual signoff progression smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "manual signoff progression smoke"
contains "delivery manifest includes all manual signoff command smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "manual signoff commands smoke"
contains "delivery manifest includes delivery status command" "scripts/prepare-universal-forms-delivery-manifest.sh" "delivery status command"
contains "delivery manifest includes delivery status JSON verifier" "scripts/prepare-universal-forms-delivery-manifest.sh" "delivery status JSON verifier"
contains "delivery manifest includes delivery status JSON contract smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "delivery status JSON contract smoke"
contains "delivery manifest includes delivery status output smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "delivery status output smoke"
contains "delivery manifest includes delivery status JSON schema" "scripts/prepare-universal-forms-delivery-manifest.sh" "delivery status JSON schema"
contains "delivery manifest includes release gate JSON schema" "scripts/prepare-universal-forms-delivery-manifest.sh" "release gate JSON schema"
contains "delivery manifest includes release gate JSON verifier" "scripts/prepare-universal-forms-delivery-manifest.sh" "release gate JSON verifier"
contains "delivery manifest includes release gate JSON contract smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "release gate JSON contract smoke"
contains "delivery manifest includes release gate output smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "release gate output smoke"
contains "delivery manifest includes MCP app AGENTS guide" "scripts/prepare-universal-forms-delivery-manifest.sh" "MCP app AGENTS guide"
contains "delivery manifest includes contributing guide" "scripts/prepare-universal-forms-delivery-manifest.sh" "contributing guide"
contains "delivery manifest includes frontend README" "scripts/prepare-universal-forms-delivery-manifest.sh" "frontend README"
contains "delivery manifest includes frontend quickstart" "scripts/prepare-universal-forms-delivery-manifest.sh" "frontend quickstart"
contains "delivery manifest includes compose stack" "scripts/prepare-universal-forms-delivery-manifest.sh" "compose stack"
contains "delivery manifest includes source Dockerfile" "scripts/prepare-universal-forms-delivery-manifest.sh" "source Dockerfile"
contains "delivery manifest includes prebuilt runtime Dockerfile" "scripts/prepare-universal-forms-delivery-manifest.sh" "prebuilt runtime Dockerfile"
contains "delivery manifest includes frontend Dockerfile" "scripts/prepare-universal-forms-delivery-manifest.sh" "frontend Dockerfile"
contains "delivery manifest includes frontend nginx config" "scripts/prepare-universal-forms-delivery-manifest.sh" "frontend nginx config"
contains "delivery manifest includes env example" "scripts/prepare-universal-forms-delivery-manifest.sh" "env example"
contains "delivery manifest includes webhook example config" "scripts/prepare-universal-forms-delivery-manifest.sh" "webhook example config"
contains "delivery manifest includes start script" "scripts/prepare-universal-forms-delivery-manifest.sh" "start script"
contains "delivery manifest includes verify script" "scripts/prepare-universal-forms-delivery-manifest.sh" "verify script"
contains "delivery manifest includes e2e test script" "scripts/prepare-universal-forms-delivery-manifest.sh" "e2e test script"
contains "delivery manifest includes API test script" "scripts/prepare-universal-forms-delivery-manifest.sh" "API test script"
contains "delivery manifest includes MCP test script" "scripts/prepare-universal-forms-delivery-manifest.sh" "MCP test script"
contains "delivery manifest includes benchmark script" "scripts/prepare-universal-forms-delivery-manifest.sh" "benchmark script"
contains "delivery manifest includes dev database script" "scripts/prepare-universal-forms-delivery-manifest.sh" "dev database script"
contains "delivery manifest includes dev check script" "scripts/prepare-universal-forms-delivery-manifest.sh" "dev check script"
contains "delivery manifest includes database init script" "scripts/prepare-universal-forms-delivery-manifest.sh" "database init script"
contains "delivery manifest includes backup script" "scripts/prepare-universal-forms-delivery-manifest.sh" "backup script"
contains "delivery manifest includes restore script" "scripts/prepare-universal-forms-delivery-manifest.sh" "restore script"
contains "delivery manifest includes stop script" "scripts/prepare-universal-forms-delivery-manifest.sh" "stop script"
contains "delivery manifest includes clean script" "scripts/prepare-universal-forms-delivery-manifest.sh" "clean script"
contains "MCP test script requires exact expected tool count" "scripts/test-mcp.sh" 'expected exactly $EXPECTED_TOOL_COUNT'
contains "MCP test script defaults to current tool count" "scripts/test-mcp.sh" 'EXPECTED_TOOL_COUNT="${EXPECTED_TOOL_COUNT:-106}"'
contains "MCP test script requires scenario template install" "scripts/test-mcp.sh" "scenario_templates.install"
contains "MCP test script requires forms.duplicate" "scripts/test-mcp.sh" "forms.duplicate"
contains "MCP test script requires form metadata tools" "scripts/test-mcp.sh" "forms.schema_summary"
contains "MCP test script requires form permission tools" "scripts/test-mcp.sh" "form_permissions.get"
contains "MCP test script requires form attachment tools" "scripts/test-mcp.sh" "form_attachments.create"
contains "MCP test script requires relation tools" "scripts/test-mcp.sh" "form_records.relation_targets"
contains "MCP test script requires child lifecycle tools" "scripts/test-mcp.sh" "form_records.child_archive"
contains "MCP test script checks plugin invocations" "scripts/test-mcp.sh" "plugin_invocations.list"
contains "dev-up exposes PostgreSQL on localhost only" "scripts/dev-up.sh" "127.0.0.1:\${POSTGRES_PORT}:5432"
contains "dev-up prints host database url for the config file" "scripts/dev-up.sh" "url = \\\"postgres://openpr:\${POSTGRES_PASSWORD}@127.0.0.1:\${POSTGRES_PORT}/openpr\\\""
contains "init-db defaults to localhost development database" "scripts/init-db.sh" 'PGHOST="${PGHOST:-127.0.0.1}"'
contains "init-db defaults to development password" "scripts/init-db.sh" 'PGPASSWORD="${PGPASSWORD:-openpr_dev_password}"'
contains "docs audit requires README delivery acceptance state" "scripts/audit-universal-forms-docs.sh" "README documents delivery acceptance state"
contains "production readiness audit requires README delivery acceptance state" "scripts/audit-universal-forms-production-readiness.sh" "README exposes delivery acceptance state"
contains "docs audit requires README signoff status JSON schema" "scripts/audit-universal-forms-docs.sh" "README links signoff status JSON schema"
contains "production readiness audit requires README next signoff command smoke" "scripts/audit-universal-forms-production-readiness.sh" "README exposes next signoff command smoke"
contains "production readiness audit requires development status JSON generation" "scripts/audit-universal-forms-production-readiness.sh" "runbook includes development status JSON generation"
contains "production readiness audit requires development status JSON verifier" "scripts/audit-universal-forms-production-readiness.sh" "runbook includes development status JSON verifier"
contains "production readiness audit requires development status JSON contract smoke" "scripts/audit-universal-forms-production-readiness.sh" "runbook includes development status JSON contract smoke"
contains "production readiness audit requires scenario catalog JSON generation" "scripts/audit-universal-forms-production-readiness.sh" "runbook includes scenario catalog JSON generation"
contains "production readiness audit requires scenario catalog JSON verifier" "scripts/audit-universal-forms-production-readiness.sh" "runbook includes scenario catalog JSON verifier"
contains "production readiness audit requires scenario catalog JSON contract smoke" "scripts/audit-universal-forms-production-readiness.sh" "runbook includes scenario catalog JSON contract smoke"
contains "production readiness audit requires implementation map JSON generation" "scripts/audit-universal-forms-production-readiness.sh" "runbook includes implementation map JSON generation"
contains "production readiness audit requires implementation map JSON verifier" "scripts/audit-universal-forms-production-readiness.sh" "runbook includes implementation map JSON verifier"
contains "production readiness audit requires implementation map JSON contract smoke" "scripts/audit-universal-forms-production-readiness.sh" "runbook includes implementation map JSON contract smoke"
contains "UI review gallery generator links project template screenshot" "scripts/prepare-universal-forms-ui-review-gallery.sh" "project-template-wizard-desktop.png"
contains "UI review gallery generator links manual rows" "scripts/prepare-universal-forms-ui-review-gallery.sh" "Restaurant template can create a project directly"
contains "UI review gallery verifier checks screenshot references" "scripts/verify-universal-forms-ui-review-gallery.sh" "gallery references"
contains "UI review gallery browser smoke verifies loaded images" "scripts/smoke-universal-forms-ui-review-gallery-render.sh" "data-gallery-loaded-images=\"8\""
contains "UI review gallery browser smoke captures mobile render" "scripts/smoke-universal-forms-ui-review-gallery-render.sh" "ui-review-gallery-mobile.png"
contains "UI review gallery render smoke suppresses Chromium screenshot chatter" "scripts/smoke-universal-forms-ui-review-gallery-render.sh" "stderr_path"
contains "UI review gallery render smoke replays Chromium failures" "scripts/smoke-universal-forms-ui-review-gallery-render.sh" "Chromium render failed"
contains "report output boundary smoke checks stdout cleanliness" "scripts/smoke-universal-forms-report-output-boundaries.sh" "leaves stdout empty when --output is used"
contains "report output boundary smoke checks markdown headings" "scripts/smoke-universal-forms-report-output-boundaries.sh" "output starts with expected first line"
contains "report output boundary smoke checks JSON schema versions" "scripts/smoke-universal-forms-report-output-boundaries.sh" "JSON schema version matches"
contains "report output boundary smoke checks stdout JSON mode" "scripts/smoke-universal-forms-report-output-boundaries.sh" "stdout JSON schema version matches"
contains "report output boundary smoke checks stdout alias" "scripts/smoke-universal-forms-report-output-boundaries.sh" "manual signoff status JSON --stdout"
contains "report output boundary smoke rejects repository dash output file" "scripts/smoke-universal-forms-report-output-boundaries.sh" "does not create repository dash output file"
contains "report output boundary smoke checks completion audit JSON" "scripts/smoke-universal-forms-report-output-boundaries.sh" "openpr.universal_forms.completion_audit.v1"
contains "report output boundary smoke checks implementation map JSON" "scripts/smoke-universal-forms-report-output-boundaries.sh" "openpr.universal_forms.implementation_map.v1"
contains "report output boundary smoke checks next signoff review" "scripts/smoke-universal-forms-report-output-boundaries.sh" "Next Signoff Review"
contains "completion audit reports UI review gallery gate" "scripts/report-universal-forms-completion-audit.sh" "UI review gallery verification"
contains "completion audit reports UI review gallery browser render gate" "scripts/report-universal-forms-completion-audit.sh" "UI review gallery browser render"
contains "delivery bundle audit verifies completion UI review gallery gate" "scripts/audit-universal-forms-delivery-bundle.sh" "completion audit UI review gallery gate passed"
contains "delivery bundle audit requires next signoff review file" "scripts/audit-universal-forms-delivery-bundle.sh" "NEXT_SIGNOFF_REVIEW_PATH"
contains "delivery bundle audit verifies next signoff review current key" "scripts/audit-universal-forms-delivery-bundle.sh" "next signoff review lists current key"
contains "delivery bundle audit verifies next signoff review evidence paths" "scripts/audit-universal-forms-delivery-bundle.sh" "next signoff review links project template desktop screenshot"
contains "delivery bundle audit runs next signoff review verifier" "scripts/audit-universal-forms-delivery-bundle.sh" "next signoff review verifier passes"
contains "delivery manifest includes restaurant UI screenshots" "scripts/prepare-universal-forms-delivery-manifest.sh" "restaurant-ordering-mobile.png"
contains "delivery manifest includes report output boundary smoke" "scripts/prepare-universal-forms-delivery-manifest.sh" "report output boundary smoke"
contains "delivery manifest verifier checks size" "scripts/verify-universal-forms-delivery-manifest.sh" "size mismatch"
contains "delivery manifest verifier checks sha256" "scripts/verify-universal-forms-delivery-manifest.sh" "sha256 mismatch"
contains "delivery bundle audit verifies delivery manifest JSON" "scripts/audit-universal-forms-delivery-bundle.sh" "delivery manifest JSON verifier passes"
contains "delivery bundle audit verifies delivery manifest JSON contract smoke" "scripts/audit-universal-forms-delivery-bundle.sh" "delivery manifest JSON contract smoke passes"
contains "delivery bundle audit verifies development status JSON" "scripts/audit-universal-forms-delivery-bundle.sh" "development status JSON verifier passes"
contains "delivery bundle audit verifies development status JSON contract smoke" "scripts/audit-universal-forms-delivery-bundle.sh" "development status JSON contract smoke passes"
contains "delivery bundle audit verifies completion audit JSON" "scripts/audit-universal-forms-delivery-bundle.sh" "completion audit JSON verifier passes"
contains "delivery bundle audit verifies completion audit JSON contract smoke" "scripts/audit-universal-forms-delivery-bundle.sh" "completion audit JSON contract smoke passes"
contains "delivery bundle audit verifies scenario catalog JSON" "scripts/audit-universal-forms-delivery-bundle.sh" "scenario catalog JSON verifier passes"
contains "delivery bundle audit verifies scenario catalog JSON contract smoke" "scripts/audit-universal-forms-delivery-bundle.sh" "scenario catalog JSON contract smoke passes"
contains "delivery bundle audit verifies implementation map JSON" "scripts/audit-universal-forms-delivery-bundle.sh" "implementation map JSON verifier passes"
contains "delivery bundle audit verifies implementation map JSON contract smoke" "scripts/audit-universal-forms-delivery-bundle.sh" "implementation map JSON contract smoke passes"
contains "delivery bundle audit verifies report output boundary smoke" "scripts/audit-universal-forms-delivery-bundle.sh" "report output boundary smoke passes"
contains "delivery bundle audit verifies next signoff command smoke" "scripts/audit-universal-forms-delivery-bundle.sh" "next signoff command smoke passes"
contains "delivery bundle audit verifies manual signoff progression smoke" "scripts/audit-universal-forms-delivery-bundle.sh" "manual signoff progression smoke passes"
contains "delivery bundle audit verifies signoff status output smoke" "scripts/audit-universal-forms-delivery-bundle.sh" "manual signoff status output smoke passes"
contains "delivery bundle audit verifies signoff dashboard" "scripts/audit-universal-forms-delivery-bundle.sh" "signoff dashboard verifier passes"
contains "delivery bundle audit verifies signoff dashboard render" "scripts/audit-universal-forms-delivery-bundle.sh" "signoff dashboard browser render smoke passes"
contains "delivery bundle audit verifies signoff dashboard progression smoke" "scripts/audit-universal-forms-delivery-bundle.sh" "signoff dashboard progression smoke passes"
contains "delivery bundle audit verifies delivery status command" "scripts/audit-universal-forms-delivery-bundle.sh" "delivery status command passes"
contains "delivery bundle audit verifies delivery status command JSON" "scripts/audit-universal-forms-delivery-bundle.sh" "delivery status command JSON passes"
contains "delivery bundle audit verifies delivery status JSON verifier" "scripts/audit-universal-forms-delivery-bundle.sh" "delivery status JSON verifier passes"
contains "delivery bundle audit verifies delivery status JSON contract smoke" "scripts/audit-universal-forms-delivery-bundle.sh" "delivery status JSON contract smoke passes"
contains "delivery bundle audit verifies delivery status output smoke" "scripts/audit-universal-forms-delivery-bundle.sh" "delivery status output smoke passes"
contains "delivery bundle audit rechecks final manifest" "scripts/audit-universal-forms-delivery-bundle.sh" "final delivery manifest verifier passes after audit drills"
contains "delivery bundle audit rechecks final manifest JSON" "scripts/audit-universal-forms-delivery-bundle.sh" "final delivery manifest JSON verifier passes after audit drills"
contains "manual signoff recorder updates runbook and evidence" "scripts/record-universal-forms-manual-signoff.sh" "verify-universal-forms-manual-signoff-consistency.sh"
contains "manual signoff recorder rejects markdown table breaks" "scripts/record-universal-forms-manual-signoff.sh" "must not contain markdown table pipes or newlines"
contains "manual signoff recorder writes runbook same-directory candidate" "scripts/record-universal-forms-manual-signoff.sh" 'mktemp "$(dirname "$RUNBOOK_PATH")/'
contains "manual signoff recorder writes evidence same-directory candidate" "scripts/record-universal-forms-manual-signoff.sh" 'mktemp "$(dirname "$EVIDENCE_PATH")/'
contains "manual signoff recorder preserves runbook permissions" "scripts/record-universal-forms-manual-signoff.sh" 'chmod --reference="$RUNBOOK_PATH" "$runbook_candidate"'
contains "manual signoff recorder preserves evidence permissions" "scripts/record-universal-forms-manual-signoff.sh" 'chmod --reference="$EVIDENCE_PATH" "$evidence_candidate"'
contains "manual signoff recorder atomically publishes runbook" "scripts/record-universal-forms-manual-signoff.sh" 'mv -f "$runbook_candidate" "$RUNBOOK_PATH"'
contains "manual signoff recorder atomically publishes evidence" "scripts/record-universal-forms-manual-signoff.sh" 'mv -f "$evidence_candidate" "$EVIDENCE_PATH"'
contains "manual signoff recorder refreshes completion audit" "scripts/record-universal-forms-manual-signoff.sh" "report-universal-forms-completion-audit.sh"
contains "manual signoff recorder refreshes completion audit JSON" "scripts/record-universal-forms-manual-signoff.sh" "report-universal-forms-completion-audit-json.sh"
contains "manual signoff recorder refreshes readiness JSON" "scripts/record-universal-forms-manual-signoff.sh" "report-universal-forms-readiness-json.sh"
contains "manual signoff recorder verifies readiness JSON" "scripts/record-universal-forms-manual-signoff.sh" "verify-universal-forms-readiness-json.sh"
contains "manual signoff recorder refreshes scenario catalog JSON" "scripts/record-universal-forms-manual-signoff.sh" "report-universal-forms-scenario-catalog-json.sh"
contains "manual signoff recorder verifies scenario catalog JSON" "scripts/record-universal-forms-manual-signoff.sh" "verify-universal-forms-scenario-catalog-json.sh"
contains "manual signoff recorder refreshes implementation map JSON" "scripts/record-universal-forms-manual-signoff.sh" "report-universal-forms-implementation-map-json.sh"
contains "manual signoff recorder verifies implementation map JSON" "scripts/record-universal-forms-manual-signoff.sh" "verify-universal-forms-implementation-map-json.sh"
contains "manual signoff recorder help documents full refresh set" "scripts/record-universal-forms-manual-signoff.sh" "readiness JSON, development status JSON, scenario catalog JSON"
contains "manual signoff recorder help documents implementation map JSON refresh" "scripts/record-universal-forms-manual-signoff.sh" "implementation map JSON"
contains "manual signoff recorder refreshes delivery manifest" "scripts/record-universal-forms-manual-signoff.sh" "verify-universal-forms-delivery-manifest.sh"
contains "manual signoff recorder refreshes delivery manifest JSON" "scripts/record-universal-forms-manual-signoff.sh" "verify-universal-forms-delivery-manifest-json.sh"
contains "manual signoff recorder refreshes delivery manifest JSON contract smoke" "scripts/record-universal-forms-manual-signoff.sh" "smoke-universal-forms-delivery-manifest-json-contract.sh"
contains "manual signoff recorder runs release gate smoke" "scripts/record-universal-forms-manual-signoff.sh" "smoke-universal-forms-release-gate.sh"
contains "manual signoff recorder runs release gate JSON verifier" "scripts/record-universal-forms-manual-signoff.sh" "verify-universal-forms-release-gate-json.sh"
contains "manual signoff recorder runs release gate JSON contract smoke" "scripts/record-universal-forms-manual-signoff.sh" "smoke-universal-forms-release-gate-json-contract.sh"
contains "manual signoff recorder runs release gate output smoke" "scripts/record-universal-forms-manual-signoff.sh" "smoke-universal-forms-release-gate-output.sh"
contains "manual signoff recorder runs delivery status command" "scripts/record-universal-forms-manual-signoff.sh" "status-universal-forms-delivery.sh"
contains "manual signoff recorder verifies delivery status JSON" "scripts/record-universal-forms-manual-signoff.sh" "verify-universal-forms-delivery-status-json.sh"
contains "manual signoff recorder runs delivery status output smoke" "scripts/record-universal-forms-manual-signoff.sh" "smoke-universal-forms-delivery-status-output.sh"
contains "manual signoff recorder runs signoff status output smoke" "scripts/record-universal-forms-manual-signoff.sh" "smoke-universal-forms-signoff-status-output.sh"
contains "manual signoff recorder runs signoff dashboard progression smoke" "scripts/record-universal-forms-manual-signoff.sh" "smoke-universal-forms-signoff-dashboard-progression.sh"
contains "manual signoff recorder runs report output boundary smoke" "scripts/record-universal-forms-manual-signoff.sh" "smoke-universal-forms-report-output-boundaries.sh"
contains "manual signoff recorder runs manual signoff progression smoke" "scripts/record-universal-forms-manual-signoff.sh" "smoke-universal-forms-manual-signoff-progression.sh"
contains "manual signoff recorder runs all manual signoff command smoke" "scripts/record-universal-forms-manual-signoff.sh" "smoke-universal-forms-manual-signoff-commands.sh"
contains "manual signoff recorder blocks early overall acceptance" "scripts/record-universal-forms-manual-signoff.sh" "Overall acceptance cannot be passed before prerequisite row is passed"
contains "manual signoff recorder blocks prerequisite downgrade after overall acceptance" "scripts/record-universal-forms-manual-signoff.sh" "Prerequisite signoff cannot be downgraded while overall acceptance is passed"
contains "delivery bundle audit drills recorder dry-run" "scripts/audit-universal-forms-delivery-bundle.sh" "manual signoff recorder accepted-row dry-run succeeds"
contains "delivery bundle audit drills early overall block" "scripts/audit-universal-forms-delivery-bundle.sh" "manual signoff recorder blocks early overall dry-run"
contains "delivery bundle audit drills recorder custom-path write" "scripts/audit-universal-forms-delivery-bundle.sh" "manual signoff recorder custom-path accepted-row write succeeds"
contains "delivery bundle audit protects official runbook during recorder drill" "scripts/audit-universal-forms-delivery-bundle.sh" "manual signoff recorder custom-path write leaves official runbook unchanged"
contains "delivery bundle audit protects official evidence during recorder drill" "scripts/audit-universal-forms-delivery-bundle.sh" "manual signoff recorder custom-path write leaves official evidence unchanged"
contains "signoff verifier supports explicit runbook argument" "scripts/verify-universal-forms-acceptance-signoff.sh" "--runbook PATH"
contains "finalizer supports explicit runbook argument" "scripts/finalize-universal-forms-acceptance.sh" "--runbook PATH"
contains "finalizer passes explicit runbook to signoff verifier" "scripts/finalize-universal-forms-acceptance.sh" 'verify-universal-forms-acceptance-signoff.sh'
contains "delivery bundle audit passes explicit runbook to finalizer" "scripts/audit-universal-forms-delivery-bundle.sh" '--runbook "$drill_dir/runbook.md"'
contains "delivery bundle audit rejects signed evidence without runbook" "scripts/audit-universal-forms-delivery-bundle.sh" "signed evidence without runbook is rejected by signoff verifier"
contains "user acceptance packet generator emits signoff key map" "scripts/prepare-universal-forms-user-acceptance-packet.sh" "## Manual Signoff Key Map"
contains "user acceptance packet generator documents full machine-readable refresh" "scripts/prepare-universal-forms-user-acceptance-packet.sh" "readiness JSON, development status JSON, scenario catalog JSON"
contains "user acceptance packet generator documents implementation map JSON refresh" "scripts/prepare-universal-forms-user-acceptance-packet.sh" "implementation map JSON"
contains "user acceptance packet generator includes completion audit JSON verifier" "scripts/prepare-universal-forms-user-acceptance-packet.sh" "verify-universal-forms-completion-audit-json.sh"
contains "user acceptance packet generator includes readiness JSON verifier" "scripts/prepare-universal-forms-user-acceptance-packet.sh" "verify-universal-forms-readiness-json.sh"
contains "user acceptance packet generator includes delivery manifest JSON verifier" "scripts/prepare-universal-forms-user-acceptance-packet.sh" "verify-universal-forms-delivery-manifest-json.sh"
contains "user acceptance packet generator includes release gate output smoke" "scripts/prepare-universal-forms-user-acceptance-packet.sh" "smoke-universal-forms-release-gate-output.sh"
contains "user acceptance packet generator includes manual signoff progression smoke" "scripts/prepare-universal-forms-user-acceptance-packet.sh" "smoke-universal-forms-manual-signoff-progression.sh"
contains "manual signoff progression smoke suppresses successful subcommand chatter" "scripts/smoke-universal-forms-manual-signoff-progression.sh" "run_quiet"
contains "manual signoff progression smoke replays failed subcommand output" "scripts/smoke-universal-forms-manual-signoff-progression.sh" "stderr:"
contains "manual signoff status reporter is read-only" "scripts/report-universal-forms-signoff-status.sh" "read-only reviewer aid"
contains "manual signoff status reporter shows next actionable row" "scripts/report-universal-forms-signoff-status.sh" "Next row:"
contains "manual signoff status reporter emits recorder command" "scripts/report-universal-forms-signoff-status.sh" "record-universal-forms-manual-signoff.sh --item"
contains "manual signoff status reporter accepts positional report path" "scripts/report-universal-forms-signoff-status.sh" "Optional positional REPORT_PATH"
contains "manual signoff status reporter writes output report" "scripts/report-universal-forms-signoff-status.sh" "--output PATH"
contains "manual signoff status JSON reporter emits schema version" "scripts/report-universal-forms-signoff-status-json.sh" "openpr.universal_forms.signoff_status.v1"
contains "manual signoff status JSON reporter is read-only" "scripts/report-universal-forms-signoff-status-json.sh" "read-only"
contains "manual signoff status JSON reporter emits final signoff flag" "scripts/report-universal-forms-signoff-status-json.sh" "final_signoff_allowed"
contains "manual signoff status JSON reporter emits row recorder command" "scripts/report-universal-forms-signoff-status-json.sh" "recorder_command"
contains "manual signoff status JSON reporter emits automated evidence" "scripts/report-universal-forms-signoff-status-json.sh" "automated_evidence"
contains "manual signoff status JSON reporter emits reviewer check" "scripts/report-universal-forms-signoff-status-json.sh" "reviewer_check"
contains "manual signoff status JSON reporter emits pending queue" "scripts/report-universal-forms-signoff-status-json.sh" "pending_queue"
contains "manual signoff status JSON reporter supports stdout mode" "scripts/report-universal-forms-signoff-status-json.sh" '[[ "$OUTPUT_PATH" == "-" ]]'
contains "manual signoff status JSON reporter supports stdout alias" "scripts/report-universal-forms-signoff-status-json.sh" "--stdout"
contains "manual signoff status output smoke suppresses successful reporter chatter" "scripts/smoke-universal-forms-signoff-status-output.sh" "run_quiet"
contains "manual signoff status output smoke replays failed reporter output" "scripts/smoke-universal-forms-signoff-status-output.sh" "stderr:"
contains "manual signoff status JSON verifier checks schema version" "scripts/verify-universal-forms-signoff-status-json.sh" "schema version is v1"
contains "manual signoff status JSON verifier checks manual row enum" "scripts/verify-universal-forms-signoff-status-json.sh" "JSON manual row keys exactly match schema enum"
contains "manual signoff status JSON verifier checks manual row order" "scripts/verify-universal-forms-signoff-status-json.sh" "JSON manual row keys match schema order"
contains "manual signoff status JSON verifier checks next row enum" "scripts/verify-universal-forms-signoff-status-json.sh" "JSON next row key is allowed by schema"
contains "manual signoff status JSON verifier rejects nested extra fields" "scripts/verify-universal-forms-signoff-status-json.sh" "JSON manual rows have no extra keys beyond schema"
contains "manual signoff status JSON verifier checks pending rows" "scripts/verify-universal-forms-signoff-status-json.sh" "JSON pending rows match evidence"
contains "manual signoff status JSON verifier checks row automated evidence" "scripts/verify-universal-forms-signoff-status-json.sh" "JSON row automated evidence mirrors evidence map"
contains "manual signoff status JSON verifier checks row reviewer check" "scripts/verify-universal-forms-signoff-status-json.sh" "JSON row reviewer check mirrors evidence map"
contains "manual signoff status JSON verifier checks next automated evidence" "scripts/verify-universal-forms-signoff-status-json.sh" "JSON next row automated evidence matches evidence map"
contains "manual signoff status JSON verifier checks next reviewer check" "scripts/verify-universal-forms-signoff-status-json.sh" "JSON next row reviewer check matches evidence map"
contains "manual signoff status JSON verifier checks pending queue length" "scripts/verify-universal-forms-signoff-status-json.sh" "JSON pending queue length matches evidence"
contains "manual signoff status JSON verifier checks pending queue order" "scripts/verify-universal-forms-signoff-status-json.sh" "JSON pending queue review order mirrors row positions"
contains "manual signoff status JSON verifier checks pending queue next marker" "scripts/verify-universal-forms-signoff-status-json.sh" "JSON pending queue next marker matches next row"
contains "manual signoff status JSON verifier checks row recorder command" "scripts/verify-universal-forms-signoff-status-json.sh" "JSON row recorder command mirrors evidence map"
contains "manual signoff status JSON contract smoke checks final flag drift" "scripts/smoke-universal-forms-signoff-status-json-contract.sh" "final signoff flag drift"
contains "manual signoff status JSON contract smoke checks next row key drift" "scripts/smoke-universal-forms-signoff-status-json-contract.sh" "unknown next manual row key"
contains "manual signoff status JSON contract smoke checks nested extra fields" "scripts/smoke-universal-forms-signoff-status-json-contract.sh" "extra manual row property"
contains "manual signoff status JSON contract smoke checks row order drift" "scripts/smoke-universal-forms-signoff-status-json-contract.sh" "manual row order drift"
contains "manual signoff status JSON contract smoke checks automated evidence drift" "scripts/smoke-universal-forms-signoff-status-json-contract.sh" "per-row automated evidence drift"
contains "manual signoff status JSON contract smoke checks reviewer check drift" "scripts/smoke-universal-forms-signoff-status-json-contract.sh" "per-row reviewer check drift"
contains "manual signoff status JSON contract smoke checks pending queue drift" "scripts/smoke-universal-forms-signoff-status-json-contract.sh" "pending queue order drift"
contains "manual signoff status JSON contract smoke checks pending queue actionable drift" "scripts/smoke-universal-forms-signoff-status-json-contract.sh" "pending queue actionable drift"
contains "manual signoff status JSON contract smoke checks recorder command drift" "scripts/smoke-universal-forms-signoff-status-json-contract.sh" "per-row recorder command drift"
contains "manual signoff status output smoke checks gate summary" "scripts/smoke-universal-forms-signoff-status-output.sh" "mirrors automated checks"
contains "manual signoff status output smoke checks rows" "scripts/smoke-universal-forms-signoff-status-output.sh" "row count mirrors JSON"
contains "manual signoff status output smoke checks recorder command" "scripts/smoke-universal-forms-signoff-status-output.sh" "mirrors recorder command"
contains "manual signoff status output smoke rejects null leakage" "scripts/smoke-universal-forms-signoff-status-output.sh" "does not leak JSON nulls"
contains "next signoff review verifier checks current key" "scripts/verify-universal-forms-next-signoff-review.sh" "review mirrors next signoff key"
contains "next signoff review verifier checks screenshot paths" "scripts/verify-universal-forms-next-signoff-review.sh" "review links artifact"
contains "next signoff review verifier checks recorder command" "scripts/verify-universal-forms-next-signoff-review.sh" "review mirrors recorder command"
contains "next signoff review contract smoke checks heading drift" "scripts/smoke-universal-forms-next-signoff-review-contract.sh" "heading drift"
contains "next signoff review contract smoke checks next key drift" "scripts/smoke-universal-forms-next-signoff-review-contract.sh" "next key drift"
contains "next signoff review contract smoke checks recorder command drift" "scripts/smoke-universal-forms-next-signoff-review-contract.sh" "recorder command drift"
contains "next signoff review contract smoke checks screenshot path drift" "scripts/smoke-universal-forms-next-signoff-review-contract.sh" "screenshot path drift"
contains "next signoff command smoke reads next row key" "scripts/smoke-universal-forms-next-signoff-command.sh" ".manual_signoff.next_row.key"
contains "next signoff command smoke dry-runs recorder" "scripts/smoke-universal-forms-next-signoff-command.sh" "record-universal-forms-manual-signoff.sh"
contains "next signoff command smoke protects official evidence" "scripts/smoke-universal-forms-next-signoff-command.sh" "official evidence path remains unchanged"
contains "manual signoff progression smoke checks initial next row" "scripts/smoke-universal-forms-manual-signoff-progression.sh" 'verify_state "$tmp_json" 0'
contains "manual signoff progression smoke checks per-row advancement" "scripts/smoke-universal-forms-manual-signoff-progression.sh" "manual signoff recorder advances temporary row"
contains "manual signoff progression smoke checks final flag" "scripts/smoke-universal-forms-manual-signoff-progression.sh" "final signoff flag"
contains "manual signoff progression smoke protects official files" "scripts/smoke-universal-forms-manual-signoff-progression.sh" "official evidence remains unchanged"
contains "manual signoff commands smoke verifies every row" "scripts/smoke-universal-forms-manual-signoff-commands.sh" ".manual_signoff.rows[]"
contains "manual signoff commands smoke verifies overall prerequisite order" "scripts/smoke-universal-forms-manual-signoff-commands.sh" "overall prerequisite rule"
contains "manual signoff commands smoke runs final signoff verifier" "scripts/smoke-universal-forms-manual-signoff-commands.sh" "verify-universal-forms-acceptance-signoff.sh"
contains "manual signoff recorder refreshes signoff status report" "scripts/record-universal-forms-manual-signoff.sh" "openpr-universal-form-signoff-status-2026-05-31.md"
contains "manual signoff recorder refreshes signoff status JSON" "scripts/record-universal-forms-manual-signoff.sh" "report-universal-forms-signoff-status-json.sh"
contains "manual signoff recorder verifies signoff status output" "scripts/record-universal-forms-manual-signoff.sh" "smoke-universal-forms-signoff-status-output.sh"
contains "manual signoff recorder refreshes signoff dashboard" "scripts/record-universal-forms-manual-signoff.sh" "prepare-universal-forms-signoff-dashboard.sh"
contains "manual signoff recorder verifies signoff dashboard" "scripts/record-universal-forms-manual-signoff.sh" "verify-universal-forms-signoff-dashboard.sh"
contains "manual signoff recorder renders signoff dashboard" "scripts/record-universal-forms-manual-signoff.sh" "smoke-universal-forms-signoff-dashboard-render.sh"
contains "manual signoff recorder validates signoff dashboard progression" "scripts/record-universal-forms-manual-signoff.sh" "smoke-universal-forms-signoff-dashboard-progression.sh"
contains "manual signoff recorder refreshes next signoff review" "scripts/record-universal-forms-manual-signoff.sh" "prepare-universal-forms-next-signoff-review.sh"
contains "manual signoff recorder verifies next signoff review" "scripts/record-universal-forms-manual-signoff.sh" "verify-universal-forms-next-signoff-review.sh"
contains "manual signoff recorder runs next signoff review contract smoke" "scripts/record-universal-forms-manual-signoff.sh" "smoke-universal-forms-next-signoff-review-contract.sh"
contains "manual signoff recorder runs next signoff command smoke" "scripts/record-universal-forms-manual-signoff.sh" "smoke-universal-forms-next-signoff-command.sh"
contains "finalizer refreshes signoff status report" "scripts/finalize-universal-forms-acceptance.sh" "openpr-universal-form-signoff-status-2026-05-31.md"
contains "finalizer refreshes signoff status JSON" "scripts/finalize-universal-forms-acceptance.sh" "report-universal-forms-signoff-status-json.sh"
contains "finalizer verifies signoff status output" "scripts/finalize-universal-forms-acceptance.sh" "smoke-universal-forms-signoff-status-output.sh"
contains "finalizer refreshes signoff dashboard" "scripts/finalize-universal-forms-acceptance.sh" "prepare-universal-forms-signoff-dashboard.sh"
contains "finalizer verifies signoff dashboard" "scripts/finalize-universal-forms-acceptance.sh" "verify-universal-forms-signoff-dashboard.sh"
contains "finalizer renders signoff dashboard" "scripts/finalize-universal-forms-acceptance.sh" "smoke-universal-forms-signoff-dashboard-render.sh"
contains "finalizer validates signoff dashboard progression" "scripts/finalize-universal-forms-acceptance.sh" "smoke-universal-forms-signoff-dashboard-progression.sh"
contains "finalizer refreshes next signoff review" "scripts/finalize-universal-forms-acceptance.sh" "prepare-universal-forms-next-signoff-review.sh"
contains "finalizer verifies next signoff review" "scripts/finalize-universal-forms-acceptance.sh" "verify-universal-forms-next-signoff-review.sh"
contains "finalizer runs next signoff review contract smoke" "scripts/finalize-universal-forms-acceptance.sh" "smoke-universal-forms-next-signoff-review-contract.sh"
contains "finalizer refreshes completion audit JSON" "scripts/finalize-universal-forms-acceptance.sh" "report-universal-forms-completion-audit-json.sh"
contains "finalizer refreshes implementation map JSON" "scripts/finalize-universal-forms-acceptance.sh" "report-universal-forms-implementation-map-json.sh"
contains "finalizer runs next signoff command smoke" "scripts/finalize-universal-forms-acceptance.sh" "smoke-universal-forms-next-signoff-command.sh"
contains "finalizer runs manual signoff progression smoke" "scripts/finalize-universal-forms-acceptance.sh" "smoke-universal-forms-manual-signoff-progression.sh"
contains "finalizer runs all manual signoff command smoke" "scripts/finalize-universal-forms-acceptance.sh" "smoke-universal-forms-manual-signoff-commands.sh"
contains "finalizer runs report output boundary smoke" "scripts/finalize-universal-forms-acceptance.sh" "smoke-universal-forms-report-output-boundaries.sh"
contains "finalizer runs release gate smoke" "scripts/finalize-universal-forms-acceptance.sh" "smoke-universal-forms-release-gate.sh"
contains "finalizer runs release gate JSON verifier" "scripts/finalize-universal-forms-acceptance.sh" "verify-universal-forms-release-gate-json.sh"
contains "finalizer runs release gate JSON contract smoke" "scripts/finalize-universal-forms-acceptance.sh" "smoke-universal-forms-release-gate-json-contract.sh"
contains "finalizer runs release gate output smoke" "scripts/finalize-universal-forms-acceptance.sh" "smoke-universal-forms-release-gate-output.sh"
contains "finalizer runs delivery status command" "scripts/finalize-universal-forms-acceptance.sh" "status-universal-forms-delivery.sh"
contains "finalizer verifies delivery status JSON" "scripts/finalize-universal-forms-acceptance.sh" "verify-universal-forms-delivery-status-json.sh"
contains "finalizer runs delivery status output smoke" "scripts/finalize-universal-forms-acceptance.sh" "smoke-universal-forms-delivery-status-output.sh"
contains "final signoff verifier requires runbook consistency input" "scripts/verify-universal-forms-acceptance-signoff.sh" "Runbook not found for final signoff consistency check"
contains "finalizer forwards runbook to final signoff verifier" "scripts/finalize-universal-forms-acceptance.sh" '--runbook "$RUNBOOK_PATH"'
contains "finalizer writes same-directory tracker candidate" "scripts/finalize-universal-forms-acceptance.sh" 'mktemp "$(dirname "$TRACKER_PATH")/'
contains "finalizer preserves tracker permissions" "scripts/finalize-universal-forms-acceptance.sh" 'chmod --reference="$TRACKER_PATH" "$tmp_file"'
contains "finalizer atomically publishes tracker" "scripts/finalize-universal-forms-acceptance.sh" 'mv -f "$tmp_file" "$TRACKER_PATH"'
contains "finalizer refreshes derived handoff after default finalization" "scripts/finalize-universal-forms-acceptance.sh" "derived acceptance handoff refreshed after finalization"
contains "finalizer verifies official bundle after default finalization" "scripts/finalize-universal-forms-acceptance.sh" "post-finalization delivery bundle verified"
contains "finalizer is idempotent for finalized trackers" "scripts/finalize-universal-forms-acceptance.sh" "universal forms acceptance already finalized in tracker"
contains "docs audit allows partial manual signoff pending counts" "scripts/audit-universal-forms-docs.sh" "user acceptance packet pending rows match evidence"

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms source coverage audit failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms source coverage audit passed.\n'
