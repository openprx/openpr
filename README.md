# OpenPR

Open-source project management platform with built-in governance, a universal
business-form engine, WASM plugins, and a first-class MCP server for AI agents.
Built with **Rust** (Axum + SeaORM), **SvelteKit**, and **PostgreSQL 16**.

## What It Provides

- **Project management** — workspaces, projects, issues, kanban board, sprints, labels, comments, activity feed, notifications, attachments.
- **Governance** — proposals, weighted voting, decision records, veto and escalation, trust scores, appeals, impact reviews, audit logs.
- **Universal forms** — project-defined business data types with grid/detail views, decimal-safe amounts, record links and child tables, formulas, per-role permissions, import/export, electronic signatures.
- **WASM plugins** — per-project sandboxed plugins for field validation, formulas, and event handlers.
- **Events** — transactional business-event ledger and HMAC-signed webhooks.
- **MCP server** — 98 tools, 4 static resources, 19 resource templates, 3 transports; the same binary is also a CLI.
- **Scenario templates** — 6 ready-to-start setups: `code_delivery_default`, `contract_review_default`, `equipment_maintenance_default`, `quality_corrective_action_default`, `customer_delivery_default`, `restaurant_ordering_default`.

## Architecture

| Component    | Path              | Role                                                                 |
| ------------ | ----------------- | -------------------------------------------------------------------- |
| `api`        | `apps/api`        | HTTP API, form engine, plugin runtime, event emission                 |
| `worker`     | `apps/worker`     | Background pipelines: AI tasks, operation-log retention, form jobs   |
| `mcp-server` | `apps/mcp-server` | MCP server (HTTP/stdio/SSE) and CLI over the API                     |
| `frontend`   | `frontend`        | SvelteKit 2 SPA (adapter-static)                                     |

`crates/platform` holds shared config, DB connection, auth, error, and logging.
`migrations/` holds the ordered SQL schema history through `0052`.

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
| `[audit]` | worker | `operation_log_retention_days` (`30`, range `1..=3650`) |
| `[migrations]` | api | `replay` (`false`), `continue_on_error` (`false`) — both are escape hatches; turn one on deliberately, then turn it back off |
| `[outbound]` | api, worker | `allowed_hosts` — a TOML **array of strings** (`[]`), `allow_private` (`false`) |
| `[mcp]` | mcp-server | `api_url` (`http://localhost:8081`), `bot_token` (`opr_` prefix; **required** for `stdio` and the CLI subcommands, unused by `http`/`sse`), `workspace_id` (**required**, real non-nil UUID), `transport` — `stdio` \| `http` \| `sse` (`stdio`), `bind_addr` (`127.0.0.1:8090`) |

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

**Outbound deliveries (api + worker).** Webhook endpoints are
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

## Events and Webhooks

**Transactional event ledger.** Business writes insert into `business_events` and `event_outbox` in the same transaction, with envelopes versioned `openpr.event.v1`. Consumers may use `events.tail` to read the resulting event stream.


**Legacy webhooks** are a separate path from the outbox, signed with HMAC-SHA256
in `X-Webhook-Signature: sha256=<hex>`. 31 event types can be emitted (issue,
comment, label, sprint, proposal, project, member, veto, escalation, appeal,
governance config, AI task), while webhook subscriptions are validated against a
narrower 14-entry allow-list in `apps/api/src/entities/webhook.rs`. **Legacy
webhook delivery has no retry** — `retry_count` is always written as `0`.

## Worker

`apps/worker` is a standalone process with a 5-second poll loop and a
`--concurrency` flag (default `4`). Concurrency is a **batch-size multiplier**,
not a parallelism level: each pipeline fetches `concurrency * N` rows per tick
and awaits them sequentially.

| Pipeline             | Source                                                                 | Behavior                                                      |
| -------------------- | ---------------------------------------------------------------------- | ------------------------------------------------------------- |
| AI task dispatch     | `ai_tasks`                                                             | POSTs to bot webhooks; retry delay `max(attempts, 1) * 30` s  |
| Operation-log cleanup | `bot_operation_logs`                                                  | Deletes metadata records older than `[audit]` retention       |
| Form jobs            | `form_import_jobs`, `form_export_jobs`, `form_attachment_package_jobs` | Plus expiry cleanup of package artifacts and signature values |

Queue-backed pipelines pick rows with `SELECT ... FOR UPDATE SKIP LOCKED`, so
multiple worker instances can share one database safely.

## MCP Server

Only `--transport http` serves all three surfaces on one port.

| Transport | Command                   | Endpoints                                                |
| --------- | ------------------------- | -------------------------------------------------------- |
| **HTTP**  | `serve --transport http`  | `POST /mcp/rpc`, `GET /sse`, `POST /messages`, `/health` |
| **stdio** | `serve --transport stdio` | stdin/stdout JSON-RPC                                    |
| **SSE**   | `serve --transport sse`   | `GET /sse`, `POST /messages`, `/health` — **no** `/mcp/rpc` |

