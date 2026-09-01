-- Sylvode Flow v0.5 — "a non-root object sits in its parent's project scope", as a database
-- constraint.
--
-- `ADR-0013` §2.2 (R17, 2026-08-31) makes this invariant the **precondition** for the
-- cross-project `move_object` cascade. Without it `P1 root -> P2 child -> P3 grandchild` is a
-- fully legal shape: `apps/api/src/flow/command.rs`'s `create_object` validates `project_id` and
-- `parent_object_id` independently (each only against the workspace), and `0054`'s schema carries
-- only `flow_objects_parent_workspace_fk` and `flow_objects_parent_not_self_check`. Cascading such
-- a subtree to `P4` would touch four navigator documents, so `move_object`'s declared
-- `BoundedMany(2)` contended-document ceiling would be false. With this constraint a subtree
-- belongs to exactly one project scope, the cascade touches exactly the source and target
-- navigators, and the declaration holds.
--
-- Why a composite foreign key and not a trigger: `0054` already proved the pattern on this very
-- table (`flow_objects_parent_workspace_fk` forces "child and parent share a workspace" the same
-- way). A trigger would be a second, weaker enforcement mechanism -- not enforced during restore,
-- bypassable with `session_replication_role`, and invisible to `information_schema`. This key
-- stays orthogonal to that one: it carries the scope column and *not* `workspace_id`, so the two
-- constraints enforce one invariant each rather than both half-enforcing the other's.
--
-- ============================================================================================
-- NULL semantics, stated exactly (this is the part a composite FK cannot express on its own)
-- ============================================================================================
-- `project_id` is nullable (`domain-model-v1.md`: "可选 `project_id`"), and NULL is a **real
-- scope**, not "unknown": `repository::fetch_navigator_document(workspace_id, None)` resolves the
-- unprojected navigator exactly the way `Some(project)` resolves a project's navigator. Ordering
-- entries for unprojected objects live in that document. So the invariant treats NULL as one more
-- scope value and demands strict equality:
--
--     parent scope  | child scope | verdict
--     --------------+-------------+---------------------------------------------------------
--     (no parent)   | anything    | allowed  -- a root/navigator defines its own scope
--     NULL          | NULL        | allowed
--     NULL          | P           | rejected -- child's entry would sit in P's navigator while
--                                 --            its parent's sits in the unprojected one
--     P             | NULL        | rejected -- the mirror image, and the one a plain MATCH
--                                 --            SIMPLE composite FK would silently let through
--     P             | P           | allowed
--     P             | Q (Q<>P)    | rejected
--
-- The looser reading ("NULL child means 'inherit', allow it under any parent") was rejected: it
-- reintroduces exactly the defect this migration exists to remove -- a subtree spanning the
-- unprojected navigator and a project's navigator is still a subtree spanning two navigators, and
-- `BoundedMany(2)` still fails on the cascade.
--
-- A foreign key on `(parent_id, project_id)` cannot express that. Its default MATCH SIMPLE
-- short-circuits whenever *any* referencing column is NULL, so `P -> NULL` would pass. MATCH FULL
-- cannot rescue it either: it would then reject every root (`parent_id IS NULL` with a project)
-- and every unprojected non-root (`project_id IS NULL` with a parent), because MATCH FULL demands
-- the referencing columns be all-NULL or none-NULL and both of those shapes are legal. The fix is
-- to remove NULL from the key instead of from the semantics: a stored generated column normalises
-- the scope to a non-NULL value, after which MATCH SIMPLE short-circuits on exactly one thing --
-- `parent_id IS NULL`, i.e. a root -- which is precisely the exemption the table above wants.

-- The nil UUID is the sentinel the generated column below maps NULL onto. `project_id` references
-- `projects(id)`, whose ids come from `gen_random_uuid()` and can therefore never be nil, but
-- "can never be" is worth one cheap constraint when the alternative is an unprojected object and
-- a nil-id project silently sharing a scope key.
ALTER TABLE flow_objects
  DROP CONSTRAINT IF EXISTS flow_objects_project_id_not_nil_check;

ALTER TABLE flow_objects
  ADD CONSTRAINT flow_objects_project_id_not_nil_check
  CHECK (project_id IS NULL OR project_id <> '00000000-0000-0000-0000-000000000000'::uuid);

-- The NULL-free restatement of `project_id`. `STORED` (not `VIRTUAL`, which PostgreSQL 16 does
-- not have) so it can carry a unique index and be a foreign key's referenced column.
ALTER TABLE flow_objects
  ADD COLUMN IF NOT EXISTS project_scope_id UUID NOT NULL
  GENERATED ALWAYS AS (COALESCE(project_id, '00000000-0000-0000-0000-000000000000'::uuid)) STORED;

