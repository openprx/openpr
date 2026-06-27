use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    events::{BusinessEventInput, insert_business_event},
    forms::schema::parse_fields,
    plugins::{
        manifest::{PluginHook, parse_manifest},
        runtime::{PluginRuntimeOutput, invoke_wasm_plugin},
    },
};

#[derive(Debug, FromQueryResult)]
struct PluginHookRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    key: String,
    manifest: Value,
    wasm_bytes: Option<Vec<u8>>,
}

pub async fn run_field_validator_hooks(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Uuid,
    form_id: Uuid,
    form_key: &str,
    schema: &Value,
    values: &Value,
) -> Result<(), ApiError> {
    let Some(values_object) = values.as_object() else {
        return Ok(());
    };
    let fields = parse_fields(schema).map_err(ApiError::BadRequest)?;
    let plugins = load_active_plugins(state, workspace_id, project_id).await?;

    for plugin in plugins {
        let manifest = parse_manifest(&plugin.manifest).map_err(ApiError::BadRequest)?;
        let matching_hooks = manifest
            .capabilities
            .hooks
            .iter()
            .filter(|hook| hook.kind == "field_validator")
            .filter(|hook| hook_matches_form(hook, form_key))
            .collect::<Vec<_>>();
        if matching_hooks.is_empty() {
            continue;
        }
        let Some(wasm_bytes) = plugin.wasm_bytes.clone() else {
            continue;
        };

        for field in &fields {
            let Some(field_value) = values_object.get(&field.key) else {
                continue;
            };
            for _hook in matching_hooks
                .iter()
                .copied()
                .filter(|hook| hook_matches_field(hook, &field.key))
            {
                let input = json!({
                    "hook_kind": "field_validator",
                    "plugin_key": plugin.key,
                    "payload": {
                        "workspace_id": workspace_id,
                        "project_id": project_id,
                        "form_id": form_id,
                        "form_key": form_key,
                        "field_key": field.key,
                        "field_type": field.field_type,
                        "value": field_value,
                        "values": values,
                    }
                });
                let result =
                    invoke_wasm_plugin(wasm_bytes.clone(), input.clone(), manifest.capabilities.runtime.clone()).await;
                match result {
                    Ok(output) => {
                        if let Some(error) = validator_rejection_message(&output.output) {
                            insert_hook_invocation(
                                state,
                                &plugin,
                                "field_validator",
                                input,
                                output,
                                Some(error.clone()),
                            )
                            .await?;
                            return Err(ApiError::BadRequest(format!("field validator rejected value: {error}")));
                        }
                        insert_hook_invocation(state, &plugin, "field_validator", input, output, None).await?;
                    }
                    Err(error) => {
                        insert_failed_hook_invocation(state, &plugin, "field_validator", input, &error).await?;
                        return Err(ApiError::BadRequest(format!("field validator plugin failed: {error}")));
                    }
                }
            }
        }
    }

    Ok(())
}

pub async fn run_formula_hooks(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Uuid,
    form_id: Uuid,
    form_key: &str,
    values: Value,
) -> Result<Value, ApiError> {
    let mut values = values;
    if !values.is_object() {
        return Ok(values);
    }
    let plugins = load_active_plugins(state, workspace_id, project_id).await?;

    for plugin in plugins {
        let manifest = parse_manifest(&plugin.manifest).map_err(ApiError::BadRequest)?;
        let matching_hooks = manifest
            .capabilities
            .hooks
            .iter()
            .filter(|hook| hook.kind == "formula")
            .filter(|hook| hook_matches_form(hook, form_key))
            .collect::<Vec<_>>();
        if matching_hooks.is_empty() {
            continue;
        }
        let Some(wasm_bytes) = plugin.wasm_bytes.clone() else {
            continue;
        };

        for hook in matching_hooks {
            let input = json!({
                "hook_kind": "formula",
                "plugin_key": plugin.key,
                "payload": {
                    "workspace_id": workspace_id,
                    "project_id": project_id,
                    "form_id": form_id,
                    "form_key": form_key,
                    "field_key": hook.field_key,
                    "values": values,
                }
            });
            let result =
                invoke_wasm_plugin(wasm_bytes.clone(), input.clone(), manifest.capabilities.runtime.clone()).await;
            match result {
                Ok(output) => {
                    let patch = formula_patch_from_output(&output.output, hook.field_key.as_deref())?;
                    insert_hook_invocation(state, &plugin, "formula", input, output, None).await?;
                    merge_patch(&mut values, patch)?;
                }
                Err(error) => {
                    insert_failed_hook_invocation(state, &plugin, "formula", input, &error).await?;
                    return Err(ApiError::BadRequest(format!("formula plugin failed: {error}")));
                }
            }
        }
    }

    Ok(values)
}

