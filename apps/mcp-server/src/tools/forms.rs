use crate::client::{ListRecordsQuery, OpenPrClient};
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

/// Shared contract note for every tool that writes form field values.
const VALUES_CONTRACT: &str = "Field values keyed by field key. Read the target schema with forms.get first. \
amount and number fields must be decimal strings (\"125000.00\"), never JSON numbers; integer, rating and \
progress fields take JSON numbers; multi_select takes a string array; signature takes a string; formula \
fields are server-computed and ignored on write.";

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// Adds an explicit truncation signal to a paginated list envelope so an agent can
/// tell whether it is looking at a complete result set.
fn with_pagination_hint(mut envelope: Value) -> Value {
    let hint = envelope.get("data").and_then(|data| {
        let page = data.get("page").and_then(Value::as_i64)?;
        let per_page = data.get("per_page").and_then(Value::as_i64)?;
        let total = data.get("total").and_then(Value::as_i64)?;
        let total_pages = data.get("total_pages").and_then(Value::as_i64).unwrap_or_default();
        let returned = data.get("items").and_then(Value::as_array).map_or(0, Vec::len);
        Some(json!({
            "returned": returned,
            "total": total,
            "page": page,
            "per_page": per_page,
            "total_pages": total_pages,
            "has_more": page < total_pages,
            "next_page": if page < total_pages { Some(page + 1) } else { None },
        }))
    });

    if let (Some(hint), Some(object)) = (hint, envelope.as_object_mut()) {
        object.insert("pagination_hint".to_string(), hint);
    }
    envelope
}

fn parse_input<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, CallToolResult> {
    serde_json::from_value(args).map_err(|err| CallToolResult::error(format!("Invalid input: {err}")))
}

pub fn list_forms_tool() -> ToolDefinition {
    ToolDefinition {
        name: "forms.list".to_string(),
        description: "List universal forms in a project. Paginated: per_page defaults to 50 (max 200); \
check pagination_hint.has_more before treating the result as complete."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string", "description": "Project UUID" },
                "page": { "type": "integer", "minimum": 1, "description": "1-based page number (default 1)" },
                "per_page": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Page size, default 50, clamped to 200" }
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
        description: "Update a universal form schema and optional title/detail layout. Pass \
expected_schema_version (from forms.get) to avoid silently overwriting a concurrent schema change."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "name": { "type": "string" },
                "description": { "type": "string" },
                "icon": { "type": "string" },
                "color": { "type": "string" },
                "title_template": { "type": "string" },
                "schema": { "type": "object" },
                "detail_layout": { "type": "object" },
                "expected_schema_version": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optimistic concurrency guard: the schema_version returned by forms.get. The update is rejected when the stored version differs."
                }
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
        description: "Upsert form-level role action permission policies for a universal form. At most 20 policies \
per request. Note that bot tokens (including this MCP server) bypass field-level and record-scope policy, so \
field restrictions configured here do not constrain agent access."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "policies": {
                    "type": "array",
                    "maxItems": 20,
                    "items": {
                        "type": "object",
                        "properties": {
                            "subject_type": { "type": "string", "enum": ["role"], "description": "Only role subjects are supported" },
                            "subject_id": { "type": "string", "enum": ["owner", "admin", "member"], "description": "Workspace role the policy applies to" },
                            "policy": {
                                "type": "object",
                                "description": "Policy object. actions maps any of form.view, form.design, record.create, record.update, record.delete, record.export to a boolean; record_scope is \"all\" or \"owned\"; fields maps a field key to { read, write } booleans and accepts at most 200 entries. Example: { \"actions\": { \"form.view\": true, \"record.update\": false }, \"record_scope\": \"owned\", \"fields\": { \"salary\": { \"read\": false } } }",
                                "properties": {
                                    "actions": {
                                        "type": "object",
                                        "additionalProperties": { "type": "boolean" }
                                    },
                                    "record_scope": { "type": "string", "enum": ["all", "owned"] },
                                    "fields": { "type": "object" }
                                }
                            }
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
                "url": { "type": "string" },
                "thumbnail_url": { "type": "string", "description": "Optional thumbnail URL, for example from files.upload" }
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
                "page": { "type": "integer", "minimum": 1, "description": "1-based page number (default 1)" },
                "per_page": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Page size, default 20, clamped to 100" }
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
                "per_page": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Page size, default 50, clamped to 200" }
            },
            "required": ["record_id"]
        }),
    }
}

