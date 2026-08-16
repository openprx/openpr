# Universal Forms Acceptance Guide

This guide points operators and reviewers to the acceptance process for the universal forms, WASM plugin, MCP, webhook, connector, and restaurant ordering work.

The authoritative execution tracker is outside the repository:

```text
report/openpr/docs/openpr-universal-form-development-execution-tracker-2026-05-31.md
```

The manual user acceptance runbook is:

```text
report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md
```

## Scope

Acceptance covers the generic business-platform path:

```text
scenario template
  -> universal forms and views
  -> sandboxed WASM plugin
  -> record creation and editing
  -> business events and outbox/inbox
  -> connector delivery and receipt
  -> MCP reads/writes/aggregate
  -> frontend grid/detail workflow
```

The restaurant ordering template is the reference scenario. It should prove that OpenPR can be used for non-code business workflows such as menu, SKU, table, order, order line, print job, and daily report management.
The full built-in scenario catalog is in `docs/scenario-templates.md`; it lists
all six templates, their generated forms, and their API/MCP/frontend usage
paths.

## Automated Evidence

Run these before manual acceptance:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo machete
cargo audit
scripts/audit-universal-forms-security-scope.sh
cargo test --workspace --all-features
cargo build --workspace --release
scripts/audit-universal-forms-source-coverage.sh
scripts/audit-universal-forms-docs.sh
scripts/smoke-scenario-template-forms.sh
scripts/smoke-forms-mcp.sh
bun --cwd frontend run smoke:restaurant-ordering
cd frontend && bun run check && bun run build
cd frontend && bun run smoke:project-template && bun run smoke:template-work-items
cd frontend && bun run smoke:forms-ui && bun run smoke:restaurant-ordering
```

For a single generated evidence report:

```bash
scripts/acceptance-universal-forms.sh --quick
scripts/acceptance-universal-forms.sh --full
scripts/audit-universal-forms-source-coverage.sh
scripts/audit-universal-forms-security-scope.sh
scripts/audit-universal-forms-production-readiness.sh
scripts/audit-universal-forms-delivery-state.sh
scripts/report-universal-forms-completion-audit.sh
scripts/prepare-universal-forms-manual-evidence-map.sh
scripts/report-universal-forms-signoff-status.sh \
  --output /opt/worker/report/openpr/docs/openpr-universal-form-signoff-status-2026-05-31.md
