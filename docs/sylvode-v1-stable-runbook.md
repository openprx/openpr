# Sylvode Flow v1.0 stable operations and rollback runbook

This runbook is the executable checklist for a staged Flow rollout. The named people are recorded only by the external `release_owner`, `rollback_owner`, `on_call_runbook`, and `stable_contract_approval` sign-off rows; this document does not self-assign or self-sign them.

## Release preparation and staged rollout

1. Freeze the source commit, migration version, release binary SHA-256 values, dashboard snapshot, and database restore point.
2. Rehearse migration, backup restore, and rollback against a production copy. Compare document fingerprints, head sequence/frontier, projections, relations, lineage, and audit events.
3. Deploy API, worker, and MCP before Web. Confirm protocol capability negotiation and the old-client window.
4. Enable the Flow feature flag for one internal workspace, then expand by workspace. Never enable all workspaces in one step.
5. Stop rollout on any data-integrity alarm, error-budget exhaustion, unexplained rejected-update increase, or projection/search lag budget breach.

## Sync degraded or hot document

Check accepted/rejected updates, coordinator timeout rate, connections, round-trip p50/p95/p99, row-lock wait/hold, WAL commit time, queue depth, and slow-consumer disconnects. Quiesce expansion of the affected workspace. If lock hold is within budget but latency rises with client count, treat it as coordinator throughput/queueing; do not raise the frozen latency budget. Capture all PostgreSQL collector files before rotation. Drain or isolate the hot workspace and preserve rejected update IDs for replay.

## Projection or search lag

Pause rollout and record accepted head sequence/frontier against projection and search frontiers. Stop the projection worker only after its lease is visible, restart it, then run the authorized projection rebuild. Verify frontier equality and sampled semantic hashes before resuming. Never repair canonical document state from a projection.

## Corruption quarantine and forced resync

On checksum, invalid-update, or non-contiguous-tail evidence, stop writes for the document and quarantine its snapshot and update tail without rewriting either. Preserve the audit correlation ID. Restore from the last verified snapshot plus contiguous updates. Force clients to resync only after checksum, head sequence/frontier, semantic hash, and projection equality all pass; never acknowledge the corrupt update.

## Backup restore

Record the source database identifier and backup SHA-256, restore into a fresh database name, run migrations, and compare document fingerprints and object/package exports. Verify retained update tails, relations, lineage, event dispatch, and authorization epochs. Keep the source untouched until the external rollback owner accepts the comparison.

## Token or permission incident

Revoke the credential, bump the authorization epoch, close affected sessions, and remove presence entries. Confirm REST, WebSocket, MCP, CLI, search, export, relation, and ticket reads reauthorize after the final epoch check. Preserve redacted audit evidence; never place token material in logs.

## Rollback procedure

The externally signed rollback owner decides rollback. Stop feature-flag expansion, quiesce new Flow writes, drain API/worker/MCP, preserve the failed release receipt, and restore the recorded rollback point. Start the previous binaries with their supported schema, verify old-client REST/MCP access and exact data fingerprints, then reopen traffic by workspace. Forward-only migrations are not deleted; if the previous binary cannot read the migrated schema, keep traffic stopped and restore the pre-migration database backup.

## Exit and evidence

Resume only when integrity checks pass, the triggering metric is within its approved budget, queues drain, and an external on-call reviewer records the incident evidence. Attach command exits, exact test counts, ignored tests, artifact hashes, remaining failures, and rollback decision to the release receipt.
