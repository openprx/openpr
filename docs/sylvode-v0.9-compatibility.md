# Sylvode v0.9 compatibility matrix

Sylvode is the default product name beginning with v0.9. The transition does not
rename stable protocols, historical database objects, or user data. Compatibility
aliases are implemented by the same code paths as their canonical forms; they are
not copied data or a second registry.

| Surface | v0.9 default | OpenPR compatibility | Conflict policy | Warning starts | Earliest removal |
| --- | --- | --- | --- | --- | --- |
| Product, Web UI, current docs | Sylvode | Historical documents may say “OpenPR” | Not applicable | v0.9 docs identify historical names | Not applicable |
| CLI | `sylvode` | `mcp-server` remains a shipped executable with the same commands | Explicit executable name selects the display surface | No runtime warning in v0.9 | Not before v1.1 and a separately accepted removal decision |
| Configuration | `config/sylvode.toml`; compose uses `sylvode.compose*.toml` | `config/openpr.toml` and `openpr.compose*.toml` are discovered only when the canonical file is absent | Both files present is an error; pass `--config` to select explicitly | Legacy discovery is reported by `scripts/start.sh` in v0.9 | Not before v1.1 and a separately accepted removal decision |
| Compose environment | `SYLVODE_BIND_HOST`, `SYLVODE_API_PORT`, `SYLVODE_FRONTEND_PORT`, `SYLVODE_MCP_PORT`, `SYLVODE_RUNTIME_BASE`, and `SYLVODE_WEBHOOK_*` | `OPENPR_BIND_HOST`, `OPENPR_API_PORT`, `OPENPR_FRONTEND_PORT`, `MCP_SERVER_PORT`, `OPENPR_RUNTIME_BASE`, and `OPENPR_WEBHOOK_*` remain readable | Equal values are accepted; different canonical and legacy values are an error | Conflict and legacy use are documented in v0.9 | Not before v1.1 and a separately accepted removal decision |
| REST API and schema IDs | `/api/v1` and the existing schema identifiers | Preserved byte-for-byte; no branded API prefix is introduced | Stable identifiers win; branding is display-only | None | No removal planned |
| MCP tools | Existing semantic tool names | Tool names and input/output schemas are unchanged | No alternate branded tool registry | None | No removal planned |
| MCP resources | `sylvode://` for every static resource and template | The corresponding `openpr://` URI reads the exact canonical bytes and returns `_meta.canonical_uri`; list endpoints emit canonical URIs only | Canonical URI is the identity | v0.9 documentation | Not before v1.1 and a separately accepted removal decision |
| Database and migrations | Existing physical names | Historical OpenPR-named databases, roles, tables, columns, and migrations are not renamed | Existing physical identity is authoritative | None | No removal planned |
| Release archives | `sylvode-<target>` | A matching `openpr-<target>` archive alias contains the same binaries | Canonical archive is shown first | v0.9 release notes | Not before v1.1 and a separately accepted removal decision |
| Telemetry and containers | Sylvode user-facing descriptions | Stable service labels (`api`, `worker`, `mcp-server`, `frontend`) are preserved so existing dashboards continue across upgrade | Service identity remains stable; product display changes | None | No removal planned |

The compatibility contract is intentionally fail-closed where two operator inputs
disagree. This avoids a deployment changing ports or configuration merely because a
new alias was added. Explicit `--config` remains available when both files must be
kept temporarily.

## Upgrade sequence

1. Back up the database and the existing configuration.
2. Install the v0.9 binaries. Existing `mcp-server` invocations and old configuration
   continue to work before any names are changed.
3. Copy the legacy configuration to the Sylvode filename, compare it, then remove or
   archive the old file before relying on default discovery.
4. Move compose variables to their `SYLVODE_*` names without defining conflicting
   values. No data-directory or database rename is required.
5. Run database migrations and health checks, then enable Flow according to the
   existing workspace policy.

Rollback to the v0.8 application retains the original `mcp-server`, `OPENPR_*`, and
OpenPR configuration names. v0.9 database migrations use the expand phase only, so
the older application does not require a down migration or copied user data.
