-- Server-side thumbnail metadata for universal form attachment/image fields.

ALTER TABLE form_attachments
  ADD COLUMN IF NOT EXISTS thumbnail_url TEXT;
