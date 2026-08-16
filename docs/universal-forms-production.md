# Universal Forms Production Runbook

This runbook is for running OpenPR as a generic open-source business platform,
not only as a project management tool. It assumes PostgreSQL is the production
database and uses the restaurant ordering template as the first acceptance
scenario.

## Production Shape

Minimum production services:

- PostgreSQL 15 or newer.
- OpenPR API.
- OpenPR worker.
- OpenPR frontend.
- OpenPR MCP server if AI assistants or external MCP clients are enabled.
- Optional connector receivers such as webhook, print, device, REST, CLI, or tunnel gateways.

Universal forms production features require these subsystems to be running
together:

```text
API writes
  -> project_forms / form_records / form_record_links
  -> business_events
  -> event_outbox
  -> worker delivery
  -> connector invocation / receipt
  -> event_inbox
  -> MCP/API/frontend reads
```

If the worker is not running, records can still be created, but connector
delivery, print jobs, retries, and receipt handling are not production-ready.

## Preflight

Before exposing a deployment to users, run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo machete
cargo audit
scripts/audit-universal-forms-security-scope.sh
cargo test --workspace --all-features
cargo build --workspace --release
cd frontend && bun run check && bun run build
```

Then run the universal business gates:

```bash
scripts/audit-universal-forms-source-coverage.sh
scripts/audit-universal-forms-security-scope.sh
scripts/audit-universal-forms-production-readiness.sh
scripts/ci-universal-forms-gates.sh
scripts/acceptance-universal-forms.sh --full \
  --output /opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md
scripts/collect-universal-forms-ui-artifacts.sh
scripts/report-universal-forms-completion-audit.sh
scripts/prepare-universal-forms-manual-evidence-map.sh
scripts/report-universal-forms-signoff-status.sh \
  --output /opt/worker/report/openpr/docs/openpr-universal-form-signoff-status-2026-05-31.md
