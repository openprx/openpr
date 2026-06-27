# OpenPR

Open-source project management platform with built-in governance, AI agent integration, and MCP support.

Built with **Rust** (Axum + SeaORM), **SvelteKit**, and **PostgreSQL**.

## What It Provides

- Full project management: issues, kanban board, sprints, labels, comments, file attachments.
- Governance center: proposals, voting, trust scores, decision records, veto & escalation.
- AI integration: bot tokens, AI agents, AI tasks, AI review, webhook callbacks.
- MCP server: 105 tools across 3 transport protocols (HTTP, stdio, SSE).
- Universal forms: project-defined business data types with grid/detail views, decimal-safe amount fields, record links, events, MCP access, and webhook/connector delivery.
- WASM plugins: sandboxed project plugins for validation, formulas, event handlers, and plugin-provided MCP tools.
- Scenario templates: ready-to-start project setups for code delivery, contract review, equipment maintenance, quality corrective action, customer delivery, and restaurant ordering.
- Skill distribution: `AGENTS.md` for coding agents, skill package for governed workflows.

## Universal Business Platform

OpenPR can be used as a generic open-source business application base, not only as an engineering issue tracker.

Project scenario templates can initialize:

- Custom forms and default grid/detail views.
- Connector suggestions for webhooks, REST, MCP, print, device, CLI, and tunnel integrations.
- Active WASM plugins when the scenario requires local calculation or event handling.

The restaurant ordering scenario is the reference end-to-end example. Creating a project with `scenario_template_key=restaurant_ordering_default` initializes menu categories, SKU, tables, orders, order lines, print jobs, business reports, print/webhook connector suggestions, and an active `restaurant_calc` WASM plugin.

```bash
curl -X POST http://localhost:8081/api/v1/workspaces/<workspace-id>/projects \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "key": "REST",
    "name": "Restaurant Ordering",
    "scenario_template_key": "restaurant_ordering_default"
  }'
```

Forms are available in the frontend at:

```text
/workspace/<workspace-id>/projects/<project-id>/forms
```

Key smoke tests:

```bash
scripts/smoke-scenario-template-forms.sh
scripts/smoke-restaurant-ordering.sh
cd frontend && bun run smoke:forms-ui && bun run smoke:restaurant-ordering
```

See `docs/universal-forms-and-plugins.md` for the data model, plugin behavior,
connector flow, and delivery checklist. See
`docs/universal-forms-implementation-map.md` for the source-module,
verification-command, and status-marker map. See
`docs/universal-forms-acceptance.md` for the acceptance process and
`docs/universal-forms-production.md` for the production runbook.

## Delivery Acceptance State

The current universal business-platform delivery is ready for user-side manual
signoff, not final acceptance. Automated gates are expected to show:

- `Total automated checks: 27`
- `Failed automated checks: 0`
- `Tracker non-manual unresolved items: 0`
- `Manual signoff rows pending: 7`

Security preflight is PostgreSQL-only for this delivery. Run
`scripts/audit-universal-forms-security-scope.sh` with `cargo audit` to confirm
the documented SQLx inactive-MySQL advisory ignore is not an active runtime
dependency path and that the workspace resolves through `sqlx-postgres`.
The `Universal Forms Gates` GitHub Actions job runs that security preflight,
the source coverage audit, and the production readiness audit on pull requests
so the open-source delivery path cannot silently drop the universal business
platform gates. CI clears repository `RUSTFLAGS` only while installing
third-party cargo tools such as `cargo-machete` and `cargo-audit`, keeping tool
installation independent from the workspace warning-as-error policy. Run
`scripts/ci-universal-forms-gates.sh` locally to execute the same Universal
Forms Gates bundle used by CI.

Use these entry points to review or refresh the handoff bundle:

