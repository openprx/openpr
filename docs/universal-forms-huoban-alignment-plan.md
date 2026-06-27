# Universal Forms Huoban-Style Alignment Plan

## Purpose

OpenPR universal forms should become a business-user-facing form builder, not a
JSON schema editor. The next stage aligns the product interaction with the
Huoban-style model:

- add and manage forms visually;
- design fields from a field library;
- edit field properties from a property panel;
- manage records through list/detail/edit flows;
- support relation fields and subform-like child tables as first-class data
  relationships;
- support calculated fields, amount fields, attachment fields, and import/export
  without losing precision or event consistency;
- keep API, MCP, webhook, plugin, and frontend data contracts consistent.

This plan starts from the current OpenPR structure after project types were
collapsed to two lanes:

- `code_project`: project management for code.
- `custom_form`: universal form business data.

Industry scenarios such as restaurant, contract, and equipment are not project
types. They belong to universal form templates and form design.

## Current State

### Database

Implemented in `migrations/0030_universal_forms.sql`.

Current tables:

- `project_forms`: form definition, JSON schema, detail layout.
- `form_views`: grid/detail/kanban/calendar/card/timeline view configs.
- `form_records`: record values stored as JSONB.
- `form_record_links`: parent-child and relation links.
- `form_events`: legacy form events.
- `form_record_field_index`: searchable/indexed projections for text,
  decimal, datetime, and boolean values.

Strengths:

- Flexible enough for custom fields.
- Record links already provide the base for subform and relation behavior.
- Decimal projections already support amount aggregates.
- Business events, MCP, webhooks, and plugins already integrate with form
  record writes.

Remaining gaps:

- No first-class form template catalog table.
- No persistent field-level UI metadata beyond `schema`.
- Export current view v1 and CSV/JSON import preview/commit v1 exist; file
  upload, mapping wizard, and long-running import job model are still pending.
- Form role action permission v1 exists; field-level and record-level policies
  remain pending.
- No first-class formula/calculation execution contract for amount and math
  fields.
- No attachment/image storage contract for universal form fields.

Implemented after the original audit:

- Schema version history through `form_schema_versions`.
- Archive/restore model for forms, views, and records.
- Stable `field_id` in schema fields and projection index.
- Record list filter/sort over indexed fields.

### Backend API

Implemented in `apps/api/src/routes/form.rs` and `apps/api/src/forms/`.

Current REST surface:

- `GET /api/v1/projects/{project_id}/forms`
- `POST /api/v1/projects/{project_id}/forms`
- `GET /api/v1/forms/{form_id}`
- `PATCH /api/v1/forms/{form_id}`
- `DELETE /api/v1/forms/{form_id}`
- `GET /api/v1/forms/{form_id}/views`
- `POST /api/v1/forms/{form_id}/views`
- `DELETE /api/v1/form-views/{view_id}`
- `GET /api/v1/forms/{form_id}/records`
- `POST /api/v1/forms/{form_id}/records`
- `GET /api/v1/form-records/{record_id}`
- `PATCH /api/v1/form-records/{record_id}`
- `DELETE /api/v1/form-records/{record_id}`
- `GET /api/v1/forms/{form_id}/aggregate`
- `GET /api/v1/forms/{form_id}/events`
- `GET /api/v1/form-records/{record_id}/events`
- `GET /api/v1/form-records/{record_id}/links`
- `POST /api/v1/form-records/{record_id}/links`

Strengths:

- Form and record CRUD exist.
- Schema validation exists.
- Record value normalization exists.
- Formula hooks, validator hooks, event hooks, MCP, and plugin surfaces are
  already wired.

Remaining gaps:

- Field add/edit/delete is only represented as full-form schema patch.
- Backend does not expose a field-level schema operation API.
- Relation field configuration is too loose for business users.
- Relation lookup and child-record query endpoints exist, but relation
  configuration and layout policy are still incomplete.
- Single-form template instantiate, form duplicate, and full multi-form scenario
  bundle install into an existing project now exist.
- No idempotency/retry contract for automation-triggered form writes.

Implemented after the original audit:

- View update endpoint.
- Schema summary, field usage, and field dependency endpoints.
- Record list filtering and sorting by projected/indexed fields.
- Schema optimistic locking through `expected_schema_version`.
- Record export endpoint for a saved view or explicit column set.

### Frontend

Implemented mainly in:

- `frontend/src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/+page.svelte`
- `frontend/src/lib/api/forms.ts`

Current behavior:

- Form list and selected form are in one page.
- Form creation has a basic field builder plus advanced JSON.
- Record creation/editing happens on the same page.
- Record table and right-side detail panel exist.
- Record links can be added manually.

Strengths:

- The first no-code field builder exists.
- Record input now uses type-aware controls for select, multi-select, amount,
  boolean, date, and text.
- Advanced JSON is already hidden behind a developer path.

Gaps:

- Dedicated modes and designer v1 now exist, but they still live inside the
  current page rather than durable routes.
- Field add/edit/delete/duplicate/reorder now exist, but backend-level schema
  migration operations are still represented as a full schema patch.
- Saved view builder v1 now supports grid column selection, but filters, sort,
  grouping, default-view selection, and ownership are still pending.
- Export current view v1 exists for business users. CSV/JSON import
  preview/commit v1 exists; file upload and mapping wizard workflows are still
  pending.
- No dedicated record detail route.
- No safe delete/empty state/error states for full workflows.
- Relation picker, child-record display, and parent-detail inline child
  add/edit/delete workflows now exist, with deployed browser smoke coverage.
- No role-aware field visibility or read-only UI.
- Attachment and image fields do not yet have a complete upload/preview/remove
  workflow.

## Target Product Model

### Top-Level Information Architecture

Inside a `custom_form` project, forms should have four primary modes:

1. `数据`
   Record list, filters, sort, views, inline actions.

2. `详情`
   Record detail drawer/page, field sections, linked records, child tables,
   event history.

3. `设计`
   Huoban-style builder: left field library, center form canvas, right property
   panel.

4. `自动化`
   MCP/webhook/plugin/event bindings for this form. This can initially link to
   existing connection/plugin surfaces and later become a dedicated form
   automation page.

### Recommended Routes

Keep the current page for compatibility, but split real workflows:

- `/workspace/{workspaceId}/projects/{projectId}/forms`
  Form home. Shows form list, create button, and selected form data grid.

- `/workspace/{workspaceId}/projects/{projectId}/forms/{formId}`
  Data grid for one form.

- `/workspace/{workspaceId}/projects/{projectId}/forms/{formId}/designer`
  Form designer.

- `/workspace/{workspaceId}/projects/{projectId}/forms/{formId}/records/{recordId}`
  Record detail page or drawer route.

- `/workspace/{workspaceId}/projects/{projectId}/forms/{formId}/settings`
  Form-level settings, archive/delete, title template, API/MCP exposure.

If route work is deferred, implement these as tabs inside the current page
first, then split routes after the UX is stable.

## Huoban-Style Designer Interaction

### Layout

Use a three-panel builder:

- Left panel: field library.
- Center panel: form canvas.
- Right panel: selected field/form properties.

The visual tone should be dense, utilitarian, and business-software oriented:
small controls, clear labels, restrained borders, no marketing layout.

### Left Field Library

Field groups:

- 基础字段: text, textarea, rich_text, number, integer, date, datetime,
  boolean.
- 选项字段: single_select, multi_select.
- 业务字段: amount, attachment, image.
- 关联字段: relation, child_table.
- 自动化字段: formula, ai_summary.

Interactions:

- Click a field type to append it to the canvas.
- Drag a field type into the canvas if drag-and-drop is available.
- Search field types.
- Field type cards show icon, name, and short description.

### Center Canvas

Canvas object model:

- Fields are ordered.
- Fields can be grouped into sections.
- Detail layout can render one-column or two-column rows.
- Selected field is highlighted.
- Empty canvas shows a compact add-field prompt.

Interactions:

- Select field.
- Rename field inline.
- Reorder fields.
- Duplicate field.
- Delete field with confirmation if records already contain data.
- Toggle required quickly.
- Add section.
- Move field across sections.

### Right Property Panel

Panel modes:

- Form properties when no field is selected.
- Field properties when a field is selected.
- Relation properties when relation/child table field is selected.

Form properties:

- Form name.
- Form key.
- Description.
- Title template.
- Icon/color.
- Default view.
- API/MCP exposure toggle.
- Webhook event exposure.
- Permissions: who can view, add, edit, delete, export, or design.
- Import/export policy.

Field properties:

- Field label.
- Field key.
- Field type.
- Required.
- Help text.
- Placeholder.
- Default value.
- Validation.
- Visibility condition.
- Read-only condition.
- List visibility.
- Detail visibility.
- Role visibility and read-only rules.
- Unique, regex, min/max, length, and value-range validation.
- Field ID, shown as read-only technical identity after creation.

Amount properties:

- Currency.
- Scale.
- Negative allowed.
- Rounding mode.

Select properties:

- Options list.
- Option color.
- Default option.
- Allow empty.

Relation properties:

- Target form.
- Single/multiple selection.
- Display field.
- Search fields.
- Filter expression.
- Sort order.
- Whether creating target records inline is allowed.
- Whether to show attached target fields in list/detail.

Child-table properties:

- Target child form.
- Parent relation key.
- Display columns.
- Inline add/edit allowed.
- Delete child records allowed.
- Aggregate columns, for example sum amount or count rows.

View properties:

- View name.
- View type.
- Visible columns.
- Column order and width.
- Default sort.
- Filter conditions.
- Grouping.
- Row density.
- Whether the view is personal, shared, or default.

System fields:

- Created time.
- Updated time.
- Creator.
- Last updater.
- Record ID.
- Schema version.

These should be available as display/filter/sort fields even if they are not
ordinary user-defined fields.

## Regression Audit Additions

The first plan covered the major CRUD and designer direction, but the regression
audit found missing delivery-critical areas. These are now explicit scope:

1. Stable field identity.
   Field `key` is user-facing and may need to change. The schema needs a stable
   `field_id` so rename, label changes, and layout changes do not accidentally
   orphan stored values.

2. Field dependency graph.
   Before deleting or changing a field, the system must detect references from
   views, formulas, AI summaries, relation filters, child tables, title
   templates, webhooks, MCP tools, and plugins.

3. Permission model.
   The next stage needs form-level, record-level, and field-level permission
   hooks. Without this, traditional business users cannot separate designers,
   operators, reviewers, and viewers.

4. Concurrency and conflict handling.
   Designer save must detect schema version mismatch and show a conflict state
   instead of silently overwriting another user's edits.

5. Data migration strategy.
   Schema changes need explicit behaviors: rename field, hide field, archive
   field, change type, convert data, or leave old data as historical.

6. Saved views.
   Huoban-style usage depends on multiple views over the same form. Grid column
   choice, filters, sort, grouping, and default view cannot be treated as a
   single generic config hidden from users.

7. Import/export.
   Traditional teams expect spreadsheet import/export. This is required for
   initial adoption even if it ships after designer v1.

8. Relation lookup and child queries.
   Relation fields need search APIs and child table needs parent-child query
   APIs. Raw `form_record_links` is not enough for the frontend workflow.

9. Template catalog.
   Restaurant, contract, equipment, quality, and delivery must become universal
   form templates with categories, preview, version, and install/instantiate
   behavior.

