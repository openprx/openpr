# OpenPR

Open-source project management platform with built-in governance, AI agent integration, and MCP support.

Built with **Rust** (Axum + SeaORM), **SvelteKit**, and **PostgreSQL**.

## Features

### Project Management
- **Workspaces & Projects** — Multi-tenant workspace isolation with role-based access
- **Issues & Board** — Kanban board with drag-and-drop, priority, assignees, labels
- **Sprints & Cycles** — Sprint planning with cycle tracking
- **Full-text Search** — PostgreSQL FTS5 across issues, comments, and proposals
- **File Uploads** — Image and document attachments on issues and proposals
- **Activity Feed** — Chronological activity stream per issue
- **Notifications & Inbox** — In-app notification center with read/unread state
- **Import / Export** — Bulk data import and export

### Governance Center
- **Proposals** — Create, review, and vote on proposals with configurable approval thresholds
- **Voting System** — Weighted voting with quorum requirements
- **Decision Records** — Immutable decision log with full audit trail
- **Veto & Escalation** — Veto power with escalation voting mechanism
- **Trust Scores** — Per-user trust scoring across decision domains, with history and appeals
- **Proposal Templates** — Reusable templates for rapid proposal creation
- **Proposal Chains** — Link related proposals into decision chains
- **Impact Reviews** — Post-decision impact assessment
- **Audit Logs** — Complete governance action audit trail
- **Analytics** — Decision analytics dashboard

### AI Integration
- **AI Agents** — Register AI participants in projects with configurable roles and permissions
- **AI Tasks** — Create and assign tasks to AI agents with progress tracking and callbacks
- **AI Review** — AI-generated review feedback on proposals with learning/alignment stats
- **MCP Server** — [Model Context Protocol](https://modelcontextprotocol.io) server for AI tool integration (HTTP + stdio transport)
- **AI Callback API** — Webhook-style callbacks for task completion, failure, and progress reporting

### MCP Server

The built-in MCP server exposes OpenPR as a tool provider for AI assistants:

| Tool | Description |
|------|-------------|
| `projects.list` / `projects.get` / `projects.create` | Project CRUD |
| `work_items.list` / `work_items.get` / `work_items.create` | Issue management |
| `comments.list` / `comments.create` | Comment on issues |
| `proposals.list` / `proposals.get` / `proposals.create` | Governance proposals |
| `sprints.list` | Sprint tracking |
| `labels.list` | Label management |
| `members.list` | Team members |
| `search` | Full-text search |

Supports **HTTP** (POST `/mcp`) and **stdio** transports. Compatible with Claude, OpenClaw, OpenPRX, and other MCP-capable agents.

### Webhooks

- **Outbound Webhooks** — HTTP POST notifications on issue/proposal/comment events
- **Delivery Tracking** — Per-webhook delivery history with retry status
- **[openpr-webhook](https://github.com/openprx/openpr-webhook)** — Standalone webhook receiver for integrating OpenPR events with external systems (Slack, Discord, CI/CD, etc.)

### Admin
- **User Management** — Admin panel for user accounts, roles, bot users
- **Workspace Settings** — Configure workspace-level preferences
- **Governance Config** — Tune voting thresholds, quorum, veto rules per workspace

## Architecture

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│  Frontend    │────▶│  API Server │────▶│ PostgreSQL  │
│  (SvelteKit) │     │  (Axum)     │     │             │
└─────────────┘     └──────┬──────┘     └─────────────┘
                           │
                    ┌──────┴──────┐
                    │             │
              ┌─────▼─────┐ ┌────▼────┐
              │ MCP Server│ │ Worker  │
              │ (Tools)   │ │ (Async) │
              └───────────┘ └─────────┘
```

| Component | Port | Description |
|-----------|------|-------------|
| **API** | 8080 | REST API (Axum + SeaORM) |
| **Frontend** | 3000 | SvelteKit app (Nginx in production) |
| **MCP Server** | 8090 | MCP tool provider (HTTP/stdio) |
| **Worker** | — | Background job processor |
| **PostgreSQL** | 5432 | Primary data store |

## Quick Start

### Prerequisites

- Docker & Docker Compose
- Git

### Deploy

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
# Edit .env with your database credentials
cargo build
cargo run --bin api

# Frontend
cd frontend
cp .env.example .env
npm install
npm run dev

# MCP Server
cargo run --bin mcp-server -- --transport http --port 8090
```

## MCP Configuration

### Claude Desktop / OpenClaw

```json
{
  "mcpServers": {
    "openpr": {
      "command": "./target/release/mcp-server",
      "args": ["--transport", "stdio"],
      "env": {
        "DATABASE_URL": "postgres://openpr:openpr@localhost:5432/openpr"
      }
    }
  }
}
```

### HTTP Mode

```bash
# Start MCP server
cargo run --bin mcp-server -- --transport http --port 8090

# Test
curl -X POST http://localhost:8090/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

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
| AI Agents | `/api/projects/*/ai-agents/*` | Register and manage AI agents |
| AI Tasks | `/api/projects/*/ai-tasks/*` | Task assignment and callbacks |
| Webhooks | `/api/workspaces/*/webhooks/*` | Webhook CRUD and delivery log |
| Search | `/api/search` | Full-text search |
| Admin | `/api/admin/*` | User and system management |

## Related Projects

| Repository | Description |
|------------|-------------|
| [openpr](https://github.com/openprx/openpr) | Core platform (this repo) |
| [openpr-webhook](https://github.com/openprx/openpr-webhook) | Webhook receiver for external integrations |
| [openprx](https://github.com/openprx/openprx) | AI assistant framework with built-in OpenPR MCP support |

## Tech Stack

- **Backend**: Rust, Axum, SeaORM, PostgreSQL
- **Frontend**: SvelteKit, TailwindCSS, shadcn-svelte
- **MCP**: JSON-RPC 2.0 (HTTP + stdio)
- **Auth**: JWT (access + refresh tokens)
- **Deployment**: Docker Compose, Nginx

## License

MIT
