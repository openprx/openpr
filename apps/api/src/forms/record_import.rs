use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{error::ApiError, forms::schema::FormField};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ImportRecordInput {
    pub row_number: Option<usize>,
    pub title: Option<String>,
    pub values: Value,
    pub source: Option<Value>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ImportRecordsRequest {
    pub rows: Vec<ImportRecordInput>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ImportRecordsFileRequest {
    pub file_url: Option<String>,
    pub upload_url: Option<String>,
    pub mapping_template_id: Option<Uuid>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateImportJobRequest {
    pub rows: Option<Vec<ImportRecordInput>>,
    pub file_url: Option<String>,
    pub upload_url: Option<String>,
    pub mapping_template_id: Option<Uuid>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListImportMappingTemplatesQuery {
    pub header_signature: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SaveImportMappingTemplateRequest {
    pub id: Option<Uuid>,
    pub name: Option<String>,
    pub header_signature: Option<String>,
    pub headers: Vec<String>,
    pub selections: Vec<String>,
    pub transforms: Vec<String>,
    pub shared: Option<bool>,
}

#[derive(Debug, Serialize, Clone)]
pub struct ImportRecordPreviewRow {
    pub row_number: usize,
    pub valid: bool,
    pub title: Option<String>,
    pub normalized_values: Option<Value>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ImportRecordsPreviewResponse {
    pub form_id: Uuid,
    pub schema_version: i32,
    pub total_rows: usize,
    pub valid_rows: usize,
    pub invalid_rows: usize,
    pub rows: Vec<ImportRecordPreviewRow>,
}

pub struct NormalizedImportMappingTemplate {
    pub id: Option<Uuid>,
    pub name: String,
    pub header_signature: String,
    pub headers: Value,
    pub selections: Value,
    pub transforms: Value,
    pub shared: bool,
}

pub struct ImportMappingTemplateConfig {
    pub header_signature: String,
    pub headers: Vec<String>,
    pub selections: Vec<String>,
    pub transforms: Vec<String>,
}

pub fn normalize_import_mapping_template_request(
    req: SaveImportMappingTemplateRequest,
    fields: &[FormField],
) -> Result<NormalizedImportMappingTemplate, ApiError> {
    if req.headers.is_empty() {
        return Err(ApiError::BadRequest("headers are required".to_string()));
    }
    if req.headers.len() > 100 {
        return Err(ApiError::BadRequest("headers cannot exceed 100 columns".to_string()));
    }
    if req.selections.len() != req.headers.len() {
        return Err(ApiError::BadRequest(
            "selections length must match headers length".to_string(),
        ));
    }
    if req.transforms.len() != req.headers.len() {
        return Err(ApiError::BadRequest(
            "transforms length must match headers length".to_string(),
        ));
    }

    let headers = req
        .headers
        .into_iter()
        .map(|header| header.trim().to_string())
        .collect::<Vec<_>>();
    let selections = req
        .selections
        .into_iter()
        .map(|selection| normalize_import_mapping_selection(&selection, fields))
        .collect::<Result<Vec<_>, _>>()?;
    let transforms = req
        .transforms
        .into_iter()
        .map(|transform| normalize_import_mapping_transform(&transform))
        .collect::<Result<Vec<_>, _>>()?;
    let header_signature = req
        .header_signature
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| import_mapping_header_signature(&headers));
    if header_signature.len() > 2000 {
        return Err(ApiError::BadRequest(
            "header_signature cannot exceed 2000 characters".to_string(),
        ));
    }

    Ok(NormalizedImportMappingTemplate {
        id: req.id,
        name: req
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Import mapping")
            .chars()
            .take(120)
            .collect(),
        header_signature,
        headers: json!(headers),
        selections: json!(selections),
        transforms: json!(transforms),
        shared: req.shared.unwrap_or(true),
    })
}

pub fn import_mapping_header_signature(headers: &[String]) -> String {
    headers.join("\u{1f}")
}

fn normalize_import_mapping_selection(selection: &str, fields: &[FormField]) -> Result<String, ApiError> {
    let selection = selection.trim();
    if selection == "__skip" || selection == "__title" {
        return Ok(selection.to_string());
    }
    fields
        .iter()
        .find(|field| field.key == selection)
        .map(|field| field.key.clone())
        .ok_or_else(|| ApiError::BadRequest(format!("invalid import mapping selection: {selection}")))
}

fn normalize_import_mapping_transform(transform: &str) -> Result<String, ApiError> {
    match transform.trim() {
        "none" | "trim" | "uppercase" | "lowercase" | "capitalize" | "titlecase" | "normalize_spaces"
        | "strip_non_digits" | "slugify" => Ok(transform.trim().to_string()),
        other => Err(ApiError::BadRequest(format!(
            "invalid import mapping transform: {other}"
        ))),
    }
}

pub fn create_import_job_input(req: &CreateImportJobRequest) -> Result<Value, ApiError> {
    let has_rows = req.rows.as_ref().is_some_and(|rows| !rows.is_empty());
    let file_url = req.file_url.as_deref().or(req.upload_url.as_deref());
    let has_file = file_url.is_some_and(|value| !value.trim().is_empty());
    if has_rows == has_file {
        return Err(ApiError::BadRequest(
            "import job requires exactly one of rows or file_url".to_string(),
        ));
    }
    serde_json::to_value(req).map_err(|_| ApiError::Internal)
}

pub fn json_string_array(value: &Value, field: &str) -> Result<Vec<String>, ApiError> {
    value
        .as_array()
        .ok_or_else(|| ApiError::BadRequest(format!("{field} must be an array")))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| ApiError::BadRequest(format!("{field} must contain strings")))
        })
        .collect()
}

pub fn import_rows_from_file_text(
    fields: &[FormField],
    text: &str,
    file_url: &str,
    mapping: Option<&ImportMappingTemplateConfig>,
) -> Result<Vec<ImportRecordInput>, ApiError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(ApiError::BadRequest("import file is empty".to_string()));
    }
    if trimmed.starts_with('[') {
        if mapping.is_some() {
            return Err(ApiError::BadRequest(
                "mapping_template_id is only supported for CSV import files".to_string(),
            ));
        }
        let values = serde_json::from_str::<Vec<Value>>(trimmed)
            .map_err(|_| ApiError::BadRequest("import JSON must be an array".to_string()))?;
        return values
            .iter()
            .enumerate()
            .map(|(index, value)| import_row_from_json_value(value, index + 1, file_url))
            .collect();
    }
    import_rows_from_csv_text(fields, trimmed, file_url, mapping)
}