scripts/report-universal-forms-signoff-status-json.sh
scripts/verify-universal-forms-signoff-status-json.sh
scripts/smoke-universal-forms-signoff-status-json-contract.sh
scripts/smoke-universal-forms-signoff-status-output.sh
scripts/verify-universal-forms-next-signoff-review.sh
scripts/smoke-universal-forms-next-signoff-review-contract.sh
scripts/smoke-universal-forms-next-signoff-command.sh
scripts/smoke-universal-forms-manual-signoff-progression.sh
scripts/smoke-universal-forms-manual-signoff-commands.sh
scripts/smoke-universal-forms-signoff-status-output.sh
scripts/status-universal-forms-delivery.sh
scripts/verify-universal-forms-delivery-status-json.sh
scripts/smoke-universal-forms-delivery-status-json-contract.sh
scripts/smoke-universal-forms-delivery-status-output.sh
scripts/prepare-universal-forms-user-acceptance-packet.sh
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
scripts/prepare-universal-forms-delivery-manifest.sh
scripts/verify-universal-forms-delivery-manifest.sh
scripts/report-universal-forms-delivery-manifest-json.sh
scripts/verify-universal-forms-delivery-manifest-json.sh
scripts/smoke-universal-forms-delivery-manifest-json-contract.sh
scripts/smoke-universal-forms-report-output-boundaries.sh
scripts/audit-universal-forms-docs.sh
scripts/audit-universal-forms-delivery-state.sh
scripts/audit-universal-forms-delivery-bundle.sh
```

Production automation should prefer
`openpr-universal-form-readiness-2026-05-31.json` for release state. Its
`manual_signoff.next_row` value carries the next reviewer key, suggested
evidence note, and recorder command template while final acceptance is still
waiting on user-side signoff. The report also includes `schema_path`, which
points to `docs/schemas/openpr-universal-forms-readiness.schema.json`, and
`reports.signoff_status_json`, which points to the dedicated machine-readable
signoff progress report. The verifier checks required fields, extra top-level keys and nested keys,
grouped fields, the signoff status JSON cross-link, and the seven allowed manual
signoff keys before release automation consumes it. It also checks typed counters and booleans,
status enums, release requirement constants, and object shape drift.

Automation that only needs reviewer signoff progress can consume
`openpr-universal-form-signoff-status-2026-05-31.json`. Its schema path is
`docs/schemas/openpr-universal-forms-signoff-status.schema.json`, and its
`manual_signoff` object records completed/pending row counts, blocked rows,
`final_signoff_allowed`, and the next recorder command. Each manual row and
`next_row` also include the automated evidence and reviewer check text from the
manual evidence map, so MCP, CLI, webhook, CI, and release automation can render
reviewer instructions without scraping Markdown. Its `pending_queue` array lists
the remaining rows in review order with `review_order`, `is_next`, `actionable`,
evidence text, reviewer check text, and recorder command. Its `next_row.key` is constrained to the seven manual signoff keys, or an empty string after final signoff, so MCP, CLI, webhook, and CI consumers get a stable completed/pending row contract without scraping Markdown.

The security scope audit is the production guard for the PostgreSQL-only
delivery boundary. It runs `cargo audit` with the repository policy, verifies
that the documented `RUSTSEC-2023-0071` ignore remains limited to SQLx's
inactive MySQL backend scope, confirms the workspace has no active `rsa` or
`sqlx-mysql` dependency tree, and confirms the active SQLx backend is
`sqlx-postgres`.
The `Universal Forms Gates` GitHub Actions job runs this security scope audit,
`scripts/audit-universal-forms-source-coverage.sh`, and
`scripts/audit-universal-forms-production-readiness.sh` for pull requests so
source, packaging, and production-readiness drift are caught before merge. CI
clears repository `RUSTFLAGS` only for third-party cargo tool installation such
as `cargo-machete` and `cargo-audit`, so tool builds do not inherit the
workspace warning-as-error policy. Run `scripts/ci-universal-forms-gates.sh`
locally to execute the same Universal Forms Gates bundle used by CI.

The report output boundary smoke is the production guard for generated handoff
artifacts. It runs the report and JSON generators with temporary `--output`
paths, requires stdout to remain empty, checks Markdown/HTML first lines, and
checks JSON schema versions before the delivery manifest is regenerated. It
also verifies `scripts/report-universal-forms-signoff-status-json.sh --output -`
and the equivalent `--stdout` alias emit valid JSON on stdout, so pipe-based
automation can read the signoff queue without relying on a temporary file and
without leaving a repository-root `-` output file.

For a local compose bootstrap, use:

```bash
bash scripts/start.sh
```

The start script generates local-only bootstrap secrets when `.env` does not
exist and runs `cargo build --workspace --release` before `docker compose up`
because the default compose file uses `Dockerfile.prebuilt`. Production
deployments must replace the generated `.env` values with deployment-owned
secrets and workspace-scoped MCP credentials.

For a local first-run demo only, seed the restaurant ordering scenario after the
stack is healthy:

```bash
scripts/bootstrap-restaurant-demo.sh
```

The demo helper creates a local demo account/workspace/project, sample
universal-form records, and a workspace-scoped MCP bot token through the public
API. When the MCP configuration file exists it writes `mcp.bot_token` and
`mcp.workspace_id` into it, then recreates a running compose `mcp-server` so
local MCP clients use the demo workspace. If the MCP HTTP endpoint is reachable, it
verifies `/mcp/rpc` with `projects.list` and confirms the demo project appears
through MCP. It refuses non-local API URLs unless `OPENPR_DEMO_ALLOW_REMOTE=1`
is set, and it is not a production data seeding path.

For release engineering, `scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh`
runs the same local demo bootstrap against disposable PostgreSQL/API/MCP
processes, uses a temporary configuration file, and forces MCP HTTP
verification. It is a
pre-production proof of the onboarding path; it is still not a production data
seeding path.

Expected pre-signoff state after a full delivery bundle refresh:

- `Total automated checks: 27`
- `Failed automated checks: 0`
- `Tracker non-manual unresolved items: 0`
- `Manual signoff rows pending: 7`
- `Overall handoff progress: 27 / 34 complete, 7 remaining`
- `用户侧人工验收: 待处理`

Do not mark final acceptance until user-side manual signoff is complete.

## Runtime Configuration

The compose deployment must not ship with demo MCP credentials. Configure these
values for the target deployment before starting OpenPR services:

The api, worker and mcp-server binaries read no environment variables at all.
Their settings live in TOML configuration files that `docker-compose.yml` mounts
read-only at `/app/config/openpr.toml`; `bash scripts/start.sh` generates a
working pair on first run. `config/openpr.example.toml` is the annotated
reference for every key.

`config/openpr.compose.toml` — api and worker:

```toml
[database]
url = "postgres://openpr:replace_with_postgres_password@postgres:5432/openpr"

