# Universal Forms Regression Audit - 2026-06-02

## Scope

This audit returns to the Huoban-style universal forms plan and checks whether
the current OpenPR implementation has missed delivery-critical work across:

- backend API and database contracts;
- frontend form designer, record list, and detail workflows;
- MCP, webhook, plugin, and connector alignment;
- validation, testing, deployment, and human acceptance markers.

The audit intentionally treats code, API registration, frontend reachability,
automated tests, browser smoke, and human acceptance as separate states.

## Current Verification

Repository state:

- Branch: `main`.
- Working tree: dirty, with universal forms code, migrations, scripts, and docs
  still uncommitted.

Checks run during this audit:

- `cargo check -p api`: passed after fixing the Phase 4B blockers.
- `cargo test -p api forms::`: passed with 12 tests.
- `cd frontend && npm run check`: passed with 0 errors and 0 warnings.
- `cd frontend && npm run build`: passed.
- `cd frontend && npm run smoke:forms-ui`: passed after extending coverage for
  workflow tabs, designer safety, destructive save confirmation, formula,
  relation, attachment metadata, record create/detail, links, and mobile
  overflow.
- `node scripts/smoke-universal-forms-deployed-crud.mjs`: passed against the
  real deployed frontend/API path and verified record create, readable detail,
  edit, delete, and frontend `/api` proxy.
- `node scripts/smoke-universal-forms-deployed-relation-child.mjs`: passed
  against the real deployed frontend/API path and verified relation picker,
  relation-targets API, automatic parent-child link, children API, and child
  table rendering.
- The same deployed relation/child smoke was extended and rerun after Phase 7B;
  it now also verifies inline child create, inline child update, inline child
  delete, and parent `child_sum` refresh from the parent detail page.
- `cargo test -p api routes::form::tests`: passed with 2 template helper tests.
- `node scripts/smoke-universal-forms-deployed-template-library.mjs`: passed
  against the real deployed frontend/API path and verified template library
  visibility, removal of the hard-coded restaurant sample action,
  `from-template` installation, default grid/detail views, and template field
  type normalization.
- `node scripts/smoke-universal-forms-deployed-saved-views.mjs`: passed against
  the real deployed frontend/API path and verified saved-view builder
  visibility, saved-view create, grid columns following the saved view,
  reload persistence, and saved-view delete.
- Release API binary was rebuilt and deployed into `openpr_api_1`.
- Health checks passed at `127.0.0.1:8081` and `10.72.0.3:8081`.
- Authenticated HTTP smoke passed for child aggregate link/update/archive/restore
  and attachment create/list/archive/include_archived/restore.
- Frontend static build was copied into `openpr_frontend_1`.
- Frontend returned HTML from `127.0.0.1:3000` and `10.72.0.3:3000`.
- Frontend Nginx `/api` proxy returned a real API response from
  `10.72.0.3:3000/api/v1/auth/me` after the proxy fix.
- `nginx -t` passed inside `openpr_frontend_1`; `/mcp` proxy returned 404
  rather than 502, confirming the proxy target is reachable even though `/mcp`
  itself is not a root health endpoint.
- Deployed browser smoke passed against the real frontend/API path for the
  custom-form project page and matched `数据 / 设计 / 自动化`.
- Active `smoke_crud_*` cleanup check returned `0` forms after the deployed
  CRUD smoke.
- Active Phase 7 temporary parent/child forms cleanup check returned `0` forms.
- Active Phase 7/8 temporary forms cleanup check returned `0` forms after the
  deployed template library smoke.
- Active Phase 7/8/9A temporary forms cleanup check returned `0` forms after
  the deployed saved-view smoke.

Second-pass return-audit checks:

- `scripts/report-universal-forms-development-status-json.sh --output /tmp/openpr-development-status.json`:
  passed and reported `9/10` rows tested-or-accepted, `0` accepted rows, `1`
  pending row, `7` manual signoff rows pending, and `final_release_allowed=false`.
- `scripts/report-universal-forms-readiness-json.sh --output /tmp/openpr-readiness.json`:
  passed and reported `27` automated checks, `0` failed automated checks, and
  `7` manual signoff rows pending.
- `scripts/audit-universal-forms-source-coverage.sh`: failed with `2` source
  coverage issues, then passed after aligning the audit and API smoke event
  checks to archive semantics.
