# Universal Forms Implementation Map

This map is the developer-facing bridge between the universal forms plan and
the current source tree. It answers four handoff questions for each delivery
area:

- where the module is implemented;
- which public surfaces expose it;
- which command proves it still works;
- which tracker status is allowed after the proof passes.

The current handoff is ready for user-side manual signoff. Do not mark final
acceptance until all seven manual signoff rows are accepted and the finalizer
plus strict release gates pass.

## Status Markers

| Marker | Meaning | Allowed transition |
| --- | --- | --- |
| `待处理` | Planned, but implementation or reviewer action has not started. | Move to `开发中` or `已完成` only after concrete implementation starts or lands. |
| `开发中` | Implementation is in progress and not yet self-tested. | Move to `已完成` after the module is code-complete. |
| `已完成` | Code or document work is complete, but the relevant gate has not passed yet. | Move to `已测试` only after the listed verification command passes. |
| `已测试` | Automated verification passed for this module. | Move to `已验收` only after user-side signoff, finalizer, strict delivery-state audit, and delivery-bundle audit pass where required. |
| `已验收` | The module satisfies delivery requirements and acceptance is finalized. | Only valid after the official acceptance chain is complete. |

## Module Map

| Delivery area | Implementation paths | Public surface | Primary verification | Current marker |
| --- | --- | --- | --- | --- |
| Project types and scenario templates | `apps/api/src/routes/project.rs`, `apps/api/src/routes/project_type.rs`, `apps/api/src/routes/scenario_template.rs`, `migrations/0029_scenario_templates.sql` | Project creation API, frontend project wizard, MCP scenario-template tools | `scripts/smoke-scenario-template-forms.sh`, `frontend/scripts/smoke-project-template-wizard.mjs` | `已测试` |
| Universal form definitions and records | `apps/api/src/forms/`, `apps/api/src/routes/form.rs`, `migrations/0030_universal_forms.sql` | REST forms API, frontend forms UI, MCP forms tools | `cargo test -p api forms::`, `scripts/smoke-universal-forms-api.sh`, `scripts/smoke-forms-mcp.sh` | `已测试` |
| Decimal-safe amount fields | `apps/api/src/forms/decimal.rs`, `apps/api/src/forms/values.rs`, `apps/api/src/forms/projections.rs` | Amount field input, indexed projections, aggregate reads | `cargo test -p api forms::`, `scripts/smoke-universal-forms-api.sh` | `已测试` |
| Subforms and record links | `apps/api/src/routes/form.rs`, `migrations/0030_universal_forms.sql`, `frontend/src/lib/api/forms.ts` | Parent-child links, reference links, frontend detail workflow | `scripts/smoke-universal-forms-api.sh`, `frontend/scripts/smoke-forms-ui.mjs` | `已测试` |
| Business events and operation records | `apps/api/src/events/`, `apps/api/src/middleware/bot_auth.rs`, `apps/worker/src/main.rs`, `migrations/0051_bot_operation_logs.sql` | Business events, metadata-only bot operation records, retention | `cargo test --workspace --all-features` | `已测试` |
| Webhooks | `apps/api/src/routes/webhook.rs`, `apps/api/src/webhook_trigger.rs` | Passive HMAC-signed webhook consumers | `scripts/smoke-webhook-generic-consumer.sh` | `已测试` |
| WASM plugin runtime | `apps/api/src/plugins/`, `apps/api/src/routes/plugin.rs`, `migrations/0033_plugins_wasm.sql` | Plugin install/invoke API, field validators, formulas, event handlers | `scripts/smoke-wasm-plugin-runtime.sh`, `scripts/smoke-plugins-mcp.sh` | `已测试` |
| MCP business surface | `apps/mcp-server/src/tools/forms.rs`, `apps/mcp-server/src/tools/plugins.rs`, `apps/mcp-server/src/tools/operation_logs.rs`, `apps/mcp-server/src/tools/scenario_templates.rs` | 98-tool MCP registry over HTTP, stdio, and SSE | `skills/openpr-mcp/scripts/validate-mcp.sh`, `skills/openpr-mcp/scripts/mcp-regression.py` | `已测试` |
| Frontend operator workflow | `frontend/src/routes/(app)/workspace/[workspaceId]/projects/`, `frontend/src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/`, `frontend/src/lib/api/forms.ts` | Project template wizard, grid/detail forms, restaurant ordering workflow | `bun run check`, `bun run build`, `bun run smoke:forms-ui`, `bun run smoke:restaurant-ordering` | `已测试` |
| Restaurant reference scenario | `scripts/bootstrap-restaurant-demo.sh`, `apps/api/src/routes/project.rs`, `frontend/scripts/smoke-restaurant-ordering.mjs` | Menu, SKU, table, order, order lines, table change, report aggregate | `frontend/scripts/smoke-restaurant-ordering.mjs`, `scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh` | `已测试` |
| Delivery evidence and release gates | `scripts/acceptance-universal-forms.sh`, `scripts/status-universal-forms-delivery.sh`, `scripts/gate-universal-forms-release.sh`, `scripts/finalize-universal-forms-acceptance.sh` | Evidence report, readiness JSON, signoff JSON, manifest JSON, release-gate JSON | `scripts/audit-universal-forms-delivery-bundle.sh`, `scripts/status-universal-forms-delivery.sh --json` | `已测试` |
| User-side manual acceptance | `/opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md`, `/opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md` | Seven manual signoff rows and final acceptance finalizer | `scripts/verify-universal-forms-acceptance-signoff.sh`, `scripts/finalize-universal-forms-acceptance.sh`, strict `scripts/gate-universal-forms-release.sh` | `待处理` |

## Delivery Rule

A module can be called code-complete only when the implementation paths exist
and the public surface is wired. It can be called tested only when the primary
verification passes and the result is recorded in the tracker or generated
evidence. It can be called accepted only when the manual signoff and finalizer
requirements for that area pass.

For a compact current-state check, run:

```bash
scripts/status-universal-forms-delivery.sh --json
```

For the full pre-signoff delivery gate, run:

```bash
scripts/audit-universal-forms-delivery-bundle.sh
```

Strict release remains blocked until user-side manual signoff is complete:

```bash
scripts/gate-universal-forms-release.sh --json
```
