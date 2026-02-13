# OpenPR (Plane-like Rust Platform)

This repository bootstraps a full-stack project management platform inspired by Plane open-source capabilities, implemented in Rust.

## Stack
- Rust edition 2024
- Axum (API + MCP HTTP transport)
- SeaORM + PostgreSQL
- Tokio runtime
- Tracing JSON logs
- Docker / docker-compose

## Services
- `api`: REST API and health/readiness endpoints.
- `worker`: asynchronous background worker (queue/scheduler processor placeholder).
- `mcp-server`: MCP tool server (HTTP + stdio transport scaffold).

## Quick Start
1. Copy env file:
```sh
cp .env.example .env
```
2. Start PostgreSQL:
```sh
./scripts/dev-up.sh
```
3. Run API locally:
```sh
DATABASE_URL=postgres://openpr:openpr@localhost:5432/openpr cargo run -p api
```
4. Validate workspace:
```sh
./scripts/dev-check.sh
```

## MSRV
- Managed through `rust-toolchain.toml` (`stable`).

## Repo Layout
- `apps/`: binaries (`api`, `worker`, `mcp-server`)
- `crates/platform`: shared config/logging/errors/state
- `migrations/`: SQL migrations and bootstrap schema
- `docs/prd/`: full PRD and workstream planning docs
- `log/`: task/done/changelog tracing records
