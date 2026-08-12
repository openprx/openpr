use crate::client::{ListRecordsQuery, ListWorkItemsQuery, OpenPrClient};
use crate::protocol::{CallToolParams, CallToolResult, JsonRpcError, JsonRpcRequest, JsonRpcResponse, ListToolsResult};
use crate::tools;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::Mutex;

const SKILL_GUIDE_MD: &str = r"# OpenPR MCP Skill Guide

## Tools (105)

### Projects: projects.list, projects.get, projects.create, projects.update, projects.delete
### Project Types: project_types.list, project_types.get
### Scenario Templates: scenario_templates.list, scenario_templates.get, scenario_templates.install
### Project Resources: project_resources.list, project_resources.create, project_resources.update, project_resources.delete
### Context: context.get_project, context.get_governance, context.get_agent_policy
### Connectors: connectors.list, connectors.get
### Invocations: invocations.list, invocations.get, invocations.create, invocations.report_progress, invocations.complete, invocations.fail
### Universal Forms: forms.list, forms.get, forms.create, forms.create_from_template, forms.update_schema, forms.duplicate, forms.schema_summary, forms.field_usage, forms.field_dependencies, form_schema_versions.list, form_schema_versions.get, form_permissions.get, form_permissions.update, form_views.list, form_attachments.list, form_attachments.create, form_attachments.archive, form_attachments.restore, form_records.list, form_records.export, form_records.import_preview, form_records.import_commit, form_records.get, form_records.create, form_records.update, form_records.link, form_records.relation_targets, form_records.children, form_records.child_create, form_records.child_update, form_records.child_archive, form_records.child_restore, form_records.aggregate, events.tail
### Plugins: plugins.list, plugins.get, plugins.install, plugins.invoke, plugin_invocations.list
### Work Items: work_items.list, work_items.get, work_items.get_by_identifier, work_items.create, work_items.update, work_items.delete, work_items.search, work_items.add_label, work_items.add_labels, work_items.remove_label, work_items.list_labels
### Comments: comments.create, comments.list, comments.delete
### Files: files.upload (base64 -> URL)
### Labels: labels.list, labels.list_by_project, labels.create, labels.update, labels.delete
### Sprints: sprints.list, sprints.create, sprints.update, sprints.delete
### Proposals: proposals.list, proposals.get, proposals.create, proposals.create_from_result, check_results.create
### Release: release.readiness.get (gates, blockers, next actions)
### Code Scenarios: code.resources.list, code.directory.get, code.task_context.get, code.change_proposal.create
### Traditional Scenarios: documents.extract_summary, documents.review_risk, approval.request, inspection.report, corrective_action.propose
### Other: members.list, search.all

## Workflow: Bug Report
1. files.upload -> upload log/screenshot
2. work_items.create -> create issue with attachments
3. work_items.add_label -> tag it
4. comments.create -> add context

## Workflow: Sprint Planning
1. sprints.create -> create sprint
2. work_items.list -> review backlog
3. work_items.update -> assign and set state

## Field Values
- state: any key of the project workflow (default set: backlog | todo | in_progress | done); omit on create to use the workflow's initial state
- priority: low | medium | high | urgent
- attachments: array of URLs from files.upload
- decimal/amount form fields: send decimal strings, never JSON numbers
- list tools are paginated: read pagination_hint / total_pages before summarising

## Authorization
Every call is checked against the agent policy of the project that owns the data.
Ids must be canonical UUIDs. Do not send a project_id that does not own the target:
the real owner is read back from the API and a mismatch is refused.

These tools currently refuse every call, because the API exposes no way to read the
owning project of the object they address, so their policy cannot be evaluated:
form_attachments.archive, form_attachments.restore (needs GET /api/v1/form-attachments/{attachment_id}),
sprints.update, sprints.delete (needs GET /api/v1/sprints/{sprint_id}),
comments.delete (needs GET /api/v1/comments/{comment_id}).

Workspace wide tools carry no owning project and are therefore not filtered by a
project policy: projects.list, projects.create, project_types.*, scenario_templates.list,
scenario_templates.get, members.list, search.all, work_items.search, files.upload,
proposals.get, labels.create, labels.list, labels.update, labels.delete.
";

const AGENTS_GUIDE_MD: &str = r#"# OpenPR Agent Guide

## Build
cargo build --release --bin mcp-server

## Test
curl -X POST http://localhost:8090/mcp/rpc -H "Content-Type: application/json" -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}"

## Commit Style: Conventional Commits (feat/fix/docs/ci)
"#;

const WORKFLOWS_GUIDE_MD: &str = r#"# Common Workflows

## Bug Report with Attachment
files.upload { filename: "error.log", content_base64: "<base64>" } -> { url }
work_items.create { project_id, title, description, state: "backlog", priority: "high", attachments: [url] }
work_items.add_label { work_item_id, label_id }

## Search and Triage
search.all { query: "error keyword" }
work_items.get_by_identifier { identifier: "PRX-42" }
work_items.update { work_item_id, state: "in_progress", priority: "urgent" }
comments.create { work_item_id, content: "triage notes" }

## Sprint Planning
sprints.create { project_id, name, start_date, end_date }
work_items.list { project_id }
work_items.update { work_item_id, state: "todo" }
"#;

/// How a single `tools/call` is bound to a project agent policy.
///
/// The scope is taken from the *registered tool schema*, never from the arguments of
/// the call, because the arguments are attacker controlled: a caller that could turn
/// `work_items.delete` into a "declared project" tool simply by adding a `project_id`
/// would pick the policy that authorizes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyScope {
    /// The request itself is addressed by `project_id` (`/api/v1/projects/{id}/...`),
    /// so the declared value *is* the call target and is authoritative — once it has
    /// been validated as a canonical UUID.
    DeclaredProject { required: bool },
    /// The request is addressed by a resource id. The owning project is read back from
    /// the API; a `project_id` in the arguments is never the policy subject and any
    /// disagreement with the real owner refuses the call.
    OwnedBy(OwnerLookup),
    /// Nothing the call touches belongs to a project: workspace wide surfaces
    /// (workspace labels, workspace search, members, uploads) and global catalogues
    /// (project types, scenario templates). No project agent policy can govern these;
    /// see `report`/`README` notes and `TOOL_POLICY_SCOPES` for the full list.
    WorkspaceWide,
}

/// The API read that maps a resource id to its owning project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerLookup {
    /// `record_id` / `form_id` / `attachment_id`.
    FormData,
    /// `work_item_id` -> `GET /api/v1/issues/{id}`.
    WorkItem,
    /// `identifier` -> `GET /api/v1/issues/by-identifier/{identifier}`.
    WorkItemIdentifier,
    /// `plugin_id` -> `GET /api/v1/plugins/{id}`.
    Plugin,
    /// `invocation_id` -> `GET /api/v1/invocations/{id}`.
    Invocation,
    /// `connector_id` -> `GET /api/v1/workspaces/{workspace_id}/connectors/{id}`.
    Connector,
    /// `check_result_id` -> `GET /api/v1/check-results/{id}`.
    CheckResult,
    /// `sprint_id`. Sprints are project owned (`migrations/0004_sprints.sql`) but the
    /// API exposes no `GET /api/v1/sprints/{sprint_id}`, so the owner cannot be read
    /// back and the call fails closed.
    Sprint,
    /// `comment_id`. Comments are owned through their work item but the API exposes no
    /// `GET /api/v1/comments/{comment_id}`, so the call fails closed.
    Comment,
}

impl OwnerLookup {
    /// The argument this lookup is keyed by, for error messages.
    const fn argument(self) -> &'static str {
        match self {
            Self::FormData => "form_id, record_id or attachment_id",
            Self::WorkItem => "work_item_id",
            Self::WorkItemIdentifier => "identifier",
            Self::Plugin => "plugin_id",
            Self::Invocation => "invocation_id",
            Self::Connector => "connector_id",
            Self::CheckResult => "check_result_id",
            Self::Sprint => "sprint_id",
            Self::Comment => "comment_id",
        }
    }
}

/// Ownership bearing arguments in resolution order. A tool without `project_id` is
/// governed through the first of these its schema declares.
///
/// `proposal_id`, `label_id` and `resource_id` are deliberately absent: proposals and
/// labels carry no project column (`migrations/0012_governance_phase1.sql`,
/// `migrations/0003_labels.sql`), and `resource_id` only ever appears next to a
/// `project_id` that already scopes the request path.
const OWNERSHIP_ARGUMENTS: [(&str, OwnerLookup); 11] = [
    ("record_id", OwnerLookup::FormData),
    ("form_id", OwnerLookup::FormData),
    ("attachment_id", OwnerLookup::FormData),
    ("work_item_id", OwnerLookup::WorkItem),
    ("identifier", OwnerLookup::WorkItemIdentifier),
    ("plugin_id", OwnerLookup::Plugin),
    ("invocation_id", OwnerLookup::Invocation),
    ("connector_id", OwnerLookup::Connector),
    ("check_result_id", OwnerLookup::CheckResult),
    ("sprint_id", OwnerLookup::Sprint),
    ("comment_id", OwnerLookup::Comment),
];