- `scripts/audit-universal-forms-docs.sh`: failed because the delivery manifest
  no longer matched the current changed files, then passed after refreshing the
  delivery manifest.
- `scripts/verify-universal-forms-delivery-manifest.sh`: passed with `145` file
  rows after that second-pass manifest refresh. This was later superseded by
  the 2026-06-02 re-audit refresh below, which verifies `146` file row(s).
- `scripts/report-universal-forms-delivery-manifest-json.sh` and
  `scripts/verify-universal-forms-delivery-manifest-json.sh`: passed after
  refreshing the machine-readable manifest to the same then-current `145` file
  rows; the current manifest count is `146`.
- `scripts/smoke-universal-forms-delivery-manifest-json-contract.sh`: passed.
- `node scripts/smoke-universal-forms-deployed-export.mjs`: passed against the
  real deployed frontend/API path and verified current-view export through API
  and browser download flow.
- `node scripts/smoke-universal-forms-deployed-import.mjs`: passed against the
  real deployed frontend/API path and verified API import preview rejection,
  API import commit, browser CSV preview, browser CSV commit, and rendered
  imported record.
- `node scripts/smoke-universal-forms-deployed-permissions.mjs`: passed against
  the real deployed frontend/API path and verified admin policy update, member
  effective denial for create/export/design, member create restore, and browser
  permissions panel save.
- `cd frontend && npm run smoke:forms-ui`: passed after Phase 9B export.
- Active `export_*` temporary forms cleanup check returned `0`.
- Active `import_*` temporary forms cleanup check returned `0`.

Corrections made during this audit:

- Fixed `create_form_record` child-aggregate parent recalculation to use
  `created_by` instead of an undefined `updated_by`.
- Restored the API default bind address to `0.0.0.0:8081` for local deployment
  access.
- Registered attachment lifecycle routes:
  - `GET /api/v1/forms/{form_id}/attachments`
  - `DELETE /api/v1/form-attachments/{attachment_id}`
  - `POST /api/v1/form-attachments/{attachment_id}/restore`
- Added frontend API client methods:
  - `formsApi.listAttachments`
  - `formsApi.deleteAttachment`
  - `formsApi.restoreAttachment`
- Fixed child aggregate formulas so ordinary formula evaluation skips `child_*`
  operations.
- Fixed record update so child updates recalculate parent records.
- Fixed frontend schema generation so ordinary amount/number/integer fields do
  not receive accidental empty formula metadata from designer defaults.
- Fixed frontend Nginx proxy DNS drift: API/MCP upstreams now use Podman DNS
  runtime resolution instead of keeping a stale API container IP after API
  restart/redeploy.
- Fixed record detail edit behavior so the detail-mode `编辑` button switches
  back to `数据` mode before showing the edit form.
- Added relation target and child-record REST APIs, relation selector UI,
  automatic `parent_child` link creation for relation fields, and child-record
  detail table rendering.

Still not covered by this audit:

- human acceptance.
- durable record detail route/drawer acceptance. Current record detail is still
  an in-page mode.
- stronger child table layout controls driven by `detail_layout`.

## Material Omissions Found

### Return Audit Addendum: Omissions Still Present

The second-pass return audit found additional omissions that were not explicit
enough in the first regression write-up.

Second-pass hard failures and repairs:

- `scripts/audit-universal-forms-source-coverage.sh` fails because
  `apps/api/src/routes/form.rs` emits `form.archived` and
  `form.view.archived`, while the source-coverage gate still requires
  `form.deleted` and `form.view.deleted`.
- This is not just a naming nit. The plan now uses archive/restore semantics,
  but the event contract and audit gate still expect delete-style events. The
  selected stable contract is archive semantics. The source-coverage gate and
  API smoke now check `form.archived` and `form.view.archived` instead of
  delete-style events.
- Verification: `scripts/audit-universal-forms-source-coverage.sh` passed after
  this correction.
- `scripts/audit-universal-forms-docs.sh` fails through delivery manifest
  verification. Current file checksums/sizes drifted for:
  - development status JSON;
  - compose stack;
  - frontend Dockerfile;
  - frontend Nginx config;
  - env example;
  - verify script.
- This means the handoff bundle is stale after recent code/config edits. It is
  not acceptable to call the delivery package current until the manifest is
  refreshed and reverified.