fn import_row_from_json_value(value: &Value, row_number: usize, file_url: &str) -> Result<ImportRecordInput, ApiError> {
    let object = value
        .as_object()
        .ok_or_else(|| ApiError::BadRequest(format!("import row {row_number} must be an object")))?;
    let values = object
        .get("values")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| value.clone());
    let title = object
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Ok(ImportRecordInput {
        row_number: Some(row_number),
        title,
        values,
        source: Some(import_file_source(file_url, row_number)),
        idempotency_key: object
            .get("idempotency_key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    })
}

pub fn import_rows_from_csv_text(
    fields: &[FormField],
    text: &str,
    file_url: &str,
    mapping: Option<&ImportMappingTemplateConfig>,
) -> Result<Vec<ImportRecordInput>, ApiError> {
    let lines = parse_csv_lines(text)?;
    if lines.len() < 2 {
        return Err(ApiError::BadRequest(
            "import CSV must include a header and at least one row".to_string(),
        ));
    }
    let headers = lines
        .first()
        .ok_or_else(|| ApiError::BadRequest("import CSV must include a header row".to_string()))?
        .iter()
        .map(|header| header.trim().to_string())
        .collect::<Vec<_>>();
    let (selections, transforms) = if let Some(mapping) = mapping {
        let actual_signature = import_mapping_header_signature(&headers);
        if mapping.header_signature != actual_signature || mapping.headers != headers {
            return Err(ApiError::BadRequest(
                "import mapping template does not match CSV headers".to_string(),
            ));
        }
        if mapping.selections.len() != headers.len() || mapping.transforms.len() != headers.len() {
            return Err(ApiError::BadRequest(
                "import mapping template column counts do not match CSV headers".to_string(),
            ));
        }
        (Some(mapping.selections.as_slice()), Some(mapping.transforms.as_slice()))
    } else {
        (None, None)
    };
    let mut rows = Vec::new();
    for (line_index, cells) in lines.iter().enumerate().skip(1) {
        if cells.iter().all(|cell| cell.trim().is_empty()) {
            continue;
        }
        let mut values = serde_json::Map::new();
        let mut title = None;
        for (column, header) in headers.iter().enumerate() {
            let raw = cells.get(column).map(String::as_str).unwrap_or_default();
            let selection = selections
                .and_then(|values| values.get(column))
                .map(String::as_str)
                .unwrap_or(header);
            let transform = transforms
                .and_then(|values| values.get(column))
                .map(String::as_str)
                .unwrap_or("trim");
            if selection == "__skip" {
                continue;
            }
            let transformed = apply_import_mapping_transform(raw, transform)?;
            if selection == "__title" || (mapping.is_none() && header.eq_ignore_ascii_case("title")) {
                let value = transformed.trim();
                if !value.is_empty() {
                    title = Some(value.to_string());
                }
                continue;
            }
            if let Some(field) = field_for_import_header(fields, selection) {
                if let Some(value) = import_cell_value(field, &transformed) {
                    values.insert(field.key.clone(), value);
                }
            }
        }
        rows.push(ImportRecordInput {
            row_number: Some(line_index),
            title,
            values: Value::Object(values),
            source: Some(import_file_source(file_url, line_index)),
            idempotency_key: None,
        });
    }
    Ok(rows)
}

pub fn apply_import_mapping_transform(value: &str, transform: &str) -> Result<String, ApiError> {
    let trimmed = value.trim();
    match normalize_import_mapping_transform(transform)?.as_str() {
        "none" => Ok(value.to_string()),
        "trim" => Ok(trimmed.to_string()),
        "uppercase" => Ok(trimmed.to_uppercase()),
        "lowercase" => Ok(trimmed.to_lowercase()),
        "capitalize" => Ok(capitalize_import_value(trimmed)),
        "titlecase" => Ok(trimmed
            .split_whitespace()
            .map(capitalize_import_value)
            .collect::<Vec<_>>()
            .join(" ")),
        "normalize_spaces" => Ok(trimmed.split_whitespace().collect::<Vec<_>>().join(" ")),
        "strip_non_digits" => Ok(trimmed.chars().filter(char::is_ascii_digit).collect()),
        "slugify" => Ok(slugify_import_value(trimmed)),
        other => Err(ApiError::BadRequest(format!(
            "unsupported import mapping transform: {other}"
        ))),
    }
}

fn capitalize_import_value(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    format!("{}{}", first.to_uppercase(), chars.as_str().to_lowercase())
}

fn slugify_import_value(value: &str) -> String {
    let mut output = String::new();
    let mut pending_dash = false;
    for ch in value.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !output.is_empty() {
                output.push('-');
            }
            output.push(ch);
            pending_dash = false;
        } else if !output.is_empty() {
            pending_dash = true;
        }
    }
    output
}