COMMENT ON COLUMN flow_objects.project_scope_id IS
  'ADR-0013 s2.2 R17: project_id with NULL normalised to the nil UUID, so the parent/child same-scope invariant can be a composite FK instead of a trigger. Generated; never written directly.';

-- ============================================================================================
-- Existing-data proof, executed on every deployment rather than asserted in a report.
--
-- Adding the FK below would fail on violating rows with PostgreSQL's generic "referenced key not
-- present" message, naming one row and no reason. This block fails first, names the count, and
-- says what the rows are -- and on a clean database it is a single index-free sequential check of
-- a table that has never had a production deployment.
-- ============================================================================================
DO $$
DECLARE
  violations BIGINT;
BEGIN
  SELECT count(*) INTO violations
    FROM flow_objects child
    JOIN flow_objects parent
      ON parent.id = child.parent_id
     AND parent.workspace_id = child.workspace_id
   WHERE child.parent_id IS NOT NULL
     AND child.project_id IS DISTINCT FROM parent.project_id;

  IF violations > 0 THEN
    RAISE EXCEPTION
      'flow_objects: % row(s) sit in a different project scope than their parent; '
      'ADR-0013 s2.2 R17 requires them repaired before this invariant can be enforced. '
      'List them with: SELECT * FROM flow_object_project_scope_violations;',
      violations;
  END IF;
END
$$;

-- The same query as a permanent, repeatable check: the `DO` block above only runs at migration
-- time, and `ADR-0013` §4's integrity story needs something an operator (or a test) can re-run
-- afterwards to show the invariant still holds. Empty means healthy.
CREATE OR REPLACE VIEW flow_object_project_scope_violations AS
  SELECT child.id AS object_id,
         child.workspace_id,
         child.parent_id,
         child.project_id AS child_project_id,
         parent.project_id AS parent_project_id
    FROM flow_objects child
    JOIN flow_objects parent
      ON parent.id = child.parent_id
     AND parent.workspace_id = child.workspace_id
   WHERE child.parent_id IS NOT NULL
     AND child.project_id IS DISTINCT FROM parent.project_id;

COMMENT ON VIEW flow_object_project_scope_violations IS
  'ADR-0013 s2.2 R17 invariant monitor: rows whose project scope differs from their parent''s. Enforced by flow_objects_parent_project_fk, so this view is expected to stay empty; it exists so "it is still empty" is a query rather than a claim.';

-- The referenced side. `id` is already the primary key, so `(id, project_scope_id)` is unique for
-- free and this index only exists because a foreign key must point at a named unique constraint.
--
-- `workspace_id` is deliberately **not** in this key even though `flow_objects_parent_workspace_fk`
-- has it. `parent_id` references the primary key, so the parent row is already identified without
-- it, and repeating the column would make this constraint quietly re-enforce "same workspace" as
-- well. That is not this constraint's job -- it is the other one's -- and the duplication has a
-- concrete cost: `flow::collab::authz`'s cross-workspace-parent fixture drops
-- `flow_objects_parent_workspace_fk` precisely to build an illegal cross-workspace shape, and a
-- second constraint silently enforcing the same rule turns that fixture's setup into a foreign-key
-- error. One constraint, one invariant.
--
-- The foreign key is dropped *first* on a replay: it is implemented on top of the unique index
-- below, so dropping that index while the key still depends on it fails outright ("cannot drop
-- constraint ... because other objects depend on it"). This file re-runs on every database whose
-- ledger has no row for it, so the drop order is part of its correctness, not tidiness.
ALTER TABLE flow_objects
  DROP CONSTRAINT IF EXISTS flow_objects_parent_project_fk;

ALTER TABLE flow_objects
  DROP CONSTRAINT IF EXISTS flow_objects_project_scope_key;

ALTER TABLE flow_objects
  ADD CONSTRAINT flow_objects_project_scope_key
  UNIQUE (id, project_scope_id);

-- The invariant itself. `ON DELETE CASCADE` mirrors `flow_objects_parent_workspace_fk`: the two
-- constraints describe the same parent/child edge, so a parent deletion must mean the same thing
-- to both of them.
--
-- MATCH SIMPLE (the default) is what makes the root exemption work: `project_scope_id` is
-- `NOT NULL`, so the only referencing column that can be NULL is `parent_id`, and a NULL there --
-- a root -- short-circuits the check. Every non-root row is compared on the full key.
ALTER TABLE flow_objects
  ADD CONSTRAINT flow_objects_parent_project_fk
  FOREIGN KEY (parent_id, project_scope_id)
  REFERENCES flow_objects (id, project_scope_id) ON DELETE CASCADE;
