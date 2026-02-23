use chrono::Utc;
use sea_orm::{ConnectionTrait, DbBackend, Statement, Value as SeaValue};
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;

pub struct GovernanceAuditLogInput {
    pub project_id: Uuid,
    pub actor_id: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub old_value: Option<Value>,
    pub new_value: Option<Value>,
    pub metadata: Option<Value>,
}

pub async fn write_governance_audit_log<C: ConnectionTrait>(
    db: &C,
    input: GovernanceAuditLogInput,
) -> Result<(), ApiError> {
    let old_value = match input.old_value {
        Some(v) => SeaValue::Json(Some(Box::new(v))),
        None => SeaValue::Json(None),
    };
    let new_value = match input.new_value {
        Some(v) => SeaValue::Json(Some(Box::new(v))),
        None => SeaValue::Json(None),
    };
    let metadata = match input.metadata {
        Some(v) => SeaValue::Json(Some(Box::new(v))),
        None => SeaValue::Json(None),
    };

    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            INSERT INTO governance_audit_logs (
                project_id,
                actor_id,
                action,
                resource_type,
                resource_id,
                old_value,
                new_value,
                metadata,
                created_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
        vec![
            input.project_id.into(),
            input.actor_id.into(),
            input.action.into(),
            input.resource_type.into(),
            input.resource_id.into(),
            old_value,
            new_value,
            metadata,
            Utc::now().into(),
        ],
    ))
    .await?;

    Ok(())
}