```bash
scripts/report-universal-forms-signoff-status.sh --reviewer "<name>"
scripts/ci-universal-forms-gates.sh
scripts/report-universal-forms-signoff-status-json.sh
scripts/verify-universal-forms-signoff-status-json.sh
scripts/smoke-universal-forms-signoff-status-json-contract.sh
scripts/smoke-universal-forms-signoff-status-output.sh
scripts/verify-universal-forms-next-signoff-review.sh
scripts/smoke-universal-forms-next-signoff-review-contract.sh
scripts/smoke-universal-forms-next-signoff-command.sh
scripts/smoke-universal-forms-manual-signoff-progression.sh
scripts/smoke-universal-forms-manual-signoff-commands.sh
scripts/status-universal-forms-delivery.sh
scripts/verify-universal-forms-delivery-status-json.sh
scripts/smoke-universal-forms-delivery-status-json-contract.sh
scripts/smoke-universal-forms-delivery-status-output.sh
scripts/report-universal-forms-completion-audit-json.sh
scripts/verify-universal-forms-completion-audit-json.sh
scripts/smoke-universal-forms-completion-audit-json-contract.sh
scripts/report-universal-forms-readiness-summary.sh
scripts/report-universal-forms-readiness-json.sh
scripts/verify-universal-forms-readiness-json.sh
scripts/report-universal-forms-development-status-json.sh
scripts/verify-universal-forms-development-status-json.sh
scripts/smoke-universal-forms-development-status-json-contract.sh
scripts/report-universal-forms-scenario-catalog-json.sh
scripts/verify-universal-forms-scenario-catalog-json.sh
scripts/smoke-universal-forms-scenario-catalog-json-contract.sh
scripts/report-universal-forms-implementation-map-json.sh
scripts/verify-universal-forms-implementation-map-json.sh
scripts/smoke-universal-forms-implementation-map-json-contract.sh
scripts/report-universal-forms-delivery-manifest-json.sh
scripts/verify-universal-forms-delivery-manifest-json.sh
scripts/smoke-universal-forms-delivery-manifest-json-contract.sh
scripts/verify-universal-forms-delivery-manifest.sh \
  /opt/worker/report/openpr/docs/openpr-universal-form-delivery-manifest-2026-05-31.md
scripts/audit-universal-forms-delivery-bundle.sh
scripts/gate-universal-forms-release.sh --allow-pending
scripts/verify-universal-forms-release-gate-json.sh
scripts/smoke-universal-forms-release-gate-json-contract.sh
scripts/smoke-universal-forms-release-gate.sh
scripts/smoke-universal-forms-release-gate-output.sh
```

