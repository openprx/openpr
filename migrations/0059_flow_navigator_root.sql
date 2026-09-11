-- Sylvode Flow v0.6 / ADR-0018 NR-1..NR-4: materialize the addressable workspace navigator
-- root and make NULL-parent non-navigator rows impossible.
--
-- The canonical workspace root is the unprojected navigator row:
--   object_type = 'navigator', project_id IS NULL, parent_id IS NULL.
-- Project-scoped navigator documents remain separate ordering documents used by v0.5 moves; the
-- partial unique index below therefore targets the canonical unprojected root, not every
-- project-scoped navigator.

DO $$
DECLARE
  duplicate_workspace UUID;
  duplicate_count BIGINT;
  incompatible_workspace UUID;
  incompatible_count BIGINT;
BEGIN
  SELECT workspace_id, count(*) INTO duplicate_workspace, duplicate_count
    FROM flow_objects
   WHERE object_type = 'navigator'
     AND project_id IS NULL
     AND parent_id IS NULL
   GROUP BY workspace_id
  HAVING count(*) > 1
   ORDER BY workspace_id
   LIMIT 1;

  IF duplicate_workspace IS NOT NULL THEN
    RAISE EXCEPTION
      'flow_objects: workspace % has % unprojected navigator roots; ADR-0018 NR-2 requires repair before root materialization',
      duplicate_workspace, duplicate_count;
  END IF;

  -- An unprojected root cannot parent a projected object because
  -- flow_objects_parent_project_fk requires strict project-scope equality. Changing project_id
  -- would silently change governance scope, so the migration deliberately refuses that history.
  SELECT workspace_id, count(*) INTO incompatible_workspace, incompatible_count
    FROM flow_objects
   WHERE parent_id IS NULL
     AND object_type <> 'navigator'
     AND project_id IS NOT NULL
   GROUP BY workspace_id
   ORDER BY workspace_id
   LIMIT 1;

  IF incompatible_workspace IS NOT NULL THEN
    RAISE EXCEPTION
      'flow_objects: workspace % has % projected non-navigator root object(s); ADR-0018 NR-2 cannot attach them to the NULL-scope navigator root without violating project scope',
      incompatible_workspace, incompatible_count;
  END IF;
END
$$;

CREATE UNIQUE INDEX IF NOT EXISTS flow_objects_workspace_navigator_root_key
  ON flow_objects (workspace_id)
  WHERE object_type = 'navigator' AND project_id IS NULL AND parent_id IS NULL;

-- Use one known-good empty Loro document for roots created by the migration. The bytes were
-- emitted by collab_core::LoroCollabEngine::new_empty(1).export_snapshot(); the frontier is the
-- engine's empty frontier. Reusing the immutable initial bytes is safe because every document has
-- its own identity and future updates carry their own actor/change ids.
DO $$
DECLARE
  workspace_row RECORD;
  root_id UUID;
  document_id UUID;
  empty_snapshot BYTEA := decode(
    '6c6f726f0000000000000000000000003ba2f83500032f0000004c4f524f0000000200767600000001000200e762c96c0100000005000000020066720002007676dbd8c9c816000000550000004c4f524f00000100000000000600830474726565030100000402000005000000000100050102000100000000060002007154a5550100000005000000060080046d6574610006008304747265659faa35f13400000000000000',
    'hex'
  );
  empty_frontier BYTEA := decode('00', 'hex');
BEGIN
  FOR workspace_row IN SELECT id FROM workspaces ORDER BY id LOOP
    SELECT id INTO root_id
      FROM flow_objects
     WHERE workspace_id = workspace_row.id
       AND object_type = 'navigator'
       AND project_id IS NULL
       AND parent_id IS NULL;

    IF root_id IS NULL THEN
      root_id := gen_random_uuid();
      document_id := gen_random_uuid();

      INSERT INTO flow_objects (
        id, workspace_id, project_id, object_type, parent_id,
        inherit_from_parent, governance_metadata, lifecycle_status
      ) VALUES (
        root_id, workspace_row.id, NULL, 'navigator', NULL,
        true, '{"system_role":"workspace_navigator_root"}'::jsonb, 'active'
      );

      INSERT INTO collab_documents (
        id, object_id, engine, format_version, snapshot, snapshot_frontier, snapshot_seq,
        head_frontier, head_seq, byte_count, update_count
      ) VALUES (
        document_id, root_id, 'loro', 'loro-1', empty_snapshot, empty_frontier, 0,
        empty_frontier, 0, octet_length(empty_snapshot), 0
      );

      INSERT INTO flow_object_projections (
        object_id, document_seq, document_frontier, title, state, plain_text,
        projection_version
      ) VALUES (
        root_id, 0, empty_frontier, '', '{"nodes":{}}'::jsonb, '', 1
      );
    END IF;
  END LOOP;
END
$$;