10. Audit and recoverability.
    Archive, restore, schema history, record history, and event diff are needed
    before destructive business use is acceptable.

11. Decimal-safe calculations.
    Amount, quantity, price, tax, discount, totals, and reports need a dedicated
    Decimal calculation contract in Rust. JSON numbers and floating point are
    not acceptable for persisted business values.

12. Attachment and image lifecycle.
    File fields need upload, preview, permission checks, virus/security policy
    hooks if deployed by an operator, delete/restore behavior, and export
    handling.

13. Automation reliability.
    MCP, webhooks, plugins, and connectors need the same event envelope,
    idempotency key, retry state, and failure visibility. Printing is a
    connector/plugin consumer of form events, not a special frontend action.

14. Phase markers.
    Each phase needs separate `开发`, `测试`, and `验收` markers. A phase is not
    complete just because code exists.

## CRUD Workflows

### Form Create

Entry:

- `新建表单` from form home.
- `从模板创建` should be inside universal forms, not project creation.

Flow:

1. Choose blank form or form template.
2. Enter form name and key.
3. Enter designer with empty canvas or template fields.
4. Save creates `project_forms`.
5. Default grid/detail views are created.

Acceptance:

- Business user can create a blank form without JSON.
- Form appears in form list immediately.
- Default grid view and detail layout work.
- MCP `forms.list` sees the form.

### Form Edit

Entry:

- `设计` tab or designer route.
- Form settings menu.

Flow:

1. Load form schema into designer.
2. User adds, edits, deletes, and reorders fields.
3. Save validates schema and patches `project_forms.schema` and
   `detail_layout`.
4. Existing records remain readable.
5. Changed field projections are refreshed where needed.

Acceptance:

- Field label/type/required/options/amount config can be edited from UI.
- Field reorder changes grid/detail display order.
- Existing records still render after schema changes.
- Unsafe changes show warning, for example deleting a field with existing data.

### Form Delete

Current backend hard-deletes forms. Next stage should add a safer archive path.

Recommended behavior:

- Primary UI action: archive form.
- Destructive action: permanently delete form, hidden behind confirmation.
- Confirmation shows record count and dependent links.

Backend options:

- Minimal v1: keep `DELETE /forms/{id}` but show record count in UI before
  delete.
- Better v1.5: add `archived_at` to `project_forms`, `form_views`, and
  `form_records`, then implement archive/restore.

Acceptance:

- User cannot delete a populated form without seeing record count.
- Deleting a form removes it from visible list.
- Event `form.deleted` or `form.archived` is emitted.

### Record Create

Entry:

- `新增记录` in grid.
- Inline add inside child table.

Flow:

1. Open drawer/page with generated input controls.
2. Validate required fields client-side.
3. Submit values to existing record create API.
4. Backend normalizes amount/integer/decimal values.
5. Grid updates and events/hooks run.

Acceptance:

- All supported field types render usable inputs.
- Amount never submits JSON numbers; it submits decimal strings or amount
  objects accepted by backend.
- Relation field uses record picker instead of raw IDs.
- Child table can create child records linked to parent record.

### Record Edit

Entry:

- Row action.
- Detail page edit button.
- Inline child table row edit.

Flow:

1. Load record and schema.
2. Render same controls as create.
3. Save patches record values.
4. Projection refresh and business events run.

Acceptance:

- Record edit preserves unrelated fields.
- Relation and child-table fields remain linked after edit.
- Updated record renders in grid and detail.

### Record Delete

Recommended behavior:

- Show confirmation with title and linked child count.
- For parent records, warn if child records exist.
- Support delete child row independently.

Backend options:

- Minimal v1: use existing hard delete endpoint.
- Better v1.5: add `archived_at` to records.

Acceptance:

- Deleted record disappears from grid.
- Business event is emitted.
- Parent detail no longer shows deleted child row.

### Record Detail

Detail should not be a small JSON panel. It should be a business detail view.

Sections:

- Header: title, form name, status if present, actions.
- Main fields: rendered by `detail_layout`.
- Child tables: linked records where relation type is `parent_child`.
- Related records: normal relation links.
- Event history: form events/business events.
- Automation: latest plugin/webhook/MCP invocations relevant to record.

Acceptance:

- Clicking a grid row opens readable detail.
- All field types render display values.
- Child tables render as embedded lists.
- Record links can be opened and created without raw JSON.

### Bulk Import and Export

Entry:

- `导入` and `导出` in form grid.
- Template download from form settings.

Flow:

1. Export current view or all records.
2. Import spreadsheet/CSV with field mapping.
3. Preview validation errors before writing.
4. Write valid rows through the same record create/update normalization path.
5. Emit business events and outbox events.

Acceptance:

- User can download a template for a form.
- User can import valid rows.
- Invalid rows show field-level errors before commit.
- Amounts remain decimal-safe.
- Import writes are visible to MCP/webhook/plugin events.

### Calculated Fields and Amount Math

Amount and math fields are business-critical. The implementation must be
decimal-safe end to end:

- frontend inputs submit amount, quantity, and decimal values as strings or
  structured amount objects;
- backend parses with Rust decimal types, never with `f64`;
- stored JSONB values keep the display value and canonical decimal string;
- `form_record_field_index.decimal_value` is the query/sort/aggregate source;
- formulas run on normalized values and write calculated fields through the
  same record normalization path;
- import, MCP writes, webhook writes, plugin writes, and UI writes all trigger
  the same calculation pipeline;
- rounding mode, scale, currency, and negative-value rules come from field
  properties;
- failed formula evaluation returns field-level errors and does not partially
  write the record.

Calculation examples:

- restaurant order total = sum child row `quantity * unit_price`;
- tax = subtotal * tax rate with configured rounding;
- contract amount = base amount + change orders;
- equipment cost = parts + labor.

Acceptance:

- No persisted amount uses floating point.
- Formula dependency changes are detected before field delete/type change.
- Child-table aggregate fields update when child rows are added, edited, or
  archived.
- Import preview shows calculation errors before commit.

### Attachment and Image Fields

Attachment-like fields need explicit storage behavior:

Flow:

1. Upload file through an authenticated file endpoint.
2. Store file metadata in the record value, not raw binary in JSONB.
3. Render preview/download/remove controls in create, edit, detail, and grid
   where configured.
4. Archive record keeps file references available for audit.
5. Export includes file metadata and optionally signed download URLs.

Acceptance:

- A user can add, preview, and remove an attachment from a record.
- Image fields render thumbnails without breaking table layout.
- Permission checks apply to download and preview.
- Record import can map external file URLs only if operator policy allows it.

### View Create/Edit/Delete

Entry:

- View switcher in form grid.
- `管理视图` in grid toolbar.

Flow:

1. Create grid/detail/card/kanban/calendar view.
2. Configure columns, filters, sort, grouping, density.
3. Save view config through API.
4. Set shared/default view if allowed.

Acceptance:

- Multiple views can exist for the same form.
- View update persists and reloads.
- Deleting a view does not delete records.
- Default view is used when opening a form.

## Backend Development Plan

### Phase B1: API Completion

Add endpoints:

- `PATCH /api/v1/form-views/{view_id}`
- `GET /api/v1/forms/{form_id}/schema-summary`
- `GET /api/v1/forms/{form_id}/field-usage`
- `GET /api/v1/forms/{form_id}/field-dependencies`
- `POST /api/v1/forms/{form_id}/duplicate`
- `POST /api/v1/projects/{project_id}/forms/from-template`
- `GET /api/v1/forms/{form_id}/relation-targets`
- `GET /api/v1/form-records/{record_id}/children`
- `POST /api/v1/forms/{form_id}/records/recalculate-preview`
- `POST /api/v1/form-records/{record_id}/recalculate`
- `POST /api/v1/forms/{form_id}/attachments`
- `POST /api/v1/forms/{form_id}/records/import-preview`
- `POST /api/v1/forms/{form_id}/records/import`
- `GET /api/v1/forms/{form_id}/records/export`

Add query support:

- `GET /api/v1/forms/{form_id}/records?sort=field:asc`
- `GET /api/v1/forms/{form_id}/records?filter=...`

Use `form_record_field_index` for common filters and sort:

- text equality/search;
- decimal range;
- datetime range;
- boolean equality.

Add optimistic locking:

- update form requires `expected_schema_version` when schema changes;
- update view requires `expected_updated_at` or version;
- stale requests return conflict with current version summary.

Add write idempotency:

- record create/update/archive accepts optional idempotency key;
- MCP, webhook, plugin, connector, and import writes must pass an idempotency
  key;
- duplicate delivery returns the original result instead of writing twice.

### Phase B2: Schema Versioning

Add table:

```sql
CREATE TABLE form_schema_versions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  form_id UUID NOT NULL REFERENCES project_forms(id) ON DELETE CASCADE,
  version INTEGER NOT NULL,
  schema JSONB NOT NULL,
  detail_layout JSONB NOT NULL DEFAULT '{}'::jsonb,
  changed_by UUID REFERENCES users(id) ON DELETE SET NULL,
  change_summary TEXT NOT NULL DEFAULT '',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(form_id, version)
);
```

Add columns:

```sql
ALTER TABLE project_forms ADD COLUMN IF NOT EXISTS schema_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE form_records ADD COLUMN IF NOT EXISTS schema_version INTEGER;
```

Extend schema fields:

```json
{
  "field_id": "fld_01h...",
  "key": "amount",
  "label": "Amount",
  "type": "amount"
}
```

Rules:

- `field_id` is generated once and never changes.
- `key` can be renamed only through a rename operation or designer migration.
- projections use `key` for query compatibility, but schema history tracks
  `field_id`.

Behavior:

- On form create, create schema version 1.
- On schema update, increment version and insert history.
- On record create, store current schema version.
- On record render, use current schema for display, but keep original
  schema_version available for audits.

### Phase B3: Safe Archive

Add:

```sql
ALTER TABLE project_forms ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ;
ALTER TABLE form_records ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ;
ALTER TABLE form_views ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ;
```

Endpoints:

- `POST /api/v1/forms/{form_id}/archive`
- `POST /api/v1/forms/{form_id}/restore`
- `POST /api/v1/form-records/{record_id}/archive`
- `POST /api/v1/form-records/{record_id}/restore`

Keep hard delete available for admin/internal use only if needed.

### Phase B4: Relation and Child Table Contracts

Extend schema metadata for relation fields:

```json
{
  "key": "customer_id",
  "label": "Customer",
  "type": "relation",
  "relation": {
    "target_form_key": "customer",
    "mode": "single",
    "display_field": "customer_name",
    "search_fields": ["customer_name", "phone"],
    "filter": {},
    "sort": [{"field": "customer_name", "direction": "asc"}],
    "inline_create": true
  }
}
```

Add child table as layout/widget metadata, not a scalar field:

```json
{
  "type": "child_table",
  "key": "order_lines",
  "target_form_key": "order_line",
  "parent_field_key": "order_id",
  "columns": ["sku_name", "quantity", "unit_price", "line_total"],
  "inline_create": true,
  "inline_edit": true,
  "aggregates": [{"field": "line_total", "aggregate": "sum"}]
}
```

This keeps subforms data-level consistent with existing `form_record_links` and
avoids embedding child rows inside one JSON record.

### Phase B5: Calculation Engine

Add a shared calculation service used by UI writes, imports, MCP, webhooks, and
plugins:

