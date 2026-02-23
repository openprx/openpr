ALTER TABLE users
  ADD COLUMN IF NOT EXISTS role varchar(32) NOT NULL DEFAULT 'user';

ALTER TABLE users
  ADD COLUMN IF NOT EXISTS is_active boolean NOT NULL DEFAULT true;

UPDATE users
SET role = 'admin'
WHERE lower(email) = 'admin@openpr.local';