-- Only NULL-scope legacy top-level objects are compatible with the canonical NULL-scope root.
-- The preflight above rejected every projected non-navigator root before this statement can make
-- any change, so migration failure is atomic and leaves no half-reparented workspace.
UPDATE flow_objects child
   SET parent_id = root.id,
       updated_at = now()
  FROM flow_objects root
 WHERE child.workspace_id = root.workspace_id
   AND child.parent_id IS NULL
   AND child.object_type <> 'navigator'
   AND child.project_id IS NULL
   AND root.object_type = 'navigator'
   AND root.project_id IS NULL
   AND root.parent_id IS NULL;

ALTER TABLE flow_objects
  DROP CONSTRAINT IF EXISTS flow_objects_non_navigator_parent_check;

ALTER TABLE flow_objects
  ADD CONSTRAINT flow_objects_non_navigator_parent_check
  CHECK (object_type = 'navigator' OR parent_id IS NOT NULL);

-- Migration-time backfill is not enough: a workspace inserted after this file ran must have the
-- same complete root aggregate before the workspace INSERT commits. Keeping this in an AFTER
-- trigger makes direct SQL writers and future workspace creation surfaces obey NR-1 too; a
-- failure to create any of the three rows aborts the workspace INSERT rather than leaving a
-- partially initialized workspace.
CREATE OR REPLACE FUNCTION flow_create_workspace_navigator_root()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  root_id UUID := gen_random_uuid();
  document_id UUID := gen_random_uuid();
  empty_snapshot BYTEA := decode(
    '6c6f726f0000000000000000000000003ba2f83500032f0000004c4f524f0000000200767600000001000200e762c96c0100000005000000020066720002007676dbd8c9c816000000550000004c4f524f00000100000000000600830474726565030100000402000005000000000100050102000100000000060002007154a5550100000005000000060080046d6574610006008304747265659faa35f13400000000000000',
    'hex'
  );
  empty_frontier BYTEA := decode('00', 'hex');
BEGIN
  INSERT INTO flow_objects (
    id, workspace_id, project_id, object_type, parent_id,
    inherit_from_parent, governance_metadata, lifecycle_status
  ) VALUES (
    root_id, NEW.id, NULL, 'navigator', NULL,
    true, '{"system_role":"workspace_navigator_root"}'::jsonb, 'active'
  );

  INSERT INTO collab_documents (
    id, object_id, engine, format_version, snapshot, snapshot_frontier, snapshot_seq,
    head_frontier, head_seq, byte_count, update_count
  ) VALUES (
    document_id, root_id, 'loro', 'loro-1', empty_snapshot, empty_frontier, 0,
    empty_frontier, 0, octet_length(empty_snapshot), 0
  );

  INSERT INTO flow_object_projections (
    object_id, document_seq, document_frontier, title, state, plain_text, projection_version
  ) VALUES (
    root_id, 0, empty_frontier, '', '{"nodes":{}}'::jsonb, '', 1
  );

  RETURN NEW;
END
$$;

DROP TRIGGER IF EXISTS workspaces_create_flow_navigator_root ON workspaces;
CREATE TRIGGER workspaces_create_flow_navigator_root
AFTER INSERT ON workspaces
FOR EACH ROW EXECUTE FUNCTION flow_create_workspace_navigator_root();

CREATE OR REPLACE VIEW flow_object_navigator_root_violations AS
  SELECT NULL::uuid AS object_id,
         workspace_row.id AS workspace_id,
         'navigator'::text AS object_type,
         NULL::uuid AS project_id,
         NULL::uuid AS parent_id,
         'missing_workspace_navigator_root'::text AS violation
    FROM workspaces workspace_row
   WHERE NOT EXISTS (
     SELECT 1
       FROM flow_objects root
      WHERE root.workspace_id = workspace_row.id
        AND root.object_type = 'navigator'
        AND root.project_id IS NULL
        AND root.parent_id IS NULL
   )
  UNION ALL
  SELECT object_row.id AS object_id,
         object_row.workspace_id,
         object_row.object_type,
         object_row.project_id,
         object_row.parent_id,
         'non_navigator_without_parent'::text AS violation
    FROM flow_objects object_row
   WHERE object_row.object_type <> 'navigator'
     AND object_row.parent_id IS NULL
  UNION ALL
  SELECT (array_agg(root.id ORDER BY root.id))[1] AS object_id,
         root.workspace_id,
         'navigator'::text AS object_type,
         NULL::uuid AS project_id,
         NULL::uuid AS parent_id,
         'multiple_workspace_navigator_roots'::text AS violation
    FROM flow_objects root
   WHERE root.object_type = 'navigator'
     AND root.project_id IS NULL
     AND root.parent_id IS NULL
   GROUP BY root.workspace_id
  HAVING count(*) > 1;

COMMENT ON VIEW flow_object_navigator_root_violations IS
  'ADR-0018 NR-2/NR-4 replayable monitor. Empty means every root shape satisfies the database invariants.';
