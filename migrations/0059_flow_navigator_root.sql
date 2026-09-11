-- Sylvode Flow v0.6 / ADR-0018 NR-1..NR-4: materialize one addressable navigator
-- root per (workspace, project scope), reparent every legacy top-level object without changing
-- its scope, and make NULL-parent non-root rows impossible.

DO $$
DECLARE
  duplicate_workspace UUID;
  duplicate_project UUID;
  duplicate_count BIGINT;
BEGIN
  SELECT workspace_id, project_id, count(*)
    INTO duplicate_workspace, duplicate_project, duplicate_count
    FROM flow_objects
   WHERE object_type = 'navigator'
     AND parent_id IS NULL
   GROUP BY workspace_id, project_id
  HAVING count(*) > 1
   ORDER BY workspace_id, project_id NULLS FIRST
   LIMIT 1;

  IF duplicate_workspace IS NOT NULL THEN
    RAISE EXCEPTION
      'flow_objects: workspace % project scope % has % navigator roots; ADR-0018 NR-2 requires repair before root materialization',
      duplicate_workspace, COALESCE(duplicate_project::text, 'NULL'), duplicate_count;
  END IF;
END
$$;

-- NULL and non-NULL project scopes need separate partial indexes so the uniqueness rule works on
-- every PostgreSQL version supported by this repository (without relying on NULLS NOT DISTINCT).
CREATE UNIQUE INDEX IF NOT EXISTS flow_objects_workspace_navigator_root_key
  ON flow_objects (workspace_id)
  WHERE object_type = 'navigator' AND project_id IS NULL AND parent_id IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS flow_objects_project_navigator_root_key
  ON flow_objects (workspace_id, project_id)
  WHERE object_type = 'navigator' AND project_id IS NOT NULL AND parent_id IS NULL;

-- API list/search/frontier queries and the projection worker call this database predicate rather
-- than maintaining independent marker/shape predicates in separate crates.
CREATE OR REPLACE FUNCTION flow_is_system_navigator_root(
  object_type_value TEXT,
  parent_id_value UUID,
  governance_metadata_value JSONB
)
RETURNS BOOLEAN
LANGUAGE sql
IMMUTABLE
PARALLEL SAFE
AS $$
  SELECT object_type_value = 'navigator'
     AND parent_id_value IS NULL
     AND governance_metadata_value->>'system_role' = 'workspace_navigator_root'
$$;

-- Every root creation path, including the public object-create compatibility path, receives the
-- same marker. Conversely, a caller cannot hide an ordinary object by supplying the marker.
CREATE OR REPLACE FUNCTION flow_mark_system_navigator_root()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF NEW.object_type = 'navigator' AND NEW.parent_id IS NULL THEN
    NEW.governance_metadata := COALESCE(NEW.governance_metadata, '{}'::jsonb)
      || '{"system_role":"workspace_navigator_root"}'::jsonb;
  ELSIF NEW.governance_metadata->>'system_role' = 'workspace_navigator_root' THEN
    RAISE EXCEPTION
      'flow_objects: system navigator root marker requires object_type=navigator and parent_id IS NULL';
  END IF;
  RETURN NEW;
END
$$;

DROP TRIGGER IF EXISTS flow_objects_mark_system_navigator_root ON flow_objects;
CREATE TRIGGER flow_objects_mark_system_navigator_root
BEFORE INSERT OR UPDATE OF object_type, parent_id, governance_metadata ON flow_objects
FOR EACH ROW EXECUTE FUNCTION flow_mark_system_navigator_root();

-- Migration and runtime both call this function, so adoption, marking and aggregate creation have
-- one implementation. The immutable initial bytes were emitted by
-- collab_core::LoroCollabEngine::new_empty(1).export_snapshot().
CREATE OR REPLACE FUNCTION flow_ensure_navigator_root(
  requested_workspace_id UUID,
  requested_project_id UUID
)
RETURNS UUID
LANGUAGE plpgsql
AS $$
DECLARE
  root_id UUID;
  empty_snapshot BYTEA := decode(
    '6c6f726f0000000000000000000000003ba2f83500032f0000004c4f524f0000000200767600000001000200e762c96c0100000005000000020066720002007676dbd8c9c816000000550000004c4f524f00000100000000000600830474726565030100000402000005000000000100050102000100000000060002007154a5550100000005000000060080046d6574610006008304747265659faa35f13400000000000000',
    'hex'
  );
  empty_frontier BYTEA := decode('00', 'hex');
