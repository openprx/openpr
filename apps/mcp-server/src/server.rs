use crate::client::OpenPrClient;
use crate::protocol::{
    CallToolParams, CallToolResult, JsonRpcError, JsonRpcRequest, JsonRpcResponse, ListToolsResult,
};
use crate::tools;
use serde_json::{Value, json};

const SKILL_GUIDE_MD: &str = r#"# OpenPR MCP Skill Guide

## Tools (34)

### Projects: projects.list, projects.get, projects.create, projects.update, projects.delete
### Work Items: work_items.list, work_items.get, work_items.get_by_identifier, work_items.create, work_items.update, work_items.delete, work_items.search, work_items.add_label, work_items.add_labels, work_items.remove_label, work_items.list_labels
### Comments: comments.create, comments.list, comments.delete
### Files: files.upload (base64 -> URL)
### Labels: labels.list, labels.list_by_project, labels.create, labels.update, labels.delete
### Sprints: sprints.list, sprints.create, sprints.update, sprints.delete
### Proposals: proposals.list, proposals.get, proposals.create
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
"#;

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
    pub fn new(client: OpenPrClient) -> Self {
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
            "notifications/initialized" => JsonRpcResponse::success(req.id, json!({})),
            "ping" => JsonRpcResponse::success(req.id, json!({})),
            "tools/list" => self.handle_list_tools(req.id),
            "tools/call" => self.handle_call_tool(req.id, req.params).await,
            "resources/list" => self.handle_resources_list(req.id),
            "resources/templates/list" => self.handle_resources_templates_list(req.id),
            "resources/read" => self.handle_resources_read(req.id, req.params).await,
            _ => JsonRpcResponse::error(
                req.id,
                JsonRpcError::method_not_found(format!("Unknown method: {}", req.method)),
            ),
        };

        if is_notification {
            None
        } else {
            Some(response)
        }
    }

    fn handle_list_tools(&self, id: Option<Value>) -> JsonRpcResponse {
        let tools = tools::get_all_tool_definitions();
        let result = ListToolsResult { tools };

        match serde_json::to_value(&result) {
            Ok(value) => JsonRpcResponse::success(id, value),
            Err(e) => JsonRpcResponse::error(
                id,
                JsonRpcError::internal_error(format!("Failed to serialize tools: {}", e)),
            ),
        }
    }

    async fn handle_call_tool(&self, id: Option<Value>, params: Option<Value>) -> JsonRpcResponse {
        let params = match params {
            Some(p) => p,
            None => {
                return JsonRpcResponse::error(id, JsonRpcError::invalid_params("Missing params"));
            }
        };

        let call_params: CallToolParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    id,
                    JsonRpcError::invalid_params(format!("Invalid params: {}", e)),
                );
            }
        };

        let args = call_params
            .arguments
            .unwrap_or(Value::Object(Default::default()));

        let result = self.execute_tool(&call_params.name, args).await;

        match serde_json::to_value(&result) {
            Ok(value) => JsonRpcResponse::success(id, value),
            Err(e) => JsonRpcResponse::error(
                id,
                JsonRpcError::internal_error(format!("Failed to serialize result: {}", e)),
            ),
        }
    }

    async fn execute_tool(&self, name: &str, args: Value) -> CallToolResult {
        match name {
            // Projects
            "projects.list" => tools::projects::list_projects(&self.client, args).await,
            "projects.get" => tools::projects::get_project(&self.client, args).await,
            "projects.create" => tools::projects::create_project(&self.client, args).await,
            "projects.update" => tools::projects::update_project(&self.client, args).await,
            "projects.delete" => tools::projects::handle_delete_project(&self.client, args).await,

            // Work Items
            "work_items.list" => tools::work_items::list_work_items(&self.client, args).await,
            "work_items.get" => tools::work_items::get_work_item(&self.client, args).await,
            "work_items.get_by_identifier" => {
                tools::work_items::get_work_item_by_identifier(&self.client, args).await
            }
            "work_items.create" => tools::work_items::create_work_item(&self.client, args).await,
            "work_items.update" => tools::work_items::update_work_item(&self.client, args).await,
            "work_items.add_label" => {
                tools::work_items::add_label_to_work_item(&self.client, args).await
            }
            "work_items.remove_label" => {
                tools::work_items::remove_label_from_work_item(&self.client, args).await
            }
            "work_items.list_labels" => {
                tools::work_items::list_work_item_labels(&self.client, args).await
            }
            "work_items.delete" => {
                tools::work_items::handle_delete_work_item(&self.client, args).await
            }
            "work_items.search" => tools::work_items::search_work_items(&self.client, args).await,
            "work_items.add_labels" => {
                tools::work_items::add_labels_to_work_item(&self.client, args).await
            }

            // Comments
            "comments.list" => tools::comments::list_comments(&self.client, args).await,
            "comments.create" => tools::comments::create_comment(&self.client, args).await,
            "comments.delete" => tools::comments::handle_delete_comment(&self.client, args).await,
            "files.upload" => tools::files::upload_file(&self.client, args).await,

            // Proposals
            "proposals.list" => tools::proposals::list_proposals(&self.client, args).await,
            "proposals.get" => tools::proposals::get_proposal(&self.client, args).await,
            "proposals.create" => tools::proposals::create_proposal(&self.client, args).await,

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
            "labels.list_by_project" => {
                tools::labels::list_project_labels(&self.client, args).await
            }
            "labels.update" => tools::labels::update_label(&self.client, args).await,
            "labels.delete" => tools::labels::handle_delete_label(&self.client, args).await,

            // Search
            "search.all" => tools::search::search_all(&self.client, args).await,

            _ => CallToolResult::error(format!("Unknown tool: {}", name)),
        }
    }

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
        ];

        JsonRpcResponse::success(id, json!({ "resources": resources }))
    }

    fn handle_resources_templates_list(&self, id: Option<Value>) -> JsonRpcResponse {
        let templates = vec![
            json!({
                "uriTemplate": "openpr://projects/{project_id}/issues",
                "name": "Project Issues",
                "description": "List issues for a specific project",
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

    async fn handle_resources_read(
        &self,
        id: Option<Value>,
        params: Option<Value>,
    ) -> JsonRpcResponse {
        let uri = match params
            .as_ref()
            .and_then(|value| value.get("uri"))
            .and_then(Value::as_str)
        {
            Some(uri) if !uri.is_empty() => uri.to_string(),
            _ => {
                return JsonRpcResponse::error(
                    id,
                    JsonRpcError::invalid_params("Missing required field: uri"),
                );
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
            _ => {}
        }

        if let Some(project_id) = parse_project_resource_uri(&uri, "/issues") {
            return match self.client.list_work_items(project_id).await {
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
                    JsonRpcError::internal_error(format!(
                        "Failed to read project issues resource: {}",
                        error
                    )),
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
                    JsonRpcError::internal_error(format!(
                        "Failed to read project sprints resource: {}",
                        error
                    )),
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
                    JsonRpcError::internal_error(format!(
                        "Failed to read issue resource: {}",
                        error
                    )),
                ),
            };
        }

        JsonRpcResponse::error(
            id,
            JsonRpcError::invalid_params(format!("Unknown resource URI: {}", uri)),
        )
    }
}

fn parse_project_resource_uri<'a>(uri: &'a str, suffix: &str) -> Option<&'a str> {
    let project_id = uri
        .strip_prefix("openpr://projects/")?
        .strip_suffix(suffix)?;
    if project_id.is_empty() || project_id.contains('/') {
        return None;
    }
    Some(project_id)
}

fn parse_issue_identifier_uri(uri: &str) -> Option<String> {
    let identifier = uri.strip_prefix("openpr://issues/")?;
    if identifier.is_empty() || identifier.contains('/') {
        return None;
    }

    Some(identifier.to_string())
}
