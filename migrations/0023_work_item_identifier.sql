-- Add sequence_number to work_items for human-readable identifiers (e.g., PRX-42)
ALTER TABLE work_items ADD COLUMN IF NOT EXISTS sequence_number integer;

-- Backfill existing rows with per-project sequential numbers
WITH ranked AS (
  SELECT id,
         ROW_NUMBER() OVER (PARTITION BY project_id ORDER BY created_at ASC, id ASC) AS rn
  FROM work_items
)
UPDATE work_items wi
SET sequence_number = r.rn
FROM ranked r
WHERE wi.id = r.id
  AND wi.sequence_number IS NULL;

-- Make it required and unique within a project
ALTER TABLE work_items ALTER COLUMN sequence_number SET NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_work_items_project_sequence
  ON work_items(project_id, sequence_number);

-- Auto-assign sequence_number on INSERT
CREATE OR REPLACE FUNCTION assign_work_item_sequence()
RETURNS TRIGGER AS $$
BEGIN
  IF NEW.sequence_number IS NULL THEN
    SELECT COALESCE(MAX(sequence_number), 0) + 1
    INTO NEW.sequence_number
    FROM work_items
    WHERE project_id = NEW.project_id;
  END IF;
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_work_item_sequence ON work_items;
CREATE TRIGGER trg_work_item_sequence
  BEFORE INSERT ON work_items
  FOR EACH ROW EXECUTE FUNCTION assign_work_item_sequence();