/// The policy scope of every registered tool.
///
/// `server::tests::tool_policy_scopes_match_the_registered_tool_schemas` re-derives
/// this table from the live tool registry (`tools::get_all_tool_definitions`), so a new
/// or renamed tool cannot silently land outside the policy gate — including tools whose
/// name shares no prefix with the family they belong to, such as `events.tail`.
const TOOL_POLICY_SCOPES: [(&str, PolicyScope); 105] = [
    ("projects.list", PolicyScope::WorkspaceWide),
    ("projects.get", PolicyScope::DeclaredProject { required: true }),
    ("projects.create", PolicyScope::WorkspaceWide),
    ("projects.update", PolicyScope::DeclaredProject { required: true }),
    ("projects.delete", PolicyScope::DeclaredProject { required: true }),
    ("project_types.list", PolicyScope::WorkspaceWide),
    ("project_types.get", PolicyScope::WorkspaceWide),
    ("scenario_templates.list", PolicyScope::WorkspaceWide),
    ("scenario_templates.get", PolicyScope::WorkspaceWide),
    (
        "scenario_templates.install",
        PolicyScope::DeclaredProject { required: true },
    ),
    (
        "project_resources.list",
        PolicyScope::DeclaredProject { required: true },
    ),
    (
        "project_resources.create",
        PolicyScope::DeclaredProject { required: true },
    ),
    (
        "project_resources.update",
        PolicyScope::DeclaredProject { required: true },
    ),
    (
        "project_resources.delete",
        PolicyScope::DeclaredProject { required: true },
    ),
    ("connectors.list", PolicyScope::DeclaredProject { required: true }),
    ("connectors.get", PolicyScope::OwnedBy(OwnerLookup::Connector)),
    ("invocations.list", PolicyScope::DeclaredProject { required: true }),
    ("invocations.get", PolicyScope::OwnedBy(OwnerLookup::Invocation)),
    ("invocations.create", PolicyScope::DeclaredProject { required: true }),
    (
        "invocations.report_progress",
        PolicyScope::OwnedBy(OwnerLookup::Invocation),
    ),
    ("invocations.complete", PolicyScope::OwnedBy(OwnerLookup::Invocation)),
    ("invocations.fail", PolicyScope::OwnedBy(OwnerLookup::Invocation)),
    ("forms.list", PolicyScope::DeclaredProject { required: true }),
    ("forms.get", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("forms.create", PolicyScope::DeclaredProject { required: true }),
    (
        "forms.create_from_template",
        PolicyScope::DeclaredProject { required: true },
    ),
    ("forms.update_schema", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("forms.duplicate", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("forms.schema_summary", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("forms.field_usage", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("forms.field_dependencies", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_schema_versions.list", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_schema_versions.get", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_permissions.get", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_permissions.update", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_views.list", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_attachments.list", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_attachments.create", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_attachments.archive", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_attachments.restore", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_records.list", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_records.export", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    (
        "form_records.import_preview",
        PolicyScope::OwnedBy(OwnerLookup::FormData),
    ),
    (
        "form_records.import_commit",
        PolicyScope::OwnedBy(OwnerLookup::FormData),
    ),
    ("form_records.get", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_records.create", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_records.update", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_records.link", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    (
        "form_records.relation_targets",
        PolicyScope::OwnedBy(OwnerLookup::FormData),
    ),
    ("form_records.children", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_records.child_create", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_records.child_update", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    (
        "form_records.child_archive",
        PolicyScope::OwnedBy(OwnerLookup::FormData),
    ),
    (
        "form_records.child_restore",
        PolicyScope::OwnedBy(OwnerLookup::FormData),
    ),
    ("form_records.aggregate", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("events.tail", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("plugins.list", PolicyScope::DeclaredProject { required: true }),
    ("plugins.get", PolicyScope::OwnedBy(OwnerLookup::Plugin)),
    ("plugins.install", PolicyScope::DeclaredProject { required: true }),
    ("plugins.invoke", PolicyScope::OwnedBy(OwnerLookup::Plugin)),
    ("plugin_invocations.list", PolicyScope::OwnedBy(OwnerLookup::Plugin)),
    ("context.get_project", PolicyScope::DeclaredProject { required: true }),
    (
        "context.get_governance",
        PolicyScope::DeclaredProject { required: true },
    ),
    (
        "context.get_agent_policy",
        PolicyScope::DeclaredProject { required: true },
    ),
    ("work_items.list", PolicyScope::DeclaredProject { required: true }),
    ("work_items.get", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    (
        "work_items.get_by_identifier",
        PolicyScope::OwnedBy(OwnerLookup::WorkItemIdentifier),
    ),
    ("work_items.create", PolicyScope::DeclaredProject { required: true }),
    ("work_items.update", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("work_items.add_label", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("work_items.remove_label", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("work_items.list_labels", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("work_items.delete", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("work_items.search", PolicyScope::WorkspaceWide),
    ("work_items.add_labels", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("comments.list", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("comments.create", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("comments.delete", PolicyScope::OwnedBy(OwnerLookup::Comment)),
    ("files.upload", PolicyScope::WorkspaceWide),
    ("proposals.list", PolicyScope::DeclaredProject { required: true }),
    ("proposals.get", PolicyScope::WorkspaceWide),
    ("proposals.create", PolicyScope::DeclaredProject { required: true }),
    (
        "proposals.create_from_result",
        PolicyScope::OwnedBy(OwnerLookup::CheckResult),
    ),
    ("check_results.create", PolicyScope::DeclaredProject { required: true }),
    ("release.readiness.get", PolicyScope::DeclaredProject { required: true }),
    ("members.list", PolicyScope::WorkspaceWide),
    ("sprints.create", PolicyScope::DeclaredProject { required: true }),
    ("sprints.list", PolicyScope::DeclaredProject { required: true }),
    ("sprints.update", PolicyScope::OwnedBy(OwnerLookup::Sprint)),
    ("sprints.delete", PolicyScope::OwnedBy(OwnerLookup::Sprint)),
    ("labels.create", PolicyScope::WorkspaceWide),
    ("labels.list", PolicyScope::WorkspaceWide),
    (
        "labels.list_by_project",
        PolicyScope::DeclaredProject { required: true },
    ),
    ("labels.update", PolicyScope::WorkspaceWide),
    ("labels.delete", PolicyScope::WorkspaceWide),
    ("search.all", PolicyScope::WorkspaceWide),
    ("code.resources.list", PolicyScope::DeclaredProject { required: true }),
    ("code.directory.get", PolicyScope::DeclaredProject { required: true }),
    ("code.task_context.get", PolicyScope::DeclaredProject { required: true }),
    (
        "code.change_proposal.create",
        PolicyScope::DeclaredProject { required: true },
    ),
    (
        "documents.extract_summary",
        PolicyScope::DeclaredProject { required: true },
    ),
    ("documents.review_risk", PolicyScope::DeclaredProject { required: true }),
    ("approval.request", PolicyScope::DeclaredProject { required: true }),
    ("inspection.report", PolicyScope::DeclaredProject { required: true }),
    (
        "corrective_action.propose",
        PolicyScope::DeclaredProject { required: true },
    ),
];

/// Scope of one tool. Unregistered names are connector provided tools dispatched by
/// `tools::connectors::invoke_connector_tool`, which requires a `project_id` itself, so
/// they are governed as mandatory declared-project calls rather than skipped.
fn tool_policy_scope(tool_name: &str) -> PolicyScope {
    TOOL_POLICY_SCOPES
        .iter()
        .find(|(name, _)| *name == tool_name)
        .map_or(PolicyScope::DeclaredProject { required: true }, |(_, scope)| *scope)
}

/// The object a form scoped call operates on. The governing project is always
/// resolved from this object through the API, never from the call arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FormScopeTarget {
    Form(String),
    Record(String),
    Attachment(String),
}

impl FormScopeTarget {
    fn cache_key(&self) -> String {
        match self {
            Self::Form(id) => format!("form:{id}"),
            Self::Record(id) => format!("record:{id}"),
            Self::Attachment(id) => format!("attachment:{id}"),
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Form(id) => format!("form {id}"),
            Self::Record(id) => format!("record {id}"),
            Self::Attachment(id) => format!("attachment {id}"),
        }
    }
}

/// Picks the identifier that decides which project owns the data a form scoped call
/// touches, plus the `form_id` the caller claims it belongs to.
///
/// `record_id` wins over `form_id`: the record is the narrower object and its API
/// payload reports the real `form_id`, so a call that names both can be checked instead
/// of trusting the wider handle. Resolving through the *wider* id was the fragile part
/// of the previous version — a future tool taking both would have been governed by the
/// one the caller can pick freely.
fn form_scope_target(args: &Value) -> Result<Option<(FormScopeTarget, Option<String>)>, String> {
    let declared_form_id = uuid_argument(args, "form_id")?;
    if let Some(record_id) = uuid_argument(args, "record_id")? {
        return Ok(Some((FormScopeTarget::Record(record_id), declared_form_id)));
    }
    if let Some(form_id) = declared_form_id {
        return Ok(Some((FormScopeTarget::Form(form_id), None)));
    }
    Ok(uuid_argument(args, "attachment_id")?.map(|id| (FormScopeTarget::Attachment(id), None)))
}

/// A resource's owning project, plus the owning form when the API reports one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedOwner {
    project_id: String,
    form_id: Option<String>,
}

/// Upper bound for the resource to project mapping cache.
const PROJECT_ID_CACHE_LIMIT: usize = 1_024;

pub struct McpServer {
    client: OpenPrClient,
    /// Maps `form:<id>` / `record:<id>` / `work_item:<id>` / ... to the owning project.
    /// Only ownership is cached, never a policy: a record cannot move between projects,
    /// but an agent policy can be edited at any time and is therefore re-read on every
    /// call. A cache hit costs zero extra requests, a miss costs exactly one lookup
    /// (two only for an attachment whose payload reports just its form).
    project_id_cache: Mutex<HashMap<String, ResolvedOwner>>,
}

impl McpServer {
    pub fn new(client: OpenPrClient) -> Self {
        Self {
            client,
            project_id_cache: Mutex::new(HashMap::new()),
        }
    }

    pub async fn handle_request(&self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        tracing::info!(method = %req.method, id = ?req.id, "Handling MCP request");

        if req.jsonrpc != "2.0" {
            return Some(JsonRpcResponse::error(
                req.id,
                JsonRpcError::invalid_request("Invalid jsonrpc version"),
            ));
        }

        let is_notification = req.id.is_none();
        if is_notification && req.method == "notifications/initialized" {
            return None;
        }

        let response = match req.method.as_str() {
            "initialize" => self.handle_initialize(req.id),
            "notifications/initialized" | "ping" => JsonRpcResponse::success(req.id, json!({})),
            "tools/list" => self.handle_list_tools(req.id, req.params).await,
            "tools/call" => self.handle_call_tool(req.id, req.params).await,
            "resources/list" => self.handle_resources_list(req.id),
            "resources/templates/list" => self.handle_resources_templates_list(req.id),
            "resources/read" => self.handle_resources_read(req.id, req.params).await,
            _ => JsonRpcResponse::error(
                req.id,
                JsonRpcError::method_not_found(format!("Unknown method: {}", req.method)),
            ),
        };

        if is_notification { None } else { Some(response) }
    }

    async fn handle_list_tools(&self, id: Option<Value>, params: Option<Value>) -> JsonRpcResponse {
        let static_tools = tools::get_all_tool_definitions();
        let requested_project_id = match extract_tools_list_project_id(params.as_ref()) {
            Ok(project_id) => project_id,
            Err(error) => return JsonRpcResponse::error(id, JsonRpcError::invalid_params(error)),
        };
        let tools = match requested_project_id {
            Some(project_id) => match self.client.get_project_agent_policy(&project_id).await {
                Ok(policy) => match tools::capabilities::extract_enabled_tool_names(&policy) {
                    Some(enabled_tool_names) => {
                        let mut all_tools = static_tools;
                        all_tools.extend(tools::capabilities::extract_connector_tool_definitions(&policy));
                        let missing = tools::capabilities::unknown_tool_names(&all_tools, &enabled_tool_names);
                        if !missing.is_empty() {
                            tracing::warn!(
                                project_id = %project_id,
                                missing_tools = ?missing,
                                "Project agent policy references tools not registered by this MCP server"
                            );
                        }
                        tools::capabilities::filter_tool_definitions(all_tools, &enabled_tool_names)
                    }
                    None => static_tools,
                },
                Err(error) => {
                    return JsonRpcResponse::error(
                        id,
                        JsonRpcError::internal_error(format!(
                            "Failed to load project agent policy for tools/list: {error}"
                        )),
                    );
                }
            },
            None => static_tools,
        };
        let result = ListToolsResult { tools };

        match serde_json::to_value(&result) {
            Ok(value) => JsonRpcResponse::success(id, value),
            Err(e) => JsonRpcResponse::error(
                id,
                JsonRpcError::internal_error(format!("Failed to serialize tools: {e}")),
            ),
        }
    }

    async fn handle_call_tool(&self, id: Option<Value>, params: Option<Value>) -> JsonRpcResponse {
        let Some(params) = params else {
            return JsonRpcResponse::error(id, JsonRpcError::invalid_params("Missing params"));
        };

        let call_params: CallToolParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(id, JsonRpcError::invalid_params(format!("Invalid params: {e}")));
            }
        };

        let args = call_params
            .arguments
            .unwrap_or_else(|| Value::Object(serde_json::Map::default()));

        let result = self.call_tool(&call_params.name, args).await;

        match serde_json::to_value(&result) {
            Ok(value) => JsonRpcResponse::success(id, value),
            Err(e) => JsonRpcResponse::error(
                id,
                JsonRpcError::internal_error(format!("Failed to serialize result: {e}")),
            ),
        }
    }

    pub async fn call_tool(&self, name: &str, args: Value) -> CallToolResult {
        let started = Instant::now();
        let audit_args = redact_tool_arguments(&args);
        let result = match self.enforce_project_tool_policy(name, &args).await {
            Ok(()) => self.execute_tool(name, args).await,
            Err(error) => CallToolResult::error(error),
        };
        self.report_tool_call_audit(name, audit_args, &result, started.elapsed().as_millis())
            .await;
        result
    }

    /// Dispatches a tool *after* it has been authorized. Private on purpose: `call_tool`
    /// is the only entry point, so no transport (stdio, HTTP, CLI) can reach a tool
    /// without passing the project policy gate and the audit trail.
    async fn execute_tool(&self, name: &str, args: Value) -> CallToolResult {
        match name {
            // Projects
            "projects.list" => tools::projects::list_projects(&self.client, args).await,
            "projects.get" => tools::projects::get_project(&self.client, args).await,
            "projects.create" => tools::projects::create_project(&self.client, args).await,
            "projects.update" => tools::projects::update_project(&self.client, args).await,
            "projects.delete" => tools::projects::handle_delete_project(&self.client, args).await,
            "project_types.list" => tools::project_types::list_project_types(&self.client, args).await,
            "project_types.get" => tools::project_types::get_project_type(&self.client, args).await,
            "scenario_templates.list" => tools::scenario_templates::list_scenario_templates(&self.client, args).await,
            "scenario_templates.get" => tools::scenario_templates::get_scenario_template(&self.client, args).await,
            "scenario_templates.install" => {
                tools::scenario_templates::install_scenario_template(&self.client, args).await
            }
            "project_resources.list" => tools::project_types::list_project_resources(&self.client, args).await,
            "project_resources.create" => tools::project_types::create_project_resource(&self.client, args).await,
            "project_resources.update" => tools::project_types::update_project_resource(&self.client, args).await,
            "project_resources.delete" => tools::project_types::delete_project_resource(&self.client, args).await,
            "connectors.list" => tools::connectors::list_connectors(&self.client, args).await,
            "connectors.get" => tools::connectors::get_connector(&self.client, args).await,
            "invocations.list" => tools::connectors::list_invocations(&self.client, args).await,
            "invocations.get" => tools::connectors::get_invocation(&self.client, args).await,
            "invocations.create" => tools::connectors::create_invocation(&self.client, args).await,
            "invocations.report_progress" => tools::connectors::report_invocation_progress(&self.client, args).await,
            "invocations.complete" => tools::connectors::complete_invocation(&self.client, args).await,
            "invocations.fail" => tools::connectors::fail_invocation(&self.client, args).await,
            "forms.list" => tools::forms::list_forms(&self.client, args).await,
            "forms.get" => tools::forms::get_form(&self.client, args).await,
            "forms.create" => tools::forms::create_form(&self.client, args).await,
            "forms.create_from_template" => tools::forms::create_form_from_template(&self.client, args).await,
            "forms.update_schema" => tools::forms::update_form_schema(&self.client, args).await,
            "forms.duplicate" => tools::forms::duplicate_form(&self.client, args).await,
            "forms.schema_summary" => tools::forms::get_form_schema_summary(&self.client, args).await,
            "forms.field_usage" => tools::forms::get_form_field_usage(&self.client, args).await,
            "forms.field_dependencies" => tools::forms::get_form_field_dependencies(&self.client, args).await,
            "form_schema_versions.list" => tools::forms::list_form_schema_versions(&self.client, args).await,
            "form_schema_versions.get" => tools::forms::get_form_schema_version(&self.client, args).await,
            "form_permissions.get" => tools::forms::get_form_permissions(&self.client, args).await,
            "form_permissions.update" => tools::forms::update_form_permissions(&self.client, args).await,
            "form_views.list" => tools::forms::list_form_views(&self.client, args).await,
            "form_attachments.list" => tools::forms::list_form_attachments(&self.client, args).await,
            "form_attachments.create" => tools::forms::create_form_attachment(&self.client, args).await,
            "form_attachments.archive" => tools::forms::archive_form_attachment(&self.client, args).await,
            "form_attachments.restore" => tools::forms::restore_form_attachment(&self.client, args).await,
            "form_records.list" => tools::forms::list_form_records(&self.client, args).await,
            "form_records.export" => tools::forms::export_form_records(&self.client, args).await,
            "form_records.import_preview" => tools::forms::preview_import_form_records(&self.client, args).await,
            "form_records.import_commit" => tools::forms::import_form_records(&self.client, args).await,
            "form_records.get" => tools::forms::get_form_record(&self.client, args).await,
            "form_records.create" => tools::forms::create_form_record(&self.client, args).await,
            "form_records.update" => tools::forms::update_form_record(&self.client, args).await,
            "form_records.link" => tools::forms::link_form_record(&self.client, args).await,
            "form_records.relation_targets" => tools::forms::list_form_relation_targets(&self.client, args).await,
            "form_records.children" => tools::forms::list_form_record_children(&self.client, args).await,
            "form_records.child_create" => tools::forms::create_child_form_record(&self.client, args).await,
            "form_records.child_update" => tools::forms::update_child_form_record(&self.client, args).await,
            "form_records.child_archive" => tools::forms::archive_child_form_record(&self.client, args).await,
            "form_records.child_restore" => tools::forms::restore_child_form_record(&self.client, args).await,
            "form_records.aggregate" => tools::forms::aggregate_form_records(&self.client, args).await,
            "events.tail" => tools::forms::events_tail(&self.client, args).await,
            "plugins.list" => tools::plugins::list_plugins(&self.client, args).await,
            "plugins.get" => tools::plugins::get_plugin(&self.client, args).await,
            "plugins.install" => tools::plugins::install_plugin(&self.client, args).await,
            "plugins.invoke" => tools::plugins::invoke_plugin(&self.client, args).await,
            "plugin_invocations.list" => tools::plugins::list_plugin_invocations(&self.client, args).await,
            "context.get_project" => tools::context::get_project_context(&self.client, args).await,
            "context.get_governance" => tools::context::get_governance_context(&self.client, args).await,
            "context.get_agent_policy" => tools::context::get_agent_policy(&self.client, args).await,

            // Work Items
            "work_items.list" => tools::work_items::list_work_items(&self.client, args).await,
            "work_items.get" => tools::work_items::get_work_item(&self.client, args).await,
            "work_items.get_by_identifier" => tools::work_items::get_work_item_by_identifier(&self.client, args).await,
            "work_items.create" => tools::work_items::create_work_item(&self.client, args).await,
            "work_items.update" => tools::work_items::update_work_item(&self.client, args).await,
            "work_items.add_label" => tools::work_items::add_label_to_work_item(&self.client, args).await,
            "work_items.remove_label" => tools::work_items::remove_label_from_work_item(&self.client, args).await,
            "work_items.list_labels" => tools::work_items::list_work_item_labels(&self.client, args).await,
            "work_items.delete" => tools::work_items::handle_delete_work_item(&self.client, args).await,
            "work_items.search" => tools::work_items::search_work_items(&self.client, args).await,
            "work_items.add_labels" => tools::work_items::add_labels_to_work_item(&self.client, args).await,

            // Comments
            "comments.list" => tools::comments::list_comments(&self.client, args).await,
            "comments.create" => tools::comments::create_comment(&self.client, args).await,
            "comments.delete" => tools::comments::handle_delete_comment(&self.client, args).await,
            "files.upload" => tools::files::upload_file(&self.client, args).await,

            // Proposals
            "proposals.list" => tools::proposals::list_proposals(&self.client, args).await,
            "proposals.get" => tools::proposals::get_proposal(&self.client, args).await,
            "proposals.create" => tools::proposals::create_proposal(&self.client, args).await,
            "proposals.create_from_result" => tools::proposals::create_proposal_from_result(&self.client, args).await,
            "check_results.create" => tools::proposals::create_check_result(&self.client, args).await,
            "release.readiness.get" => tools::release::get_release_readiness(&self.client, args).await,

            // Members
            "members.list" => tools::members::list_members(&self.client, args).await,

            // Sprints
            "sprints.create" => tools::sprints::create_sprint(&self.client, args).await,
            "sprints.list" => tools::sprints::list_sprints(&self.client, args).await,
            "sprints.update" => tools::sprints::update_sprint(&self.client, args).await,
            "sprints.delete" => tools::sprints::handle_delete_sprint(&self.client, args).await,

            // Labels
            "labels.create" => tools::labels::create_label(&self.client, args).await,
            "labels.list" => tools::labels::list_labels(&self.client, args).await,
            "labels.list_by_project" => tools::labels::list_project_labels(&self.client, args).await,
            "labels.update" => tools::labels::update_label(&self.client, args).await,
            "labels.delete" => tools::labels::handle_delete_label(&self.client, args).await,

            // Search
            "search.all" => tools::search::search_all(&self.client, args).await,
            "code.resources.list" => tools::scenario_tools::code_resources_list(&self.client, args).await,
            "code.directory.get" => tools::scenario_tools::code_directory_get(&self.client, args).await,
            "code.task_context.get" => tools::scenario_tools::code_task_context_get(&self.client, args).await,
            "code.change_proposal.create" => {
                tools::scenario_tools::code_change_proposal_create(&self.client, args).await
            }
            "documents.extract_summary" => tools::scenario_tools::documents_extract_summary(&self.client, args).await,
            "documents.review_risk" => tools::scenario_tools::documents_review_risk(&self.client, args).await,
            "approval.request" => tools::scenario_tools::approval_request(&self.client, args).await,
            "inspection.report" => tools::scenario_tools::inspection_report(&self.client, args).await,
            "corrective_action.propose" => tools::scenario_tools::corrective_action_propose(&self.client, args).await,

            _ => tools::connectors::invoke_connector_tool(&self.client, name, args).await,
        }
    }

    /// Resolves the project whose agent policy governs this call.
    ///
    /// The scope comes from [`tool_policy_scope`], i.e. from the registered schema, not
    /// from the arguments. A tool addressed by a resource id carries no trustworthy
    /// `project_id`, so the owning project is always read back from the API; a
    /// `project_id` smuggled into the arguments is never the policy subject and any
    /// disagreement with the real owner refuses the call, because a caller naming a
    /// foreign project is itself a bypass attempt. When the owner cannot be resolved the
    /// call is refused as well (fail closed).
    async fn resolve_policy_project_id(&self, tool_name: &str, args: &Value) -> Result<Option<String>, String> {
        match tool_policy_scope(tool_name) {
            PolicyScope::WorkspaceWide => {
                // Nothing this tool touches belongs to a project, so there is no policy
                // to evaluate. Refuse anyway if the call somehow carries an ownership
                // bearing id: that means the classification and the schema disagree, and
                // the safe reading of a disagreement is "not authorized".
                if let Some((argument, _)) = OWNERSHIP_ARGUMENTS
                    .iter()
                    .find(|(argument, _)| args.get(*argument).is_some_and(|value| !value.is_null()))
                {
                    tracing::warn!(
                        tool_name = %tool_name,
                        argument = %argument,
                        "Refusing workspace scoped call that carries a project owned identifier"
                    );
                    return Err(format!(
                        "Tool '{tool_name}' is registered as workspace scoped but was called with '{argument}'; \
                         refusing because the owning project would go unchecked"
                    ));
                }
                Ok(None)
            }
            PolicyScope::DeclaredProject { required } => match declared_project_id(args)? {
                Some(project_id) => Ok(Some(project_id)),
                None if required => {
                    tracing::warn!(
                        tool_name = %tool_name,
                        "Refusing project scoped call: no project_id to evaluate the project agent policy against"
                    );
                    Err(format!(
                        "Tool '{tool_name}' requires a project_id, so the project agent policy cannot be evaluated \
                         without one"
                    ))
                }
                None => Ok(None),
            },
            PolicyScope::OwnedBy(lookup) => {
                let owner = self.resolve_owning_project(tool_name, lookup, args).await?;
                self.reject_foreign_project_claims(tool_name, args, &owner)?;
                Ok(Some(owner))
            }
        }
    }

    /// Reads the owning project of an id addressed call back from the API.
    async fn resolve_owning_project(
        &self,
        tool_name: &str,
        lookup: OwnerLookup,
        args: &Value,
    ) -> Result<String, String> {
        match lookup {
            OwnerLookup::FormData => self.resolve_form_data_owner(tool_name, args).await,
            OwnerLookup::Sprint | OwnerLookup::Comment => {
                let id = required_owner_argument(tool_name, lookup, args)?;
                let endpoint = match lookup {
                    OwnerLookup::Sprint => "GET /api/v1/sprints/{sprint_id}",
                    _ => "GET /api/v1/comments/{comment_id}",
                };
                tracing::warn!(
                    tool_name = %tool_name,
                    resource_id = %id,
                    "Refusing call: the API exposes no read endpoint to resolve the owning project"
                );
                Err(format!(
                    "Tool '{tool_name}' is refused because the owning project of {} {id} cannot be resolved: the API \
                     exposes no `{endpoint}` returning project_id, so the project agent policy cannot be evaluated",
                    lookup.argument()
                ))
            }
            OwnerLookup::WorkItemIdentifier => {
                let identifier = identifier_argument(args)?.ok_or_else(|| {
                    format!("Tool '{tool_name}' carries no identifier, so the owning project cannot be resolved")
                })?;
                let cache_key = format!("identifier:{}", identifier.to_ascii_lowercase());
                let description = format!("work item {identifier}");
                if let Some(owner) = self.cached_project_id(&cache_key).await {
                    return Ok(owner.project_id);
                }
                let response = self
                    .client
                    .get_work_item_by_identifier(&identifier)
                    .await
                    .map_err(|error| format!("Failed to resolve owning project for {description}: {error}"))?;
                self.store_owner(cache_key, &description, &response)
                    .await
                    .map(|owner| owner.project_id)
            }
            _ => {
                let id = required_owner_argument(tool_name, lookup, args)?;
                let (prefix, kind) = match lookup {
                    OwnerLookup::WorkItem => ("work_item", "work item"),
                    OwnerLookup::Plugin => ("plugin", "plugin"),
                    OwnerLookup::Invocation => ("invocation", "invocation"),
                    OwnerLookup::Connector => ("connector", "connector"),
                    _ => ("check_result", "check result"),
                };
                let cache_key = format!("{prefix}:{id}");
                let description = format!("{kind} {id}");
                if let Some(owner) = self.cached_project_id(&cache_key).await {
                    return Ok(owner.project_id);
                }
                let response = match lookup {
                    OwnerLookup::WorkItem => self.client.get_work_item(&id).await,
                    OwnerLookup::Plugin => self.client.get_plugin(&id).await,
                    OwnerLookup::Invocation => self.client.get_invocation(&id).await,
                    OwnerLookup::Connector => self.client.get_connector(&id).await,
                    _ => self.client.get_check_result(&id).await,
                }
                .map_err(|error| format!("Failed to resolve owning project for {description}: {error}"))?;
                self.store_owner(cache_key, &description, &response)
                    .await
                    .map(|owner| owner.project_id)
            }
        }
    }

    /// Resolves the owner of a form, record or attachment. When the caller names both a
    /// record and a form, the form claim is checked against the record's real form.
    async fn resolve_form_data_owner(&self, tool_name: &str, args: &Value) -> Result<String, String> {
        let Some((target, declared_form_id)) = form_scope_target(args)? else {
            tracing::warn!(
                tool_name = %tool_name,
                "Refusing form scoped call: no form_id, record_id or attachment_id to resolve the owning project"
            );
            return Err(format!(
                "Tool '{tool_name}' carries no form_id, record_id or attachment_id, so the owning project cannot be \
                 resolved and the project agent policy cannot be evaluated"
            ));
        };

        let owner = self.lookup_form_data_owner(tool_name, &target).await?;
        if let Some(declared_form_id) = declared_form_id {
            match owner.form_id.as_deref() {
                Some(actual) if actual.eq_ignore_ascii_case(&declared_form_id) => {}
                Some(actual) => {
                    tracing::warn!(
                        tool_name = %tool_name,
                        declared_form_id = %declared_form_id,
                        owning_form_id = %actual,
                        target = %target.describe(),
                        "Refusing call: caller supplied a form_id that does not own the record"
                    );
                    return Err(format!(
                        "Tool '{tool_name}' was called with form_id '{declared_form_id}', but {} belongs to form \
                         '{actual}'; refusing to authorize a call that names a foreign form",
                        target.describe()
                    ));
                }
                None => {
                    return Err(format!(
                        "Tool '{tool_name}' was called with form_id '{declared_form_id}', but the API response for {} \
                         reports no form_id to check it against",
                        target.describe()
                    ));
                }
            }
        }
        Ok(owner.project_id)
    }

    /// Looks up the owning project of a form scoped target. Attachments have no read
    /// endpoint on the API today, so they fail closed with an actionable message
    /// instead of silently skipping the policy.
    async fn lookup_form_data_owner(&self, tool_name: &str, target: &FormScopeTarget) -> Result<ResolvedOwner, String> {
        let resolved = self.lookup_project_id(target).await;
        match (target, resolved) {
            (_, Ok(owner)) => Ok(owner),
            (FormScopeTarget::Attachment(id), Err(error)) => {
                tracing::warn!(
                    tool_name = %tool_name,
                    attachment_id = %id,
                    error = %error,
                    "Refusing attachment tool call: owning project cannot be resolved"
                );
                Err(format!(
                    "Tool '{tool_name}' is refused because the owning project of attachment {id} cannot be resolved \
                     ({error}); the API has to expose GET /api/v1/form-attachments/{{attachment_id}} returning \
                     project_id or form_id before attachment tools can be authorized"
                ))
            }
            (_, Err(error)) => Err(error),
        }
    }

    async fn lookup_project_id(&self, target: &FormScopeTarget) -> Result<ResolvedOwner, String> {
        let cache_key = target.cache_key();
        if let Some(owner) = self.cached_project_id(&cache_key).await {
            return Ok(owner);
        }

        let response = match target {
            FormScopeTarget::Form(id) => self.client.get_form(id).await,
            FormScopeTarget::Record(id) => self.client.get_form_record(id).await,
            FormScopeTarget::Attachment(id) => self.client.get_form_attachment(id).await,
        }
        .map_err(|error| format!("Failed to resolve owning project for {}: {error}", target.describe()))?;

        let data = response.get("data");
        let form_id = data
            .and_then(|data| data.get("form_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let project_id = if let Some(project_id) = data.and_then(|data| data.get("project_id")).and_then(Value::as_str)
        {
            project_id.to_string()
        } else {
            // Payloads that only identify the owning form (attachments) are resolved
            // one level further instead of being treated as unresolvable.
            let form_id = form_id.as_deref().ok_or_else(|| {
                format!(
                    "API response for {} carries neither project_id nor form_id",
                    target.describe()
                )
            })?;
            self.fetch_form_project_id(form_id).await?
        };

        let owner = ResolvedOwner { project_id, form_id };
        self.cache_project_id(cache_key, &owner).await;
        Ok(owner)
    }

    async fn fetch_form_project_id(&self, form_id: &str) -> Result<String, String> {
        let target = FormScopeTarget::Form(form_id.to_string());
        let cache_key = target.cache_key();
        let description = target.describe();
        if let Some(owner) = self.cached_project_id(&cache_key).await {
            return Ok(owner.project_id);
        }
        let response = self
            .client
            .get_form(form_id)
            .await
            .map_err(|error| format!("Failed to resolve owning project for {description}: {error}"))?;
        self.store_owner(cache_key, &description, &response)
            .await
            .map(|owner| owner.project_id)
    }

    /// Reads the ownership fields out of an API payload and caches them. An answer that
    /// carries no `project_id` is an error, never a silent "no policy applies".
    async fn store_owner(
        &self,
        cache_key: String,
        description: &str,
        response: &Value,
    ) -> Result<ResolvedOwner, String> {
        let data = response.get("data");
        let project_id = data
            .and_then(|data| data.get("project_id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("API response for {description} carries no project_id"))?;
        let owner = ResolvedOwner {
            project_id,
            form_id: data
                .and_then(|data| data.get("form_id"))
                .and_then(Value::as_str)
                .map(str::to_string),
        };

        self.cache_project_id(cache_key, &owner).await;
        Ok(owner)
    }

    /// Refuses a call that names any project other than the resolved owner, wherever the
    /// claim is hidden (`project_id`, `projectId`, `payload.project_id`, ...).
    #[allow(clippy::unused_self)]
    fn reject_foreign_project_claims(&self, tool_name: &str, args: &Value, owner: &str) -> Result<(), String> {
        for claimed in claimed_project_ids(args) {
            if !claimed.eq_ignore_ascii_case(owner) {
                let claimed = sanitize_for_error(&claimed);
                tracing::warn!(
                    tool_name = %tool_name,
                    declared_project_id = %claimed,
                    owning_project_id = %owner,
                    "Refusing call: caller supplied a project_id that does not own the target"
                );
                return Err(format!(
                    "Tool '{tool_name}' was called with project_id '{claimed}', but the target belongs to project \
                     '{owner}'; refusing to evaluate the project agent policy against a foreign project"
                ));
            }
        }
        Ok(())
    }

    async fn cached_project_id(&self, cache_key: &str) -> Option<ResolvedOwner> {
        let cache = self.project_id_cache.lock().await;
        cache.get(cache_key).cloned()
    }

    async fn cache_project_id(&self, cache_key: String, owner: &ResolvedOwner) {
        let mut cache = self.project_id_cache.lock().await;
        if cache.len() >= PROJECT_ID_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(cache_key, owner.clone());
    }

    async fn enforce_project_tool_policy(&self, tool_name: &str, args: &Value) -> Result<(), String> {
        let Some(project_id) = self.resolve_policy_project_id(tool_name, args).await? else {
            return Ok(());
        };

        let policy = self
            .client
            .get_project_agent_policy(&project_id)
            .await
            .map_err(|error| format!("Failed to load project agent policy for tools/call: {error}"))?;
        // The policy subject is a validated UUID and the client rejects anything that is
        // not a `{code, message, data}` envelope, so a response that reached this point
        // really is an agent policy. Requiring the `data` object as well keeps the gate
        // fail closed if either guarantee is ever weakened.
        if !policy.get("data").is_some_and(Value::is_object) {
            return Err(format!(
                "Refusing '{tool_name}': the agent-policy response for project {project_id} carries no policy object"
            ));
        }
        if is_tool_enabled_by_policy(&policy, tool_name) {
            Ok(())
        } else {
            Err(format!(
                "Tool '{tool_name}' is disabled by project agent policy for project {project_id}"
            ))
        }
    }

    async fn report_tool_call_audit(
        &self,
        tool_name: &str,
        arguments: Value,
        result: &CallToolResult,
        duration_ms: u128,
    ) {
        let Some(invocation_id) = self.client.invocation_id().map(str::to_string) else {
            return;
        };

        let is_error = result.is_error.unwrap_or(false);
        let payload = json!({
            "tool_name": tool_name,
            "transport": std::env::var("OPENPR_MCP_TRANSPORT").unwrap_or_else(|_| "mcp_stdio".to_string()),
            "status": if is_error { "failed" } else { "succeeded" },
            "arguments": arguments,
            "result_summary": summarize_tool_result(result),
            "error_message": if is_error { summarize_tool_result(result) } else { None },
            "duration_ms": i64::try_from(duration_ms).unwrap_or(i64::MAX)
        });

        if let Err(error) = self.client.report_invocation_tool_call(&invocation_id, payload).await {
            tracing::warn!(
                invocation_id = %invocation_id,
                tool_name = %tool_name,
                error = %error,
                "Failed to report MCP tool-call audit"
            );
        }
    }

    #[allow(clippy::unused_self)]
    fn handle_initialize(&self, id: Option<Value>) -> JsonRpcResponse {
        let result = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {
                    "listChanged": false
                },
                "resources": {
                    "subscribe": false,
                    "listChanged": false
                },
                "resourceTemplates": {
                    "listChanged": false
                }
            },
            "serverInfo": {
                "name": "openpr-mcp-server",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        JsonRpcResponse::success(id, result)
    }

    #[allow(clippy::unused_self)]
    fn handle_resources_list(&self, id: Option<Value>) -> JsonRpcResponse {
        let resources = vec![
            json!({
                "uri": "openpr://skills/openpr-mcp",
                "name": "OpenPR MCP Skill Guide",
                "description": "Complete guide for using OpenPR MCP tools: workflow patterns, field reference, and templates.",
                "mimeType": "text/markdown"
            }),
            json!({
                "uri": "openpr://guides/agents",
                "name": "OpenPR Agent Development Guide",
                "description": "Repository guidelines, build commands, coding style, and testing procedures.",
                "mimeType": "text/markdown"
            }),
            json!({
                "uri": "openpr://guides/workflows",
                "name": "Common Workflow Patterns",
                "description": "Bug report, sprint planning, code review, and triage workflow templates.",
                "mimeType": "text/markdown"
            }),
            json!({
                "uri": "openpr://scenario-templates",
                "name": "Scenario Templates",
                "description": "Project creation templates with workflow, fields, resources, AI roles, governance, and connection suggestions.",
                "mimeType": "application/json"
            }),
        ];

        JsonRpcResponse::success(id, json!({ "resources": resources }))
    }

    #[allow(clippy::unused_self)]
    fn handle_resources_templates_list(&self, id: Option<Value>) -> JsonRpcResponse {
        let templates = vec![
            json!({
                "uriTemplate": "openpr://projects/{project_id}/issues",
                "name": "Project Issues",
                "description": "List issues for a specific project",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://projects/{project_id}/forms",
                "name": "Project Forms",
                "description": "List universal forms for a specific project",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://forms/{form_id}",
                "name": "Form Schema",
                "description": "Read one universal form schema and display metadata",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://forms/{form_id}/records",
                "name": "Form Records",
                "description": "List records for one universal form",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://forms/{form_id}/events",
                "name": "Form Events",
                "description": "Read recent business events for one universal form",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://form-records/{record_id}",
                "name": "Form Record",
                "description": "Read one universal form record",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://form-records/{record_id}/events",
                "name": "Form Record Events",
                "description": "Read recent business events for one universal form record",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://scenario-templates/{key}",
                "name": "Scenario Template",
                "description": "Read one project creation scenario template by key",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://projects/{project_id}/context",
                "name": "Project Context",
                "description": "Read project type, resources, connectors, governance, workflow, and policy in one context payload",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://projects/{project_id}/governance",
                "name": "Project Governance",
                "description": "Read governance, workflow, and recent decision context for a project",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://projects/{project_id}/agent-policy",
                "name": "Project Agent Policy",
                "description": "Read effective MCP/AI action policy for a project",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://projects/{project_id}/release-readiness",
                "name": "Project Release Readiness",
                "description": "Read acceptance gates, blockers, and release readiness evidence for a project",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://projects/{project_id}/type",
                "name": "Project Type",
                "description": "Read project type, settings, and scenario metadata for a project",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://projects/{project_id}/resources",
                "name": "Project Resources",
                "description": "Read project resources such as repositories, directories, documents, equipment, or business records",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://projects/{project_id}/connectors",
                "name": "Project Connectors",
                "description": "Read project automation connectors such as webhook, MCP, REST, CLI, or OpenPRX tunnel",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://projects/{project_id}/invocations",
                "name": "Project Invocations",
                "description": "Read AI execution invocation ledger entries for a project",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://projects/{project_id}/recent-decisions",
                "name": "Project Recent Decisions",
                "description": "Read recent decision context available to project agents",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://projects/{project_id}/sprints",
                "name": "Project Sprints",
                "description": "List sprints for a specific project",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "openpr://issues/{identifier}",
                "name": "Issue by Identifier",
                "description": "Get issue details by human-readable identifier (e.g. PRX-42)",
                "mimeType": "application/json"
            }),
        ];

        JsonRpcResponse::success(id, json!({ "resourceTemplates": templates }))
    }

    async fn handle_resources_read(&self, id: Option<Value>, params: Option<Value>) -> JsonRpcResponse {
        let uri = match params
            .as_ref()
            .and_then(|value| value.get("uri"))
            .and_then(Value::as_str)
        {
            Some(uri) if !uri.is_empty() => uri.to_string(),
            _ => {
                return JsonRpcResponse::error(id, JsonRpcError::invalid_params("Missing required field: uri"));
            }
        };

        match uri.as_str() {
            "openpr://skills/openpr-mcp" => {
                return JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "text/markdown",
                            "text": SKILL_GUIDE_MD
                        }]
                    }),
                );
            }
            "openpr://guides/agents" => {
                return JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "text/markdown",
                            "text": AGENTS_GUIDE_MD
                        }]
                    }),
                );
            }
            "openpr://guides/workflows" => {
                return JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "text/markdown",
                            "text": WORKFLOWS_GUIDE_MD
                        }]
                    }),
                );
            }
            "openpr://scenario-templates" => {
                return match self.client.list_scenario_templates(None, None).await {
                    Ok(templates) => JsonRpcResponse::success(
                        id,
                        json!({
                            "contents": [{
                                "uri": uri,
                                "mimeType": "application/json",
                                "text": templates.to_string()
                            }]
                        }),
                    ),
                    Err(error) => JsonRpcResponse::error(
                        id,
                        JsonRpcError::internal_error(format!("Failed to read scenario templates: {error}")),
                    ),
                };
            }
            _ => {}
        }

        if let Some(key) = parse_scenario_template_uri(&uri) {
            return match self.client.get_scenario_template(&key).await {
                Ok(template) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": template.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read scenario template: {error}")),
                ),
            };
        }

        if let Some(project_id) = parse_project_resource_uri(&uri, "/issues") {
            return match self
                .client
                .list_work_items(
                    project_id,
                    &ListWorkItemsQuery {
                        page: Some(1),
                        per_page: Some(50),
                        ..ListWorkItemsQuery::default()
                    },
                )
                .await
            {
                Ok(issues) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": issues.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read project issues resource: {error}")),
                ),
            };
        }

        if let Some(project_id) = parse_project_resource_uri(&uri, "/forms") {
            return match self.client.list_forms(project_id, None, None).await {
                Ok(forms) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": forms.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read project forms resource: {error}")),
                ),
            };
        }

        if let Some(form_id) = parse_form_resource_uri(&uri, "") {
            return match self.client.get_form(form_id).await {
                Ok(form) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": form.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read form resource: {error}")),
                ),
            };
        }

        if let Some(form_id) = parse_form_resource_uri(&uri, "/records") {
            return match self
                .client
                .list_form_records(form_id, &ListRecordsQuery::default())
                .await
            {
                Ok(records) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": records.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read form records resource: {error}")),
                ),
            };
        }

        if let Some(form_id) = parse_form_resource_uri(&uri, "/events") {
            return match self.client.list_form_events(form_id, None, None, None).await {
                Ok(events) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": events.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read form events resource: {error}")),
                ),
            };
        }

        if let Some(record_id) = parse_form_record_resource_uri(&uri, "") {
            return match self.client.get_form_record(record_id).await {
                Ok(record) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": record.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read form record resource: {error}")),
                ),
            };
        }

        if let Some(record_id) = parse_form_record_resource_uri(&uri, "/events") {
            return match self.client.list_form_record_events(record_id, None, None, None).await {
                Ok(events) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": events.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read form record events resource: {error}")),
                ),
            };
        }

        if let Some(project_id) = parse_project_resource_uri(&uri, "/context") {
            return match self.client.get_project_context(project_id).await {
                Ok(context) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": context.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read project context: {error}")),
                ),
            };
        }

        if let Some(project_id) = parse_project_resource_uri(&uri, "/governance") {
            return match self.client.get_project_governance_context(project_id).await {
                Ok(governance) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": governance.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read project governance: {error}")),
                ),
            };
        }

        if let Some(project_id) = parse_project_resource_uri(&uri, "/agent-policy") {
            return match self.client.get_project_agent_policy(project_id).await {
                Ok(policy) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": policy.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read project agent policy: {error}")),
                ),
            };
        }

        if let Some(project_id) = parse_project_resource_uri(&uri, "/release-readiness") {
            return match self.client.get_project_release_readiness(project_id).await {
                Ok(readiness) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": readiness.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read project release readiness: {error}")),
                ),
            };
        }

        if let Some(project_id) = parse_project_resource_uri(&uri, "/type") {
            return match self.client.get_project(project_id).await {
                Ok(project) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": project.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read project type resource: {error}")),
                ),
            };
        }

        if let Some(project_id) = parse_project_resource_uri(&uri, "/resources") {
            return match self.client.list_project_resources(project_id).await {
                Ok(resources) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": resources.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read project resources: {error}")),
                ),
            };
        }

        if let Some(project_id) = parse_project_resource_uri(&uri, "/connectors") {
            return match self.client.list_connectors(project_id, None).await {
                Ok(connectors) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": connectors.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read project connectors: {error}")),
                ),
            };
        }

        if let Some(project_id) = parse_project_resource_uri(&uri, "/invocations") {
            return match self.client.list_invocations(project_id, None, None, None).await {
                Ok(invocations) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": invocations.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read project invocations: {error}")),
                ),
            };
        }

        if let Some(project_id) = parse_project_resource_uri(&uri, "/recent-decisions") {
            return match self.client.get_project_governance_context(project_id).await {
                Ok(governance) => {
                    let recent_decisions = governance
                        .get("data")
                        .and_then(|data| data.get("recent_decisions"))
                        .cloned()
                        .unwrap_or_else(|| json!([]));
                    JsonRpcResponse::success(
                        id,
                        json!({
                            "contents": [{
                                "uri": uri,
                                "mimeType": "application/json",
                                "text": recent_decisions.to_string()
                            }]
                        }),
                    )
                }
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read project recent decisions: {error}")),
                ),
            };
        }

        if let Some(project_id) = parse_project_resource_uri(&uri, "/sprints") {
            return match self.client.list_sprints(project_id).await {
                Ok(sprints) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": sprints.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read project sprints resource: {error}")),
                ),
            };
        }

        if let Some(identifier) = parse_issue_identifier_uri(&uri) {
            return match self.client.get_work_item_by_identifier(&identifier).await {
                Ok(issue) => JsonRpcResponse::success(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": issue.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to read issue resource: {error}")),
                ),
            };
        }

        JsonRpcResponse::error(id, JsonRpcError::invalid_params(format!("Unknown resource URI: {uri}")))
    }
}