The readiness JSON includes the next manual signoff key, suggested evidence
note, and recorder command template so CI, release automation, MCP tools, and
webhook consumers can use the handoff state without parsing Markdown.
The next signoff review verifier and contract smoke keep the one-page reviewer
aid aligned with the current signoff JSON, screenshot paths, verification
commands, and recorder command before a reviewer uses it.
Its contract is documented in
`docs/schemas/openpr-universal-forms-readiness.schema.json`.
The readiness JSON, signoff status JSON, release gate JSON, and delivery status JSON all constrain next manual keys to the same seven manual signoff keys, or an empty string after final signoff.
The signoff status JSON is the narrower reviewer-progress contract. It reports
the seven manual rows, each row's recorder command, the final-signoff flag, and
the next recorder command. Each row and the next-row object also include the
automated evidence and reviewer check text from the manual evidence map, so
MCP, CLI, webhook, CI, and release consumers can present reviewer instructions
without scraping Markdown. It also exposes `pending_queue`, an ordered
machine-readable review queue with `review_order`, `is_next`, `actionable`,
evidence text, reviewer check text, and the recorder command for every
remaining manual row. Its contract is documented in
`docs/schemas/openpr-universal-forms-signoff-status.schema.json`.
The readiness and signoff status JSON schemas also pin the seven manual rows in
review order from `restaurant_template` through `overall`, so automation cannot
move final acceptance ahead of prerequisite reviewer rows.
The compact delivery status JSON adds a `completion_summary` object for the
management view: engineering checks completed/total, manual accepted/total/
remaining rows, total handoff items completed/total/remaining/percent, whether
release is blocked only by manual signoff, and the derived delivery state. It
also includes `completion_breakdown`, splitting the same progress into
engineering checks, user-side manual signoff, and overall handoff rows with
completed/total/remaining/percent plus the current status marker. It
also includes `release_blockers`, a fixed three-row list for automated checks,
non-manual tracker rows, and user-side manual signoff. Each row carries
clear/blocking status, blocker count, required action, and evidence path, so
automation does not have to parse the release `reason` string.
It also mirrors the signoff `pending_queue` as
`manual_signoff_queue`, so MCP, CLI, webhook, CI, release automation, and web
dashboards can consume the same ordered reviewer queue from the shortest status
entrypoint. Its `next_manual_signoff` object also mirrors the next row's automated evidence, reviewer check,
suggested evidence note, actionable flag, and recorder command from signoff status JSON, so a compact dashboard can show
exactly what the next reviewer must inspect. The same compact JSON also includes `review_surfaces`, a pinned set
of reviewer-facing paths for the runbook, automated evidence, manual evidence
map, user acceptance packet, signoff status report, next-row review, signoff
dashboard, and UI review gallery. Its `next_actions` array pins the five
operator actions in order: review status, review next signoff, record the next
signoff, verify all manual signoff rows, and finalize acceptance. Each action
has an enabled flag, blocking reason keys, command, and evidence path, so MCP,
CLI, webhook, CI, release automation, and dashboards can render the next step
without reconstructing it from counters.
`/opt/worker/report/openpr/docs/openpr-universal-form-signoff-dashboard-2026-05-31.html`
renders that queue with a Start Here section, evidence links, primary
screenshots, and recorder commands for all pending rows. Regenerate it with
`scripts/prepare-universal-forms-signoff-dashboard.sh` and verify it with
`scripts/verify-universal-forms-signoff-dashboard.sh`. Browser-render it with
`scripts/smoke-universal-forms-signoff-dashboard-render.sh` to capture the
desktop/mobile reviewer screenshots in the delivery artifacts; that smoke reads the current signoff status JSON, so it follows the next reviewer key after each
accepted row instead of assuming the first row is still pending. After all
manual rows are accepted, the same dashboard switches Start Here to finalization
commands instead of showing an empty next reviewer command.
`scripts/smoke-universal-forms-next-signoff-command.sh` proves that the next
recorder command can dry-run against temporary runbook/evidence copies without
mutating official handoff files.
`scripts/smoke-universal-forms-manual-signoff-progression.sh` records each
manual row on temporary copies one at a time and verifies accepted/pending
counts, next-row routing, actionable state, and the final signoff flag after
every step.
`scripts/smoke-universal-forms-manual-signoff-commands.sh` records all seven
manual signoff keys on temporary copies, in prerequisite order, then runs the
final signoff verifier against those temporary fully signed files.
`scripts/smoke-universal-forms-signoff-status-output.sh` keeps the reviewer-facing
manual signoff status Markdown aligned with the signoff status JSON, including
gate counts, the seven row table, next action, and recorder command.
`scripts/status-universal-forms-delivery.sh` is the shortest read-only status
view for humans and CI; it verifies JSON contracts, next-row dry-run,
row-by-row signoff progression, all manual row command dry-runs, and the
pre-signoff release gate before printing counts. Successful prerequisite checks
stay quiet on stdout and stderr; failures replay the captured prerequisite log.
Use `--json` when automation needs the same summary.
`scripts/smoke-universal-forms-delivery-status-output.sh` keeps the default
human-readable output aligned with that JSON summary, including the next manual
signoff key and recorder command.
`scripts/smoke-universal-forms-signoff-dashboard-progression.sh` renders the
manual signoff dashboard on temporary runbook/evidence copies for 0/7 accepted,
after the first row moves to `frontend_usability`, and after 7/7 accepted with
finalizer commands, while confirming the official handoff files remain
unchanged. Successful subcommands stay quiet, and failures replay captured
stdout/stderr for diagnosis.
Its JSON contract is documented in
`docs/schemas/openpr-universal-forms-delivery-status.schema.json`.
That compact JSON is intentionally generated on demand rather than persisted in
the checksum manifest because it includes the current generation time and the
manifest file count.
The release gate JSON is the machine-readable release decision contract for CI,
MCP tools, webhook consumers, and release automation. Its schema is
`docs/schemas/openpr-universal-forms-release-gate.schema.json`; the verifier
checks parity with readiness, development status, signoff status, and final
flags, while the contract smoke rejects release-allowed, pending-row, next-key,
and tracker-status drift.
`scripts/smoke-universal-forms-release-gate-output.sh` keeps the human-readable
release gate output aligned with that JSON decision for both pre-signoff and
strict release modes.
The release gate mode enum is `blocked`, `pre_signoff`, and `release`; the compact delivery status JSON mirrors that same enum.
The release and delivery status next manual keys are constrained to the same seven manual signoff keys, or an empty string after final signoff, so automation cannot route a reviewer action to an unknown row.
The compact delivery status JSON also includes `verified_prerequisites`, a pinned eight-item list covering readiness, development status, signoff status, delivery manifest, next signoff command smoke, manual signoff progression smoke, all-row manual command smoke, and the pre-signoff release gate JSON.
The readiness, signoff status, release gate, and delivery status verifiers also reject extra fields inside nested status objects, so CI, MCP tools, webhook consumers, and release automation can treat those JSON shapes as pinned contracts.
The completion audit JSON is the machine-readable completion gate. Its schema
is `docs/schemas/openpr-universal-forms-completion-audit.schema.json`; the
verifier checks automated evidence, non-manual tracker rows, manual signoff
counts, finalizer safety, and finalization flags before automation consumes the
handoff. It also rejects extra fields inside `reports`, `gates`,
`tracker_status`, `manual_signoff`, and `finalization`, so completion-gate
consumers do not need to tolerate undocumented nested shape drift.
The development status JSON mirrors the tracker development matrix with each
engineering phase, engineering requirement, completion rule, current status,
evidence, and final-release flag. Its contract is documented in
`docs/schemas/openpr-universal-forms-development-status.schema.json`; the schema
pins the ten development rows in tracker order and the verifier rejects extra
fields inside `status_summary` and row objects.
The scenario catalog JSON mirrors the built-in scenario template catalog,
including template keys, project types, generated forms, integrations,
operator entrypoints, operator steps, primary MCP tools, connector kinds,
plugin keys, and acceptance focus points. Its contract is documented in
`docs/schemas/openpr-universal-forms-scenario-catalog.schema.json`; the schema
pins the six built-in templates in catalog order and the verifier rejects extra
fields inside operator entrypoint and template objects.
The live scenario template API and MCP tools also return the same runtime
`usage_guide` shape, so `scenario_templates.list`, `scenario_templates.get`,
`scenario_templates.install`,
and `openpr://scenario-templates` can drive frontend onboarding, AI routing,
connector setup, and template marketplace views without scraping Markdown or
the delivery JSON report.
The implementation map JSON mirrors the developer implementation and
acceptance map, including delivery areas, source paths, public surfaces,
verification commands, status markers, and the manual-release boundary. Its
contract is documented in
`docs/schemas/openpr-universal-forms-implementation-map.schema.json`; the schema
pins status marker and module order, and the verifier rejects extra fields
inside marker, module, and release-boundary objects.
The delivery manifest JSON mirrors the Markdown checksum manifest for CI,
release automation, MCP tools, and webhook consumers that need exact file rows
without parsing Markdown. Its contract is documented in
`docs/schemas/openpr-universal-forms-delivery-manifest.schema.json`.
Its contract smoke confirms selected malformed JSON copies are rejected before
automation relies on the delivery manifest.

