DO $$
BEGIN
  IF to_regclass('public.proposal_comments') IS NULL THEN
    RETURN;
  END IF;

  IF NOT EXISTS (
    SELECT 1
    FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'proposal_comments' AND column_name = 'comment_type'
  ) THEN
    ALTER TABLE proposal_comments
      ADD COLUMN comment_type varchar(20) NOT NULL DEFAULT 'general';
  END IF;

  IF NOT EXISTS (
    SELECT 1
    FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'proposal_comments' AND column_name = 'content'
  ) THEN
    ALTER TABLE proposal_comments
      ADD COLUMN content text;
  END IF;

  IF EXISTS (
    SELECT 1
    FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'proposal_comments' AND column_name = 'body'
  ) THEN
    UPDATE proposal_comments
    SET content = COALESCE(content, body)
    WHERE content IS NULL;
  END IF;

  UPDATE proposal_comments
  SET content = ''
  WHERE content IS NULL;

  ALTER TABLE proposal_comments
    ALTER COLUMN content SET NOT NULL;

  IF NOT EXISTS (
    SELECT 1
    FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'proposal_comments' AND column_name = 'author_type'
  ) THEN
    ALTER TABLE proposal_comments
      ADD COLUMN author_type author_type;
  END IF;

  IF EXISTS (
    SELECT 1
    FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'proposal_comments' AND column_name = 'author_type'
      AND udt_name <> 'author_type'
  ) THEN
    ALTER TABLE proposal_comments
      ALTER COLUMN author_type TYPE author_type
      USING (
        CASE lower(author_type::text)
          WHEN 'ai' THEN 'ai'::author_type
          WHEN 'bot' THEN 'ai'::author_type
          ELSE 'human'::author_type
        END
      );
  END IF;

  UPDATE proposal_comments
  SET author_type = 'human'::author_type
  WHERE author_type IS NULL;

  ALTER TABLE proposal_comments
    ALTER COLUMN author_type SET NOT NULL;

  IF EXISTS (
    SELECT 1
    FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'proposal_comments' AND column_name = 'author_id'
      AND udt_name <> 'text'
  ) THEN
    ALTER TABLE proposal_comments
      ALTER COLUMN author_id TYPE text USING author_id::text;
  END IF;
END $$;