pub async fn run_event_handler_hooks(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Uuid,
    form_id: Uuid,
    form_key: &str,
    record_id: Option<Uuid>,
    event_type: &str,
    payload: Value,
) -> Result<(), ApiError> {
    let plugins = load_active_plugins(state, workspace_id, project_id).await?;

    for plugin in plugins {
        let manifest = parse_manifest(&plugin.manifest).map_err(ApiError::BadRequest)?;
        let matching_hooks = manifest
            .capabilities
            .hooks
            .iter()
            .filter(|hook| hook.kind == "event_handler")
            .filter(|hook| hook_matches_form(hook, form_key))
            .filter(|hook| hook.event_type.as_deref().is_none_or(|value| value == event_type))
            .collect::<Vec<_>>();
        if matching_hooks.is_empty() {
            continue;
        }
        let Some(wasm_bytes) = plugin.wasm_bytes.clone() else {
            continue;
        };

        for _hook in matching_hooks {
            let input = json!({
                "hook_kind": "event_handler",
                "plugin_key": plugin.key,
                "payload": {
                    "workspace_id": workspace_id,
                    "project_id": project_id,
                    "form_id": form_id,
                    "form_key": form_key,
                    "record_id": record_id,
                    "event_type": event_type,
                    "event_payload": payload,
                }
            });
            let result =
                invoke_wasm_plugin(wasm_bytes.clone(), input.clone(), manifest.capabilities.runtime.clone()).await;
            match result {
                Ok(output) => {
                    insert_hook_invocation(state, &plugin, "event_handler", input, output, None).await?;
                }
                Err(error) => {
                    insert_failed_hook_invocation(state, &plugin, "event_handler", input, &error).await?;
                }
            }
        }
    }

    Ok(())
}

fn hook_matches_form(hook: &PluginHook, form_key: &str) -> bool {
    hook.form_key.as_deref().is_none_or(|value| value == form_key)
}

fn hook_matches_field(hook: &PluginHook, field_key: &str) -> bool {
    hook.field_key.as_deref().is_none_or(|value| value == field_key)
}

fn validator_rejection_message(output: &Value) -> Option<String> {
    if output.get("ok").and_then(Value::as_bool) != Some(false) {
        return None;
    }
    Some(
        output
            .get("error")
            .or_else(|| output.get("message"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("validator returned ok=false")
            .to_string(),
    )
}

fn formula_patch_from_output(output: &Value, field_key: Option<&str>) -> Result<Value, ApiError> {
    if let Some(patch) = output.get("patch") {
        if patch.is_object() {
            return Ok(patch.clone());
        }
        return Err(ApiError::BadRequest(
            "formula plugin patch must be an object".to_string(),
        ));
    }
    if let Some(field_key) = field_key
        && let Some(value) = output.get("value")
    {
        return Ok(json!({ field_key: value }));
    }
    Ok(json!({}))
}

fn merge_patch(values: &mut Value, patch: Value) -> Result<(), ApiError> {
    let Some(values_object) = values.as_object_mut() else {
        return Err(ApiError::BadRequest(
            "formula target values must be an object".to_string(),
        ));
    };
    let Some(patch_object) = patch.as_object() else {
        return Err(ApiError::BadRequest("formula patch must be an object".to_string()));
    };
    for (key, value) in patch_object {
        values_object.insert(key.clone(), value.clone());
    }
    Ok(())
}

async fn load_active_plugins(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Vec<PluginHookRow>, ApiError> {
    Ok(PluginHookRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"SELECT id, workspace_id, project_id, key, manifest, wasm_bytes
             FROM plugins
            WHERE workspace_id = $1
              AND project_id = $2
              AND status = 'active'
              AND wasm_bytes IS NOT NULL
            ORDER BY key ASC",
        vec![workspace_id.into(), project_id.into()],
    ))
    .all(&state.db)
    .await?)
}

async fn insert_hook_invocation(
    state: &AppState,
    plugin: &PluginHookRow,
    hook_kind: &str,
    input: Value,
    output: PluginRuntimeOutput,
    error_message: Option<String>,
) -> Result<(), ApiError> {
    insert_invocation(
        state,
        plugin,
        InvocationInsert {
            hook_kind,
            status: if error_message.is_some() { "failed" } else { "completed" },
            input,
            output: output.output,
            error_message,
            duration_ms: i64::try_from(output.duration_ms).unwrap_or(i64::MAX),
            fuel_consumed: output.fuel_consumed.and_then(|value| i64::try_from(value).ok()),
        },
    )
    .await
}

async fn insert_failed_hook_invocation(
    state: &AppState,
    plugin: &PluginHookRow,
    hook_kind: &str,
    input: Value,
    error: &str,
) -> Result<(), ApiError> {
    insert_invocation(
        state,
        plugin,
        InvocationInsert {
            hook_kind,
            status: "failed",
            input,
            output: json!({}),
            error_message: Some(error.to_string()),
            duration_ms: 0,
            fuel_consumed: None,
        },
    )
    .await
}

struct InvocationInsert<'a> {
    hook_kind: &'a str,
    status: &'a str,
    input: Value,
    output: Value,
    error_message: Option<String>,
    duration_ms: i64,
    fuel_consumed: Option<i64>,
}

