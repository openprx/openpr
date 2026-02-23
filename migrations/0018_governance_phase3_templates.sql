ALTER TABLE proposals
  ADD COLUMN IF NOT EXISTS template_id text;

DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1
    FROM pg_constraint
    WHERE conname = 'fk_proposals_template_id'
  ) THEN
    ALTER TABLE proposals
      ADD CONSTRAINT fk_proposals_template_id
      FOREIGN KEY (template_id)
      REFERENCES proposal_templates(id)
      ON DELETE SET NULL;
  END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_proposals_template_id ON proposals(template_id);
