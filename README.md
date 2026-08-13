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

`scripts/start.sh` generates the deployment's configuration on first run —
`config/openpr.compose.toml` for the API and the worker,
`config/openpr.compose.mcp.toml` for the MCP server, plus a compose-only `.env`
— filling in random non-production bootstrap secrets without echoing any of them
to the terminal. It then builds release binaries for `Dockerfile.prebuilt` and
runs `docker compose up -d --build`. The generated files are `chmod 600` and
hold real secrets: replace them before production use, and never commit them.
Services publish on `${OPENPR_BIND_HOST:-127.0.0.1}`: frontend `:3000`, API
`:8081`, MCP `:8090`.
For demo data once healthy, `scripts/bootstrap-restaurant-demo.sh` creates a
demo account, workspace, `restaurant_ordering_default` project with sample
records, and a workspace-scoped bot token; it refuses non-local API URLs unless
`OPENPR_DEMO_ALLOW_REMOTE=1`.

### Local development

```bash
# Prerequisites: stable Rust with edition 2024 support, Bun, PostgreSQL 16
scripts/dev-up.sh                                  # start only PostgreSQL from compose
cp config/openpr.example.toml config/openpr.toml
$EDITOR config/openpr.toml                         # database.url, auth.jwt_secret, [mcp]

cargo run --bin api -- --config config/openpr.toml     # listens on server.bind_addr, default 0.0.0.0:8081
cargo run --bin worker -- --config config/openpr.toml

cd frontend && bun install && bun run dev

cargo run --bin mcp-server -- serve --config config/openpr.toml --transport http
```

> A host-side run reaches PostgreSQL through the published port, so
> `database.url` must name `localhost`, not the compose hostname `postgres`.
> `mcp.api_url` follows the same rule: `http://localhost:8081` from the host,
> the `api` service address from inside the compose network.
>
> `--config` is optional; every binary falls back to `config/openpr.toml`
> relative to its working directory, which is what these commands would use
> anyway when run from the repository root.

## Configuration

**One TOML file, no environment variables.** `api`, `worker` and `mcp-server`
read every setting from a single configuration file and **no environment
variable at all**. The path comes from `--config <PATH>`, defaulting to
`config/openpr.toml` relative to the process working directory. A missing file
is a startup error, never a silent fallback: the binaries never invent a
database URL or a signing key. `config/openpr.example.toml` is the annotated
reference — copy it to `config/openpr.toml` and edit it.

Unknown keys are rejected, so a misspelled setting fails startup instead of
being silently ignored.

| Section | Read by | Keys (defaults in parentheses) |
| --- | --- | --- |
| `[server]` | api, worker | `app_name`, `bind_addr`. Both optional; each binary keeps its own default when they are omitted (api listens on `0.0.0.0:8081`). |
| `[database]` | api, worker | `url` (**required**; full URL, password included, never logged), `max_connections` (`20`), `min_connections` (`2`), `connect_timeout_seconds` (`5`), `idle_timeout_seconds` (`30`), `acquire_timeout_seconds` (`5`) |
| `[auth]` | api, worker | `jwt_secret` (**required**; minimum 16 characters, 64 hex recommended — `openssl rand -hex 32`), `access_ttl_seconds` (`1296000`), `refresh_ttl_seconds` (`1728000`), `default_author_id` (optional, must be a real non-nil UUID) |
| `[logging]` | all | `filter` — `tracing` directives, validated at startup (`<service>=info,tower_http=info`), `format` — `json` \| `text` (`json`), `output` — `stderr` \| `stdout` (`stderr`) |
| `[storage]` | api | `backend` — `local` \| `s3` (`local`), `dir` (`./uploads`); `[storage.s3]` with `endpoint`, `bucket`, `region` (`us-east-1`), `access_key_id`, `secret_access_key`, `session_token`, required only when `backend = "s3"` and left unread otherwise |
| `[migrations]` | api | `replay` (`false`), `continue_on_error` (`false`) — both are escape hatches; turn one on deliberately, then turn it back off |
| `[outbound]` | api, worker | `allowed_hosts` — a TOML **array of strings** (`[]`), `allow_private` (`false`) |
| `[mcp]` | mcp-server | `api_url` (`http://localhost:8081`), `bot_token` (**required**, `opr_` prefix), `workspace_id` (**required**, real non-nil UUID), `auth_token` (inbound bearer token, minimum 16 characters), `transport` — `stdio` \| `http` \| `sse` (`stdio`), `bind_addr` (`127.0.0.1:8090`), `invocation_id` (optional ledger correlation id) |
| `[connectors.secrets."<workspace-uuid>"]` | api, worker | One table per workspace, `NAME = "value"` |