scripts/prepare-universal-forms-user-acceptance-packet.sh
scripts/report-universal-forms-readiness-summary.sh
scripts/report-universal-forms-readiness-json.sh
scripts/verify-universal-forms-readiness-json.sh
scripts/prepare-universal-forms-delivery-manifest.sh
scripts/verify-universal-forms-delivery-manifest.sh
scripts/collect-universal-forms-ui-artifacts.sh
scripts/verify-universal-forms-ui-artifacts.sh
scripts/verify-universal-forms-ui-review-gallery.sh
scripts/smoke-universal-forms-ui-review-gallery-render.sh
scripts/smoke-universal-forms-report-output-boundaries.sh
scripts/audit-universal-forms-delivery-bundle.sh
scripts/refresh-universal-forms-delivery-bundle.sh --full
```

`--quick` runs the source coverage audit, production readiness audit, and focused acceptance smoke set. `--full` also runs CI-grade Rust and frontend gates. Both modes generate a Markdown report with command output and a manual signoff table. When regenerating an existing formal evidence report, the acceptance script preserves existing manual signoff rows instead of resetting reviewer decisions to `Pending`.
The docs/protocol audit treats the manual signoff pending count as dynamic, so partially signed reviewer handoffs remain valid while the remaining rows are still pending.

The source coverage audit is a fast structural gate. It verifies that the tracked delivery surface still has concrete migrations, API routes, worker outbox handling, MCP tools, frontend Forms route/API modules, and smoke scripts before heavier behavior tests run.

The security scope audit is the fast proof for the PostgreSQL-only security
boundary. It keeps `cargo audit` green with the repository policy while proving
the SQLx MySQL advisory ignore is not an active runtime dependency path.

The production readiness audit is also structural. It checks the production runbook, docker-compose service shape, health checks, prebuilt runtime image, frontend nginx API/MCP proxies, and finalization gates.

The delivery-state audit does not rerun heavy checks. It verifies that the tracker, generated evidence report, manual signoff state, and finalization scripts agree.

The completion audit report summarizes the generated evidence and tracker into one delivery status: automated checks, non-manual unresolved tracker rows, end-to-end acceptance state, and remaining manual signoff rows. It is the fastest way to see whether the project is ready for user-side acceptance or finalization.

The user acceptance packet is the reviewer-facing handoff. It links the runbook, generated evidence, completion audit, pending signoff rows, and finalization commands in one Markdown file.

The readiness summary is the shortest release-readiness view. It reports the current stage, every tracker module status, automated check index counts, manual pending rows, delivery manifest verification, and the next required action without replacing the longer audit reports.

The readiness JSON report is the machine-readable readiness view for CI,
release automation, MCP tools, webhook consumers, and deployment scripts. It
uses schema `openpr.universal_forms.readiness.v1` and reports the current
stage, automated gates, tracker statuses, pending manual rows, next signoff
item, and final release requirements without changing acceptance state.
The `manual_signoff.next_row` object also includes the suggested evidence note
and recorder command template from the signoff status report, so automation can
surface the next reviewer action without scraping Markdown.
The `reports.signoff_status_json` field points at the dedicated machine-readable
signoff progress report, allowing MCP, CLI, webhook, and CI consumers to jump
from the overall readiness state to exact completed/pending reviewer rows.
The JSON report includes `schema_path`, which points to
`docs/schemas/openpr-universal-forms-readiness.schema.json` for consumers that
need a stable contract file.
The readiness JSON, signoff status JSON, release gate JSON, and delivery status JSON all constrain next manual keys to the same seven manual signoff keys, or an empty string after final signoff.
The readiness JSON verifier checks that this JSON still matches the
authoritative evidence, completion audit, readiness summary, signoff status
report, signoff status JSON, and tracker before automation consumes it. It also
checks schema-level required fields, top-level additional-property drift,
nested additional-property drift, grouped required fields, the seven allowed
manual signoff keys, typed counters and booleans, status enums, and release
requirement constants.

The development status JSON is the machine-readable development matrix for CI,
release automation, MCP tools, webhook consumers, and deployment scripts. It
mirrors the tracker row for requirements/protocol, database/model, backend API,
MCP, Webhook/Connector, WASM plugin, frontend UI, scenario template, delivery
bundle, and user-side manual acceptance. The report mirrors each row's
engineering requirement and completion rule, keeps `已完成`, `已测试`,
`已验收`, and `待处理` status markers programmatically visible, and includes a
`final_release_allowed` flag that remains false until manual signoff and
finalization are complete. Its schema pins the ten development rows in tracker
order, and the verifier rejects extra fields inside `status_summary` and row
objects:

```bash
scripts/report-universal-forms-development-status-json.sh
scripts/verify-universal-forms-development-status-json.sh
scripts/smoke-universal-forms-development-status-json-contract.sh
```

Its schema is
`docs/schemas/openpr-universal-forms-development-status.schema.json`.

The scenario catalog JSON is the machine-readable scenario catalog for CI,
release automation, MCP tools, webhook consumers, deployment scripts, and
future template marketplaces. It mirrors `docs/scenario-templates.md`, listing
each built-in template key, project type, generated forms, integration paths,
operator entrypoints, operator steps, primary MCP tools, connector kinds,
plugin keys, and acceptance focus points. The verifier checks Markdown parity,
smoke coverage for all six templates, the pinned catalog order, nested
operator-entrypoint and template object shape, and the restaurant reference
plugin/form/print requirements:

```bash
scripts/report-universal-forms-scenario-catalog-json.sh
scripts/verify-universal-forms-scenario-catalog-json.sh
scripts/smoke-universal-forms-scenario-catalog-json-contract.sh
```

Its schema is
`docs/schemas/openpr-universal-forms-scenario-catalog.schema.json`.

The runtime API and MCP response for every scenario template also includes
`usage_guide` with the same operator entrypoints, operator steps, primary MCP
tools, connector kinds, plugin keys, and acceptance focus fields. The scenario
template smoke verifies that `restaurant_ordering_default` exposes
`projects.create`, `form_records.aggregate`, `print`, and `restaurant_calc`
through both list and detail API responses before the manual reviewer uses the
project template wizard.

The implementation map JSON is the machine-readable implementation map for CI,
release automation, MCP tools, webhook consumers, and deployment scripts. It
mirrors `docs/universal-forms-implementation-map.md`, listing each delivery
area, implementation path, public surface, primary verification command, status
marker, and strict release boundary. Its schema pins status marker and module
order, and the verifier rejects extra fields inside marker, module, and
release-boundary objects:

```bash
scripts/report-universal-forms-implementation-map-json.sh
scripts/verify-universal-forms-implementation-map-json.sh
scripts/smoke-universal-forms-implementation-map-json-contract.sh
```

Its schema is
`docs/schemas/openpr-universal-forms-implementation-map.schema.json`.

The manual evidence map links each of the seven signoff rows to the automated checks, screenshots, reviewer commands, and suggested evidence note that support that row. It is a pre-signoff aid only; it does not mark acceptance as passed.

The signoff status report is the fastest reviewer prompt for the next manual row. It reads the formal evidence and manual evidence map, prints the current row statuses, shows the next actionable item key, and emits the recorder command template with the suggested evidence note. It is read-only:

```bash
scripts/report-universal-forms-signoff-status.sh --reviewer "<name>"
scripts/report-universal-forms-signoff-status.sh \
  --output /opt/worker/report/openpr/docs/openpr-universal-form-signoff-status-2026-05-31.md