fn parse_project_resource_uri<'a>(uri: &'a str, suffix: &str) -> Option<&'a str> {
    let project_id = uri.strip_prefix("openpr://projects/")?.strip_suffix(suffix)?;
    if project_id.is_empty() || project_id.contains('/') {
        return None;
    }
    Some(project_id)
}

fn parse_form_resource_uri<'a>(uri: &'a str, suffix: &str) -> Option<&'a str> {
    let form_id = uri.strip_prefix("openpr://forms/")?.strip_suffix(suffix)?;
    if form_id.is_empty() || form_id.contains('/') {
        return None;
    }
    Some(form_id)
}

fn parse_form_record_resource_uri<'a>(uri: &'a str, suffix: &str) -> Option<&'a str> {
    let record_id = uri.strip_prefix("openpr://form-records/")?.strip_suffix(suffix)?;
    if record_id.is_empty() || record_id.contains('/') {
        return None;
    }
    Some(record_id)
}

fn parse_issue_identifier_uri(uri: &str) -> Option<String> {
    let identifier = uri.strip_prefix("openpr://issues/")?;
    if identifier.is_empty() || identifier.contains('/') {
        return None;
    }

    Some(identifier.to_string())
}

fn parse_scenario_template_uri(uri: &str) -> Option<String> {
    let key = uri.strip_prefix("openpr://scenario-templates/")?;
    if key.is_empty() || key.contains('/') {
        return None;
    }

    Some(key.to_string())
}

