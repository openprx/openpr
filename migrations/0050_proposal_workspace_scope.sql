-- Give governance proposals a tenant.
--
-- `proposals` was created without a workspace or project column, so every proposal lived in one
-- flat instance wide namespace and `GET /api/v1/proposals` returned all of them to anyone who got
-- past the door. The column is added here and the API filters every proposal read by it.
--
-- The column stays nullable on purpose. A proposal that cannot be attributed from the data
-- already in the database is left NULL rather than being guessed into somebody's workspace; the
-- API treats NULL as "unattributed" and shows those rows to instance administrators only, so
-- nothing is deleted, nothing is hidden from an operator, and no tenant sees another tenant's
-- proposal. New proposals always get a workspace.

ALTER TABLE proposals
  ADD COLUMN IF NOT EXISTS workspace_id uuid;

DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM pg_constraint WHERE conname = 'fk_proposals_workspace_id'
  ) THEN
    ALTER TABLE proposals
      ADD CONSTRAINT fk_proposals_workspace_id
      FOREIGN KEY (workspace_id)
      REFERENCES workspaces(id)
      ON DELETE SET NULL;
  END IF;
END $$;

-- First source of truth: the issues a proposal is linked to. Only unambiguous cases are used, so
-- a proposal that spans two workspaces is left for an operator instead of being cut in half.
WITH linked_scope AS (
  SELECT
    pil.proposal_id AS proposal_id,
    COUNT(DISTINCT pr.workspace_id) AS workspace_count,
    (array_agg(DISTINCT pr.workspace_id))[1] AS workspace_id
  FROM proposal_issue_links pil
  JOIN work_items wi ON wi.id = pil.issue_id
  JOIN projects pr ON pr.id = wi.project_id
  GROUP BY pil.proposal_id
)
UPDATE proposals p
SET workspace_id = linked_scope.workspace_id
FROM linked_scope
WHERE p.id = linked_scope.proposal_id
  AND linked_scope.workspace_count = 1
  AND p.workspace_id IS NULL;

-- Second source: the author, when they belong to exactly one workspace. A draft that was never
-- linked to an issue has no other evidence of where it belongs.
WITH author_candidates AS (
  SELECT
    p.id AS proposal_id,
    CASE
      WHEN p.author_id ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
        THEN p.author_id::uuid
    END AS author_uuid
  FROM proposals p
  WHERE p.workspace_id IS NULL
),
author_scope AS (
  SELECT
    ac.proposal_id AS proposal_id,
    COUNT(DISTINCT wm.workspace_id) AS workspace_count,
    (array_agg(DISTINCT wm.workspace_id))[1] AS workspace_id
  FROM author_candidates ac
  JOIN workspace_members wm ON wm.user_id = ac.author_uuid
  WHERE ac.author_uuid IS NOT NULL
  GROUP BY ac.proposal_id
)
UPDATE proposals p
SET workspace_id = author_scope.workspace_id
FROM author_scope
WHERE p.id = author_scope.proposal_id
  AND author_scope.workspace_count = 1
  AND p.workspace_id IS NULL;

CREATE INDEX IF NOT EXISTS idx_proposals_workspace_id
  ON proposals(workspace_id, created_at DESC);