scripts/report-universal-forms-signoff-status.sh \
  /opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md \
  --reviewer "<name>"
```

The generated signoff status file is part of the delivery bundle. It gives reviewers a persistent completed/pending row snapshot instead of only transient terminal output.

The next signoff review report is a one-page reviewer aid generated from the
verified signoff status JSON. It links the current manual row, row-specific
review scope, evidence files, screenshots, verification commands, and recorder
command without changing signoff state:

```bash
scripts/prepare-universal-forms-next-signoff-review.sh
scripts/prepare-universal-forms-next-signoff-review.sh \
  --output /opt/worker/report/openpr/docs/openpr-universal-form-next-signoff-review-2026-05-31.md
```

The report output boundary smoke verifies that generated handoff reports are
safe for automation and shell redirection. It runs the Markdown, HTML, and JSON
generators with `--output` paths in a temporary directory, requires stdout to
stay empty, checks Markdown/HTML first lines, checks JSON schema versions, and
verifies `scripts/report-universal-forms-signoff-status-json.sh --output -`
and the equivalent `--stdout` alias emit valid JSON directly on stdout without leaving a repository-root `-` output file for pipe-based MCP, CLI, webhook, CI,
and release automation consumers:

```bash
scripts/smoke-universal-forms-report-output-boundaries.sh
```

The manual signoff recorder updates one accepted, failed, rework, or pending row in both the runbook and the formal evidence report, then runs the consistency verifier on the candidate files. For the default handoff paths, it also refreshes the completion audit, completion audit JSON, manual evidence map, UI review gallery, signoff status report, signoff status JSON, user acceptance packet, readiness summary, readiness JSON, development status JSON, scenario catalog JSON, implementation map JSON, report output boundary smoke, delivery manifest, delivery manifest JSON, and their focused verifiers/contract smokes so pending counts and checksums stay synchronized. Use it after the reviewer has completed the corresponding runbook step:

```bash
scripts/record-universal-forms-manual-signoff.sh --list-items
scripts/record-universal-forms-manual-signoff.sh \
  --item restaurant_template \
  --status accepted \
  --reviewer "<name>" \
  --evidence "<review note>"
