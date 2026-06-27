use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

fn parse_input<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, CallToolResult> {
    serde_json::from_value(args).map_err(|err| CallToolResult::error(format!("Invalid input: {err}")))
}

pub fn list_forms_tool() -> ToolDefinition {
    ToolDefinition {
        name: "forms.list".to_string(),
        description: "List universal forms in a project".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string", "description": "Project UUID" }
            },
            "required": ["project_id"]
        }),
    }
}

pub fn get_form_tool() -> ToolDefinition {
    ToolDefinition {
        name: "forms.get".to_string(),
        description: "Get one universal form schema by UUID".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string", "description": "Form UUID" }
            },
            "required": ["form_id"]
        }),
    }
}

pub fn create_form_tool() -> ToolDefinition {
    ToolDefinition {
        name: "forms.create".to_string(),
        description: "Create a universal form in a project".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string" },
                "key": { "type": "string" },
                "name": { "type": "string" },
                "description": { "type": "string" },
                "icon": { "type": "string" },
                "color": { "type": "string" },
                "title_template": { "type": "string" },
                "schema": { "type": "object" },
                "detail_layout": { "type": "object" }
            },
            "required": ["project_id", "key", "name"]
        }),
    }
}

pub fn create_form_from_template_tool() -> ToolDefinition {
    ToolDefinition {
        name: "forms.create_from_template".to_string(),
        description: "Create one universal form from a scenario template inside an existing project".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string" },
                "template_key": { "type": "string", "description": "Scenario template key, for example restaurant_ordering_default" },
                "key": { "type": "string", "description": "Optional form key override" },
                "name": { "type": "string", "description": "Optional form name override" },
                "description": { "type": "string", "description": "Optional description override" },
                "title_template": { "type": "string", "description": "Optional title template override" }
            },
            "required": ["project_id", "template_key"]
        }),
    }
}

pub fn update_form_schema_tool() -> ToolDefinition {
    ToolDefinition {
        name: "forms.update_schema".to_string(),
        description: "Update a universal form schema and optional title/detail layout".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "name": { "type": "string" },
                "description": { "type": "string" },
                "title_template": { "type": "string" },
                "schema": { "type": "object" },
                "detail_layout": { "type": "object" }
            },
            "required": ["form_id"]
        }),
    }
}

pub fn duplicate_form_tool() -> ToolDefinition {
    ToolDefinition {
        name: "forms.duplicate".to_string(),
        description:
            "Duplicate a universal form schema, detail layout, metadata, and active views without copying records"
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "key": { "type": "string", "description": "Optional explicit key for the duplicate" },
                "name": { "type": "string", "description": "Optional explicit name for the duplicate" },
                "description": { "type": "string", "description": "Optional explicit description for the duplicate" }
            },
            "required": ["form_id"]
        }),
    }
}

pub fn get_form_schema_summary_tool() -> ToolDefinition {
    ToolDefinition {
        name: "forms.schema_summary".to_string(),
        description: "Get schema, view, record, and archive summary metadata for a universal form".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" }
            },
            "required": ["form_id"]
        }),
    }
}

pub fn get_form_field_usage_tool() -> ToolDefinition {
    ToolDefinition {
        name: "forms.field_usage".to_string(),
        description: "Get value usage and dependency counts for fields in a universal form".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" }
            },
            "required": ["form_id"]
        }),
    }
}

pub fn get_form_field_dependencies_tool() -> ToolDefinition {
    ToolDefinition {
        name: "forms.field_dependencies".to_string(),
        description: "List view, formula, relation, and title-template dependencies for form fields".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" }
            },
            "required": ["form_id"]
        }),
    }
}

pub fn list_form_schema_versions_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_schema_versions.list".to_string(),
        description: "List archived schema versions for a universal form".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" }
            },
            "required": ["form_id"]
        }),
    }
}

pub fn get_form_schema_version_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_schema_versions.get".to_string(),
        description: "Get one archived schema version for a universal form".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "version": { "type": "integer", "minimum": 1 }
            },
            "required": ["form_id", "version"]
        }),
    }
}