pub fn parse_csv_lines(text: &str) -> Result<Vec<Vec<String>>, ApiError> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if quoted {
            if ch == '"' && chars.peek() == Some(&'"') {
                cell.push('"');
                chars.next();
            } else if ch == '"' {
                quoted = false;
            } else {
                cell.push(ch);
            }
            continue;
        }
        match ch {
            '"' => quoted = true,
            ',' => {
                row.push(cell);
                cell = String::new();
            }
            '\n' => {
                row.push(cell);
                rows.push(row);
                row = Vec::new();
                cell = String::new();
            }
            '\r' => {}
            _ => cell.push(ch),
        }
    }
    if quoted {
        return Err(ApiError::BadRequest(
            "import CSV has an unterminated quoted cell".to_string(),
        ));
    }
    row.push(cell);
    rows.push(row);
    Ok(rows)
}

fn field_for_import_header<'a>(fields: &'a [FormField], header: &str) -> Option<&'a FormField> {
    let normalized = header.trim();
    if normalized.is_empty() {
        return None;
    }
    fields.iter().find(|field| {
        field.key == normalized
            || field.label == normalized
            || field.key.eq_ignore_ascii_case(normalized)
            || field.label.eq_ignore_ascii_case(normalized)
    })
}

fn import_cell_value(field: &FormField, raw: &str) -> Option<Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match field.field_type.as_str() {
        "boolean" => Some(Value::Bool(matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "true" | "1" | "yes" | "y"
        ))),
        "multi_select" => Some(Value::Array(
            trimmed
                .split([';', '|'])
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| Value::String(value.to_string()))
                .collect(),
        )),
        "attachment" | "image" | "relation" | "child_table" | "formula" => {
            Some(serde_json::from_str(trimmed).unwrap_or_else(|_| Value::String(trimmed.to_string())))
        }
        _ => Some(Value::String(trimmed.to_string())),
    }
}