- normalize input values;
- evaluate formulas using decimal-safe arithmetic;
- evaluate child-table aggregates;
- refresh indexed projections;
- return field-level validation/calculation errors;
- emit one stable business event after successful write.

Recommended implementation boundary:

- `apps/api/src/forms/normalization.rs`: parse and normalize raw values.
- `apps/api/src/forms/calculation.rs`: evaluate formulas and aggregates.
- `apps/api/src/forms/dependencies.rs`: detect field dependency graph.
- `apps/api/src/forms/projection.rs`: refresh `form_record_field_index`.

Formula v1 should stay intentionally small:

- arithmetic: add, subtract, multiply, divide;
- aggregate: sum, count, min, max over child rows;
- functions: round, coalesce, concat;
- no arbitrary code execution.

### Phase B6: Attachment Storage

Add file metadata for form fields:

```sql
CREATE TABLE form_attachments (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  form_id UUID NOT NULL REFERENCES project_forms(id) ON DELETE CASCADE,
  record_id UUID REFERENCES form_records(id) ON DELETE SET NULL,
  field_id TEXT NOT NULL,
  file_name TEXT NOT NULL,
  content_type TEXT NOT NULL DEFAULT '',
  byte_size BIGINT NOT NULL DEFAULT 0,
  storage_key TEXT NOT NULL,
  created_by UUID REFERENCES users(id) ON DELETE SET NULL,
  archived_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

The storage backend can initially be local/operator-configured. The form value
stores attachment IDs and display metadata.

### Phase B7: Permissions and Audit

Add a minimal permission model:

```sql
CREATE TABLE form_permissions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  form_id UUID NOT NULL REFERENCES project_forms(id) ON DELETE CASCADE,
  subject_type TEXT NOT NULL,
  subject_id TEXT NOT NULL,
  policy JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

Initial policy actions:

- `form.view`
- `form.design`
- `record.create`
- `record.update`
- `record.delete`
- `record.export`

Field-level visibility can start as JSON policy in schema metadata and later
move into a dedicated table if needed.

### Phase B8: Template Catalog

Add first-class template storage:

