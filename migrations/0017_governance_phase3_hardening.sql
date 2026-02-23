DO $$
BEGIN
  IF EXISTS (
    SELECT 1
    FROM information_schema.columns
    WHERE table_schema = 'public'
      AND table_name = 'review_participants'
      AND column_name = 'user_id'
      AND data_type <> 'character varying'
  ) THEN
    ALTER TABLE review_participants
      ALTER COLUMN user_id TYPE varchar(100) USING user_id::text;
  END IF;
END $$;

DO $$
DECLARE
  fk_name text;
BEGIN
  SELECT tc.constraint_name
  INTO fk_name
  FROM information_schema.table_constraints tc
  JOIN information_schema.constraint_column_usage ccu
    ON tc.constraint_name = ccu.constraint_name
   AND tc.table_schema = ccu.table_schema
  JOIN information_schema.key_column_usage kcu
    ON tc.constraint_name = kcu.constraint_name
   AND tc.table_schema = kcu.table_schema
  WHERE tc.constraint_type = 'FOREIGN KEY'
    AND tc.table_schema = 'public'
    AND tc.table_name = 'review_participants'
    AND kcu.column_name = 'user_id'
    AND ccu.table_name = 'users'
  LIMIT 1;

  IF fk_name IS NOT NULL THEN
    EXECUTE format('ALTER TABLE review_participants DROP CONSTRAINT %I', fk_name);
  END IF;
END $$;

DO $$
BEGIN
  DELETE FROM trust_score_logs t
  USING trust_score_logs dup
  WHERE t.id > dup.id
    AND t.user_id = dup.user_id
    AND t.project_id = dup.project_id
    AND t.domain = dup.domain
    AND t.event_type = dup.event_type
    AND t.event_id = dup.event_id;
END $$;

CREATE UNIQUE INDEX IF NOT EXISTS uq_trust_score_logs_event_scope
  ON trust_score_logs (user_id, project_id, domain, event_type, event_id);