```

Record `overall` only after the first six concrete rows have been accepted; the recorder rejects early overall acceptance. If `overall` is already accepted, reopen the `overall` row before downgrading any prerequisite row; the recorder rejects prerequisite downgrade while overall acceptance is passed.

The manual signoff status JSON is the machine-readable mirror of the reviewer-facing signoff report. It uses schema `openpr.universal_forms.signoff_status.v1`, includes `schema_path`, gate summary values, completed/pending row counts, row-level recorder command templates, the next actionable row, and a recorder command template. Each manual row and `next_row` also include the automated evidence and reviewer check text from the manual evidence map, so CI, MCP tools, webhook consumers, and release automation can show reviewers what to inspect without scraping Markdown. The `pending_queue` array exposes every remaining manual row in prerequisite order with `review_order`, `is_next`, `actionable`, evidence text, reviewer check text, and the recorder command, so a web page, MCP tool, webhook consumer, or release bot can render the whole review queue without reconstructing it from Markdown. Its `next_row.key` is constrained to the seven manual signoff keys, or an empty string after final signoff, so CI, MCP tools, webhook consumers, and release automation can route reviewer actions without scraping Markdown:

`scripts/prepare-universal-forms-signoff-dashboard.sh` turns that same
`pending_queue` into
`/opt/worker/report/openpr/docs/openpr-universal-form-signoff-dashboard-2026-05-31.html`,
a reviewer-facing dashboard with a Start Here section, evidence links,
screenshots, and recorder commands for all pending rows. Verify it with
`scripts/verify-universal-forms-signoff-dashboard.sh`, then browser-render it
with `scripts/smoke-universal-forms-signoff-dashboard-render.sh` so the handoff
contains desktop/mobile screenshots of the exact reviewer queue. The render
smoke reads the current signoff status JSON, so it follows the queue count and
next reviewer key after each accepted row. When the pending queue reaches zero,
the dashboard switches Start Here to the finalizer, strict delivery-state audit,
and delivery-bundle audit commands.

```bash
scripts/prepare-universal-forms-signoff-dashboard.sh
scripts/verify-universal-forms-signoff-dashboard.sh
scripts/smoke-universal-forms-signoff-dashboard-render.sh
scripts/report-universal-forms-signoff-status-json.sh
scripts/verify-universal-forms-signoff-status-json.sh
scripts/smoke-universal-forms-signoff-status-json-contract.sh
scripts/smoke-universal-forms-signoff-status-output.sh
scripts/verify-universal-forms-next-signoff-review.sh
scripts/smoke-universal-forms-next-signoff-review-contract.sh
scripts/smoke-universal-forms-next-signoff-command.sh
scripts/smoke-universal-forms-manual-signoff-progression.sh
scripts/smoke-universal-forms-manual-signoff-commands.sh
scripts/status-universal-forms-delivery.sh
scripts/verify-universal-forms-delivery-status-json.sh
scripts/smoke-universal-forms-delivery-status-json-contract.sh
scripts/smoke-universal-forms-delivery-status-output.sh
```

The signoff status JSON contract smoke verifies that malformed copies with missing schema path, extra top-level keys, nested extra keys, string counters, invalid row keys, row order drift, invalid next-row keys, invalid row statuses, pending-count drift, evidence-map drift, final-signoff flag drift, or missing rows are rejected by the verifier. Its schema lives at `docs/schemas/openpr-universal-forms-signoff-status.schema.json`.
The readiness and signoff status JSON schemas pin the seven manual rows in reviewer order from `restaurant_template` through `overall`, so MCP, CLI, webhook, CI, and release consumers see the same prerequisite sequence as the reviewer runbook.
The signoff status output smoke verifies the reviewer-facing Markdown status report against the signoff status JSON, including gate summary counts, row table contents, next action, recorder command, finalizer branch, and null-leak prevention. Successful reporter subcommands stay quiet, and failed reporter subcommands replay captured stdout/stderr.
The next signoff command smoke reads the JSON next row, checks that the Markdown status report mirrors it, and dry-runs the recorder against temporary runbook/evidence copies so the suggested reviewer command stays executable.
The manual signoff progression smoke records each row on temporary runbook/evidence copies one at a time and verifies the accepted/pending counters, next-row key, actionable flag, and final signoff flag after every reviewer step. Successful recorder/status subcommands stay quiet, and failed subcommands replay captured stdout/stderr.
The signoff dashboard progression smoke renders and verifies the dashboard on temporary runbook/evidence copies for the initial 0/7 queue, after `restaurant_template` advances the next row to `frontend_usability`, and after 7/7 accepted when the dashboard must show finalizer and strict audit commands. It also confirms the official handoff files are unchanged, keeps successful subcommands quiet, and replays captured stdout/stderr only when a subcommand fails.
The UI review gallery and signoff dashboard browser render smokes also suppress Chromium screenshot success chatter, while replaying captured stderr if Chromium fails.
The next signoff review verifier checks the one-page reviewer aid against the signoff status JSON, row-specific evidence links, screenshot paths, and recorder command before a reviewer uses that page. Its contract smoke mutates temporary copies and requires heading, counter, next-key, recorder-command, screenshot-path, and verification-command drift to be rejected.
The delivery status command is the shortest read-only operator view. It verifies the JSON contracts, next signoff command smoke, row-by-row manual signoff progression smoke, all manual row command dry-runs, and pre-signoff release gate, then prints the stage, pending rows, accepted manual rows, total handoff progress, manifest file count, release-gate mode, and next reviewer evidence/check/recorder command. Successful prerequisite checks stay quiet on stdout and stderr; if a prerequisite fails, the command replays the captured prerequisite log for diagnosis. Use `--json` for CI or MCP consumers. The compact JSON includes `manual_signoff_queue`, mirroring the signoff `pending_queue` so MCP, CLI, webhook, CI, release automation, and web dashboards can consume the ordered reviewer queue from the same status response. Its `next_manual_signoff` object mirrors the next row's automated evidence, reviewer check, suggested evidence note, actionable flag, and recorder command from signoff status JSON. The delivery status JSON verifier cross-checks that compact output against readiness, development status, signoff status, delivery manifest, and release-gate JSON; its schema lives at `docs/schemas/openpr-universal-forms-delivery-status.schema.json`. The delivery status output smoke keeps the default human-readable output aligned with the compact JSON so reviewers see the same next key, reviewer check, evidence note, and recorder command as automation and now also rejects successful stderr chatter. The compact status JSON is generated on demand, not persisted as a manifest row, because it carries the current generation time and depends on the manifest file count.
The release and delivery status next manual keys are constrained to the same seven manual signoff keys, or an empty string after final signoff, so MCP, CLI, webhook, CI, and release consumers cannot route reviewer actions to an unknown row.
The delivery status JSON verifier also confirms those nested objects are
closed in the schema, rejects missing required keys or extra fields inside
`reports`, `next_manual_signoff`, `final_flags`, and `release_gate`, and its
contract smoke includes nested missing-field and extra-field samples so machine
consumers can rely on the compact status shape.
The compact status JSON also includes `verified_prerequisites`, a pinned
eight-item list covering readiness, development status, signoff status, delivery
manifest, next signoff command smoke, manual signoff progression smoke, all-row
manual command smoke, and the pre-signoff release gate JSON. The verifier
checks that list against schema order, and the contract smoke rejects missing,
extra, or reordered prerequisite entries.
The compact status JSON also includes `completion_summary`, which pins the
headline completion numbers used by MCP, CLI, webhook, CI, and release
consumers: engineering checks completed/total, engineering-complete flag,
manual accepted/total/remaining/blocked rows, total handoff
completed/total/remaining/percent, manual-signoff release block flag, and
derived delivery state. `completion_breakdown` splits the same progress into
engineering checks, user-side manual signoff, and overall handoff rows with
completed/total/remaining/percent plus the current status marker.
`release_blockers` adds a fixed automated-checks, non-manual-tracker, and
manual-signoff clear/blocking list with blocker counts, required actions, and
evidence paths, so reviewer dashboards can explain the remaining release block
without parsing a reason string. `next_actions` pins the five ordered operator
actions with enabled flags, blocker keys, commands, and evidence paths, so
reviewer dashboards can show the next executable step directly from the compact
status response. It also includes `review_surfaces`, a compact set of
reviewer-facing paths for the runbook, automated evidence, manual evidence map,
user acceptance packet, signoff status report, next-row review, signoff
dashboard, and UI review gallery, so MCP, CLI, webhook, CI, release automation,
and dashboards can render the acceptance handoff without scraping Markdown.

The delivery manifest records the checksums for the formal evidence, completion audit, user acceptance packet, readiness summary, manual signoff status report, manual signoff status JSON, manual evidence map, runbook, UI artifacts, gate scripts, and repository docs. The delivery manifest verifier checks every manifest file row for path existence, byte size, and SHA256 so reviewers can confirm they are signing the same synchronized handoff bundle that passed the automated gates.

The delivery manifest JSON is the machine-readable mirror of the Markdown checksum manifest. It uses schema `openpr.universal_forms.delivery_manifest.v1`, includes `schema_path`, gate summary values, `file_count`, and every delivery file row with label, path, byte size, and SHA256. The JSON verifier checks pinned top-level keys, gate summary keys, file row keys, Markdown parity, and live file size/SHA so CI, release automation, MCP tools, and webhook consumers can consume the delivery bundle without scraping Markdown:

```bash
scripts/report-universal-forms-delivery-manifest-json.sh
scripts/verify-universal-forms-delivery-manifest-json.sh
scripts/smoke-universal-forms-delivery-manifest-json-contract.sh
```

The delivery manifest JSON contract smoke verifies that malformed copies with
missing schema path, extra top-level or nested keys, string counters,
file-count drift, missing file row checksums, checksum drift, file row order
drift, or a missing Markdown manifest are rejected by the verifier.

The UI artifact collector reruns the frontend build and browser smoke checks with screenshot capture enabled. It writes desktop and mobile screenshots for the project template wizard, template work-item creation, Forms UI, and restaurant ordering workflow so reviewers can inspect the actual rendered experience before signing. The UI artifact verifier checks that those screenshots are valid PNG files with the expected dimensions and that the browser smoke logs passed.

The UI review gallery groups those screenshots into one local HTML reviewer page and maps them back to the manual signoff rows. Use `scripts/verify-universal-forms-ui-review-gallery.sh` to confirm the gallery references every screenshot and smoke log before asking a reviewer to sign frontend-related rows. Use `scripts/smoke-universal-forms-ui-review-gallery-render.sh` to confirm Chromium can render the gallery and load all eight screenshot images.

The delivery bundle audit checks the generated handoff as one unit. It compares the tracker, formal evidence report, completion audit, user acceptance packet, manual evidence map, runbook, UI artifact manifest, and UI review gallery, verifies unsigned evidence is still rejected, and runs a temporary signed-copy finalizer drill without modifying the official tracker.

The delivery bundle refresh script is the operator-facing one-shot command for the handoff. In `--full` mode it regenerates the formal evidence, completion audit, completion audit JSON, manual evidence map, signoff status report, signoff status JSON, next signoff review, user acceptance packet, development status JSON, scenario catalog JSON, implementation map JSON, report output boundary smoke, delivery manifest, and delivery manifest JSON, then runs docs/protocol, delivery-state, and delivery-bundle audits in order. Its `--quick` mode writes only a temporary preflight report and does not overwrite formal delivery evidence. The script does not finalize acceptance; pending manual signoff must still be completed by a reviewer.

For a local first-run walkthrough, `scripts/bootstrap-restaurant-demo.sh` seeds
the restaurant scenario through the public API after `bash scripts/start.sh`.
It also writes local MCP demo credentials to `.env` when present and recreates a
running compose `mcp-server` so MCP clients use the same workspace. If the MCP
HTTP endpoint is reachable, it verifies `/mcp/rpc` with `projects.list` and
confirms the demo project appears through MCP. This is useful for reviewer
orientation, but it does not replace the formal acceptance evidence or manual
signoff.
For a repeatable technical proof of that same onboarding path, run
`scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh`; it uses temporary
PostgreSQL/API/MCP processes and a temporary env file.

The manual acceptance pass should not replace these commands. It checks whether the workflow is usable and understandable by a real operator.

## Manual Acceptance Summary

A reviewer should verify:

- A restaurant project can be created from `restaurant_ordering_default`.
- The project contains 7 forms: `menu_category`, `sku`, `table`, `order`, `order_line`, `print_job`, and `business_report`.
- The `restaurant_calc` plugin is installed automatically, active, has `wasm_sha256`, and declares an `order_line` formula hook.
- A user can create menu data, tables, an order, an order line, print jobs, and a daily report from the frontend.
- `order_line.line_total` is calculated as a decimal amount without floating-point drift.
- Parent-child record links and table-change events are visible through the API/event ledger.
- MCP aggregate returns the same revenue total as REST/frontend data.
- Webhook and print connectors can consume business events without requiring an agent.

Only after the runbook is completed should the tracker mark user-side acceptance as `已验收`.

Before changing the tracker to `已验收`, verify the signed evidence report:

```bash
scripts/report-universal-forms-signoff-status.sh --reviewer "<name>"
scripts/report-universal-forms-signoff-status.sh \
  --output /opt/worker/report/openpr/docs/openpr-universal-form-signoff-status-2026-05-31.md
