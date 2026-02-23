# OpenPR MCP Server

Model Context Protocol (MCP) server for OpenPR project management system.

## Overview

The MCP Server provides AI models with tools to interact with OpenPR's project management features, including:
- Project management (CRUD operations)
- Work item/issue tracking
- Comments and collaboration
- Global search across all entities

## Features

- **12 MCP Tools**: Complete project management toolkit
- **Two Transport Modes**: stdio (for MCP clients) and HTTP (for REST APIs)
- **JSON Schema Validation**: All tool parameters are strongly typed
- **PostgreSQL Backend**: Leverages existing OpenPR database
- **Async/Await**: Built on Tokio for high performance

## Quick Start

### Prerequisites

```bash
# Required environment variables
export DATABASE_URL="postgres://openpr:openpr@localhost:5432/openpr"
export JWT_SECRET="your-secret-key"
export RUST_LOG="info"
```

### Build

```bash
cargo build -p mcp-server --release
```

### Run

#### stdio Mode (for MCP clients)
```bash
./target/release/mcp-server --transport stdio
```

#### HTTP Mode (for testing/debugging)
```bash
./target/release/mcp-server --transport http --bind-addr 0.0.0.0:8090
```

## Available Tools

### Projects
- `projects.list` - List all projects in a workspace
- `projects.get` - Get project details by ID
- `projects.create` - Create a new project
- `projects.update` - Update project details

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

## Usage Examples

### List Tools
```bash
# Using stdin/stdout
echo '{"jsonrpc": "2.0", "id": 1, "method": "tools/list"}' | \
  ./target/release/mcp-server --transport stdio
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
}' | ./target/release/mcp-server --transport stdio
```

### HTTP Mode Example
```bash
# Start server
./target/release/mcp-server --transport http --bind-addr 0.0.0.0:8090

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
docker-compose up -d mcp-server

# MCP server will be available at localhost:8090
```

### Configuration in docker-compose.yml

```yaml
mcp-server:
  build:
    context: .
    dockerfile: Dockerfile
    args:
      APP_BIN: mcp-server
  environment:
    APP_NAME: mcp-server
    DATABASE_URL: postgres://openpr:openpr@postgres:5432/openpr
    RUST_LOG: info
  command: ["/app/mcp-server", "--transport", "http", "--bind-addr", "0.0.0.0:8090"]
  ports:
    - "8090:8090"
  depends_on:
    postgres:
      condition: service_healthy
```

## Development

### Project Structure

```
apps/mcp-server/
├── src/
│   ├── main.rs           # Entry point and transport layer
│   ├── lib.rs            # Library exports
│   ├── protocol.rs       # MCP protocol types
│   ├── server.rs         # Core MCP server logic
│   ├── db/               # Database operations
│   │   ├── projects.rs
│   │   ├── work_items.rs
│   │   └── comments.rs
│   ├── tools/            # Tool implementations
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

This will output all 12 tools with their complete JSON Schema definitions.

### Adding a New Tool

1. Add database function in `src/db/<module>.rs`
2. Add tool definition in `src/tools/<module>.rs`:
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
# Start PostgreSQL
docker-compose up -d postgres

# Run MCP server
cargo run -p mcp-server -- --transport stdio

# Test with example request
echo '{"jsonrpc": "2.0", "id": 1, "method": "tools/list"}' | \
  cargo run -q -p mcp-server -- --transport stdio | jq '.'
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
      "args": ["--transport", "stdio"],
      "env": {
        "DATABASE_URL": "postgres://...",
        "JWT_SECRET": "..."
      }
    }
  }
}
```

## Troubleshooting

### "DATABASE_URL is required"
Make sure the `DATABASE_URL` environment variable is set:
```bash
export DATABASE_URL="postgres://openpr:openpr@localhost:5432/openpr"
```

### Connection Refused (PostgreSQL)
Ensure PostgreSQL is running:
```bash
docker-compose up -d postgres
# Wait for healthy status
docker-compose ps postgres
```

### Tool Not Found
Use `cargo run --bin list-tools` to see all available tools and their exact names.

## Performance

- **Database Queries**: Optimized with raw SQL and proper indexing
- **Search Limits**: Results limited to prevent large responses
  - Work item search: 50 results
  - Global search: 20 results per category
- **Async I/O**: Non-blocking operations throughout

## Security

⚠️ **Current Status**: Authentication infrastructure exists but is not enforced.

**TODO**:
- Implement JWT token validation
- Add rate limiting
- Implement workspace/project access control

## License

AGPL-3.0-or-later

## Resources

- [MCP Specification](https://modelcontextprotocol.io/)
- [OpenPR API Documentation](../../docs/)
- [Database Schema](../../migrations/)
