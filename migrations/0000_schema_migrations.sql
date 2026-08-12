-- Migration ledger.
--
-- Before this table existed the runner replayed every migration file on every API start and
-- downgraded any failure to a warning ("likely already applied"). Two consequences:
--   * a partially applied schema was indistinguishable from a healthy one, and
--   * every non idempotent statement ran again on each restart (0048 used to DELETE rows).
-- Each migration is now executed at most once and its outcome is recorded here, so operators can
-- answer "what is applied" and "what broke" with a single query.
--
-- status:
--   applied - executed by this runner and committed
--   adopted - already present in a database created before the ledger existed, never executed
--   failed  - execution failed; API startup aborts and the row keeps the error for the operator
CREATE TABLE IF NOT EXISTS schema_migrations (
    name       TEXT PRIMARY KEY,
    checksum   TEXT NOT NULL,
    status     TEXT NOT NULL DEFAULT 'applied',
    error      TEXT,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Added separately so a ledger created by an earlier build of this file also gains the check.
DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1
    FROM pg_constraint
    WHERE conname = 'schema_migrations_status_check'
      AND conrelid = 'public.schema_migrations'::regclass
  ) THEN
    ALTER TABLE schema_migrations
      ADD CONSTRAINT schema_migrations_status_check
      CHECK (status IN ('applied', 'adopted', 'failed'));
  END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_schema_migrations_status
  ON schema_migrations(status)
  WHERE status <> 'applied';