[auth]
jwt_secret = "replace_with_long_random_secret"
# default_author_id must name a user that exists; leave it out otherwise.
```

`config/openpr.compose.mcp.toml` — mcp-server. It carries no `[database]` and no
`[auth]`: the service opens no database connection and signs no token.

```toml
[mcp]
api_url = "http://api:8080"
workspace_id = "replace_with_workspace_uuid"
# bot_token is only needed if this deployment also runs stdio or CLI subcommands against
# this workspace; http/sse ignore it entirely. Leave it unset for an http/sse-only deployment.
# bot_token = "opr_replace_with_workspace_bot_token"
```

`.env` is left to docker-compose's own `${...}` interpolation and to the
postgres image. Nothing in it reaches OpenPR's own code:

```bash
POSTGRES_PASSWORD=replace_with_postgres_password
OPENPR_FRONTEND_PORT=3000
OPENPR_API_PORT=8081
MCP_SERVER_PORT=8090
VITE_API_BASE_URL=
```

`POSTGRES_PASSWORD` and the password inside `database.url` must be the same
concrete value, not the example one: the postgres image runs initdb from
`POSTGRES_PASSWORD` exactly once, and a later disagreement fails every query
with an authentication error. `auth.jwt_secret` must be a concrete deployment
secret, not the example value.
`mcp.api_url` should point to the API service on the compose network. `mcp.workspace_id`
must be the production workspace that AI assistants, CLI calls, and MCP clients are allowed
to operate on. `mcp.bot_token` is only needed for `stdio` and CLI subcommands run against
this workspace; the compose deployment serves `http`, where `mcp.bot_token` is unused and
can be left unset — every inbound request instead carries its own caller's MCP-type account
token as `Authorization: Bearer opr_...`, which the server forwards to the API unchanged and
the API authenticates against the same `mcp.workspace_id`. There is no `mcp.auth_token` key
any more; if a configuration file still sets one, the server refuses to start rather than
run with a key that no longer has any effect.
For Docker Compose frontend delivery, keep `VITE_API_BASE_URL` empty so browser
requests use the nginx same-origin API proxy. For local frontend development
outside compose, set `VITE_API_BASE_URL=http://localhost:8081`.

PostgreSQL is exposed only inside the compose network. Do not publish the
database port unless an operator has a separate firewall, backup, and access
control plan. API, MCP, frontend, and optional connector receiver host ports bind
to `127.0.0.1` by default. Put a deployment-owned reverse proxy or tunnel in
front of the frontend and any intentional API/MCP endpoint, and terminate TLS
there. Optional connector receivers, such as `openpr-webhook`, run under the
`connectors` profile and must use deployment-local image and config values, not
machine-specific `/opt/...` paths.

The repository includes `config/openpr-webhook.example.toml` so the optional
connector receiver profile has a portable starter config. For production, copy
it to a deployment-owned path, set a concrete `webhook_secrets` value, keep
`allow_unsigned = false`, and point `OPENPR_WEBHOOK_CONFIG` at that file.

The compose file intentionally avoids fixed `container_name` values. This lets
operators run multiple OpenPR deployments on the same host by using different
compose project names while keeping service discovery on compose service names
such as `api`, `postgres`, `worker`, `mcp-server`, and `frontend`.

## Database

Production is PostgreSQL-only for this delivery path. The universal forms stack
depends on:

- `project_forms`
- `form_views`
- `form_records`
- `form_record_links`
- `form_record_field_index`
- `business_events`
- `event_outbox`
- `event_inbox`
- `plugins`
- `plugin_invocations`

Operational requirements:

- Back up PostgreSQL before migrations and before plugin/runtime upgrades.
- Treat `business_events`, `event_outbox`, `event_inbox`, `agent_invocations`, and `plugin_invocations` as audit data.
- Do not truncate event or invocation tables during incident recovery unless the business accepts loss of audit history.
- Verify `event_outbox` has no growing backlog before declaring connector delivery healthy.

