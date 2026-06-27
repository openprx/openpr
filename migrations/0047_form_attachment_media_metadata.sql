-- Structured media metadata for attachment/image preview policies.

ALTER TABLE form_attachments
  ADD COLUMN IF NOT EXISTS media_metadata JSONB NOT NULL DEFAULT '{}'::jsonb;
