# Upgrading OpenPR 0.2.0 → 0.3.0

0.3.0 is a breaking release. A 0.2.0 deployment will **not** start on 0.3.0 without changes, and a
few things that used to work will start returning `403` after it does start.

Read this page end to end before you upgrade. Every step is written as **symptom → cause → fix**,
with the exact SQL, configuration and commands to run. Steps 1, 2 and 6 must be done before the
first start; steps 3, 4, 5, 7, 8 and 9 change behaviour that only shows up once you are running.

Two items **cannot be migrated automatically** and need a human decision:

- [Step 4 — connector credential references](#step-4--connector-credentials-are-referenced-by-bare-name): every
  stored `auth_policy.secret_ref` has to be rewritten by hand, and the credential value moved into
  the configuration file.
- [Step 7 — proposals without a workspace](#step-7--proposals-are-authenticated-and-tenant-scoped): proposals
  whose owning workspace cannot be derived from the data are left `NULL` and become visible to
  instance administrators only, until someone assigns them.

---

## Table of contents

| Step | What changes | When it bites |
| --- | --- | --- |
| [0](#step-0--before-you-begin) | Backups and preflight | — |
| [1](#step-1--configuration-moves-from-environment-variables-to-a-toml-file) | All configuration moves to TOML | Startup |
| [2](#step-2--mcp-inbound-authentication-is-per-caller) | MCP shared secret removed | Startup / every MCP call |
| [3](#step-3--bot-token-permissions-are-enforced) | Bot tokens become read-only | First write through a bot token |
| [4](#step-4--connector-credentials-are-referenced-by-bare-name) | `env:` secret refs rejected | Connector save / delivery |
| [5](#step-5--outbound-requests-are-ssrf-filtered) | Private outbound targets blocked | Connector/webhook save, delivery |
| [6](#step-6--migrations-are-tracked-and-a-failure-aborts-startup) | Migration ledger, fail-fast | Startup |
| [7](#step-7--proposals-are-authenticated-and-tenant-scoped) | Proposal auth, tenancy, settlement | Proposal reads; expiring proposals |
| [8](#step-8--label-updates-require-owner-or-admin) | `PUT /api/v1/labels/:id` restricted | Members editing labels |
| [9](#step-9--attachment-downloads-require-authorization) | `/uploads/*` no longer public | Hot-linked attachments |
| [10](#step-10--new-deployment-options) | New build/deploy options | Optional |

---

## Step 0 — before you begin

1. **Take a database backup.** Migration `0050_proposal_workspace_scope.sql` writes to `proposals`,
   and `0000_schema_migrations.sql` creates the migration ledger.

   ```bash
   pg_dump -Fc -U openpr -d openpr -f openpr-pre-0.3.0.dump
   ```

2. **Record what you have**, so you can rebuild it as TOML and verify it afterwards:

   ```bash
   # Environment currently feeding the services (compose deployment).
   docker compose config | grep -E 'DATABASE_URL|JWT_SECRET|RUST_LOG|BIND_ADDR|OPENPR_'

   # Bot tokens that will become read-only in step 3.
   psql -U openpr -d openpr -c \
     "SELECT id, workspace_id, name, token_prefix, permissions, is_active FROM workspace_bots ORDER BY workspace_id;"

   # Connector credential references that need rewriting in step 4.
   psql -U openpr -d openpr -c \
     "SELECT id, workspace_id, name, auth_policy->>'mode' AS mode, auth_policy->>'secret_ref' AS secret_ref
        FROM connectors WHERE auth_policy ? 'secret_ref';"

   # Outbound targets that step 5 may start refusing.
   psql -U openpr -d openpr -c "SELECT id, workspace_id, name, url FROM webhooks WHERE active;"
   psql -U openpr -d openpr -c "SELECT id, workspace_id, name, endpoint FROM connectors WHERE is_active;"
   ```

3. **Plan for downtime.** The API refuses to start until its configuration file is valid, and a
   failed migration now aborts startup instead of being warned past.

---

## Step 1 — configuration moves from environment variables to a TOML file

### Symptom

The service exits immediately with a message naming a missing configuration file, or — worse — it
starts and behaves as if nothing you set exists: the database URL you exported is ignored, logs come
out at the default filter, object storage points at `./uploads`.

```
Error: configuration file config/openpr.toml not found
```

### Cause

`api`, `worker` and `mcp-server` read **no environment variables at all** in 0.3.0. The file named by
`--config <PATH>` is the only source of configuration; without the flag the path is
`config/openpr.toml`, resolved relative to the process working directory. There are no built-in
fallbacks for a database URL or a signing key — an unconfigured service fails rather than inventing
one.

These variables are now inert. Passing them changes nothing:

| Retired variable | Replacement key |
| --- | --- |
| `DATABASE_URL` | `[database] url` |
| `JWT_SECRET` | `[auth] jwt_secret` |
| `RUST_LOG` | `[logging] filter` |
| `BIND_ADDR` | `[server] bind_addr` |
| `APP_NAME` | `[server] app_name` |
| `OPENPR_OBJECT_STORAGE_BACKEND` | `[storage] backend` |
| `OPENPR_OBJECT_STORAGE_DIR` | `[storage] dir` |
| `OPENPR_OBJECT_STORAGE_S3_ENDPOINT` | `[storage.s3] endpoint` |
| `OPENPR_OBJECT_STORAGE_S3_BUCKET` | `[storage.s3] bucket` |
| `OPENPR_OBJECT_STORAGE_S3_REGION` | `[storage.s3] region` |
| `OPENPR_OBJECT_STORAGE_S3_ACCESS_KEY_ID` | `[storage.s3] access_key_id` |
| `OPENPR_OBJECT_STORAGE_S3_SECRET_ACCESS_KEY` | `[storage.s3] secret_access_key` |
| `OPENPR_OBJECT_STORAGE_S3_SESSION_TOKEN` | `[storage.s3] session_token` |
| `OPENPR_MIGRATIONS_REPLAY` | `[migrations] replay` |
| `OPENPR_MIGRATIONS_CONTINUE_ON_ERROR` | `[migrations] continue_on_error` |
| `OPENPR_OUTBOUND_ALLOWED_HOSTS` | `[outbound] allowed_hosts` |
| `OPENPR_OUTBOUND_ALLOW_PRIVATE` | `[outbound] allow_private` |
| `OPENPR_API_URL` | `[mcp] api_url` |
| `OPENPR_BOT_TOKEN` | `[mcp] bot_token` |
| `OPENPR_WORKSPACE_ID` | `[mcp] workspace_id` |
| `OPENPR_MCP_TRANSPORT` | `[mcp] transport` |
| `OPENPR_INVOCATION_ID` | `[mcp] invocation_id` |
| `OPENPR_CONNECTOR_SECRET_W_<uuid>_<NAME>` | `[connectors.secrets."<uuid>"] <NAME>` (see [step 4](#step-4--connector-credentials-are-referenced-by-bare-name)) |
| `OPENPR_MCP_AUTH_TOKEN` / `mcp.auth_token` | **removed, no replacement** (see [step 2](#step-2--mcp-inbound-authentication-is-per-caller)) |

### Fix

Start from the annotated reference and fill it in:

```bash
cp config/openpr.example.toml config/openpr.toml
$EDITOR config/openpr.toml
```

A minimal working file for a single-host API + worker deployment:

```toml
[server]
bind_addr = "0.0.0.0:8081"

[database]
url = "postgres://openpr:REAL_PASSWORD@localhost:5432/openpr"
max_connections = 20
min_connections = 2

[auth]
# openssl rand -hex 32 — minimum 16 characters. Changing it invalidates every issued token.
jwt_secret = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
access_ttl_seconds = 1296000
refresh_ttl_seconds = 1728000

[logging]
filter = "api=info,tower_http=info,sea_orm=warn"
format = "json"
output = "stderr"

[storage]
backend = "local"
dir = "./uploads"

[migrations]
replay = false
continue_on_error = false

[outbound]
allowed_hosts = []
allow_private = false
```

Then start the binaries with an explicit path:

```bash
./target/release/api        --config /etc/openpr/openpr.toml
./target/release/worker     --config /etc/openpr/openpr.toml
./target/release/mcp-server --config /etc/openpr/openpr.mcp.toml
```

**`[database]` and `[auth]` are validated lazily.** Their shape is checked whenever a value is
present, but their *presence* is only required by the binary that needs them. A deployment that runs
only `mcp-server` can delete both sections entirely rather than inventing a database URL and a
signing key it never uses — which is exactly what `config/openpr.compose.mcp.toml` does.

### Fix — compose deployments

`docker-compose.yml` now mounts two generated files read-only and names them with `--config`:

- `config/openpr.compose.toml` → `/app/config/openpr.toml` in **api** and **worker**
- `config/openpr.compose.mcp.toml` → `/app/config/openpr.toml` in **mcp-server**

`scripts/start.sh` generates both on first run with local bootstrap values, and validates them on
every run. To generate and check them without starting anything:

```bash
./scripts/start.sh --check-config
```

Replace the generated bootstrap `jwt_secret` and database password before using this anywhere real,
and keep the password in `config/openpr.compose.toml` in step with `POSTGRES_PASSWORD` in `.env` —
the postgres image only runs `initdb` once.

### Two operational traps

**1. The file must be readable by the user inside the container.** The generated files are
deliberately mode `0644`, not `0600`: the containers run as uid 1000, and under a rootless runtime
the uid mapping means a `0600` file owned by another uid is simply unreadable inside the container.
The service then fails to start with a file-not-readable error that looks nothing like a permission
problem. Protect the directory instead of the file:

```bash
chmod 644 config/openpr.compose.toml config/openpr.compose.mcp.toml
chown 1000:1000 config/openpr.compose.toml config/openpr.compose.mcp.toml
chmod 750 config    # keep the secrets unreadable to other users on the host
```

`scripts/start.sh` warns when the files are owned by a uid other than 1000.

**2. Never edit a mounted config by atomic rename.** A bind mount binds the *inode*. Writing a new
file and `mv`-ing it over the path gives the host a new inode while the container keeps reading the
old one — so your change appears to have no effect, indefinitely. Edit the file **in place**:

```bash
# WRONG — the container keeps reading the old inode
$EDITOR /tmp/new.toml && mv /tmp/new.toml config/openpr.compose.toml

# RIGHT — same inode, then recreate the service
$EDITOR config/openpr.compose.toml
docker compose up -d --force-recreate api worker
```

### Verify

```bash
./scripts/start.sh --check-config
curl -fsS http://127.0.0.1:8081/health
```

Validation reports **every** unusable value at once, so one editing pass fixes the whole file.
Note that the configuration structs use `deny_unknown_fields`: a misspelled key is a startup error,
not a silently ignored line. Empty strings, `replace_with_*` placeholders and unexpanded `${...}`
templates are all rejected on purpose.

---

## Step 2 — MCP inbound authentication is per-caller

### Symptom

`mcp-server` refuses to start:

```
mcp.auth_token has been removed: an inbound http/sse caller now presents its own workspace bot
token in `Authorization: Bearer opr_...`, ...
```

Or it starts, and every HTTP/SSE call comes back `401` with
`Missing Authorization: Bearer <opr_ bot token>`, or `403 bot not authorized for this workspace`.

### Cause

The shared inbound secret `mcp.auth_token` is **gone**. It is still recognised by the parser only so
that a file carrying it gets this explanation instead of a bare "unknown field" — its presence is a
hard startup error.

The MCP server no longer holds an identity it can act as on a shared listener. Under `http` and
`sse`, every request must present **its own caller's** MCP-type account token, which the server
forwards to the API unchanged; the API authenticates it and the audit trail records that bot. The
server verifies nothing itself — it has no signing key and no bot registry, so compromising it does
not let anyone mint identities. Only `/health` is exempt from the check.

`stdio` is unchanged in spirit: one process per person, so the identity is `mcp.bot_token` from the
configuration file. The CLI subcommands use it too.

### Fix

1. **Delete the key.** In every MCP configuration file:

   ```diff
    [mcp]
    api_url = "http://api:8080"
   -auth_token = "some-shared-secret"
    workspace_id = "8aa4950e-0627-465a-82d6-b2cd15992ad2"
   ```

2. **Issue one MCP-type bot account per caller** in the workspace named by `mcp.workspace_id`, and
   distribute the `opr_...` tokens. Callers send them per request:

   ```bash
   curl -sS http://127.0.0.1:8090/messages \
     -H 'Authorization: Bearer opr_your_own_bot_token' \
     -H 'Content-Type: application/json' \
     -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
   ```

3. **For `stdio` and the CLI**, keep a concrete `bot_token` in the file:

   ```toml
   [mcp]
   api_url = "http://localhost:8081"
   bot_token = "opr_..."          # must carry the opr_ prefix
   workspace_id = "8aa4950e-0627-465a-82d6-b2cd15992ad2"
   transport = "stdio"
   ```

   `workspace_id` is required to run the server at all, and must be a real, non-nil UUID.

### The 403 you will hit

A calling bot must belong to the workspace named by `mcp.workspace_id`. A token from any other
workspace is refused with `403 bot not authorized for this workspace` — the check is in the API, not
in the MCP server, so it holds no matter which transport the call came in on. Check the pairing:

```sql
SELECT id, name, token_prefix, workspace_id, permissions, is_active, expires_at
FROM workspace_bots
WHERE workspace_id = '<mcp.workspace_id>' AND is_active;
```

---

## Step 3 — bot token permissions are enforced

### Symptom

Everything a bot token used to write now returns `403`. Reads still work. This hits **every bot that
existed before the upgrade**, all at once.

### Cause

`workspace_bots.permissions` has always defaulted to `'["read"]'::jsonb`, but the column was not
actually consulted. It is now. Safe HTTP methods require `read`; everything else requires `write`,
with `admin` implying both. Label mutations require `admin` specifically (see
[step 8](#step-8--label-updates-require-owner-or-admin)).

So an existing bot that was silently doing writes is now read-only, and this cannot be fixed
automatically: granting write to every existing bot would be exactly the escalation the enforcement
exists to prevent.

### Fix

Look at what you have, then grant deliberately — **do not** blanket-update every row:

```sql
SELECT id, workspace_id, name, token_prefix, permissions, is_active
FROM workspace_bots
ORDER BY workspace_id, name;
```

Grant write to the specific bots that need it:

```sql
UPDATE workspace_bots
SET permissions = '["read","write"]'::jsonb
WHERE id = '<bot-uuid>';
```

Grant admin (implies read and write; required for label mutations):

```sql
UPDATE workspace_bots
SET permissions = '["read","write","admin"]'::jsonb
WHERE id = '<bot-uuid>';
```

Grant write to every bot in one workspace, once you have decided that is correct:

```sql
UPDATE workspace_bots
SET permissions = '["read","write"]'::jsonb
WHERE workspace_id = '<workspace-uuid>' AND is_active;
```

### Also new at issuance time

`POST` of a new bot token now validates the permission list:

- an **empty array** is rejected — `permissions must contain at least one of: read, write, admin`
- **unknown names** are rejected (`owner`, `delete`, …)
- names are **case-sensitive**: `"Read"` is refused, `"read"` is accepted
- duplicates are deduplicated, surrounding whitespace is trimmed

A token that is issued with no usable permission at all reaches nothing, the same way a user with no
workspace membership row does.

---

## Step 4 — connector credentials are referenced by bare name

> **Manual migration.** There is no automatic path. Each affected connector needs its
> `auth_policy.secret_ref` rewritten *and* its credential value moved into the configuration file.

### Symptom

Saving a connector returns `400`, and deliveries for connectors that declare `hmac` or `bearer` are
refused with an explanation naming the retired reference form.

### Cause

Credentials used to be process environment variables named
`OPENPR_CONNECTOR_SECRET_W_<workspace uuid, no dashes, uppercase>_<NAME>`, referenced from
`auth_policy.secret_ref` as `env:<that whole variable>`. Every workspace's credentials were reachable
from one `std::env::var` call, with only a name prefix between them.

Credentials now live in `[connectors.secrets]`, one table per workspace keyed by the workspace UUID,
and `secret_ref` holds the **bare name** only. The lookup selects the workspace's table first and
searches for the name only inside it, so no reference — however it is spelled — can reach another
tenant's credentials. Two workspaces may reuse the same credential name without colliding.

Anything starting with `env:` is refused outright, including `env:SHIPPING` — so the failure is never
ambiguous about which of the two schemes you are on.

### Fix

1. **List what needs changing:**

   ```sql
   SELECT id, workspace_id, name, auth_policy->>'mode' AS mode, auth_policy->>'secret_ref' AS secret_ref
   FROM connectors
   WHERE auth_policy->>'secret_ref' LIKE 'env:%'
   ORDER BY workspace_id;
   ```

2. **File each value under its workspace** in the configuration file. Drop the whole
   `OPENPR_CONNECTOR_SECRET_W_<uuid>_` prefix and keep the remaining name:

   ```toml
   # Was: OPENPR_CONNECTOR_SECRET_W_0F8A1B2C3D4E4F60818293A4B5C6D7E8_SHIPPING
   [connectors.secrets."0f8a1b2c-3d4e-4f60-8182-93a4b5c6d7e8"]
   SHIPPING = "the actual credential"
   PAYMENTS = "the actual credential"
   ```

   Names must match `[A-Z_][A-Z0-9_]*`, must not start with `OPENPR_`, `POSTGRES_`, `PG` or `AWS_`,
   and must not be `JWT_SECRET`, `DATABASE_URL` or `RUST_LOG`. Declare each workspace UUID exactly
   once — two spellings of the same UUID (case, dashes, urn form) are rejected rather than silently
   collapsed. Values are never logged.

3. **Rewrite the stored references.** Review the result before committing; this strips the prefix
   mechanically and assumes the trailing segment is the credential name:

   ```sql
   -- Inspect first.
   SELECT id, name,
          auth_policy->>'secret_ref' AS old_ref,
          regexp_replace(auth_policy->>'secret_ref', '^env:OPENPR_CONNECTOR_SECRET_W_[0-9A-Fa-f]+_', '') AS new_ref
   FROM connectors
   WHERE auth_policy->>'secret_ref' LIKE 'env:OPENPR_CONNECTOR_SECRET_W_%';

   -- Then apply.
   BEGIN;
   UPDATE connectors
   SET auth_policy = jsonb_set(
         auth_policy,
         '{secret_ref}',
         to_jsonb(regexp_replace(auth_policy->>'secret_ref', '^env:OPENPR_CONNECTOR_SECRET_W_[0-9A-Fa-f]+_', ''))
       )
   WHERE auth_policy->>'secret_ref' LIKE 'env:OPENPR_CONNECTOR_SECRET_W_%';
   COMMIT;
   ```

   A connector whose `secret_ref` does not follow that exact pattern must be rewritten by hand.

4. **Confirm nothing is left:**

   ```sql
   SELECT count(*) FROM connectors WHERE auth_policy->>'secret_ref' LIKE 'env:%';   -- expect 0
   ```

A connector that declares `hmac` or `bearer` without a resolvable credential is now **refused**
instead of quietly delivering unsigned, so a reference you miss shows up as a failed delivery rather
than as a silently unauthenticated one.

---

## Step 5 — outbound requests are SSRF-filtered

### Symptom

Creating or updating a connector or webhook returns `400` for a target that used to be accepted;
existing deliveries to internal hosts stop and are recorded as failed with the reason. In compose,
this typically hits service names like `api:8080` or `webhook:9090`.

### Cause

Outbound targets resolving to loopback, private, link-local or CGNAT addresses are blocked by
default, so a connector cannot be pointed at the cluster's metadata service or an internal admin
port. Validation runs on create, on update **and** again at delivery time, redirects are disabled,
and response bodies are capped.

### Fix

List the hosts you legitimately need in `[outbound] allowed_hosts`. Entries are matched **literally
and case-insensitively**, as `host` or `host:port` — no wildcards, no URLs, no paths. A wildcard or a
URL is rejected at startup rather than silently never matching:

```toml
[outbound]
allowed_hosts = ["webhook:9090", "api:8080", "mcp-server:8090", "frontend:80"]
allow_private = false
```

That list is exactly what `config/openpr.compose.toml` ships with, covering the in-compose services.

Find the targets you need to list:

```sql
SELECT DISTINCT url      FROM webhooks   WHERE active;
SELECT DISTINCT endpoint FROM connectors WHERE is_active AND endpoint IS NOT NULL;
```

The API logs a startup audit naming stored webhook endpoints that the new validation would refuse —
check the log after the first start rather than waiting for deliveries to fail.

`allow_private = true` disables the check entirely. Use it only on a closed network you control end
to end; prefer the allowlist.

> **Also changed:** creating a webhook now requires the workspace `admin` or `owner` role, matching
> what update and delete already required. A plain member who could previously create webhooks now
> gets `403 Workspace admin or owner required`.

---

## Step 6 — migrations are tracked, and a failure aborts startup

### Symptom

The API or worker exits during startup instead of logging a warning and carrying on:

```
migration <name> failed: <error>
```

### Cause

Before 0.3.0 the runner replayed every migration file on every start and downgraded any failure to a
warning ("likely already applied"). A partially applied schema was indistinguishable from a healthy
one, and every non-idempotent statement ran again on each restart.

There is now a ledger table, `schema_migrations`, and each migration executes at most once with its
outcome recorded — `applied`, `adopted` (already present in a database created before the ledger
existed, never executed) or `failed`. A failure aborts startup and keeps the error for you to read.
A row that already records `applied` or `adopted` is never downgraded to `failed`.

### Fix

**Nothing to do for an existing database.** On the first 0.3.0 start the runner marks the historical
migrations as `adopted` and moves on. Confirm afterwards:

```sql
SELECT status, count(*) FROM schema_migrations GROUP BY status;
SELECT name, status, error, applied_at FROM schema_migrations WHERE status <> 'applied' ORDER BY name;
```

If a migration does fail, read the recorded error, fix the cause, and restart — the next ordinary
start retries it. The two escape hatches are for recovery, not for normal operation, and both belong
in the configuration file rather than the environment:

```toml
[migrations]
# Re-execute every migration file once, reporting failures without aborting. Safe on a live
# database: a failure never downgrades a ledger row that already records a success.
replay = false

# Start despite a failed migration or a schema gap — the degraded path for getting a service up in
# order to inspect it. The failure is still recorded and logged.
continue_on_error = false
```

Turn one on deliberately, then turn it back off.

The worker additionally verifies at startup that the delivery-pipeline objects from migrations 0048
and 0049 exist, and refuses to start naming every missing object — previously those statements failed
on every poll with nothing in the log explaining why deliveries had stopped.

---

## Step 7 — proposals are authenticated and tenant-scoped

> **Partly manual.** Proposals whose workspace cannot be derived are left `NULL` and need a business
> decision.

### Symptom (three separate ones)

1. Unauthenticated tooling that read `GET /api/v1/proposals` now gets `401`.
2. Users see fewer proposals than before — some are visible only to instance administrators.
3. Proposals stop settling on their deadline in a deployment that runs the API but not the worker.

### Cause

`GET /api/v1/proposals` and its sub-resources had **no authentication at all**, and `proposals` had
no workspace or project column: every proposal lived in one flat instance-wide namespace and was
returned to anyone who got past the door. Migration `0050_proposal_workspace_scope.sql` adds
`workspace_id`, and every proposal read is now filtered by it.

The column is nullable on purpose. A proposal that cannot be attributed from data already in the
database is left `NULL` rather than being guessed into somebody's workspace. The API treats `NULL` as
"unattributed" and shows those rows to **instance administrators only** — nothing is deleted, nothing
is hidden from an operator, and no tenant sees another tenant's proposal.

Backfill uses two sources, in order, and only unambiguous cases:

1. the workspaces of the issues the proposal is linked to, when they all agree on one;
2. the author's workspace, when the author belongs to exactly one.

Settlement of expired proposals used to run on the API read path — a `GET` wrote decisions and trust
scores. It now runs only in the worker's polling pipeline, under a per-proposal advisory lock so
several worker replicas can run it concurrently.

### Fix

1. **Authenticate your proposal clients.** Anything scripted against `/api/v1/proposals` needs a
   session or a bot token now.

2. **Run the worker.** This is no longer optional if you use governance:

   ```bash
   ./target/release/worker --config /etc/openpr/openpr.toml
   # compose:
   docker compose up -d worker
   ```

3. **Find and assign the unattributed proposals.** These need a human to say where they belong:

   ```sql
   SELECT p.id, p.title, p.author_id, p.status, p.created_at
   FROM proposals p
   WHERE p.workspace_id IS NULL
   ORDER BY p.created_at DESC;
   ```

   Assign each one once you have decided:

   ```sql
   UPDATE proposals SET workspace_id = '<workspace-uuid>' WHERE id = '<proposal-uuid>';
   ```

   Confirm the backfill result overall:

   ```sql
   SELECT count(*) FILTER (WHERE workspace_id IS NULL) AS unattributed,
          count(*) FILTER (WHERE workspace_id IS NOT NULL) AS attributed
   FROM proposals;
   ```

4. **Send `workspace_id` when creating a proposal** if the author belongs to more than one
   workspace. Otherwise creation fails with
   `workspace_id is required: the author belongs to more than one workspace`:

   ```bash
   curl -sS -X POST http://127.0.0.1:8081/api/v1/proposals \
     -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
     -d '{"title":"...","workspace_id":"<workspace-uuid>", ...}'
   ```

   A caller may only name a workspace they belong to; instance administrators may name any. When the
   proposal is created against a project, the workspace is derived from that project.

---

## Step 8 — label updates require owner or admin

### Symptom

A workspace member editing a label gets `403 only owners and admins can update labels`. A bot token
gets `403 bot token requires the 'admin' permission to update labels`.

### Cause

`PUT /api/v1/labels/:id` was open to any workspace member, but labels are a **workspace-level**
object — one edit changes the label everywhere it is used, across every project in the workspace.
Update and delete now both require the `owner` or `admin` workspace role, and a bot token needs the
`admin` permission bit.

### Fix

There is no configuration for this. Either promote the people who legitimately curate labels:

```sql
UPDATE workspace_members SET role = 'admin'
WHERE workspace_id = '<workspace-uuid>' AND user_id = '<user-uuid>';
```

or route label edits through someone who already holds the role. For bots, see
[step 3](#step-3--bot-token-permissions-are-enforced) — grant `admin`, not just `write`.

---

## Step 9 — attachment downloads require authorization

### Symptom

Images and attachments that used to render now come back `401`/`403`, or `404` from anything outside
a logged-in browser session — most visibly in emails, chat unfurls, external dashboards and any
service that hot-links `/uploads/...`.

### Cause

`/uploads/*` and `/api/v1/uploads/*` — including the `thumbnails/`, `previews/`, `variants/` and
`signatures/` sub-paths — were completely public. Anyone with a URL could read any tenant's
attachment.

Both prefixes are now behind an access middleware that admits exactly two paths, and the handlers
then scope the object to the workspace that owns it:

- a normal **session**: a JWT cookie or bearer, or a bot token; or
- a valid, unexpired **signed download URL**.

### Fix

- **In-app usage** needs nothing: the frontend calls these paths with the session it already has.
- **Server-to-server** callers should send their existing token:

  ```bash
  curl -fsS -H "Authorization: Bearer $TOKEN" \
    http://127.0.0.1:8081/api/v1/uploads/<file_name> -o attachment.bin
  ```

- **Anything that cannot authenticate** — an email client, a webhook consumer, a public page — must
  be handed a signed URL instead of a bare path. A signed URL carries `expires` and `signature`
  query parameters and is validated against the requested object key before the handler runs; it is
  the only anonymous way in.

Audit your integrations for hard-coded `/uploads/...` links before upgrading; they will all break at
once.

---

## Step 10 — new deployment options

Not breaking, but they solve the problems that make a 0.3.0 rollout awkward on a host that is not a
full build machine.

### Deploying to a host without a Rust toolchain

Build elsewhere, copy `target/release/{api,worker,mcp-server}` to the host, then:

```bash
./scripts/start.sh --no-build
```

The script checks that all three binaries exist and are executable before doing anything, and tells
you which are missing if not. Build them on a host with the **same or older glibc** than the target.

### Deploying to a host without a bun toolchain

`frontend/Dockerfile` builds the frontend from source. `frontend/Dockerfile.prebuilt` serves an
existing `frontend/build/` directory instead. Select it with:

```bash
export OPENPR_FRONTEND_DOCKERFILE=Dockerfile.prebuilt
docker compose up -d --build frontend
```

Build `frontend/build/` on a machine that has bun and copy the directory over.

### Runtime base image

`Dockerfile.prebuilt` takes its runtime base from `OPENPR_RUNTIME_BASE`. The binaries are dynamically
linked against the build host's glibc, so **the base image must ship a glibc at least as new as the
host that built them**, or the containers die with a `GLIBC_x.yz not found` loader error.

`scripts/start.sh` derives it from `/etc/os-release` on a Debian host (for example
`debian:trixie-slim`) and persists it to `.env` so a plain `docker compose up --build` keeps the same
base. Pin it explicitly when building and running on different distributions:

```bash
export OPENPR_RUNTIME_BASE=debian:trixie-slim
```

---

## Post-upgrade verification

```bash
# 1. Configuration parses and validates.
./scripts/start.sh --check-config

# 2. Services answer.
curl -fsS http://127.0.0.1:8081/health     # api
curl -fsS http://127.0.0.1:8090/health     # mcp-server (open by design)

# 3. MCP refuses an anonymous call and accepts an authenticated one.
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8090/messages          # expect 401
curl -s -o /dev/null -w '%{http_code}\n' -H "Authorization: Bearer $OPR_TOKEN" \
     -H 'Content-Type: application/json' \
     -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' http://127.0.0.1:8090/messages   # expect 200

# 4. Uploads are no longer anonymous.
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8081/api/v1/uploads/<file_name>  # expect 401
```

```sql
-- 5. Migrations are all accounted for.
SELECT status, count(*) FROM schema_migrations GROUP BY status;
SELECT name, status, error FROM schema_migrations WHERE status = 'failed';

-- 6. No retired connector references remain.
SELECT count(*) FROM connectors WHERE auth_policy->>'secret_ref' LIKE 'env:%';

-- 7. Bot permissions are what you intended.
SELECT workspace_id, name, permissions FROM workspace_bots WHERE is_active ORDER BY workspace_id;

-- 8. Proposal attribution.
SELECT count(*) FILTER (WHERE workspace_id IS NULL) AS needs_a_decision FROM proposals;
```

Then exercise one write through a bot token, one connector delivery and one attachment download —
those are the three paths where the new refusals show up in production rather than at startup.

## Rolling back

0.3.0 adds `schema_migrations` and `proposals.workspace_id`; neither is read by 0.2.0, so the schema
itself is backward compatible and a rollback of the binaries alone will run. What does **not** roll
back is your configuration: 0.2.0 reads only environment variables and will ignore the TOML file
entirely. Keep the environment you recorded in [step 0](#step-0--before-you-begin) until you are
confident in the upgrade.

If you restore the pre-upgrade dump instead, remember that any manual work from steps 3, 4 and 7 is
in that database and will be lost with it.
