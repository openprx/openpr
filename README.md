# OpenPR

Open-source project management platform with built-in governance, a universal
business-form engine, WASM plugins, and a first-class MCP server for AI agents.
Built with **Rust** (Axum + SeaORM), **SvelteKit**, and **PostgreSQL 16**.

## What It Provides

- **Project management** — workspaces, projects, issues, kanban board, sprints, labels, comments, activity feed, notifications, attachments.
- **Governance** — proposals, weighted voting, decision records, veto and escalation, trust scores, appeals, impact reviews, audit logs.
- **Universal forms** — project-defined business data types with grid/detail views, decimal-safe amounts, record links and child tables, formulas, per-role permissions, import/export, electronic signatures.
- **WASM plugins** — per-project sandboxed plugins for field validation, formulas, and event handlers.
- **Events** — transactional outbox/inbox with asynchronous connector delivery, plus a legacy webhook path.
- **MCP server** — 105 tools, 4 static resources, 19 resource templates, 3 transports; the same binary is also a CLI.
- **Scenario templates** — 6 ready-to-start setups: `code_delivery_default`, `contract_review_default`, `equipment_maintenance_default`, `quality_corrective_action_default`, `customer_delivery_default`, `restaurant_ordering_default`.

## Architecture

| Component    | Path              | Role                                                                 |
| ------------ | ----------------- | -------------------------------------------------------------------- |
| `api`        | `apps/api`        | HTTP API (301 endpoints), form engine, plugin runtime, event emission |
| `worker`     | `apps/worker`     | Background pipelines: AI tasks, event outbox/inbox, connectors, jobs  |
| `mcp-server` | `apps/mcp-server` | MCP server (HTTP/stdio/SSE) and CLI over the API                     |
| `frontend`   | `frontend`        | SvelteKit 2 SPA (adapter-static)                                     |

`crates/platform` holds shared config, DB connection, auth, error, and logging.
`migrations/` holds 47 SQL migrations (`0001`–`0047`) defining 69 tables.

## Quick Start

```bash
git clone https://github.com/openprx/openpr.git
cd openpr
bash scripts/start.sh
```

`scripts/start.sh` creates a local `.env` on first run with random non-production
`POSTGRES_PASSWORD` / `JWT_SECRET`, builds release binaries for
`Dockerfile.prebuilt`, then runs `docker compose up -d --build`. Replace the
generated secrets before production use. Services publish on
`${OPENPR_BIND_HOST:-127.0.0.1}`: frontend `:3000`, API `:8081`, MCP `:8090`.
For demo data once healthy, `scripts/bootstrap-restaurant-demo.sh` creates a
demo account, workspace, `restaurant_ordering_default` project with sample
records, and a workspace-scoped bot token; it refuses non-local API URLs unless
`OPENPR_DEMO_ALLOW_REMOTE=1`.

### Local development

```bash
# Prerequisites: stable Rust with edition 2024 support, Bun, PostgreSQL 16
scripts/dev-up.sh          # start only PostgreSQL from compose
cp .env.example .env
cargo run --bin api        # listens on BIND_ADDR, default 0.0.0.0:8080
cargo run --bin worker

cd frontend && bun install && bun run dev

OPENPR_API_URL=http://localhost:8081 \
OPENPR_BOT_TOKEN=opr_your_token_here \
OPENPR_WORKSPACE_ID=your-workspace-uuid \
cargo run --bin mcp-server -- serve --transport http
```

## Configuration

**Read by the programs.** api and worker: `DATABASE_URL`, `JWT_SECRET`
(required), `JWT_ACCESS_TTL_SECONDS`, `JWT_REFRESH_TTL_SECONDS`, `APP_NAME`,
`BIND_ADDR` (api), `DEFAULT_AUTHOR_ID`, `UPLOAD_DIR`,
`OPENPR_OBJECT_STORAGE_BACKEND` (`local` | `s3-compatible`),
`OPENPR_OBJECT_STORAGE_DIR`, and `OPENPR_OBJECT_STORAGE_S3_{ENDPOINT, BUCKET,
REGION, ACCESS_KEY_ID, SECRET_ACCESS_KEY, SESSION_TOKEN}`. mcp-server:
`OPENPR_API_URL`, `OPENPR_BOT_TOKEN`, `OPENPR_WORKSPACE_ID`,
`OPENPR_INVOCATION_ID` (optional ledger correlation), `OPENPR_MCP_TRANSPORT`
(transport label in tool payloads, default `mcp_stdio`). All binaries read
`RUST_LOG` as the `tracing` env filter.