```sql
CREATE TABLE form_templates (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  key TEXT NOT NULL,
  workspace_id UUID REFERENCES workspaces(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  category TEXT NOT NULL DEFAULT 'general',
  schema JSONB NOT NULL,
  detail_layout JSONB NOT NULL DEFAULT '{}'::jsonb,
  views JSONB NOT NULL DEFAULT '[]'::jsonb,
  sample_records JSONB NOT NULL DEFAULT '[]'::jsonb,
  version INTEGER NOT NULL DEFAULT 1,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

Template install creates `project_forms`, `form_views`, and optional sample
records through normal APIs so events remain consistent.

## Frontend Development Plan

### Phase F1: Split Forms UI Into Workflows

Create components:

- `FormShell.svelte`: header, form switcher, mode tabs.
- `FormListPanel.svelte`: forms list, create, archive.
- `FormGrid.svelte`: records table, view selection, filters.
- `RecordEditor.svelte`: create/edit drawer.
- `RecordDetail.svelte`: detail layout, links, child tables, events.
- `FormDesigner.svelte`: three-panel builder.
- `FieldLibrary.svelte`: grouped field type palette.
- `FormCanvas.svelte`: selected/reordered fields and sections.
- `FieldPropertyPanel.svelte`: properties by field type.
- `RelationPicker.svelte`: relation record selector.
- `ChildTable.svelte`: embedded child record list/editor.

### Phase F2: Designer v1

Functions:

- Add field from library.
- Select field.
- Edit label/key/type/required.
- Edit options.
- Edit amount config.
- Edit formula config.
- Edit attachment/image config.
- Edit help text, placeholder, default value, validation, and visibility.
- Reorder fields.
- Delete field.
- Duplicate field.
- Show field dependencies before destructive changes.
- Save schema.
- Load schema into designer.

Constraints:

- Changing field key after records exist must show warning.
- Deleting field after records exist must show usage count.
- Type changes across incompatible families must show warning.
- Formula fields show dependency and calculation preview.
- Attachment fields show accepted file types, size limit, and preview behavior.

### Phase F3: Record List and Detail v1

Functions:

- Grid columns from schema and view config.
- Saved view switcher.
- Filter/sort/group controls.
- Click row to open detail.
- Create/edit/delete record from drawer.
- Detail page or drawer shows all fields.
- Events and links render in detail.
- Import/export buttons.
- Recalculate action for formula/aggregate fields when a designer changes
  formula behavior.

### Phase F4: Relation and Child Table v1

Functions:

- Relation field property panel.
- Relation record picker.
- Child table layout block.
- Inline add child record.
- Inline edit/delete child record.
- Parent detail shows child rows and aggregates.
- Child table changes refresh parent aggregate/calculated fields.

### Phase F5: Formula, Amount, and Attachment v1

Functions:

- Formula field property editor.
- Calculation preview against sample/current record.
- Amount formatting with currency, scale, and rounding metadata.
- Attachment upload, preview, remove, and download.
- Image thumbnails in grid/detail where enabled.
- Field-level error rendering for calculation and upload failures.

Acceptance:

- Restaurant menu/order totals can be modeled without custom code.
- Amount values keep exact decimal display after create/edit/import.
- Attachment fields work in create, edit, detail, and export metadata.

### Phase F6: Template Library v1

Move industry scenarios into universal form templates:

- Restaurant order system.
- Contract review.
- Equipment maintenance.
- Customer delivery.
- Quality corrective action.

Template entry belongs in universal forms:

- `新建表单 -> 从模板创建`
- `表单模板库`

Not in project type creation.

### Phase F7: Automation Bindings UI

Functions:

- Form automation tab lists MCP tools, webhook subscriptions, plugin bindings,
  and connector bindings for this form.
- Show last delivery status, retry count, and failure reason.
- Allow operators to bind events such as `form.record.created` to a print
  connector/plugin without changing the form UI.
- Keep `@AI` execution and openprx-webhooks as automation consumers of the same
  event model, not a separate project-management-only path.

Acceptance:

- A form record create can trigger webhook/plugin/MCP/connector through the
  same business event.
- Failed automation is visible and retryable by an operator.
- Printing is configured as a connector/plugin binding and does not require a
  hardcoded frontend action.

### Phase F8: Permissions, Empty States, and Accessibility

Functions:

- Hide or disable actions based on form permission policy.
- Clear empty states for no forms, no fields, no records, no child records.
- Keyboard navigable designer panels.
- Focus states for field library, canvas, and property panel.
- Confirmation dialogs for destructive actions.
- Actionable error messages with field labels.

Acceptance:

- A non-designer cannot enter designer mode.
- Keyboard users can add/select/edit fields.
- Empty and error states tell users what to do next.

## MCP, Webhook, Plugin Alignment

Every form operation should keep event semantics stable:

- `form.created`
- `form.updated`
- `form.archived`
- `form.record.created`
- `form.record.updated`
- `form.record.archived`
- `form.schema.updated`
- `form.view.updated`
- `form.record.linked`

MCP tools should expose:

- forms list/get/create/update/archive;
- forms duplicate;
- form schema summary;
- field usage and field dependencies;
- form records query/get/create/update/archive;
- relation lookup;
- child records list/create/update;
- form templates list/install;
- schema versions list/get;
- import/export, permissions, attachments, and idempotency receipts.

Webhook and plugin contracts should consume the same events. There should be
no separate frontend-only workflow.

Shared event envelope:

- `event_id`
- `event_type`
- `workspace_id`
- `project_id`
- `form_id`
- `record_id`
- `schema_version`
- `occurred_at`
- `actor`
- `idempotency_key`
- `payload`

Delivery rules:

- event writes go through outbox/inbox where available;
- webhook, plugin, MCP, and connector delivery records keep status, retry count,
  last error, and timestamps;
- consumer retries must not duplicate form records or print jobs;
- connector consumers, including thermal printing, are event consumers and can
  be implemented by native connector code or WASM plugin code.

## Testing Plan

### Backend

Commands:

```bash
cargo test -p api forms::
cargo check -p api
scripts/smoke-universal-forms-api.sh
scripts/smoke-form-events-outbox.sh
scripts/smoke-forms-mcp.sh
```

New backend tests:

- schema version insert on create/update;
- archive form hides from list;
- archive record hides from list;
- field usage count;
- relation schema validation;
- record filter/sort against `form_record_field_index`;
- child-table link creation.
- optimistic locking conflict;
- field ID preserved after key rename;
- template install creates form and views;
- import preview rejects invalid rows without writing;
- permission denial for non-designer actions.
- decimal formula and child aggregate use exact values;
- idempotency prevents duplicate webhook/plugin/import writes;
- attachment metadata persists and permission checks apply;
- event envelope contains schema version and idempotency key.

### Frontend

Commands:

```bash
cd frontend
npm run check
npm run build
```

New browser smokes:

- create blank form through designer;
- edit field properties and save;
- delete field with warning;
- create record;
- edit record;
- delete/archive record;
- open record detail;
- create relation field and pick related record;
- create child table and inline child record.
- create saved view and reload it;
- import preview with invalid rows;
- export current view;
- permission-gated designer entry;
- keyboard add/select/edit field path.
- formula preview and recalculation;
- attachment upload/preview/remove;
- automation binding status page;
- print connector binding visible as an event consumer.

### Production/Integration

Commands:

```bash
scripts/smoke-universal-forms-api.sh
scripts/smoke-forms-mcp.sh
scripts/smoke-webhook-generic-consumer.sh
scripts/smoke-wasm-plugin-runtime.sh
scripts/smoke-restaurant-ordering.sh
scripts/smoke-print-connector.sh
```

## Delivery Phases and Acceptance Markers

| Phase | Scope | 开发 | 测试 | 验收 | Automated gate | Human acceptance |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | Current structure audit and Huoban-style design plan | `已完成` | `已测试` | `待处理` | Document exists | User approves direction |
| 2 | Backend API completion: view update, schema summary, field usage, record query | `已完成` | `已测试` | `待处理` | API tests and smokes pass | API contracts accepted |
| 3 | Schema versioning, archive model, field identity, optimistic locking | `已完成` | `已测试` | `待处理` | migration tests, API tests | destructive-flow acceptance |
| 4A | Decimal formula engine and attachment metadata | `已完成` | `已测试` | `待处理` | calculation/attachment smokes pass | money and metadata flows accepted |
| 4B | Child aggregates and attachment metadata lifecycle APIs | `已完成` | `已测试` | `待处理` | child aggregate/attachment lifecycle smokes pass | child totals and file lifecycle accepted |
| 5 | Frontend designer v1: field library, canvas, property panel | `已完成` | `已测试` | `待处理` | check/build/browser smoke | business user can design form |
| 6 | Record list/detail CRUD v1 | `已完成` | `已测试` | `待处理` | browser CRUD smoke | business user can operate data |
| 7 | Relation and child table v1 | `已完成` | `已测试` | `待处理` | relation/child table smoke | parent-child data accepted |
| 8 | Template library v1 | `已完成` | `已测试` | `待处理` | template create smoke | scenarios moved out of project type |
| 9A | Saved views builder | `已完成` | `已测试` | `待处理` | saved-view browser smoke | business user can manage grid views |
| 9B | Current-view export v1 | `已完成` | `已测试` | `待处理` | export browser/API smoke | business user accepts CSV export |
| 9C | CSV/JSON paste import v1 | `已完成` | `已测试` | `待处理` | import browser/API smoke | business user accepts import workflow |
| 9D | Role action permission policy v1 | `已完成` | `已测试` | `待处理` | permission browser/API smoke | business user accepts permission model |
| 9E | Form duplicate workflow v1 | `已完成` | `已测试` | `待处理` | duplicate browser/API smoke | business user accepts duplicate semantics |
| 9R | Remaining Phase 9 governance: field/record permissions, broader import/export, saved-view filters/sort/group/default/ownership | `已完成` | `已测试` | `待处理` | source coverage audit, forms UI smoke, deployed import/export/permissions smokes | business ops accepted |
| 10A | MCP form duplicate parity | `已完成` | `已测试` | `待处理` | MCP registry and duplicate smoke | external automation accepts duplicate semantics |
| 10B | MCP form metadata and relation read parity | `已完成` | `已测试` | `待处理` | live MCP 94-tool registry and MCP form smoke | external automation can inspect schema, dependencies, relation targets, and child rows |
| 10C | MCP form permission and attachment metadata parity | `已完成` | `已测试` | `待处理` | MCP 94-tool registry and forms MCP smoke | external automation can manage role policies and attachment metadata |
| 10D | MCP template and import/export parity | `已完成` | `已测试` | `待处理` | MCP 98-tool registry and forms MCP smoke | external automation can create template forms, export records, preview import rows, and commit imports |
| 10E | MCP schema version history parity | `已完成` | `已测试` | `待处理` | MCP 100-tool registry and forms MCP smoke | external automation can inspect baseline and updated schema versions |
| 10F | MCP child record write ergonomics | `已完成` | `已测试` | `待处理` | MCP 102-tool registry and forms MCP smoke | external automation can create and update linked child rows without manual two-step link orchestration |
| 10G | MCP child record lifecycle ergonomics | `已完成` | `已测试` | `待处理` | MCP 104-tool registry and forms MCP smoke | external automation can archive and restore linked child rows without falling back to generic record archive/restore |
| 10H | MCP scenario bundle install parity | `已完成` | `已测试` | `待处理` | MCP 105-tool registry and forms MCP smoke | external automation can install complete multi-form scenario bundles into existing custom-form projects |
| 10I | API/MCP/import form record write idempotency receipts v1 | `已完成` | `已测试` | `待处理` | forms MCP smoke and source coverage audit | external automation can retry record create and import writes without duplicate records |
| 10R | Remaining MCP/webhook/plugin/connector alignment | `已完成` | `已测试` | `待处理` | source coverage audit, forms MCP smoke, webhook/connector/plugin smokes, production automation/object-storage smokes | external automation accepted |

## Definition of Done

A phase is not complete until:

- code is implemented;
- database migration is present if needed;
- REST API contract is typed in frontend API client;
- frontend workflow is reachable from the deployed UI;
- `npm run check` and `npm run build` pass;
- relevant Rust tests or smoke scripts pass;
- browser smoke validates the actual deployed page at `0.0.0.0:3000`;
- the phase has explicit `开发`, `测试`, and `验收` markers using only
  `待处理 / 已完成 / 已测试 / 已验收`.

## Implementation Log

### 2026-06-02 Return Regression Audit

Written audit report:

- `docs/universal-forms-regression-audit-2026-06-02.md`

Findings:

- There were omissions in implementation and status tracking.
- The older development-status artifact can overstate progress because it tracks
  scenario/delivery-bundle evidence, while this Huoban plan tracks product
  workflows.
- Phase 4B had compile/API-client/route gaps; they were corrected, tested,
  deployed, and verified with authenticated HTTP smoke.
- Phase 5 designer v1 is now `已完成 / 已测试 / 待处理`.
- Phase 6 record list/detail CRUD v1 is now `已完成 / 已测试 / 待处理`.
- Phase 7 relation picker, child record display, inline child CRUD, and parent
  child-sum refresh are implemented/tested; the overall Phase 7 row is now
  `已完成 / 已测试 / 待处理`.
- Phase 8 template library v1 is now `已完成 / 已测试 / 待处理`.
- Form duplicate workflow v1 is now `已完成 / 已测试 / 待处理`.
- Phase 9A saved views builder, Phase 9B current-view export, Phase 9C
  CSV/JSON import v1, Phase 9D role permission policy v1, and Phase 9R
  governance are now complete/tested; remaining acceptance is human signoff.
- Phase 10A MCP form duplicate parity is now `已完成 / 已测试 / 待处理`.
- Phase 10B MCP form metadata and relation read parity is now
  `已完成 / 已测试 / 待处理`.
- The phase table now splits Phase 9 into completed tested slices, including
  the 9R governance closure, so the project status no longer hides completed
  Phase 9A-9R work behind a single `待处理` row.
- Signoff JSON has two different counters: `manual_signoff.pending_rows = 7`
  reflects real user-side acceptance still pending, while
  `release_requirement.pending_rows = 0` is a schema-pinned release precondition
  constant. Use `manual_signoff` for current acceptance status.
- No phase is currently `已验收`.

### 2026-06-02 Phase 3 Backend Foundation

Implemented:

- Added migration `0036_universal_forms_schema_version_archive.sql`.
- Added `schema_version` and `archived_at` to forms, views, and records.
- Added `form_schema_versions` baseline/history table.
- Added stable `field_id` backfill for existing form schemas.
- Added `field_id` to `form_record_field_index`.
- Form create/update now fills missing `field_id`.
- Schema/detail updates now increment `schema_version` and write schema history.
- Optional `expected_schema_version` now returns conflict on stale schema writes.
- Form/view/record delete now archives instead of hard-deleting.
- Added restore endpoints for forms and records.
- Added view update endpoint.
- Scenario-template form creation now normalizes `field_id` and writes baseline
  schema versions.
- Frontend forms API client now exposes `schema_version`, `archived_at`, view
  CRUD, and restore calls.

Verified:

- `cargo check -p api`
- `cargo test -p api forms::`
- `cd frontend && npm run check`
- `cd frontend && npm run build`
- Applied migration to local `openpr_postgres_1`; 6 existing forms received
  stable field IDs and 6 baseline schema versions.
- Rebuilt release API binary and replaced `/app/api` inside `openpr_api_1`.
- Health checks passed at `127.0.0.1:8081` and `10.72.0.3:8081`.
- Authenticated HTTP smoke passed for form create, generated `field_id`,
  schema version increment, stale schema `409`, view update, record
  archive/restore, and form archive/restore.
- Temporary smoke form was archived after verification.

Still pending before Phase 3 can be marked `已验收`:

- Browser smoke for destructive-flow confirmation after frontend UI uses these
  endpoints.
- Human acceptance of destructive-flow behavior.

### 2026-06-02 Phase 2 Backend Query APIs

Implemented:

- Added `GET /api/v1/forms/{form_id}/schema-summary`.
- Added `GET /api/v1/forms/{form_id}/field-usage`.
- Added `GET /api/v1/forms/{form_id}/field-dependencies`.
- Extended `GET /api/v1/forms/{form_id}/records` with `sort`,
  `filter_field`, `filter_op`, and `filter_value`.
- Record query supports indexed text, decimal, boolean, and datetime fields.
- Date/datetime fields now refresh `form_record_field_index.value_datetime`.
- Frontend forms API client now exposes schema summary, field usage, field
  dependencies, and query parameters for record list filtering/sorting.

Verified:

- `cargo check -p api`
- `cargo test -p api forms::`
- `cd frontend && npm run check`
- `cd frontend && npm run build`
- Rebuilt release API binary and replaced `/app/api` inside `openpr_api_1`.
- Health checks passed at `127.0.0.1:8081` and `10.72.0.3:8081`.
- Authenticated HTTP smoke passed for schema summary, field usage, field
  dependencies, amount filter/sort, and datetime filter/sort.
- Temporary smoke form was archived after verification.

Still pending before Phase 2 can be marked `已验收`:

- Frontend view builder must call these APIs in the actual UI.
- Human acceptance of API contract shape for form designer and record grid.

### 2026-06-02 Phase 4A Decimal Formula and Attachment Metadata

Implemented:

- Added `apps/api/src/forms/calculation.rs`.
- Added Decimal-safe formula v1 for `add`, `sum`, `subtract`, `multiply`,
  `divide`, `min`, `max`, and `count`.
- Formula config can live on a target field and writes calculated values before
  validation/normalization.
- Record create/update now run local formula evaluation after plugin formula
  hooks and before value normalization.
- Added `POST /api/v1/forms/{form_id}/records/recalculate-preview`.
- Added `POST /api/v1/form-records/{record_id}/recalculate`.
- Added migration `0037_form_attachments.sql`.
- Added `form_attachments` metadata table for attachment/image fields.
- Added `POST /api/v1/forms/{form_id}/attachments`.
- Frontend forms API client now exposes formula preview, record recalculation,
  and attachment metadata creation.

Verified:

- `cargo check -p api`
- `cargo test -p api forms::`
- `cd frontend && npm run check`
- `cd frontend && npm run build`
- Applied migration `0037_form_attachments.sql` to local `openpr_postgres_1`.
- Rebuilt release API binary and replaced `/app/api` inside `openpr_api_1`.
- Health checks passed at `127.0.0.1:8081` and `10.72.0.3:8081`.
- Authenticated HTTP smoke passed for formula preview, record create with
  calculated amount, record recalculation, and attachment metadata creation.
- Temporary smoke form was archived after verification.

Still pending before Phase 4A can be marked `已验收`:

- Frontend formula editor and attachment UI must call these APIs.
- Human acceptance of formula v1 contract.

Still pending for Phase 4B:

- Upload-to-attachment binding flow in UI.
- Import preview integration with formula calculation.

### 2026-06-02 Phase 4B Child Aggregates and Attachment Lifecycle

Implemented:

- Child aggregate formulas are now separated from ordinary formula evaluation.
  `child_sum`, `child_count`, `child_min`, and `child_max` are handled by the
  child aggregate pipeline instead of requiring `formula.args`.
- Parent records are recalculated when child records are created, updated,
  archived, or restored, and when `parent_child` record links are created.
- Registered attachment lifecycle routes:
  - `GET /api/v1/forms/{form_id}/attachments`
  - `DELETE /api/v1/form-attachments/{attachment_id}`
  - `POST /api/v1/form-attachments/{attachment_id}/restore`
- Frontend forms API client now exposes attachment list, archive, and restore
  methods.

Verified:

- `cargo check -p api`
- `cargo test -p api forms::`
- `cd frontend && npm run check`
- `cd frontend && npm run build`
- Rebuilt release API binary and replaced `/app/api` inside `openpr_api_1`.
- Health checks passed at `127.0.0.1:8081` and `10.72.0.3:8081`.
- Authenticated HTTP smoke passed for:
  - parent-child link child aggregate total `12.50`;
  - child update parent total `22.50`;
  - child archive parent total `2.50`;
  - child restore parent total `22.50`;
  - attachment create/list/archive/active-hidden/include-archived/restore.
- Temporary smoke forms were archived after verification.

Still pending before Phase 4B can be marked `已验收`:

- Human acceptance of child total behavior.
- Human acceptance of attachment metadata lifecycle behavior.
- Frontend upload/preview/remove UI remains part of Phase 5/6/9 work, not
  Phase 4B acceptance.

### 2026-06-02 Phase 5 Frontend Workflow Shell and Designer Split

Implemented:

- Added `FormWorkflowTabs.svelte` for `数据 / 详情 / 设计 / 自动化` modes.
- Added `FormDesignerWorkspace.svelte` with Huoban-style three-panel structure:
  field library, form canvas, and property panel.
- Added `FormAutomationPanel.svelte` to separate MCP, webhook, plugin, and
  connector context from the data grid.
- The existing forms page now separates:
  - data mode: record create/edit and record grid;
  - detail mode: selected record fields and record links;
  - design mode: field library/canvas/property editing;
  - automation mode: automation binding overview and print jobs.
- Existing form schema can be edited from the design mode and saved through
  `formsApi.update` with `expected_schema_version`.
- Field design supports add, select, edit label/key/type/required/options/amount
  metadata, duplicate, delete, and reorder.
- Design mode now loads field usage and field dependency data from REST when the
  designer opens and after schema save.
- The property panel now shows field safety status with stored value count,
  indexed value count, dependency count, and blocking dependency count.
- Field removal is blocked in the designer when the selected field has blocking
  dependencies.
- Designer schema save now keeps stable `field_id` values and shows a
  confirmation modal before deleting, renaming, or changing the type of existing
  fields.
- The schema-change confirmation lists each destructive change with stored
  value count, dependency count, and blocking dependency count before saving.
- The designer property panel now supports placeholder, help text, default
  value, list/detail visibility, read-only policy, and basic validation
  metadata.
- Record input now consumes placeholder, help text, default value, read-only
  policy, and basic validation attributes from the saved form schema.
- Added Chinese and English i18n for the new workflow modes and designer labels.

Verified:

- `cd frontend && npm run check`
- `cd frontend && npm run build`
- `cd frontend && npm run smoke:forms-ui`
- Copied the static build into `openpr_frontend_1:/usr/share/nginx/html`.
- Frontend returned HTML from `127.0.0.1:3000` and `10.72.0.3:3000`.
- Fixed the frontend Nginx `/api` proxy after return audit found it still held
  a stale API container IP after API redeploy.
- `10.72.0.3:3000/api/v1/auth/me` now proxies to the real API again.
- Deployed browser smoke opened the real custom-form project page and matched
  `数据 / 设计 / 自动化`.
- Browser smoke now covers workflow tabs, automation mode, design mode schema
  field-safety warning, blocking dependency delete disablement, rename
  confirmation, confirmed schema save, placeholder/help/default/validation
  metadata save, data-mode default/help rendering, record creation, detail-mode
  record links, and mobile horizontal-overflow regression.

Still pending after Phase 5 designer v1:

- Destructive schema changes now require confirmation, but backend-level schema
  migration operations for rename, type-change, and field archival are still
  pending.
- Dedicated route split is still pending; this step keeps modes inside the
  current page.

### 2026-06-02 Phase 5 Return Audit and Type-Specific Field Configuration

Implemented:

- Added type-specific designer property panels for formula-capable fields,
  relation fields, and attachment/image fields.
- Formula config can now save regular formulas such as `multiply(quantity,
  unit_price)` and child aggregate formulas such as `child_sum` with a
  `relation_key`.
- Relation config can now save target form key, relation type, relation key,
  and display field. `parent_child` is treated as the current schema-level
  expression for child/subform relationships and is now backed by Phase 7B
  inline child-table CRUD.
- Attachment/image config can now save accepted MIME types, max size MB,
  multiple-file behavior, preview behavior, and image thumbnail preference.
- Fixed a regression found during return audit: amount/number/integer fields no
  longer receive default empty formula metadata just because `formulaOp` has a
  default UI value.
- Browser smoke now asserts that relation, formula, and attachment metadata are
  persisted and that ordinary amount fields remain free of accidental formula
  metadata.

Verified:

- `cd frontend && npm run check`
- `cd frontend && npm run build`
- `cd frontend && npm run smoke:forms-ui`
- Copied the static build into `openpr_frontend_1:/usr/share/nginx/html`.
- Frontend returned HTML from `127.0.0.1:3000` and `10.72.0.3:3000`.
- `nginx -t` passed in `openpr_frontend_1` after the Podman DNS runtime
  resolver proxy fix.
- `10.72.0.3:3000/api/v1/auth/me` returned a real API response through the
  frontend proxy.
- Deployed browser smoke opened the real custom-form project page and matched
  `数据 / 设计 / 自动化`.

Phase 5 decision:

- Phase 5 designer v1 is now `已完成 / 已测试 / 待处理`.
- Acceptance remains pending because it still needs user review in the deployed
  browser.

Remaining omissions moved out of Phase 5:

- Child/subform inline table add/edit/delete belongs to Phase 7.
- Attachment upload/preview/remove record interaction belongs to Phase 6/9.
- Formula preview and recalculation buttons in the UI belong to Phase 6.
- Relation record picker belongs to Phase 7A and is now implemented/tested.
- Dedicated route split remains a product cleanup task after the tabbed workflow
  stabilizes.

### 2026-06-02 Phase 6 Record List/Detail CRUD Deployed Smoke

Implemented:

- Fixed detail-mode edit behavior: clicking `编辑` from record detail now
  switches back to `数据` mode and opens the edit form with the selected record
  values.
- Added `scripts/smoke-universal-forms-deployed-crud.mjs` as a real deployed
  browser CRUD smoke.
- The smoke logs in through the frontend `/api` proxy, creates a temporary
  universal form fixture, opens the deployed forms page, creates a record,
  opens readable detail, edits the record, deletes it, verifies no active
  records remain, and archives the temporary form.

Verified:

- `cd frontend && npm run check`
- `cd frontend && npm run build`
- Copied the static build and Nginx config into `openpr_frontend_1`.
- `nginx -t` passed inside `openpr_frontend_1`.
- `10.72.0.3:3000/api/v1/auth/me` returned a real API response through the
  frontend proxy.
- `node scripts/smoke-universal-forms-deployed-crud.mjs` passed and verified
  `create`, `detail`, `edit`, `delete`, and `frontend_api_proxy`.
- `cd frontend && npm run smoke:forms-ui` still passed.
- Cleanup check found `0` active `smoke_crud_*` forms.

Phase 6 decision:

- Phase 6 record list/detail CRUD v1 is now `已完成 / 已测试 / 待处理`.
- Acceptance remains pending because it still needs user review in the deployed
  browser.

Remaining omissions moved out of Phase 6:

- Dedicated record detail route/drawer remains pending after the current tabbed
  in-page detail v1.
- Saved views, filters, import/export, and permissions remain Phase 9 work.
- Inline child add/edit/delete moved into Phase 7B and is now implemented and
  browser-tested.
- Attachment upload/preview/remove remains Phase 9 work.

### 2026-06-02 Phase 7A Relation Picker and Child Record Display

Implemented:

- Added `GET /api/v1/forms/{form_id}/relation-targets`.
  - Supports relation field lookup through `field_key`.
  - Resolves target form from relation metadata `form_key`.
  - Returns paginated record targets with display text from `display_field`.
- Added `GET /api/v1/form-records/{record_id}/children`.
  - Reads `parent_child` links.
  - Returns child records through the same REST/auth path as other form APIs.
- Frontend forms API client now exposes `listRelationTargets` and
  `listRecordChildren`.
- Relation fields with `relation.form_key` now render as a record selector
  instead of a raw JSON/text input.
- Creating a record with a `parent_child` relation field now automatically
  writes a `form_record_links` row, so business users do not need to manually
  create the link after selecting the child record.
- Record detail now shows a child-record table for linked `parent_child`
  records.
- Relation display values now prefer `display` or `title` instead of rendering
  raw JSON.
- Added `scripts/smoke-universal-forms-deployed-relation-child.mjs` as a real
  deployed browser smoke for relation picker and child record display.

Verified:

- `cargo check -p api`
- `cargo test -p api forms::`
- Rebuilt release API binary, replaced `/app/api` inside `openpr_api_1`, and
  verified `/health`.
- HTTP smoke through `10.72.0.3:3000/api` verified:
  - `relation-targets` returns the child record display value;
  - `children` returns the linked child record.
- `cd frontend && npm run check`
- `cd frontend && npm run build`
- Copied the static build and Nginx config into `openpr_frontend_1`.
- `node scripts/smoke-universal-forms-deployed-relation-child.mjs` passed and
  verified relation picker, relation-targets API, automatic parent-child link,
  children API, and child table rendering.
- `cd frontend && npm run smoke:forms-ui` still passed after updating the mock
  API and test flow for relation selectors.
- Cleanup check found `0` active Phase 7 temporary forms.

Phase 7 decision:

- Phase 7A relation picker and child record display is implemented and tested.
- The overall Phase 7 row remained pending after Phase 7A because inline child
  CRUD and parent child-aggregate refresh still needed browser proof.

### 2026-06-02 Phase 7B Inline Child CRUD and Parent Refresh

Implemented:

- Parent record detail now exposes child-table actions for every relation field
  configured as `relation_type=parent_child`.
- Business users can add a child record inline from parent detail.
- Business users can edit an existing child record inline from parent detail.
- Business users can delete a child record from the child table.
- After parent-child link creation or inline child create/edit/delete, the
  frontend reloads the parent record so child aggregate formula values such as
  `child_sum` are visible immediately.
- Added localized Chinese and English labels for child record add/edit/delete
  and success toasts.
- Extended `scripts/smoke-universal-forms-deployed-relation-child.mjs` to cover
  inline child create, update, delete, and parent `child_sum` refresh against
  the real deployed frontend/API path.

Verified:

- `cd frontend && npm run check`
- `cd frontend && npm run build`
- Copied the static build and Nginx config into `openpr_frontend_1`.
- `nginx -t` passed inside `openpr_frontend_1`.
- `node scripts/smoke-universal-forms-deployed-relation-child.mjs` passed and
  verified relation picker, relation-targets API, automatic parent-child link,
  children API, child table rendering, inline child create, inline child
  update, inline child delete, and parent `child_sum` refresh.
- `cd frontend && npm run smoke:forms-ui` still passed.
- Cleanup check found `0` active Phase 7 temporary forms.

Phase 7 decision:

- Phase 7 relation picker, child table display, inline child CRUD, and parent
  child-aggregate refresh are now `已完成 / 已测试 / 待处理`.
- Acceptance remains pending because it still needs user review in the deployed
  browser.

Remaining Phase 7 work:

- Stronger child table layout controls driven by `detail_layout`.

### 2026-06-02 Phase 8 Template Library v1

Implemented:

- Added `POST /api/v1/projects/{project_id}/forms/from-template`.
  - Reads `scenario_templates.field_schema` for the selected template.
  - Normalizes legacy template `select` fields to universal-form
    `single_select`.
  - Fills missing schema `field_id` values.
  - Creates a default detail layout plus `grid` and `detail` views.
  - Emits a `form.created_from_template` event and writes a baseline schema
    version.
- Added frontend `formsApi.createFromTemplate`.
- Replaced the hard-coded restaurant sample action in the generic form create
  panel with a real universal form template library.
- The template library loads `custom_form` scenario templates through the
  existing scenario-template REST API.
- Operators can either apply a template to the form draft or directly install a
  template-backed form through the new backend endpoint.
- Added `scripts/smoke-universal-forms-deployed-template-library.mjs` to verify
  the deployed UI and API path.

Verified:

- `cargo check -p api`
- `cargo test -p api forms::`
- `cargo test -p api routes::form::tests`
- `cd frontend && npm run check`
- `cd frontend && npm run build`
- Rebuilt release API binary, replaced `/app/api` inside `openpr_api_1`,
  restarted the API container, and verified `/health`.
- Copied the static frontend build and Nginx config into `openpr_frontend_1`.
- `nginx -t` passed inside `openpr_frontend_1`.
- Direct deployed API smoke verified `from-template` creates a restaurant
  template form and normalizes `select` to `single_select`.
- `node scripts/smoke-universal-forms-deployed-template-library.mjs` passed and
  verified template library visibility, removal of the hard-coded restaurant
  sample entry, real `from-template` installation, default grid/detail views,
  and field type normalization.
- `cd frontend && npm run smoke:forms-ui` still passed.
- Cleanup check found `0` active Phase 7/8 temporary forms.

Phase 8 decision:

- Phase 8 template library v1 is now `已完成 / 已测试 / 待处理`.
- Acceptance remains pending because it still needs user review in the deployed
  browser.

Remaining Phase 8 work:

- First-class `form_templates` catalog table remains a future backend cleanup;
  v1 uses the existing `scenario_templates` catalog as the universal form
  template source.
- Installing a full multi-form scenario bundle into an existing project is not
  yet implemented; v1 installs a single editable form from template
  `field_schema`.

### 2026-06-02 Phase 9A Saved Views Builder

Implemented:

- The forms page now loads backend `form_views` for the selected form.
- Record grid columns are driven by the selected view's `config.columns`.
- Added a business-user saved-view builder in `数据` mode.
  - Select current view.
  - Create a new view.
  - Edit view name and type.
  - Choose visible grid columns.
  - Save view to backend `form_views`.
  - Delete/archive a saved view.
- Form switching, normal form creation, and template-based form creation now
  reset and reload the selected form's views.
- Added `scripts/smoke-universal-forms-deployed-saved-views.mjs` to verify the
  deployed browser workflow.

Verified:

- `cd frontend && npm run check`
- `cd frontend && npm run build`
- Copied the static frontend build and Nginx config into `openpr_frontend_1`.
- `nginx -t` passed inside `openpr_frontend_1`.
- `node scripts/smoke-universal-forms-deployed-saved-views.mjs` passed and
  verified saved-view builder visibility, view creation, grid columns following
  the saved view, reload persistence, and view deletion.
- `cd frontend && npm run smoke:forms-ui` still passed.
- Cleanup check found `0` active Phase 7/8/9A temporary forms.

Phase 9A decision:

- Saved views builder is now `已完成 / 已测试 / 待处理`.
- Phase 9 overall remains `待处理 / 待处理 / 待处理` until import and permission
  workflows are implemented and tested.

### 2026-06-02 Phase 9B Export Current View

Implemented:

- Added `GET /api/v1/forms/{form_id}/records/export`.
- Export accepts either a saved `view_id` or explicit `columns`.
- Export validates that the selected view belongs to the form and that all
  requested columns exist in the form schema.
- Export response returns a JSON contract with:
  - `format = csv`;
  - `file_name`;
  - selected column metadata;
  - row display values;
  - CSV text.
- CSV generation escapes commas, quotes, and newlines.
- Frontend `formsApi.exportRecords` now calls the export endpoint.
- The forms data view now has a business-user `导出当前视图` action next to
  saved-view controls.
- Browser download uses the selected saved view columns. If no saved view is
  selected, it exports the current visible grid columns.
- Added `scripts/smoke-universal-forms-deployed-export.mjs`.

Verified:

- `cargo fmt --all`
- `cargo check -p api`
- `cargo test -p api routes::form::tests`
- `cd frontend && npm run check`
- `cd frontend && npm run build`
- Release API binary was rebuilt and deployed into `openpr_api_1`.
- Health checks passed at `127.0.0.1:8081` and `10.72.0.3:8081`.
- Frontend static build and Nginx config were deployed into
  `openpr_frontend_1`.
- `nginx -t` passed inside `openpr_frontend_1`.
- Frontend returned HTML from `127.0.0.1:3000` and `10.72.0.3:3000`.
- `node scripts/smoke-universal-forms-deployed-export.mjs` passed and verified
  API export column selection, CSV escaping, hidden-column exclusion, browser
  export button, and browser CSV capture.
- `cd frontend && npm run smoke:forms-ui` still passed.
- Cleanup check found `0` active `export_*` temporary forms.

Phase 9B decision:

- Export current view v1 is now `已完成 / 已测试 / 待处理`.
- Phase 9 overall remains `待处理 / 待处理 / 待处理` until import workflows,
  permission governance, saved-view governance, and human acceptance are
  complete.

Remaining Phase 9 work:

- File upload, template-download, mapping wizard, long-running import job, and
  import-specific permission workflows beyond CSV/JSON paste v1.
- Export all records/template-download workflow beyond current-view CSV v1.
- Field-level and record-level permission policy/UI beyond role form action v1.
- Filters, sort, grouping, default view, and view ownership beyond the current
  column-selection v1.

### 2026-06-02 Phase 9C Import Preview And Commit

Implemented:

- Added `POST /api/v1/forms/{form_id}/records/import-preview`.
- Added `POST /api/v1/forms/{form_id}/records/import`.
- Import accepts up to 500 rows per request.
- Import preview validates source metadata, runs formula hooks, normalizes
  values, runs field validator hooks, renders the preview title, and returns
  row-level valid/error state without writing records.
- Import commit first runs the same preview gate. If any row is invalid, the
  endpoint returns the preview result with `created_count = 0` and writes no
  records.
- Valid import rows reuse `create_record_for_form`, so imported records follow
  the same normalization, validator, projection, form event, business event,
  plugin event hook, and parent child-aggregate recalculation path as normal UI
  record creation.
- Frontend `formsApi.previewImportRecords` and `formsApi.importRecords` now
  expose the import endpoints.
- The forms data view now has a business-user `导入记录` action.
- The import modal supports JSON array paste and CSV paste. CSV headers can use
  field keys or field labels, and `Title`/`标题` maps to record title.
- Numeric and amount cells are sent as strings to preserve Rust Decimal
  semantics and keep JSON numbers rejected by backend validation.
- Added `scripts/smoke-universal-forms-deployed-import.mjs`.

Verified:

- `cargo fmt --all`
- `cargo check -p api`
- `cargo test -p api routes::form::tests`
- `cd frontend && npm run check`
- `cd frontend && npm run build`
- `node --check scripts/smoke-universal-forms-deployed-import.mjs`
- Release API binary was rebuilt and deployed into `openpr_api_1`.
- Health checks passed at `127.0.0.1:8081` and `10.72.0.3:8081`.
- Frontend static build and Nginx config were deployed into
  `openpr_frontend_1`.
- `nginx -t` passed inside `openpr_frontend_1`.
- Frontend returned HTML from `127.0.0.1:3000` and `10.72.0.3:3000`.
- `node scripts/smoke-universal-forms-deployed-import.mjs` passed and verified
  API import preview rejection for amount JSON numbers, API import commit for a
  valid Decimal string row, browser import modal visibility, CSV preview, CSV
  commit, and rendered imported record.
- Cleanup check found `0` active `import_*` temporary forms.
- Delivery manifest now includes the deployed import smoke and verifies with
  `144` file rows.

Phase 9C decision:

- Import preview/commit paste v1 is now `已完成 / 已测试 / 待处理`.
- Phase 9 overall remains `待处理 / 待处理 / 待处理` until field/record-level
  permission governance, broader import/export governance, saved-view
  filters/sort/grouping/default/ownership, and human acceptance are complete.

### 2026-06-02 Phase 9D Role Permission Policy V1

Implemented:

- Added migration `0038_form_permissions.sql` with `form_permissions`.
- Added `GET /api/v1/forms/{form_id}/permissions`.
- Added `PATCH /api/v1/forms/{form_id}/permissions`.
- Permission action keys are `form.view`, `form.design`, `record.create`,
  `record.update`, `record.delete`, and `record.export`.
- Missing policies remain backward-compatible and allow existing behavior.
- `owner` and `admin` always retain all form actions.
- Only `owner` and `admin` can update form permission policy.
- Member role restrictions are enforced across form read, design, create,
  update, delete, export, import, relation, attachment, and recalculation paths.
- Project form lists now filter restricted member rows when `form.view` is
  false, including the paginated total count.
- The forms automation tab now includes a `权限` panel for member role actions.
- Added `scripts/smoke-universal-forms-deployed-permissions.mjs`.
- Delivery manifest now includes the deployed permissions smoke and verifies
  with `145` file rows.

Verified:

- `cargo fmt --all`
- `cargo check -p api`
- `cargo test -p api routes::form::tests`
- `cd frontend && npm run check`
- `cd frontend && npm run build`
- `node --check scripts/smoke-universal-forms-deployed-permissions.mjs`
- `scripts/audit-universal-forms-source-coverage.sh`
- Migration `0038_form_permissions.sql` applied to deployed Postgres.
- Release API binary was rebuilt and deployed into `openpr_api_1`.
- Health checks passed at `127.0.0.1:8081` and `10.72.0.3:8081`.
- Frontend static build and Nginx config were deployed into
  `openpr_frontend_1`.
- `nginx -t` passed inside `openpr_frontend_1`.
- Frontend returned HTML from `127.0.0.1:3000` and `10.72.0.3:3000`.
- `node scripts/smoke-universal-forms-deployed-permissions.mjs` passed and
  verified admin policy update, member effective denial for create/export/design,
  member create restore, and browser permissions panel save.
- Active `perms_*` temporary forms cleanup check returned `0`.

Phase 9D decision:

- Role permission policy v1 is now `已完成 / 已测试 / 待处理`.
- Phase 9 overall remains `待处理 / 待处理 / 待处理` until field-level and
  record-level policies, import file/mapping/job workflows, broader export
  workflows, saved-view filters/sort/grouping/default/ownership, and human
  acceptance are complete.

### 2026-06-02 Form Duplicate Workflow V1

Implemented:

- Added `POST /api/v1/forms/{form_id}/duplicate`.
- Duplicate copies form metadata, title template, schema, detail layout, and
  active views.
- Duplicate does not copy records or form permission policy rows.
- Duplicate auto-generates a project-unique key with `_copy`, then numeric
  suffixes when needed.
- Duplicate requires `form.design` on the source form.
- The forms UI now exposes `复制表单` from the selected form header.
- Added `scripts/smoke-universal-forms-deployed-duplicate.mjs`.

Verified:

- `cargo fmt --all`
- `cargo check -p api`
- `cargo test -p api routes::form::tests`
- `cd frontend && npm run check`
- `cd frontend && npm run build`
- `node --check scripts/smoke-universal-forms-deployed-duplicate.mjs`
- `scripts/audit-universal-forms-source-coverage.sh`
- Release API binary was rebuilt and deployed into `openpr_api_1`.
- Frontend static build and Nginx config were deployed into
  `openpr_frontend_1`.
- `node scripts/smoke-universal-forms-deployed-duplicate.mjs` passed and
  verified direct API duplicate, MCP duplicate, deployed browser duplicate
  button visibility, copied schema/title template/view, and no copied records.

Form duplicate decision:

- Form duplicate workflow v1 is now `已完成 / 已测试 / 待处理`.
- MCP now exposes `forms.duplicate`; remaining MCP form tools still lag the full
  REST/frontend Huoban surface.

### 2026-06-02 Phase 10A MCP Form Duplicate Parity

Implemented:

- Added MCP tool `forms.duplicate`.
- Added MCP API client call for `POST /api/v1/forms/{form_id}/duplicate`.
- Registered the tool in the MCP registry and dispatch path.
- Updated embedded MCP guide, MCP README, MCP AGENTS guide, OpenPR MCP skill,
  validation scripts, and regression scripts to the registry surface current
  at Phase 10A.
- Updated local MCP smoke and deployed duplicate smoke to verify MCP duplicate
  semantics.
- Repaired the local MCP runtime bot/workspace configuration for the deployed
  OpenPR workspace.

Verified:

- `cargo fmt --all --check`
- `cargo check -p mcp-server`
- `cargo test -p mcp-server project_type_and_resource_tools_are_registered_once`
- `cargo test -p mcp-server embedded_skill_guide_matches_registered_universal_tool_surface`
- `scripts/audit-universal-forms-source-coverage.sh`
- `MCP_URL=http://10.72.0.3:8090 scripts/test-mcp.sh`
- `node --check scripts/smoke-universal-forms-deployed-duplicate.mjs`
- `node scripts/smoke-universal-forms-deployed-duplicate.mjs`

