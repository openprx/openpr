# Scenario Template Catalog

OpenPR scenario templates turn a new project into a ready-to-use business
workspace. A template sets the project type, workflow states, issue fields,
project resources, universal forms, default grid/detail views, connector
suggestions, and optional WASM plugins.

## How to Use a Template

Create a project from the API:

```bash
curl -sS -X POST "$OPENPR_API_URL/api/v1/workspaces/$WORKSPACE_ID/projects" \
  -H "Authorization: Bearer $OPENPR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Restaurant Demo",
    "key": "RESTDEMO",
    "type_key": "restaurant_ordering",
    "scenario_template_key": "restaurant_ordering_default"
  }'
```

Use MCP when an AI or automation client needs the same flow:

```text
scenario_templates.list
scenario_templates.get {"key":"restaurant_ordering_default"}
scenario_templates.install {"project_id":"<project-id>","template_key":"restaurant_ordering_default"}
projects.create {"scenario_template_key":"restaurant_ordering_default", ...}
forms.list {"project_id":"..."}
```

Use the frontend when a non-technical operator creates a project:

1. Open the workspace project list.
2. Pick a scenario template card.
3. Fill the project name/key and scenario fields.
4. Open the generated project Forms page.

Automation can read the same usage contract from
`openpr-universal-form-scenario-catalog-2026-05-31.json`. Each template row
includes operator steps, primary MCP tools, connector kinds, plugin keys, and
acceptance focus points, so frontend, MCP, CLI, webhook, and deployment
consumers can render the same scenario guidance without scraping this Markdown
body.

The runtime REST and MCP surfaces expose the same shape as
`usage_guide` on `GET /api/v1/scenario-templates`,
`GET /api/v1/scenario-templates/{key}`,
`POST /api/v1/projects/{project_id}/scenario-templates/{template_key}/install`,
`scenario_templates.list`, `scenario_templates.get`, `scenario_templates.install`,
and the `openpr://scenario-templates` MCP resource.
That keeps operator onboarding, AI routing, connector setup, and template
marketplace consumers on the same contract as the generated delivery catalog.

## Built-In Templates

| Template key | Project type | Best fit | Generated universal forms | Integrations |
| --- | --- | --- | --- | --- |
| `code_delivery_default` | `code_project` | AI-assisted software delivery, code review, CI/release evidence | `code_task`, `change_record`, `release_check` | MCP coding/review assistants, code execution webhook suggestion |
| `contract_review_default` | `contract_review` | Contract intake, legal/commercial risk review, approval chain | `contract`, `risk_clause`, `approval_record` | MCP document assistant, approval webhook suggestion |
| `equipment_maintenance_default` | `equipment_maintenance` | Fault report, maintenance ownership, inspection acceptance | `equipment`, `repair_order`, `inspection_record` | Maintenance webhook, MCP inspection assistant |
| `quality_corrective_action_default` | `quality_corrective_action` | Defect classification, root-cause analysis, corrective action, recheck | `defect_record`, `root_cause_analysis`, `corrective_action` | Quality audit MCP assistant, recheck webhook |
| `customer_delivery_default` | `customer_delivery` | Customer milestones, acceptance materials, risk and change governance | `customer`, `delivery_milestone`, `change_request` | MCP delivery assistant, customer update webhook, CRM REST suggestion |
| `restaurant_ordering_default` | `restaurant_ordering` | Menu, SKU, table, order, print job, receipt, and daily revenue reporting | `menu_category`, `sku`, `table`, `order`, `order_line`, `print_job`, `business_report` | MCP kitchen assistant, print connectors, order webhook, `restaurant_calc` WASM plugin |

## Scenario Details

### Code Delivery

Use `code_delivery_default` when the project is still software work but needs
structured governance around AI edits. The generated forms record repository
tasks, changed files, verification evidence, release checks, CI status, and
risk level. MCP clients can read project resources and write review evidence
through the same API surface used by the frontend.

### Contract Review

Use `contract_review_default` for document-heavy work where the operator needs
contract master data, clause risk extraction, and approval decisions. Amounts
should use decimal strings at API boundaries. Webhook or MCP assistants can
extract summaries, flag risky clauses, and create approval requests while the
human reviewer signs the final decision.

### Equipment Maintenance

Use `equipment_maintenance_default` for factories, sites, and service teams.
The generated forms track equipment assets, repair orders, downtime impact,
inspection records, attachments, and signatures. The template is suitable for
passive webhooks from a maintenance system and MCP-assisted inspection reports.

### Quality Corrective Action

Use `quality_corrective_action_default` for defect handling and CAPA workflows.
The generated forms connect a defect record to root-cause analysis and the
corrective action plan. This scenario is designed for auditability: every
change can emit business events and every external assistant action can be
recorded as an invocation.

### Customer Delivery

Use `customer_delivery_default` for professional service or implementation
teams that need customer milestones, acceptance status, delivery risk, and
change request tracking. A CRM connector can remain passive at first; OpenPR
can still manage internal acceptance and risk work with universal forms.

### Restaurant Ordering

Use `restaurant_ordering_default` as the reference non-code business workflow.
It proves that OpenPR can run operational data, not just software issues:

1. Create menu categories, SKU, and tables.
2. Create an order and `order_line` children.
3. Let the `restaurant_calc` WASM plugin calculate `order_line.line_total`.
4. Emit table-change and print events.
5. Route kitchen/receipt print jobs to print connectors.
6. Store daily business reports and query revenue through MCP aggregates.

For a local seeded demo, run:

```bash
bash scripts/start.sh
scripts/bootstrap-restaurant-demo.sh
```

For a disposable API -> MCP HTTP verification, run:

```bash
scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh
```

## Extension Rules

When adding a new scenario template:

1. Add or update the project type and scenario template migration data.
2. Add default universal forms and grid/detail views.
3. Add connector suggestions for the expected integration path.
4. Add a WASM plugin only when the scenario needs isolated custom validation,
   formula, event handling, or MCP tool behavior.
5. Cover initialization in `scripts/smoke-scenario-template-forms.sh`.
6. Document the scenario here and include its acceptance evidence in the
   development tracker before marking it `已测试`.
