# OpenPR MCP Server

Model Context Protocol (MCP) server for OpenPR project management system.

## Overview

The MCP Server provides AI models with tools to interact with OpenPR's project management features, including:
- Project management (CRUD operations)
- Project types, scenario templates, and project resources
- Universal forms, form records, aggregate queries, and business events
- WASM plugin install/invoke and plugin invocation history
- Connectors and invocation lifecycle tracking
- Work item/issue tracking
- Comments and collaboration
- Global search across all entities

## Features

- **105 MCP Tools**: Project, governance, universal forms, WASM plugin, connector, invocation, release next actions, template, and scenario toolkit
- **Three Transport Modes**: stdio (for MCP clients), HTTP JSON-RPC, and SSE
- **JSON Schema Validation**: All tool parameters are strongly typed
- **OpenPR API Backend**: Calls the OpenPR API with workspace-scoped bot credentials
- **Async/Await**: Built on Tokio for high performance

## Quick Start

### Prerequisites

OpenPR reads no environment variables. Create a configuration file carrying the `[mcp]`
section — `config/openpr.example.toml` is the full annotated reference:

```toml
[mcp]
api_url = "http://localhost:8081"
# Required for transport = "stdio" and for the CLI subcommands. Not used by http/sse.
bot_token = "opr_your_token_here"
workspace_id = "your-workspace-uuid"
```

Save it as `config/openpr.toml` (the default path the binary looks for) or anywhere else and
pass `--config <path>`:

```bash
mcp-server serve --config config/openpr.toml
```

| Config key | Required | Purpose |
| --- | --- | --- |
| `mcp.api_url` | no, default `http://localhost:8081` | Base URL of the OpenPR API. |
| `mcp.bot_token` | conditional | Workspace bot token, used only by `stdio` and the CLI subcommands, which have no per-request caller to act on. Ignored by `http`/`sse`. |
| `mcp.workspace_id` | yes | Workspace UUID the calls are scoped to. |

There is no `mcp.auth_token` key any more. If your configuration file still has one, delete
it — the server refuses to start rather than run with a key that no longer does anything.

### Inbound authentication (HTTP/SSE)

There is no shared secret and no server-side identity for the networked transports to fall
back to. Every request to `/mcp/rpc`, `/sse` and `/messages` must carry the *caller's own*
MCP-type account token:

```
Authorization: Bearer opr_...
```

The MCP server does not itself validate this token beyond reading it out of the header — it
forwards it to the OpenPR API unchanged, and the API authenticates it and enforces
authorization. A request with no `Authorization` header, or a malformed one, is rejected
with `401` and `WWW-Authenticate: Bearer` before it reaches any tool. Because every caller
supplies their own credential, `mcp.bind_addr` can be `0.0.0.0` or any other reachable
address without extra configuration — there is no "anonymous acting as the server" case to
guard against.

The caller's token must belong to the same workspace named by `mcp.workspace_id` in this
server's configuration; the API returns `403` if it belongs to a different workspace.

`/health` is exempt: it answers a constant `OK`, reads nothing, and discloses nothing a
caller who completed the TCP handshake does not already know, so orchestrator probes need
no credential.

**stdio is unaffected**: it is a pipe pair owned by the process that spawned the server, so
the caller is already established, and the process authenticates to the API with
`mcp.bot_token` from the configuration file.

```bash
# Authenticated HTTP call. Use the caller's own MCP-type account token, not a shared secret.
curl -H "Authorization: Bearer opr_your_own_account_token" \
     -H 'Content-Type: application/json' \
     -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' \
     http://localhost:8090/mcp/rpc
```

### Build

```bash
cargo build -p mcp-server --release
```

### Run

#### stdio Mode (for MCP clients)
```bash
./target/release/mcp-server serve --transport stdio --config config/openpr.toml
```

#### HTTP Mode (for testing/debugging)
```bash
# Loopback default (127.0.0.1:8090). Every request still needs its own caller bot token.
./target/release/mcp-server serve --transport http --config config/openpr.toml

# Reachable from other hosts: no extra config needed, since there is no server-side
# credential to expose. Each request still authenticates with its own caller bot token.
./target/release/mcp-server serve --transport http --bind-addr 0.0.0.0:8090 --config config/openpr.toml
```

#### SSE Mode (for streaming clients)
```bash
./target/release/mcp-server serve --transport sse --config config/openpr.toml
```

## Available Tools

### Projects
- `projects.list` - List all projects in a workspace
- `projects.get` - Get project details by ID
- `projects.create` - Create a new project
- `projects.update` - Update project details

### Project Types, Templates, and Resources
- `project_types.list`, `project_types.get`
- `scenario_templates.list`, `scenario_templates.get`, `scenario_templates.install`
- `project_resources.list`, `project_resources.create`, `project_resources.update`, `project_resources.delete`

