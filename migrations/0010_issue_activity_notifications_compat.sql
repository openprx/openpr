-- Compatibility migration for issue activity tracking and notification workspace scope.

-- Activities: add new fields expected by issue update audit trail.
ALTER TABLE activities ADD COLUMN IF NOT EXISTS workspace_id UUID REFERENCES workspaces(id);
ALTER TABLE activities ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES projects(id);
ALTER TABLE activities ADD COLUMN IF NOT EXISTS issue_id UUID REFERENCES work_items(id);
ALTER TABLE activities ADD COLUMN IF NOT EXISTS user_id UUID REFERENCES users(id);
ALTER TABLE activities ADD COLUMN IF NOT EXISTS action VARCHAR(50);
ALTER TABLE activities ADD COLUMN IF NOT EXISTS detail JSONB;

-- Backfill new columns from legacy shape where possible.
UPDATE activities
SET issue_id = resource_id
WHERE issue_id IS NULL AND resource_type = 'issue';

UPDATE activities
SET project_id = resource_id
WHERE project_id IS NULL AND resource_type = 'project';

UPDATE activities
SET workspace_id = resource_id
WHERE workspace_id IS NULL AND resource_type = 'workspace';

UPDATE activities
SET user_id = actor_id
WHERE user_id IS NULL AND actor_id IS NOT NULL;

UPDATE activities
SET action = event_type
WHERE (action IS NULL OR TRIM(action) = '') AND event_type IS NOT NULL;

UPDATE activities
SET detail = payload
WHERE detail IS NULL AND payload IS NOT NULL;

UPDATE activities
SET detail = '{}'::jsonb
WHERE detail IS NULL;

ALTER TABLE activities ALTER COLUMN detail SET DEFAULT '{}'::jsonb;

CREATE INDEX IF NOT EXISTS idx_activities_issue_id ON activities(issue_id);
CREATE INDEX IF NOT EXISTS idx_activities_user_id ON activities(user_id);
CREATE INDEX IF NOT EXISTS idx_activities_workspace_id ON activities(workspace_id);
CREATE INDEX IF NOT EXISTS idx_activities_project_id ON activities(project_id);

-- Notifications: add workspace scope for assignment notifications.
ALTER TABLE notifications ADD COLUMN IF NOT EXISTS workspace_id UUID REFERENCES workspaces(id);

CREATE INDEX IF NOT EXISTS idx_notifications_user_id ON notifications(user_id);
CREATE INDEX IF NOT EXISTS idx_notifications_unread
    ON notifications(user_id, is_read)
    WHERE is_read = false;