> **Eager shape, lazy presence.** Every value the file *does* contain is
> shape-checked at startup by whichever binary reads it, but whether a mandatory
> value is *present* is decided by the binary that needs it. A deployment that
> runs only the MCP server therefore needs no `[database]` and no `[auth]`
> section at all, and is never asked to invent two credentials it never uses.
> When api or worker is missing them, validation reports every missing or
> unusable value in **one** error instead of one restart per mistake.

A complete MCP-only configuration is three lines:

```toml
[mcp]
bot_token = "opr_..."
workspace_id = "..."
```

**Logging** replaces `RUST_LOG`: the level is part of the deployment's
configuration, not of whatever the surrounding shell happened to export. The MCP
server's stdio transport frames JSON-RPC on stdout, where one log line ends the
session, so it always logs to stderr and reports `output = "stdout"` as
overridden rather than honouring it.

**Outbound deliveries (api + worker).** Connector and webhook endpoints are
validated when they are configured and again before every delivery: an endpoint
whose host resolves to a loopback, private, link-local, NAT64/6to4 or otherwise
internal address is refused, and redirects are not followed.
`outbound.allowed_hosts` lists the exemptions as `"host"` or `"host:port"`,
matched literally and case-insensitively — no wildcards, no URLs, no paths, all
three are rejected at startup because they would silently never match. Internal
receivers (compose services, in-cluster bots) must be listed there or their
deliveries are refused. `outbound.allow_private = true` disables the checks
entirely and is only for a closed network you control end to end.

```toml
[outbound]
allowed_hosts = ["webhook:9090", "api:8080", "mcp-server:8090", "frontend:80"]
allow_private = false
```

**Connector credentials.** Anything a connector `auth_policy` presents to a
third party is filed under the UUID of the workspace that owns the connector:

```toml
[connectors.secrets."0f8a1b2c-3d4e-4f60-8182-93a4b5c6d7e8"]
SHIPPING = "..."
```

and referenced by its short name — `"auth_policy": {"mode":"hmac","secret_ref":"SHIPPING"}`.
The lookup selects the workspace table first and searches only inside it, so no
reference, however it is spelled, can reach another tenant's credentials: any
user can create a workspace and is its admin there, so a process-wide namespace
would be readable by every tenant. Names must match `[A-Z_][A-Z0-9_]*`, must not
start with `OPENPR_`, `POSTGRES_`, `PG` or `AWS_`, and must not be `JWT_SECRET`,
`DATABASE_URL` or `RUST_LOG`.

> The retired `OPENPR_CONNECTOR_SECRET_W_<UUID>_<NAME>` environment variables map
> onto this table: drop the whole prefix and file the remaining name under the
> workspace UUID. `secret_ref` / `token_ref` now carry the bare name, **not** the
> old `env:...` form.

### Compose deployment

`scripts/start.sh` generates two configuration files and `docker-compose.yml`
mounts each **read-only** at `/app/config/openpr.toml` inside its containers,
with every service passing `--config /app/config/openpr.toml`:

| File | Used by | Sections |
| --- | --- | --- |
| `config/openpr.compose.toml` | api, worker | `[database]`, `[auth]`, `[logging]`, `[storage]`, `[migrations]`, `[outbound]`, `[connectors]` |
| `config/openpr.compose.mcp.toml` | mcp-server | `[logging]`, `[mcp]` |

The split follows the trust boundary. api and worker are one trust domain — same
database, same signing key, and they must not drift apart. The MCP server is the
network-exposed, agent-facing surface, and it needs neither the database URL nor
the JWT signing key; handing it the signing key would turn a compromise of the
MCP server into the ability to forge any user's token. The lazy
`[database]`/`[auth]` presence check above is what makes an MCP config file
without those sections a valid one.

Both generated files are `chmod 600` and hold real secrets. **Do not commit
them.**

