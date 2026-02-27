# OpenPR

Open-source project management platform with built-in governance, AI agent integration, and MCP support.

Built with **Rust** (Axum + SeaORM), **SvelteKit**, and **PostgreSQL**.

## What It Provides

- Full project management: issues, kanban board, sprints, labels, comments, file attachments.
- Governance center: proposals, voting, trust scores, decision records, veto & escalation.
- AI integration: bot tokens, AI agents, AI tasks, AI review, webhook callbacks.
- MCP server: 34 tools across 3 transport protocols (HTTP, stdio, SSE).
- Skill distribution: `AGENTS.md` for coding agents, skill package for governed workflows.

## Quick Start

### Docker Compose (Recommended)

```bash
git clone https://github.com/openprx/openpr.git
cd openpr
cp .env.example .env
docker-compose up -d
```

Services:
- **Frontend**: http://localhost:3000
- **API**: http://localhost:8080
- **MCP**: http://localhost:8090

### Development

```bash
# Prerequisites: Rust 1.75+, Node.js 20+, PostgreSQL 15+

# Backend
cp .env.example .env
cargo build
cargo run --bin api

# Frontend
cd frontend && npm install && npm run dev

# MCP Server
cargo run --bin mcp-server -- --transport http --bind-addr 0.0.0.0:8090
```

## MCP Server

The built-in MCP server exposes OpenPR as a tool provider for AI assistants.
Three transport protocols are supported simultaneously.

### Transport Protocols

| Protocol | Use Case | Endpoint |
|----------|----------|----------|
| **HTTP** | Web integrations, OpenClaw plugins | `POST /mcp/rpc` |
| **stdio** | Claude Desktop, Codex, local CLI | stdin/stdout JSON-RPC |
| **SSE** | Streaming clients, real-time UIs | `GET /sse` → `POST /messages?session_id=<id>` |

> HTTP mode exposes all three protocols on a single port: `/mcp/rpc` (HTTP), `/sse` + `/messages` (SSE), and health at `/health`.

### MCP Client Configuration

#### Claude Desktop / Cursor / Codex (stdio)

```json
{
  "mcpServers": {
    "openpr": {
      "command": "/path/to/mcp-server",
      "args": ["--transport", "stdio"],
      "env": {
        "OPENPR_API_URL": "http://localhost:3000",
        "OPENPR_BOT_TOKEN": "opr_your_token_here",
        "OPENPR_WORKSPACE_ID": "your-workspace-uuid"
      }
    }
  }
}
```

#### HTTP Mode

```bash
./target/release/mcp-server --transport http --bind-addr 0.0.0.0:8090

# Verify
curl -X POST http://localhost:8090/mcp/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

#### SSE Mode

```bash
# 1. Connect SSE stream (returns session endpoint)
curl -N -H "Accept: text/event-stream" http://localhost:8090/sse
# → event: endpoint
# → data: /messages?session_id=<uuid>

# 2. POST request to the returned endpoint
curl -X POST "http://localhost:8090/messages?session_id=<uuid>" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"projects.list","arguments":{}}}'
# → Response arrives via SSE stream as event: message
```

#### Docker Compose

```yaml
mcp-server:
  build:
    context: .
    dockerfile: Dockerfile.prebuilt
    args:
      BINARY: mcp-server
  environment:
    - OPENPR_API_URL=http://api:8080
    - OPENPR_BOT_TOKEN=opr_your_token
    - OPENPR_WORKSPACE_ID=your-workspace-uuid
  ports:
    - "8090:8090"
  command: ["./mcp-server", "--transport", "http", "--bind-addr", "0.0.0.0:8090"]
```

### Environment Variables

| Variable | Required | Description | Example |
|----------|----------|-------------|---------|
| `OPENPR_API_URL` | Yes | API server base URL | `http://localhost:3000` |
| `OPENPR_BOT_TOKEN` | Yes | Bot token (`opr_` prefix) | `opr_abc123...` |
| `OPENPR_WORKSPACE_ID` | Yes | Default workspace UUID | `e5166fd1-...` |

### Bot Token Authentication

MCP authenticates via **Bot Tokens** (prefix `opr_`), managed at **Workspace → Members → Bot Tokens** in the frontend.

