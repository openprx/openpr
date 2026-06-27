ALTER TABLE users
  ADD COLUMN IF NOT EXISTS avatar_url text;

ALTER TABLE users
  ADD COLUMN IF NOT EXISTS notification_prefs jsonb NOT NULL DEFAULT '{
    "email_notification": true,
    "mention_only": false,
    "daily_digest": true
  }'::jsonb;

UPDATE users
SET notification_prefs = '{
  "email_notification": true,
  "mention_only": false,
  "daily_digest": true
}'::jsonb
WHERE notification_prefs IS NULL;
