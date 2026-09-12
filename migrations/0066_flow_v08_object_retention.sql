-- Explicit eligibility marker for irreversible object cleanup.

ALTER TABLE flow_objects
  ADD COLUMN IF NOT EXISTS permanent_cleanup_after TIMESTAMPTZ;

ALTER TABLE flow_objects
  DROP CONSTRAINT IF EXISTS flow_objects_permanent_cleanup_archived_check,
  ADD CONSTRAINT flow_objects_permanent_cleanup_archived_check
    CHECK (permanent_cleanup_after IS NULL OR lifecycle_status = 'archived');

CREATE INDEX IF NOT EXISTS idx_flow_objects_permanent_cleanup
  ON flow_objects(permanent_cleanup_after, id)
  WHERE permanent_cleanup_after IS NOT NULL AND lifecycle_status = 'archived';

COMMENT ON COLUMN flow_objects.permanent_cleanup_after IS
  'Set only by an archive whose derived ADR-0012 tier is full_access; ordinary edit-tier soft archives remain reversible indefinitely';
