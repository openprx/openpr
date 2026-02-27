use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn list_work_items_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.list".to_string(),
        description: "List all work items in a project".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["project_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ListWorkItemsInput {
    project_id: String,
}

pub async fn list_work_items(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: ListWorkItemsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.list_work_items(&input.project_id).await {
        Ok(items) => {
            let json = serde_json::to_string_pretty(&items).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn get_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.get".to_string(),
        description: "Get details of a specific work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["work_item_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct GetWorkItemInput {
    work_item_id: String,
}

pub async fn get_work_item(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: GetWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.get_work_item(&input.work_item_id).await {
        Ok(item) => {
            let json = serde_json::to_string_pretty(&item).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn get_work_item_by_identifier_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.get_by_identifier".to_string(),
        description: "Get details of a specific work item by identifier (e.g. PRX-42)".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "identifier": {
                    "type": "string",
                    "description": "Human-readable work item identifier, e.g. PRX-42",
                    "pattern": "^[A-Za-z0-9]+-[0-9]+$"
                }
            },
            "required": ["identifier"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct GetWorkItemByIdentifierInput {
    identifier: String,
}

pub async fn get_work_item_by_identifier(
    client: &OpenPrClient,
    args: serde_json::Value,
) -> CallToolResult {
    let input: GetWorkItemByIdentifierInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let identifier = input.identifier.trim();
    let (project_identifier, sequence_id) = match parse_identifier(identifier) {
        Ok(v) => v,
        Err(e) => return CallToolResult::error(e),
    };

    // Prefer backend-native identifier lookup when available.
    if let Ok(item) = client.get_work_item_by_identifier(identifier).await {
        let json = serde_json::to_string_pretty(&item).unwrap_or_default();
        return CallToolResult::success(json);
    }

    let projects = match client.list_projects().await {
        Ok(p) => p,
        Err(e) => return CallToolResult::error(e),
    };

    let project_id = match find_project_id_by_identifier(&projects, project_identifier) {
        Some(id) => id,
        None => {
            return CallToolResult::error(format!(
                "Project with identifier '{}' not found",
                project_identifier
            ));
        }
    };

    // Prefer project-scoped lookup to reduce false positives and payload size.
    let project_search_path = format!(
        "/api/v1/projects/{}/issues?search={}&per_page=100",
        project_id, identifier
    );
    let project_search: Value = match client.get(&project_search_path).await {
        Ok(v) => v,
        Err(e) => return CallToolResult::error(e),
    };

    let mut work_item_id = find_work_item_id_by_identifier(
        &project_search,
        identifier,
        project_identifier,
        sequence_id,
        true,
    );

    if work_item_id.is_none() {
        // Fallback to global search for deployments where identifier search is indexed globally.
        if let Ok(global_search) = client.search_work_items(identifier).await {
            work_item_id = find_work_item_id_by_identifier(
                &global_search,
                identifier,
                project_identifier,
                sequence_id,
                false,
            );
        }
    }

    if work_item_id.is_none() {
        // API list/search payloads may not include sequence/identifier fields.
        // In that case, derive the N-th issue in stable creation order as a fallback.
        match find_work_item_id_by_sequence_position(client, &project_id, sequence_id).await {
            Ok(id) => work_item_id = id,
            Err(e) => return CallToolResult::error(e),
        }
    }

    let work_item_id = match work_item_id {
        Some(id) => id,
        None => {
            return CallToolResult::error(format!(
                "Work item '{}' not found in project '{}'",
                identifier, project_identifier
            ));
        }
    };

    match client.get_work_item(&work_item_id).await {
        Ok(item) => {
            let json = serde_json::to_string_pretty(&item).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

async fn find_work_item_id_by_sequence_position(
    client: &OpenPrClient,
    project_id: &str,
    sequence_id: u64,
) -> Result<Option<String>, String> {
    if sequence_id == 0 {
        return Ok(None);
    }

    let per_page = 100u64;
    let mut page = 1u64;
    let mut current_sequence = 0u64;

    loop {
        let path = format!(
            "/api/v1/projects/{}/issues?page={}&per_page={}&sort_by=created_at&sort_order=asc",
            project_id, page, per_page
        );
        let payload: Value = client.get(&path).await?;
        let data = extract_data(&payload);

        let items = if let Some(arr) = data.get("items").and_then(Value::as_array) {
            arr
        } else if let Some(arr) = data.as_array() {
            arr
        } else {
            return Ok(None);
        };

        if items.is_empty() {
            return Ok(None);
        }

        for item in items {
            current_sequence += 1;
            if current_sequence == sequence_id {
                return Ok(item
                    .get("id")
                    .and_then(Value::as_str)
                    .map(ToString::to_string));
            }
        }

        let reached_last_page = data
            .get("total_pages")
            .and_then(value_to_u64)
            .map(|total_pages| page >= total_pages)
            .unwrap_or(items.len() < per_page as usize);

        if reached_last_page {
            return Ok(None);
        }

        page += 1;
    }
}

fn parse_identifier(identifier: &str) -> Result<(&str, u64), String> {
    let (project_identifier, sequence_text) = identifier
        .rsplit_once('-')
        .ok_or_else(|| "identifier must be in format PROJECT-123".to_string())?;

    if project_identifier.trim().is_empty() {
        return Err("identifier project prefix cannot be empty".to_string());
    }

    let sequence_id = sequence_text
        .parse::<u64>()
        .map_err(|_| "identifier sequence part must be a positive integer".to_string())?;

    Ok((project_identifier, sequence_id))
}

fn extract_data(value: &Value) -> &Value {
    value.get("data").unwrap_or(value)
}

fn find_project_id_by_identifier(
    projects_payload: &Value,
    project_identifier: &str,
) -> Option<String> {
    let project_identifier_upper = project_identifier.to_ascii_uppercase();
    let projects_data = extract_data(projects_payload);

    let projects = if let Some(items) = projects_data.get("items").and_then(Value::as_array) {
        items
    } else if let Some(arr) = projects_data.as_array() {
        arr
    } else {
        return None;
    };

    projects
        .iter()
        .find(|project| {
            project
                .get("key")
                .and_then(Value::as_str)
                .or_else(|| project.get("identifier").and_then(Value::as_str))
                .map(|key| key.eq_ignore_ascii_case(&project_identifier_upper))
                .unwrap_or(false)
        })
        .and_then(|project| project.get("id").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn find_work_item_id_by_identifier(
    payload: &Value,
    identifier: &str,
    project_identifier: &str,
    sequence_id: u64,
    project_scoped: bool,
) -> Option<String> {
    let mut candidates: Vec<&Value> = Vec::new();
    let data = extract_data(payload);

    if let Some(items) = data.get("items").and_then(Value::as_array) {
        candidates.extend(items);
    }
    if let Some(results) = data.get("results").and_then(Value::as_array) {
        for result in results {
            if result
                .get("type")
                .and_then(Value::as_str)
                .map(|t| t.eq_ignore_ascii_case("issue"))
                .unwrap_or(false)
            {
                candidates.push(result);
            }
        }
    }
    if let Some(arr) = data.as_array() {
        candidates.extend(arr);
    }
    if candidates.is_empty() && data.is_object() {
        candidates.push(data);
    }

    candidates
        .into_iter()
        .find(|item| {
            item_matches_identifier(
                item,
                identifier,
                project_identifier,
                sequence_id,
                project_scoped,
            )
        })
        .and_then(|item| item.get("id").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn item_matches_identifier(
    item: &Value,
    identifier: &str,
    project_identifier: &str,
    sequence_id: u64,
    project_scoped: bool,
) -> bool {
    let identifier_upper = identifier.to_ascii_uppercase();
    let project_identifier_upper = project_identifier.to_ascii_uppercase();

    let identifier_fields = [
        "key",
        "identifier",
        "human_identifier",
        "display_id",
        "work_item_identifier",
    ];
    if identifier_fields.iter().any(|field| {
        item.get(*field)
            .and_then(Value::as_str)
            .map(|v| v.eq_ignore_ascii_case(&identifier_upper))
            .unwrap_or(false)
    }) {
        return true;
    }

    let sequence_match = ["sequence_id", "sequence_number", "number", "seq", "index"]
        .iter()
        .any(|field| {
            item.get(*field)
                .and_then(value_to_u64)
                .map(|n| n == sequence_id)
                .unwrap_or(false)
        });

    if !sequence_match {
        return false;
    }

    if project_scoped {
        return true;
    }

    ["project_identifier", "project_key", "project_code"]
        .iter()
        .any(|field| {
            item.get(*field)
                .and_then(Value::as_str)
                .map(|v| v.eq_ignore_ascii_case(&project_identifier_upper))
                .unwrap_or(false)
        })
}

fn value_to_u64(value: &Value) -> Option<u64> {
    if let Some(v) = value.as_u64() {
        return Some(v);
    }
    if let Some(v) = value.as_i64() {
        return u64::try_from(v).ok();
    }
    value.as_str()?.trim().parse::<u64>().ok()
}

const VALID_WORK_ITEM_STATES: &[&str] = &["backlog", "todo", "in_progress", "done"];

fn validate_work_item_state(state: &str) -> Result<(), String> {
    if VALID_WORK_ITEM_STATES.contains(&state) {
        return Ok(());
    }

    Err(format!(
        "Invalid state '{}'. Expected one of: {}",
        state,
        VALID_WORK_ITEM_STATES.join(", ")
    ))
}

fn append_attachments_to_text(
    text: Option<String>,
    attachments: Option<Vec<String>>,
) -> Option<String> {
    match attachments {
        Some(items) if !items.is_empty() => {
            let mut output = text.unwrap_or_default();
            output.push_str("\n\n**附件：**\n");
            for url in items {
                let name = attachment_name_from_url(&url);
                output.push_str(&format!("- [{}]({})\n", name, url));
            }
            Some(output.trim_end().to_string())
        }
        _ => text,
    }
}

fn attachment_name_from_url(url: &str) -> String {
    url.rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(url)
        .to_string()
}

pub fn create_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.create".to_string(),
        description: "Create a new work item in a project".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "title": {
                    "type": "string",
                    "description": "Work item title"
                },
                "description": {
                    "type": "string",
                    "description": "Work item description (optional)"
                },
                "attachments": {
                    "type": "array",
                    "description": "Uploaded file URLs (optional)",
                    "items": {
                        "type": "string"
                    }
                },
                "state": {
                    "type": "string",
                    "description": "Work item state. Valid values: backlog, todo, in_progress, done",
                    "enum": ["backlog", "todo", "in_progress", "done"],
                    "default": "backlog"
                },
                "priority": {
                    "type": "string",
                    "description": "Work item priority (e.g., 'low', 'medium', 'high', 'critical')",
                    "default": "medium"
                },
                "assignee_id": {
                    "type": "string",
                    "description": "UUID of the assignee (optional)",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "due_at": {
                    "type": "string",
                    "description": "Due date in RFC3339 format (optional)"
                }
            },
            "required": ["project_id", "title"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CreateWorkItemInput {
    project_id: String,
    title: String,
    description: Option<String>,
    attachments: Option<Vec<String>>,
    state: Option<String>,
    priority: Option<String>,
    assignee_id: Option<String>,
    due_at: Option<String>,
}

pub async fn create_work_item(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: CreateWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let state = input.state.unwrap_or_else(|| "backlog".to_string());
    if let Err(e) = validate_work_item_state(&state) {
        return CallToolResult::error(e);
    }

    let description = append_attachments_to_text(input.description, input.attachments);

    let body = json!({
        "title": input.title,
        "description": description,
        "state": state,
        "priority": input.priority.unwrap_or_else(|| "medium".to_string()),
        "assignee_id": input.assignee_id,
        "due_at": input.due_at
    });

    match client.create_work_item(&input.project_id, body).await {
        Ok(item) => {
            let json = serde_json::to_string_pretty(&item).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn update_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.update".to_string(),
        description: "Update an existing work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "title": {
                    "type": "string",
                    "description": "New title (optional)"
                },
                "description": {
                    "type": "string",
                    "description": "New description (optional)"
                },
                "attachments": {
                    "type": "array",
                    "description": "Uploaded file URLs (optional)",
                    "items": {
                        "type": "string"
                    }
                },
                "state": {
                    "type": "string",
                    "description": "New state (optional). Valid values: backlog, todo, in_progress, done",
                    "enum": ["backlog", "todo", "in_progress", "done"]
                },
                "priority": {
                    "type": "string",
                    "description": "New priority (optional)"
                },
                "assignee_id": {
                    "type": "string",
                    "description": "New assignee UUID or null to unassign (optional)",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "due_at": {
                    "type": "string",
                    "description": "New due date in RFC3339 format or null to clear (optional)"
                }
            },
            "required": ["work_item_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateWorkItemInput {
    work_item_id: String,
    title: Option<String>,
    description: Option<String>,
    attachments: Option<Vec<String>>,
    state: Option<String>,
    priority: Option<String>,
    assignee_id: Option<String>,
    due_at: Option<String>,
}

pub async fn update_work_item(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let args_obj = args.as_object().cloned();

    let input: UpdateWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let mut body = serde_json::Map::new();
    if let Some(title) = input.title {
        body.insert("title".to_string(), json!(title));
    }
    if let Some(desc) = append_attachments_to_text(input.description, input.attachments) {
        body.insert("description".to_string(), json!(desc));
    }
    if let Some(state) = input.state {
        if let Err(e) = validate_work_item_state(&state) {
            return CallToolResult::error(e);
        }
        body.insert("state".to_string(), json!(state));
    }
    if let Some(priority) = input.priority {
        body.insert("priority".to_string(), json!(priority));
    }
    if args_obj
        .as_ref()
        .and_then(|o| o.get("assignee_id"))
        .is_some()
    {
        match input.assignee_id.as_deref() {
            Some("") | Some("null") | None => {
                body.insert("assignee_id".to_string(), serde_json::Value::Null);
            }
            Some(aid) => {
                body.insert("assignee_id".to_string(), json!(aid));
            }
        }
    }
    if args_obj.as_ref().and_then(|o| o.get("due_at")).is_some() {
        match input.due_at.as_deref() {
            Some("") | Some("null") | None => {
                body.insert("due_at".to_string(), serde_json::Value::Null);
            }
            Some(d) => {
                body.insert("due_at".to_string(), json!(d));
            }
        }
    }

    match client
        .update_work_item(&input.work_item_id, serde_json::Value::Object(body))
        .await
    {
        Ok(item) => {
            let json = serde_json::to_string_pretty(&item).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn search_work_items_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.search".to_string(),
        description: "Search work items across all projects in a workspace".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (matches title and description)"
                }
            },
            "required": ["query"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct SearchWorkItemsInput {
    query: String,
}

pub async fn search_work_items(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: SearchWorkItemsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.search_work_items(&input.query).await {
        Ok(items) => {
            let json = serde_json::to_string_pretty(&items).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn add_label_to_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.add_label".to_string(),
        description: "Add a label to a work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "label_id": {
                    "type": "string",
                    "description": "UUID of the label",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["work_item_id", "label_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct AddLabelToWorkItemInput {
    work_item_id: String,
    label_id: String,
}

pub async fn add_label_to_work_item(
    client: &OpenPrClient,
    args: serde_json::Value,
) -> CallToolResult {
    let input: AddLabelToWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client
        .add_label_to_issue(&input.work_item_id, &input.label_id)
        .await
    {
        Ok(()) => CallToolResult::success("Label added to work item"),
        Err(e) => CallToolResult::error(e),
    }
}

pub fn remove_label_from_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.remove_label".to_string(),
        description: "Remove a label from a work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "label_id": {
                    "type": "string",
                    "description": "UUID of the label",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["work_item_id", "label_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct RemoveLabelFromWorkItemInput {
    work_item_id: String,
    label_id: String,
}

pub async fn remove_label_from_work_item(
    client: &OpenPrClient,
    args: serde_json::Value,
) -> CallToolResult {
    let input: RemoveLabelFromWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client
        .remove_label_from_issue(&input.work_item_id, &input.label_id)
        .await
    {
        Ok(()) => CallToolResult::success("Label removed from work item"),
        Err(e) => CallToolResult::error(e),
    }
}

pub fn list_work_item_labels_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.list_labels".to_string(),
        description: "List labels of a work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["work_item_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ListWorkItemLabelsInput {
    work_item_id: String,
}

pub async fn list_work_item_labels(
    client: &OpenPrClient,
    args: serde_json::Value,
) -> CallToolResult {
    let input: ListWorkItemLabelsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.get_issue_labels(&input.work_item_id).await {
        Ok(labels) => {
            let json = serde_json::to_string_pretty(&labels).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn delete_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.delete".to_string(),
        description: "Delete a work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["work_item_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct DeleteWorkItemInput {
    work_item_id: String,
}

pub async fn handle_delete_work_item(
    client: &OpenPrClient,
    args: serde_json::Value,
) -> CallToolResult {
    let input: DeleteWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.delete_work_item(&input.work_item_id).await {
        Ok(()) => CallToolResult::success("Work item deleted"),
        Err(e) => CallToolResult::error(e),
    }
}

pub fn add_labels_to_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.add_labels".to_string(),
        description: "Add multiple labels to a work item in one request".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "label_ids": {
                    "type": "array",
                    "description": "List of label UUIDs to add",
                    "items": {
                        "type": "string",
                        "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                    },
                    "minItems": 1
                }
            },
            "required": ["work_item_id", "label_ids"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct AddLabelsToWorkItemInput {
    work_item_id: String,
    label_ids: Vec<String>,
}

pub async fn add_labels_to_work_item(
    client: &OpenPrClient,
    args: serde_json::Value,
) -> CallToolResult {
    let input: AddLabelsToWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client
        .add_labels_to_issue(&input.work_item_id, &input.label_ids)
        .await
    {
        Ok(()) => CallToolResult::success("Labels added to work item"),
        Err(e) => CallToolResult::error(e),
    }
}