- The manifest was refreshed with
  `scripts/prepare-universal-forms-delivery-manifest.sh`.
- Verification: `scripts/verify-universal-forms-delivery-manifest.sh` passed
  with `145` file rows during that pass; the later re-audit refresh below now
  verifies `146` file row(s). `scripts/report-universal-forms-delivery-manifest-json.sh`
  refreshed the machine-readable manifest; `scripts/verify-universal-forms-delivery-manifest-json.sh`,
  `scripts/smoke-universal-forms-delivery-manifest-json-contract.sh`, and
  `scripts/audit-universal-forms-docs.sh` passed.

Status-model omission:

- The repository-level development status still reports broad delivery rows as
  `已测试`, while the Huoban alignment plan correctly keeps Phase 9 overall and
  Phase 10 as `待处理`.
- This difference is now confirmed by generated JSON: automated gates can be
  green while user-side manual acceptance remains `7/7` pending and final
  release remains blocked.
- The next status model should explicitly distinguish:
  - old scenario delivery bundle readiness;
  - Huoban-style universal form product workflow readiness;
  - deployed smoke evidence;
  - human acceptance.

Functional omissions still open:

- Form record import preview/import paste v1 exists and is deployed-tested.
  File upload, template-download, mapping wizard, import job persistence, and
  import-specific permissions remain pending.
- Form record current-view export endpoint exists; broader export workflows
  remain pending.
- Form duplicate workflow v1 exists and is deployed-tested; it copies schema,
  title template, detail layout, and active views, but not records.
- Form role action permission v1 exists and is deployed-tested; record-level and
  field-level permission policies remain pending.
- Project form list visibility is now filtered by `form.view`, including the
  paginated total count, and the deployed permission smoke covers hidden and
  restored list visibility.
- No view optimistic locking.
- No first-class `form_templates` catalog table; Phase 8 still uses scenario
  templates as the backing source.
- No end-to-end form write idempotency for UI/import/MCP/webhook/plugin writes,
  even though business events and connector receipts have idempotency support.
- MCP form tools remain behind REST and frontend for idempotency receipts and
  real attachment binary upload/download/preview/export flows. Phase 10C closes
  role permission policy and attachment metadata lifecycle MCP parity; later
  10D-10H closes template/import/export/schema/child/scenario-install parity.
- The automation tab remains an overview panel, not a binding editor with
  delivery status, retry controls, or per-form MCP/webhook/plugin/connector
  subscriptions.
- Attachment/image fields still have metadata lifecycle, but not upload,
  preview, remove, download, thumbnail, and export workflows.
- The frontend is still centered on one large forms route. The business
  workflows exist as tabs/components, but durable routes for form detail,
  designer, record detail, settings, and automation are not split yet.

Decision from this addendum:

- The original regression audit did catch the major product gaps, but it missed
  two machine-verifiable regressions: event contract drift and delivery-manifest
  drift.
- Event contract drift is now repaired and source-coverage verified.
- Delivery-manifest drift is now repaired and verified.
- Permission work and broader import/export governance remain the next Phase 9
  implementation scope after the hard audit gates are stable.

### 0. Return Audit Addendum After Phase 5 Shell

Phase 5 now has a first workflow shell and designer split, but it is not yet a
completed Huoban-style form product.

Rechecked state:

- runtime `project_types` contains only `code_project` and `custom_form`;
- the frontend project create modal prefers only those two project types;
- the forms page now exposes `数据 / 详情 / 设计 / 自动化` modes;
- the designer has a field library, canvas, property panel, duplicate, delete,
  reorder, and schema save through optimistic locking;
- the designer now loads field usage/dependencies and shows field safety counts
  before field removal;
- blocking field dependencies now disable field removal in the designer;
- deleting, renaming, or changing the type of an existing field now opens a
  save-confirmation modal with value/dependency impact counts;
- designer schema save preserves stable `field_id` values so rename detection is
  based on field identity instead of only field key text;
- the designer can now edit placeholder, help text, default value, list/detail
  visibility, read-only policy, and basic validation metadata;
- the designer can now edit formula metadata for regular formulas and child
  aggregate formulas;
- the designer can now edit relation metadata, including `parent_child`
  relation type for the current child/subform schema expression;
- the designer can now edit attachment/image metadata for accepted types, max
  size, multi-file behavior, preview, and image thumbnail preference;