**Outbound deliveries (api + worker).** Connector and webhook endpoints are
validated when they are configured and again before every delivery: an endpoint
whose host resolves to a loopback, private, link-local, NAT64/6to4 or otherwise
internal address is refused, and redirects are not followed.

| Variable | Default | Meaning |
| --- | --- | --- |
| `OPENPR_OUTBOUND_ALLOWED_HOSTS` | compose sets `webhook:9090,api:8080,mcp-server:8090,frontend:80` | Comma separated `host` or `host:port` entries exempt from the private address checks. Internal receivers (compose services, in-cluster bots) must be listed here or their deliveries are rejected. |
| `OPENPR_OUTBOUND_ALLOW_PRIVATE` | unset (checks on) | `1`/`true`/`yes` disables the private address checks entirely. Only for a fully trusted network. |
| `OPENPR_CONNECTOR_SECRET_W_<WORKSPACE>_*` | unset | Credentials a connector `auth_policy` may reference as `{"mode":"bearer","token_ref":"env:OPENPR_CONNECTOR_SECRET_W_<WORKSPACE>_MYHOOK"}`. `<WORKSPACE>` is the owning workspace UUID with dashes removed and uppercased, and a connector may only reference names inside the namespace of the workspace that owns it: any user can create a workspace and is its admin there, so a process wide namespace would be readable by every tenant. Only this prefix can be referenced, so a connector cannot be pointed at `JWT_SECRET`, `DATABASE_URL`, `POSTGRES_*`, `AWS_*` or `OPENPR_OBJECT_STORAGE_*`. Container deployments must add each variable to the api and worker environment in `docker-compose.yml`. |

**Consumed only by docker-compose**, never by Rust code: `OPENPR_BIND_HOST`
(default `127.0.0.1`), `OPENPR_API_PORT`, `OPENPR_FRONTEND_PORT`,
`MCP_SERVER_PORT`, `POSTGRES_DB`, `POSTGRES_USER`, `POSTGRES_PASSWORD`,
`PGHOST`, `PGPORT`, `PGDATABASE`, `PGUSER`, `PGPASSWORD`, `VITE_API_BASE_URL`,
`OPENPR_WEBHOOK_IMAGE`, `OPENPR_WEBHOOK_CONFIG`, `OPENPR_WEBHOOK_PORT`.

> `.env.example` sets `OPENPR_API_URL=http://api:8080`, the address **inside**
> the compose network. Running the MCP server on the host requires
> `http://localhost:8081`.

## Universal Forms

A form owns a schema, grid and detail views, per-role permissions, records,
links, attachments, and events. Frontend entry point:
`/workspace/<workspace-id>/projects/<project-id>/forms`.

**27 field types**: `text`, `textarea`, `rich_text`, `phone`, `email`,
`address`, `location`, `scan`, `signature`, `autonumber`, `member`, `number`,
`integer`, `amount`, `rating`, `progress`, `date`, `datetime`, `single_select`,
`multi_select`, `boolean`, `attachment`, `image`, `relation`, `child_table`,
`formula`, `ai_summary`. Also: schema versioning with archived versions, field
usage and dependency reports, form duplication, parent/child relations with
child create/update/archive/restore, aggregation, import preview/commit,
background import/export jobs, mapping templates, and attachment package jobs.

### Boundaries