/// The project whose policy filters `tools/list`. Validated as a UUID for the same
/// reason the `tools/call` gate is: the value is interpolated into the policy URL.
fn extract_tools_list_project_id(params: Option<&Value>) -> Result<Option<String>, String> {
    let Some(params) = params else {
        return Ok(None);
    };
    let Some(raw) = params
        .get("project_id")
        .or_else(|| params.get("projectId"))
        .or_else(|| params.get("context").and_then(|context| context.get("project_id")))
        .or_else(|| params.get("context").and_then(|context| context.get("projectId")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    canonical_uuid(raw).map(Some).ok_or_else(|| {
        format!(
            "Invalid params: project_id '{}' is not a canonical UUID",
            sanitize_for_error(raw)
        )
    })
}

fn string_argument(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Parses the canonical UUID text the API uses for every primary key
/// (`8-4-4-4-12` hex with hyphens; the API itself binds these as `Path<Uuid>`).
///
/// Everything else is rejected: path traversal (`../`), percent escapes, query or
/// fragment smuggling (`?`, `#`), embedded slashes, blank strings, over-long input and
/// non-hex characters. This is what stops a caller supplied id from reshaping the URL
/// it is interpolated into — `"<uuid>/work-items?pad="` used to turn the policy lookup
/// `GET /api/v1/projects/{id}/agent-policy` into a completely different, always
/// permissive request. The result is lower-cased so that two spellings of the same id
/// compare equal.
fn canonical_uuid(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() != 36 {
        return None;
    }
    for (index, byte) in value.bytes().enumerate() {
        let accepted = match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        };
        if !accepted {
            return None;
        }
    }
    Some(value.to_ascii_lowercase())
}

/// Trims attacker controlled text before it is echoed into an error message, so a
/// hostile id cannot inject newlines or flood the transcript.
fn sanitize_for_error(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|character| !character.is_control())
        .take(64)
        .collect();
    if value.chars().filter(|c| !c.is_control()).count() > 64 {
        format!("{cleaned}...")
    } else {
        cleaned
    }
}

/// Reads a UUID argument. A present-but-invalid value is an error, never a silently
/// missing one: treating it as absent would drop the call back to "no policy applies".
fn uuid_argument(args: &Value, key: &str) -> Result<Option<String>, String> {
    let Some(raw) = args.get(key) else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let text = raw
        .as_str()
        .ok_or_else(|| format!("Invalid input: {key} must be a string carrying a UUID"))?;
    canonical_uuid(text).map(Some).ok_or_else(|| {
        format!(
            "Invalid input: {key} '{}' is not a canonical UUID",
            sanitize_for_error(text)
        )
    })
}

/// Human readable work item identifiers (`PRX-42`) are not UUIDs, so they get their own
/// conservative character allowlist before they are used to address the API.
fn identifier_argument(args: &Value) -> Result<Option<String>, String> {
    let Some(identifier) = string_argument(args, "identifier") else {
        return Ok(None);
    };
    let accepted = identifier.len() <= 64
        && identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if accepted {
        Ok(Some(identifier))
    } else {
        Err(format!(
            "Invalid input: identifier '{}' is not a work item identifier",
            sanitize_for_error(&identifier)
        ))
    }
}

fn required_owner_argument(tool_name: &str, lookup: OwnerLookup, args: &Value) -> Result<String, String> {
    let key = lookup.argument();
    uuid_argument(args, key)?.ok_or_else(|| {
        format!(
            "Tool '{tool_name}' carries no {key}, so the owning project cannot be resolved and the project agent \
             policy cannot be evaluated"
        )
    })
}

/// The `project_id` a declared-project call is authorized against. Only the top level
/// argument counts, because that is the one the tool implementations actually send to
/// the API; a nested `payload.project_id` is caller payload, not a routing decision.
fn declared_project_id(args: &Value) -> Result<Option<String>, String> {
    if let Some(project_id) = uuid_argument(args, "project_id")? {
        return Ok(Some(project_id));
    }
    uuid_argument(args, "projectId")
}

/// Every place a caller can claim a project, used to refuse id addressed calls that
/// name someone other than the resolved owner.
fn claimed_project_ids(args: &Value) -> Vec<String> {
    ["project_id", "projectId"]
        .iter()
        .filter_map(|key| args.get(*key))
        .chain(
            args.get("payload")
                .into_iter()
                .flat_map(|payload| ["project_id", "projectId"].map(|key| payload.get(key)))
                .flatten(),
        )
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn is_tool_enabled_by_policy(policy_response: &Value, tool_name: &str) -> bool {
    tools::capabilities::extract_enabled_tool_names(policy_response).is_none_or(|enabled| enabled.contains(tool_name))
}

fn summarize_tool_result(result: &CallToolResult) -> Option<String> {
    let text = result.content.iter().find_map(|item| match item {
        crate::protocol::ToolContent::Text { text } => Some(text.as_str()),
        _ => None,
    })?;
    if text.chars().count() > 2_000 {
        Some(text.chars().take(2_000).collect())
    } else {
        Some(text.to_string())
    }
}

fn redact_tool_arguments(args: &Value) -> Value {
    match args {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    let redacted = if lower.contains("token")
                        || lower.contains("secret")
                        || lower.contains("password")
                        || lower.contains("authorization")
                    {
                        Value::String("[REDACTED]".to_string())
                    } else {
                        redact_tool_arguments(value)
                    };
                    (key.clone(), redacted)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact_tool_arguments).collect()),
        _ => args.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AGENTS_GUIDE_MD, OWNERSHIP_ARGUMENTS, PolicyScope, SKILL_GUIDE_MD, TOOL_POLICY_SCOPES, canonical_uuid,
        claimed_project_ids, declared_project_id, extract_tools_list_project_id, is_tool_enabled_by_policy,
        redact_tool_arguments, summarize_tool_result, tool_policy_scope,
    };
    use crate::protocol::{CallToolResult, ToolContent};
    use axum::{
        Json, Router,
        routing::{delete, get, post},
    };
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    // Canonical UUIDs: every id the policy gate touches now has to be one.
    const PROJECT: &str = "11111111-1111-4111-8111-111111111111";
    const FOREIGN_PROJECT: &str = "99999999-9999-4999-8999-999999999999";
    const FORM: &str = "22222222-2222-4222-8222-222222222222";
    const OTHER_FORM: &str = "2b2b2b2b-2b2b-4b2b-8b2b-2b2b2b2b2b2b";
    const RECORD: &str = "33333333-3333-4333-8333-333333333333";
    const ATTACHMENT: &str = "44444444-4444-4444-8444-444444444444";
    const WORK_ITEM: &str = "55555555-5555-4555-8555-555555555555";
    const LABEL: &str = "66666666-6666-4666-8666-666666666666";
    const SPRINT: &str = "77777777-7777-4777-8777-777777777777";
    const COMMENT: &str = "88888888-8888-4888-8888-888888888888";
    const RESOURCE: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const PLUGIN: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

    fn server(base_url: String) -> Result<super::McpServer, String> {
        Ok(super::McpServer::new(crate::client::OpenPrClient::new(
            base_url,
            "opr_test_token".to_string(),
            "workspace-1".to_string(),
        )?))
    }

    fn policy_route(enabled_tools: Value) -> axum::routing::MethodRouter {
        get(move || {
            let enabled_tools = enabled_tools.clone();
            async move {
                Json(json!({
                    "code": 0,
                    "data": { "mcp": { "tool_registry": { "enabled_tools": enabled_tools } } }
                }))
            }
        })
    }

    fn message(result: &CallToolResult) -> String {
        summarize_tool_result(result).unwrap_or_default()
    }

    #[test]
    fn tools_list_project_id_accepts_top_level_and_context_params() -> TestResult {
        assert_eq!(
            extract_tools_list_project_id(Some(&json!({ "project_id": PROJECT })))?.as_deref(),
            Some(PROJECT)
        );
        assert_eq!(
            extract_tools_list_project_id(Some(&json!({ "context": { "projectId": PROJECT } })))?.as_deref(),
            Some(PROJECT)
        );
        assert_eq!(
            extract_tools_list_project_id(Some(&json!({ "project_id": " " })))?,
            None
        );
        assert!(extract_tools_list_project_id(Some(&json!({ "project_id": "p1" }))).is_err());
        Ok(())
    }

    #[test]
    fn embedded_skill_guide_matches_registered_universal_tool_surface() {
        assert!(SKILL_GUIDE_MD.contains("## Tools (105)"));
        assert!(SKILL_GUIDE_MD.contains("scenario_templates.install"));
        assert!(SKILL_GUIDE_MD.contains("forms.list"));
        assert!(SKILL_GUIDE_MD.contains("forms.create_from_template"));
        assert!(SKILL_GUIDE_MD.contains("forms.duplicate"));
        assert!(SKILL_GUIDE_MD.contains("forms.schema_summary"));
        assert!(SKILL_GUIDE_MD.contains("form_schema_versions.list"));
        assert!(SKILL_GUIDE_MD.contains("form_schema_versions.get"));
        assert!(SKILL_GUIDE_MD.contains("form_permissions.get"));
        assert!(SKILL_GUIDE_MD.contains("form_attachments.create"));
        assert!(SKILL_GUIDE_MD.contains("form_records.export"));
        assert!(SKILL_GUIDE_MD.contains("form_records.import_commit"));
        assert!(SKILL_GUIDE_MD.contains("form_records.relation_targets"));
        assert!(SKILL_GUIDE_MD.contains("form_records.children"));
        assert!(SKILL_GUIDE_MD.contains("form_records.child_create"));
        assert!(SKILL_GUIDE_MD.contains("form_records.child_update"));
        assert!(SKILL_GUIDE_MD.contains("form_records.child_archive"));
        assert!(SKILL_GUIDE_MD.contains("form_records.child_restore"));
        assert!(SKILL_GUIDE_MD.contains("form_records.aggregate"));
        assert!(SKILL_GUIDE_MD.contains("plugins.install"));
        assert!(SKILL_GUIDE_MD.contains("plugin_invocations.list"));
        assert!(SKILL_GUIDE_MD.contains("decimal/amount form fields: send decimal strings"));
        assert!(!SKILL_GUIDE_MD.contains("Tools (65)"));
        assert!(AGENTS_GUIDE_MD.contains("cargo build --release --bin mcp-server"));
    }

    #[test]
    fn declared_project_id_only_trusts_the_top_level_argument() -> TestResult {
        assert_eq!(
            declared_project_id(&json!({ "project_id": PROJECT }))?.as_deref(),
            Some(PROJECT)
        );
        assert_eq!(
            declared_project_id(&json!({ "projectId": PROJECT }))?.as_deref(),
            Some(PROJECT)
        );
        assert_eq!(declared_project_id(&json!({ "work_item_id": WORK_ITEM }))?, None);
        // A nested payload is caller data, never a routing decision.
        assert_eq!(
            declared_project_id(&json!({ "payload": { "project_id": PROJECT } }))?,
            None
        );
        Ok(())
    }

    /// Every shape the audit called out has to be refused, not silently normalised into
    /// "no project declared", because that would drop the call back to "no policy".
    #[test]
    fn declared_project_id_rejects_every_smuggling_shape() {
        for hostile in [
            json!({ "project_id": format!("{PROJECT}/work-items?pad=") }),
            json!({ "project_id": format!("{PROJECT}?") }),
            json!({ "project_id": format!("{PROJECT}#") }),
            json!({ "project_id": format!("../../{PROJECT}") }),
            json!({ "project_id": format!("{PROJECT}%2F..%2Fx") }),
            json!({ "project_id": format!("{PROJECT}\r\nX-Injected: 1") }),
            json!({ "project_id": "" }),
            json!({ "project_id": "   " }),
            json!({ "project_id": "not-a-uuid" }),
            json!({ "project_id": "1111111111114111811111111111111" }),
            json!({ "project_id": format!("{PROJECT}{PROJECT}") }),
            json!({ "project_id": 42 }),
            json!({ "project_id": ["11111111-1111-4111-8111-111111111111"] }),
            json!({ "project_id": "gggggggg-1111-4111-8111-111111111111" }),
        ] {
            assert!(
                declared_project_id(&hostile).is_err(),
                "{hostile} was accepted as a project id"
            );
        }
    }

    #[test]
    fn canonical_uuid_normalises_case_and_surrounding_space() {
        assert_eq!(
            canonical_uuid("  11111111-1111-4111-8111-11111111111A  ").as_deref(),
            Some("11111111-1111-4111-8111-11111111111a")
        );
        assert_eq!(canonical_uuid("11111111_1111_4111_8111_111111111111"), None);
    }

    #[test]
    fn claimed_project_ids_sees_every_hiding_place() {
        let claims = claimed_project_ids(&json!({
            "project_id": "a",
            "projectId": "b",
            "payload": { "project_id": "c", "projectId": "d" }
        }));
        assert_eq!(claims, vec!["a", "b", "c", "d"]);
        assert!(claimed_project_ids(&json!({ "record_id": RECORD })).is_empty());
    }

    #[test]
    fn project_tool_policy_denies_disabled_tool_names() {
        let policy = json!({
            "code": 0,
            "data": {
                "mcp": {
                    "tool_registry": {
                        "enabled_tools": ["context.get_project", "work_items.list"]
                    }
                }
            }
        });

        assert!(is_tool_enabled_by_policy(&policy, "context.get_project"));
        assert!(!is_tool_enabled_by_policy(&policy, "sprints.create"));
    }

    #[test]
    fn tool_call_audit_redacts_sensitive_arguments() {
        let redacted = redact_tool_arguments(&json!({
            "content": "ok",
            "nested": { "bot_token": "secret" },
            "items": [{ "password": "pw" }]
        }));

        assert_eq!(redacted.get("content").and_then(Value::as_str), Some("ok"));
        assert_eq!(
            redacted
                .get("nested")
                .and_then(|nested| nested.get("bot_token"))
                .and_then(Value::as_str),
            Some("[REDACTED]")
        );
        assert_eq!(
            redacted
                .get("items")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("password"))
                .and_then(Value::as_str),
            Some("[REDACTED]")
        );
    }

    /// The coverage gate. It re-derives the scope of every tool from the *live* registry
    /// (`tools::get_all_tool_definitions`), so it sees exactly the tools that are served
    /// and cannot be fooled by naming: a name based filter never saw `events.tail`, which
    /// is how that tool stayed unguarded while its guard test was green.
    #[test]
    fn tool_policy_scopes_match_the_registered_tool_schemas() {
        let tools = crate::tools::get_all_tool_definitions();
        assert_eq!(
            tools.len(),
            TOOL_POLICY_SCOPES.len(),
            "every registered tool needs an explicit policy scope"
        );

        for tool in &tools {
            let properties = tool
                .input_schema
                .get("properties")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();

            let expected = if properties.contains_key("project_id") {
                // Deliberately *not* `required: required.contains("project_id")`. Deriving
                // the flag from the schema is what let `connectors.list` ship as
                // `required: false`: an optional `project_id` means the caller picks
                // whether the project agent policy applies, and omitting it degrades the
                // call into a workspace wide read. A tool that accepts a `project_id` is
                // always gated on one; `declared_project_id_is_mandatory_for_every_project_scoped_tool`
                // holds the other half of the invariant on the schema itself.
                PolicyScope::DeclaredProject { required: true }
            } else if let Some((_, lookup)) = OWNERSHIP_ARGUMENTS
                .iter()
                .find(|(argument, _)| properties.contains_key(*argument))
            {
                PolicyScope::OwnedBy(*lookup)
            } else {
                PolicyScope::WorkspaceWide
            };

            assert_eq!(
                tool_policy_scope(&tool.name),
                expected,
                "{} is classified against its own schema",
                tool.name
            );
        }

        let registered = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<std::collections::HashSet<_>>();
        for (name, _) in TOOL_POLICY_SCOPES {
            assert!(
                registered.contains(name),
                "TOOL_POLICY_SCOPES lists an unregistered tool: {name}"
            );
        }

        // Tools nobody can address without naming a project must never be workspace wide.
        for destructive in [
            "projects.delete",
            "work_items.delete",
            "comments.delete",
            "sprints.delete",
            "form_records.update",
            "plugins.invoke",
            "invocations.complete",
            "connectors.get",
            "proposals.create_from_result",
            "events.tail",
        ] {
            assert_ne!(
                tool_policy_scope(destructive),
                PolicyScope::WorkspaceWide,
                "{destructive} must not run outside the project agent policy"
            );
        }

        // An unregistered name is a connector provided tool and needs a project id.
        assert_eq!(
            tool_policy_scope("acme.deploy"),
            PolicyScope::DeclaredProject { required: true }
        );
    }

    /// `PolicyScope::DeclaredProject { required: false }` is a bypass, not a relaxation:
    /// `resolve_policy_project_id` answers `Ok(None)` for it and `enforce_project_tool_policy`
    /// then returns `Ok(())` without reading any policy, while the tool itself still runs
    /// against a workspace addressed endpoint. `connectors.list` shipped that way and
    /// returned every connector in the workspace to any caller that simply left
    /// `project_id` out.
    ///
    /// The invariant is checked against the *live registry*, not against a name list, so a
    /// tool added later with an optional `project_id` fails here instead of shipping as a
    /// hole. Both halves matter: the scope table must say `required: true`, and the
    /// published schema must tell the caller so.
    #[test]
    fn declared_project_id_is_mandatory_for_every_project_scoped_tool() {
        let mut optional_scope = Vec::new();
        let mut optional_schema = Vec::new();

        for tool in &crate::tools::get_all_tool_definitions() {
            if tool_policy_scope(&tool.name) == (PolicyScope::DeclaredProject { required: false }) {
                optional_scope.push(tool.name.clone());
            }

            let declares_project_id = tool
                .input_schema
                .get("properties")
                .and_then(Value::as_object)
                .is_some_and(|properties| properties.contains_key("project_id"));
            if !declares_project_id {
                continue;
            }
            let schema_requires_project_id = tool
                .input_schema
                .get("required")
                .and_then(Value::as_array)
                .is_some_and(|required| required.iter().any(|value| value.as_str() == Some("project_id")));
            if !schema_requires_project_id {
                optional_schema.push(tool.name.clone());
            }
        }

        assert!(
            optional_scope.is_empty(),
            "these tools are gated on a project_id the caller may omit, which skips the policy entirely: {optional_scope:?}"
        );
        assert!(
            optional_schema.is_empty(),
            "these tools accept a project_id their schema does not require, so callers are invited to omit it: {optional_schema:?}"
        );
    }

    /// The other half of the same class: a tool must not be `WorkspaceWide` while its
    /// schema offers a `project_id`, because `WorkspaceWide` skips the policy outright and
    /// `OWNERSHIP_ARGUMENTS` does not list `project_id`, so nothing would refuse the call.
    #[test]
    fn workspace_wide_tools_do_not_accept_a_project_id() {
        let offenders = crate::tools::get_all_tool_definitions()
            .iter()
            .filter(|tool| tool_policy_scope(&tool.name) == PolicyScope::WorkspaceWide)
            .filter(|tool| {
                tool.input_schema
                    .get("properties")
                    .and_then(Value::as_object)
                    .is_some_and(|properties| properties.contains_key("project_id"))
            })
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();

        assert!(
            offenders.is_empty(),
            "workspace scoped tools that take a project_id are ungoverned project tools: {offenders:?}"
        );
    }

    #[test]
    fn form_scope_target_prefers_the_record_over_the_form() -> TestResult {
        assert_eq!(
            super::form_scope_target(&json!({ "form_id": FORM, "record_id": RECORD }))?,
            Some((
                super::FormScopeTarget::Record(RECORD.to_string()),
                Some(FORM.to_string())
            ))
        );
        assert_eq!(
            super::form_scope_target(&json!({ "form_id": FORM }))?,
            Some((super::FormScopeTarget::Form(FORM.to_string()), None))
        );
        assert_eq!(
            super::form_scope_target(&json!({ "attachment_id": ATTACHMENT }))?,
            Some((super::FormScopeTarget::Attachment(ATTACHMENT.to_string()), None))
        );
        assert_eq!(super::form_scope_target(&json!({ "project_id": PROJECT }))?, None);
        assert!(super::form_scope_target(&json!({ "form_id": "../x" })).is_err());
        Ok(())
    }

    #[test]
    fn string_argument_ignores_blank_and_non_string_values() {
        assert_eq!(
            super::string_argument(&json!({ "form_id": " f1 " }), "form_id").as_deref(),
            Some("f1")
        );
        assert_eq!(super::string_argument(&json!({ "form_id": "  " }), "form_id"), None);
        assert_eq!(super::string_argument(&json!({ "form_id": 7 }), "form_id"), None);
    }

    #[test]
    fn error_messages_do_not_echo_unbounded_caller_input() {
        let hostile = format!("{}\n\nINJECTED", "x".repeat(200));
        let sanitized = super::sanitize_for_error(&hostile);
        assert!(!sanitized.contains('\n'));
        assert!(sanitized.len() <= 67, "{sanitized}");
    }

    #[tokio::test]
    async fn form_scoped_calls_are_denied_by_project_policy() -> TestResult {
        let form_lookups = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&form_lookups);
        let router = Router::new()
            .route(
                &format!("/api/v1/forms/{FORM}"),
                get(move || {
                    let counter = Arc::clone(&counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({ "code": 0, "data": { "id": FORM, "project_id": PROJECT } }))
                    }
                }),
            )
            .route(
                &format!("/api/v1/form-records/{RECORD}"),
                get(|| async { Json(json!({ "code": 0, "data": { "id": RECORD, "project_id": PROJECT } })) }),
            )
            .route(
                &format!("/api/v1/projects/{PROJECT}/agent-policy"),
                policy_route(json!(["forms.get"])),
            );
        let server = server(crate::client::test_api::spawn(router).await?)?;

        let by_form = server.call_tool("form_records.list", json!({ "form_id": FORM })).await;
        let by_record = server
            .call_tool("form_records.update", json!({ "record_id": RECORD, "values": {} }))
            .await;

        assert_eq!(by_form.is_error, Some(true));
        assert_eq!(by_record.is_error, Some(true));
        assert!(message(&by_form).contains("disabled by project agent policy"));

        // Second call for the same form must be served from the cache.
        let repeat = server.call_tool("form_records.list", json!({ "form_id": FORM })).await;
        assert_eq!(repeat.is_error, Some(true));
        assert_eq!(form_lookups.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn form_scoped_calls_pass_when_policy_enables_the_tool() -> TestResult {
        let router = Router::new()
            .route(
                &format!("/api/v1/forms/{FORM}"),
                get(|| async { Json(json!({ "code": 0, "data": { "id": FORM, "project_id": PROJECT } })) }),
            )
            .route(
                &format!("/api/v1/forms/{FORM}/records"),
                get(|| async {
                    Json(json!({
                        "code": 0,
                        "data": { "items": [], "total": 0, "page": 1, "per_page": 50, "total_pages": 0 }
                    }))
                }),
            )
            .route(
                &format!("/api/v1/projects/{PROJECT}/agent-policy"),
                policy_route(json!(["form_records.list"])),
            );
        let server = server(crate::client::test_api::spawn(router).await?)?;

        let result = server.call_tool("form_records.list", json!({ "form_id": FORM })).await;

        assert_eq!(result.is_error, None, "{}", message(&result));
        Ok(())
    }

    #[tokio::test]
    async fn form_scoped_call_rejects_a_smuggled_foreign_project_id() -> TestResult {
        let updates = Arc::new(AtomicUsize::new(0));
        let foreign_policy_reads = Arc::new(AtomicUsize::new(0));
        let update_counter = Arc::clone(&updates);
        let foreign_counter = Arc::clone(&foreign_policy_reads);

        let router = Router::new()
            .route(
                &format!("/api/v1/form-records/{RECORD}"),
                get(|| async { Json(json!({ "code": 0, "data": { "id": RECORD, "project_id": PROJECT } })) }).patch(
                    move || {
                        let counter = Arc::clone(&update_counter);
                        async move {
                            counter.fetch_add(1, Ordering::SeqCst);
                            Json(json!({ "code": 0, "data": { "id": RECORD } }))
                        }
                    },
                ),
            )
            .route(
                &format!("/api/v1/projects/{PROJECT}/agent-policy"),
                policy_route(json!(["forms.get"])),
            )
            .route(
                &format!("/api/v1/projects/{FOREIGN_PROJECT}/agent-policy"),
                get(move || {
                    let counter = Arc::clone(&foreign_counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "code": 0,
                            "data": { "mcp": { "tool_registry": { "enabled_tools": ["form_records.update"] } } }
                        }))
                    }
                }),
            );
        let server = server(crate::client::test_api::spawn(router).await?)?;

        let smuggled = server
            .call_tool(
                "form_records.update",
                json!({ "record_id": RECORD, "values": {}, "project_id": FOREIGN_PROJECT }),
            )
            .await;

        assert_eq!(smuggled.is_error, Some(true));
        let text = message(&smuggled);
        assert!(text.contains(&format!("belongs to project '{PROJECT}'")), "{text}");
        assert!(text.contains("foreign project"), "{text}");

        // The same trick through the nested payload object must fail as well.
        let nested = server
            .call_tool(
                "form_records.update",
                json!({ "record_id": RECORD, "values": {}, "payload": { "project_id": FOREIGN_PROJECT } }),
            )
            .await;
        assert_eq!(nested.is_error, Some(true));

        // Without the smuggled id the owning project decides, and it says no.
        let honest = server
            .call_tool("form_records.update", json!({ "record_id": RECORD, "values": {} }))
            .await;
        assert_eq!(honest.is_error, Some(true));
        assert!(message(&honest).contains("disabled by project agent policy"));

        assert_eq!(updates.load(Ordering::SeqCst), 0, "the record must never be written");
        assert_eq!(
            foreign_policy_reads.load(Ordering::SeqCst),
            0,
            "the policy of a project that does not own the record must never be consulted"
        );
        Ok(())
    }

    #[tokio::test]
    async fn form_scoped_call_accepts_a_project_id_that_matches_the_owner() -> TestResult {
        let updates = Arc::new(AtomicUsize::new(0));
        let update_counter = Arc::clone(&updates);
        let router = Router::new()
            .route(
                &format!("/api/v1/form-records/{RECORD}"),
                get(|| async {
                    Json(json!({ "code": 0, "data": { "id": RECORD, "form_id": FORM, "project_id": PROJECT } }))
                })
                .patch(move || {
                    let counter = Arc::clone(&update_counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({ "code": 0, "data": { "id": RECORD } }))
                    }
                }),
            )
            .route(
                &format!("/api/v1/projects/{PROJECT}/agent-policy"),
                policy_route(json!(["form_records.update"])),
            );
        let server = server(crate::client::test_api::spawn(router).await?)?;

        let result = server
            .call_tool(
                "form_records.update",
                json!({ "record_id": RECORD, "values": {}, "project_id": PROJECT }),
            )
            .await;

        assert_eq!(result.is_error, None, "{}", message(&result));
        assert_eq!(updates.load(Ordering::SeqCst), 1);
        Ok(())
    }

    /// A call that names both a record and a form is authorized through the record, and
    /// the wider `form_id` claim is checked against the record's real owner instead of
    /// being trusted.
    #[tokio::test]
    async fn a_record_call_refuses_a_form_id_that_does_not_own_it() -> TestResult {
        let listings = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&listings);
        let router = Router::new()
            .route(
                &format!("/api/v1/form-records/{RECORD}"),
                get(|| async {
                    Json(json!({ "code": 0, "data": { "id": RECORD, "form_id": FORM, "project_id": PROJECT } }))
                }),
            )
            .route(
                &format!("/api/v1/forms/{OTHER_FORM}/attachments"),
                get(move || {
                    let counter = Arc::clone(&counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({ "code": 0, "data": { "items": [] } }))
                    }
                }),
            )
            .route(
                &format!("/api/v1/projects/{PROJECT}/agent-policy"),
                policy_route(json!(["form_attachments.list"])),
            );
        let server = server(crate::client::test_api::spawn(router).await?)?;

        let mismatched = server
            .call_tool(
                "form_attachments.list",
                json!({ "form_id": OTHER_FORM, "record_id": RECORD }),
            )
            .await;

        assert_eq!(mismatched.is_error, Some(true));
        assert!(
            message(&mismatched).contains("foreign form"),
            "{}",
            message(&mismatched)
        );
        assert_eq!(listings.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn events_tail_is_governed_by_the_owning_project_policy() -> TestResult {
        let reads = Arc::new(AtomicUsize::new(0));
        let read_counter = Arc::clone(&reads);
        let router = Router::new()
            .route(
                &format!("/api/v1/forms/{FORM}"),
                get(|| async { Json(json!({ "code": 0, "data": { "id": FORM, "project_id": PROJECT } })) }),
            )
            .route(
                &format!("/api/v1/forms/{FORM}/events"),
                get(move || {
                    let counter = Arc::clone(&read_counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({ "code": 0, "data": { "items": [] } }))
                    }
                }),
            )
            .route(
                &format!("/api/v1/projects/{PROJECT}/agent-policy"),
                policy_route(json!(["forms.get"])),
            );
        let server = server(crate::client::test_api::spawn(router).await?)?;

        let denied = server.call_tool("events.tail", json!({ "form_id": FORM })).await;
        assert_eq!(denied.is_error, Some(true));
        assert!(message(&denied).contains("disabled by project agent policy"));

        let smuggled = server
            .call_tool("events.tail", json!({ "form_id": FORM, "project_id": FOREIGN_PROJECT }))
            .await;
        assert_eq!(smuggled.is_error, Some(true));

        let unscoped = server.call_tool("events.tail", json!({ "per_page": 50 })).await;
        assert_eq!(unscoped.is_error, Some(true));
        assert!(message(&unscoped).contains("carries no form_id, record_id or attachment_id"));

        assert_eq!(reads.load(Ordering::SeqCst), 0, "events must never be read");
        Ok(())
    }

    #[tokio::test]
    async fn attachment_tools_fail_closed_while_the_api_has_no_lookup() -> TestResult {
        let mutations = Arc::new(AtomicUsize::new(0));
        let archive_counter = Arc::clone(&mutations);
        let restore_counter = Arc::clone(&mutations);
        // Only the mutating routes exist, exactly like the current API surface.
        let router = Router::new()
            .route(
                &format!("/api/v1/form-attachments/{ATTACHMENT}"),
                delete(move || {
                    let counter = Arc::clone(&archive_counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({ "code": 0, "message": "ok" }))
                    }
                }),
            )
            .route(
                &format!("/api/v1/form-attachments/{ATTACHMENT}/restore"),
                post(move || {
                    let counter = Arc::clone(&restore_counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({ "code": 0, "data": { "id": ATTACHMENT } }))
                    }
                }),
            );
        let server = server(crate::client::test_api::spawn(router).await?)?;

        for tool in ["form_attachments.archive", "form_attachments.restore"] {
            let result = server.call_tool(tool, json!({ "attachment_id": ATTACHMENT })).await;
            assert_eq!(result.is_error, Some(true), "{tool} must not be allowed through");
            let text = message(&result);
            assert!(text.contains("GET /api/v1/form-attachments/{attachment_id}"), "{text}");
        }
        assert_eq!(
            mutations.load(Ordering::SeqCst),
            0,
            "attachments must not be mutated while the policy cannot be evaluated"
        );
        Ok(())
    }

    #[tokio::test]
    async fn attachment_tools_are_governed_once_the_api_exposes_the_lookup() -> TestResult {
        fn router(enabled_tools: Value, archives: &Arc<AtomicUsize>) -> Router {
            let counter = Arc::clone(archives);
            Router::new()
                .route(
                    &format!("/api/v1/form-attachments/{ATTACHMENT}"),
                    // The lookup only knows the owning form; the project is resolved from it.
                    get(|| async { Json(json!({ "code": 0, "data": { "id": ATTACHMENT, "form_id": FORM } })) }).delete(
                        move || {
                            let counter = Arc::clone(&counter);
                            async move {
                                counter.fetch_add(1, Ordering::SeqCst);
                                Json(json!({ "code": 0, "message": "ok" }))
                            }
                        },
                    ),
                )
                .route(
                    &format!("/api/v1/forms/{FORM}"),
                    get(|| async { Json(json!({ "code": 0, "data": { "id": FORM, "project_id": PROJECT } })) }),
                )
                .route(
                    &format!("/api/v1/projects/{PROJECT}/agent-policy"),
                    policy_route(enabled_tools),
                )
        }

        let denied_archives = Arc::new(AtomicUsize::new(0));
        let denied_server =
            server(crate::client::test_api::spawn(router(json!(["forms.get"]), &denied_archives)).await?)?;
        let denied = denied_server
            .call_tool("form_attachments.archive", json!({ "attachment_id": ATTACHMENT }))
            .await;
        assert_eq!(denied.is_error, Some(true));
        assert!(message(&denied).contains("disabled by project agent policy"));
        assert_eq!(denied_archives.load(Ordering::SeqCst), 0);

        let allowed_archives = Arc::new(AtomicUsize::new(0));
        let allowed_server = server(
            crate::client::test_api::spawn(router(json!(["form_attachments.archive"]), &allowed_archives)).await?,
        )?;
        let allowed = allowed_server
            .call_tool("form_attachments.archive", json!({ "attachment_id": ATTACHMENT }))
            .await;
        assert_eq!(allowed.is_error, None, "{}", message(&allowed));
        assert_eq!(allowed_archives.load(Ordering::SeqCst), 1);

        // A foreign project_id stays refused even when the lookup works.
        let smuggled = allowed_server
            .call_tool(
                "form_attachments.archive",
                json!({ "attachment_id": ATTACHMENT, "project_id": FOREIGN_PROJECT }),
            )
            .await;
        assert_eq!(smuggled.is_error, Some(true));
        assert_eq!(allowed_archives.load(Ordering::SeqCst), 1);
        Ok(())
    }

    /// The round-2 gap: everything outside the form family ran with no project policy at
    /// all. Work items, comments, plugins, invocations, connectors and check results are
    /// now resolved back to their owning project exactly like form data.
    #[tokio::test]
    async fn id_addressed_non_form_tools_are_governed_by_the_owning_project() -> TestResult {
        let mutations = Arc::new(AtomicUsize::new(0));
        let delete_counter = Arc::clone(&mutations);
        let invoke_counter = Arc::clone(&mutations);
        let issue_lookups = Arc::new(AtomicUsize::new(0));
        let issue_counter = Arc::clone(&issue_lookups);

        let router = Router::new()
            .route(
                &format!("/api/v1/issues/{WORK_ITEM}"),
                get(move || {
                    let counter = Arc::clone(&issue_counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({ "code": 0, "data": { "id": WORK_ITEM, "project_id": PROJECT } }))
                    }
                })
                .delete(move || {
                    let counter = Arc::clone(&delete_counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({ "code": 0, "message": "ok" }))
                    }
                }),
            )
            .route(
                &format!("/api/v1/plugins/{PLUGIN}"),
                get(|| async { Json(json!({ "code": 0, "data": { "id": PLUGIN, "project_id": PROJECT } })) }),
            )
            .route(
                &format!("/api/v1/plugins/{PLUGIN}/invoke"),
                post(move || {
                    let counter = Arc::clone(&invoke_counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({ "code": 0, "data": {} }))
                    }
                }),
            )
            .route(
                &format!("/api/v1/projects/{PROJECT}/agent-policy"),
                policy_route(json!(["work_items.list"])),
            );
        let server = server(crate::client::test_api::spawn(router).await?)?;

        for (tool, args) in [
            ("work_items.delete", json!({ "work_item_id": WORK_ITEM })),
            (
                "work_items.update",
                json!({ "work_item_id": WORK_ITEM, "state": "done" }),
            ),
            ("comments.list", json!({ "work_item_id": WORK_ITEM })),
            (
                "plugins.invoke",
                json!({ "plugin_id": PLUGIN, "hook_kind": "on_create" }),
            ),
        ] {
            let result = server.call_tool(tool, args).await;
            assert_eq!(result.is_error, Some(true), "{tool} escaped the policy gate");
            assert!(
                message(&result).contains("disabled by project agent policy"),
                "{tool}: {}",
                message(&result)
            );
        }
        assert_eq!(mutations.load(Ordering::SeqCst), 0, "no backend mutation may happen");
        // Four calls, two distinct owners, and the owner of each is read exactly once.
        assert_eq!(issue_lookups.load(Ordering::SeqCst), 1, "owner lookups must be cached");
        Ok(())
    }

    /// The blocker from the round-2 audit, end to end: a caller aims a destructive tool
    /// at a project it does not own by smuggling the id, and the smuggled project's
    /// policy must never be consulted nor the destructive endpoint reached.
    #[tokio::test]
    async fn a_smuggled_project_id_cannot_reach_a_destructive_tool() -> TestResult {
        let deletes = Arc::new(AtomicUsize::new(0));
        let policy_reads = Arc::new(AtomicUsize::new(0));
        let delete_counter = Arc::clone(&deletes);
        let policy_counter = Arc::clone(&policy_reads);

        let router = Router::new()
            .route(
                &format!("/api/v1/projects/{PROJECT}"),
                delete(move || {
                    let counter = Arc::clone(&delete_counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({ "code": 0, "message": "deleted" }))
                    }
                }),
            )
            .route(
                &format!("/api/v1/projects/{PROJECT}/work-items"),
                get(|| async { Json(json!({ "code": 0, "data": { "items": [] } })) }),
            )
            .route(
                &format!("/api/v1/projects/{PROJECT}/agent-policy"),
                get(move || {
                    let counter = Arc::clone(&policy_counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "code": 0,
                            "data": { "mcp": { "tool_registry": { "enabled_tools": ["projects.list"] } } }
                        }))
                    }
                }),
            );
        let server = server(crate::client::test_api::spawn(router).await?)?;

        // The exact shapes the audit used to walk the policy lookup off `/agent-policy`.
        for smuggled in [
            format!("{PROJECT}?"),
            format!("{PROJECT}/work-items?pad="),
            format!("{PROJECT}#"),
            format!("{PROJECT}/"),
        ] {
            let result = server
                .call_tool("projects.delete", json!({ "project_id": smuggled }))
                .await;
            assert_eq!(result.is_error, Some(true), "'{smuggled}' was accepted");
            assert!(
                message(&result).contains("is not a canonical UUID"),
                "'{smuggled}': {}",
                message(&result)
            );
        }
        assert_eq!(deletes.load(Ordering::SeqCst), 0, "the project must never be deleted");
        assert_eq!(
            policy_reads.load(Ordering::SeqCst),
            0,
            "a malformed project id must not even reach the policy endpoint"
        );

        // The honest call is still evaluated, and the policy still says no.
        let honest = server
            .call_tool("projects.delete", json!({ "project_id": PROJECT }))
            .await;
        assert_eq!(honest.is_error, Some(true));
        assert!(message(&honest).contains("disabled by project agent policy"));
        assert_eq!(deletes.load(Ordering::SeqCst), 0);
        assert_eq!(policy_reads.load(Ordering::SeqCst), 1);
        Ok(())
    }

    /// Tools whose owner the API cannot report must refuse rather than run unguarded.
    #[tokio::test]
    async fn tools_without_an_owner_lookup_fail_closed() -> TestResult {
        let mutations = Arc::new(AtomicUsize::new(0));
        let sprint_counter = Arc::clone(&mutations);
        let comment_counter = Arc::clone(&mutations);
        let router = Router::new()
            .route(
                &format!("/api/v1/sprints/{SPRINT}"),
                delete(move || {
                    let counter = Arc::clone(&sprint_counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({ "code": 0, "message": "ok" }))
                    }
                }),
            )
            .route(
                &format!("/api/v1/comments/{COMMENT}"),
                delete(move || {
                    let counter = Arc::clone(&comment_counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({ "code": 0, "message": "ok" }))
                    }
                }),
            );
        let server = server(crate::client::test_api::spawn(router).await?)?;

        for (tool, args, endpoint) in [
            (
                "sprints.delete",
                json!({ "sprint_id": SPRINT }),
                "GET /api/v1/sprints/{sprint_id}",
            ),
            (
                "sprints.update",
                json!({ "sprint_id": SPRINT, "name": "renamed" }),
                "GET /api/v1/sprints/{sprint_id}",
            ),
            (
                "comments.delete",
                json!({ "comment_id": COMMENT }),
                "GET /api/v1/comments/{comment_id}",
            ),
        ] {
            let result = server.call_tool(tool, args).await;
            assert_eq!(result.is_error, Some(true), "{tool} ran unguarded");
            assert!(message(&result).contains(endpoint), "{tool}: {}", message(&result));
        }
        assert_eq!(mutations.load(Ordering::SeqCst), 0);
        Ok(())
    }

    /// Workspace scoped tools have no owning project by design, but a call that smuggles
    /// a project owned id into one is a classification mismatch and is refused.
    #[tokio::test]
    async fn workspace_scoped_tools_reject_project_owned_identifiers() -> TestResult {
        let router = Router::new().route(
            "/api/v1/workspaces/workspace-1/labels",
            get(|| async { Json(json!({ "code": 0, "data": { "items": [] } })) }),
        );
        let server = server(crate::client::test_api::spawn(router).await?)?;

        let clean = server.call_tool("labels.list", json!({})).await;
        assert_eq!(clean.is_error, None, "{}", message(&clean));

        let smuggled = server.call_tool("labels.list", json!({ "record_id": RECORD })).await;
        assert_eq!(smuggled.is_error, Some(true));
        assert!(
            message(&smuggled).contains("registered as workspace scoped"),
            "{}",
            message(&smuggled)
        );
        Ok(())
    }

    /// `connectors.list` shipped as `DeclaredProject { required: false }`. Omitting
    /// `project_id` made `resolve_policy_project_id` answer `Ok(None)`, which skipped the
    /// policy load entirely, while the tool still called the workspace addressed endpoint
    /// and returned every connector in the workspace — endpoints and auth policy
    /// references included. The refusal has to happen *before* the API is touched, so the
    /// backend hit counter is part of the assertion.
    #[tokio::test]
    async fn connectors_list_without_a_project_id_never_reaches_the_workspace_endpoint() -> TestResult {
        let connector_hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&connector_hits);
        let queries = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
        let seen = Arc::clone(&queries);

        let router = Router::new()
            .route(
                "/api/v1/workspaces/workspace-1/connectors",
                get(move |axum::extract::RawQuery(query): axum::extract::RawQuery| {
                    let counter = Arc::clone(&counter);
                    let seen = Arc::clone(&seen);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        seen.lock().await.push(query.unwrap_or_default());
                        Json(json!({
                            "code": 0,
                            "data": { "items": [{
                                "id": RESOURCE,
                                "kind": "webhook",
                                "endpoint": "https://internal.example/hooks/payroll",
                                "auth_policy": { "secret_ref": "OPENPR_CONNECTOR_SECRET_W_X_DEFAULT" }
                            }] }
                        }))
                    }
                }),
            )
            .route(
                &format!("/api/v1/projects/{PROJECT}/agent-policy"),
                policy_route(json!(["connectors.list"])),
            );
        let server = server(crate::client::test_api::spawn(router).await?)?;

        for bypass in [json!({}), json!({ "kind": "webhook" }), json!({ "project_id": null })] {
            let refused = server.call_tool("connectors.list", bypass.clone()).await;
            assert_eq!(refused.is_error, Some(true), "{bypass} was not refused");
            assert!(
                message(&refused).contains("requires a project_id"),
                "{}",
                message(&refused)
            );
        }
        assert_eq!(
            connector_hits.load(Ordering::SeqCst),
            0,
            "a refused call must never reach the workspace connector endpoint"
        );

        // Naming the project loads that project's policy and scopes the backend query.
        let allowed = server
            .call_tool("connectors.list", json!({ "project_id": PROJECT }))
            .await;
        assert_eq!(allowed.is_error, None, "{}", message(&allowed));
        assert_eq!(connector_hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            queries.lock().await.as_slice(),
            [format!("project_id={PROJECT}")],
            "the declared project must reach the API as the filter it was authorized against"
        );
        Ok(())
    }

    /// The declared project is not just a formality: a project whose agent policy does not
    /// enable the tool is refused, which is the whole point of making the id mandatory.
    #[tokio::test]
    async fn connectors_list_is_denied_by_the_declared_project_policy() -> TestResult {
        let connector_hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&connector_hits);
        let router = Router::new()
            .route(
                "/api/v1/workspaces/workspace-1/connectors",
                get(move || {
                    let counter = Arc::clone(&counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({ "code": 0, "data": { "items": [] } }))
                    }
                }),
            )
            .route(
                &format!("/api/v1/projects/{PROJECT}/agent-policy"),
                policy_route(json!(["work_items.list"])),
            );
        let server = server(crate::client::test_api::spawn(router).await?)?;

        let denied = server
            .call_tool("connectors.list", json!({ "project_id": PROJECT }))
            .await;
        assert_eq!(denied.is_error, Some(true));
        assert!(
            message(&denied).contains("disabled by project agent policy"),
            "{}",
            message(&denied)
        );
        assert_eq!(connector_hits.load(Ordering::SeqCst), 0);
        Ok(())
    }

    /// Every `Ok(_) => success("... deleted")` call site must still report backend
    /// failures, because the API answers HTTP 200 and carries the real status in the
    /// response envelope.
    #[tokio::test]
    async fn mutation_tools_report_envelope_errors_instead_of_fake_success() -> TestResult {
        async fn denied() -> Json<Value> {
            Json(json!({ "code": 403, "message": "policy denied", "data": null }))
        }

        let router = Router::new()
            .route(&format!("/api/v1/projects/{PROJECT}"), delete(denied))
            .route(
                &format!("/api/v1/projects/{PROJECT}/resources/{RESOURCE}"),
                delete(denied),
            )
            .route(&format!("/api/v1/labels/{LABEL}"), delete(denied))
            .route(
                &format!("/api/v1/issues/{WORK_ITEM}"),
                get(|| async { Json(json!({ "code": 0, "data": { "id": WORK_ITEM, "project_id": PROJECT } })) })
                    .delete(denied),
            )
            .route(
                &format!("/api/v1/issues/{WORK_ITEM}/labels/{LABEL}"),
                post(denied).delete(denied),
            )
            .route(&format!("/api/v1/issues/{WORK_ITEM}/labels/batch"), post(denied))
            .route(
                &format!("/api/v1/projects/{PROJECT}/agent-policy"),
                get(|| async { Json(json!({ "code": 0, "data": {} })) }),
            );
        let server = server(crate::client::test_api::spawn(router).await?)?;

        let calls = [
            ("projects.delete", json!({ "project_id": PROJECT })),
            (
                "project_resources.delete",
                json!({ "project_id": PROJECT, "resource_id": RESOURCE }),
            ),
            ("labels.delete", json!({ "label_id": LABEL })),
            ("work_items.delete", json!({ "work_item_id": WORK_ITEM })),
            (
                "work_items.add_label",
                json!({ "work_item_id": WORK_ITEM, "label_id": LABEL }),
            ),
            (
                "work_items.remove_label",
                json!({ "work_item_id": WORK_ITEM, "label_id": LABEL }),
            ),
            (
                "work_items.add_labels",
                json!({ "work_item_id": WORK_ITEM, "label_ids": [LABEL] }),
            ),
        ];

        for (tool, args) in calls {
            let result = server.call_tool(tool, args).await;
            let text = message(&result);
            assert_eq!(result.is_error, Some(true), "{tool} reported success: {text}");
            assert!(text.contains("API error 403"), "{tool}: {text}");
            assert!(text.contains("policy denied"), "{tool}: {text}");
        }
        Ok(())
    }

    /// An agent-policy answer that is not the `{code, message, data}` envelope must not
    /// authorize anything: that fail-open default is what made a smuggled policy URL
    /// useful in the first place.
    #[tokio::test]
    async fn a_non_envelope_policy_answer_never_authorizes() -> TestResult {
        let deletes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&deletes);
        let router = Router::new()
            .route(
                &format!("/api/v1/projects/{PROJECT}"),
                delete(move || {
                    let counter = Arc::clone(&counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({ "code": 0, "message": "deleted" }))
                    }
                }),
            )
            .route(
                &format!("/api/v1/projects/{PROJECT}/agent-policy"),
                // A 200 with a project object rather than a policy envelope.
                get(|| async { Json(json!({ "id": PROJECT, "name": "victim" })) }),
            );
        let server = server(crate::client::test_api::spawn(router).await?)?;

        let result = server
            .call_tool("projects.delete", json!({ "project_id": PROJECT }))
            .await;

        assert_eq!(result.is_error, Some(true));
        assert!(message(&result).contains("Malformed response"), "{}", message(&result));
        assert_eq!(deletes.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn tool_call_audit_summary_uses_text_content() {
        let result = CallToolResult {
            content: vec![ToolContent::text("done")],
            is_error: None,
        };

        assert_eq!(summarize_tool_result(&result).as_deref(), Some("done"));
    }
}
