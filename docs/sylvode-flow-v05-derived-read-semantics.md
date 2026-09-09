# Sylvode Flow v0.5 derived-read semantics

## Projection-lag aggregate scope

`GET /api/v1/workspaces/{workspace_id}/flow/projection-lag` returns a page in `items`, but
`max_lag` and `p95_lag` are not page statistics. They cover the complete policy-visible object
scope selected by `workspace_id` and the optional `project_id`, before cursor/page slicing.
Consequently, following `next_cursor` changes `items` but does not make either aggregate jump.

This interpretation follows `contracts/mcp-surface-v1.md`, which calls the fields
“policy-filtered max/p95”, rather than “page max/p95”. Candidate rows are authorized before they
can contribute. The full scan is bounded by the frozen `authorized_scan_rows_max=1000`; crossing
that ceiling fails with `limit_exceeded`/`scan_budget` instead of returning a partial aggregate.
The response deliberately exposes no pre-policy total, examined-row count, or hidden-object
metadata.

## Diff replay boundary

`GET /api/v1/flow/objects/{object_id}/diff` limits retained-history reads to 1001 rows and rejects
the request when the frozen 1000-row scan budget would be crossed. It then performs snapshot
load, tail replay, historical forks, semantic snapshots, and semantic diff in the killable
isolated worker under the frozen decode/apply CPU, wall-clock, and memory ceilings.

The Flow contracts do not freeze a diff-history-specific row budget. Reusing the existing scan
budget is therefore an implementation safety bound, not a claim that a new contractual value was
approved. Contract owners should freeze a dedicated history-replay row/byte budget if this
endpoint must support more than 1000 retained updates in one request.
