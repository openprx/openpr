use sea_orm::{ConnectionTrait, DbBackend, Statement, Value as SeaValue};
use serde_json::Value;
use uuid::Uuid;

use super::schema::parse_fields;
use crate::error::ApiError;

pub async fn refresh_record_projection<C>(
    db: &C,
    project_id: Uuid,
    form_id: Uuid,
    record_id: Uuid,
    schema: &Value,
    values: &Value,
) -> Result<(), ApiError>
where
    C: ConnectionTrait,
{
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM form_record_field_index WHERE record_id = $1",
        vec![record_id.into()],
    ))
    .await?;

    let fields = parse_fields(schema).map_err(ApiError::BadRequest)?;
    for field in fields {
        let Some(value) = values.get(&field.key) else {
            continue;
        };
        let mut value_text: Option<String> = None;
        let mut value_decimal: Option<String> = None;
        let mut value_datetime: Option<String> = None;
        let mut value_bool: Option<bool> = None;

        match field.field_type.as_str() {
            "text" | "textarea" | "rich_text" | "single_select" | "ai_summary" => {
                value_text = value.as_str().map(str::to_string);
            }
            "date" | "datetime" => {
                value_datetime = value.as_str().map(str::to_string);
                value_text = value_datetime.clone();
            }
            "boolean" => {
                value_bool = value.as_bool();
            }
            "amount" | "number" => {
                value_decimal = value.get("decimal").and_then(Value::as_str).map(str::to_string);
                value_text = value_decimal.clone();
            }
            "integer" => {
                value_decimal = value.get("value").and_then(Value::as_i64).map(|item| item.to_string());
                value_text = value_decimal.clone();
            }
            _ => {}
        }

        if value_text.is_none() && value_decimal.is_none() && value_datetime.is_none() && value_bool.is_none() {
            continue;
        }

        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO form_record_field_index (
                    record_id, form_id, project_id, field_key, field_id,
                    value_text, value_decimal, value_datetime, value_bool
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7::numeric, $8::timestamptz, $9)
            ",
            vec![
                record_id.into(),
                form_id.into(),
                project_id.into(),
                field.key.into(),
                field.field_id.into(),
                value_text.into(),
                value_decimal
                    .map(SeaValue::from)
                    .unwrap_or_else(|| SeaValue::from(None::<String>)),
                value_datetime
                    .map(SeaValue::from)
                    .unwrap_or_else(|| SeaValue::from(None::<String>)),
                value_bool.into(),
            ],
        ))
        .await?;
    }

    Ok(())
}