Final acceptance is only complete after all seven manual signoff rows are
accepted, `scripts/finalize-universal-forms-acceptance.sh` succeeds, and the
strict delivery-state and delivery-bundle audits pass. Until then, the tracker
must keep `用户侧人工验收` as `待处理`.

## Quick Start

### Docker Compose (Recommended)

```bash
git clone https://github.com/openprx/openpr.git
cd openpr
bash scripts/start.sh
```

The start script creates a local `.env` on first run, generates non-production
bootstrap secrets, builds release binaries for `Dockerfile.prebuilt`, and then
starts the compose stack. Replace the generated secrets before production use.

To create a local restaurant-ordering demo after the stack is healthy:

```bash
scripts/bootstrap-restaurant-demo.sh
```

The demo script uses the public API to create or reuse a local demo account,
workspace, `restaurant_ordering_default` project, and sample universal-form
records. It also creates a workspace-scoped MCP bot token, writes
`OPENPR_BOT_TOKEN` and `OPENPR_WORKSPACE_ID` to the local `.env` when present,
and recreates a running compose `mcp-server` so MCP clients can use the same
demo workspace. When the MCP HTTP endpoint is reachable, it also verifies
`projects.list` through `/mcp/rpc` and checks that the demo project is visible.
It prints the frontend Forms URL when finished. By default it refuses non-local
API URLs unless `OPENPR_DEMO_ALLOW_REMOTE=1` is set for an intentional demo
environment.

To run the same bootstrap against disposable PostgreSQL/API/MCP processes and
force the MCP HTTP JSON-RPC proof without touching your local `.env`:

```bash
scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh
```

Services:

- **Frontend**: http://localhost:3000
- **API**: http://localhost:8081
- **MCP**: http://localhost:8090

### Development

```bash
# Prerequisites: stable Rust with edition 2024 support, Bun 1.3+, PostgreSQL 15+

# Backend
cp .env.example .env
cargo build
cargo run --bin api

# Frontend
cd frontend && bun install && bun run dev

# MCP Server
OPENPR_API_URL=http://localhost:8081 \
OPENPR_BOT_TOKEN=opr_your_token_here \
OPENPR_WORKSPACE_ID=your-workspace-uuid \
cargo run --bin mcp-server -- serve --transport http
```

## MCP Server

The built-in MCP server exposes OpenPR as a tool provider for AI assistants.
Three transport protocols are supported simultaneously.

### Transport Protocols

| Protocol  | Use Case                           | Endpoint                                      |
| --------- | ---------------------------------- | --------------------------------------------- |
| **HTTP**  | Web integrations, OpenClaw plugins | `POST /mcp/rpc`                               |
| **stdio** | Claude Desktop, Codex, local CLI   | stdin/stdout JSON-RPC                         |
| **SSE**   | Streaming clients, real-time UIs   | `GET /sse` → `POST /messages?session_id=<id>` |

> HTTP mode exposes all three protocols on a single port: `/mcp/rpc` (HTTP), `/sse` + `/messages` (SSE), and health at `/health`.

### MCP Client Configuration

#### Claude Desktop / Cursor / Codex (stdio)

```json
{
  "mcpServers": {
    "openpr": {
      "command": "/path/to/mcp-server",
      "args": ["serve", "--transport", "stdio"],
      "env": {
        "OPENPR_API_URL": "http://localhost:8081",
        "OPENPR_BOT_TOKEN": "opr_your_token_here",
        "OPENPR_WORKSPACE_ID": "your-workspace-uuid"
      }
    }
  }
}
```

#### HTTP Mode

```bash
OPENPR_API_URL=http://localhost:8081 \
OPENPR_BOT_TOKEN=opr_your_token_here \
OPENPR_WORKSPACE_ID=your-workspace-uuid \
./target/release/mcp-server serve --transport http

# Verify
curl -X POST http://localhost:8090/mcp/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'

# Verify project-aware capability filtering
curl -X POST http://localhost:8090/mcp/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"project_id":"<project-uuid>"}}'
```

#### SSE Mode