Runtime verification:

- Live MCP `tools/list` returned the expected registry count for Phase 10A.
- Live MCP includes `forms.duplicate`.
- Deployed duplicate smoke confirms API duplicate and MCP duplicate copy
  schema, title template, and active views, while leaving records empty.
- Deployed frontend smoke confirms the selected form page loads and the
  `复制表单` button is visible/enabled. Browser-click automation is not used as
  duplicate semantic evidence; API and MCP calls provide that proof.
- Active temporary `duplicate_*` form cleanup returned `0`.

Phase 10A decision:

- MCP form duplicate parity is now `已完成 / 已测试 / 待处理`.
- Phase 10R remains `待处理 / 待处理 / 待处理` until schema versions,
  multi-form scenario bundle install parity, child create/update contracts,
  idempotency/retry receipts, webhooks, plugins, connectors, and real attachment
  binary workflows are aligned.

### 2026-06-02 Phase 10B MCP Form Metadata And Relation Read Parity

Implemented:

- Added MCP tools:
  - `forms.schema_summary`
  - `forms.field_usage`
  - `forms.field_dependencies`
  - `form_records.relation_targets`
  - `form_records.children`
- Added matching MCP API client calls to existing REST endpoints.
- Registered the tools in the MCP registry and dispatcher.
- Updated embedded MCP guide, MCP README, MCP AGENTS guide, root README, docs
  index, OpenPR MCP skill, validation scripts, and regression scripts to the
  then-current `88` tool surface. Phase 10C later moves the current surface to
  `94` tools.
