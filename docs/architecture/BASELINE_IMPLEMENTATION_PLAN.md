# Baseline Implementation Plan

## Goal
Ship an executable baseline that supports API, worker, and MCP process boot with one PostgreSQL backend.

## Current Baseline Delivered
- Rust workspace with edition 2024.
- Shared platform crate for config/logging/database.
- API service with `/health` and `/ready`.
- Worker service process scaffold.
- MCP server scaffold (HTTP and stdio bootstrap).
- PostgreSQL schema bootstrap including cache/queue/scheduler tables.
- Docker compose orchestration for local environment.

## Next Build Targets
- Domain modules and SeaORM entities.
- Queue polling and scheduler execution loop in worker.
- MCP tool routing to real application services.
- AuthN/AuthZ and role-based access control.
- Frontend app bootstrap and API integration.
