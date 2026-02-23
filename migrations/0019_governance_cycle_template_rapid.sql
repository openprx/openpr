DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1
    FROM pg_enum e
    JOIN pg_type t ON t.oid = e.enumtypid
    WHERE t.typname = 'cycle_template' AND e.enumlabel = 'rapid'
  ) THEN
    ALTER TYPE cycle_template ADD VALUE 'rapid';
  END IF;
END $$;
