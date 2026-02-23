ALTER TABLE users
  ADD COLUMN IF NOT EXISTS password_hash text;

ALTER TABLE users
  ADD COLUMN IF NOT EXISTS name text;

DO $$
BEGIN
  IF EXISTS (
    SELECT 1
    FROM information_schema.columns
    WHERE table_name = 'users' AND column_name = 'display_name'
  ) THEN
    UPDATE users
    SET name = COALESCE(name, display_name)
    WHERE name IS NULL;
  END IF;
END $$;

UPDATE users
SET name = COALESCE(name, email)
WHERE name IS NULL;

ALTER TABLE users
  ALTER COLUMN name SET NOT NULL;

UPDATE users
SET password_hash = ''
WHERE password_hash IS NULL;

ALTER TABLE users
  ALTER COLUMN password_hash SET NOT NULL;

ALTER TABLE users
  DROP COLUMN IF EXISTS display_name;