pub fn get_form_permissions_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_permissions.get".to_string(),
        description: "Get form-level role action permission policies and the caller's effective actions".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" }
            },
            "required": ["form_id"]
        }),
    }
}

pub fn update_form_permissions_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_permissions.update".to_string(),
        description: "Upsert form-level role action permission policies for a universal form".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "policies": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "subject_type": { "type": "string", "description": "Currently role" },
                            "subject_id": { "type": "string", "description": "owner, admin, member, or viewer" },
                            "policy": { "type": "object", "description": "Policy object, for example { actions: { form.view: true } }" }
                        },
                        "required": ["subject_type", "subject_id", "policy"]
                    }
                }
            },
            "required": ["form_id", "policies"]
        }),
    }
}

pub fn list_form_views_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_views.list".to_string(),
        description: "List configured views for a universal form".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" }
            },
            "required": ["form_id"]
        }),
    }
}

pub fn list_form_attachments_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_attachments.list".to_string(),
        description: "List attachment or image metadata rows for a universal form".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "record_id": { "type": "string" },
                "field_key": { "type": "string" },
                "include_archived": { "type": "boolean" },
                "page": { "type": "integer", "minimum": 1 },
                "per_page": { "type": "integer", "minimum": 1, "maximum": 200 }
            },
            "required": ["form_id"]
        }),
    }
}

pub fn create_form_attachment_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_attachments.create".to_string(),
        description: "Create attachment or image metadata for a form field. This does not upload file bytes."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "field_key": { "type": "string" },
                "record_id": { "type": "string" },
                "file_name": { "type": "string" },
                "content_type": { "type": "string" },
                "byte_size": { "type": "integer", "minimum": 0 },
                "storage_key": { "type": "string" },
                "url": { "type": "string" }
            },
            "required": ["form_id", "field_key", "file_name", "storage_key"]
        }),
    }
}

pub fn archive_form_attachment_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_attachments.archive".to_string(),
        description: "Archive one form attachment metadata row".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "attachment_id": { "type": "string" }
            },
            "required": ["attachment_id"]
        }),
    }
}

pub fn restore_form_attachment_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_attachments.restore".to_string(),
        description: "Restore one archived form attachment metadata row".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "attachment_id": { "type": "string" }
            },
            "required": ["attachment_id"]
        }),
    }
}

pub fn list_form_relation_targets_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.relation_targets".to_string(),
        description: "List selectable target records for a relation field in a universal form".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "field_key": { "type": "string", "description": "Relation field key on the source form" },
                "form_key": { "type": "string", "description": "Explicit target form key when no field_key is supplied" },
                "q": { "type": "string", "description": "Search text" },
                "page": { "type": "integer", "minimum": 1 },
                "per_page": { "type": "integer", "minimum": 1, "maximum": 100 }
            },
            "required": ["form_id"]
        }),
    }
}

pub fn list_form_record_children_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.children".to_string(),
        description: "List child records linked to a parent form record".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "record_id": { "type": "string" },
                "relation_key": { "type": "string" },
                "child_form_key": { "type": "string" },
                "page": { "type": "integer", "minimum": 1 },
                "per_page": { "type": "integer", "minimum": 1, "maximum": 100 }
            },
            "required": ["record_id"]
        }),
    }
}

pub fn create_child_form_record_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.child_create".to_string(),
        description: "Create a child form record and link it to a parent record through a parent_child relation"
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "record_id": { "type": "string", "description": "Parent record UUID" },
                "relation_key": { "type": "string", "description": "Parent relation field key" },
                "child_form_id": { "type": "string", "description": "Child form UUID" },
                "child_form_key": { "type": "string", "description": "Child form key when child_form_id is omitted" },
                "values": { "type": "object" },
                "title": { "type": "string" },
                "source": { "type": "object" },
                "metadata": { "type": "object" }
            },
            "required": ["record_id", "relation_key", "values"]
        }),
    }
}