- **Import/export supports CSV and JSON only.** There is no XLSX reader or writer.
- **Attachment media processing is image-only** (`gif`, `jpeg`, `png`, `webp`). Thumbnails and dimensions are derived for images; no EXIF extraction, no video processing. The generic upload endpoint accepts more file types but does not process them.
- **Formulas are a JSON structure, not an expression language.** A formula is `{"op": ..., "args": [...], "scale": n}` with 7 operators — `add`, `sum`, `subtract`, `multiply`, `divide`, `min`, `max`, `count` (`add` and `sum` are the same op) — plus cross-record aggregates `child_sum`, `child_count`, `child_min`, `child_max`. There is no dependency graph; formula fields evaluate in field declaration order.
- **Electronic signatures are handwritten canvas PNGs plus a SHA-256 audit chain**, with optional reason/consent capture and retention-based expiry. Not PKI: no certificate, no asymmetric key, no timestamp authority.
- **Amounts use `rust_decimal` and must be passed as strings.** The API rejects JSON numbers with `field '<key>' must be a decimal string, not a JSON number`.

### Scenario templates

Passing `scenario_template_key` to `POST /api/v1/workspaces/{id}/projects` (or
calling `scenario_templates.install` on an existing project) initializes forms,
views, connector suggestions, and — where the scenario needs local computation —
an active WASM plugin. `restaurant_ordering_default` is the reference end-to-end
example: menu categories, SKUs, tables, orders, order lines, print jobs,
business reports, print/webhook connector suggestions, and an active
`restaurant_calc` plugin.

## WASM Plugins

Per-project WebAssembly modules executed by `wasmtime` 45 under three limits:
**fuel metering** (per-manifest budget, max 1e9), a **memory ceiling** via
`StoreLimits` (1 instance, 1 memory, 1 table), and a **wall-clock timeout**
(per-manifest, max 30000 ms). **Zero host functions** — modules are instantiated
with an empty import list, so there is no WASI, no filesystem, no network, and
no clock; communication happens only through linear memory.

ABI (`docs/plugins/openpr-plugin-v1.wit`): export `memory`,
`openpr_alloc(len: i32) -> i32`, and `openpr_invoke(ptr: i32, len: i32) -> i64`
(packed pointer/length return); optionally `openpr_plugin_abi_version() -> i32`
returning `1`. Input and output are UTF-8 JSON, decimals stay strings.

| Hook              | Effect on the write path                                  |
| ----------------- | --------------------------------------------------------- |
| `field_validator` | Can reject the record write                                |
| `formula`         | Returns a value patch applied to the record               |
| `event_handler`   | Fire-and-forget; failure is recorded but does not block   |

> Plugins do **not** add entries to MCP `tools/list`. A manifest may declare
> `capabilities.tools`, but that list is only stored and used to authorize
> `plugins.invoke`; plugin logic is reached indirectly through the `hook_kind`
> argument of `plugins.invoke`.

## Events, Connectors, and Webhooks

**Transactional outbox/inbox.** Business writes insert into `business_events`
and `event_outbox` in the same transaction, with envelopes versioned
`openpr.event.v1`. The worker leases outbox rows, fans them out into connector
invocations, delivers them, and records receipts back through `event_inbox`.
**Connector tools are therefore asynchronous**: creating an invocation through
MCP or the API returns a ledger entry, not a delivery result. Poll
`invocations.get` or read `events.tail` for the outcome.

**Legacy webhooks** are a separate path from the outbox, signed with HMAC-SHA256
in `X-Webhook-Signature: sha256=<hex>`. 31 event types can be emitted (issue,
comment, label, sprint, proposal, project, member, veto, escalation, appeal,
governance config, AI task), while webhook subscriptions are validated against a
narrower 14-entry allow-list in `apps/api/src/entities/webhook.rs`. **Legacy
webhook delivery has no retry** — `retry_count` is always written as `0`. Use
connectors when delivery must survive a failing endpoint.

## Worker

`apps/worker` is a standalone process with a 5-second poll loop and a
`--concurrency` flag (default `4`). Concurrency is a **batch-size multiplier**,
not a parallelism level: each pipeline fetches `concurrency * N` rows per tick
and awaits them sequentially.

