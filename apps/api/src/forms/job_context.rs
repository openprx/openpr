use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::ApiError;

#[derive(Debug, Deserialize)]
struct FormJobWorkerContext {
    actor_id: Option<Uuid>,
    role: Option<String>,
    is_bot: Option<bool>,
}

pub fn add_form_job_worker_context(input: Value, actor_id: Uuid, role: &str, is_bot: bool) -> Result<Value, ApiError> {
    let mut object = input
        .as_object()
        .cloned()
        .ok_or_else(|| ApiError::BadRequest("form job input must be a JSON object".to_string()))?;
    object.insert(
        "worker_context".to_string(),
        json!({
            "actor_id": actor_id,
            "role": role,
            "is_bot": is_bot,
        }),
    );
    Ok(Value::Object(object))
}

pub fn form_job_worker_context(input: &Value, created_by: Option<Uuid>) -> Result<(Uuid, String, bool), ApiError> {
    let context = input
        .get("worker_context")
        .cloned()
        .map(serde_json::from_value::<FormJobWorkerContext>)
        .transpose()
        .map_err(|_| ApiError::BadRequest("form job worker_context is invalid".to_string()))?;
    let actor_id = context
        .as_ref()
        .and_then(|ctx| ctx.actor_id)
        .or(created_by)
        .unwrap_or_else(Uuid::nil);
    let role = context
        .as_ref()
        .and_then(|ctx| ctx.role.clone())
        .filter(|role| !role.trim().is_empty())
        .unwrap_or_else(|| "admin".to_string());
    let is_bot = context
        .as_ref()
        .and_then(|ctx| ctx.is_bot)
        .unwrap_or(created_by.is_none());
    Ok((actor_id, role, is_bot))
}

#[cfg(test)]
mod tests {
    use super::{add_form_job_worker_context, form_job_worker_context};
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn persists_form_job_worker_context() {
        let actor_id = Uuid::new_v4();
        let input = add_form_job_worker_context(json!({"format": "csv"}), actor_id, "member", false).unwrap();
        let (parsed_actor_id, role, is_bot) = form_job_worker_context(&input, None).unwrap();

        assert_eq!(parsed_actor_id, actor_id);
        assert_eq!(role, "member");
        assert!(!is_bot);
    }

    #[test]
    fn falls_back_to_creator_for_old_form_jobs() {
        let actor_id = Uuid::new_v4();
        let (parsed_actor_id, role, is_bot) =
            form_job_worker_context(&json!({"format": "csv"}), Some(actor_id)).unwrap();

        assert_eq!(parsed_actor_id, actor_id);
        assert_eq!(role, "admin");
        assert!(!is_bot);
    }

    #[test]
    fn treats_missing_creator_old_jobs_as_bot_admin() {
        let (parsed_actor_id, role, is_bot) = form_job_worker_context(&json!({"format": "csv"}), None).unwrap();

        assert_eq!(parsed_actor_id, Uuid::nil());
        assert_eq!(role, "admin");
        assert!(is_bot);
    }
}