pub fn update_child_form_record_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.child_update".to_string(),
        description: "Update a child form record after verifying it is linked to the parent record".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "record_id": { "type": "string", "description": "Parent record UUID" },
                "child_record_id": { "type": "string", "description": "Child record UUID" },
                "relation_key": { "type": "string", "description": "Optional parent relation field key" },
                "values": { "type": "object" },
                "title": { "type": "string" },
                "source": { "type": "object" }
            },
            "required": ["record_id", "child_record_id"]
        }),
    }
}

pub fn archive_child_form_record_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.child_archive".to_string(),
        description: "Archive a child form record after verifying it is linked to the parent record".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "record_id": { "type": "string", "description": "Parent record UUID" },
                "child_record_id": { "type": "string", "description": "Child record UUID" },
                "relation_key": { "type": "string", "description": "Optional parent relation field key" }
            },
            "required": ["record_id", "child_record_id"]
        }),
    }
}

pub fn restore_child_form_record_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.child_restore".to_string(),
        description: "Restore an archived child form record after verifying it is linked to the parent record"
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "record_id": { "type": "string", "description": "Parent record UUID" },
                "child_record_id": { "type": "string", "description": "Child record UUID" },
                "relation_key": { "type": "string", "description": "Optional parent relation field key" }
            },
            "required": ["record_id", "child_record_id"]
        }),
    }
}

pub fn list_form_records_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.list".to_string(),
        description: "List records in a universal form".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" }
            },
            "required": ["form_id"]
        }),
    }
}

pub fn export_form_records_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.export".to_string(),
        description: "Export universal form records as current-view or explicit-column CSV/row data".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "view_id": { "type": "string", "description": "Optional saved view UUID for column selection" },
                "columns": { "type": "string", "description": "Optional comma-separated field keys" },
                "format": { "type": "string", "enum": ["csv", "json"], "description": "Export payload format" },
                "scope": { "type": "string", "enum": ["view", "all_accessible"], "description": "Use saved-view filters or all accessible records" },
                "include_archived": { "type": "boolean", "description": "Include archived records; requires form admin scope" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 50000, "description": "Maximum rows to export" }
            },
            "required": ["form_id"]
        }),
    }
}

pub fn preview_import_form_records_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.import_preview".to_string(),
        description: "Preview CSV/JSON-style universal form record import rows without writing records".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "rows": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "row_number": { "type": "integer", "minimum": 1 },
                            "title": { "type": "string" },
                            "values": { "type": "object" },
                            "source": { "type": "object" }
                        },
                        "required": ["values"]
                    }
                }
            },
            "required": ["form_id", "rows"]
        }),
    }
}

pub fn import_form_records_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.import_commit".to_string(),
        description: "Commit validated universal form import rows through the normal record create pipeline"
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "rows": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "row_number": { "type": "integer", "minimum": 1 },
                            "title": { "type": "string" },
                            "values": { "type": "object" },
                            "source": { "type": "object" },
                            "idempotency_key": { "type": "string" }
                        },
                        "required": ["values"]
                    }
                },
                "idempotency_key": {
                    "type": "string",
                    "description": "Optional batch receipt key; rows derive <key>:row:<row_number> unless a row key is supplied"
                }
            },
            "required": ["form_id", "rows"]
        }),
    }
}

pub fn get_form_record_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.get".to_string(),
        description: "Get one universal form record by UUID".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "record_id": { "type": "string" }
            },
            "required": ["record_id"]
        }),
    }
}

pub fn create_form_record_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.create".to_string(),
        description: "Create a universal form record. Amount fields must use decimal strings, not JSON numbers."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "values": { "type": "object" },
                "title": { "type": "string" },
                "source": { "type": "object" },
                "idempotency_key": {
                    "type": "string",
                    "description": "Optional write receipt key for retry-safe automation"
                }
            },
            "required": ["form_id", "values"]
        }),
    }
}