| Pipeline             | Source                                                                 | Behavior                                                      |
| -------------------- | ---------------------------------------------------------------------- | ------------------------------------------------------------- |
| AI task dispatch     | `ai_tasks`                                                             | POSTs to bot webhooks; retry delay `max(attempts, 1) * 30` s  |
| Event outbox fan-out | `event_outbox` → `connector_invocations`                               | Backoff `LEAST(attempts * 30, 300)` s                        |
| Event inbox receipts | `event_inbox`                                                          | Applies delivery receipts back onto invocations              |
| Connector delivery   | `connector_invocations`                                                | HTTP delivery, 10 s client timeout, then status update       |
| Form jobs            | `form_import_jobs`, `form_export_jobs`, `form_attachment_package_jobs` | Plus expiry cleanup of package artifacts and signature values |

Every pipeline picks rows with `SELECT ... FOR UPDATE SKIP LOCKED`, so multiple
worker instances scale horizontally against one database with no extra
coordination.

## MCP Server

Only `--transport http` serves all three surfaces on one port.

| Transport | Command                   | Endpoints                                                |
| --------- | ------------------------- | -------------------------------------------------------- |
| **HTTP**  | `serve --transport http`  | `POST /mcp/rpc`, `GET /sse`, `POST /messages`, `/health` |
| **stdio** | `serve --transport stdio` | stdin/stdout JSON-RPC                                    |
| **SSE**   | `serve --transport sse`   | `GET /sse`, `POST /messages`, `/health` — **no** `/mcp/rpc` |

> **Security — the MCP HTTP/SSE endpoints are unauthenticated.**
>
> `/mcp/rpc`, `/sse`, and `/messages` have **no auth middleware**. The bot token
> is used only on the mcp-server → API hop, so anyone who can reach the MCP port
> gets full workspace access under the server's bot identity. This is why
> `docker-compose.yml` binds the port to `${OPENPR_BIND_HOST:-127.0.0.1}`. Never
> expose the MCP port to a network without your own authenticating proxy.

### Bot tokens

MCP authenticates to the API with **bot tokens** (prefix `opr_`), managed under
**Workspace → Members → Bot Tokens**. A token has a display name shown in
activity feeds, is scoped to one workspace, creates a `bot_mcp` user entity for
audit-trail integrity, and can perform any read/write a workspace member can.

### Client configuration — stdio (Claude Desktop / Cursor / Codex)

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

HTTP — plain JSON-RPC; passing `params.project_id` returns the
project-capability-filtered tool set. SSE — open the stream, POST to the session
endpoint it returns, and the response arrives back on the stream as
`event: message`.

```bash
curl -X POST http://localhost:8090/mcp/rpc -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
curl -X POST http://localhost:8090/mcp/rpc -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"project_id":"<project-uuid>"}}'

curl -N -H "Accept: text/event-stream" http://localhost:8090/sse
# → event: endpoint / data: /messages?session_id=<uuid>
curl -X POST "http://localhost:8090/messages?session_id=<uuid>" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"projects.list","arguments":{}}}'
```

### Tools (105)

Per-domain counts; the total is pinned by an `assert_eq!(tools.len(), 105)` test
in `apps/mcp-server/src/tools/mod.rs`.

| Domain                    | Count | Representative tools                                                      |
| ------------------------- | ----: | ------------------------------------------------------------------------- |
| Universal forms & events  |    34 | `forms.create`, `forms.update_schema`, `form_records.create`, `events.tail` |
| Work items                |    11 | `work_items.create`, `work_items.get_by_identifier`, `work_items.search`   |
| Scenario tools            |     9 | `code.change_proposal.create`, `documents.review_risk`, `approval.request` |
| Connectors & invocations  |     8 | `connectors.list`, `invocations.create`, `invocations.complete`            |
| Project types & resources |     6 | `project_types.get`, `project_resources.create`                            |
| Projects                  |     5 | `projects.list`, `projects.create`                                         |
| Labels                    |     5 | `labels.create`, `labels.list_by_project`                                  |
| Plugins                   |     5 | `plugins.install`, `plugins.invoke`, `plugin_invocations.list`             |
| Proposals & check results |     5 | `proposals.create`, `check_results.create`                                 |
| Sprints                   |     4 | `sprints.create`, `sprints.update`                                         |
| Comments                  |     3 | `comments.create`, `comments.list`                                         |
| Context                   |     3 | `context.get_project`, `context.get_agent_policy`                          |
| Scenario templates        |     3 | `scenario_templates.list`, `scenario_templates.install`                    |
| Single-tool domains       |   4×1 | `files.upload`, `members.list`, `search.all`, `release.readiness.get`      |

