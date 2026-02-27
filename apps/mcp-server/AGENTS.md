# Repository Guidelines

## Project Overview
- OpenPR is an open-source project management platform with governance and AI integration.
- The MCP server exposes 34 tools for managing projects, issues, sprints, labels, comments, proposals, and files.
- Transports: HTTP (`POST /mcp/rpc`), stdio (stdin/stdout), SSE (`GET /sse` + `POST /messages`).

## MCP Surface (Quick Reference)
- Transports:
  - `stdio` (default for Claude Desktop, Codex, local CLI)
  - `HTTP` (web integrations, OpenClaw plugins)
  - `SSE` (streaming clients; also available on HTTP port)
- Core tools:
  - `projects.list`, `projects.get`, `projects.create`, `projects.update`, `projects.delete`
  - `work_items.list`, `work_items.get`, `work_items.get_by_identifier`, `work_items.create`, `work_items.update`, `work_items.delete`, `work_items.search`
  - `work_items.add_label`, `work_items.add_labels`, `work_items.remove_label`, `work_items.list_labels`
  - `comments.create`, `comments.list`, `comments.delete`
  - `files.upload`
  - `labels.list`, `labels.list_by_project`, `labels.create`, `labels.update`, `labels.delete`
  - `sprints.list`, `sprints.create`, `sprints.update`, `sprints.delete`
  - `proposals.list`, `proposals.get`, `proposals.create`
  - `members.list`, `search.all`
- Authentication: Bot token via `OPENPR_BOT_TOKEN` env var (prefix `opr_`).
- Skill package: `skills/openpr-mcp/SKILL.md`

## Project Structure & Module Organization
- `apps/api/` — Axum REST API server
- `apps/mcp-server/` — MCP server (tools, client, transport)
  - `src/tools/` — Tool implementations (one file per domain)
  - `src/client/` — HTTP client to API server (including file upload)
  - `src/server.rs` — MCP request dispatcher
  - `src/main.rs` — Transport setup (HTTP, stdio, SSE)
- `apps/worker/` — Background task worker
- `frontend/` — SvelteKit frontend
- `migrations/` — PostgreSQL migrations (SeaORM)
- `skills/` — MCP skill packages for AI agents

## Build, Test, and Development Commands
```bash
# Build
source ~/.cargo/env
cargo build --release --bin api
cargo build --release --bin mcp-server

# Frontend
cd frontend && bun run build  # or npm run build

# Format and lint
cargo fmt
cargo clippy --all-targets -- -D warnings

# Deploy (local)
podman-compose down && podman-compose up -d --build

# Test MCP
curl -X POST http://localhost:8090/mcp/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

## Coding Style & Naming Conventions
- Rust 2021 idioms, 4-space indentation.
- File/module: `snake_case`. Types: `PascalCase`. Functions: `snake_case`. Constants: `SCREAMING_SNAKE_CASE`.
- MCP tool names: `domain.action` (e.g. `work_items.create`, `labels.update`).
- Tool implementations: one file per domain in `apps/mcp-server/src/tools/`.
- Frontend: SvelteKit conventions, components in `src/lib/components/`.
- Run `cargo fmt` before committing.

## Testing Guidelines
- MCP regression: test all 34 tools across 3 transports (HTTP, stdio, SSE).
- API: test via `curl` or MCP client against running instance.
- Frontend: `bun run build` must succeed.
- When adding a new MCP tool:
  1. Add tool definition in `src/tools/<domain>.rs`
  2. Register in `src/tools/mod.rs`
  3. Add dispatch in `src/server.rs`
  4. Add client method in `src/client/mod.rs` if needed
  5. Test via all three transports

## Commit & Pull Request Guidelines
- Conventional Commits:
  - `feat: add files.upload MCP tool`
  - `fix: PATCH→PUT method mismatch in work_items.update`
  - `docs: update MCP tool reference in README`
- PRs should include: problem/solution summary, test evidence, config changes.

## Security & Configuration
- Never commit bot tokens or API keys.
- Environment variables for all secrets (`OPENPR_BOT_TOKEN`, `DATABASE_URL`).
- Bot tokens are workspace-scoped; each creates a `bot_mcp` user for audit integrity.
- File uploads: server-side type validation and size limits.