async fn insert_invocation(
    state: &AppState,
    plugin: &PluginHookRow,
    invocation: InvocationInsert<'_>,
) -> Result<(), ApiError> {
    let invocation_id = Uuid::new_v4();
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO plugin_invocations (
                    id, workspace_id, project_id, plugin_id, plugin_key, hook_kind, status,
                    input, output, error_message, duration_ms, fuel_consumed
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        vec![
            invocation_id.into(),
            plugin.workspace_id.into(),
            plugin.project_id.into(),
            plugin.id.into(),
            plugin.key.clone().into(),
            invocation.hook_kind.to_string().into(),
            invocation.status.to_string().into(),
            invocation.input.into(),
            invocation.output.into(),
            invocation.error_message.into(),
            invocation.duration_ms.into(),
            invocation.fuel_consumed.into(),
        ],
    ))
    .await?;
    insert_business_event(
        &tx,
        BusinessEventInput {
            workspace_id: plugin.workspace_id,
            project_id: Some(plugin.project_id),
            event_type: "plugin.invoked".to_string(),
            aggregate_type: "plugin".to_string(),
            aggregate_id: plugin.id.to_string(),
            actor_id: None,
            source: json!({
                "type": "system",
                "origin": "wasm_hook",
                "hook_kind": invocation.hook_kind
            }),
            payload: json!({
                "plugin_id": plugin.id,
                "plugin_key": plugin.key,
                "invocation_id": invocation_id,
                "hook_kind": invocation.hook_kind,
                "status": invocation.status,
                "duration_ms": invocation.duration_ms,
                "fuel_consumed": invocation.fuel_consumed
            }),
            metadata: json!({
                "plugin_id": plugin.id,
                "plugin_key": plugin.key,
                "invocation_id": invocation_id,
                "hook_kind": invocation.hook_kind,
                "automatic_hook": true
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        formula_patch_from_output, hook_matches_field, hook_matches_form, merge_patch, validator_rejection_message,
    };
    use crate::plugins::manifest::PluginHook;
    use serde_json::json;

    #[test]
    fn hook_matching_respects_optional_form_and_field_scope() {
        let scoped = PluginHook {
            kind: "field_validator".to_string(),
            form_key: Some("order".to_string()),
            field_key: Some("amount".to_string()),
            event_type: None,
        };
        assert!(hook_matches_form(&scoped, "order"));
        assert!(!hook_matches_form(&scoped, "sku"));
        assert!(hook_matches_field(&scoped, "amount"));
        assert!(!hook_matches_field(&scoped, "quantity"));

        let global = PluginHook {
            kind: "field_validator".to_string(),
            form_key: None,
            field_key: None,
            event_type: None,
        };
        assert!(hook_matches_form(&global, "order"));
        assert!(hook_matches_field(&global, "amount"));
    }

    #[test]
    fn validator_output_can_reject_values() {
        assert_eq!(
            validator_rejection_message(&json!({"ok": false, "error": "amount too high"})).as_deref(),
            Some("amount too high")
        );
        assert!(validator_rejection_message(&json!({"ok": true})).is_none());
        assert!(validator_rejection_message(&json!({})).is_none());
    }

    #[test]
    fn formula_output_can_patch_values() {
        let patch = formula_patch_from_output(&json!({"patch": {"total": "0.30"}}), None).expect("patch");
        assert_eq!(patch.get("total").and_then(serde_json::Value::as_str), Some("0.30"));

        let patch = formula_patch_from_output(&json!({"value": "0.30"}), Some("total")).expect("field patch");
        assert_eq!(patch.get("total").and_then(serde_json::Value::as_str), Some("0.30"));

        let mut values = json!({"subtotal": "0.10"});
        merge_patch(&mut values, patch).expect("merge");
        assert_eq!(values.get("total").and_then(serde_json::Value::as_str), Some("0.30"));
    }
}