The full list with exact parameter schemas is generated from the code — do not
transcribe it. Get it with `cargo run --bin list-tools` (no running API needed)
or a `tools/list` JSON-RPC call.

### Resources

Four static resources — `openpr://skills/openpr-mcp`, `openpr://guides/agents`,
`openpr://guides/workflows`, `openpr://scenario-templates` — plus 19 resource
templates via `resources/templates/list`, including
`openpr://projects/{project_id}/forms`, `openpr://forms/{form_id}/records`,
`openpr://form-records/{record_id}/events`,
`openpr://projects/{project_id}/context`,
`openpr://projects/{project_id}/release-readiness`, and
`openpr://issues/{identifier}`.

### The same binary is a CLI

Besides `serve`, `mcp-server` exposes 8 command groups: `projects`,
`work-items`, `comments`, `labels`, `sprints`, `search`, `files upload`, and
`tools call`. The global `--format json|table` selects the output shape, and
`tools call` reaches any of the 105 tools by name — a complete escape hatch for
anything without a dedicated subcommand.

```bash
mcp-server projects list --format table
mcp-server work-items create --project <uuid> --title "Fix login" --priority high
mcp-server files upload --file ./report.pdf
mcp-server tools call --name forms.list --args-json '{"project_id":"<uuid>"}'
```

## API

301 method+path endpoints (214 `.route()` calls), all registered in
`apps/api/src/main.rs`. Every route lives under `/api/v1/`, plus an unversioned
`/health`.

Prefixes, all relative to `/api/v1`: auth and admin (`/auth/*`, `/admin/*`,
`/users/*`, `/my/*`); core PM (`/workspaces/*`, `/projects/*`, `/issues/*`,
`/comments/*`, `/sprints/*`, `/labels/*`); forms (`/forms/*`,
`/form-records/*`, `/form-views/*`, `/form-attachments/*`, `/form-*-jobs/*`);
plugins and events (`/plugins/*`, `/invocations/*`, `/check-results/*`);
governance (`/proposals/*`, `/decisions/*`, `/governance/*`, `/trust-scores/*`,
`/impact-reviews/*`, `/vetoers/*`); AI (`/ai/*`, `/ai-participants/*`,
`/ai-learning/*`); templates (`/scenario-templates/*`, `/project-types/*`,
`/proposal-templates/*`); files and misc (`/upload`, `/uploads/*`, `/search`,
`/export/*`, `/notifications/*`, `/workflows/*`).

Search (`/api/v1/search`, MCP `search.all`) matches issues, comments, and
proposals with case-insensitive substring matching.

Responses are `{"code": 0, "message": "success", "data": {...}}` on success and
`{"code": 400, "message": "error description"}` on failure.

## Frontend

SvelteKit 2.50 on Svelte 5 with Tailwind 4, built with **Bun**. The adapter is
`@sveltejs/adapter-static` with `fallback: index.html` — the app ships as a pure
SPA served by nginx with same-origin API proxying in the production image. i18n
is a minimal in-repo store aliased to `svelte-i18n`
(`frontend/src/lib/i18n/svelte-i18n.ts`); `en.json` and `zh.json` each carry
2199 keys.

## Scripts

All under `scripts/`.

