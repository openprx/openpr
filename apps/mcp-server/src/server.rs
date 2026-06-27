use crate::client::OpenPrClient;
use crate::protocol::{CallToolParams, CallToolResult, JsonRpcError, JsonRpcRequest, JsonRpcResponse, ListToolsResult};
use crate::tools;
use serde_json::{Value, json};
use std::time::Instant;

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
- state: backlog | todo | in_progress | done
- priority: none | low | medium | high | urgent
- attachments: array of URLs from files.upload
- decimal/amount form fields: send decimal strings, never JSON numbers
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

pub struct McpServer {
    client: OpenPrClient,
}

impl McpServer {
    pub const fn new(client: OpenPrClient) -> Self {
        Self { client }
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
        let tools = match extract_tools_list_project_id(params.as_ref()) {
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

    pub async fn execute_tool(&self, name: &str, args: Value) -> CallToolResult {
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

    async fn enforce_project_tool_policy(&self, tool_name: &str, args: &Value) -> Result<(), String> {
        let Some(project_id) = extract_call_project_id(args) else {
            return Ok(());
        };

        let policy = self
            .client
            .get_project_agent_policy(&project_id)
            .await
            .map_err(|error| format!("Failed to load project agent policy for tools/call: {error}"))?;
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
                "version": "0.1.0"
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
            return match self.client.list_work_items(project_id, 1, 50).await {
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
            return match self.client.list_forms(project_id).await {
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
            return match self.client.list_form_records(form_id).await {
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
            return match self.client.list_form_events(form_id, None).await {
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
            return match self.client.list_form_record_events(record_id, None).await {
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
            return match self.client.list_connectors(Some(project_id)).await {
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
            return match self.client.list_invocations(project_id).await {
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

fn extract_tools_list_project_id(params: Option<&Value>) -> Option<String> {
    let params = params?;
    params
        .get("project_id")
        .or_else(|| params.get("projectId"))
        .or_else(|| params.get("context").and_then(|context| context.get("project_id")))
        .or_else(|| params.get("context").and_then(|context| context.get("projectId")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn extract_call_project_id(args: &Value) -> Option<String> {
    args.get("project_id")
        .or_else(|| args.get("projectId"))
        .or_else(|| args.get("payload").and_then(|payload| payload.get("project_id")))
        .or_else(|| args.get("payload").and_then(|payload| payload.get("projectId")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
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
        AGENTS_GUIDE_MD, SKILL_GUIDE_MD, extract_call_project_id, extract_tools_list_project_id,
        is_tool_enabled_by_policy, redact_tool_arguments, summarize_tool_result,
    };
    use crate::protocol::{CallToolResult, ToolContent};
    use serde_json::{Value, json};

    #[test]
    fn tools_list_project_id_accepts_top_level_and_context_params() {
        assert_eq!(
            extract_tools_list_project_id(Some(&json!({ "project_id": "p1" }))).as_deref(),
            Some("p1")
        );
        assert_eq!(
            extract_tools_list_project_id(Some(&json!({ "context": { "projectId": "p2" } }))).as_deref(),
            Some("p2")
        );
        assert_eq!(extract_tools_list_project_id(Some(&json!({ "project_id": " " }))), None);
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
    fn tools_call_project_id_accepts_top_level_and_payload_params() {
        assert_eq!(
            extract_call_project_id(&json!({ "project_id": "p1" })).as_deref(),
            Some("p1")
        );
        assert_eq!(
            extract_call_project_id(&json!({ "payload": { "projectId": "p2" } })).as_deref(),
            Some("p2")
        );
        assert_eq!(extract_call_project_id(&json!({ "work_item_id": "w1" })), None);
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

    #[test]
    fn tool_call_audit_summary_uses_text_content() {
        let result = CallToolResult {
            content: vec![ToolContent::text("done")],
            is_error: None,
        };

        assert_eq!(summarize_tool_result(&result).as_deref(), Some("done"));
    }
}
