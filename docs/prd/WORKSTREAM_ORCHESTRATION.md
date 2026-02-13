# PRD Sub-Agent Workstream Orchestration

## Objective
Define parallel planning paths (sub-agents) to produce a full, implementation-ready PRD and execution package.

## Sub-Agent Paths
1. `SA-1 Product/Domain` (owner: Product Architect)
- Owns business rules, state machines, role matrix, acceptance criteria.
- Output: domain rules and user story pack.

2. `SA-2 Backend/API` (owner: Backend Lead)
- Owns API contracts, service boundaries, error model, MCP integration points.
- Output: OpenAPI draft, endpoint matrix, error code registry.

3. `SA-3 Data/Platform` (owner: Platform Lead)
- Owns schema/indexes, migrations, PostgreSQL cache/queue/scheduler design.
- Output: ERD spec, migration plan, worker semantics.

4. `SA-4 Frontend/UX` (owner: Frontend Lead)
- Owns page IA, routes, component responsibilities, interaction states.
- Output: route map, screen contract, API binding matrix.

5. `SA-5 QA/Security/Release` (owner: QA+SRE)
- Owns test strategy, quality gates, NFR, release and rollback process.
- Output: test plan, NFR verification plan, runbooks.

## Parallelization Rules
- SA-1 defines canonical terms first; others reuse exact glossary.
- SA-2/SA-3 run in parallel after SA-1 baseline.
- SA-4 can start with stable route/feature list from SA-1.
- SA-5 starts once SA-2/SA-3 draft interfaces exist.

## Merge Gates
- Gate A: shared glossary + role matrix approved.
- Gate B: API schema and DB schema cross-reviewed.
- Gate C: frontend contract aligned with API error/pagination.
- Gate D: test and release criteria attached to every milestone.

## Cadence
- Daily sync: 15 min, blocker only.
- Twice-weekly spec review: decision log mandatory.
- Weekly baseline freeze: update PRD version and changelog.
