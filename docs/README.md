# Documentation Index

## 1. Product and Setup

- `../README.md` — Project overview, MCP configuration, tool reference, quick start.
- `../apps/mcp-server/AGENTS.md` — Coding agent guidelines: build, test, commit conventions.

## 2. MCP Skill Package

- `../skills/openpr-mcp/SKILL.md` — Full MCP skill: workflow lines, field reference, templates.
- `../skills/openpr-mcp/scripts/mcp-regression.py` — 34-tool × 3-protocol regression test.
- `../skills/openpr-mcp/scripts/validate-mcp.sh` — Quick smoke test.

## 3. Source Code

- `../apps/api/` — REST API server (Axum + SeaORM).
- `../apps/mcp-server/` — MCP server (34 tools, HTTP/stdio/SSE).
- `../apps/worker/` — Background task worker.
- `../frontend/` — SvelteKit frontend.
- `../migrations/` — PostgreSQL schema migrations.

## 4. Deployment

- `../docker-compose.yml` — Full stack: API, frontend, MCP, worker, webhook, PostgreSQL.
- `../Dockerfile.prebuilt` — Production image using pre-built binaries.
