-- Compatibility migration for legacy notifications schema:
-- Legacy columns: kind, payload, read_at, created_at
-- Current code expects: type, title, content, link, related_issue_id,
-- related_comment_id, related_project_id, is_read, metadata, created_at, read_at

ALTER TABLE notifications ADD COLUMN IF NOT EXISTS type TEXT;
ALTER TABLE notifications ADD COLUMN IF NOT EXISTS title TEXT;
ALTER TABLE notifications ADD COLUMN IF NOT EXISTS content TEXT;
ALTER TABLE notifications ADD COLUMN IF NOT EXISTS link TEXT;
ALTER TABLE notifications ADD COLUMN IF NOT EXISTS related_issue_id UUID;
ALTER TABLE notifications ADD COLUMN IF NOT EXISTS related_comment_id UUID;
ALTER TABLE notifications ADD COLUMN IF NOT EXISTS related_project_id UUID;
ALTER TABLE notifications ADD COLUMN IF NOT EXISTS is_read BOOLEAN;
ALTER TABLE notifications ADD COLUMN IF NOT EXISTS metadata JSONB;

DO $$
BEGIN
    -- Backfill type from legacy kind when available.
    IF EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'notifications'
          AND column_name = 'kind'
    ) THEN
        EXECUTE $sql$
            UPDATE notifications
            SET type = COALESCE(NULLIF(TRIM(kind), ''), 'info')
            WHERE type IS NULL OR TRIM(type) = ''
        $sql$;
    ELSE
        EXECUTE $sql$
            UPDATE notifications
            SET type = 'info'
            WHERE type IS NULL OR TRIM(type) = ''
        $sql$;
    END IF;
END $$;

DO $$
BEGIN
    -- Backfill text/link/related_issue_id from legacy payload when available.
    IF EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'notifications'
          AND column_name = 'payload'
    ) THEN
        EXECUTE $sql$
            UPDATE notifications
            SET
                title = COALESCE(NULLIF(title, ''), COALESCE(payload->>'title', '')),
                content = COALESCE(NULLIF(content, ''), COALESCE(payload->>'content', payload::text, '')),
                link = COALESCE(link, payload->>'link'),
                metadata = COALESCE(metadata, payload),
                related_issue_id = COALESCE(
                    related_issue_id,
                    CASE
                        WHEN payload ? 'related_issue_id'
                            AND (payload->>'related_issue_id') ~* '^[0-9a-f-]{36}$'
                            THEN (payload->>'related_issue_id')::uuid
                        ELSE NULL
                    END
                )
            WHERE
                payload IS NOT NULL
                AND (
                    title IS NULL OR title = ''
                    OR content IS NULL OR content = ''
                    OR link IS NULL
                    OR related_issue_id IS NULL
                )
        $sql$;
    END IF;
END $$;

-- Keep read flags consistent for legacy rows.
UPDATE notifications
SET is_read = (read_at IS NOT NULL)
WHERE is_read IS NULL;

-- Final null-safety backfill before NOT NULL constraints.
UPDATE notifications
SET
    type = COALESCE(NULLIF(TRIM(type), ''), 'info'),
    title = COALESCE(title, ''),
    content = COALESCE(content, ''),
    is_read = COALESCE(is_read, false);

-- Ensure non-null defaults required by current code path.
ALTER TABLE notifications ALTER COLUMN type SET DEFAULT 'info';
ALTER TABLE notifications ALTER COLUMN type SET NOT NULL;
ALTER TABLE notifications ALTER COLUMN title SET DEFAULT '';
ALTER TABLE notifications ALTER COLUMN title SET NOT NULL;
ALTER TABLE notifications ALTER COLUMN content SET DEFAULT '';
ALTER TABLE notifications ALTER COLUMN content SET NOT NULL;
ALTER TABLE notifications ALTER COLUMN is_read SET DEFAULT false;
ALTER TABLE notifications ALTER COLUMN is_read SET NOT NULL;

-- Helpful indexes for API list/read flows.
CREATE INDEX IF NOT EXISTS idx_notifications_user_created
    ON notifications(user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_notifications_user_unread
    ON notifications(user_id, is_read)
    WHERE is_read = false;