BEGIN
  IF NOT EXISTS (SELECT 1 FROM workspaces WHERE id = requested_workspace_id) THEN
    RAISE EXCEPTION 'flow_objects: navigator root workspace % does not exist', requested_workspace_id;
  END IF;

  IF requested_project_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM projects
     WHERE id = requested_project_id AND workspace_id = requested_workspace_id
  ) THEN
    RAISE EXCEPTION
      'flow_objects: navigator root project % does not belong to workspace %',
      requested_project_id, requested_workspace_id;
  END IF;

  SELECT id INTO root_id
    FROM flow_objects
   WHERE workspace_id = requested_workspace_id
     AND project_id IS NOT DISTINCT FROM requested_project_id
     AND object_type = 'navigator'
     AND parent_id IS NULL;

  IF root_id IS NOT NULL THEN
    -- This no-op-looking update intentionally passes adopted legacy roots through the one marker
    -- trigger above while preserving every unrelated governance_metadata key.
    UPDATE flow_objects
       SET governance_metadata = governance_metadata
     WHERE id = root_id;
    RETURN root_id;
  END IF;

  WITH inserted_root AS (
    INSERT INTO flow_objects (
      id, workspace_id, project_id, object_type, parent_id,
      inherit_from_parent, governance_metadata, lifecycle_status
    ) VALUES (
      gen_random_uuid(), requested_workspace_id, requested_project_id, 'navigator', NULL,
      true, '{}'::jsonb, 'active'
    )
    ON CONFLICT DO NOTHING
    RETURNING id
  ), inserted_document AS (
    INSERT INTO collab_documents (
      id, object_id, engine, format_version, snapshot, snapshot_frontier, snapshot_seq,
      head_frontier, head_seq, byte_count, update_count
    )
    SELECT gen_random_uuid(), id, 'loro', 'loro-1', empty_snapshot, empty_frontier, 0,
           empty_frontier, 0, octet_length(empty_snapshot), 0
      FROM inserted_root
  ), inserted_projection AS (
    INSERT INTO flow_object_projections (
      object_id, document_seq, document_frontier, title, state, plain_text,
      projection_version
    )
    SELECT id, 0, empty_frontier, '', '{"nodes":{}}'::jsonb, '', 1
      FROM inserted_root
  )
  SELECT id INTO root_id FROM inserted_root;

  IF root_id IS NOT NULL THEN
    RETURN root_id;
  END IF;

  -- A concurrent caller may have won either partial unique index. A new statement sees that
  -- committed winner and also repairs its marker before returning it.
  SELECT id INTO root_id
    FROM flow_objects
   WHERE workspace_id = requested_workspace_id
     AND project_id IS NOT DISTINCT FROM requested_project_id
     AND object_type = 'navigator'
     AND parent_id IS NULL;
  IF root_id IS NULL THEN
    RAISE EXCEPTION
      'flow_objects: navigator root materialization produced no root for workspace % project scope %',
      requested_workspace_id, COALESCE(requested_project_id::text, 'NULL');
  END IF;
  UPDATE flow_objects SET governance_metadata = governance_metadata WHERE id = root_id;
  RETURN root_id;
END
$$;

-- Materialize both the unprojected scope and every existing project scope. Existing navigator
-- rows are adopted in place; their document/projection contents and ids are preserved.
DO $$
DECLARE
  scope_row RECORD;
BEGIN
  FOR scope_row IN
    SELECT workspace_id, project_id
      FROM (
        SELECT id AS workspace_id, NULL::uuid AS project_id FROM workspaces
        UNION ALL
        SELECT workspace_id, id AS project_id FROM projects
      ) scopes
     ORDER BY workspace_id, project_id NULLS FIRST
  LOOP
    PERFORM flow_ensure_navigator_root(scope_row.workspace_id, scope_row.project_id);
  END LOOP;