> **Security — an MCP HTTP/SSE request is made as its own caller.**
>
> `/mcp/rpc`, `/sse` and `/messages` require `Authorization: Bearer <opr_ bot
> token>` and reject everything else with 401; `/health` is exempt so
> healthchecks keep working. There is no shared inbound secret and no
> configuration that relaxes this: the server holds no identity of its own on
> these transports, forwards the token the request presented to the API
> unchanged, and lets the API authenticate it. Every call is therefore made by a
> named bot, and the audit trail records that bot rather than the server.
>
> Because an unauthenticated caller can reach nothing but `/health`, binding a
> reachable address publishes no anonymous surface — which is what lets the
> compose container bind `0.0.0.0:8090` with no secret in its configuration file
> at all. `mcp.auth_token` was the old shared secret and **has been removed**: a
> configuration file that still carries the key is refused at startup, so delete
> the line rather than leaving it for later.
>
> `mcp.bot_token` still names the identity for `stdio` and for the CLI
> subcommands, which have no per-request header to read one from. It is unused
> by `http` and `sse`.

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

### Client configuration — HTTP/SSE

One shared server, many callers: the token belongs to the client, not to the
server, so each client puts **its own** bot token in the header.

```json
{
  "mcpServers": {
    "openpr": {
      "type": "http",
      "url": "http://localhost:8090/mcp/rpc",
      "headers": { "Authorization": "Bearer opr_your_own_workspace_bot_token" }
    }
  }
}
```

HTTP — plain JSON-RPC; passing `params.project_id` returns the
project-capability-filtered tool set. SSE — open the stream, POST to the session
endpoint it returns, and the response arrives back on the stream as
`event: message`.

```bash
# The Authorization header is mandatory on every one of these; only /health is exempt.
# This shell variable is a convenience for curl, not application configuration: the value
# is your own workspace bot token, created under Workspace → Members → Bot Tokens. The
# server forwards it to the API unchanged and the call is made as that bot.
export OPENPR_MCP_BOT_TOKEN=opr_your_own_workspace_bot_token

curl -X POST http://localhost:8090/mcp/rpc -H "Content-Type: application/json" \
  -H "Authorization: Bearer $OPENPR_MCP_BOT_TOKEN" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
curl -X POST http://localhost:8090/mcp/rpc -H "Content-Type: application/json" \
  -H "Authorization: Bearer $OPENPR_MCP_BOT_TOKEN" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"project_id":"<project-uuid>"}}'

curl -N -H "Accept: text/event-stream" \
  -H "Authorization: Bearer $OPENPR_MCP_BOT_TOKEN" http://localhost:8090/sse
# → event: endpoint / data: /messages?session_id=<uuid>
curl -X POST "http://localhost:8090/messages?session_id=<uuid>" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $OPENPR_MCP_BOT_TOKEN" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"projects.list","arguments":{}}}'
```

### Tools (98)

Per-domain counts; the total is pinned by an `assert_eq!(tools.len(), 98)` test
in `apps/mcp-server/src/tools/mod.rs`.

| Domain                    | Count | Representative tools                                                      |
| ------------------------- | ----: | ------------------------------------------------------------------------- |
| Universal forms & events  |    34 | `forms.create`, `forms.update_schema`, `form_records.create`, `events.tail` |
| Work items                |    11 | `work_items.create`, `work_items.get_by_identifier`, `work_items.search`   |
| Scenario tools            |     9 | `code.change_proposal.create`, `documents.review_risk`, `approval.request` |
| Project types & resources |     6 | `project_types.get`, `project_resources.create`                            |
| Projects                  |     5 | `projects.list`, `projects.create`                                         |
| Labels                    |     5 | `labels.create`, `labels.list_by_project`                                  |
| Plugins                   |     5 | `plugins.install`, `plugins.invoke`, `plugin_invocations.list`             |
| Proposals & check results |     5 | `proposals.create`, `check_results.create`                                 |
| Sprints                   |     4 | `sprints.create`, `sprints.update`                                         |
| Comments                  |     3 | `comments.create`, `comments.list`                                         |
| Context                   |     3 | `context.get_project`, `context.get_agent_policy`                          |
| Scenario templates        |     3 | `scenario_templates.list`, `scenario_templates.install`                    |
| Operation records         |     1 | `bot_operation_logs.list`                                                  |
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

Besides `serve`, `mcp-server` exposes 9 command groups: `projects`,
`work-items`, `comments`, `labels`, `sprints`, `search`, `files upload`,
`operation-logs list`, and `tools call`. The global `--format json|table` selects the output shape, and
`tools call` reaches any of the 98 tools by name — a complete escape hatch for
anything without a dedicated subcommand.

```bash
mcp-server projects list --format table
mcp-server work-items create --project <uuid> --title "Fix login" --priority high
mcp-server files upload --file ./report.pdf
mcp-server operation-logs list --outcome error --limit 50
mcp-server tools call --name forms.list --args-json '{"project_id":"<uuid>"}'
```

## API

302 method+path endpoints (215 `.route()` calls), all registered in
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
| Verification | `e2e-test.sh` (one-shot end-to-end with automatic teardown), `test-api.sh`, `test-mcp.sh` (asserts the 98 tool count), `verify.sh` (component health check)                                                                           |
| Development | `dev-check.sh` (`cargo fmt --check`, `check`, `clippy -D warnings`, `test`), `ci-universal-forms-gates.sh` (reproduce the CI-only `Universal Forms Gates` bundle locally)                                                              |
| Demo data   | `bootstrap-restaurant-demo.sh`, `bun --cwd frontend run smoke:restaurant-ordering`                                                                                                                                                   |
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

**Frontend smoke scripts — 6 `.mjs` scripts** in `frontend/scripts/`, run via
`bun run smoke:*` (`smoke:connections`,
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