scripts/report-universal-forms-signoff-status-json.sh
scripts/verify-universal-forms-signoff-status-json.sh
scripts/smoke-universal-forms-signoff-status-json-contract.sh
scripts/verify-universal-forms-next-signoff-review.sh
scripts/smoke-universal-forms-next-signoff-review-contract.sh
scripts/smoke-universal-forms-next-signoff-command.sh
scripts/smoke-universal-forms-manual-signoff-progression.sh
scripts/verify-universal-forms-acceptance-signoff.sh \
  /opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md \
  --runbook /opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md
```

The verifier fails if the runbook is missing, the runbook and evidence manual tables disagree, automated checks failed, PASS status lines do not match the automated check total, the manual signoff section is missing, or any signoff row is still pending or marked for rework.

After the verifier passes, finalize the tracker with:

```bash
scripts/finalize-universal-forms-acceptance.sh \
  --runbook /opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md
scripts/audit-universal-forms-delivery-state.sh --strict
scripts/audit-universal-forms-delivery-bundle.sh
scripts/report-universal-forms-completion-audit-json.sh
scripts/verify-universal-forms-completion-audit-json.sh
scripts/smoke-universal-forms-completion-audit-json-contract.sh
scripts/gate-universal-forms-release.sh
scripts/verify-universal-forms-release-gate-json.sh
scripts/smoke-universal-forms-release-gate-json-contract.sh
scripts/smoke-universal-forms-release-gate.sh
scripts/smoke-universal-forms-release-gate-output.sh
scripts/smoke-universal-forms-next-signoff-review-contract.sh
scripts/smoke-universal-forms-next-signoff-command.sh
scripts/smoke-universal-forms-manual-signoff-progression.sh
scripts/smoke-universal-forms-manual-signoff-commands.sh
scripts/status-universal-forms-delivery.sh
scripts/verify-universal-forms-delivery-status-json.sh
scripts/smoke-universal-forms-delivery-status-json-contract.sh
scripts/smoke-universal-forms-delivery-status-output.sh
```

The finalize script reruns the signoff verifier, checks runbook/evidence manual signoff consistency, writes a candidate tracker, runs the strict delivery-state audit against that candidate and runbook, and only then replaces the tracker. An unsigned or internally inconsistent evidence report cannot be marked accepted by mistake.
The signoff verifier requires the runbook file as part of final evidence; it fails if the runbook is missing instead of accepting the evidence report alone.
The completion audit JSON schema lives at `docs/schemas/openpr-universal-forms-completion-audit.schema.json`; verify it with `scripts/verify-universal-forms-completion-audit-json.sh` and keep `scripts/smoke-universal-forms-completion-audit-json-contract.sh` green before CI, MCP tools, webhook consumers, or release automation consume the completion gate. The verifier rejects extra fields inside `reports`, `gates`, `tracker_status`, `manual_signoff`, and `finalization`, so completion-gate consumers do not need to accept undocumented nested shape drift.
Before final user signoff is complete, `scripts/gate-universal-forms-release.sh --allow-pending` is the expected pre-release handoff gate. Strict `scripts/gate-universal-forms-release.sh` must continue to fail until all seven manual rows are accepted and the tracker has been finalized.
`scripts/smoke-universal-forms-release-gate.sh` keeps the pre-signoff, strict, and JSON release-gate outputs under regression coverage.
`scripts/smoke-universal-forms-release-gate-output.sh` verifies that the human-readable release gate output mirrors the release gate JSON decision in both `--allow-pending` and strict modes.
The release gate JSON schema lives at `docs/schemas/openpr-universal-forms-release-gate.schema.json`; verify it with `scripts/verify-universal-forms-release-gate-json.sh` and keep its negative contract checks green with `scripts/smoke-universal-forms-release-gate-json-contract.sh` before CI, MCP tools, webhook consumers, or release automation consume the gate decision. The verifier confirms `reports` and `tracker_status` are closed in the schema, and rejects missing required keys or extra fields inside those objects, so release consumers do not need to tolerate undocumented nested shape drift.
The release gate mode enum is `blocked`, `pre_signoff`, and `release`; the compact delivery status JSON mirrors that same enum so final release is not reported as a separate `ready` mode.
The release and delivery status next manual keys are constrained to the same seven manual signoff keys, or an empty string after final signoff, so the release decision and compact status view expose the same reviewer routing boundary.
`scripts/smoke-universal-forms-next-signoff-review-contract.sh` keeps the reviewer-facing next signoff page under negative contract coverage before and after each row is recorded.
`scripts/smoke-universal-forms-next-signoff-command.sh` keeps the machine-readable next manual signoff command executable before and after each row is recorded.
`scripts/smoke-universal-forms-manual-signoff-progression.sh` keeps the row-by-row acceptance progression under smoke coverage before final signoff.
`scripts/smoke-universal-forms-signoff-dashboard-progression.sh` keeps the reviewer dashboard under browser-render regression coverage for initial, after-first-row, and finalizer states using only temporary signoff files.
`scripts/smoke-universal-forms-manual-signoff-commands.sh` records all seven manual signoff keys on temporary runbook/evidence copies, keeps `overall` last, and runs the final signoff verifier against those temporary fully signed files.
For the default handoff paths, successful finalization refreshes the completion audit, completion audit JSON, manual evidence map, UI review gallery, signoff status report, signoff status JSON, next signoff review, user acceptance packet, readiness summary, readiness JSON, development status JSON, scenario catalog JSON, implementation map JSON, report output boundary smoke, next signoff review contract smoke, next signoff command smoke, manual signoff progression smoke, signoff dashboard progression smoke, all manual signoff command smoke, delivery manifest, delivery manifest JSON, and their focused verifiers/contract smokes after the tracker is replaced so checksums and final status stay synchronized. It then reruns strict delivery-state and delivery-bundle audits against the official handoff.
If the tracker is already finalized, rerunning the finalizer is idempotent: it verifies the existing final state and does not append another final acceptance evidence row.
