# Universal Forms, Plugins, and Scenario Templates

OpenPR supports project-defined business applications through universal forms, connector events, MCP tools, and sandboxed WASM plugins.

## Runtime Model

Universal forms are project-scoped data types. Each form defines a schema, title template, optional detail layout, and one or more views. Records store normalized values and write field indexes for query and aggregate paths.

Core tables:

- `project_forms` defines business types such as `order`, `order_line`, `print_job`, or `business_report`.
- `form_views` stores grid/detail view configuration.
- `form_records` stores record values.
- `form_record_links` stores parent-child and reference relationships.
- `form_record_field_index` stores typed projections, including decimal amount values.
- `business_events`, `event_outbox`, and `event_inbox` provide event delivery and idempotent receipts.
- `plugins` and `plugin_invocations` store WASM plugin packages and execution logs.

Amount fields use decimal strings at API boundaries. JSON numbers are rejected for amount input so money and business totals do not pass through floating-point math.

## API, MCP, and Connectors

The REST API is the source of truth for forms, records, links, events, aggregates, plugins, connectors, and print receipts.

MCP exposes the same business surface to agents through forms tools, plugin tools, events resources, and aggregate reads. This lets human users work in the frontend while AI agents use the same project data and event ledger.

Connectors are passive or active integration endpoints. A webhook does not have to be an agent. Print, device, REST, webhook, MCP, CLI, and tunnel connectors are all treated as event consumers with auditable invocation and receipt records.

## WASM Plugins

Plugins use `openpr.plugin.v1` manifests and a small core WASM ABI:

- `field_validator` can block invalid record writes.
- `formula` can return a patch before the final schema and decimal validation pass.
- `event_handler` can react to business events after writes commit.
- Plugin-provided tools appear through MCP when declared in the manifest.

WASM modules run under wasmtime with fuel, timeout, and memory limits. Plugins have no host imports or WASI access in the current runtime.

## Scenario Templates

Creating a project with a scenario template initializes the project as a ready-to-use business workspace.

Current built-in templates:

- `code_delivery_default`
- `contract_review_default`
- `equipment_maintenance_default`
- `quality_corrective_action_default`
- `customer_delivery_default`
- `restaurant_ordering_default`

See `docs/scenario-templates.md` for the full scenario catalog: business fit,
generated forms, connector suggestions, MCP usage, frontend usage, and extension
rules for adding new scenarios.

Each template creates default forms and grid/detail views. Templates also create connector suggestions. The restaurant template additionally auto-installs and activates the `restaurant_calc` WASM plugin.

## Restaurant Reference Flow

The restaurant scenario is the delivery reference for universal business usage:

1. Create a restaurant project from `restaurant_ordering_default`.
2. Create menu category, SKU, and table records.
3. Create an order and order line.
4. The `restaurant_calc` formula plugin calculates `line_total`.
5. Link order line to order through `parent_child`.
6. Change table and emit `order.table_changed`.
7. Create kitchen and receipt `print_job` records.
8. Deliver print events to a print connector and accept receipts.
9. Create `business_report` and query revenue through MCP aggregate.

For a local stack started with `bash scripts/start.sh`, this can be seeded
through the public API with:

```bash
scripts/bootstrap-restaurant-demo.sh
```

The demo helper creates or reuses a local user, workspace, restaurant scenario
project, sample menu/table/order/order-line/report records, a `parent_child`
link, and a workspace-scoped MCP bot token. When the MCP configuration file
exists it writes `mcp.bot_token` and `mcp.workspace_id` into it, then recreates
a running compose
`mcp-server` so MCP clients use the same demo workspace. If the MCP HTTP
endpoint is reachable, it verifies `/mcp/rpc` with `projects.list` and confirms
the demo project appears through MCP. It refuses non-local API URLs by default
so the built-in demo credentials are not accidentally used against production.
For a disposable verification of that full API -> bot token -> MCP HTTP path,
run `scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh`; it uses a temporary
database and a temporary configuration file.

## Verification

Backend and integration:

```bash
cargo fmt --all -- --check
cargo clippy -p api -p worker -p mcp-server --all-targets --all-features -- -D warnings
cargo test -p api routes::project::tests::
scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh
cargo test -p api forms::
scripts/audit-universal-forms-docs.sh
scripts/smoke-forms-mcp.sh
scripts/smoke-scenario-template-forms.sh
scripts/smoke-restaurant-ordering.sh
```

Frontend:

```bash
cd frontend
bun run check
bun run build
bun run smoke:forms-ui
bun run smoke:restaurant-ordering
```

Delivery requires the relevant checklist in `report/openpr/docs/openpr-universal-form-development-execution-tracker-2026-05-31.md` to be marked `已测试` or `已验收` with command evidence.
