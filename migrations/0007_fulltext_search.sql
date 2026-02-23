-- Full-text search setup (simple config for mixed-language search, incl. CJK fallback).

-- work_items
ALTER TABLE work_items ADD COLUMN IF NOT EXISTS search_vector tsvector;
UPDATE work_items
SET search_vector = to_tsvector('simple', COALESCE(title, '') || ' ' || COALESCE(description, ''));
CREATE INDEX IF NOT EXISTS idx_work_items_search ON work_items USING GIN(search_vector);
DROP TRIGGER IF EXISTS work_items_search_vector_trigger ON work_items;
CREATE TRIGGER work_items_search_vector_trigger
BEFORE INSERT OR UPDATE ON work_items
FOR EACH ROW EXECUTE FUNCTION tsvector_update_trigger(search_vector, 'pg_catalog.simple', title, description);

-- projects
ALTER TABLE projects ADD COLUMN IF NOT EXISTS search_vector tsvector;
UPDATE projects
SET search_vector = to_tsvector('simple', COALESCE(key, '') || ' ' || COALESCE(name, '') || ' ' || COALESCE(description, ''));
CREATE INDEX IF NOT EXISTS idx_projects_search ON projects USING GIN(search_vector);
DROP TRIGGER IF EXISTS projects_search_vector_trigger ON projects;
CREATE TRIGGER projects_search_vector_trigger
BEFORE INSERT OR UPDATE ON projects
FOR EACH ROW EXECUTE FUNCTION tsvector_update_trigger(search_vector, 'pg_catalog.simple', key, name, description);

-- comments
ALTER TABLE comments ADD COLUMN IF NOT EXISTS search_vector tsvector;
UPDATE comments
SET search_vector = to_tsvector('simple', COALESCE(body, ''));
CREATE INDEX IF NOT EXISTS idx_comments_search ON comments USING GIN(search_vector);
DROP TRIGGER IF EXISTS comments_search_vector_trigger ON comments;
CREATE TRIGGER comments_search_vector_trigger
BEFORE INSERT OR UPDATE ON comments
FOR EACH ROW EXECUTE FUNCTION tsvector_update_trigger(search_vector, 'pg_catalog.simple', body);