Each bot token:
- Has a display name (shown in activity feeds)
- Is scoped to one workspace
- Creates a `bot_mcp` user entity for audit trail integrity
- Supports all read/write operations available to workspace members

### Tool Reference (34 tools)

#### Projects (5)
| Tool | Required Params | Description |
|------|-----------------|-------------|
| `projects.list` | — | List all projects in workspace |
| `projects.get` | `project_id` | Get project details with issue counts |
| `projects.create` | `key`, `name` | Create a project (key e.g. `PRX`) |
| `projects.update` | `project_id` | Update name/description |
| `projects.delete` | `project_id` | Delete a project |

#### Work Items / Issues (12)
| Tool | Required Params | Description |
|------|-----------------|-------------|
| `work_items.list` | `project_id` | List issues in a project |
| `work_items.get` | `work_item_id` | Get issue by UUID |
| `work_items.get_by_identifier` | `identifier` | Get by human ID (e.g. `PRX-42`) |
| `work_items.create` | `project_id`, `title` | Create issue. Optional: `state` (backlog/todo/in_progress/done), `priority`, `description`, `assignee_id`, `due_at`, `attachments` |
| `work_items.update` | `work_item_id` | Update any field. `attachments` appended as markdown links |
| `work_items.delete` | `work_item_id` | Delete an issue |
| `work_items.search` | `query` | Full-text search across all projects |
| `work_items.add_label` | `work_item_id`, `label_id` | Add one label |
| `work_items.add_labels` | `work_item_id`, `label_ids` | Add multiple labels |
| `work_items.remove_label` | `work_item_id`, `label_id` | Remove a label |
| `work_items.list_labels` | `work_item_id` | List labels on an issue |

#### Comments (3)
| Tool | Required Params | Description |
|------|-----------------|-------------|
| `comments.create` | `work_item_id`, `content` | Create comment. Optional: `attachments` |
| `comments.list` | `work_item_id` | List comments on an issue |
| `comments.delete` | `comment_id` | Delete a comment |

#### Files (1)
| Tool | Required Params | Description |
|------|-----------------|-------------|
| `files.upload` | `filename`, `content_base64` | Upload file (base64), returns `{ url, filename }`. Types: images, `.zip`, `.gz`, `.log`, `.txt`, `.pdf`, `.json`, `.csv`, `.xml` |

#### Labels (5)
| Tool | Required Params | Description |
|------|-----------------|-------------|
| `labels.list` | — | List all workspace labels |
| `labels.list_by_project` | `project_id` | List labels for a project |
| `labels.create` | `name`, `color` | Create label (color: hex e.g. `#2563eb`) |
| `labels.update` | `label_id` | Update name/color/description |
| `labels.delete` | `label_id` | Delete a label |

#### Sprints (4)
| Tool | Required Params | Description |
|------|-----------------|-------------|
| `sprints.list` | `project_id` | List sprints in a project |
| `sprints.create` | `project_id`, `name` | Create sprint. Optional: `start_date`, `end_date` |
| `sprints.update` | `sprint_id` | Update name/dates/status |
| `sprints.delete` | `sprint_id` | Delete a sprint |

#### Proposals (3)
| Tool | Required Params | Description |
|------|-----------------|-------------|
| `proposals.list` | `project_id` | List proposals, optional `status` filter |
| `proposals.get` | `proposal_id` | Get proposal details |
| `proposals.create` | `project_id`, `title`, `description` | Create a governance proposal |

#### Members & Search (2)
| Tool | Required Params | Description |
|------|-----------------|-------------|
| `members.list` | — | List workspace members and roles |
| `search.all` | `query` | Global search across projects, issues, comments |

### Response Format

Success:
```json
{ "code": 0, "message": "success", "data": { ... } }
```

Error:
```json
{ "code": 400, "message": "error description" }
```

## Skills and Agent Integration

- **Agent guide**: `apps/mcp-server/AGENTS.md` — workflow patterns and full tool examples for coding agents.
- **Skill package**: `skills/openpr-mcp/SKILL.md` — governed skill with workflow lines, templates, and scripts.
- **Client discovery**:
  1. Load `AGENTS.md` for tool semantics.
  2. Use `tools/list` to enumerate available tools at runtime.
  3. Follow workflow patterns (search → create → label → comment) for structured task execution.

