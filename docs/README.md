# Documentation Index

## 1. Product and Setup

- `../README.md` — Project overview, MCP configuration, tool reference, quick start.
- `./universal-forms-and-plugins.md` — Universal forms, WASM plugins, scenario templates, connector flow, and restaurant delivery reference.
- `./universal-forms-implementation-map.md` — Source-module, public-surface, verification-command, and status-marker map for the universal business platform.
- `./scenario-templates.md` — Catalog of built-in scenario templates, generated forms, integrations, and usage paths.
- `./universal-forms-acceptance.md` — Acceptance guide for the universal forms and restaurant reference workflow.
- `./universal-forms-production.md` — Production runbook for operating OpenPR as a generic business platform.
- `./schemas/openpr-universal-forms-readiness.schema.json` — JSON Schema for the machine-readable universal forms readiness report.
- `./schemas/openpr-universal-forms-signoff-status.schema.json` — JSON Schema for the machine-readable manual signoff progress report.
- `./schemas/openpr-universal-forms-completion-audit.schema.json` — JSON Schema for the machine-readable completion audit gate report.
- `./schemas/openpr-universal-forms-development-status.schema.json` — JSON Schema for the machine-readable development matrix status report.
- `./schemas/openpr-universal-forms-scenario-catalog.schema.json` — JSON Schema for the machine-readable scenario template catalog.
- `./schemas/openpr-universal-forms-implementation-map.schema.json` — JSON Schema for the machine-readable implementation map.
- `./schemas/openpr-universal-forms-delivery-status.schema.json` — JSON Schema for the compact one-command delivery status summary.
- `./schemas/openpr-universal-forms-delivery-manifest.schema.json` — JSON Schema for the machine-readable delivery manifest mirror.
- `./schemas/openpr-universal-forms-release-gate.schema.json` — JSON Schema for the machine-readable release gate decision.
- `./schemas/openpr-project-release-readiness.schema.json` — JSON Schema for project release readiness API/MCP responses.
- `../apps/mcp-server/AGENTS.md` — Coding agent guidelines: build, test, commit conventions.
- `./prd/OPENPR_PUBLIC_LAUNCH_AND_PRX_OFFICIAL_REGISTRATION_PLAN.md` — Master plan for public launch, private deployment profile, and PRX official registration.
- `./prd/OPENPR_PUBLIC_LAUNCH_AND_PRX_IMPLEMENTATION_ROADMAP.md` — Milestones, launch gates, 30/60/90-day roadmap, and prioritized backlog.
- `./prd/OPENPR_PUBLIC_LAUNCH_MESSAGING_AND_FAQ.md` — External announcement, homepage copy, FAQ, privacy statement, and launch checklist.
- `./prd/OPENPR_PRX_RUNTIME_API_AND_DATA_MODEL_DRAFT.md` — Runtime registration API, data model, trust, sync, and submission schema draft.

## 2. MCP Skill Package

- `../skills/openpr-mcp/SKILL.md` — Full MCP skill: workflow lines, field reference, templates.
- `../skills/openpr-mcp/scripts/mcp-regression.py` — 98-tool registry and core-surface regression test across HTTP, stdio, and SSE.
- `../skills/openpr-mcp/scripts/validate-mcp.sh` — Quick smoke test.

## 3. Source Code

- `../apps/api/` — REST API server (Axum + SeaORM).
- `../apps/mcp-server/` — MCP server (98 tools, HTTP/stdio/SSE).
- `../apps/worker/` — Background task worker.
- `../frontend/` — SvelteKit frontend.
- `../migrations/` — PostgreSQL schema migrations.

## 4. Deployment

- `../docker-compose.yml` — Full stack: API, frontend, MCP, worker, webhook, PostgreSQL.
- `../Dockerfile.prebuilt` — Production image using pre-built binaries.