```bash
# 1. Connect SSE stream (returns session endpoint)
curl -N -H "Accept: text/event-stream" http://localhost:8090/sse
# → event: endpoint
# → data: /messages?session_id=<uuid>

# 2. POST request to the returned endpoint
curl -X POST "http://localhost:8090/messages?session_id=<uuid>" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"projects.list","arguments":{}}}'
# → Response arrives via SSE stream as event: message
```

#### Docker Compose

```yaml
mcp-server:
  build:
    context: .
    dockerfile: Dockerfile.prebuilt
    args:
      APP_BIN: mcp-server
  environment:
    - OPENPR_API_URL=http://api:8080
    - OPENPR_BOT_TOKEN=opr_your_token
    - OPENPR_WORKSPACE_ID=your-workspace-uuid
  ports:
    - "127.0.0.1:8090:8090"
  command:
    [
      "/app/mcp-server",
      "serve",
      "--transport",
      "http",
      "--bind-addr",
      "0.0.0.0:8090",
    ]
```

### Environment Variables

| Variable              | Required | Description               | Example                 |
| --------------------- | -------- | ------------------------- | ----------------------- |
| `OPENPR_API_URL`      | Yes      | API server base URL       | `http://localhost:8081` |
| `OPENPR_BOT_TOKEN`    | Yes      | Bot token (`opr_` prefix) | `opr_abc123...`         |
| `OPENPR_WORKSPACE_ID` | Yes      | Default workspace UUID    | `e5166fd1-...`          |

### Bot Token Authentication

MCP authenticates via **Bot Tokens** (prefix `opr_`), managed at **Workspace → Members → Bot Tokens** in the frontend.

Each bot token:

- Has a display name (shown in activity feeds)
- Is scoped to one workspace
- Creates a `bot_mcp` user entity for audit trail integrity
- Supports all read/write operations available to workspace members

### Tool Reference (105 tools)

#### Projects (5)

| Tool              | Required Params | Description                                                                                       |
| ----------------- | --------------- | ------------------------------------------------------------------------------------------------- |
| `projects.list`   | —               | List all projects in workspace                                                                    |
| `projects.get`    | `project_id`    | Get project details with issue counts                                                             |
| `projects.create` | `key`, `name`   | Create a project (key e.g. `PRX`). Optional: `type_key`, `type_settings`, `scenario_template_key` |
| `projects.update` | `project_id`    | Update name/description/type                                                                      |
| `projects.delete` | `project_id`    | Delete a project                                                                                  |

#### Project Types, Templates & Resources (9)

| Tool                       | Required Params              | Description                                                                                                       |
| -------------------------- | ---------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| `project_types.list`       | -                            | List scenario types available to the workspace                                                                    |
| `project_types.get`        | `key`                        | Get a scenario type capability schema                                                                             |
| `scenario_templates.list`  | -                            | List project creation templates with workflow, fields, resources, AI roles, governance, and suggested connections |
| `scenario_templates.get`   | `key`                        | Get one scenario template for project setup or agent instructions                                                 |
| `scenario_templates.install` | `project_id`, `template_key` | Install a complete multi-form scenario template bundle into an existing project                                   |
| `project_resources.list`   | `project_id`                 | List project-bound resources for AI context                                                                       |
| `project_resources.create` | `project_id`, `kind`, `name` | Add a repository, directory, document library, equipment, site, business record, or custom resource               |
| `project_resources.update` | `project_id`, `resource_id`  | Update resource metadata, locator, policy, or sync status                                                         |
| `project_resources.delete` | `project_id`, `resource_id`  | Delete a project resource                                                                                         |

#### Context (3)

| Tool                       | Required Params | Description                                                                                                                  |
| -------------------------- | --------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `context.get_project`      | `project_id`    | Get project-aware MCP context: project type, resources, connectors, governance, workflow, recent decisions, and agent policy |
| `context.get_governance`   | `project_id`    | Get governance, workflow, and recent decision context                                                                        |
| `context.get_agent_policy` | `project_id`    | Get effective MCP/AI action policy and project capability tool registry                                                      |

#### Connectors & Invocations (8)

| Tool                          | Required Params              | Description                                                  |
| ----------------------------- | ---------------------------- | ------------------------------------------------------------ |
| `connectors.list`             | -                            | List workspace or project automation connectors              |
| `connectors.get`              | `connector_id`               | Get a connector by UUID                                      |
| `invocations.list`            | `project_id`                 | List AI execution ledger entries for a project               |
| `invocations.get`             | `invocation_id`              | Get an invocation ledger entry by UUID                       |
| `invocations.create`          | `project_id`, `trigger_kind` | Create an auditable MCP/AI invocation ledger entry           |
| `invocations.report_progress` | `invocation_id`              | Mark an invocation running and attach progress payload       |
| `invocations.complete`        | `invocation_id`              | Complete an invocation with a result payload                 |
| `invocations.fail`            | `invocation_id`              | Fail an invocation with an error message and optional result |

#### Universal Forms & Events (33)

