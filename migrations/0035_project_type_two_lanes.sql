-- Collapse industry-specific project types into two product lanes.
-- Industry scenarios belong to universal form templates, not project type.

WITH default_workflow AS (
  SELECT id
  FROM workflows
  WHERE is_system_default = TRUE
  LIMIT 1
)
INSERT INTO project_types (
  key,
  name,
  description,
  domain,
  default_workflow_id,
  enabled_capabilities,
  field_schema,
  artifact_schema,
  default_connectors
)
SELECT
  seed.key,
  seed.name,
  seed.description,
  seed.domain,
  default_workflow.id,
  seed.enabled_capabilities::jsonb,
  seed.field_schema::jsonb,
  seed.artifact_schema::jsonb,
  seed.default_connectors::jsonb
FROM (
  VALUES
    (
      'code_project',
      'Code Project',
      'Software delivery project with repositories, directories, CI, code review, and AI coding agents.',
      'software',
      '["issues","board","sprints","mcp","webhook","code_context","governance","forms","plugins"]',
      '{"resources":[{"kind":"repo","required":false},{"kind":"directory","required":false}]}',
      '{"primary":["comment","check_result","change_proposal","pull_request"]}',
      '["mcp","webhook","cli"]'
    ),
    (
      'custom_form',
      'Custom Form',
      'Universal form project for custom business data, records, links, automation, MCP, webhooks, and plugins.',
      'custom',
      '["forms","mcp","webhook","plugins","reporting"]',
      '{"resources":[],"fields":[]}',
      '{"primary":["form","form_record","business_event"]}',
      '["mcp","webhook","rest"]'
    )
) AS seed(key, name, description, domain, enabled_capabilities, field_schema, artifact_schema, default_connectors)
CROSS JOIN default_workflow
ON CONFLICT (key) DO UPDATE
SET
  name = EXCLUDED.name,
  description = EXCLUDED.description,
  domain = EXCLUDED.domain,
  default_workflow_id = COALESCE(project_types.default_workflow_id, EXCLUDED.default_workflow_id),
  enabled_capabilities = EXCLUDED.enabled_capabilities,
  field_schema = EXCLUDED.field_schema,
  artifact_schema = EXCLUDED.artifact_schema,
  default_connectors = EXCLUDED.default_connectors,
  updated_at = NOW();

UPDATE projects
SET type_key = 'custom_form'
WHERE type_key IN (
  'contract_review',
  'customer_delivery',
  'equipment_maintenance',
  'quality_corrective_action',
  'restaurant_ordering'
);

UPDATE scenario_templates
SET project_type_key = 'custom_form',
    updated_at = NOW()
WHERE project_type_key IN (
  'contract_review',
  'customer_delivery',
  'equipment_maintenance',
  'quality_corrective_action',
  'restaurant_ordering'
);

DELETE FROM project_types
WHERE key IN (
  'contract_review',
  'customer_delivery',
  'equipment_maintenance',
  'quality_corrective_action',
  'restaurant_ordering'
);