## First Business Scenario

Use `restaurant_ordering_default` as the first production acceptance scenario.

Create a project with:

```json
{
  "key": "REST",
  "name": "Restaurant Ordering",
  "scenario_template_key": "restaurant_ordering_default"
}
```

The project should initialize:

- Forms: `menu_category`, `sku`, `table`, `order`, `order_line`, `print_job`, `business_report`.
- Grid/detail views for each form.
- Print and webhook connector suggestions.
- Active `restaurant_calc` WASM plugin with an `order_line` formula hook.

Production acceptance flow:

1. Create menu category, SKU, and table records.
2. Create an order.
3. Create order line records.
4. Confirm `line_total` is decimal-safe and has no floating-point drift.
5. Link order lines to the order with `parent_child`.
6. Change table and confirm `order.table_changed` event exists.
7. Create kitchen and receipt `print_job` records.
8. Confirm print connector invocation and receipt path.
9. Create `business_report`.
10. Query revenue through MCP `form_records.aggregate`.

## MCP

MCP is the business automation interface for AI assistants and external tools.
In production, verify:

- MCP server health endpoint responds.
- `tools/list` includes `forms.*`, `form_records.*`, `events.tail`, `plugins.*`, `connectors.*`, project type, and scenario template tools.
- The bot token in use — the caller's own `Authorization: Bearer opr_...` token for `http`/`sse`, or `mcp.bot_token` for `stdio`/CLI — belongs to the production workspace named by `mcp.workspace_id`; a token from another workspace gets `403`.
- Project-aware capability filtering still exposes forms tools for the restaurant project.
- Generic CLI tool calls work for `forms.list` and `form_records.aggregate`.

Smoke command:

```bash
scripts/smoke-forms-mcp.sh
```

## Object Storage

Production object storage backs upload source files, generated image
derivatives, CSV import-file artifacts, and attachment package ZIP artifacts.
Before accepting an external object-storage deployment, run the deployed smoke
against the production surface. Use `OPENPR_EXPECT_OBJECT_STORAGE_BACKEND=s3`
when the environment is expected to use the S3-compatible backend; leave it
unset only when the deployment intentionally uses the local backend.
The API and worker read the backend from `[storage]` in
`config/openpr.compose.toml`. Set `backend = "s3"` and fill in `[storage.s3]`
before starting the stack when using S3-compatible storage; `endpoint`,
`bucket`, `access_key_id` and `secret_access_key` are required in that mode,
`region` defaults to `us-east-1`, and `session_token` is optional for temporary
credentials. `[storage.s3]` is left unread while `backend = "local"`, so it can
stay filled in and the deployment switches by editing one line:

```toml
[storage]
backend = "s3"

[storage.s3]
endpoint = "https://s3.eu-central-1.amazonaws.com"
bucket = "openpr-uploads"
region = "us-east-1"
access_key_id = "replace_with_s3_access_key_id"
secret_access_key = "replace_with_s3_secret_access_key"
```

The smoke command logs in to the deployed API, creates a temporary form, uploads
an image and a CSV through `/api/v1/upload`, verifies source and thumbnail
reads, imports records through `import-file`, creates image attachment metadata
that forces server JPEG/WebP thumbnail, preview, and named variant derivatives,
creates an attachment package job, and downloads the protected ZIP artifact. A
passing run proves the deployed object-storage path for upload, derivative,
import-file, and package artifact bytes, including configured derivative format
policy for `thumbnail_format`, `preview_format`, and `variants[].format`. Use
`OPENPR_OBJECT_STORAGE_CLEANUP=0` when operators need to inspect the temporary
form and attachment package job after a failed run.
The deployed `/api/v1/upload` response must expose `storage_backend`,
`object_key`, and image `thumbnail_url`; if those fields are absent, the API
build is older than the object-storage acceptance contract and must be
redeployed before this smoke can prove production storage behavior.

Smoke command:

```bash
OPENPR_EXPECT_OBJECT_STORAGE_BACKEND=s3 scripts/smoke-universal-forms-production-object-storage.mjs
```

## Attachment Lifecycle