### Connectors and Invocations
- `connectors.list`, `connectors.get`
- `invocations.list`, `invocations.get`, `invocations.create`
- `invocations.report_progress`, `invocations.complete`, `invocations.fail`

### Universal Forms and Events
- `forms.list`, `forms.get`, `forms.create`, `forms.create_from_template`, `forms.update_schema`, `forms.duplicate`
- `forms.schema_summary`, `forms.field_usage`, `forms.field_dependencies`
- `form_schema_versions.list`, `form_schema_versions.get`
- `form_views.list`
- `form_permissions.get`, `form_permissions.update`
- `form_attachments.list`, `form_attachments.create`, `form_attachments.archive`, `form_attachments.restore`
- `form_records.list`, `form_records.export`, `form_records.import_preview`, `form_records.import_commit`
- `form_records.get`, `form_records.create`, `form_records.update`
- `form_records.link`, `form_records.relation_targets`, `form_records.children`
- `form_records.child_create`, `form_records.child_update`, `form_records.child_archive`, `form_records.child_restore`, `form_records.aggregate`
- `events.tail`

### WASM Plugins
- `plugins.list`, `plugins.get`, `plugins.install`, `plugins.invoke`
- `plugin_invocations.list`

### Work Items
- `work_items.list` - List work items in a project
- `work_items.get` - Get work item details by ID
- `work_items.create` - Create a new work item
- `work_items.update` - Update work item details
- `work_items.search` - Search work items by text

### Comments
- `comments.list` - List comments on a work item
- `comments.create` - Add a comment to a work item

### Search
- `search.all` - Global search across projects, work items, and comments

### Scenario Tools
- `code.resources.list`, `code.directory.get`, `code.task_context.get`, `code.change_proposal.create`
- `documents.extract_summary`, `documents.review_risk`, `approval.request`
- `inspection.report`, `corrective_action.propose`

## Usage Examples

### List Tools
```bash
# Using stdin/stdout
echo '{"jsonrpc": "2.0", "id": 1, "method": "tools/list"}' | \
  ./target/release/mcp-server serve --transport stdio --config config/openpr.toml

# Project-aware capability filtering
echo '{"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"project_id": "<project-uuid>"}}' | \
  ./target/release/mcp-server serve --transport stdio --config config/openpr.toml
```

### Call a Tool
```bash
# List projects in a workspace
echo '{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "projects.list",
    "arguments": {
      "workspace_id": "550e8400-e29b-41d4-a716-446655440000"
    }
  }
}' | ./target/release/mcp-server serve --transport stdio --config config/openpr.toml
```

### HTTP Mode Example
```bash
# Start server
./target/release/mcp-server serve --transport http --config config/openpr.toml

# Call tool via HTTP -- Authorization carries the caller's own MCP-type account token.
curl -X POST http://localhost:8090/mcp/rpc \
  -H "Authorization: Bearer opr_your_own_account_token" \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "tools/call",
    "params": {
      "name": "work_items.search",
      "arguments": {
        "workspace_id": "550e8400-e29b-41d4-a716-446655440000",
        "query": "bug"
      }
    }
  }'
```

## Docker

### Using Docker Compose

```bash
# Start all services (including MCP server)
docker compose up -d mcp-server

# MCP server will be available at localhost:8090
```

### Configuration in docker-compose.yml

```yaml
mcp-server:
  build:
    context: .
    dockerfile: Dockerfile.prebuilt
    args:
      APP_BIN: mcp-server
  command:
    - "/app/mcp-server"
    - "serve"
    - "--config"
    - "/app/config/openpr.toml"
    - "--transport"
    - "http"
    - "--bind-addr"
    - "0.0.0.0:8090"
  volumes:
    # Carries [logging] and [mcp] only -- no database URL and no signing key reach this service.
    - ./config/openpr.compose.mcp.toml:/app/config/openpr.toml:ro
  ports:
    - "127.0.0.1:8090:8090"
  depends_on:
    api:
      condition: service_healthy
```

The default compose stack uses `Dockerfile.prebuilt`, so build the release
binary first or use `bash scripts/start.sh`, which performs that build before
starting compose. `scripts/start.sh` also generates `config/openpr.compose.mcp.toml`
from `config/openpr.example.toml`.

## Development

### Project Structure

```
apps/mcp-server/
├── src/
│   ├── main.rs           # Entry point and transport layer
│   ├── lib.rs            # Library exports
│   ├── protocol.rs       # MCP protocol types
│   ├── server.rs         # Core MCP server logic
│   ├── client/           # OpenPR API client helpers
│   ├── tools/            # Tool implementations
│   │   ├── forms.rs
│   │   ├── plugins.rs
│   │   ├── connectors.rs
│   │   ├── project_types.rs
│   │   ├── scenario_templates.rs
│   │   ├── projects.rs
│   │   ├── work_items.rs
│   │   ├── comments.rs
│   │   └── search.rs
│   └── bin/
│       └── list-tools.rs # Utility to list all tools
```