pub fn update_form_record_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.update".to_string(),
        description: "Update a universal form record".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "record_id": { "type": "string" },
                "values": { "type": "object" },
                "title": { "type": "string" },
                "source": { "type": "object" },
                "idempotency_key": {
                    "type": "string",
                    "description": "Optional write receipt key for retry-safe automation"
                }
            },
            "required": ["record_id"]
        }),
    }
}

pub fn link_form_record_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.link".to_string(),
        description: "Link a form record to another form record, work item, project resource, or external object"
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "record_id": { "type": "string" },
                "target_type": { "type": "string" },
                "target_id": { "type": "string" },
                "relation_key": { "type": "string" },
                "relation_type": { "type": "string" },
                "metadata": { "type": "object" }
            },
            "required": ["record_id", "target_type", "target_id", "relation_key", "relation_type"]
        }),
    }
}

pub fn aggregate_form_records_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.aggregate".to_string(),
        description:
            "Aggregate a numeric, integer, or amount field using projection indexes. Results use decimal strings."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "field_key": { "type": "string" },
                "aggregate": {
                    "type": "string",
                    "description": "sum, avg, min, max, or count"
                }
            },
            "required": ["form_id", "field_key"]
        }),
    }
}

pub fn events_tail_tool() -> ToolDefinition {
    ToolDefinition {
        name: "events.tail".to_string(),
        description: "Read recent business events for a form or a form record".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "record_id": { "type": "string" },
                "event_type": { "type": "string" }
            }
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ProjectInput {
    project_id: String,
}

#[derive(Debug, Deserialize)]
struct FormInput {
    form_id: String,
}

#[derive(Debug, Deserialize)]
struct FormSchemaVersionInput {
    form_id: String,
    version: i32,
}

#[derive(Debug, Deserialize)]
struct RecordInput {
    record_id: String,
}

#[derive(Debug, Deserialize)]
struct CreateFormInput {
    project_id: String,
    key: String,
    name: String,
    description: Option<String>,
    icon: Option<String>,
    color: Option<String>,
    title_template: Option<String>,
    schema: Option<Value>,
    detail_layout: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct CreateFormFromTemplateInput {
    project_id: String,
    template_key: String,
    key: Option<String>,
    name: Option<String>,
    description: Option<String>,
    title_template: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateFormSchemaInput {
    form_id: String,
    name: Option<String>,
    description: Option<String>,
    title_template: Option<String>,
    schema: Option<Value>,
    detail_layout: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct DuplicateFormInput {
    form_id: String,
    key: Option<String>,
    name: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdatePermissionsInput {
    form_id: String,
    policies: Value,
}

#[derive(Debug, Deserialize)]
struct ListAttachmentsInput {
    form_id: String,
    record_id: Option<String>,
    field_key: Option<String>,
    include_archived: Option<bool>,
    page: Option<u32>,
    per_page: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct CreateAttachmentInput {
    form_id: String,
    field_key: String,
    record_id: Option<String>,
    file_name: String,
    content_type: Option<String>,
    byte_size: Option<i64>,
    storage_key: String,
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AttachmentInput {
    attachment_id: String,
}

#[derive(Debug, Deserialize)]
struct RelationTargetsInput {
    form_id: String,
    field_key: Option<String>,
    form_key: Option<String>,
    q: Option<String>,
    page: Option<u32>,
    per_page: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct RecordChildrenInput {
    record_id: String,
    relation_key: Option<String>,
    child_form_key: Option<String>,
    page: Option<u32>,
    per_page: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct CreateChildRecordInput {
    record_id: String,
    relation_key: String,
    child_form_id: Option<String>,
    child_form_key: Option<String>,
    values: Value,
    title: Option<String>,
    source: Option<Value>,
    metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct UpdateChildRecordInput {
    record_id: String,
    child_record_id: String,
    relation_key: Option<String>,
    values: Option<Value>,
    title: Option<String>,
    source: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ChildRecordMutationInput {
    record_id: String,
    child_record_id: String,
    relation_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateRecordInput {
    form_id: String,
    values: Value,
    title: Option<String>,
    source: Option<Value>,
    idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExportRecordsInput {
    form_id: String,
    view_id: Option<String>,
    columns: Option<String>,
    format: Option<String>,
    scope: Option<String>,
    include_archived: Option<bool>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ImportRecordsInput {
    form_id: String,
    rows: Value,
    idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateRecordInput {
    record_id: String,
    values: Option<Value>,
    title: Option<String>,
    source: Option<Value>,
    idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LinkRecordInput {
    record_id: String,
    target_type: String,
    target_id: String,
    relation_key: String,
    relation_type: String,
    metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct AggregateInput {
    form_id: String,
    field_key: String,
    aggregate: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EventsTailInput {
    form_id: Option<String>,
    record_id: Option<String>,
    event_type: Option<String>,
}

pub async fn list_forms(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ProjectInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client.list_forms(&input.project_id).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn get_form(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: FormInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client.get_form(&input.form_id).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn create_form(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: CreateFormInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({
        "key": input.key,
        "name": input.name,
        "description": input.description,
        "icon": input.icon,
        "color": input.color,
        "title_template": input.title_template,
        "schema": input.schema,
        "detail_layout": input.detail_layout
    });
    match client.create_form(&input.project_id, body).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn create_form_from_template(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: CreateFormFromTemplateInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({
        "template_key": input.template_key,
        "key": input.key,
        "name": input.name,
        "description": input.description,
        "title_template": input.title_template
    });
    match client.create_form_from_template(&input.project_id, body).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn update_form_schema(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: UpdateFormSchemaInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({
        "name": input.name,
        "description": input.description,
        "title_template": input.title_template,
        "schema": input.schema,
        "detail_layout": input.detail_layout
    });
    match client.update_form_schema(&input.form_id, body).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn duplicate_form(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: DuplicateFormInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({
        "key": input.key,
        "name": input.name,
        "description": input.description
    });
    match client.duplicate_form(&input.form_id, body).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn get_form_schema_summary(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: FormInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client.get_form_schema_summary(&input.form_id).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn get_form_field_usage(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: FormInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client.get_form_field_usage(&input.form_id).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn get_form_field_dependencies(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: FormInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client.get_form_field_dependencies(&input.form_id).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn list_form_schema_versions(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: FormInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client.list_form_schema_versions(&input.form_id).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn get_form_schema_version(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: FormSchemaVersionInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client.get_form_schema_version(&input.form_id, input.version).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn get_form_permissions(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: FormInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client.get_form_permissions(&input.form_id).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn update_form_permissions(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: UpdatePermissionsInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({ "policies": input.policies });
    match client.update_form_permissions(&input.form_id, body).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn list_form_views(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: FormInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client.list_form_views(&input.form_id).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn list_form_attachments(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ListAttachmentsInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client
        .list_form_attachments(
            &input.form_id,
            input.record_id.as_deref(),
            input.field_key.as_deref(),
            input.include_archived,
            input.page,
            input.per_page,
        )
        .await
    {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn create_form_attachment(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: CreateAttachmentInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({
        "field_key": input.field_key,
        "record_id": input.record_id,
        "file_name": input.file_name,
        "content_type": input.content_type,
        "byte_size": input.byte_size,
        "storage_key": input.storage_key,
        "url": input.url
    });
    match client.create_form_attachment(&input.form_id, body).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn archive_form_attachment(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: AttachmentInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client.archive_form_attachment(&input.attachment_id).await {
        Ok(()) => CallToolResult::success(pretty(&json!({
            "success": true,
            "attachment_id": input.attachment_id,
            "archived": true
        }))),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn restore_form_attachment(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: AttachmentInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client.restore_form_attachment(&input.attachment_id).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn list_form_relation_targets(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: RelationTargetsInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client
        .list_form_relation_targets(
            &input.form_id,
            input.field_key.as_deref(),
            input.form_key.as_deref(),
            input.q.as_deref(),
            input.page,
            input.per_page,
        )
        .await
    {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn list_form_record_children(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: RecordChildrenInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client
        .list_form_record_children(
            &input.record_id,
            input.relation_key.as_deref(),
            input.child_form_key.as_deref(),
            input.page,
            input.per_page,
        )
        .await
    {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn create_child_form_record(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: CreateChildRecordInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({
        "child_form_id": input.child_form_id,
        "child_form_key": input.child_form_key,
        "relation_key": input.relation_key,
        "values": input.values,
        "title": input.title,
        "source": input.source,
        "metadata": input.metadata
    });
    match client.create_child_form_record(&input.record_id, body).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn update_child_form_record(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: UpdateChildRecordInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({
        "relation_key": input.relation_key,
        "values": input.values,
        "title": input.title,
        "source": input.source
    });
    match client
        .update_child_form_record(&input.record_id, &input.child_record_id, body)
        .await
    {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn archive_child_form_record(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ChildRecordMutationInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client
        .archive_child_form_record(&input.record_id, &input.child_record_id, input.relation_key.as_deref())
        .await
    {
        Ok(()) => CallToolResult::success(pretty(&json!({
            "record_id": input.record_id,
            "child_record_id": input.child_record_id,
            "archived": true
        }))),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn restore_child_form_record(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ChildRecordMutationInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client
        .restore_child_form_record(&input.record_id, &input.child_record_id, input.relation_key.as_deref())
        .await
    {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn list_form_records(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: FormInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client.list_form_records(&input.form_id).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn export_form_records(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ExportRecordsInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client
        .export_form_records(
            &input.form_id,
            input.view_id.as_deref(),
            input.columns.as_deref(),
            input.format.as_deref(),
            input.scope.as_deref(),
            input.include_archived,
            input.limit,
        )
        .await
    {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn preview_import_form_records(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ImportRecordsInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({ "rows": input.rows });
    match client.preview_import_form_records(&input.form_id, body).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn import_form_records(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ImportRecordsInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({ "rows": input.rows, "idempotency_key": input.idempotency_key });
    match client.import_form_records(&input.form_id, body).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn get_form_record(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: RecordInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client.get_form_record(&input.record_id).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn create_form_record(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: CreateRecordInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({
        "values": input.values,
        "title": input.title,
        "source": input.source,
        "idempotency_key": input.idempotency_key
    });
    match client.create_form_record(&input.form_id, body).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn update_form_record(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: UpdateRecordInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({
        "values": input.values,
        "title": input.title,
        "source": input.source,
        "idempotency_key": input.idempotency_key
    });
    match client.update_form_record(&input.record_id, body).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn link_form_record(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: LinkRecordInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({
        "target_type": input.target_type,
        "target_id": input.target_id,
        "relation_key": input.relation_key,
        "relation_type": input.relation_type,
        "metadata": input.metadata
    });
    match client.link_form_record(&input.record_id, body).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn aggregate_form_records(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: AggregateInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let aggregate = input.aggregate.unwrap_or_else(|| "sum".to_string());
    match client
        .aggregate_form_records(&input.form_id, &input.field_key, &aggregate)
        .await
    {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn events_tail(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: EventsTailInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match (input.form_id, input.record_id) {
        (Some(form_id), None) => match client.list_form_events(&form_id, input.event_type.as_deref()).await {
            Ok(value) => CallToolResult::success(pretty(&value)),
            Err(err) => CallToolResult::error(err),
        },
        (None, Some(record_id)) => match client
            .list_form_record_events(&record_id, input.event_type.as_deref())
            .await
        {
            Ok(value) => CallToolResult::success(pretty(&value)),
            Err(err) => CallToolResult::error(err),
        },
        _ => CallToolResult::error("Provide exactly one of form_id or record_id".to_string()),
    }
}