| Group       | Scripts                                                                   |
| ----------- | ------------------------------------------------------------------------- |
| Lifecycle   | `start.sh` (first-run `.env` + random secrets, build release binaries, `compose up -d`), `dev-up.sh` (PostgreSQL only, for host-side Rust), `stop.sh`, `clean.sh` (**tears down volumes** — destroys database data, asks to confirm) |
| Database    | `init-db.sh` (apply migrations in order), `backup-db.sh` (gzipped dump into `backups/`), `restore-db.sh`                                                                                                                              |
| Verification | `e2e-test.sh` (one-shot end-to-end with automatic teardown), `test-api.sh`, `test-mcp.sh` (asserts the 105 tool count), `verify.sh` (component health check)                                                                           |
| Development | `dev-check.sh` (`cargo fmt --check`, `check`, `clippy -D warnings`, `test`), `ci-universal-forms-gates.sh` (reproduce the CI-only `Universal Forms Gates` bundle locally)                                                              |
| Demo data   | `bootstrap-restaurant-demo.sh`, `smoke-restaurant-ordering.sh`                                                                                                                                                                        |
| Other       | `benchmark.sh` (API latency/throughput), `bump-version.sh` (`major\|minor\|patch`, syncs `Cargo.toml` and `frontend/package.json`)                                                                                                     |

> The remaining ~80 `scripts/*universal-forms*` files are historical delivery
> acceptance and signoff scripts, kept for audit traceability. They are not part
> of the normal build or test flow.

## Testing

**Rust unit tests — 308 total** (`api` 251, `mcp-server` 23, `worker` 3,
`platform` 31), run with `cargo test --workspace`.

**Playwright E2E — 8 specs**, all covering universal forms, in `tests/e2e/web/`:
field design save, human flow, import/export, interaction/IA, mobile + dark
mode, permissions, record CRUD, record detail edit/delete. `playwright.config.ts`
hard-codes an internal `baseURL` of `http://10.72.0.3:3000`; override it with
`BASE_URL=http://localhost:3000 npx playwright test`.

**Frontend smoke scripts — 7 `.mjs` scripts** in `frontend/scripts/`, run via
`bun run smoke:*` (`smoke:mcp-admin`, `smoke:connections`,
`smoke:phase1-project-types`, `smoke:project-template`,
`smoke:template-work-items`, `smoke:forms-ui`, `smoke:restaurant-ordering`).

**CI.** The `universal-forms` job is not a conventional test suite — it is in-repo
shell audits that grep and pattern-match the source tree to assert that claimed
delivery surfaces still have concrete entrypoints. Behavior is covered by
`cargo test` and the smoke/E2E scripts; treat the two as separate signals.

## Documentation

- `docs/universal-forms-and-plugins.md` — form data model, plugin behavior, connector flow
- `docs/scenario-templates.md` — built-in scenario template catalog
- `docs/universal-forms-production.md` — production runbook
- `docs/universal-forms-implementation-map.md` — source module / verification command map
- `docs/plugins/openpr-plugin-v1.wit` — plugin ABI v1
- `apps/mcp-server/AGENTS.md` — coding-agent workflow patterns and tool examples
- `skills/openpr-mcp/SKILL.md` — governed MCP skill package

## Tech Stack

- **Backend**: Rust edition 2024, axum 0.8, SeaORM 1 (`sqlx-postgres`, rustls), `rust_decimal`, wasmtime 45, PostgreSQL 16
- **Frontend**: SvelteKit 2.50, Svelte 5, Tailwind 4, `@sveltejs/adapter-static`, Bun
- **MCP**: JSON-RPC 2.0 over HTTP, stdio, and SSE
- **Auth**: JWT access + refresh, bot tokens (`opr_`)
- **Deployment**: Docker Compose / Podman, nginx

## Related Projects

| Repository                                                  | Description                                     |
| ----------------------------------------------------------- | ----------------------------------------------- |
| [openpr](https://github.com/openprx/openpr)                 | Core platform (this repo)                       |
| [openpr-webhook](https://github.com/openprx/openpr-webhook) | Webhook receiver for external integrations      |
| [prx](https://github.com/openprx/prx)                       | AI assistant framework with built-in OpenPR MCP |
| [prx-memory](https://github.com/openprx/prx-memory)         | Local-first MCP memory for coding agents        |
| [wacli](https://github.com/openprx/wacli)                   | WhatsApp CLI with JSON-RPC daemon               |

## Links

[Documentation](https://docs.openprx.dev/en/openpr/) ·
[Community](https://community.openprx.dev) · [OpenPRX](https://openprx.dev)

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your
option. `Cargo.toml` declares `license = "MIT OR Apache-2.0"`.