- Extended `scripts/smoke-forms-mcp.sh` so MCP creates relation and child data,
  then verifies schema summary, field usage, field dependencies, relation
  target lookup, and child-record listing through MCP tools.

Verified:

- `cargo fmt --all`
- `cargo check -p mcp-server`
- `bash -n scripts/test-mcp.sh skills/openpr-mcp/scripts/validate-mcp.sh scripts/audit-universal-forms-source-coverage.sh scripts/audit-universal-forms-docs.sh scripts/smoke-forms-mcp.sh`
- `cargo fmt --all --check`
- `cargo check -p api -p mcp-server`
- `scripts/smoke-forms-mcp.sh`
- `scripts/smoke-universal-forms-api.sh`
- `scripts/smoke-plugins-mcp.sh`
- `scripts/audit-universal-forms-source-coverage.sh`
- `scripts/audit-universal-forms-production-readiness.sh`
- `scripts/prepare-universal-forms-delivery-manifest.sh`
- `scripts/report-universal-forms-delivery-manifest-json.sh`
- `scripts/verify-universal-forms-delivery-manifest.sh`
- `scripts/verify-universal-forms-delivery-manifest-json.sh`
- `scripts/smoke-universal-forms-delivery-manifest-json-contract.sh`
- `scripts/audit-universal-forms-docs.sh`