- record input now renders placeholder/help/default/read-only/basic validation
  behavior from saved schema metadata;
- deployed frontend is reachable at `0.0.0.0:3000` through the container port
  mapping.
- deployed frontend `/api` proxy now reaches the current API container instead
  of a stale Podman IP.
- deployed browser CRUD smoke now verifies create, detail, edit, delete, and
  cleanup against the real frontend/API path.
- relation picker and child record display now have deployed browser smoke
  coverage through temporary parent/child form fixtures.

Remaining omissions after this improvement:

- destructive field changes are guarded in the frontend, but backend-level
  schema migration operations for rename, type-change, and field archival are
  not yet first-class APIs;
- formula/relation/attachment metadata editors exist, and relation picker,
  child record display, inline child CRUD, and parent child-sum refresh now
  exist, but formula preview, upload/preview/remove, and image thumbnail
  workflows are not complete;
- record detail works as an in-page mode, but is not yet a durable detail
  route/drawer driven by `detail_layout`;
- automation mode is currently an overview panel, not a binding editor for MCP,
  webhooks, plugins, connectors, retries, and delivery receipts;
- the hard-coded restaurant sample button has been replaced by the Phase 8
  universal form template library, and full multi-form scenario bundle install
  into an existing project is now available through API and MCP;
- i18n still contains legacy scenario project-type labels for compatibility,
  which can confuse future UI surfaces if project-type data drifts;
- MCP now exposes duplicate, schema summary, field usage, field dependencies,
  relation target lookup, child record listing/write/lifecycle, attachment
  metadata lifecycle, templates, import/export, permission policy management,
  and scenario bundle install; idempotency receipts and real attachment binary
  workflows remain open.
- deployed browser CRUD now has a controlled temporary form fixture; deployed
  designer mutation against real data still needs equivalent fixture coverage.

Decision:

- Phase 5 designer v1 can now be treated as `已完成 / 已测试 / 待处理`.
- Phase 5 must not be marked `已验收` until the user accepts the deployed
  browser workflow.
- Phase 6 record list/detail CRUD v1 can now be treated as
  `已完成 / 已测试 / 待处理`.
- Phase 6 must not be marked `已验收` until the user accepts the deployed CRUD
  workflow.
- Phase 8 now replaces hard-coded scenario sample actions with a real universal
  form template library, but must not be marked `已验收` until the user accepts
  the deployed browser workflow.
- Phase 10 must include MCP parity for every REST contract that a human can use
  from the form workflow.

### 1. Phase Status Drift

The generated development-status artifact reports many rows as `已测试`, but
the Huoban alignment plan still correctly marks phases 5 through 10 as
`待处理`.

This is a real governance mismatch:

- the older delivery-status tracker is scenario/delivery-bundle oriented;
- the Huoban plan is product-workflow oriented;
- current frontend still does not provide the Huoban-style designer, dedicated
  detail workflow, saved view builder, import/export, permission UI, or
  automation bindings UI.

Required fix:

- Update status/report scripts or tracker rows so they distinguish old
  restaurant/scenario delivery evidence from the new Huoban product-workflow
  phases.

### 2. Phase 4B Initially Was Not Actually Tested

Before this audit, Phase 4B had code-level gaps:

- API compile failed due to an undefined `updated_by`.
- Attachment lifecycle handlers existed but were not registered in `main.rs`.
- Frontend API client only exposed attachment creation, not list/archive/restore.
- Child aggregate formulas were incorrectly evaluated by the ordinary formula
  engine and failed without `formula.args`.
- Child record update did not trigger parent aggregate recalculation.

These gaps are now fixed and verified against the deployed service.

Verified behavior:

- parent total after two child links: `12.50`;
- parent total after child update: `22.50`;
- parent total after child archive: `2.50`;
- parent total after child restore: `22.50`;
- attachment metadata can be created, listed, archived, hidden from active list,
  returned by `include_archived=true`, and restored.

Remaining Phase 4B acceptance work:

- human acceptance of child total behavior;
- human acceptance of attachment metadata lifecycle behavior.

### 3. Backend API Scope Still Missing

The plan lists several APIs that remain unimplemented or incomplete:

- broader export workflows beyond current-view CSV v1;
- file-upload import, template download, mapping wizard, and import job
  persistence beyond CSV/JSON paste v1;

Additional backend gaps:

- no write idempotency contract for import, MCP, webhook, plugin, or connector
  writes;
- no view optimistic locking;
- no field-level schema operation API for add/edit/delete/reorder;
- no dedicated `form_templates` catalog table yet; Phase 8 v1 uses
  `scenario_templates` plus `POST /projects/{project_id}/forms/from-template`;
- no record-level or field-level permission policy model beyond role action v1.

### 4. MCP Surface Is Behind The REST Plan

Current MCP form tools cover basic list/get/create/update schema, form
duplicate, schema summary, field usage, field dependencies, records, links,
relation target lookup, child record listing, aggregates, and events. They do
not yet expose the full Huoban alignment surface:

- archive/restore form and record;
- attachments list/create/archive/restore;
- child records create/update as explicit child-table operations;
- templates list/install parity for the Phase 8 `from-template` workflow;
- schema versions list/get;
- import and export beyond current-view CSV v1.

Required fix:

- Extend `apps/mcp-server/src/tools/forms.rs` and the client layer after REST
  contracts are stable.
- Keep MCP as an API client, not a direct database integration.

### 5. Frontend Product Workflow Still Has Gaps

The current forms UI now has a Huoban-style workflow shell and designer v1, but
it is still a tabbed single-page implementation rather than a fully separated
business application surface.

Implemented frontend workflow coverage:

- dedicated `数据 / 详情 / 设计 / 自动化` modes;
- three-panel designer with field library, canvas, and property panel;
- field duplicate/delete/reorder with dependency warnings and destructive save
  confirmation;
- formula, relation, and attachment/image metadata editors;
- placeholder, help text, default value, list/detail visibility, read-only, and
  basic validation metadata editors;
- relation picker, child table rendering, inline child create/edit/delete, and
  parent `child_sum` refresh from the detail page;
- universal form template library with apply-to-draft and direct install
  actions;
- form duplicate action from the selected form header;
- saved-view builder with backend create/update/delete and grid columns driven
  by `form_views.config.columns`;
- browser smoke for the production static build with mocked API responses.

Still missing frontend workflows:

- saved view filters, sort, grouping, default-view selection, and ownership;
- dedicated record detail route/drawer driven by `detail_layout`;
- stronger child-table layout controls driven by `detail_layout`;
- attachment upload, preview, remove, and image thumbnails;
- file-upload import, mapping wizard, and broader export workflow beyond
  current-view CSV v1;
- permission-gated designer entry;
- keyboard and accessibility coverage;

This is the core remaining reason the product can still feel mixed: forms,
record links, restaurant sample actions, print jobs, and automation overview are
separated by tabs, but they are not yet split into durable routes and dedicated
task surfaces.

### 6. Database And Data Lifecycle Gaps

Implemented database work is a good foundation, but production business usage
still needs:

- `form_templates` or equivalent first-class template catalog;
- field-level and record-level permission policy tables or equivalent policy
  contract beyond `form_permissions` role action v1;
- import job and import preview persistence if imports can be long-running;
- export job/status metadata if exports are asynchronous;
- idempotency records for automation writes;
- schema migration operations for rename/hide/archive/change type;
- stronger attachment storage/download policy, not only metadata rows.

### 7. Decimal And Formula Scope Needs More Validation

Formula v1 exists and uses decimal-safe Rust handling, but business math is not
fully accepted yet.

Remaining risks:

- child aggregate formulas have passed deployed browser smoke for parent-detail
  inline child create/update/delete refresh;
- rounding, scale, currency, and negative-value rules need end-to-end tests;
- import preview now uses the same formula hook and normalization path before
  commit, but currency/rounding edge cases still need more business math
  acceptance;
- MCP/webhook/plugin writes must use the same formula path and reject duplicate
  idempotency keys;
- restaurant line total should be provable as `quantity * unit_price`, and order
  total should be provable as child sum.

### 8. Automation Alignment Is Not Yet A User Workflow

The backend has business events, webhooks, connectors, and plugins, but the form
user cannot yet configure or inspect automation from a form automation tab.

Missing:

- form-level automation bindings UI;
- failed delivery visibility and retry controls;
- stable shared event envelope exposed to MCP/webhook/plugin/connector
  consumers;
- idempotent event consumers for printing and other connectors;
- openprx-webhooks shown as one consumer path rather than a separate hidden
  integration.

