# Documentation Index

## 1. Product and Setup

- `../README.md` — Project overview, MCP configuration, tool reference, quick start.
- `../apps/mcp-server/AGENTS.md` — Coding agent guidelines: build, test, commit conventions.
- `./prd/OPENPR_PUBLIC_LAUNCH_AND_PRX_OFFICIAL_REGISTRATION_PLAN.md` — Master plan for public launch, private deployment profile, and PRX official registration.
- `./prd/OPENPR_PUBLIC_LAUNCH_AND_PRX_IMPLEMENTATION_ROADMAP.md` — Milestones, launch gates, 30/60/90-day roadmap, and prioritized backlog.
- `./prd/OPENPR_PUBLIC_LAUNCH_MESSAGING_AND_FAQ.md` — External announcement, homepage copy, FAQ, privacy statement, and launch checklist.
- `./prd/OPENPR_PRX_RUNTIME_API_AND_DATA_MODEL_DRAFT.md` — Runtime registration API, data model, trust, sync, and submission schema draft.

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