Regression omissions closed during re-audit:

- Registered `0038_form_permissions.sql` in API runtime migrations after MCP
  form smoke exposed `relation "form_permissions" does not exist` on a fresh
  database.
- Added source coverage to assert the `0038_form_permissions.sql` runtime
  migration registration, not only the migration file existence.
- Updated `scripts/smoke-forms-mcp.sh` to use the current `custom_form` project
  type, robust API/MCP data unwrapping, and an explicit active view fixture
  before asserting duplicate-view copy behavior.
- Replaced stale `customer_delivery` project-type usage in API/plugin/frontend
  smoke fixtures with `custom_form`; `customer_delivery_default` remains only as
  a scenario template key.
- Updated production readiness audit to the then-current `88` MCP tool registry
  and current Podman/Nginx service-discovery model. Phase 10C later updates the
  current registry count to `94`.
- Refreshed the delivery manifest and manifest JSON after the changed audit and
  MCP scripts.

Phase 10B decision:

- MCP form metadata and relation read parity is now
  `已完成 / 已测试 / 待处理`.
- Phase 10R remains `待处理 / 待处理 / 待处理` for import/export, permission
  policy, attachments, schema versions, template install parity, idempotency
  receipts, webhook/plugin/connector delivery controls, and child create/update
  ergonomics beyond generic record create/update/link tools. Phase 10C later
  closes MCP role permission policy parity and attachment metadata lifecycle
  parity, but not binary upload/download/preview workflows.

### 2026-06-02 Phase 10C MCP Permission And Attachment Metadata Parity

Implemented:

- Added MCP tools:
  - `form_permissions.get`
  - `form_permissions.update`
  - `form_attachments.list`
  - `form_attachments.create`
  - `form_attachments.archive`
  - `form_attachments.restore`
- Added matching MCP API client calls to existing REST endpoints:
  - `GET /api/v1/forms/{form_id}/permissions`
  - `PATCH /api/v1/forms/{form_id}/permissions`
  - `GET /api/v1/forms/{form_id}/attachments`
  - `POST /api/v1/forms/{form_id}/attachments`
  - `DELETE /api/v1/form-attachments/{attachment_id}`
  - `POST /api/v1/form-attachments/{attachment_id}/restore`
- Registered the tools in the MCP registry and dispatcher.
- Updated embedded MCP guide, MCP README, MCP AGENTS guide, root README, docs
  index, OpenPR MCP skill, validation scripts, regression scripts, source
  coverage audit, production readiness audit, and MCP integration test to the
  current `94` tool surface.
- Extended `scripts/smoke-forms-mcp.sh` so MCP reads and updates form role
  permission policies, then creates, lists, archives, lists archived, and
  restores attachment metadata through MCP tools.

Verified:

- `bash -n scripts/test-mcp.sh skills/openpr-mcp/scripts/validate-mcp.sh scripts/audit-universal-forms-source-coverage.sh scripts/audit-universal-forms-docs.sh scripts/audit-universal-forms-production-readiness.sh scripts/smoke-forms-mcp.sh`
- `cargo fmt --all --check`
- `cargo check -p mcp-server`
- `cargo test -p mcp-server project_type_and_resource_tools_are_registered_once`
- `cargo test -p mcp-server embedded_skill_guide_matches_registered_universal_tool_surface`
- `scripts/smoke-forms-mcp.sh`
- `scripts/audit-universal-forms-source-coverage.sh`
- `scripts/audit-universal-forms-production-readiness.sh`

Phase 10C decision:

- MCP form role permission policy parity and attachment metadata lifecycle parity
  are now `已完成 / 已测试 / 待处理`.
- Phase 10R remains `待处理 / 待处理 / 待处理` for import/export MCP parity,
  schema version MCP parity, template install parity, idempotency/retry
  receipts, webhook/plugin/connector delivery controls, child create/update
  ergonomics beyond generic record create/update/link tools, and real attachment
  binary upload/download/preview/export flows beyond metadata. Phase 10D later
  closes current-view export, import preview/commit, and single-form
  create-from-template MCP parity, but not multi-form scenario bundle install.

### 2026-06-02 Phase 10D MCP Template And Import/Export Parity

Implemented:

- Added MCP tools:
  - `forms.create_from_template`
  - `form_records.export`
  - `form_records.import_preview`
  - `form_records.import_commit`
- Added matching MCP API client calls to existing REST endpoints:
  - `POST /api/v1/projects/{project_id}/forms/from-template`
  - `GET /api/v1/forms/{form_id}/records/export`
  - `POST /api/v1/forms/{form_id}/records/import-preview`
  - `POST /api/v1/forms/{form_id}/records/import`
- Registered the tools in the MCP registry and dispatcher.
- Updated embedded MCP guide, MCP README, MCP AGENTS guide, root README, docs
  index, OpenPR MCP skill, validation scripts, regression scripts, source
  coverage audit, production readiness audit, and MCP integration test to the
  current `98` tool surface.
- Extended `scripts/smoke-forms-mcp.sh` so MCP creates a form from a scenario
  template, exports form records with amount values, previews import rows, then
  commits valid import rows through the backend record create pipeline.

Verified:

- `cargo fmt --all`
- `cargo fmt --all --check`
- `cargo check -p api -p mcp-server`
- `cargo test -p api mcp_tool_registry_enables_tools_from_project_capabilities`
- `cargo test -p mcp-server project_type_and_resource_tools_are_registered_once`
- `cargo test -p mcp-server embedded_skill_guide_matches_registered_universal_tool_surface`
- `bash -n scripts/test-mcp.sh skills/openpr-mcp/scripts/validate-mcp.sh scripts/audit-universal-forms-source-coverage.sh scripts/audit-universal-forms-docs.sh scripts/audit-universal-forms-production-readiness.sh scripts/smoke-forms-mcp.sh`
- `scripts/smoke-forms-mcp.sh`
- `scripts/audit-universal-forms-source-coverage.sh`
- `scripts/audit-universal-forms-production-readiness.sh`

Regression omission closed during Phase 10D:

- Project-aware MCP `tools/call` originally rejected
  `forms.create_from_template` because the API `agent_policy.tool_registry`
  forms capability group still listed the older form tool set. The registry now
  includes Phase 10A-D form tools, and source coverage asserts the API policy
  includes template, permission, attachment, import/export, and relation tools.

Phase 10D decision:

- MCP single-form template creation plus current export/import-preview/import
  commit parity is now `已完成 / 已测试 / 待处理`.
- Phase 10R remains `待处理 / 待处理 / 待处理` for multi-form scenario bundle
  install parity, idempotency/retry receipts,
  webhook/plugin/connector delivery controls, child create/update ergonomics
  beyond generic record create/update/link tools, and real attachment binary
  upload/download/preview/export flows beyond metadata.

## 2026-06-02 Phase 10E MCP Schema Version History Parity

Regression audit finding:

- `form_schema_versions` existed and was written on form create/update, but the
  API/MCP surface could not list or fetch archived schema versions. That meant
  AI, CLI, webhooks, plugins, and connectors could not inspect design history
  through the same hub used for other universal-form operations.

Implemented:

- Added `GET /api/v1/forms/{form_id}/schema-versions`.
- Added `GET /api/v1/forms/{form_id}/schema-versions/{version}`.
- Added MCP tools:
  - `form_schema_versions.list`
  - `form_schema_versions.get`
