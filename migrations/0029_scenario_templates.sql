-- Scenario templates for AI-native project creation and traditional industry onboarding.

CREATE TABLE IF NOT EXISTS scenario_templates (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  key TEXT NOT NULL,
  workspace_id UUID NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  industry TEXT NOT NULL DEFAULT 'general',
  project_type_key TEXT NOT NULL REFERENCES project_types(key) ON DELETE RESTRICT,
  audience_label TEXT NOT NULL DEFAULT 'AI connection',
  workflow_template JSONB NOT NULL DEFAULT '{}'::jsonb,
  field_schema JSONB NOT NULL DEFAULT '{}'::jsonb,
  resource_schema JSONB NOT NULL DEFAULT '{}'::jsonb,
  ai_roles JSONB NOT NULL DEFAULT '[]'::jsonb,
  governance_policy JSONB NOT NULL DEFAULT '{}'::jsonb,
  connector_suggestions JSONB NOT NULL DEFAULT '[]'::jsonb,
  sample_data JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_scenario_templates_system_key
  ON scenario_templates(key)
  WHERE workspace_id IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_scenario_templates_workspace_key
  ON scenario_templates(workspace_id, key)
  WHERE workspace_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_scenario_templates_project_type
  ON scenario_templates(project_type_key);

CREATE INDEX IF NOT EXISTS idx_scenario_templates_industry
  ON scenario_templates(industry);

INSERT INTO scenario_templates (
  key,
  name,
  description,
  industry,
  project_type_key,
  audience_label,
  workflow_template,
  field_schema,
  resource_schema,
  ai_roles,
  governance_policy,
  connector_suggestions,
  sample_data
)
VALUES
  (
    'code_delivery_default',
    'Code Delivery',
    'AI-assisted software delivery with code context, review, CI status, and release approval.',
    'software',
    'code_project',
    'AI coding assistant',
    '{"states":[{"key":"backlog","name":"Backlog","category":"active","initial":true},{"key":"ready","name":"Ready","category":"active"},{"key":"in_progress","name":"In Progress","category":"active"},{"key":"review","name":"Review","category":"active"},{"key":"release_approval","name":"Release Approval","category":"active"},{"key":"done","name":"Done","category":"done","terminal":true}],"default_labels":["bug","feature","review","release-risk"]}',
    '{"fields":[{"key":"repo","label":"Repository","type":"text","required":false},{"key":"directory","label":"Directory","type":"text","required":false},{"key":"branch","label":"Branch","type":"text","required":false},{"key":"ci_status","label":"CI status","type":"select","options":["unknown","passing","failing"],"required":false}]}',
    '{"resources":[{"kind":"repo","label":"Repository","required":false},{"kind":"directory","label":"Working directory","required":false}]}',
    '[{"key":"coding_assistant","label":"Coding assistant","agent_type":"cli","provider":"codex-cli","model":"codex","capabilities":["context.get_project","code.task_context.get","code.change_proposal.create","comments.create"],"writes_require_approval":true},{"key":"review_assistant","label":"Review assistant","agent_type":"mcp","provider":"openpr-mcp","model":"mcp-agent","capabilities":["code.resources.list","documents.review_risk","check_results.create"],"writes_require_approval":true}]',
    '{"risk_model":"code_change","approval_required_for":["code.change_proposal.create","release_approval"],"human_terms":{"proposal":"change request","invocation":"execution history","mcp":"AI connection"}}',
    '[{"key":"openprx_webhook","label":"Code execution connection","type":"webhook","enabled":false,"route_by":["bot_context.bot_name","project_type","trigger_kind"]},{"key":"mcp","label":"AI connection","type":"mcp","enabled":true}]',
    '{"example_project_key":"CODE","example_issue_title":"Review checkout flow race condition"}'
  ),
  (
    'contract_review_default',
    'Contract Review',
    'Document-centered review for counterparty, amount, legal risk, and approval chain.',
    'legal',
    'custom_form',
    'Document review assistant',
    '{"states":[{"key":"intake","name":"Intake","category":"active","initial":true},{"key":"extract","name":"Extract Terms","category":"active"},{"key":"risk_review","name":"Risk Review","category":"active"},{"key":"approval","name":"Approval","category":"active"},{"key":"signed","name":"Signed","category":"done","terminal":true}],"default_labels":["legal-risk","commercial-risk","approval-needed","signed"]}',
    '{"fields":[{"key":"counterparty","label":"Counterparty","type":"text","required":true},{"key":"amount","label":"Amount","type":"number","required":false},{"key":"risk_level","label":"Risk level","type":"select","options":["low","medium","high"],"required":true},{"key":"review_deadline","label":"Review deadline","type":"date","required":false}]}',
    '{"resources":[{"kind":"document_library","label":"Contract documents","required":true}]}',
    '[{"key":"document_assistant","label":"Document review assistant","agent_type":"mcp","provider":"openpr-mcp","model":"mcp-agent","capabilities":["documents.extract_summary","documents.review_risk","approval.request"],"writes_require_approval":true},{"key":"approval_assistant","label":"Approval assistant","agent_type":"webhook","provider":"openpr-webhook","model":"external-agent","capabilities":["approval.request","comments.create"],"writes_require_approval":true}]',
    '{"risk_model":"financial_legal_compliance","approval_required_for":["documents.review_risk","approval.request"],"human_terms":{"proposal":"approval request","invocation":"execution history","mcp":"AI connection","webhook":"external system connection"}}',
    '[{"key":"document_review_bot","label":"Document review connection","type":"webhook","enabled":false,"route_by":["bot_context.bot_name","bot_agent_type","project_type"]},{"key":"mcp","label":"AI connection","type":"mcp","enabled":true}]',
    '{"example_project_key":"LEGAL","example_issue_title":"Review supplier framework agreement"}'
  ),
  (
    'equipment_maintenance_default',
    'Equipment Maintenance',
    'Operations workflow for fault reports, evidence, repair ownership, and inspection acceptance.',
    'operations',
    'custom_form',
    'Maintenance assistant',
    '{"states":[{"key":"reported","name":"Reported","category":"active","initial":true},{"key":"triage","name":"Triage","category":"active"},{"key":"repairing","name":"Repairing","category":"active"},{"key":"inspection","name":"Inspection","category":"active"},{"key":"closed","name":"Closed","category":"done","terminal":true}],"default_labels":["safety","downtime","parts-needed","inspection-needed"]}',
    '{"fields":[{"key":"equipment_id","label":"Equipment ID","type":"text","required":true},{"key":"fault_type","label":"Fault type","type":"text","required":true},{"key":"downtime_impact","label":"Downtime impact","type":"select","options":["none","minor","major","critical"],"required":false},{"key":"repair_owner","label":"Repair owner","type":"text","required":false}]}',
    '{"resources":[{"kind":"equipment","label":"Equipment record","required":true},{"kind":"site","label":"Site","required":false}]}',
    '[{"key":"maintenance_assistant","label":"Maintenance assistant","agent_type":"webhook","provider":"openpr-webhook","model":"external-agent","capabilities":["inspection.report","comments.create"],"writes_require_approval":false},{"key":"inspection_assistant","label":"Inspection assistant","agent_type":"mcp","provider":"openpr-mcp","model":"mcp-agent","capabilities":["inspection.report","approval.request"],"writes_require_approval":true}]',
    '{"risk_model":"safety_operations","approval_required_for":["inspection.report"],"human_terms":{"proposal":"inspection approval","invocation":"execution history","mcp":"AI connection","webhook":"external system connection"}}',
    '[{"key":"maintenance_notification_bot","label":"Maintenance notification connection","type":"webhook","enabled":false,"route_by":["bot_context.bot_name","bot_agent_type","trigger_kind"]},{"key":"mcp","label":"AI connection","type":"mcp","enabled":true}]',
    '{"example_project_key":"MAINT","example_issue_title":"Pump A17 bearing temperature alarm"}'
  ),
  (
    'quality_corrective_action_default',
    'Quality Corrective Action',
    'Quality workflow for defect classification, root cause, corrective action, and recheck.',
    'quality',
    'custom_form',
    'Quality audit assistant',
    '{"states":[{"key":"detected","name":"Detected","category":"active","initial":true},{"key":"root_cause","name":"Root Cause","category":"active"},{"key":"action_plan","name":"Action Plan","category":"active"},{"key":"recheck","name":"Recheck","category":"active"},{"key":"closed","name":"Closed","category":"done","terminal":true}],"default_labels":["defect","root-cause","corrective-action","recheck"]}',
    '{"fields":[{"key":"defect_type","label":"Defect type","type":"text","required":true},{"key":"batch","label":"Batch or order","type":"text","required":false},{"key":"root_cause","label":"Root cause","type":"textarea","required":false},{"key":"recheck_result","label":"Recheck result","type":"select","options":["pending","passed","failed"],"required":false}]}',
    '{"resources":[{"kind":"erp_order","label":"Order or batch","required":false},{"kind":"document_library","label":"Quality documents","required":false}]}',
    '[{"key":"quality_audit_assistant","label":"Quality audit assistant","agent_type":"mcp","provider":"openpr-mcp","model":"mcp-agent","capabilities":["corrective_action.propose","inspection.report","comments.create"],"writes_require_approval":true},{"key":"recheck_assistant","label":"Recheck assistant","agent_type":"webhook","provider":"openpr-webhook","model":"external-agent","capabilities":["inspection.report"],"writes_require_approval":false}]',
    '{"risk_model":"quality_compliance","approval_required_for":["corrective_action.propose"],"human_terms":{"proposal":"corrective action request","invocation":"execution history","mcp":"AI connection","webhook":"external system connection"}}',
    '[{"key":"quality_audit_bot","label":"Quality audit connection","type":"webhook","enabled":false,"route_by":["bot_context.bot_name","project_type","trigger_kind"]},{"key":"mcp","label":"AI connection","type":"mcp","enabled":true}]',
    '{"example_project_key":"QA","example_issue_title":"Batch Q2026-05 paint adhesion defect"}'
  ),
  (
    'customer_delivery_default',
    'Customer Delivery',
    'Delivery governance for customer milestones, acceptance material, risks, and change requests.',
    'delivery',
    'custom_form',
    'Delivery assistant',
    '{"states":[{"key":"planned","name":"Planned","category":"active","initial":true},{"key":"in_delivery","name":"In Delivery","category":"active"},{"key":"acceptance","name":"Acceptance","category":"active"},{"key":"change_review","name":"Change Review","category":"active"},{"key":"delivered","name":"Delivered","category":"done","terminal":true}],"default_labels":["milestone","acceptance","change-request","payment-risk"]}',
    '{"fields":[{"key":"customer","label":"Customer","type":"text","required":true},{"key":"milestone","label":"Milestone","type":"text","required":true},{"key":"acceptance_status","label":"Acceptance status","type":"select","options":["not_started","submitted","accepted","rejected"],"required":false},{"key":"delivery_risk","label":"Delivery risk","type":"select","options":["low","medium","high"],"required":false}]}',
    '{"resources":[{"kind":"crm_account","label":"Customer account","required":true},{"kind":"document_library","label":"Acceptance materials","required":false}]}',
    '[{"key":"delivery_assistant","label":"Delivery assistant","agent_type":"mcp","provider":"openpr-mcp","model":"mcp-agent","capabilities":["approval.request","documents.extract_summary","comments.create"],"writes_require_approval":true},{"key":"customer_update_assistant","label":"Customer update assistant","agent_type":"webhook","provider":"openpr-webhook","model":"external-agent","capabilities":["comments.create"],"writes_require_approval":false}]',
    '{"risk_model":"delivery_commercial","approval_required_for":["approval.request","change_review"],"human_terms":{"proposal":"approval/change request","invocation":"execution history","mcp":"AI connection","webhook":"external system connection"}}',
    '[{"key":"delivery_notification_bot","label":"Delivery notification connection","type":"webhook","enabled":false,"route_by":["bot_context.bot_name","bot_agent_type","project_type"]},{"key":"mcp","label":"AI connection","type":"mcp","enabled":true},{"key":"crm","label":"Customer system connection","type":"rest","enabled":false}]',
    '{"example_project_key":"DELIV","example_issue_title":"Milestone 2 acceptance materials for ACME"}'
  ),
  (
    'restaurant_ordering_default',
    'Restaurant Ordering',
    'Restaurant ordering with menu SKU, tables, orders, kitchen tickets, cashier receipts, and business reporting.',
    'restaurant',
    'custom_form',
    'Restaurant operator',
    '{"states":[{"key":"draft","name":"Draft","category":"active","initial":true},{"key":"sent_to_kitchen","name":"Sent to Kitchen","category":"active"},{"key":"served","name":"Served","category":"active"},{"key":"paid","name":"Paid","category":"done","terminal":true},{"key":"cancelled","name":"Cancelled","category":"done","terminal":true}],"default_labels":["menu","kitchen","receipt","revenue"]}',
    '{"fields":[{"key":"order_no","label":"Order No.","type":"text","required":true},{"key":"table_no","label":"Table","type":"text","required":true},{"key":"total_amount","label":"Total Amount","type":"amount","amount":{"currency":"CNY","scale":2},"required":false},{"key":"status","label":"Status","type":"single_select","options":["draft","sent_to_kitchen","served","paid","cancelled"],"required":true}]}',
    '{"resources":[{"kind":"site","label":"Restaurant site","required":false},{"kind":"equipment","label":"Thermal printer","required":false}]}',
    '[{"key":"kitchen_assistant","label":"Kitchen routing assistant","agent_type":"mcp","provider":"openpr-mcp","model":"mcp-agent","capabilities":["forms.list","form_records.create","events.tail"],"writes_require_approval":false},{"key":"restaurant_calc","label":"Restaurant calculation plugin","agent_type":"wasm","provider":"openpr-plugin","model":"openpr-plugin-v1","capabilities":["plugins.invoke","form_records.update"],"writes_require_approval":false}]',
    '{"risk_model":"business_operations","approval_required_for":["order.cancelled","price.override"],"human_terms":{"proposal":"business action","invocation":"execution history","mcp":"AI connection","webhook":"external system connection","plugin":"business plugin"}}',
    '[{"key":"mcp","label":"AI connection","type":"mcp","enabled":true},{"key":"kitchen_printer","label":"Kitchen printer","type":"print","enabled":false,"events":["print_job.created"],"connector_kinds":["print"]},{"key":"receipt_printer","label":"Receipt printer","type":"print","enabled":false,"events":["print_job.created"],"connector_kinds":["print"]},{"key":"order_webhook","label":"Order event webhook","type":"webhook","enabled":false,"events":["form.record.created","form.record.updated"],"form_keys":["order","order_line","print_job"]}]',
    '{"example_project_key":"REST","example_issue_title":"Open table A03 dinner order"}'
  )
ON CONFLICT (key) WHERE workspace_id IS NULL DO UPDATE
SET
  name = EXCLUDED.name,
  description = EXCLUDED.description,
  industry = EXCLUDED.industry,
  project_type_key = EXCLUDED.project_type_key,
  audience_label = EXCLUDED.audience_label,
  workflow_template = EXCLUDED.workflow_template,
  field_schema = EXCLUDED.field_schema,
  resource_schema = EXCLUDED.resource_schema,
  ai_roles = EXCLUDED.ai_roles,
  governance_policy = EXCLUDED.governance_policy,
  connector_suggestions = EXCLUDED.connector_suggestions,
  sample_data = EXCLUDED.sample_data,
  updated_at = NOW();
