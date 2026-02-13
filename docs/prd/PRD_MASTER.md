# Plane Rust Clone PRD (Open-Source Scope + MCP)

## 1. Document Control
- Version: v1.0
- Date: 2026-02-13
- Status: Draft for implementation
- In scope: Plane open-source capabilities + MCP integration for AI tools
- Out of scope: commercial-only capabilities (SSO/SAML/SCIM, enterprise governance bundle)

## 2. Product Goals
- Build a production-grade project management platform compatible with Plane-like core workflows.
- Deliver complete API + Web + MCP capability with one shared domain model.
- Run all persistence/cache/queue/scheduling on PostgreSQL.

## 3. Personas
- Workspace Owner: manages workspace settings, members, projects.
- Project Admin: manages project config, workflows, cycles, module structure.
- Member: creates/updates work items, comments, pages.
- Viewer: read-only in allowed scope.
- AI Agent (via MCP token): performs controlled project operations under scoped ACL.

## 4. Capability Scope
- Identity and workspace membership
- Projects, work items, comments, attachments, activity stream, time logs
- Cycles, modules, milestones/epics (MVP-level)
- Views: list, kanban, calendar, timeline data endpoint
- Pages/wiki with revision history
- Intake and inbox notifications
- Search and baseline analytics dashboard
- Webhooks + CSV import/export
- MCP server tools for read/write operations

## 5. Functional Requirements
### 5.1 Workspace & Identity
- User can create and join workspace.
- Owner/Admin can invite members and assign role.
- RBAC enforced at workspace and project level.

### 5.2 Project & Work Items
- User can create project with key, name, workflow states.
- Work item fields: id, title, description, type, state, priority, assignee, labels, estimates, due date.
- Support CRUD, filtering, sorting, grouping, bulk update.
- Every mutation generates activity event.

### 5.3 Planning
- Cycles with start/end dates and scope assignment.
- Modules for thematic grouping.
- Dependencies between work items (blocking/blocked).

### 5.4 Collaboration
- Threaded comments on work item.
- Attachments metadata and object storage reference.
- Wiki pages with tree hierarchy and revisions.

### 5.5 Views
- List view: server-driven pagination/filter/sort.
- Kanban: status columns + drag-drop status update.
- Calendar: due-date and cycle date projection.
- Timeline endpoint for frontend rendering.

### 5.6 Intake, Notification, Search
- Intake entry creates triage item.
- Inbox stores user-facing events, read/unread state.
- Global search across projects, items, comments, pages.

### 5.7 Integrations
- Webhook subscription per project/workspace.
- Delivery with signing, retry, dead-letter.
- CSV import/export baseline.

### 5.8 MCP
- MCP tools expose project/item/page/search operations.
- Tool call identity mapped to internal actor + ACL enforcement.
- MCP call audit log required.

## 6. Role-Permission Matrix (Core)
- Owner: full workspace/project CRUD, member mgmt, token mgmt.
- Admin: project/work item/cycle/module/page full CRUD, no workspace deletion.
- Member: create/update own and permitted project resources, no role assignment.
- Viewer: read only.
- Agent Token: custom scoped permissions, deny by default.

## 7. Data Model Requirements
- Core entities: users, workspaces, memberships, projects, project_members, work_items, work_item_states, labels, comments, attachments, activities, cycles, modules, pages, page_revisions, notifications, webhook_subscriptions, webhook_deliveries.
- Shared fields: `id (uuid)`, `created_at`, `updated_at`, `created_by`, `updated_by`, soft-delete where needed.
- Index requirements:
  - work_items: `(project_id, state, updated_at desc)`
  - notifications: `(user_id, read_at nulls first, created_at desc)`
  - activities: `(resource_type, resource_id, created_at desc)`

## 8. API Contract Requirements
- API prefix: `/api/v1`.
- Response envelope: `{ data, meta, error }`.
- Error codes: stable machine-readable codes (e.g., `AUTH_INVALID`, `ITEM_NOT_FOUND`, `PERMISSION_DENIED`).
- Pagination: cursor-based for lists, page-based only where explicitly allowed.
- Idempotency: create endpoints accept `Idempotency-Key`.

## 9. Frontend Requirements
- App routes under `/auth/*` and `/workspace/:workspaceId/*`.
- Required pages (Phase-1): login, projects, project overview, issues list, issue detail, board, cycles, inbox, my-work.
- UX state requirements: loading/empty/error on all list/detail screens.
- Client-side access guard: auth guard + workspace membership guard.

## 10. MCP Requirements
- Transport: stdio and HTTP mode.
- Initial toolset:
  - `projects.list`, `projects.create`, `projects.get`
  - `work_items.list`, `work_items.create`, `work_items.get`, `work_items.update`
  - `work_items.add_comment`
  - `cycles.list`, `cycles.create`, `cycles.assign`
  - `pages.search`, `pages.get`, `pages.create`
  - `search.global`
- MCP response must include deterministic error object and request correlation id.

## 11. NFR (Non-Functional Requirements)
- Availability target: 99.9% for API in production.
- API latency target: P95 < 300ms for standard read endpoints under baseline load.
- Security:
  - No secret in logs.
  - Token hashing at rest.
  - Input validation on all external payloads.
- Observability:
  - Structured logs via tracing JSON.
  - Request id propagation across API/worker/MCP.
  - Metrics + health endpoints.

## 12. Testing & Quality Gates
- Mandatory: `cargo fmt -- --check`, `cargo check`, `cargo clippy -- -D warnings`, `cargo test`.
- Required suites:
  - domain unit tests
  - API integration tests
  - DB migration tests
  - MCP contract tests
  - frontend E2E for critical flows

## 13. Release & Operations
- Deploy with Docker and docker-compose for local/staging bootstrap.
- Migration strategy: forward migration + rollback script for each release.
- Backup/restore runbook for PostgreSQL.
- Incident playbook for webhook backlog and queue lag.

## 14. Milestones
- M1 (2 weeks): foundation + auth/workspace/project/item CRUD + basic web pages.
- M2 (2 weeks): board/cycles/comments/notifications + queue/scheduler.
- M3 (2 weeks): pages/search/webhook + MCP full toolset + hardening.

## 15. Acceptance Criteria
- User can complete end-to-end flow: login -> create project -> create item -> update item on board -> comment -> view inbox.
- AI agent can execute MCP flow with scoped token and full audit trail.
- CI gates pass and staging smoke tests pass.