- Added the tools to the MCP registry, dispatcher, embedded skill guide,
  project-aware agent policy registry, skill validation scripts, regression
  scripts, README/AGENTS documentation, source coverage audit, and production
  readiness audit.

Validated:

- `scripts/smoke-forms-mcp.sh` now verifies:
  - baseline schema version `1` is listed and fetched after form creation;
  - `forms.update_schema` increments the schema to version `2`;
  - version `2` can be listed and fetched with the updated field set.

Phase 10E decision:

- MCP schema version history parity is now
  `已完成 / 已测试 / 待处理`.
- Phase 10R remains `待处理 / 待处理 / 待处理` for multi-form scenario bundle
  install parity, idempotency/retry receipts, webhook/plugin/connector delivery
  controls, child create/update ergonomics beyond generic record
  create/update/link tools, and real attachment binary upload/download/preview
  export flows beyond metadata. Phase 10F later closes the MCP child
  create/update ergonomics slice.

## 2026-06-02 Phase 10F MCP Child Record Write Ergonomics

Regression audit finding:

- Frontend Phase 7 already supports inline child create/update/delete from the
  parent detail page, but MCP automation still had to manually call
  `form_records.create` and then `form_records.link`. That preserved the data
  model, but left child-table write behavior as orchestration knowledge instead
  of a first-class automation contract.

Implemented:

- Added `POST /api/v1/form-records/{record_id}/children`.
  - Resolves `child_form_id` or `child_form_key`.
  - Validates the parent form relation field has
    `relation_type=parent_child` and targets the child form.
  - Creates the child record, creates the `parent_child` link, emits a child
    creation event, and recalculates parent child aggregates.
- Added `PATCH /api/v1/form-records/{record_id}/children/{child_record_id}`.
  - Verifies the child record is linked to the parent through a
    `parent_child` link before updating.
  - Reuses the same schema validation, formula, field validator, event, and
    parent aggregate refresh path as normal record updates.
- Added MCP tools:
  - `form_records.child_create`
  - `form_records.child_update`
- Added the tools to the MCP registry, dispatcher, API client, project-aware
  agent policy registry, skill validation scripts, MCP regression scripts,
  README/AGENTS documentation, source coverage audit, docs audit, and
  production readiness audit.

Validated:

- `scripts/smoke-forms-mcp.sh` now verifies:
  - parent form schema can include a `lines` relation with
    `relation_type=parent_child`;
  - MCP `form_records.child_create` creates a child row and parent-child link;
  - MCP `form_records.child_update` updates the linked child row;
  - MCP `form_records.children` returns the updated child row.

Phase 10F decision:

- MCP child record create/update ergonomics are now
  `已完成 / 已测试 / 待处理`.
- Phase 10R remains `待处理 / 待处理 / 待处理` for multi-form scenario bundle
  install parity, idempotency/retry receipts, webhook/plugin/connector delivery
  controls, real attachment binary upload/download/preview/export flows beyond
  metadata, and child delete ergonomics beyond the existing generic record
  archive/restore API. Phase 10G later closes the explicit MCP child archive
  and restore surface.

## 2026-06-02 Phase 10G MCP Child Record Lifecycle Ergonomics

Finding:

- Phase 10F made child create/update explicit for MCP automation, but deletion
  still required callers to know the child record id and fall back to generic
  record archive/restore. That bypassed the parent-child relation validation
  that makes subform automation understandable to non-engineering operators.

Implementation:

- Added REST child lifecycle routes:
  - `DELETE /api/v1/form-records/{record_id}/children/{child_record_id}`
  - `POST /api/v1/form-records/{record_id}/children/{child_record_id}/restore`
- Both routes verify the parent record, child record, optional `relation_key`,
  parent `record.update` permission, child `record.delete` permission, and the
  existing `parent_child` link before archiving or restoring the child row.
- Refactored generic record archive/restore into shared backend helpers so
  child lifecycle and normal record lifecycle keep the same event and aggregate
  recalculation behavior.
- Added MCP tools:
  - `form_records.child_archive`
  - `form_records.child_restore`
- Updated the MCP registry, embedded guide, MCP docs, agent policy, validation
  scripts, and forms MCP smoke to the current `104` tool surface.

Validation:

- `scripts/smoke-forms-mcp.sh` now creates a child row, updates it, archives it,
  verifies it is hidden from active `form_records.children`, restores it, and
  verifies it appears again.
- Source coverage, production readiness, and docs/protocol audits assert the
  child lifecycle API/MCP/docs/script surface.

Phase 10G decision:

- MCP child record lifecycle ergonomics are now
  `已完成 / 已测试 / 待处理`.
- Phase 10R remains `待处理 / 待处理 / 待处理` for idempotency/retry receipts,
  webhook/plugin/connector delivery controls, and real attachment binary
  upload/download/preview/export flows beyond metadata.

## 2026-06-02 Phase 10H MCP Scenario Bundle Install Parity

Finding:

- Project creation already used the full scenario-template application path, but
  existing projects only had `forms.create_from_template`, which installs one
  editable form from a template. AI/MCP automation therefore could not take an
  existing `custom_form` project and install a complete multi-form business
  bundle such as contract review or restaurant ordering.

Implemented:

- Added `POST /api/v1/projects/{project_id}/scenario-templates/{template_key}/install`.
- The endpoint loads the existing project, authorizes user or MCP bot workspace
  access, merges scenario metadata into `type_settings`, backfills a template
  workflow when the project has none, emits
  `project.scenario_template.installed`, and then reuses
  `apply_scenario_template`.
- The shared template application path is now repeatable for existing projects:
  existing form keys reuse the real existing `form_id`, baseline schema/view
  inserts are conflict-safe, and duplicate connector suggestions are skipped.
- MCP now exposes `scenario_templates.install`; the API agent policy enables it
  for forms-capable projects.
- `scripts/smoke-forms-mcp.sh` installs `contract_review_default` into a
  temporary `custom_form` project through MCP and verifies `contract`,
  `risk_clause`, and `approval_record` exist through `forms.list`.

Validation:

- `cargo check -p api -p mcp-server`
- `cargo fmt --all --check`
- `cargo test -p api mcp_tool_registry_enables_tools_from_project_capabilities`
- `cargo test -p mcp-server project_type_and_resource_tools_are_registered_once`
- `cargo test -p mcp-server embedded_skill_guide_matches_registered_universal_tool_surface`
- `cargo test -p mcp-server scenario_template_tools`
- `scripts/smoke-forms-mcp.sh`

Phase 10H decision:

- MCP scenario bundle install parity is now
  `已完成 / 已测试 / 待处理`.
- Phase 10R remains `待处理 / 待处理 / 待处理` for idempotency/retry receipts,
  webhook/plugin/connector delivery controls, and real attachment binary
  upload/download/preview/export flows beyond metadata.

## 2026-06-02 Phase 10I API/MCP/Import Form Write Idempotency Receipts v1

Finding:

- The event model had `business_events.idempotency_key` and connector receipt
  rows, but universal form record writes did not accept a write key at the API
  or MCP boundary. A retried MCP/import write could therefore create duplicate
  form records before external automation could reason about delivery state.

Implemented:

- Added optional `idempotency_key` to form record create and update request
  bodies.
- Added optional `idempotency_key` to import commit requests. A batch key
  derives row keys as `<batch-key>:row:<row_number>` unless a row supplies its
  own key.
- Added optional query `idempotency_key` support for record archive and restore
  routes.
- Added a shared idempotency receipt lookup that checks
  `business_events(workspace_id, idempotency_key)` before mutating. Matching
  retries return the original record; mismatched event type or form usage
  returns conflict.
- Added `insert_form_event_with_idempotency` so `form_events`,
  `business_events`, and `event_outbox` keep the same receipt key on record
  create/update/archive/restore events.
- MCP `form_records.create`, `form_records.update`, and
  `form_records.import_commit` now accept and forward `idempotency_key`.
- `scripts/smoke-forms-mcp.sh` now retries a create and import commit with the
  same keys, verifies the original record ids are returned, verifies duplicate
  retry values are not exported, and verifies `events.tail` exposes the create
  receipt key.
- `scripts/audit-universal-forms-source-coverage.sh` now asserts the API,
  MCP, and smoke coverage for form write idempotency receipts.

Validation:

- `cargo fmt --all --check`
- `cargo check -p api -p mcp-server`
- `scripts/smoke-forms-mcp.sh`

Phase 10I decision:

- API/MCP/import form record write idempotency receipts v1 is now
  `已完成 / 已测试 / 待处理`.
- Phase 10R is closed as `已完成 / 已测试 / 待处理` by the current
  MCP/webhook/plugin/connector source coverage, forms MCP idempotency smoke,
  webhook/connector receipt smokes, production automation smoke, and
  production object-storage/attachment lifecycle smokes. Final external
  automation acceptance still belongs to the user-side manual signoff queue.

## 2026-06-08 Phase 9R/10R Closure

Closure decision:

- Phase 9R is now `已完成 / 已测试 / 待处理`.
- Phase 10R is now `已完成 / 已测试 / 待处理`.
- No phase is `已验收` until the seven user-side manual signoff rows are
  accepted and the finalizer plus strict release gates pass.

Evidence used for Phase 9R:

- Field-level read/write policy and record-level `record_scope` enforcement are
  implemented in `apps/api/src/forms/permissions.rs` and applied by
  `apps/api/src/routes/form.rs`.
- Saved-view ownership/default/filter/sort/group behavior is represented by
  `form_views.config`, private-view ownership checks, default-view clearing,
  server-side view filtering/sorting, and the frontend view-builder workflow.
- Broader import/export governance is represented by view-scoped export,
  durable export/import jobs, file import preview/commit, reusable import
  mapping templates, and current-view export APIs.
- Verification: `scripts/audit-universal-forms-source-coverage.sh`,
  `bun run check`, `bun run build`, and `bun run smoke:forms-ui`.

Evidence used for Phase 10R:

- MCP form writes expose idempotency keys; forms MCP smoke verifies create and
  import retry behavior and receipt visibility.
- Webhook/connector receipts are idempotent through `event_inbox`, connector
  receipt APIs, worker inbox/outbox processing, and replay controls.
- Plugin install/update/invoke and automatic hook paths emit business events and
  participate in the shared event/connector architecture.
- Attachment binary acceptance is covered by production object-storage,
  attachment lifecycle, signature lifecycle, and package export smokes, while
  MCP still exposes attachment metadata operations.
- Verification: `scripts/audit-universal-forms-source-coverage.sh`,
  `scripts/smoke-forms-mcp.sh`, `scripts/smoke-webhook-generic-consumer.sh`,
  `scripts/smoke-wasm-plugin-runtime.sh`, `scripts/smoke-plugins-mcp.sh`, and
  the production automation/object-storage smoke family.

## Recommended Next Implementation Order

1. Run user-side manual acceptance and record all seven signoff rows.
2. Refresh the delivery bundle and manifest after any source/report drift.
3. Add durable record detail route or drawer driven by `detail_layout`.
4. Add stronger child-table layout controls driven by `detail_layout`.
5. Split the large frontend forms route into reusable designer, list, detail,
   import/export, permission, and automation components without changing
   behavior.
6. Add keyboard and accessibility coverage for designer/list/detail/import
   workflows.
7. Run deployed browser acceptance again and collect fresh screenshots.
10. Complete the seven user-side manual signoff rows, then run finalizer,
    strict delivery-state audit, and delivery-bundle audit.