END
$$;

-- Every legacy top-level non-navigator object is attached to the root for its existing scope.
-- project_id is deliberately not written: migration must never rewrite governance scope.
UPDATE flow_objects child
   SET parent_id = root.id,
       updated_at = now()
  FROM flow_objects root
 WHERE child.workspace_id = root.workspace_id
   AND child.parent_id IS NULL
   AND child.object_type <> 'navigator'
   AND child.project_id IS NOT DISTINCT FROM root.project_id
   AND flow_is_system_navigator_root(
         root.object_type, root.parent_id, root.governance_metadata
       );

ALTER TABLE flow_objects
  DROP CONSTRAINT IF EXISTS flow_objects_non_navigator_parent_check;

ALTER TABLE flow_objects
  ADD CONSTRAINT flow_objects_non_navigator_parent_check
  CHECK (object_type = 'navigator' OR parent_id IS NOT NULL);

ALTER TABLE flow_objects
  DROP CONSTRAINT IF EXISTS flow_objects_navigator_root_role_check;

ALTER TABLE flow_objects
  ADD CONSTRAINT flow_objects_navigator_root_role_check
  CHECK (
    (parent_id IS NULL) = flow_is_system_navigator_root(
      object_type, parent_id, governance_metadata
    )
  );

-- A workspace inserted after this migration immediately gets its NULL-scope root. Project roots
-- are deliberately lazy for newly-created projects and are materialized by the same function on
-- the first project-scoped Flow operation.
CREATE OR REPLACE FUNCTION flow_create_workspace_navigator_root()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  PERFORM flow_ensure_navigator_root(NEW.id, NULL);
  RETURN NEW;
END
$$;

DROP TRIGGER IF EXISTS workspaces_create_flow_navigator_root ON workspaces;
CREATE TRIGGER workspaces_create_flow_navigator_root
AFTER INSERT ON workspaces
FOR EACH ROW EXECUTE FUNCTION flow_create_workspace_navigator_root();

CREATE OR REPLACE VIEW flow_object_navigator_root_violations AS
  WITH expected_scopes AS (
    SELECT id AS workspace_id, NULL::uuid AS project_id FROM workspaces
    UNION ALL
    SELECT workspace_id, id AS project_id FROM projects
  )
  SELECT NULL::uuid AS object_id,
         scope.workspace_id,
         'navigator'::text AS object_type,
         scope.project_id,
         NULL::uuid AS parent_id,
         'missing_scope_navigator_root'::text AS violation
    FROM expected_scopes scope
   WHERE NOT EXISTS (
     SELECT 1
       FROM flow_objects root
      WHERE root.workspace_id = scope.workspace_id
        AND root.project_id IS NOT DISTINCT FROM scope.project_id
        AND flow_is_system_navigator_root(
              root.object_type, root.parent_id, root.governance_metadata
            )
   )
  UNION ALL
  SELECT object_row.id,
         object_row.workspace_id,
         object_row.object_type,
         object_row.project_id,
         object_row.parent_id,
         'non_root_without_parent'::text AS violation
    FROM flow_objects object_row
   WHERE object_row.parent_id IS NULL
     AND NOT flow_is_system_navigator_root(
           object_row.object_type, object_row.parent_id, object_row.governance_metadata
         )
  UNION ALL
  SELECT (array_agg(root.id ORDER BY root.id))[1] AS object_id,
         root.workspace_id,
         'navigator'::text AS object_type,
         root.project_id,
         NULL::uuid AS parent_id,
         'multiple_scope_navigator_roots'::text AS violation
    FROM flow_objects root
   WHERE root.object_type = 'navigator' AND root.parent_id IS NULL
   GROUP BY root.workspace_id, root.project_id
  HAVING count(*) > 1;

COMMENT ON VIEW flow_object_navigator_root_violations IS
  'ADR-0018 NR-1..NR-4 replayable monitor. Empty means each workspace/project scope has exactly one marked navigator root and every other object has a parent.';
