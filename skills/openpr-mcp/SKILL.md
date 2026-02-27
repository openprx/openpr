---
name: openpr-mcp
description: Manage projects, issues, sprints, labels, comments, proposals, and files via the OpenPR MCP server. Supports HTTP, stdio, and SSE transports with bot token authentication.
---

# OpenPR MCP Skill

## When to use
Use this skill when:
- Creating, updating, or searching issues/work items
- Managing sprints, labels, or project settings
- Uploading files (logs, zips, screenshots) and attaching to issues or comments
- Creating or reviewing governance proposals
- Searching across projects for related work
- Automating project management workflows from a coding agent

## Fast start (functional lines)

Run these in order so a client can use OpenPR MCP immediately.

### 1. Capability line (verify connectivity)
```
tools/list                          → enumerate all 34 tools
members.list                       → verify auth + workspace access
projects.list                      → verify project data
```

### 2. Read line (explore existing data)
```
projects.list → projects.get → work_items.list → work_items.get
search.all                         → full-text search
work_items.get_by_identifier       → lookup by human ID (e.g. PRX-42)
labels.list                        → available labels
sprints.list                       → active sprints
proposals.list                     → governance proposals
```

### 3. Write line (create and modify)
```
work_items.create → work_items.update → work_items.delete
comments.create → comments.delete
labels.create → labels.update → labels.delete
sprints.create → sprints.update → sprints.delete
proposals.create
```

### 4. Label management line
```
work_items.add_label → work_items.add_labels → work_items.list_labels → work_items.remove_label
```

### 5. File upload line
```
files.upload                       → upload base64 file, returns URL
work_items.create { attachments }  → create issue with uploaded files
comments.create { attachments }    → comment with uploaded files
work_items.update { attachments }  → add files to existing issue
```

### 6. Cleanup line
```
work_items.delete                  → remove test data
labels.delete                      → remove test labels
sprints.delete                     → remove test sprints
comments.delete                    → remove test comments
```

## Mandatory workflow

### Before creating an issue
1. `search.all` with keywords to check for duplicates.
2. `labels.list` to find existing labels (don't create duplicates).
3. `members.list` if assigning (get valid `assignee_id`).
4. Create with appropriate `state` and `priority`.

### File attachments
1. Upload file first: `files.upload { filename, content_base64 }`.
2. Use returned URL in `attachments` array of `work_items.create`, `work_items.update`, or `comments.create`.
3. Attachments are appended to description/content as markdown links.

### Issue lifecycle
```
backlog → todo → in_progress → done
```
- Priority: `none` | `low` | `medium` | `high` | `urgent`
- Use `work_items.update` to transition state.

## Field reference

### work_items.create
| Field | Required | Type | Values |
|-------|----------|------|--------|
| `project_id` | Yes | UUID | From `projects.list` |
| `title` | Yes | string | Issue title |
| `description` | No | string | Markdown description |
| `state` | No | enum | `backlog` / `todo` / `in_progress` / `done` |
| `priority` | No | enum | `none` / `low` / `medium` / `high` / `urgent` |
| `assignee_id` | No | UUID | From `members.list` |
| `due_at` | No | ISO 8601 | e.g. `2026-03-15T00:00:00Z` |
| `attachments` | No | string[] | URLs from `files.upload` |

### files.upload
| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `filename` | Yes | string | e.g. `error.log`, `debug.zip` |
| `content_base64` | Yes | string | Base64-encoded file content |

Supported types: images, videos, `.zip`, `.gz`, `.tar.gz`, `.log`, `.txt`, `.pdf`, `.json`, `.csv`, `.xml`

### labels.create
| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `name` | Yes | string | Label name |
| `color` | Yes | string | Hex color, e.g. `#ef4444` |

### sprints.create
| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `project_id` | Yes | UUID | Target project |
| `name` | Yes | string | Sprint name |
| `start_date` | No | string | `YYYY-MM-DD` |
| `end_date` | No | string | `YYYY-MM-DD` |

## Response format

All tools return:
```json
{ "code": 0, "message": "success", "data": { ... } }
```

Errors:
```json
{ "code": 400, "message": "state must be one of: backlog, todo, in_progress, done" }
```

## Workflow templates

### Bug report with log attachment
```
files.upload { filename: "error.log", content_base64: "<base64>" }
  → { url: "/api/v1/uploads/uuid.log" }

work_items.create {
  project_id: "...",
  title: "Login fails with 500",
  description: "Steps to reproduce:\n1. ...\n2. ...",
  state: "backlog",
  priority: "high",
  attachments: ["/api/v1/uploads/uuid.log"]
}

work_items.add_label { work_item_id: "...", label_id: "<bug-label-id>" }
```

### Sprint kickoff
```
sprints.create { project_id: "...", name: "Sprint 5", start_date: "2026-03-01", end_date: "2026-03-14" }
work_items.list { project_id: "..." }
  → review backlog items
work_items.update { work_item_id: "...", state: "todo" }
  → move selected items to sprint
```

### Code review comment with screenshot
```
files.upload { filename: "screenshot.png", content_base64: "<base64>" }
comments.create {
  work_item_id: "...",
  content: "Found the issue in auth middleware. See screenshot.",
  attachments: ["/api/v1/uploads/uuid.png"]
}
```

## Scripts

- Regression test: `scripts/mcp-regression.py` — tests all 34 tools across 3 transports
- Validation: `scripts/validate-mcp.sh` — quick smoke test for connectivity

## References

- Tool implementations: `apps/mcp-server/src/tools/`
- API client: `apps/mcp-server/src/client/mod.rs`
- Transport setup: `apps/mcp-server/src/main.rs`
- Bot token routes: `apps/api/src/routes/bot.rs`
- Upload handler: `apps/api/src/routes/upload.rs`
