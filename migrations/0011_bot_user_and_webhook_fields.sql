ALTER TABLE users
  ADD COLUMN IF NOT EXISTS entity_type VARCHAR(10) DEFAULT 'human' NOT NULL;

ALTER TABLE users
  ADD COLUMN IF NOT EXISTS agent_type VARCHAR(20);

ALTER TABLE users
  ADD COLUMN IF NOT EXISTS agent_config JSONB;

ALTER TABLE webhooks
  ADD COLUMN IF NOT EXISTS bot_user_id UUID REFERENCES users(id);

DO $$
BEGIN
  IF EXISTS (
    SELECT 1
    FROM information_schema.columns
    WHERE table_schema = 'public'
      AND table_name = 'webhooks'
      AND column_name = 'events'
      AND data_type = 'ARRAY'
  ) THEN
    ALTER TABLE webhooks
      ALTER COLUMN events TYPE JSONB USING to_jsonb(events);
  END IF;
END $$;
