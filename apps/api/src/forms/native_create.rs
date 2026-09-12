//! Transaction-aware Forms creation services shared by HTTP and cross-module bridges.
//!
//! Callers supply an existing transaction so a bridge can commit the native Forms record,
//! its native events, and its own lineage atomically. Validation and plugin hooks stay identical
//! to direct Forms creation; this module is deliberately independent of Axum extractors.

use platform::app::AppState;
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    forms::{
        permissions::ensure_field_write_policy_allows,
        projections::refresh_record_projection,
        schema::{ensure_schema_field_ids, normalize_key, validate_schema},
        signature_media::{
            annotate_signature_audit_entries, append_signature_audit_source, materialize_signature_values_with_audit,
        },
        validation::validate_and_normalize_values,
    },
    plugins::hooks::{run_event_handler_hooks, run_field_validator_hooks, run_formula_hooks},
    routes::form::{
        FormResponse, RecordResponse, apply_autonumber_values, calculate_values, ensure_json_object,
        ensure_record_objects_claimable, find_form_with_conn, find_idempotent_record, find_record,
        insert_form_event_with_idempotency, insert_schema_version, recalculate_parent_records_for_child, render_title,
    },
};

pub struct NativeFormCreateRequest {
    pub form_id: Option<Uuid>,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub title_template: Option<String>,
    pub schema: Value,
    pub detail_layout: Value,
    pub created_by: Option<Uuid>,
    pub source: Value,
    pub change_summary: String,
}

/// Runs the native Forms form-create validation, insert, event, and schema-version pipeline.
pub async fn create_form_in_transaction(
    tx: &DatabaseTransaction,
    req: NativeFormCreateRequest,
) -> Result<FormResponse, ApiError> {
    let key = normalize_key(&req.key).map_err(ApiError::BadRequest)?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name is required".to_string()));
    }
    let schema = ensure_schema_field_ids(req.schema).map_err(ApiError::BadRequest)?;
    validate_schema(&schema).map_err(ApiError::BadRequest)?;
    let detail_layout = ensure_json_object(req.detail_layout, "detail_layout")?;
    let form_id = req.form_id.unwrap_or_else(Uuid::new_v4);
    let source = ensure_json_object(req.source, "source")?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO project_forms \
         (id, workspace_id, project_id, key, name, description, icon, color, title_template, schema, \
          detail_layout, created_by) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        vec![
            form_id.into(),
            req.workspace_id.into(),
            req.project_id.into(),
            key.into(),
            name.to_string().into(),
            req.description.into(),
            req.icon.into(),
            req.color.into(),
            req.title_template.unwrap_or_else(|| "{id}".to_string()).into(),
            schema.into(),
            detail_layout.into(),
            req.created_by.into(),
        ],
    ))
    .await?;
    let form = find_form_with_conn(tx, form_id).await?;
    insert_form_event_with_idempotency(
        tx,
        &form,
        None,
        "form.created",
        req.created_by,
        source,
        json!({"form_id":form.id,"key":form.key,"name":form.name}),
        None,
    )
    .await?;
    insert_schema_version(tx, &form, req.created_by, &req.change_summary).await?;
    Ok(form)
}

pub struct NativeRecordCreateRequest {
    pub record_id: Option<Uuid>,
    pub values: Value,
    pub title: Option<String>,
    pub source: Value,
    pub idempotency_key: Option<String>,
}

pub struct NativeRecordCreation {
    pub record_id: Uuid,
    pub values: Value,
    pub created_by: Option<Uuid>,
    pub event_payload: Value,
    pub was_created: bool,
}

/// Runs the complete native Forms record-create pipeline inside the caller's transaction.
pub async fn create_record_in_transaction(
    state: &AppState,
    tx: &DatabaseTransaction,
    form: &FormResponse,
    actor_id: Uuid,
    role: &str,
    is_bot: bool,
    req: NativeRecordCreateRequest,
) -> Result<NativeRecordCreation, ApiError> {
    if let Some(record) =
        find_idempotent_record(state, form, req.idempotency_key.as_deref(), "form.record.created").await?
    {
        return Ok(NativeRecordCreation {
            record_id: record.id,
            values: record.values.clone(),
            created_by: record.created_by,
            event_payload: json!({"record_id":record.id,"values":record.values}),
            was_created: false,
        });
    }

    ensure_field_write_policy_allows(state, form.id, role, &req.values).await?;
    let values_with_formula = run_formula_hooks(
        state,
        form.workspace_id,
        form.project_id,
        form.id,
        &form.key,
        req.values,
    )
    .await?;
    let record_id = req.record_id.unwrap_or_else(Uuid::new_v4);
    let created_by = (!is_bot).then_some(actor_id);
    let source = ensure_json_object(req.source, "source")?;
    let calculated = calculate_values(state, form, None, values_with_formula).await?;
    let with_autonumber = apply_autonumber_values(tx, form, None, calculated).await?;
    let normalized = validate_and_normalize_values(&form.schema, with_autonumber).map_err(ApiError::BadRequest)?;
    let signature_materialization = materialize_signature_values_with_audit(&form.schema, normalized).await?;
    let normalized = signature_materialization.values;
    ensure_record_objects_claimable(state, form.workspace_id, &normalized, None).await?;
    let signature_audit_entries = annotate_signature_audit_entries(
        signature_materialization.audit_entries,
        if is_bot { "bot" } else { "user" },
        actor_id,
        "record.create",
        form.id,
        record_id,
        form.schema_version,
    );
    let source = append_signature_audit_source(source, signature_audit_entries)?;
    run_field_validator_hooks(
        state,
        form.workspace_id,
        form.project_id,
        form.id,
        &form.key,
        &form.schema,
        &normalized,
    )
    .await?;
    let title = req
        .title
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| render_title(&form.title_template, record_id, &normalized));

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO form_records \
         (id, workspace_id, project_id, form_id, title, values, source, schema_version, created_by, updated_by) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$9)",
        vec![
            record_id.into(),
            form.workspace_id.into(),
            form.project_id.into(),
            form.id.into(),
            title.into(),
            normalized.clone().into(),
            source.clone().into(),
            form.schema_version.into(),
            created_by.into(),
        ],
    ))
    .await?;
    refresh_record_projection(tx, form.project_id, form.id, record_id, &form.schema, &normalized).await?;
    let event_payload = json!({"record_id":record_id,"values":normalized});
    insert_form_event_with_idempotency(
        tx,
        form,
        Some(record_id),
        "form.record.created",
        created_by,
        source,
        event_payload.clone(),
        req.idempotency_key,
    )
    .await?;

    Ok(NativeRecordCreation {
        record_id,
        values: normalized,
        created_by,
        event_payload,
        was_created: true,
    })
}

/// Runs native post-commit hooks only after the caller atomically commits its transaction.
pub async fn finish_record_creation(
    state: &AppState,
    form: &FormResponse,
    creation: &NativeRecordCreation,
) -> Result<RecordResponse, ApiError> {
    if creation.was_created {
        run_event_handler_hooks(
            state,
            form.workspace_id,
            form.project_id,
            form.id,
            &form.key,
            Some(creation.record_id),
            "form.record.created",
            creation.event_payload.clone(),
        )
        .await?;
        recalculate_parent_records_for_child(state, creation.record_id, creation.created_by).await?;
    }
    find_record(state, creation.record_id).await
}