Production attachment lifecycle acceptance proves that private attachment
metadata follows the same state rules as the underlying object-storage bytes.
Before accepting attachment-heavy deployments, run the deployed smoke against
the production surface. Use `OPENPR_EXPECT_OBJECT_STORAGE_BACKEND=s3` when the
environment is expected to use the S3-compatible backend, and use
`OPENPR_ATTACHMENT_LIFECYCLE_CLEANUP=0` when operators need to inspect the
temporary form, record, and attachment metadata after a failed run.

The smoke command logs in to the deployed API, creates a temporary form with a
private image field, uploads a PNG through `/api/v1/upload`, creates a
record-scoped attachment metadata row, verifies source and thumbnail reads,
creates and follows a protected attachment signed download URL, archives the
attachment, checks that active listing hides it and signed download rejects
archived attachments, then restores the attachment and verifies signed download
works again. A passing run proves record-scoped metadata create/list,
archive/restore state transitions, active versus include-archived listing, and
protected download behavior against the deployed storage backend.
The acceptance explicitly archives the attachment during the run, and signed download rejects archived attachments until the restore step clears `archived_at`.

Smoke command:

```bash
OPENPR_EXPECT_OBJECT_STORAGE_BACKEND=s3 scripts/smoke-universal-forms-production-attachment-lifecycle.mjs
```

## Signature Lifecycle

Production signature lifecycle acceptance proves that signature fields are not
only captured as form values, but materialized into protected object-storage
objects with record-scoped audit evidence. Before accepting a deployment with
signature-heavy workflows, run the deployed smoke against the production
surface. Use `OPENPR_SIGNATURE_CLEANUP=0` when operators need to inspect the
temporary form, record, and signature audit output after a failed run.

The smoke command logs in to the deployed API, creates a temporary form with a
reason-required signature field and consent statement, creates a record from a
PNG data URL, reads the materialized signature image, creates and follows a
signed signature download URL, updates the record with a replacement signature,
and calls
`/api/v1/form-records/{record_id}/signatures/audit-verification`. A passing run
proves digest verification, the `signature_lifecycle` summary for
active-with-audit and replacement metadata, and a
`signature_workflow_verification.status=verified` decision that combines active
field state, current audit-entry coverage, digest verification, reason/consent
presence, actor operation attribution, and verifiable event counts.

Smoke command:

```bash
scripts/smoke-universal-forms-production-signature-lifecycle.mjs
```

## Webhooks

Webhooks are generic event consumers. They do not have to be agents.

Production webhook checks:

- API CRUD persists webhook definitions without mirroring them into another table.
- Signed delivery is verified by the external webhook consumer.
- Optional webhook receiver containers are started only when the deployment enables its compose profile.
- Generic webhook delivery is verified independently of bot operation records.

Smoke commands:

```bash
scripts/smoke-webhook-generic-consumer.sh
scripts/smoke-universal-forms-api.sh
bun --cwd frontend run smoke:restaurant-ordering
```

## WASM Plugins

Production plugin rules:

- Use `openpr.plugin.v1` manifests.
- Keep field validation, formula, and event handler hooks explicit in the manifest.
- Amount and numeric patches returned from plugins must pass the main system schema and decimal validation.
- Plugins run under wasmtime with fuel, timeout, memory limits, no host imports, and no WASI access.
- Review `plugin_invocations` for failures before accepting a scenario as production-ready.

Smoke commands:

```bash
scripts/smoke-wasm-plugin-runtime.sh
scripts/smoke-plugins-mcp.sh
```

## Frontend

Production frontend checks:

- The project detail page exposes the Forms entry.
- Forms grid and detail views load for every template form.
- Record create, edit, detail, parent-child link, aggregate display, and print job list work.
- Project template, template work-item, Forms UI, and restaurant desktop/mobile screenshots pass verifier dimensions and smoke logs.

Smoke commands:

```bash
cd frontend && bun run check && bun run build
cd frontend && bun run smoke:project-template && bun run smoke:template-work-items
cd frontend && bun run smoke:forms-ui && bun run smoke:restaurant-ordering
scripts/collect-universal-forms-ui-artifacts.sh
scripts/verify-universal-forms-ui-artifacts.sh
scripts/verify-universal-forms-ui-review-gallery.sh
scripts/smoke-universal-forms-ui-review-gallery-render.sh
```

