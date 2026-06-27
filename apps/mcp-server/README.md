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

```bash
# Required environment variables
export OPENPR_API_URL="http://localhost:8081"
export OPENPR_BOT_TOKEN="opr_your_token_here"
export OPENPR_WORKSPACE_ID="your-workspace-uuid"
export RUST_LOG="info"
```

### Build

```bash
cargo build -p mcp-server --release
```

### Run

#### stdio Mode (for MCP clients)
```bash
OPENPR_API_URL=http://localhost:8081 \
OPENPR_BOT_TOKEN=opr_your_token_here \
OPENPR_WORKSPACE_ID=your-workspace-uuid \
./target/release/mcp-server serve --transport stdio
```

#### HTTP Mode (for testing/debugging)
```bash
OPENPR_API_URL=http://localhost:8081 \
OPENPR_BOT_TOKEN=opr_your_token_here \
OPENPR_WORKSPACE_ID=your-workspace-uuid \
./target/release/mcp-server serve --transport http
```

#### SSE Mode (for streaming clients)
```bash
OPENPR_API_URL=http://localhost:8081 \
OPENPR_BOT_TOKEN=opr_your_token_here \
OPENPR_WORKSPACE_ID=your-workspace-uuid \
./target/release/mcp-server serve --transport sse
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
  OPENPR_API_URL=http://localhost:8081 \
  OPENPR_BOT_TOKEN=opr_your_token_here \
  OPENPR_WORKSPACE_ID=your-workspace-uuid \
  ./target/release/mcp-server serve --transport stdio

# Project-aware capability filtering
echo '{"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"project_id": "<project-uuid>"}}' | \
  OPENPR_API_URL=http://localhost:8081 \
  OPENPR_BOT_TOKEN=opr_your_token_here \
  OPENPR_WORKSPACE_ID=your-workspace-uuid \
  ./target/release/mcp-server serve --transport stdio
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
}' | OPENPR_API_URL=http://localhost:8081 \
  OPENPR_BOT_TOKEN=opr_your_token_here \
  OPENPR_WORKSPACE_ID=your-workspace-uuid \
  ./target/release/mcp-server serve --transport stdio
```

### HTTP Mode Example
```bash
# Start server
OPENPR_API_URL=http://localhost:8081 \
OPENPR_BOT_TOKEN=opr_your_token_here \
OPENPR_WORKSPACE_ID=your-workspace-uuid \
./target/release/mcp-server serve --transport http

# Call tool via HTTP
curl -X POST http://localhost:8090/mcp/rpc \
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
  environment:
    APP_NAME: mcp-server
    RUST_LOG: info
    OPENPR_API_URL: http://api:8080
    OPENPR_BOT_TOKEN: ${OPENPR_BOT_TOKEN:?set OPENPR_BOT_TOKEN for the MCP server}
    OPENPR_WORKSPACE_ID: ${OPENPR_WORKSPACE_ID:?set OPENPR_WORKSPACE_ID for the MCP server}
  command: ["/app/mcp-server", "serve", "--transport", "http", "--bind-addr", "0.0.0.0:8090"]
  ports:
    - "127.0.0.1:8090:8090"
  depends_on:
    api:
      condition: service_healthy
```

The default compose stack uses `Dockerfile.prebuilt`, so build the release
binary first or use `bash scripts/start.sh`, which performs that build before
starting compose.

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
OPENPR_API_URL=http://localhost:8081 \
OPENPR_BOT_TOKEN=opr_your_token_here \
OPENPR_WORKSPACE_ID=your-workspace-uuid \
cargo run -p mcp-server -- serve --transport stdio

# Test with example request
echo '{"jsonrpc": "2.0", "id": 1, "method": "tools/list"}' | \
  OPENPR_API_URL=http://localhost:8081 \
  OPENPR_BOT_TOKEN=opr_your_token_here \
  OPENPR_WORKSPACE_ID=your-workspace-uuid \
  cargo run -q -p mcp-server -- serve --transport stdio | jq '.'

# Test project-aware tool discovery
echo '{"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"project_id": "<project-uuid>"}}' | \
  OPENPR_API_URL=http://localhost:8081 \
  OPENPR_BOT_TOKEN=opr_your_token_here \
  OPENPR_WORKSPACE_ID=your-workspace-uuid \
  cargo run -q -p mcp-server -- serve --transport stdio | jq '.'
```

### Integration with MCP Clients

The MCP server works with any MCP-compatible client:
- Claude Desktop
- MCP Inspector
- Custom MCP clients

Configure the client to use the server:
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

## Troubleshooting

### "OPENPR_BOT_TOKEN is required"
Create a workspace bot token and pass it with the API URL and workspace ID:
```bash
export OPENPR_API_URL="http://localhost:8081"
export OPENPR_BOT_TOKEN="opr_your_token_here"
export OPENPR_WORKSPACE_ID="your-workspace-uuid"
```

### API Connection Refused
MCP talks to the OpenPR API, not directly to PostgreSQL. Ensure the API is
running and `OPENPR_API_URL` points at it:

```bash
curl http://localhost:8081/health
export OPENPR_API_URL="http://localhost:8081"
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
does not bypass OpenPR authorization.

- Configure MCP with `OPENPR_API_URL`, `OPENPR_BOT_TOKEN`, and
  `OPENPR_WORKSPACE_ID`.
- The API authenticates `opr_` bot tokens by SHA-256 token hash in
  `workspace_bots`.
- Disabled or expired bot tokens are rejected.
- Workspace access is enforced by the API; a bot token can only act inside its workspace.
- JWT user tokens still use the normal API JWT validation path.
- For public HTTP exposure, place MCP behind your reverse proxy or tunnel and
  apply deployment-level rate limits.

## License

AGPL-3.0-or-later

## Resources

- [MCP Specification](https://modelcontextprotocol.io/)
- [OpenPR API Documentation](../../docs/)
- [Database Schema](../../migrations/)
