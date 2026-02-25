-- Ensure all foreign keys referencing work_items that should cascade on issue deletion
-- are configured with ON DELETE CASCADE.

DO $$
DECLARE
    rec RECORD;
BEGIN
    -- activities.issue_id -> work_items(id)
    IF to_regclass('public.activities') IS NOT NULL
       AND EXISTS (
           SELECT 1
           FROM information_schema.columns
           WHERE table_schema = 'public'
             AND table_name = 'activities'
             AND column_name = 'issue_id'
       ) THEN
        FOR rec IN
            SELECT c.conname
            FROM pg_constraint c
            JOIN pg_class t ON t.oid = c.conrelid
            JOIN pg_namespace n ON n.oid = t.relnamespace
            WHERE c.contype = 'f'
              AND n.nspname = 'public'
              AND t.relname = 'activities'
              AND c.confrelid = 'public.work_items'::regclass
              AND array_length(c.conkey, 1) = 1
              AND c.conkey[1] = (
                  SELECT a.attnum
                  FROM pg_attribute a
                  WHERE a.attrelid = t.oid
                    AND a.attname = 'issue_id'
                    AND a.attnum > 0
                    AND NOT a.attisdropped
              )
              AND c.confdeltype <> 'c'
        LOOP
            EXECUTE format('ALTER TABLE public.activities DROP CONSTRAINT %I', rec.conname);
        END LOOP;

        IF NOT EXISTS (
            SELECT 1
            FROM pg_constraint c
            JOIN pg_class t ON t.oid = c.conrelid
            JOIN pg_namespace n ON n.oid = t.relnamespace
            WHERE c.contype = 'f'
              AND n.nspname = 'public'
              AND t.relname = 'activities'
              AND c.confrelid = 'public.work_items'::regclass
              AND c.confdeltype = 'c'
              AND array_length(c.conkey, 1) = 1
              AND c.conkey[1] = (
                  SELECT a.attnum
                  FROM pg_attribute a
                  WHERE a.attrelid = t.oid
                    AND a.attname = 'issue_id'
                    AND a.attnum > 0
                    AND NOT a.attisdropped
              )
        ) THEN
            ALTER TABLE public.activities
                ADD CONSTRAINT fk_activities_issue_id_work_items
                FOREIGN KEY (issue_id) REFERENCES public.work_items(id) ON DELETE CASCADE;
        END IF;
    END IF;

    -- notifications.related_issue_id -> work_items(id)
    IF to_regclass('public.notifications') IS NOT NULL
       AND EXISTS (
           SELECT 1
           FROM information_schema.columns
           WHERE table_schema = 'public'
             AND table_name = 'notifications'
             AND column_name = 'related_issue_id'
       ) THEN
        FOR rec IN
            SELECT c.conname
            FROM pg_constraint c
            JOIN pg_class t ON t.oid = c.conrelid
            JOIN pg_namespace n ON n.oid = t.relnamespace
            WHERE c.contype = 'f'
              AND n.nspname = 'public'
              AND t.relname = 'notifications'
              AND c.confrelid = 'public.work_items'::regclass
              AND array_length(c.conkey, 1) = 1
              AND c.conkey[1] = (
                  SELECT a.attnum
                  FROM pg_attribute a
                  WHERE a.attrelid = t.oid
                    AND a.attname = 'related_issue_id'
                    AND a.attnum > 0
                    AND NOT a.attisdropped
              )
              AND c.confdeltype <> 'c'
        LOOP
            EXECUTE format('ALTER TABLE public.notifications DROP CONSTRAINT %I', rec.conname);
        END LOOP;

        IF NOT EXISTS (
            SELECT 1
            FROM pg_constraint c
            JOIN pg_class t ON t.oid = c.conrelid
            JOIN pg_namespace n ON n.oid = t.relnamespace
            WHERE c.contype = 'f'
              AND n.nspname = 'public'
              AND t.relname = 'notifications'
              AND c.confrelid = 'public.work_items'::regclass
              AND c.confdeltype = 'c'
              AND array_length(c.conkey, 1) = 1
              AND c.conkey[1] = (
                  SELECT a.attnum
                  FROM pg_attribute a
                  WHERE a.attrelid = t.oid
                    AND a.attname = 'related_issue_id'
                    AND a.attnum > 0
                    AND NOT a.attisdropped
              )
        ) THEN
            ALTER TABLE public.notifications
                ADD CONSTRAINT fk_notifications_related_issue_id_work_items
                FOREIGN KEY (related_issue_id) REFERENCES public.work_items(id) ON DELETE CASCADE;
        END IF;
    END IF;
END $$;
