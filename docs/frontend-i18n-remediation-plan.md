# Frontend i18n remediation plan

Date: 2026-06-01

## Scope

Audit and remediate user-visible internationalization coverage in the Svelte
frontend under `frontend/src/routes`.

The runtime supports `zh` and `en` through `frontend/src/lib/i18n/en.json`,
`frontend/src/lib/i18n/zh.json`, and the local `svelte-i18n` shim.

## Current audit baseline

Route pages audited: 42 `+page.svelte` files.

| Category | Count | Meaning |
| --- | ---: | --- |
| No i18n integration | 2 | Page has visible strings but no `$t()` usage |
| Partial i18n | 7 | Page uses `$t()` but still has visible hardcoded strings |
| Clean or no detected visible hardcoded text | 33 | No obvious visible hardcoded route text found by source scan |

## Pages requiring remediation

### No i18n integration

- `frontend/src/routes/(app)/workspace/[workspaceId]/connections/+page.svelte`
  - Connector list, MCP control panel, invocation history, create connector modal,
    invocation detail modal, toast messages, validation errors.
- `frontend/src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/+page.svelte`
  - Universal forms header, form definition, record editor, record grid, detail
    pane, record links, print jobs, toast messages, validation errors.

### Partial i18n

- `frontend/src/routes/(app)/workspace/[workspaceId]/projects/+page.svelte`
  - Project types management.
- `frontend/src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/+page.svelte`
  - Project resources management and Forms entry card label.
- `frontend/src/routes/(app)/governance/+page.svelte`
  - Runtime statistics strings are hardcoded Chinese and break English locale.
- `frontend/src/routes/(app)/workspace/+page.svelte`
  - Load more activity button.
- `frontend/src/routes/(app)/ai-agents/[id]/learning/+page.svelte`
  - Review and Rating labels.
- `frontend/src/routes/(app)/governance/audit-logs/+page.svelte`
  - Old/new diff labels.
- `frontend/src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/[issueId]/+page.svelte`
  - Sprint label.

Low-risk exceptions:

- Brand text such as `OpenPR`.
- Technical protocol labels such as `MCP`, `CLI`, `REST`, `SKU`, `JSON`.
- User-authored data returned by API, for example project names, form names,
  connector names, and dynamic schema field labels.

## Execution plan

1. Add i18n groups for `connections`, `forms`, `projectTypes`, `resources`,
   and `governance.stats`.
2. Convert the two no-i18n pages to use `$t()` for all chrome, labels, buttons,
   validation, confirms, toast messages, and empty states.
3. Convert partial pages listed above.
4. Synchronize `document.documentElement.lang` with the selected locale and
   stop serving a permanently incorrect `lang="en"` for the default Chinese UI.
5. Re-run the page source audit and JSON parity check.
6. Build frontend, rebuild the Podman frontend image, deploy, and verify
   `http://10.72.0.3:3000` after restart.

## Acceptance criteria

- `en.json` and `zh.json` contain the same leaf-key set.
- `frontend/src/app.html` no longer contradicts the default runtime locale.
- All listed no-i18n and partial-i18n pages are converted or documented as
  intentional technical/brand exceptions.
- Frontend build succeeds.
- Local Podman deployment is rebuilt and reachable through `10.72.0.3`.

## Execution result

Status after remediation:

| Check | Result |
| --- | --- |
| Route pages audited | 42 |
| No i18n integration pages | 0 |
| Partial i18n pages | 0 |
| Clean or no detected visible hardcoded text | 42 |
| English i18n leaf keys | 1324 |
| Chinese i18n leaf keys | 1324 |
| Key parity | PASS |
| Non-i18n CJK in Svelte/TS | Comments only |
| `npm run check` | PASS, 0 errors / 0 warnings |
| `npm run build` | PASS |

Notes:

- Brand and protocol tokens such as `OpenPR`, `MCP`, `REST`, `CLI`, `SKU`,
  `JSON`, `URL`, and `ID` remain as intentional technical/brand exceptions.
- API-owned user data such as project names, form names, schema field labels,
  connector names, and tool names remain untranslated by design.