### 9. Human Acceptance Remains Pending

No phase should be marked `已验收` yet. The existing artifacts still show manual
acceptance pending, and the Huoban-specific flows have not been browser-tested
or user-signed.

Required acceptance rows:

- designer acceptance;
- record CRUD/detail acceptance;
- relation and child table acceptance;
- amount/formula acceptance;
- attachment acceptance;
- automation binding acceptance;
- overall business-user acceptance.

## Immediate Repair Plan

1. Resolve the event contract drift.
   Completed in this return pass by selecting archive semantics for universal
   form lifecycle events and updating the source-coverage/API smoke checks to
   `form.archived` and `form.view.archived`.

2. Refresh and verify the delivery manifest.
   Completed in this return pass with
   `scripts/prepare-universal-forms-delivery-manifest.sh`,
   `scripts/verify-universal-forms-delivery-manifest.sh`, and
   `scripts/audit-universal-forms-docs.sh`.

3. Keep Phase 4B verification evidence stable.
   Phase 4B backend/API verification is complete for this audit. Preserve the
   Rust tests, frontend build, deployed health checks, and authenticated HTTP
   smoke evidence while waiting for human acceptance.

4. Normalize status artifacts.
   Align development-status scripts/tracker rows with the Huoban phases so old
   scenario evidence cannot mark new product workflows as `已测试`.

5. Continue Phase 9 permission and broader import/export governance work.
   Phase 9A saved views, Phase 9B current-view export, Phase 9C import
   preview/commit paste v1, and Phase 9D role permission policy v1 are only
   partial Phase 9 results. Field-level policies, record-level policies,
   file-upload import, mapping wizard, import jobs, and broader export policy
   remain the next implementation scope after the hard audit failures are
   cleared.

6. Continue frontend decomposition.
   Split the current mixed forms page into shell, grid, record editor/detail,
   designer, field library, canvas, and property panel components.

7. Implement relation and child table contracts before polishing templates.
   This is the data-model heart of universal forms and is required before
   restaurant/contract/equipment templates feel real.

8. Extend MCP only after REST contracts are stable.
   MCP should mirror the same API capabilities and permission checks, not grow a
   separate behavior surface.

## Audit Decision

There were omissions. The plan covers most of them conceptually, but current
implementation and status artifacts were out of sync.

Current truth:

- Backend foundations for phases 2, 3, and 4A are mostly implemented and tested
  by prior smoke evidence.
- Phase 4B backend/API work is implemented and tested by authenticated deployed
  HTTP smoke, but still needs human acceptance.
- Phase 5 designer v1 is implemented and browser-tested, but still needs human
  acceptance.
- Phase 6 record list/detail CRUD v1 is implemented and tested by deployed
  browser CRUD smoke, but still needs human acceptance.
- Phase 7 relation picker, child record display, inline child create/update/delete,
  and parent child-sum refresh are implemented and tested by deployed browser
  smoke, but still need human acceptance.
- Phase 8 template library v1 is implemented and tested by deployed browser
  smoke, but still needs human acceptance.
- Form duplicate workflow v1 is implemented and tested by deployed browser/API
  smoke, and MCP duplicate parity is implemented/tested, but still needs human
  acceptance.
- Phase 9A saved views builder, Phase 9B current-view export, Phase 9C
  import preview/commit paste v1, and Phase 9D role permission policy v1 are
  implemented and tested by deployed browser smoke, but Phase 9 as a whole
  still needs field-level and record-level permission governance, broader
  import/export governance, saved-view filters/sort/grouping/default/ownership,
  and human acceptance.
- The remaining Phase 9 scope and Phase 10 remain product-workflow work, not
  accepted delivery.
- Phase 10A MCP form duplicate parity, Phase 10B MCP form metadata/relation
  read parity, and Phase 10C MCP role permission plus attachment metadata parity
  are implemented/tested, while Phase 10R remains pending.
- No phase is currently `已验收`.

## 2026-06-02 Re-Audit Addendum

This re-audit found no new deployed outage and no failed automated delivery
gate, but it did find status and scope items that were easy to misread as
missing or complete.

Confirmed deployed state:

- API, MCP, and frontend health checks are reachable on the local LAN/WireGuard
  deployment surface.
- The delivery manifest now records `146` files and includes the deployed form
  duplicate smoke.
- Automated delivery/signoff gates report `27` automated checks and `0` failed
  automated checks.