| Tool                       | Required Params       | Description                                                            |
| -------------------------- | --------------------- | ---------------------------------------------------------------------- |
| `forms.list`               | `project_id`          | List project form definitions                                          |
| `forms.get`                | `form_id`             | Get one form definition, fields, and views                             |
| `forms.create`             | `project_id`, `key`   | Create a project-defined business form                                 |
| `forms.create_from_template` | `project_id`, `template_key` | Create one universal form from a scenario template inside an existing project |
| `forms.update_schema`      | `form_id`, `schema`   | Update a form schema through validation and business-event tracking     |
| `forms.duplicate`          | `form_id`             | Duplicate form metadata, schema, detail layout, and active views        |
| `forms.schema_summary`     | `form_id`             | Get schema, view, record, and archive summary metadata                  |
| `forms.field_usage`        | `form_id`             | Get field value usage and dependency counts                             |
| `forms.field_dependencies` | `form_id`             | List field dependencies from views, formulas, relations, and templates  |
| `form_schema_versions.list` | `form_id`            | List archived schema versions for a form                                |
| `form_schema_versions.get` | `form_id`, `version`  | Get one archived schema version for a form                              |
| `form_permissions.get`     | `form_id`             | Get role action permission policies and caller effective actions         |
| `form_permissions.update`  | `form_id`, `policies` | Upsert form-level role action permission policies                       |
| `form_views.list`          | `form_id`             | List grid/detail views for a form                                      |
| `form_attachments.list`    | `form_id`             | List attachment and image metadata rows for a form                      |
| `form_attachments.create`  | `form_id`, `field_key` | Create attachment or image metadata without uploading file bytes       |
| `form_attachments.archive` | `attachment_id`       | Archive one attachment metadata row                                    |
| `form_attachments.restore` | `attachment_id`       | Restore one archived attachment metadata row                           |
| `form_records.list`        | `form_id`             | List records for a form                                                |
| `form_records.export`      | `form_id`             | Export records as current-view or explicit-column CSV/row data         |
| `form_records.import_preview` | `form_id`, `rows`  | Preview import rows through backend normalization without writing       |
| `form_records.import_commit` | `form_id`, `rows`   | Commit valid import rows through the normal record create pipeline      |
| `form_records.get`         | `record_id`           | Get one form record with values and links                              |
| `form_records.create`      | `form_id`, `values`   | Create a record; decimal/amount fields must be strings, not JSON numbers |
| `form_records.update`      | `record_id`, `values` | Update a record through schema validation                              |
| `form_records.link`        | `source_record_id`    | Link records for subform or parent-child relationships                 |
| `form_records.relation_targets` | `form_id`        | List selectable relation target records                                |
| `form_records.children`    | `record_id`           | List child records linked to a parent record                           |
| `form_records.child_create` | `record_id`, `relation_key`, `values` | Create and link a child record through a parent-child relation |
| `form_records.child_update` | `record_id`, `child_record_id` | Update a child record after verifying it is linked to the parent |
| `form_records.child_archive` | `record_id`, `child_record_id` | Archive a child record after verifying it is linked to the parent |
| `form_records.child_restore` | `record_id`, `child_record_id` | Restore an archived child record after verifying it is linked to the parent |
| `form_records.aggregate`   | `form_id`             | Aggregate numeric/decimal projections                                  |
| `events.tail`              | `project_id`          | Read recent business events for forms, records, plugins, and connectors |

#### WASM Plugins (5)

| Tool                      | Required Params              | Description                                              |
| ------------------------- | ---------------------------- | -------------------------------------------------------- |
| `plugins.list`            | `project_id`                 | List project plugins and their active state              |
| `plugins.get`             | `plugin_id`                  | Get one plugin manifest and runtime metadata             |
| `plugins.install`         | `project_id`, `manifest`     | Install a sandboxed plugin with declared capabilities    |
| `plugins.invoke`          | `plugin_id`, `function_name` | Manually invoke a plugin function through the audit path |
| `plugin_invocations.list` | `project_id`                 | List plugin invocation history                           |

#### Work Items / Issues (11)

| Tool                           | Required Params             | Description                                                                                                                        |
| ------------------------------ | --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `work_items.list`              | `project_id`                | List issues in a project                                                                                                           |
| `work_items.get`               | `work_item_id`              | Get issue by UUID                                                                                                                  |
| `work_items.get_by_identifier` | `identifier`                | Get by human ID (e.g. `PRX-42`)                                                                                                    |
| `work_items.create`            | `project_id`, `title`       | Create issue. Optional: `state` (backlog/todo/in_progress/done), `priority`, `description`, `assignee_id`, `due_at`, `attachments` |
| `work_items.update`            | `work_item_id`              | Update any field. `attachments` appended as markdown links                                                                         |
| `work_items.delete`            | `work_item_id`              | Delete an issue                                                                                                                    |
| `work_items.search`            | `query`                     | Full-text search across all projects                                                                                               |
| `work_items.add_label`         | `work_item_id`, `label_id`  | Add one label                                                                                                                      |
| `work_items.add_labels`        | `work_item_id`, `label_ids` | Add multiple labels                                                                                                                |
| `work_items.remove_label`      | `work_item_id`, `label_id`  | Remove a label                                                                                                                     |
| `work_items.list_labels`       | `work_item_id`              | List labels on an issue                                                                                                            |