## Manual Signoff

Reviewer handoff files:

- `/opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md`
- `/opt/worker/report/openpr/docs/openpr-universal-form-manual-evidence-map-2026-05-31.md`
- `/opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-packet-2026-05-31.md`
- `/opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md`

Finalization commands after all seven manual rows are accepted:

```bash
scripts/report-universal-forms-signoff-status.sh --reviewer "<name>"
scripts/report-universal-forms-signoff-status.sh --output \
  /opt/worker/report/openpr/docs/openpr-universal-form-signoff-status-2026-05-31.md
scripts/report-universal-forms-signoff-status.sh \
  /opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md \
  --reviewer "<name>"
scripts/record-universal-forms-manual-signoff.sh --list-items
scripts/verify-universal-forms-manual-signoff-consistency.sh \
  /opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md \
  /opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md
scripts/verify-universal-forms-acceptance-signoff.sh \
  /opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md \
  --runbook /opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md
scripts/finalize-universal-forms-acceptance.sh \
  --runbook /opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md
scripts/audit-universal-forms-delivery-state.sh --strict
scripts/audit-universal-forms-delivery-bundle.sh
scripts/report-universal-forms-completion-audit-json.sh
scripts/verify-universal-forms-completion-audit-json.sh
scripts/smoke-universal-forms-completion-audit-json-contract.sh
scripts/gate-universal-forms-release.sh
scripts/verify-universal-forms-release-gate-json.sh
scripts/smoke-universal-forms-release-gate-json-contract.sh
scripts/smoke-universal-forms-release-gate.sh
scripts/smoke-universal-forms-release-gate-output.sh
scripts/smoke-universal-forms-next-signoff-review-contract.sh
scripts/smoke-universal-forms-next-signoff-command.sh
scripts/smoke-universal-forms-manual-signoff-progression.sh
scripts/smoke-universal-forms-manual-signoff-commands.sh
scripts/status-universal-forms-delivery.sh
scripts/verify-universal-forms-delivery-status-json.sh
scripts/smoke-universal-forms-delivery-status-json-contract.sh
scripts/smoke-universal-forms-delivery-status-output.sh
scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh
```