- At that re-audit point, live MCP exposed `88` tools and included `forms.duplicate`,
  `forms.schema_summary`, `forms.field_usage`, `forms.field_dependencies`,
  `form_records.relation_targets`, and `form_records.children`.
- After Phase 10C, the MCP registry moved to `94` tools and also included
  `form_permissions.get`, `form_permissions.update`,
  `form_attachments.list`, `form_attachments.create`,
  `form_attachments.archive`, and `form_attachments.restore`.
- After Phase 10D, the MCP registry was `98` tools and also included
  `forms.create_from_template`, `form_records.export`,
  `form_records.import_preview`, and `form_records.import_commit`.
- After Phase 10E, the MCP registry was `100` tools and also included
  `form_schema_versions.list` and `form_schema_versions.get`.
- After Phase 10F, the MCP registry was `102` tools and also included
  `form_records.child_create` and `form_records.child_update`.
- After Phase 10G, the MCP registry was `104` tools and also included
  `form_records.child_archive` and `form_records.child_restore`.
- After Phase 10H, the current MCP registry is `105` tools and also includes
  `scenario_templates.install`.
- Active temporary `duplicate_*` forms cleanup returned `0`.

Omissions or unclear status found:

- The Huoban alignment phase table still collapsed Phase 9 into one
  `待处理 / 待处理 / 待处理` row even though Phase 9A saved views, Phase 9B
  current-view export, Phase 9C paste import, Phase 9D role permission policy,
  and form duplicate v1 have all been implemented and deployed-tested. The plan
  now splits these into explicit completed/tested rows plus a remaining Phase
  9R governance row.
- MCP form parity is still behind REST/frontend. It now exposes form duplicate,
  schema summary, field usage/dependencies, relation target lookup, child
  record listing, role permission policy management, attachment metadata
  lifecycle, single-form template creation, current export, import preview, and
  import commit, schema version history, explicit child
  create/update/archive/restore tools, and multi-form scenario bundle install,
  but still does not expose idempotency receipts or real attachment binary
  workflows.
- The signoff JSON has a naming trap: `manual_signoff.pending_rows = 7` is the
  real current user acceptance queue, while `release_requirement.pending_rows =
  0` is a schema-pinned release precondition constant. Human acceptance must be
  read from `manual_signoff`.
- Frontend business workflow is tested, but still too concentrated in one
  forms page. Dedicated record detail route/drawer, stronger detail-layout
  rendering, attachment media UI, mapping wizard, and saved-view
  filter/sort/group/default/ownership workflows remain product gaps.
- Deployed duplicate smoke now treats the browser path as visibility evidence
  for the `复制表单` button and uses API/MCP calls for duplicate semantics.
  Browser-click automation should not be counted as semantic duplicate
  acceptance until a stable browser interaction proof is added.
- The event hub exists, but write idempotency for import, MCP, webhook, plugin,
  and connector writes remains missing. This is the most important consistency
  gap before treating OpenPR as a generalized enterprise workflow hub.

Re-audit decision:

- No deployed outage was found, but the re-audit did find fresh-database and
  automation blockers that are now closed.
- There was a documentation/status omission around Phase 9 progress, now
  corrected in the Huoban alignment plan.
- There was a stale MCP status omission around `forms.duplicate`; Phase 10A is
  now separated as completed/tested MCP duplicate parity. Phase 10B is also
  separated as completed/tested MCP form metadata and relation read parity.
- The API runtime migration list omitted `0038_form_permissions.sql`; fresh
  MCP form smoke failed on `relation "form_permissions" does not exist`. The
  migration is now registered and source coverage asserts the registration.
- Several smoke fixtures still used `customer_delivery` as a project type after
  project types were reduced to `code_project` and `custom_form`. Backend and
  frontend smoke fixtures now use `custom_form`; `customer_delivery_default`
  remains only as a scenario template key.
- Production readiness audit still expected old `82` MCP tool counts and old
  Docker/localhost Nginx assertions. It was updated to check the then-current
  `88` tool registry and the current Podman DNS plus upstream-alias proxy model.
  Phase 10C later moves the current registry count to `94`.
- Delivery manifest and manifest JSON were refreshed after these changes; the
  manifest again verifies `146` file row(s).