#### Comments (3)

| Tool              | Required Params           | Description                             |
| ----------------- | ------------------------- | --------------------------------------- |
| `comments.create` | `work_item_id`, `content` | Create comment. Optional: `attachments` |
| `comments.list`   | `work_item_id`            | List comments on an issue               |
| `comments.delete` | `comment_id`              | Delete a comment                        |

#### Files (1)

| Tool           | Required Params              | Description                                                                                                                      |
| -------------- | ---------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `files.upload` | `filename`, `content_base64` | Upload file (base64), returns `{ url, filename }`. Types: images, `.zip`, `.gz`, `.log`, `.txt`, `.pdf`, `.json`, `.csv`, `.xml` |

#### Labels (5)

| Tool                     | Required Params | Description                              |
| ------------------------ | --------------- | ---------------------------------------- |
| `labels.list`            | —               | List all workspace labels                |
| `labels.list_by_project` | `project_id`    | List labels for a project                |
| `labels.create`          | `name`, `color` | Create label (color: hex e.g. `#2563eb`) |
| `labels.update`          | `label_id`      | Update name/color/description            |
| `labels.delete`          | `label_id`      | Delete a label                           |

#### Sprints (4)

| Tool             | Required Params      | Description                                       |
| ---------------- | -------------------- | ------------------------------------------------- |
| `sprints.list`   | `project_id`         | List sprints in a project                         |
| `sprints.create` | `project_id`, `name` | Create sprint. Optional: `start_date`, `end_date` |
| `sprints.update` | `sprint_id`          | Update name/dates/status                          |
| `sprints.delete` | `sprint_id`          | Delete a sprint                                   |

#### Proposals & Check Results (5)

| Tool                           | Required Params                      | Description                                                  |
| ------------------------------ | ------------------------------------ | ------------------------------------------------------------ |
| `proposals.list`               | `project_id`                         | List proposals, optional `status` filter                     |
| `proposals.get`                | `proposal_id`                        | Get proposal details                                         |
| `proposals.create`             | `project_id`, `title`, `description` | Create a governance proposal                                 |
| `check_results.create`         | `project_id`, `title`, `summary`     | Record a governed MCP/AI check result for a high-risk action |
| `proposals.create_from_result` | `check_result_id`                    | Create a governance proposal from a check result             |

#### Release Readiness (1)

| Tool                    | Required Params | Description                                                                  |
| ----------------------- | --------------- | ---------------------------------------------------------------------------- |
| `release.readiness.get` | `project_id`    | Read acceptance gates, blockers, release metrics, next actions, and AI governance evidence |

The `release.readiness.get` response is versioned as
`openpr.project.release_readiness.v1` and points to
`docs/schemas/openpr-project-release-readiness.schema.json`, so MCP clients,
webhook consumers, and release automation can validate gates, metrics, and
`next_actions` without scraping prose.

#### Scenario Tools (9)

| Tool                          | Required Params                  | Description                                                   |
| ----------------------------- | -------------------------------- | ------------------------------------------------------------- |
| `code.resources.list`         | `project_id`                     | List code-oriented resources such as repos and directories    |
| `code.directory.get`          | `project_id`                     | Read a directory/repo resource locator and policy             |
| `code.task_context.get`       | `project_id`                     | Read project context plus optional work item context          |
| `code.change_proposal.create` | `project_id`, `title`, `summary` | Record a governed code change proposal as a check result      |
| `documents.extract_summary`   | `project_id`, `title`, `summary` | Record document summary output as a governed check result     |
| `documents.review_risk`       | `project_id`, `title`, `summary` | Record document risk review output as a governed check result |
| `approval.request`            | `project_id`, `title`, `summary` | Record a governed approval request                            |
| `inspection.report`           | `project_id`, `title`, `summary` | Record inspection findings                                    |
| `corrective_action.propose`   | `project_id`, `title`, `summary` | Record a corrective-action proposal                           |

#### Members & Search (2)

| Tool           | Required Params | Description                                     |
| -------------- | --------------- | ----------------------------------------------- |
| `members.list` | —               | List workspace members and roles                |
| `search.all`   | `query`         | Global search across projects, issues, comments |

### Response Format

Success:

```json
{ "code": 0, "message": "success", "data": { ... } }
```

Error:

```json
{ "code": 400, "message": "error description" }
```

## Skills and Agent Integration

- **Agent guide**: `apps/mcp-server/AGENTS.md` — workflow patterns and full tool examples for coding agents.
- **Skill package**: `skills/openpr-mcp/SKILL.md` — governed skill with workflow lines, templates, and scripts.
- **Client discovery**:
  1. Load `AGENTS.md` for tool semantics.
  2. Use `tools/list` to enumerate all tools, or pass `params.project_id` to enumerate project-capability-filtered tools.
  3. Follow workflow patterns (search → create → label → comment) for structured task execution.

