# Flow v0.8 backup and restore

Use a maintenance window: stop API writes, compaction, retention, and import promotion before the
source fingerprint and keep them stopped through `pg_dump`. Object/package exports are a separate
portable recovery layer; the PostgreSQL drill below verifies the complete canonical database.

Set `OPENPR_BACKUP_SOURCE_DATABASE_URL` to the database being backed up and
`OPENPR_BACKUP_RESTORE_ADMIN_URL` to that cluster's `postgres` database. Credentials belong in the
environment or a PostgreSQL password file, never in the repository. The verifier only permits a
disposable restore database whose name starts with `v08_restore_`, drops it on exit, and keeps its
dump and comparison material under `/opt/worker/.cache`.

```bash
export OPENPR_BACKUP_SOURCE_DATABASE_URL='postgresql://.../openpr'
export OPENPR_BACKUP_RESTORE_ADMIN_URL='postgresql://.../postgres'
export OPENPR_BACKUP_RESTORE_DATABASE_NAME='v08_restore_drill'
scripts/verify-flow-backup-restore-v0.8.sh evidence/v0.8/backup-restore-result.json
```

The command takes a checksummed PostgreSQL plain-SQL backup, restores it with one transaction and
`ON_ERROR_STOP`, and
independently reconstructs every collaboration document on both sides. It fails unless the ordered
sets are non-empty and exactly equal by document id, head sequence, head frontier, semantic hash,
and projection sequence. Snapshot checksums and every retained update's checksum/frontier chain are
validated while producing those fingerprints. The result records restore time, measured clock
resolution, exact document count, dump checksum, and zero document loss. A duration within twice
the measured clock resolution is marked `inconclusive_below_instrument_resolution`.

After a production restore, keep write traffic disabled until this comparison passes, run the
scoped projection/search rebuild and document verification dry-runs, then re-enable workers before
APIs. Preserve the dump checksum, result JSON, operator timeline, and signed manual review as the
restore evidence. Never treat a successful `pg_restore` alone as proof of Flow recovery.
