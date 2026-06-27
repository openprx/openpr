use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

use crate::error::ApiError;

const DEFAULT_EXPORT_RECORD_LIMIT: i64 = 5_000;
const MAX_EXPORT_RECORD_LIMIT: i64 = 50_000;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ExportRecordsQuery {
    pub view_id: Option<Uuid>,
    pub columns: Option<String>,
    pub format: Option<String>,
    pub scope: Option<String>,
    pub include_archived: Option<bool>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ExportColumnResponse {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct ExportRecordRowResponse {
    pub record_id: Uuid,
    pub title: String,
    pub values: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct ExportRecordsResponse {
    pub form_id: Uuid,
    pub format: String,
    pub file_name: String,
    pub export_policy: ExportRecordsPolicyResponse,
    pub columns: Vec<ExportColumnResponse>,
    pub rows: Vec<ExportRecordRowResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csv: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct ExportRecordsPolicyResponse {
    pub scope: String,
    pub view_id: Option<Uuid>,
    pub include_archived: bool,
    pub row_limit: i64,
    pub row_count: usize,
    pub truncated: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateExportJobRequest {
    pub view_id: Option<Uuid>,
    pub columns: Option<Vec<String>>,
    pub format: Option<String>,
    pub scope: Option<String>,
    pub include_archived: Option<bool>,
    pub limit: Option<i64>,
}

pub fn export_columns_from_config(config: &Value) -> Vec<String> {
    config
        .get("columns")
        .and_then(Value::as_array)
        .map(|columns| {
            columns
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub fn export_display_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(items) => items
            .iter()
            .map(export_display_value)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(object) => {
            for key in ["display", "title", "label", "name", "decimal", "value"] {
                if let Some(value) = object.get(key) {
                    let display = export_display_value(value);
                    if !display.is_empty() {
                        if key == "decimal"
                            && let Some(currency) = object.get("currency").and_then(Value::as_str)
                        {
                            return format!("{display} {currency}");
                        }
                        return display;
                    }
                }
            }
            serde_json::to_string(value).unwrap_or_default()
        }
    }
}

pub fn build_records_csv(columns: &[ExportColumnResponse], rows: &[ExportRecordRowResponse]) -> String {
    let mut csv = String::new();
    let mut header = vec!["Title".to_string()];
    header.extend(columns.iter().map(|column| column.label.clone()));
    csv.push_str(&csv_line(&header));
    for row in rows {
        let mut values = vec![row.title.clone()];
        values.extend(
            columns
                .iter()
                .map(|column| row.values.get(&column.key).cloned().unwrap_or_default()),
        );
        csv.push_str(&csv_line(&values));
    }
    csv
}

pub fn export_records_format(format: Option<&str>) -> Result<&'static str, ApiError> {
    match format.unwrap_or("csv").trim().to_ascii_lowercase().as_str() {
        "" | "csv" => Ok("csv"),
        "json" => Ok("json"),
        other => Err(ApiError::BadRequest(format!("unsupported export format: {other}"))),
    }
}

pub fn export_records_scope(scope: Option<&str>) -> Result<&'static str, ApiError> {
    match scope.unwrap_or("view").trim().to_ascii_lowercase().as_str() {
        "" | "view" => Ok("view"),
        "all" | "all_accessible" => Ok("all_accessible"),
        other => Err(ApiError::BadRequest(format!("unsupported export scope: {other}"))),
    }
}

pub fn export_records_include_archived(include_archived: bool, can_include_archived: bool) -> Result<bool, ApiError> {
    if include_archived && !can_include_archived {
        return Err(ApiError::Forbidden(
            "only form admins can export archived records".to_string(),
        ));
    }
    Ok(include_archived)
}

pub fn export_records_limit(limit: Option<i64>) -> Result<i64, ApiError> {
    let limit = limit.unwrap_or(DEFAULT_EXPORT_RECORD_LIMIT);
    if !(1..=MAX_EXPORT_RECORD_LIMIT).contains(&limit) {
        return Err(ApiError::BadRequest(format!(
            "export limit must be between 1 and {MAX_EXPORT_RECORD_LIMIT}"
        )));
    }
    Ok(limit)
}

pub fn create_export_job_input(req: &CreateExportJobRequest) -> Result<Value, ApiError> {
    export_records_format(req.format.as_deref())?;
    export_records_scope(req.scope.as_deref())?;
    export_records_limit(req.limit)?;
    if let Some(columns) = req.columns.as_ref() {
        if columns.len() > 100 {
            return Err(ApiError::BadRequest("export job columns cannot exceed 100".to_string()));
        }
        if columns.iter().any(|column| column.trim().is_empty()) {
            return Err(ApiError::BadRequest(
                "export job columns cannot include blank values".to_string(),
            ));
        }
    }
    serde_json::to_value(req).map_err(|_| ApiError::Internal)
}

pub fn export_records_query_from_job(req: CreateExportJobRequest) -> Result<ExportRecordsQuery, ApiError> {
    let format = export_records_format(req.format.as_deref())?.to_string();
    let scope = export_records_scope(req.scope.as_deref())?.to_string();
    let limit = export_records_limit(req.limit)?;
    let columns = req.columns.map(|columns| {
        columns
            .into_iter()
            .map(|column| column.trim().to_string())
            .filter(|column| !column.is_empty())
            .collect::<Vec<_>>()
            .join(",")
    });
    Ok(ExportRecordsQuery {
        view_id: req.view_id,
        columns: columns.filter(|value| !value.is_empty()),
        format: Some(format),
        scope: Some(scope),
        include_archived: req.include_archived,
        limit: Some(limit),
    })
}

fn csv_line(values: &[String]) -> String {
    let mut line = values
        .iter()
        .map(|value| csv_escape(value))
        .collect::<Vec<_>>()
        .join(",");
    line.push('\n');
    line
}

fn csv_escape(value: &str) -> String {
    if value.contains('"') || value.contains(',') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CreateExportJobRequest, ExportColumnResponse, ExportRecordRowResponse, build_records_csv,
        create_export_job_input, export_columns_from_config, export_display_value, export_records_format,
        export_records_include_archived, export_records_limit, export_records_query_from_job, export_records_scope,
    };
    use serde_json::json;
    use std::collections::BTreeMap;
    use uuid::Uuid;

    #[test]
    fn extracts_export_columns_from_view_config() {
        assert_eq!(
            export_columns_from_config(&json!({"columns": ["dish", "qty", "", 42, "status"]})),
            vec!["dish", "qty", "status"]
        );
    }

    #[test]
    fn renders_export_display_values_for_common_business_shapes() {
        assert_eq!(
            export_display_value(&json!({"decimal": "12.30", "currency": "CNY"})),
            "12.30 CNY"
        );
        assert_eq!(export_display_value(&json!({"display": "张三"})), "张三");
        assert_eq!(export_display_value(&json!(["a", "b"])), "a, b");
    }

    #[test]
    fn builds_csv_with_escaped_cells() {
        let columns = vec![
            ExportColumnResponse {
                key: "dish".to_string(),
                label: "菜品".to_string(),
            },
            ExportColumnResponse {
                key: "note".to_string(),
                label: "备注".to_string(),
            },
        ];
        let mut values = BTreeMap::new();
        values.insert("dish".to_string(), "米饭,大".to_string());
        values.insert("note".to_string(), "第一行\n第二行 \"加急\"".to_string());
        let rows = vec![ExportRecordRowResponse {
            record_id: uuid::Uuid::nil(),
            title: "订单 1".to_string(),
            values,
        }];

        let csv = build_records_csv(&columns, &rows);
        assert_eq!(
            csv,
            "Title,菜品,备注\n订单 1,\"米饭,大\",\"第一行\n第二行 \"\"加急\"\"\"\n"
        );
    }

    #[test]
    fn parses_export_record_formats() {
        assert_eq!(export_records_format(None).unwrap(), "csv");
        assert_eq!(export_records_format(Some("")).unwrap(), "csv");
        assert_eq!(export_records_format(Some("JSON")).unwrap(), "json");
        assert!(export_records_format(Some("xml")).is_err());
    }

    #[test]
    fn validates_export_job_input() {
        assert!(
            create_export_job_input(&CreateExportJobRequest {
                view_id: Some(Uuid::new_v4()),
                columns: Some(vec!["dish".to_string(), "qty".to_string()]),
                format: Some("json".to_string()),
                scope: Some("view".to_string()),
                include_archived: None,
                limit: Some(1000),
            })
            .is_ok()
        );
        assert!(
            create_export_job_input(&CreateExportJobRequest {
                view_id: None,
                columns: Some(vec!["dish".to_string(), " ".to_string()]),
                format: Some("csv".to_string()),
                scope: None,
                include_archived: None,
                limit: None,
            })
            .is_err()
        );
        assert!(
            create_export_job_input(&CreateExportJobRequest {
                view_id: None,
                columns: None,
                format: Some("xml".to_string()),
                scope: None,
                include_archived: None,
                limit: None,
            })
            .is_err()
        );
        assert!(
            create_export_job_input(&CreateExportJobRequest {
                view_id: None,
                columns: None,
                format: Some("csv".to_string()),
                scope: Some("everything".to_string()),
                include_archived: None,
                limit: None,
            })
            .is_err()
        );

        let query = export_records_query_from_job(CreateExportJobRequest {
            view_id: None,
            columns: Some(vec![" dish ".to_string(), "qty".to_string()]),
            format: None,
            scope: Some("all_accessible".to_string()),
            include_archived: Some(false),
            limit: Some(250),
        })
        .unwrap();
        assert_eq!(query.columns.as_deref(), Some("dish,qty"));
        assert_eq!(query.format.as_deref(), Some("csv"));
        assert_eq!(query.scope.as_deref(), Some("all_accessible"));
        assert_eq!(query.include_archived, Some(false));
        assert_eq!(query.limit, Some(250));
    }

    #[test]
    fn validates_export_policy_options() {
        assert_eq!(export_records_scope(None).unwrap(), "view");
        assert_eq!(export_records_scope(Some("all")).unwrap(), "all_accessible");
        assert_eq!(export_records_limit(None).unwrap(), 5000);
        assert_eq!(export_records_limit(Some(50_000)).unwrap(), 50_000);
        assert!(export_records_limit(Some(0)).is_err());
        assert!(export_records_limit(Some(50_001)).is_err());
        assert!(export_records_include_archived(true, false).is_err());
        assert!(export_records_include_archived(true, true).unwrap());
        assert!(!export_records_include_archived(false, false).unwrap());
    }
}