## Features

### Project Management

- Workspaces & Projects — Multi-tenant isolation with role-based access
- Issues & Board — Kanban with drag-and-drop, priority, assignees, labels
- Sprints & Cycles — Sprint planning with cycle tracking
- Full-text Search — PostgreSQL FTS across issues, comments, proposals
- File Uploads — Attachments on issues, comments, proposals (images, docs, logs, archives)
- Activity Feed — Per-issue activity stream
- Notifications & Inbox — In-app notification center
- Import / Export — Bulk data operations

### Governance Center

- Proposals — Create, review, vote with configurable thresholds
- Voting System — Weighted voting with quorum
- Decision Records — Immutable log with audit trail
- Veto & Escalation — Veto power with escalation voting
- Trust Scores — Per-user scoring with history and appeals
- Proposal Templates, Chains, Impact Reviews
- Audit Logs & Analytics

### AI Integration

- AI Agents — Register AI participants with roles and permissions
- AI Tasks — Assign tasks to agents with progress tracking
- AI Review — AI feedback on proposals
- Bot Tokens — `opr_` prefixed workspace-scoped tokens
- MCP Server — 105 tools, 3 transports
- Webhook Callbacks — Task completion/failure/progress

## API Overview

| Category   | Endpoints                      | Description                       |
| ---------- | ------------------------------ | --------------------------------- |
| Auth       | `/api/auth/*`                  | Register, login, refresh token    |
| Projects   | `/api/workspaces/*/projects/*` | CRUD, members, settings           |
| Issues     | `/api/projects/*/issues/*`     | CRUD, assign, label, comment      |
| Board      | `/api/projects/*/board`        | Kanban board state                |
| Sprints    | `/api/projects/*/sprints/*`    | Sprint CRUD and planning          |
| Proposals  | `/api/proposals/*`             | Create, vote, submit, archive     |
| Governance | `/api/governance/*`            | Config, audit logs                |
| Decisions  | `/api/decisions/*`             | Decision records                  |
| Trust      | `/api/trust-scores/*`          | Trust scores, history, appeals    |
| Veto       | `/api/veto/*`                  | Veto, escalation, voting          |
| AI Agents  | `/api/projects/*/ai-agents/*`  | Agent registration and management |
| AI Tasks   | `/api/projects/*/ai-tasks/*`   | Task assignment and callbacks     |
| Bots       | `/api/workspaces/*/bots`       | Bot token CRUD                    |
| Upload     | `/api/v1/upload`               | File upload (multipart/form-data) |
| Webhooks   | `/api/workspaces/*/webhooks/*` | Webhook CRUD and delivery log     |
| Search     | `/api/search`                  | Full-text search                  |
| Admin      | `/api/admin/*`                 | User and system management        |

## Documentation Map

| Document         | Location                     | Purpose                           |
| ---------------- | ---------------------------- | --------------------------------- |
| Project overview | `README.md`                  | Setup, MCP config, tool reference |
| Agent guide      | `apps/mcp-server/AGENTS.md`  | Coding agent workflow patterns    |
| Skill package    | `skills/openpr-mcp/SKILL.md` | Governed MCP skill for agents     |
| API routes       | `apps/api/src/main.rs`       | Route registration                |
| MCP tools        | `apps/mcp-server/src/tools/` | Tool implementations              |
| Frontend         | `frontend/`                  | SvelteKit app                     |
| Migrations       | `migrations/`                | Database schema                   |

## Links

- [Documentation](https://docs.openprx.dev/en/openpr/) — Full OpenPR documentation (10 languages)
- [Community](https://community.openprx.dev) — OpenPRX community forum
- [OpenPRX](https://openprx.dev) — Project homepage

## Related Projects

| Repository                                                  | Description                                     |
| ----------------------------------------------------------- | ----------------------------------------------- |
| [openpr](https://github.com/openprx/openpr)                 | Core platform (this repo)                       |
| [openpr-webhook](https://github.com/openprx/openpr-webhook) | Webhook receiver for external integrations      |
| [prx](https://github.com/openprx/prx)                       | AI assistant framework with built-in OpenPR MCP |
| [prx-memory](https://github.com/openprx/prx-memory)         | Local-first MCP memory for coding agents        |
| [wacli](https://github.com/openprx/wacli)                   | WhatsApp CLI with JSON-RPC daemon               |

## Tech Stack

- **Backend**: Rust, Axum, SeaORM, PostgreSQL
- **Frontend**: SvelteKit, TailwindCSS, shadcn-svelte
- **MCP**: JSON-RPC 2.0 (HTTP + stdio + SSE)
- **Auth**: JWT (access + refresh) + Bot Tokens (`opr_`)
- **Deployment**: Docker Compose, Podman, Nginx

## License

Dual-licensed under [MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE).