## Features

### Project Management
- Workspaces & Projects — Multi-tenant isolation with role-based access
- Issues & Board — Kanban with drag-and-drop, priority, assignees, labels
- Sprints & Cycles — Sprint planning with cycle tracking
- Full-text Search — PostgreSQL FTS across issues, comments, proposals
- File Uploads — Attachments on issues, comments, proposals (images, docs, logs, archives)
- Activity Feed — Per-issue activity stream
- Notifications & Inbox — In-app notification center
- Import / Export — Bulk data operations

### Governance Center
- Proposals — Create, review, vote with configurable thresholds
- Voting System — Weighted voting with quorum
- Decision Records — Immutable log with audit trail
- Veto & Escalation — Veto power with escalation voting
- Trust Scores — Per-user scoring with history and appeals
- Proposal Templates, Chains, Impact Reviews
- Audit Logs & Analytics

### AI Integration
- AI Agents — Register AI participants with roles and permissions
- AI Tasks — Assign tasks to agents with progress tracking
- AI Review — AI feedback on proposals
- Bot Tokens — `opr_` prefixed workspace-scoped tokens
- MCP Server — 34 tools, 3 transports
- Webhook Callbacks — Task completion/failure/progress

## API Overview

| Category | Endpoints | Description |
|----------|-----------|-------------|
| Auth | `/api/auth/*` | Register, login, refresh token |
| Projects | `/api/workspaces/*/projects/*` | CRUD, members, settings |
| Issues | `/api/projects/*/issues/*` | CRUD, assign, label, comment |
| Board | `/api/projects/*/board` | Kanban board state |
| Sprints | `/api/projects/*/sprints/*` | Sprint CRUD and planning |
| Proposals | `/api/proposals/*` | Create, vote, submit, archive |
| Governance | `/api/governance/*` | Config, audit logs |
| Decisions | `/api/decisions/*` | Decision records |
| Trust | `/api/trust-scores/*` | Trust scores, history, appeals |
| Veto | `/api/veto/*` | Veto, escalation, voting |
| AI Agents | `/api/projects/*/ai-agents/*` | Agent registration and management |
| AI Tasks | `/api/projects/*/ai-tasks/*` | Task assignment and callbacks |
| Bots | `/api/workspaces/*/bots` | Bot token CRUD |
| Upload | `/api/v1/upload` | File upload (multipart/form-data) |
| Webhooks | `/api/workspaces/*/webhooks/*` | Webhook CRUD and delivery log |
| Search | `/api/search` | Full-text search |
| Admin | `/api/admin/*` | User and system management |

## Documentation Map

| Document | Location | Purpose |
|----------|----------|---------|
| Project overview | `README.md` | Setup, MCP config, tool reference |
| Agent guide | `apps/mcp-server/AGENTS.md` | Coding agent workflow patterns |
| Skill package | `skills/openpr-mcp/SKILL.md` | Governed MCP skill for agents |
| API routes | `apps/api/src/main.rs` | Route registration |
| MCP tools | `apps/mcp-server/src/tools/` | Tool implementations |
| Frontend | `frontend/` | SvelteKit app |
| Migrations | `migrations/` | Database schema |

## Related Projects

| Repository | Description |
|------------|-------------|
| [openpr](https://github.com/openprx/openpr) | Core platform (this repo) |
| [openpr-webhook](https://github.com/openprx/openpr-webhook) | Webhook receiver for external integrations |
| [prx](https://github.com/openprx/prx) | AI assistant framework with built-in OpenPR MCP |
| [prx-memory](https://github.com/openprx/prx-memory) | Local-first MCP memory for coding agents |
| [wacli](https://github.com/openprx/wacli) | WhatsApp CLI with JSON-RPC daemon |

## Tech Stack

- **Backend**: Rust, Axum, SeaORM, PostgreSQL
- **Frontend**: SvelteKit, TailwindCSS, shadcn-svelte
- **MCP**: JSON-RPC 2.0 (HTTP + stdio + SSE)
- **Auth**: JWT (access + refresh) + Bot Tokens (`opr_`)
- **Deployment**: Docker Compose, Podman, Nginx

## License

MIT