pub fn create_child_form_record_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.child_create".to_string(),
        description: "Create a child form record and link it to a parent record through a parent_child relation. \
Amount and number fields must use decimal strings, not JSON numbers."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "record_id": { "type": "string", "description": "Parent record UUID" },
                "relation_key": { "type": "string", "description": "Parent relation field key" },
                "child_form_id": { "type": "string", "description": "Child form UUID" },
                "child_form_key": { "type": "string", "description": "Child form key when child_form_id is omitted" },
                "values": { "type": "object", "description": VALUES_CONTRACT },
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
        description: "Update a child form record after verifying it is linked to the parent record. Amount and \
number fields must use decimal strings, not JSON numbers."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "record_id": { "type": "string", "description": "Parent record UUID" },
                "child_record_id": { "type": "string", "description": "Child record UUID" },
                "relation_key": { "type": "string", "description": "Optional parent relation field key" },
                "values": { "type": "object", "description": VALUES_CONTRACT },
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
        description: "List records in a universal form with paging, saved-view, sorting and single-field \
filtering. Returns at most per_page records (default 50, max 200) ordered by updated_at DESC unless sort is \
given; archived records are never returned. The response carries pagination_hint with total, total_pages, \
has_more and next_page: never summarise or count records without checking has_more first."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "page": { "type": "integer", "minimum": 1, "description": "1-based page number (default 1)" },
                "per_page": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Page size, default 50, clamped to 200" },
                "view_id": { "type": "string", "description": "Saved view UUID from form_views.list; its filter and sort apply unless overridden here" },
                "sort": { "type": "string", "description": "field:asc or field:desc. Sortable keys: title, created_at, updated_at, or any indexable schema field key" },
                "filter_field": { "type": "string", "description": "Schema field key to filter on; must be an indexable field" },
                "filter_op": {
                    "type": "string",
                    "enum": ["eq", "neq", "not_equals", "contains", "gt", "gte", "lt", "lte", "not_empty"],
                    "description": "Filter operator; requires filter_field. All operators except not_empty also require filter_value"
                },
                "filter_value": { "type": "string", "description": "Comparison value; decimal fields expect a decimal string" }
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
                "limit": { "type": "integer", "minimum": 1, "maximum": 50000, "description": "Maximum rows to export. Defaults to 5000 when omitted, hard maximum 50000. Compare the row count against forms.schema_summary to detect truncation" }
            },
            "required": ["form_id"]
        }),
    }
}

pub fn preview_import_form_records_tool() -> ToolDefinition {
    ToolDefinition {
        name: "form_records.import_preview".to_string(),
        description: "Preview CSV/JSON-style universal form record import rows without writing records. At most \
500 rows per request. Amount and number fields must use decimal strings, not JSON numbers."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "rows": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 500,
                    "items": {
                        "type": "object",
                        "properties": {
                            "row_number": { "type": "integer", "minimum": 1 },
                            "title": { "type": "string" },
                            "values": { "type": "object", "description": VALUES_CONTRACT },
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
        description: "Commit validated universal form import rows through the normal record create pipeline. At \
most 500 rows per request; any invalid row rejects the whole batch and writes nothing. Amount and number fields \
must use decimal strings, not JSON numbers."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "rows": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 500,
                    "items": {
                        "type": "object",
                        "properties": {
                            "row_number": { "type": "integer", "minimum": 1 },
                            "title": { "type": "string" },
                            "values": { "type": "object", "description": VALUES_CONTRACT },
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
        description: "Create a universal form record. Amount and number fields must use decimal strings, not JSON \
numbers."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "values": { "type": "object", "description": VALUES_CONTRACT },
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
        description: "Update a universal form record. Only the supplied fields change. Amount and number fields \
must use decimal strings, not JSON numbers."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "record_id": { "type": "string" },
                "values": { "type": "object", "description": VALUES_CONTRACT },
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
                "record_id": { "type": "string", "description": "Source form record UUID" },
                "target_type": { "type": "string", "enum": ["form_record", "work_item", "project_resource", "external_object"] },
                "target_id": { "type": "string" },
                "relation_key": { "type": "string" },
                "relation_type": { "type": "string", "enum": ["parent_child", "relation", "external_relation"] },
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
                    "enum": ["sum", "avg", "min", "max", "count"],
                    "description": "Aggregate function, defaults to sum"
                }
            },
            "required": ["form_id", "field_key"]
        }),
    }
}