fn import_file_source(file_url: &str, row_number: usize) -> Value {
    json!({
        "type": "server",
        "origin": "import_file",
        "file_url": file_url,
        "row_number": row_number,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CreateImportJobRequest, ImportMappingTemplateConfig, ImportRecordInput, SaveImportMappingTemplateRequest,
        apply_import_mapping_transform, create_import_job_input, import_mapping_header_signature,
        import_rows_from_csv_text, import_rows_from_file_text, normalize_import_mapping_template_request,
        parse_csv_lines,
    };
    use crate::forms::schema::FormField;
    use serde_json::json;

    #[test]
    fn parses_import_csv_lines_with_quotes() {
        assert_eq!(
            parse_csv_lines("Title,notes\n\"A, B\",\"quoted \"\"value\"\"\"").unwrap(),
            vec![
                vec!["Title".to_string(), "notes".to_string()],
                vec!["A, B".to_string(), "quoted \"value\"".to_string()]
            ]
        );
    }

    #[test]
    fn imports_server_csv_rows_using_field_keys_and_labels() {
        let fields = vec![
            test_field("status", "Status", "text"),
            test_field("tags", "Tags", "multi_select"),
            test_field("active", "Active", "boolean"),
        ];
        let rows = import_rows_from_csv_text(
            &fields,
            "Title,Status,Tags,Active\nOrder 1,open,red|blue,yes",
            "/api/v1/uploads/import.csv",
            None,
        )
        .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].row_number, Some(1));
        assert_eq!(rows[0].title.as_deref(), Some("Order 1"));
        assert_eq!(rows[0].values["status"], json!("open"));
        assert_eq!(rows[0].values["tags"], json!(["red", "blue"]));
        assert_eq!(rows[0].values["active"], json!(true));
        assert_eq!(rows[0].source.as_ref().unwrap()["origin"], json!("import_file"));
    }

    #[test]
    fn imports_server_csv_rows_with_mapping_template_and_advanced_transforms() {
        let fields = vec![
            test_field("status", "Status", "text"),
            test_field("phone", "Phone", "text"),
            test_field("slug", "Slug", "text"),
        ];
        let headers = vec![
            "Name".to_string(),
            "Phone".to_string(),
            "Status".to_string(),
            "Slug".to_string(),
        ];
        let mapping = ImportMappingTemplateConfig {
            header_signature: import_mapping_header_signature(&headers),
            headers,
            selections: vec![
                "__title".to_string(),
                "phone".to_string(),
                "status".to_string(),
                "slug".to_string(),
            ],
            transforms: vec![
                "titlecase".to_string(),
                "strip_non_digits".to_string(),
                "capitalize".to_string(),
                "slugify".to_string(),
            ],
        };
        let rows = import_rows_from_csv_text(
            &fields,
            "Name,Phone,Status,Slug\n\"jane doe\", \"(555) 010-2222\", pending review, \"Summer Menu 2026!\"",
            "/api/v1/uploads/import.csv",
            Some(&mapping),
        )
        .unwrap();

        assert_eq!(rows[0].title.as_deref(), Some("Jane Doe"));
        assert_eq!(rows[0].values["phone"], json!("5550102222"));
        assert_eq!(rows[0].values["status"], json!("Pending review"));
        assert_eq!(rows[0].values["slug"], json!("summer-menu-2026"));
    }

    #[test]
    fn applies_advanced_import_mapping_transforms() {
        assert_eq!(
            apply_import_mapping_transform("  hello   world  ", "normalize_spaces").unwrap(),
            "hello world"
        );
        assert_eq!(
            apply_import_mapping_transform("  summer MENU 2026!  ", "slugify").unwrap(),
            "summer-menu-2026"
        );
        assert_eq!(
            apply_import_mapping_transform("  mIXed value  ", "capitalize").unwrap(),
            "Mixed value"
        );
    }

    #[test]
    fn reports_unsupported_import_mapping_transforms_as_errors() {
        let error = apply_import_mapping_transform("value", "explode");
        assert!(error.is_err());
        assert!(apply_import_mapping_transform("value", "").is_err());
    }

    #[test]
    fn rejects_csv_without_a_header_and_a_data_row() {
        let fields = vec![test_field("status", "Status", "text")];
        assert!(import_rows_from_csv_text(&fields, "", "/api/v1/uploads/import.csv", None).is_err());
        assert!(import_rows_from_csv_text(&fields, "Title,Status", "/api/v1/uploads/import.csv", None).is_err());
    }

    #[test]
    fn validates_import_job_input_source() {
        let row = ImportRecordInput {
            row_number: Some(1),
            title: Some("Order 1".to_string()),
            values: json!({"status": "open"}),
            source: None,
            idempotency_key: None,
        };
        assert!(
            create_import_job_input(&CreateImportJobRequest {
                rows: Some(vec![row.clone()]),
                file_url: None,
                upload_url: None,
                mapping_template_id: None,
                idempotency_key: None,
            })
            .is_ok()
        );
        assert!(
            create_import_job_input(&CreateImportJobRequest {
                rows: None,
                file_url: Some("/api/v1/uploads/import.csv".to_string()),
                upload_url: None,
                mapping_template_id: None,
                idempotency_key: None,
            })
            .is_ok()
        );
        assert!(
            create_import_job_input(&CreateImportJobRequest {
                rows: Some(vec![row]),
                file_url: Some("/api/v1/uploads/import.csv".to_string()),
                upload_url: None,
                mapping_template_id: None,
                idempotency_key: None,
            })
            .is_err()
        );
        assert!(
            create_import_job_input(&CreateImportJobRequest {
                rows: None,
                file_url: None,
                upload_url: None,
                mapping_template_id: None,
                idempotency_key: None,
            })
            .is_err()
        );
    }

    #[test]
    fn imports_server_json_rows_with_values_wrapper() {
        let rows = import_rows_from_file_text(
            &[],
            r#"[{"title":"Order 1","values":{"status":"open"},"idempotency_key":"row-1"}]"#,
            "/api/v1/uploads/import.json",
            None,
        )
        .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title.as_deref(), Some("Order 1"));
        assert_eq!(rows[0].values["status"], json!("open"));
        assert_eq!(rows[0].idempotency_key.as_deref(), Some("row-1"));
    }

    #[test]
    fn normalizes_import_mapping_template_request() {
        let fields = vec![test_field("status", "Status", "text")];
        let normalized = normalize_import_mapping_template_request(
            SaveImportMappingTemplateRequest {
                id: None,
                name: Some(" Daily import ".to_string()),
                header_signature: None,
                headers: vec!["Title".to_string(), "Status".to_string()],
                selections: vec!["__title".to_string(), "status".to_string()],
                transforms: vec!["trim".to_string(), "uppercase".to_string()],
                shared: Some(true),
            },
            &fields,
        )
        .unwrap();

        assert_eq!(normalized.name, "Daily import");
        assert_eq!(normalized.header_signature, "Title\u{1f}Status");
        assert_eq!(normalized.selections, json!(["__title", "status"]));
        assert_eq!(normalized.transforms, json!(["trim", "uppercase"]));
        assert!(normalized.shared);
    }

    #[test]
    fn rejects_invalid_import_mapping_template_entries() {
        let fields = vec![test_field("status", "Status", "text")];
        let invalid_selection = normalize_import_mapping_template_request(
            SaveImportMappingTemplateRequest {
                id: None,
                name: None,
                header_signature: None,
                headers: vec!["Missing".to_string()],
                selections: vec!["missing".to_string()],
                transforms: vec!["trim".to_string()],
                shared: None,
            },
            &fields,
        );
        assert!(invalid_selection.is_err());

        let invalid_transform = normalize_import_mapping_template_request(
            SaveImportMappingTemplateRequest {
                id: None,
                name: None,
                header_signature: None,
                headers: vec!["Status".to_string()],
                selections: vec!["status".to_string()],
                transforms: vec!["unknown_transform".to_string()],
                shared: None,
            },
            &fields,
        );
        assert!(invalid_transform.is_err());
    }

    fn test_field(key: &str, label: &str, field_type: &str) -> FormField {
        FormField {
            field_id: format!("fld_{key}"),
            key: key.to_string(),
            label: label.to_string(),
            field_type: field_type.to_string(),
            required: false,
            options: Vec::new(),
            option_disabled: Vec::new(),
            option_archived: Vec::new(),
            conditional: None,
            amount: None,
            signature: None,
        }
    }
}
