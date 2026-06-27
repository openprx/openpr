ALTER TABLE form_views
  DROP CONSTRAINT IF EXISTS form_views_type_check;

ALTER TABLE form_views
  ADD CONSTRAINT form_views_type_check
  CHECK (view_type IN ('grid', 'detail', 'kanban', 'calendar', 'card', 'timeline', 'pivot', 'gantt'));
