ALTER TABLE work_items
  ADD COLUMN IF NOT EXISTS proposal_id text REFERENCES proposals(id) ON DELETE SET NULL,
  ADD COLUMN IF NOT EXISTS governance_exempt boolean NOT NULL DEFAULT false,
  ADD COLUMN IF NOT EXISTS governance_exempt_reason text;

CREATE INDEX IF NOT EXISTS idx_work_items_proposal_id ON work_items(proposal_id);
