# WS3 Data & Platform PRD Path

## Scope
- PostgreSQL schema, migrations, queue/scheduler/cache patterns, worker model.

## Deliverables
- Table/index/constraint specification.
- Migration and rollback policy.
- Queue processing semantics (`SKIP LOCKED`, retry, DLQ).
- Scheduler execution and idempotency policy.

## Exit Criteria
- All core queries have index strategy.
- Data lifecycle and retention rules are documented.