Before the seven user-side manual rows are accepted, use `scripts/gate-universal-forms-release.sh --allow-pending` only as a pre-release handoff signal. A production release cut requires strict `scripts/gate-universal-forms-release.sh` to pass.
The completion audit JSON schema is `docs/schemas/openpr-universal-forms-completion-audit.schema.json`; `scripts/verify-universal-forms-completion-audit-json.sh` and `scripts/smoke-universal-forms-completion-audit-json-contract.sh` keep the machine-readable completion gate aligned with automated evidence, tracker status, manual signoff counts, finalizer safety, finalization flags, and nested object shape.
`scripts/smoke-universal-forms-release-gate.sh` verifies the release gate output before the handoff is treated as production-ready.
`scripts/smoke-universal-forms-release-gate-output.sh` verifies that the human-readable release gate output mirrors the release gate JSON decision in both `--allow-pending` and strict modes.
The release gate JSON schema is `docs/schemas/openpr-universal-forms-release-gate.schema.json`; `scripts/verify-universal-forms-release-gate-json.sh` and `scripts/smoke-universal-forms-release-gate-json-contract.sh` keep the machine-readable gate aligned with readiness, development, signoff, final flags, tracker status, nested required fields, and nested object shape.
The release gate mode enum is `blocked`, `pre_signoff`, and `release`; the compact delivery status JSON mirrors that same enum so production automation does not treat final release as a separate `ready` mode.
The readiness JSON, signoff status JSON, release gate JSON, and delivery status JSON all constrain next manual keys to the same seven manual signoff keys, or an empty string after final signoff. The release and delivery status next manual keys are constrained to the same seven manual signoff keys, or an empty string after final signoff, so release automation and compact status consumers share the same reviewer routing boundary.
The readiness and signoff status JSON schemas also pin the seven manual rows in reviewer order from `restaurant_template` through `overall`; release automation must treat that order as the prerequisite sequence for final signoff.
`scripts/smoke-universal-forms-next-signoff-command.sh` verifies that the JSON next-row signoff command remains executable as a dry-run and does not mutate official evidence.
`scripts/smoke-universal-forms-manual-signoff-progression.sh` verifies row-by-row manual acceptance on temporary copies, including accepted/pending counters, next reviewer key movement, actionable state, and the final signoff flag.
`scripts/verify-universal-forms-next-signoff-review.sh` verifies the one-page next-row reviewer aid against the signoff status JSON, linked evidence files, screenshot paths, and recorder command before it is handed to a reviewer.
`scripts/smoke-universal-forms-next-signoff-review-contract.sh` mutates temporary reviewer-page copies and requires heading, counter, next-key, recorder-command, screenshot-path, and verification-command drift to be rejected before reviewer handoff.
`scripts/smoke-universal-forms-manual-signoff-commands.sh` verifies every manual signoff key on temporary copies and proves the final signoff verifier accepts a fully signed temporary handoff.
The signoff status JSON includes automated evidence, reviewer check text, and a recorder command for every manual row so CI, MCP tools, webhook consumers, or release automation can present the same accepted-row instructions without reconstructing shell arguments. It also includes `pending_queue`, an ordered queue of the remaining manual rows with `review_order`, `is_next`, `actionable`, evidence text, reviewer check text, and the row recorder command.
`scripts/smoke-universal-forms-signoff-status-output.sh` verifies that the reviewer-facing signoff status Markdown mirrors the signoff status JSON row table, next action, recorder command, and finalization branch before handoff.
`scripts/prepare-universal-forms-next-signoff-review.sh` generates the one-page reviewer checklist for the current next manual row from the verified signoff status JSON. It links row-specific evidence, screenshots, verification commands, and the recorder command without mutating runbook or evidence files.
`scripts/status-universal-forms-delivery.sh --json` is the compact CI-facing status view for this same handoff state. It verifies JSON contracts, next-row command dry-run, row-by-row signoff progression, all manual row command dry-runs, and the pre-signoff release gate before emitting counts. Successful prerequisite checks stay quiet on stdout and stderr; failed prerequisites replay their captured logs. Verify it with `scripts/verify-universal-forms-delivery-status-json.sh`; its contract smoke rejects schema-path, counter, nested missing-field, nested extra-field, next-key, manifest-count, final-flag, completion-summary, and release-gate drift before automation consumes the status. The compact status JSON is intentionally generated on demand instead of stored in the checksum manifest, because it includes the current generation time and reads the manifest file count.
The compact status JSON includes `verified_prerequisites`, a pinned eight-item
list covering readiness, development status, signoff status, delivery manifest,
next signoff command smoke, manual signoff progression smoke, all-row manual
command smoke, and the pre-signoff release gate JSON. Production automation can
use that list to confirm the status result was generated after the expected
read-only checks, not from raw counters alone.
It also includes `completion_summary`, a stable management and automation
contract for engineering checks completed/total, manual accepted/total/
remaining rows, total handoff items completed/total/remaining/percent, the
manual-signoff release block flag, and the derived delivery state.
`completion_breakdown` gives production automation the same progress split into
engineering checks, user-side manual signoff, and overall handoff rows, each
with completed/total/remaining/percent and a status marker. `release_blockers`
then provides a fixed automated-checks, non-manual-tracker, and manual-signoff
clear/blocking list with blocker counts, required actions, and evidence paths,
so release automation can explain why final release is still blocked without
scraping the free-form reason. `next_actions` adds the ordered production
handoff actions with enabled flags, blocker keys, commands, and evidence paths;
while manual signoff is pending, review and record actions are enabled and
verify/finalize actions remain blocked by `manual_signoff`. It mirrors
the signoff `pending_queue` as `manual_signoff_queue`, giving
MCP, CLI, webhook, CI, release automation, and web dashboards the full ordered
reviewer queue from the compact status response. `next_manual_signoff` mirrors
the current row's automated evidence, reviewer check, suggested evidence note,
actionable flag, and recorder command, so production handoff UIs can show the
next reviewer task without opening the larger signoff report. It also carries
`review_surfaces`, a stable set of reviewer-facing paths for the runbook,
automated evidence, manual evidence map, user acceptance packet, signoff status
report, next-row review, signoff dashboard, and UI review gallery.
`/opt/worker/report/openpr/docs/openpr-universal-form-signoff-dashboard-2026-05-31.html`
is the reviewer-facing HTML view of that same queue, including a Start Here
section with the next key, status command, and recorder command. Generate it with
`scripts/prepare-universal-forms-signoff-dashboard.sh` and verify it with
`scripts/verify-universal-forms-signoff-dashboard.sh`; run
`scripts/smoke-universal-forms-signoff-dashboard-render.sh` to capture
desktop/mobile render screenshots before handing the queue to a non-technical
reviewer. The render smoke reads the current signoff status JSON, so it keeps
working as the next reviewer key moves through the seven-row queue. After all
manual rows are accepted, the dashboard Start Here section switches to
finalizer, strict delivery-state audit, and delivery-bundle audit commands.
`scripts/smoke-universal-forms-signoff-dashboard-progression.sh` verifies those
dashboard transitions with temporary signoff files for 0/7 accepted, after the
first accepted row, and after 7/7 accepted, then checks the official handoff
files stayed unchanged. Successful subcommands stay quiet; failed subcommands
replay captured stdout/stderr for diagnosis.
`scripts/smoke-universal-forms-manual-signoff-progression.sh` and
`scripts/smoke-universal-forms-signoff-status-output.sh` use the same output
boundary: successful recorder/status reporter subcommands stay quiet, while
failed subcommands replay captured stdout/stderr.
The UI review gallery and signoff dashboard browser render smokes suppress
Chromium screenshot success chatter, and replay captured stderr if Chromium
fails.
`scripts/smoke-universal-forms-delivery-status-output.sh` verifies the default human-readable status output mirrors the compact JSON values, including the next manual signoff key and recorder command that operators hand to reviewers.