- Phase 10C adds MCP role permission policy and attachment metadata lifecycle
  parity; local MCP smoke verifies read/update permission policy plus
  create/list/archive/list-archived/restore attachment metadata.
- Phase 10D adds MCP single-form template creation and import/export parity;
  local MCP smoke verifies template form creation, record export, import
  preview, import commit, and decimal-safe imported amount normalization.
- Phase 10D also closed a project-aware MCP policy omission: the API
  `agent_policy.tool_registry` forms capability group still listed the older
  form tool set, causing `tools/call` to reject `forms.create_from_template`.
  The policy now includes Phase 10A-D form tools and source coverage asserts
  template, permission, attachment, import/export, and relation tools.
- This regression audit found a real Phase 10 omission: schema version rows
  existed and were written, but no API/MCP list/get surface exposed them to AI,
  CLI, webhook, plugin, or connector flows. Phase 10E now adds
  `form_schema_versions.list` and `form_schema_versions.get`, updates the
  project-aware agent policy registry, moved the registry to `100`
  tools, and extends the forms MCP smoke to verify baseline version `1`, schema
  update version `2`, and exact version fetch.
- Phase 10F closes another concrete MCP ergonomics gap: frontend inline child
  create/update already existed, while MCP automation still had to orchestrate
  generic record creation plus manual link creation. The API now exposes
  child-record create/update routes that validate the parent-child relation and
  the MCP registry now includes `form_records.child_create` and
  `form_records.child_update`; forms MCP smoke verifies both tools and
  `form_records.children` reads back the updated child row.
- Phase 10G closes the child lifecycle half of the same gap: the API now exposes
  child archive/restore routes that verify the parent-child link before
  lifecycle mutation, and MCP now exposes `form_records.child_archive` and
  `form_records.child_restore`; forms MCP smoke verifies archive hides the child
  from active children and restore makes it visible again.
- Phase 10H closes scenario bundle install parity: the API now installs a full
  scenario template into an existing project, MCP exposes
  `scenario_templates.install`, and forms MCP smoke verifies a contract review
  bundle creates `contract`, `risk_clause`, and `approval_record` forms in a
  temporary `custom_form` project.
- As of the 2026-06-02 re-audit, remaining work was still centered on Phase 9R
  governance, Phase 10R API/MCP/webhook/plugin/connector parity, and the `7/7`
  manual acceptance queue.

## 2026-06-08 Closure Addendum

This closure pass rechecked the current source tree and moved Phase 9R and
Phase 10R out of the implementation backlog. They are now code-complete and
automated-tested, but still not user-accepted.

Closed Phase 9R scope:

- Field-level read/write permission policy and record-level `record_scope`
  governance are implemented in `apps/api/src/forms/permissions.rs` and enforced
  by `apps/api/src/routes/form.rs`.
- Saved-view filtering, sorting, grouping/default behavior, and private/shared
  ownership are backed by `form_views.config`, server-side record query logic,
  private-view ownership checks, and frontend view-builder controls.
- Broader import/export governance is backed by current-view export, import
  preview/commit, file import, reusable import mapping templates, and durable
  import/export jobs.

Closed Phase 10R scope:

- MCP form write tools expose retry-safe idempotency keys; forms MCP smoke
  verifies record create/import retry behavior and event receipt visibility.
- Webhook and connector delivery share `business_events`, `event_outbox`,
  `event_inbox`, connector receipt APIs, replay controls, and worker processing.
- Plugin install/update/invoke plus automatic form hooks emit business events and
  remain part of the same event/connector architecture.
- Attachment binary production flows are covered by object-storage, attachment
  lifecycle, signature lifecycle, and package export smokes; MCP currently
  exposes attachment metadata operations.

Verification from this pass:

- `scripts/audit-universal-forms-source-coverage.sh` passed.
- `cd frontend && bun run check` passed with `0` errors and `0` warnings.
- `cd frontend && bun run build` passed.
- `cd frontend && bun run smoke:forms-ui` passed.
- `cd frontend && bun run smoke:restaurant-ordering` failed at the current
  mock browser entrypoint while waiting for the forms page. Treat that as a
  frontend smoke fixture drift to fix before using restaurant browser smoke as
  current acceptance evidence.

Current remaining boundary:

- No phase is `已验收`.
- The only product-delivery blocker is still the `7/7` user-side manual
  acceptance queue plus strict release/delivery gates after signoff.