pub fn events_tail_tool() -> ToolDefinition {
    ToolDefinition {
        name: "events.tail".to_string(),
        description: "Read recent business events for a form or a form record, newest first. Provide exactly one \
of form_id or record_id. Paginated: per_page defaults to 50 (max 200)."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "form_id": { "type": "string" },
                "record_id": { "type": "string" },
                "event_type": {
                    "type": "string",
                    "description": "Exact event type filter such as form_record.created; lowercase dot-separated segments, at most 160 characters"
                },
                "page": { "type": "integer", "minimum": 1, "description": "1-based page number (default 1)" },
                "per_page": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Page size, default 50, clamped to 200" }
            }
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ListFormsInput {
    project_id: String,
    page: Option<u32>,
    per_page: Option<u32>,
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
    icon: Option<String>,
    color: Option<String>,
    title_template: Option<String>,
    schema: Option<Value>,
    detail_layout: Option<Value>,
    expected_schema_version: Option<i32>,
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
    thumbnail_url: Option<String>,
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
    page: Option<u32>,
    per_page: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ListRecordsInput {
    form_id: String,
    page: Option<u32>,
    per_page: Option<u32>,
    view_id: Option<String>,
    sort: Option<String>,
    filter_field: Option<String>,
    filter_op: Option<String>,
    filter_value: Option<String>,
}

pub async fn list_forms(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ListFormsInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client.list_forms(&input.project_id, input.page, input.per_page).await {
        Ok(value) => CallToolResult::success(pretty(&with_pagination_hint(value))),
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
        "icon": input.icon,
        "color": input.color,
        "title_template": input.title_template,
        "schema": input.schema,
        "detail_layout": input.detail_layout,
        "expected_schema_version": input.expected_schema_version
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
        Ok(value) => CallToolResult::success(pretty(&with_pagination_hint(value))),
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
        "url": input.url,
        "thumbnail_url": input.thumbnail_url
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
        Ok(value) => CallToolResult::success(pretty(&value)),
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
        Ok(value) => CallToolResult::success(pretty(&with_pagination_hint(value))),
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
        Ok(value) => CallToolResult::success(pretty(&with_pagination_hint(value))),
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
        Ok(value) => CallToolResult::success(pretty(&value)),
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
    let input: ListRecordsInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let query = ListRecordsQuery {
        page: input.page,
        per_page: input.per_page,
        view_id: input.view_id.as_deref(),
        sort: input.sort.as_deref(),
        filter_field: input.filter_field.as_deref(),
        filter_op: input.filter_op.as_deref(),
        filter_value: input.filter_value.as_deref(),
    };
    match client.list_form_records(&input.form_id, &query).await {
        Ok(value) => CallToolResult::success(pretty(&with_pagination_hint(value))),
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
        (Some(form_id), None) => match client
            .list_form_events(&form_id, input.event_type.as_deref(), input.page, input.per_page)
            .await
        {
            Ok(value) => CallToolResult::success(pretty(&with_pagination_hint(value))),
            Err(err) => CallToolResult::error(err),
        },
        (None, Some(record_id)) => match client
            .list_form_record_events(&record_id, input.event_type.as_deref(), input.page, input.per_page)
            .await
        {
            Ok(value) => CallToolResult::success(pretty(&with_pagination_hint(value))),
            Err(err) => CallToolResult::error(err),
        },
        _ => CallToolResult::error("Provide exactly one of form_id or record_id".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        aggregate_form_records_tool, archive_child_form_record, archive_form_attachment, create_child_form_record_tool,
        create_form_attachment_tool, create_form_record_tool, events_tail_tool, export_form_records_tool,
        import_form_records_tool, list_form_records, list_form_records_tool, preview_import_form_records_tool,
        update_child_form_record_tool, update_form_permissions_tool, update_form_record_tool, update_form_schema_tool,
        with_pagination_hint,
    };
    use crate::client::{OpenPrClient, test_api};
    use axum::{
        Json, Router,
        routing::{delete, get},
    };
    use serde_json::{Value, json};
    use std::error::Error;

    fn schema_property<'a>(schema: &'a Value, name: &str) -> Option<&'a Value> {
        schema.get("properties").and_then(|props| props.get(name))
    }

    fn client(base_url: String) -> Result<OpenPrClient, String> {
        OpenPrClient::new(base_url, "opr_test_token".to_string(), "workspace-1".to_string())
    }

    fn result_text(result: &crate::protocol::CallToolResult) -> String {
        result
            .content
            .iter()
            .find_map(|item| match item {
                crate::protocol::ToolContent::Text { text } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    #[test]
    fn list_records_schema_exposes_every_backend_query_parameter() {
        let schema = list_form_records_tool().input_schema;

        for property in [
            "page",
            "per_page",
            "view_id",
            "sort",
            "filter_field",
            "filter_op",
            "filter_value",
        ] {
            assert!(
                schema_property(&schema, property).is_some(),
                "form_records.list must expose {property}"
            );
        }
        assert_eq!(
            schema_property(&schema, "per_page")
                .and_then(|value| value.get("maximum"))
                .and_then(Value::as_i64),
            Some(200)
        );
    }

    #[test]
    fn permission_subject_enums_match_backend_accepted_values() {
        let schema = update_form_permissions_tool().input_schema;
        let policy_item = schema
            .get("properties")
            .and_then(|props| props.get("policies"))
            .and_then(|policies| policies.get("items"))
            .and_then(|items| items.get("properties"))
            .cloned()
            .unwrap_or_else(|| json!({}));

        let subject_ids = policy_item
            .get("subject_id")
            .and_then(|subject| subject.get("enum"))
            .cloned()
            .unwrap_or_else(|| json!([]));
        assert_eq!(subject_ids, json!(["owner", "admin", "member"]));

        let subject_types = policy_item
            .get("subject_type")
            .and_then(|subject| subject.get("enum"))
            .cloned()
            .unwrap_or_else(|| json!([]));
        assert_eq!(subject_types, json!(["role"]));
    }

    #[test]
    fn write_tools_document_the_decimal_string_contract() {
        for tool in [
            create_form_record_tool(),
            update_form_record_tool(),
            create_child_form_record_tool(),
            update_child_form_record_tool(),
            preview_import_form_records_tool(),
            import_form_records_tool(),
        ] {
            assert!(
                tool.description.contains("decimal strings"),
                "{} must document the decimal string rule",
                tool.name
            );
        }
    }

    #[test]
    fn export_tool_documents_the_default_row_limit() {
        let schema = export_form_records_tool().input_schema;
        let description = schema_property(&schema, "limit")
            .and_then(|limit| limit.get("description"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        assert!(description.contains("5000"), "{description}");
    }

    #[test]
    fn schema_update_and_attachment_tools_expose_all_backend_fields() {
        let update_schema = update_form_schema_tool().input_schema;
        for property in ["icon", "color", "expected_schema_version"] {
            assert!(schema_property(&update_schema, property).is_some(), "{property}");
        }

        let attachment_schema = create_form_attachment_tool().input_schema;
        assert!(schema_property(&attachment_schema, "thumbnail_url").is_some());
    }

    #[test]
    fn enumerated_tools_constrain_backend_validated_values() {
        let aggregate = aggregate_form_records_tool().input_schema;
        assert_eq!(
            schema_property(&aggregate, "aggregate")
                .and_then(|value| value.get("enum"))
                .cloned(),
            Some(json!(["sum", "avg", "min", "max", "count"]))
        );

        let events = events_tail_tool().input_schema;
        assert!(schema_property(&events, "page").is_some());
        assert!(schema_property(&events, "per_page").is_some());
    }

    #[test]
    fn pagination_hint_reports_truncation() {
        let hinted = with_pagination_hint(json!({
            "code": 0,
            "data": { "items": [1, 2], "total": 120, "page": 1, "per_page": 50, "total_pages": 3 }
        }));
        let hint = hinted.get("pagination_hint").cloned().unwrap_or_else(|| json!({}));

        assert_eq!(hint.get("has_more").and_then(Value::as_bool), Some(true));
        assert_eq!(hint.get("next_page").and_then(Value::as_i64), Some(2));
        assert_eq!(hint.get("returned").and_then(Value::as_i64), Some(2));
    }

    #[test]
    fn pagination_hint_is_absent_for_non_paginated_payloads() {
        let hinted = with_pagination_hint(json!({ "code": 0, "data": { "id": "f1" } }));

        assert!(hinted.get("pagination_hint").is_none());
    }

    #[tokio::test]
    async fn archive_tools_report_backend_errors_instead_of_fake_success() -> Result<(), Box<dyn Error>> {
        async fn forbidden() -> Json<Value> {
            Json(json!({ "code": 403, "message": "record.delete denied", "data": null }))
        }
        let router = Router::new()
            .route("/api/v1/form-attachments/a1", delete(forbidden))
            .route("/api/v1/form-records/r1/children/c1", delete(forbidden));
        let client = client(test_api::spawn(router).await?)?;

        let attachment = archive_form_attachment(&client, json!({ "attachment_id": "a1" })).await;
        let child = archive_child_form_record(&client, json!({ "record_id": "r1", "child_record_id": "c1" })).await;

        for result in [attachment, child] {
            assert_eq!(result.is_error, Some(true), "{}", result_text(&result));
            let text = result_text(&result);
            assert!(text.contains("403"), "{text}");
            assert!(!text.contains("\"archived\": true"), "{text}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn archive_attachment_passes_backend_payload_through_on_success() -> Result<(), Box<dyn Error>> {
        let router = Router::new().route(
            "/api/v1/form-attachments/a1",
            delete(|| async { Json(json!({ "code": 0, "message": "success", "data": { "archived_at": "now" } })) }),
        );
        let client = client(test_api::spawn(router).await?)?;

        let result = archive_form_attachment(&client, json!({ "attachment_id": "a1" })).await;

        assert_eq!(result.is_error, None);
        assert!(result_text(&result).contains("archived_at"));
        Ok(())
    }

    #[tokio::test]
    async fn record_listing_adds_pagination_hint_for_agents() -> Result<(), Box<dyn Error>> {
        let router = Router::new().route(
            "/api/v1/forms/f1/records",
            get(|| async {
                Json(json!({
                    "code": 0,
                    "data": { "items": [], "total": 5000, "page": 1, "per_page": 50, "total_pages": 100 }
                }))
            }),
        );
        let client = client(test_api::spawn(router).await?)?;

        let result = list_form_records(&client, json!({ "form_id": "f1" })).await;
        let text = result_text(&result);

        assert_eq!(result.is_error, None);
        assert!(text.contains("pagination_hint"), "{text}");
        assert!(text.contains("\"has_more\": true"), "{text}");
        Ok(())
    }

    #[tokio::test]
    async fn record_listing_reports_api_errors() -> Result<(), Box<dyn Error>> {
        let router = Router::new().route(
            "/api/v1/forms/f1/records",
            get(|| async { Json(json!({ "code": 404, "message": "form not found", "data": null })) }),
        );
        let client = client(test_api::spawn(router).await?)?;

        let result = list_form_records(&client, json!({ "form_id": "f1" })).await;

        assert_eq!(result.is_error, Some(true));
        assert!(result_text(&result).contains("form not found"));
        Ok(())
    }
}