Final acceptance is complete only when the tracker marks `端到端验收` and
`用户侧人工验收` as `已验收`, and the unfinished section says `无。`.

The machine-readable delivery manifest JSON must stay aligned with the Markdown
checksum manifest. It uses `openpr.universal_forms.delivery_manifest.v1`, points
`schema_path` at
`docs/schemas/openpr-universal-forms-delivery-manifest.schema.json`, and is
verified for pinned top-level and nested keys, Markdown row parity, file row
order, and live file size/SHA before a release can consume it. Its contract
smoke must also pass so malformed JSON copies with schema, counter, file-count,
file-row, checksum, row-order, or Markdown path drift are rejected before
automation consumes the delivery bundle.

The machine-readable development status JSON must stay aligned with the tracker
development matrix. It uses `openpr.universal_forms.development_status.v1`,
points `schema_path` at
`docs/schemas/openpr-universal-forms-development-status.schema.json`, and keeps
every engineering phase, requirement, completion rule, status, and
`final_release_allowed` available to CI, MCP tools, webhook consumers, and
deployment scripts without parsing Markdown. The schema pins the ten
development rows in tracker order, and the verifier rejects extra fields inside
`status_summary` and row objects.

The machine-readable scenario catalog JSON must stay aligned with the built-in
scenario template catalog. It uses `openpr.universal_forms.scenario_catalog.v1`,
points `schema_path` at
`docs/schemas/openpr-universal-forms-scenario-catalog.schema.json`, and exposes
template keys, project types, generated forms, integrations, operator
entrypoints, operator steps, primary MCP tools, connector kinds, plugin keys,
and acceptance focus points for production automation without parsing Markdown.
The schema pins the three operator entrypoints, pins the six built-in templates
in catalog order, and the verifier rejects extra fields inside operator
entrypoint and template objects.
The live scenario template API and MCP tools return the same runtime
`usage_guide` shape on list, detail, and `openpr://scenario-templates`
resource reads, so production onboarding, AI routing, connector setup, and
template marketplace consumers use the same contract as the delivery catalog.

The machine-readable implementation map JSON must stay aligned with
`docs/universal-forms-implementation-map.md`. It uses
`openpr.universal_forms.implementation_map.v1`, points `schema_path` at
`docs/schemas/openpr-universal-forms-implementation-map.schema.json`, and
exposes delivery areas, source paths, public surfaces, primary verification
commands, status markers, and the manual-release boundary for production
automation without parsing Markdown. The schema pins status marker and module
order, and the verifier rejects extra fields inside marker, module, and
release-boundary objects.