Host-side helper scripts — `scripts/test-mcp.sh`,
`scripts/bootstrap-restaurant-demo.sh` and
`skills/openpr-mcp/scripts/validate-mcp.sh` — read `mcp.auth_token` out of
`config/openpr.compose.mcp.toml`; point them at another file with
`OPENPR_CONFIG_FILE`, or override the token for the script itself by exporting
`OPENPR_MCP_AUTH_TOKEN`.

**Consumed only by docker-compose**, never read by the Rust binaries:
`POSTGRES_PASSWORD` (the `postgres:16` image initialises itself from it, so it
must match the password inside `database.url`), `OPENPR_BIND_HOST` (default
`127.0.0.1`), `OPENPR_API_PORT`, `MCP_SERVER_PORT`, `OPENPR_FRONTEND_PORT`,
`OPENPR_WEBHOOK_PORT`, `OPENPR_RUNTIME_BASE`, `OPENPR_WEBHOOK_IMAGE`,
`OPENPR_WEBHOOK_CONFIG`, and `VITE_API_BASE_URL` (frontend build). These live in
`.env`, which exists purely for compose's `${...}` interpolation.

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

> **Security — the MCP HTTP/SSE endpoints are guarded by `mcp.auth_token`.**
>
> When `mcp.auth_token` is set (minimum 16 characters), `/mcp/rpc`, `/sse` and
> `/messages` require `Authorization: Bearer <token>` and reject everything
> else; `/health` is exempt so healthchecks keep working. The check is
> fail-closed: if the bind address is not a loopback address and no token is
> configured, the server **refuses to start** rather than serving an open port.
> A loopback bind without a token stays open to anything local, which is why
> `docker-compose.yml` still publishes the port on
> `${OPENPR_BIND_HOST:-127.0.0.1}` and requires the token for the container's
> `0.0.0.0:8090` bind.
>
> The token is a single shared secret, not a per-caller identity: everyone
> holding it acts under the server's bot token, with full access to its
> workspace. Treat it as one trust boundary and put your own authenticating
> proxy in front when different callers need different rights.

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
      "args": ["serve", "--config", "/absolute/path/to/config/openpr.toml"]
    }
  }
}
```

No `env` block: the binary reads no environment variables, so `mcp.api_url`,
`mcp.bot_token` and `mcp.workspace_id` come from the file the `--config` path
names. An absolute path is what makes this work — the default
`config/openpr.toml` is relative to whatever working directory the MCP client
happens to launch the process in.

> `--api-url`, `--bot-token`, `--workspace-id`, `--transport` and `--bind-addr`
> exist as command-line overrides and win over the file. Prefer the file for
> anything secret: a token in `argv` is readable by any local process through
> `/proc`.

HTTP — plain JSON-RPC; passing `params.project_id` returns the
project-capability-filtered tool set. SSE — open the stream, POST to the session
endpoint it returns, and the response arrives back on the stream as
`event: message`.

```bash
# The Authorization header is required whenever the server was started with a token.
# This shell variable is a convenience for curl, not application configuration: the
# value is whatever `mcp.auth_token` says in the server's configuration file (under
# compose, config/openpr.compose.mcp.toml).
export OPENPR_MCP_AUTH_TOKEN=your_mcp_auth_token

curl -X POST http://localhost:8090/mcp/rpc -H "Content-Type: application/json" \
  -H "Authorization: Bearer $OPENPR_MCP_AUTH_TOKEN" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
curl -X POST http://localhost:8090/mcp/rpc -H "Content-Type: application/json" \
  -H "Authorization: Bearer $OPENPR_MCP_AUTH_TOKEN" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"project_id":"<project-uuid>"}}'

curl -N -H "Accept: text/event-stream" \
  -H "Authorization: Bearer $OPENPR_MCP_AUTH_TOKEN" http://localhost:8090/sse
# → event: endpoint / data: /messages?session_id=<uuid>
curl -X POST "http://localhost:8090/messages?session_id=<uuid>" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $OPENPR_MCP_AUTH_TOKEN" \
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
| Lifecycle   | `start.sh` (first-run `config/openpr.compose.toml` + `config/openpr.compose.mcp.toml` + compose `.env`, random bootstrap secrets, build release binaries, `compose up -d`), `dev-up.sh` (PostgreSQL only, for host-side Rust), `stop.sh`, `clean.sh` (**tears down volumes** — destroys database data, asks to confirm) |
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