### List All Tools

```bash
cargo run --bin list-tools
```

This outputs the currently registered MCP tools with their complete JSON Schema definitions.

### Adding a New Tool

1. Add or extend an OpenPR API client helper in `src/client/`.
2. Add tool definition and handler in `src/tools/<module>.rs`:
   ```rust
   pub fn my_tool_definition() -> ToolDefinition {
       ToolDefinition {
           name: "namespace.action".to_string(),
           description: "What this tool does".to_string(),
           input_schema: json!({
               "type": "object",
               "properties": { ... },
               "required": [ ... ]
           }),
       }
   }
   
   pub async fn my_tool(state: &AppState, args: Value) -> CallToolResult {
       // Implementation
   }
   ```
3. Register in `src/tools/mod.rs`
4. Add dispatch case in `src/server.rs`

## Testing

### Manual Testing

```bash
# Start the OpenPR API stack
bash scripts/start.sh

# Run MCP server
cargo run -p mcp-server -- serve --transport stdio --config config/openpr.toml

# Test with example request
echo '{"jsonrpc": "2.0", "id": 1, "method": "tools/list"}' | \
  cargo run -q -p mcp-server -- serve --transport stdio --config config/openpr.toml | jq '.'

# Test project-aware tool discovery
echo '{"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"project_id": "<project-uuid>"}}' | \
  cargo run -q -p mcp-server -- serve --transport stdio --config config/openpr.toml | jq '.'
```

### Integration with MCP Clients

The MCP server works with any MCP-compatible client:
- Claude Desktop
- MCP Inspector
- Custom MCP clients

Configure the client to use the server. The MCP server reads no environment variables, so
the client config points `--config` at a file carrying `[mcp]` (see Prerequisites above):
```json
{
  "mcpServers": {
    "openpr": {
      "command": "/path/to/mcp-server",
      "args": ["serve", "--transport", "stdio", "--config", "/absolute/path/to/openpr.toml"]
    }
  }
}
```

## Troubleshooting

### "mcp.auth_token has been removed"
The server refuses to start if the configuration file still has an `mcp.auth_token` key —
that key was a shared inbound secret in an earlier version and no longer does anything.
Delete the line. Every HTTP/SSE caller now presents its own workspace bot token instead;
see [Inbound authentication (HTTP/SSE)](#inbound-authentication-httpsse) above.

### "mcp.bot_token is required by the stdio transport and by the CLI subcommands..."
`stdio` and the CLI subcommands have no per-request caller to act on, so they need an
identity of their own. Create a workspace bot token and set it, along with the API URL and
workspace ID, in the `[mcp]` section of the configuration file:
```toml
[mcp]
api_url = "http://localhost:8081"
bot_token = "opr_your_token_here"
workspace_id = "your-workspace-uuid"
```
`http` and `sse` never need `mcp.bot_token`: every request already carries its own caller's
bot token.

### API Connection Refused
MCP talks to the OpenPR API, not directly to PostgreSQL. Confirm the API is running, then
point `mcp.api_url` at it in the configuration file:

```bash
curl http://localhost:8081/health
```

```toml
[mcp]
api_url = "http://localhost:8081"
```

### Tool Not Found
Use `cargo run --bin list-tools` to see all available tools and their exact names.

## Performance

- **API Requests**: Uses the OpenPR API client and relies on API-side indexes and authorization
- **Search Limits**: Results limited to prevent large responses
  - Work item search: 50 results
  - Global search: 20 results per category
- **Async I/O**: Non-blocking operations throughout

## Security

MCP is an API client. It does not accept arbitrary database credentials and it
does not bypass OpenPR authorization. It never verifies a bot token itself — it has no
signing key and no bot registry, so it forwards the token it holds and lets the API decide.

- `stdio` and the CLI subcommands act as the identity in `mcp.bot_token`, configured in the
  file.
- `http` and `sse` act as whoever called them: every request must carry its own caller's
  bot token in `Authorization: Bearer opr_...`, which this server forwards to the API
  unchanged. There is no shared secret and no server-side fallback identity for these
  transports — a request without a usable caller token gets `401` before it reaches any
  tool (`/health` is exempt).
- The API authenticates `opr_` bot tokens by SHA-256 token hash in
  `workspace_bots`.
- Disabled or expired bot tokens are rejected.
- Workspace access is enforced by the API; a bot token can only act inside its workspace,
  and must belong to the workspace named by this server's `mcp.workspace_id` — a token from
  another workspace gets `403`.
- JWT user tokens still use the normal API JWT validation path.
- For public HTTP exposure, place MCP behind your reverse proxy or tunnel and
  apply deployment-level rate limits.

## License

AGPL-3.0-or-later

## Resources

- [MCP Specification](https://modelcontextprotocol.io/)
- [OpenPR API Documentation](../../docs/)
- [Database Schema](../../migrations/)
