-- Labels table
CREATE TABLE IF NOT EXISTS labels (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  name text NOT NULL,
  color text NOT NULL DEFAULT '#gray',
  description text NOT NULL DEFAULT '',
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE(workspace_id, name)
);

CREATE INDEX IF NOT EXISTS idx_labels_workspace ON labels(workspace_id);

-- Work item labels (many-to-many)
CREATE TABLE IF NOT EXISTS work_item_labels (
  work_item_id uuid NOT NULL REFERENCES work_items(id) ON DELETE CASCADE,
  label_id uuid NOT NULL REFERENCES labels(id) ON DELETE CASCADE,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (work_item_id, label_id)
);

CREATE INDEX IF NOT EXISTS idx_work_item_labels_label ON work_item_labels(label_id);
