use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::{HeaderName, HeaderValue, header},
    response::{IntoResponse, Redirect, Response},
};
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use uuid::Uuid;

use crate::{
    error::ApiError,
    events::{BusinessEventInput, insert_business_event},
    forms::{
        attachment_media::{
            attachment_signed_url_ttl_minutes, ensure_attachment_media, server_owned_upload_file_name,
            validate_attachment_create_policy,
        },
        attachment_package::{
            ZipEntry, attachment_package_artifact_key, attachment_package_job_result_string, build_stored_zip,
            sanitize_file_stem, sanitize_zip_file_name,
        },
        calculation::{evaluate_calculated_values, merge_values_for_calculation, overlay_calculated_values},
        event_redaction::{redact_event_metadata, redact_event_payload, redact_event_source},
        job_context::{add_form_job_worker_context, form_job_worker_context},
        permissions::{
            append_record_scope_sql, denied_read_field_keys, ensure_field_read_policy_allows,
            ensure_field_write_policy_allows, ensure_record_scope_allows_created_by, filter_values_for_read_policy,
            form_action_allowed, form_permissions_response, form_record_scope, list_form_permission_policies,
            normalize_permission_subject_id, normalize_permission_subject_type, role_is_form_admin,
            role_record_scope_predicate_sql, validate_permission_action, validate_permission_policy,
        },
        projections::refresh_record_projection,
        record_comments::{
            CreateRecordCommentRequest, InsertRecordCommentInput, ListRecordCommentsQuery, count_record_comments,
            find_record_comment, insert_record_comment, list_record_comments, record_comment_page,
            required_record_comment_body,
        },
        record_export::{
            CreateExportJobRequest, ExportColumnResponse, ExportRecordRowResponse, ExportRecordsPolicyResponse,
            ExportRecordsQuery, ExportRecordsResponse, build_records_csv, create_export_job_input,
            export_columns_from_config, export_display_value, export_records_format, export_records_include_archived,
            export_records_limit, export_records_query_from_job, export_records_scope,
        },
        record_import::{
            CreateImportJobRequest, ImportMappingTemplateConfig, ImportRecordInput, ImportRecordPreviewRow,
            ImportRecordsFileRequest, ImportRecordsPreviewResponse, ImportRecordsRequest,
            ListImportMappingTemplatesQuery, SaveImportMappingTemplateRequest, create_import_job_input,
            import_rows_from_file_text, json_string_array, normalize_import_mapping_template_request,
        },
        record_query::{
            RecordQueryConfig, append_record_filter_expression_sql, query_filter_expression,
            record_filter_expression_from_config, record_sort_sql,
        },
        relations::{
            ChildRecordRelationQuery, ChildRecordRow, CreateChildRecordRequest, CreateLinkRequest, LinkResponse,
            ListChildrenQuery, RelationTargetResponse, RelationTargetsQuery, UpdateChildRecordRequest, find_link,
            find_parent_child_link, normalize_optional_child_form_key, normalize_optional_relation_key,
            normalize_relation_type, normalize_target_type, relation_target_config,
            validate_optional_parent_child_relation, validate_parent_child_relation,
        },
        schema::{FormField, ensure_schema_field_ids, normalize_key, parse_fields, validate_schema},
        schema_insights::{
            SchemaInsightInput, find_schema_field, form_field_dependencies, form_field_usage, form_schema_summary,
        },
        signature_media::{
            annotate_signature_audit_entries, append_signature_audit_source,
            cleanup_expired_replaced_signature_audit_entries, cleanup_expired_signature_values,
            materialize_signature_values_with_audit, materialize_signature_values_with_existing_audit,
            signature_lifecycle_summary, signature_workflow_verification_summary, verify_signature_audit_source,
        },
        validation::{
            validate_and_normalize_values, validate_and_normalize_values_with_existing,
            validate_and_normalize_values_with_existing_report,
        },
    },
    middleware::bot_auth::{BotAuthContext, require_workspace_access_from_auth},
    plugins::hooks::{run_event_handler_hooks, run_field_validator_hooks, run_formula_hooks},
    response::{ApiResponse, PaginatedData},
    routes::upload::{
        UploadObjectOwner, resolve_upload_object_owner, signature_object_key_claimed_by_record_value,
        upload_object_key_from_path,
    },
    services::object_storage::ObjectStorage,
};
use rust_decimal::Decimal;

const MAX_IMPORT_FILE_BYTES: usize = 2 * 1024 * 1024;
const ATTACHMENT_PACKAGE_JOB_RETENTION_HOURS: i64 = 24;

#[derive(Debug, Deserialize)]
pub struct ListFormsQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ListRecordsQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub view_id: Option<Uuid>,
    pub sort: Option<String>,
    pub filter_field: Option<String>,
    pub filter_op: Option<String>,
    pub filter_value: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ExportAttachmentPackageQuery {
    pub view_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct UpsertFormPermissionPolicy {
    pub subject_type: String,
    pub subject_id: String,
    pub policy: Value,
}

#[derive(Debug, Deserialize)]
pub struct UpdateFormPermissionsRequest {
    pub policies: Vec<UpsertFormPermissionPolicy>,
}

#[derive(Debug, Deserialize)]
pub struct ListAttachmentsQuery {
    pub record_id: Option<Uuid>,
    pub field_key: Option<String>,
    pub include_archived: Option<bool>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct AttachmentDownloadQuery {
    pub expires: i64,
    pub signature: String,
}

#[derive(Debug, Serialize)]
pub struct AttachmentSignedDownloadResponse {
    pub url: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize)]
pub struct ListEventsQuery {
    pub event_type: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct IdempotencyQuery {
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AggregateQuery {
    pub field_key: String,
    pub aggregate: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateFormRequest {
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub title_template: Option<String>,
    pub schema: Option<Value>,
    pub detail_layout: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct DuplicateFormRequest {
    pub key: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateFormFromTemplateRequest {
    pub template_key: String,
    pub key: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub title_template: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateFormRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub title_template: Option<String>,
    pub schema: Option<Value>,
    pub detail_layout: Option<Value>,
    pub expected_schema_version: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct CreateRecordRequest {
    pub values: Value,
    pub title: Option<String>,
    pub source: Option<Value>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRecordRequest {
    pub values: Option<Value>,
    pub title: Option<String>,
    pub source: Option<Value>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RecalculatePreviewRequest {
    pub values: Value,
}

#[derive(Debug, Deserialize)]
pub struct CreateAttachmentRequest {
    pub field_key: String,
    pub record_id: Option<Uuid>,
    pub file_name: String,
    pub content_type: Option<String>,
    pub byte_size: Option<i64>,
    pub storage_key: String,
    pub url: Option<String>,
    pub thumbnail_url: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct ChildDecimalRow {
    decimal: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct ParentRecordRow {
    source_record_id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct RecordIdRow {
    id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct SignatureRetentionCleanupCandidate {
    form_id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    form_key: String,
    form_name: String,
    form_description: String,
    form_icon: Option<String>,
    form_color: Option<String>,
    title_template: String,
    schema: Value,
    detail_layout: Value,
    schema_version: i32,
    form_created_by: Option<Uuid>,
    form_archived_at: Option<chrono::DateTime<chrono::Utc>>,
    form_created_at: chrono::DateTime<chrono::Utc>,
    form_updated_at: chrono::DateTime<chrono::Utc>,
    record_id: Uuid,
    record_values: Value,
    record_source: Value,
    record_updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateViewRequest {
    pub key: String,
    pub name: String,
    pub view_type: String,
    pub config: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateViewRequest {
    pub name: Option<String>,
    pub view_type: Option<String>,
    pub config: Option<Value>,
    pub expected_updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Serialize, FromQueryResult, Clone)]
pub struct FormResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub title_template: String,
    pub schema: Value,
    pub detail_layout: Value,
    pub schema_version: i32,
    pub created_by: Option<Uuid>,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, FromQueryResult, Clone)]
pub struct FormSchemaVersionResponse {
    pub id: Uuid,
    pub form_id: Uuid,
    pub version: i32,
    pub schema: Value,
    pub detail_layout: Value,
    pub changed_by: Option<Uuid>,
    pub change_summary: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, FromQueryResult)]
struct ScenarioFormTemplateRow {
    key: String,
    name: String,
    description: String,
    industry: String,
    field_schema: Value,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct RecordResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub form_id: Uuid,
    pub title: String,
    pub values: Value,
    pub source: Value,
    pub schema_version: Option<i32>,
    pub created_by: Option<Uuid>,
    pub updated_by: Option<Uuid>,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CreateAttachmentPackageJobRequest {
    pub view_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct ImportRecordsResponse {
    pub form_id: Uuid,
    pub schema_version: i32,
    pub total_rows: usize,
    pub created_count: usize,
    pub invalid_rows: usize,
    pub rows: Vec<ImportRecordPreviewRow>,
    pub records: Vec<RecordResponse>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ViewResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub form_id: Uuid,
    pub key: String,
    pub name: String,
    pub view_type: String,
    pub config: Value,
    pub created_by: Option<Uuid>,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct ChildRecordResponse {
    pub link_id: Uuid,
    pub relation_key: String,
    pub relation_type: String,
    pub record: RecordResponse,
}

#[derive(Debug, Serialize)]
pub struct ChildRecordMutationResponse {
    pub link: LinkResponse,
    pub record: RecordResponse,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct BusinessEventResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub actor_id: Option<Uuid>,
    pub source: Value,
    pub payload: Value,
    pub metadata: Value,
    pub correlation_id: Option<Uuid>,
    pub causation_id: Option<Uuid>,
    pub idempotency_key: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, FromQueryResult)]
struct IdempotencyReceiptRow {
    event_type: String,
    aggregate_id: String,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct FormAggregateResponse {
    pub form_id: Uuid,
    pub field_key: String,
    pub field_type: String,
    pub aggregate: String,
    pub decimal: Option<String>,
    pub count: i64,
    pub currency: Option<String>,
    pub scale: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct RecalculatePreviewResponse {
    pub form_id: Uuid,
    pub schema_version: i32,
    pub values: Value,
    pub normalized_values: Value,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct FormAttachmentResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub form_id: Uuid,
    pub record_id: Option<Uuid>,
    pub field_id: String,
    pub field_key: String,
    pub file_name: String,
    pub content_type: String,
    pub byte_size: i64,
    pub storage_key: String,
    pub url: String,
    pub thumbnail_url: Option<String>,
    pub media_metadata: Value,
    pub created_by: Option<Uuid>,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, FromQueryResult, Clone)]
pub struct FormImportMappingTemplateResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub form_id: Uuid,
    pub name: String,
    pub header_signature: String,
    pub headers: Value,
    pub selections: Value,
    pub transforms: Value,
    pub shared: bool,
    pub created_by: Option<Uuid>,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, FromQueryResult, Clone)]
pub struct FormImportJobResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub form_id: Uuid,
    pub status: String,
    pub input: Value,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub created_by: Option<Uuid>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, FromQueryResult, Clone)]
pub struct FormExportJobResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub form_id: Uuid,
    pub status: String,
    pub input: Value,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub created_by: Option<Uuid>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, FromQueryResult, Clone)]
pub struct FormAttachmentPackageJobResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub form_id: Uuid,
    pub status: String,
    pub input: Value,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub created_by: Option<Uuid>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, FromQueryResult)]
struct ProjectWorkspace {
    workspace_id: Uuid,
}

pub async fn list_project_forms(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ListFormsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let project = find_project_workspace(&state, project_id).await?;
    let (_, role, _) =
        require_workspace_access_from_auth(&state, &claims, bot.as_ref().map(|b| &b.0), project.workspace_id).await?;
    let can_bypass_form_view = role_is_form_admin(&role);
    let permission_subject_id = normalize_permission_subject_id(&role)?;
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(50).clamp(1, 200);
    let offset = (page - 1) * per_page;

    let total = count_query(
        &state,
        r"
            SELECT COUNT(*)::bigint AS count
            FROM project_forms
            WHERE workspace_id = $1 AND project_id = $2
              AND archived_at IS NULL
              AND (
                  $3::boolean
                  OR COALESCE((
                      SELECT (form_permissions.policy -> 'actions' ->> 'form.view')::boolean
                      FROM form_permissions
                      WHERE form_permissions.form_id = project_forms.id
                        AND form_permissions.subject_type = 'role'
                        AND form_permissions.subject_id = $4
                      LIMIT 1
                  ), true)
              )
        ",
        vec![
            project.workspace_id.into(),
            project_id.into(),
            can_bypass_form_view.into(),
            permission_subject_id.clone().into(),
        ],
    )
    .await?;
    let items = FormResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, key, name, description, icon, color,
                   title_template, schema, detail_layout, schema_version, created_by, archived_at, created_at, updated_at
            FROM project_forms
            WHERE workspace_id = $1 AND project_id = $2
              AND archived_at IS NULL
              AND (
                  $5::boolean
                  OR COALESCE((
                      SELECT (form_permissions.policy -> 'actions' ->> 'form.view')::boolean
                      FROM form_permissions
                      WHERE form_permissions.form_id = project_forms.id
                        AND form_permissions.subject_type = 'role'
                        AND form_permissions.subject_id = $6
                      LIMIT 1
                  ), true)
              )
            ORDER BY updated_at DESC
            LIMIT $3 OFFSET $4
        ",
        vec![
            project.workspace_id.into(),
            project_id.into(),
            per_page.into(),
            offset.into(),
            can_bypass_form_view.into(),
            permission_subject_id.into(),
        ],
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages: total_pages(total, per_page),
    }))
}

pub async fn create_project_form(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<CreateFormRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (workspace_id, actor_id) =
        ensure_project_actor(&state, &claims, bot.as_ref().map(|b| &b.0), project_id).await?;
    let key = normalize_key(&req.key).map_err(ApiError::BadRequest)?;
    let name = required_trimmed(&req.name, "name")?;
    let schema = ensure_schema_field_ids(
        req.schema
            .unwrap_or_else(|| json!({ "version": "openpr.form.schema.v1", "fields": [] })),
    )
    .map_err(ApiError::BadRequest)?;
    validate_schema(&schema).map_err(ApiError::BadRequest)?;
    let detail_layout = ensure_json_object(req.detail_layout.unwrap_or_else(|| json!({})), "detail_layout")?;
    let form_id = Uuid::new_v4();
    let source = json!({ "type": if actor_id.is_some() { "user" } else { "bot" }, "actor_id": actor_id });
    let tx = state.db.begin().await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
                INSERT INTO project_forms (
                    id, workspace_id, project_id, key, name, description, icon, color,
                    title_template, schema, detail_layout, created_by, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now(), now())
            ",
        vec![
            form_id.into(),
            workspace_id.into(),
            project_id.into(),
            key.into(),
            name.into(),
            req.description.unwrap_or_default().into(),
            req.icon.into(),
            req.color.into(),
            req.title_template.unwrap_or_else(|| "{id}".to_string()).into(),
            schema.into(),
            detail_layout.into(),
            actor_id.into(),
        ],
    ))
    .await?;
    let form = find_form_with_conn(&tx, form_id).await?;
    insert_form_event(
        &tx,
        &form,
        None,
        "form.created",
        actor_id,
        source,
        json!({ "form_id": form.id, "key": form.key, "name": form.name }),
    )
    .await?;
    insert_schema_version(&tx, &form, actor_id, "initial schema").await?;
    tx.commit().await?;

    Ok(ApiResponse::success(form))
}

pub async fn duplicate_form(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Json(req): Json<DuplicateFormRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let source_form = find_form(&state, form_id).await?;
    let (actor_id, _, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &source_form, "form.design").await?;
    let key = match req.key.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
        Some(key) => {
            let key = normalize_key(key).map_err(ApiError::BadRequest)?;
            ensure_form_key_available(&state, source_form.project_id, &key).await?;
            key
        }
        None => next_duplicate_form_key(&state, source_form.project_id, &source_form.key).await?,
    };
    let name = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{} Copy", source_form.name));
    let description = req.description.unwrap_or_else(|| source_form.description.clone());
    let new_form_id = Uuid::new_v4();
    let source = json!({
        "type": if is_bot { "bot" } else { "user" },
        "actor_id": actor_id,
        "origin": "duplicate_form",
        "source_form_id": source_form.id
    });
    let tx = state.db.begin().await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO project_forms (
                id, workspace_id, project_id, key, name, description, icon, color,
                title_template, schema, detail_layout, created_by, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now(), now())
        ",
        vec![
            new_form_id.into(),
            source_form.workspace_id.into(),
            source_form.project_id.into(),
            key.into(),
            name.into(),
            description.into(),
            source_form.icon.clone().into(),
            source_form.color.clone().into(),
            source_form.title_template.clone().into(),
            source_form.schema.clone().into(),
            source_form.detail_layout.clone().into(),
            if is_bot { None::<Uuid> } else { Some(actor_id) }.into(),
        ],
    ))
    .await?;
    let duplicated_form = find_form_with_conn(&tx, new_form_id).await?;
    let copied_view_count = duplicate_form_views(
        &tx,
        &source_form,
        &duplicated_form,
        if is_bot { None } else { Some(actor_id) },
    )
    .await?;
    insert_form_event(
        &tx,
        &duplicated_form,
        None,
        "form.duplicated",
        if is_bot { None } else { Some(actor_id) },
        source,
        json!({
            "form_id": duplicated_form.id,
            "key": duplicated_form.key,
            "name": duplicated_form.name,
            "source_form_id": source_form.id,
            "source_form_key": source_form.key,
            "copied_view_count": copied_view_count
        }),
    )
    .await?;
    insert_schema_version(
        &tx,
        &duplicated_form,
        if is_bot { None } else { Some(actor_id) },
        "duplicated form baseline schema",
    )
    .await?;
    tx.commit().await?;

    Ok(ApiResponse::success(duplicated_form))
}

pub async fn create_project_form_from_template(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<CreateFormFromTemplateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (workspace_id, actor_id) =
        ensure_project_actor(&state, &claims, bot.as_ref().map(|b| &b.0), project_id).await?;
    let template_key = normalize_key(&req.template_key).map_err(ApiError::BadRequest)?;
    let template = load_form_scenario_template(&state, workspace_id, &template_key).await?;
    let requested_key = req
        .key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_form_key_from_template(&template.key));
    let form_key = normalize_key(&requested_key).map_err(ApiError::BadRequest)?;
    let name = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&template.name)
        .to_string();
    let description = req
        .description
        .unwrap_or_else(|| format!("{} ({})", template.description, template.industry));
    let schema = normalize_scenario_field_schema(template.field_schema)?;
    let title_template = req
        .title_template
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_title_template_from_schema(&schema));
    let detail_layout = default_detail_layout_for_schema(&schema);
    let form_id = Uuid::new_v4();
    let source = json!({
        "type": if actor_id.is_some() { "user" } else { "bot" },
        "actor_id": actor_id,
        "origin": "scenario_template",
        "scenario_template_key": template.key
    });
    let tx = state.db.begin().await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
                INSERT INTO project_forms (
                    id, workspace_id, project_id, key, name, description, icon, color,
                    title_template, schema, detail_layout, created_by, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now(), now())
            ",
        vec![
            form_id.into(),
            workspace_id.into(),
            project_id.into(),
            form_key.into(),
            name.into(),
            description.into(),
            Option::<String>::None.into(),
            Some("#2563eb".to_string()).into(),
            title_template.into(),
            schema.clone().into(),
            detail_layout.clone().into(),
            actor_id.into(),
        ],
    ))
    .await?;
    let form = find_form_with_conn(&tx, form_id).await?;
    create_default_template_views(&tx, &form, &schema, actor_id).await?;
    insert_form_event(
        &tx,
        &form,
        None,
        "form.created_from_template",
        actor_id,
        source,
        json!({
            "form_id": form.id,
            "key": form.key,
            "name": form.name,
            "scenario_template_key": template.key
        }),
    )
    .await?;
    insert_schema_version(&tx, &form, actor_id, "scenario template baseline schema").await?;
    tx.commit().await?;

    Ok(ApiResponse::success(form))
}

pub async fn get_form(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.view").await?;
    Ok(ApiResponse::success(form))
}

pub async fn update_form(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Json(req): Json<UpdateFormRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, _, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.design").await?;

    let mut set_parts = Vec::new();
    let mut values = Vec::new();
    let mut changed_fields = Vec::new();
    let mut schema_changed = false;
    let mut idx = 1;
    macro_rules! push_set {
        ($column:literal, $value:expr) => {{
            set_parts.push(format!("{} = ${}", $column, idx));
            values.push($value);
            idx += 1;
        }};
    }

    if let Some(name) = req.name {
        let name = required_trimmed(&name, "name")?;
        changed_fields.push("name");
        push_set!("name", name.into());
    }
    if let Some(description) = req.description {
        changed_fields.push("description");
        push_set!("description", description.into());
    }
    if let Some(icon) = req.icon {
        changed_fields.push("icon");
        push_set!("icon", icon.into());
    }
    if let Some(color) = req.color {
        changed_fields.push("color");
        push_set!("color", color.into());
    }
    if let Some(title_template) = req.title_template {
        changed_fields.push("title_template");
        push_set!("title_template", title_template.into());
    }
    if let Some(schema) = req.schema {
        let schema = ensure_schema_field_ids(schema).map_err(ApiError::BadRequest)?;
        validate_schema(&schema).map_err(ApiError::BadRequest)?;
        changed_fields.push("schema");
        schema_changed = true;
        push_set!("schema", schema.into());
    }
    if let Some(detail_layout) = req.detail_layout {
        changed_fields.push("detail_layout");
        schema_changed = true;
        push_set!(
            "detail_layout",
            ensure_json_object(detail_layout, "detail_layout")?.into()
        );
    }
    if set_parts.is_empty() {
        return Ok(ApiResponse::success(form));
    }
    if schema_changed {
        if let Some(expected) = req.expected_schema_version {
            if expected != form.schema_version {
                return Err(ApiError::Conflict(format!(
                    "schema version conflict: expected {}, current {}",
                    expected, form.schema_version
                )));
            }
        }
        set_parts.push("schema_version = schema_version + 1".to_string());
    }
    set_parts.push("updated_at = now()".to_string());
    values.push(form_id.into());
    let sql = format!("UPDATE project_forms SET {} WHERE id = ${idx}", set_parts.join(", "));
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres, &sql, values))
        .await?;
    let updated_form = find_form_with_conn(&tx, form_id).await?;
    let source = json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id });
    insert_form_event(
        &tx,
        &updated_form,
        None,
        "form.updated",
        if is_bot { None } else { Some(actor_id) },
        source,
        json!({
            "form_id": updated_form.id,
            "key": updated_form.key,
            "changed_fields": changed_fields
        }),
    )
    .await?;
    if schema_changed {
        insert_schema_version(
            &tx,
            &updated_form,
            if is_bot { None } else { Some(actor_id) },
            "schema updated",
        )
        .await?;
    }
    tx.commit().await?;

    Ok(ApiResponse::success(updated_form))
}

pub async fn delete_form(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, _, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.design").await?;
    let tx = state.db.begin().await?;
    insert_form_event(
        &tx,
        &form,
        None,
        "form.archived",
        if is_bot { None } else { Some(actor_id) },
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({ "form_id": form.id, "key": form.key, "name": form.name }),
    )
    .await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE project_forms SET archived_at = COALESCE(archived_at, now()), updated_at = now() WHERE id = $1",
        vec![form_id.into()],
    ))
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::ok())
}

pub async fn restore_form(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, _, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.design").await?;
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE project_forms SET archived_at = NULL, updated_at = now() WHERE id = $1",
        vec![form_id.into()],
    ))
    .await?;
    let restored = find_form_with_conn(&tx, form_id).await?;
    insert_form_event(
        &tx,
        &restored,
        None,
        "form.restored",
        if is_bot { None } else { Some(actor_id) },
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({ "form_id": restored.id, "key": restored.key, "name": restored.name }),
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(restored))
}

pub async fn create_form_view(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Json(req): Json<CreateViewRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, _, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.design").await?;
    let key = normalize_key(&req.key).map_err(ApiError::BadRequest)?;
    let name = required_trimmed(&req.name, "name")?;
    let view_type = normalize_view_type(&req.view_type)?;
    let config = normalize_view_config(req.config.unwrap_or_else(|| json!({})))?;
    let mark_default = config_marks_default(&config);
    let view_id = Uuid::new_v4();
    let tx = state.db.begin().await?;
    if mark_default {
        clear_default_form_views(&tx, form.id, None).await?;
    }

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
                INSERT INTO form_views (
                    id, workspace_id, project_id, form_id, key, name, view_type,
                    config, created_by, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now(), now())
            ",
        vec![
            view_id.into(),
            form.workspace_id.into(),
            form.project_id.into(),
            form.id.into(),
            key.into(),
            name.into(),
            view_type.into(),
            config.into(),
            if is_bot { None::<Uuid> } else { Some(actor_id) }.into(),
        ],
    ))
    .await?;

    let view = find_view_with_conn(&tx, view_id).await?;
    insert_form_event(
        &tx,
        &form,
        None,
        "form.view.created",
        if is_bot { None } else { Some(actor_id) },
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({
            "form_id": form.id,
            "view_id": view.id,
            "key": view.key,
            "view_type": view.view_type
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(view))
}

pub async fn list_form_views(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.view").await?;
    let can_view_private = is_bot || role_is_form_admin(&role);
    let views = ViewResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, form_id, key, name, view_type,
                   config, created_by, archived_at, created_at, updated_at
            FROM form_views
            WHERE form_id = $1
              AND archived_at IS NULL
              AND (
                  COALESCE(config->>'visibility', 'shared') <> 'private'
                  OR created_by = $2
                  OR $3 = true
              )
            ORDER BY (config->>'is_default' = 'true') DESC,
                     created_at ASC
        ",
        vec![form_id.into(), actor_id.into(), can_view_private.into()],
    ))
    .all(&state.db)
    .await?;
    Ok(ApiResponse::success(views))
}

pub async fn update_form_view(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(view_id): Path<Uuid>,
    Json(req): Json<UpdateViewRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let view = find_view(&state, view_id).await?;
    let form = find_form(&state, view.form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.design").await?;
    ensure_can_manage_view(&view, actor_id, &role, is_bot)?;
    if let Some(expected_updated_at) = req.expected_updated_at {
        if expected_updated_at != view.updated_at {
            return Err(ApiError::Conflict(format!(
                "view update conflict: expected {}, current {}",
                expected_updated_at.to_rfc3339(),
                view.updated_at.to_rfc3339()
            )));
        }
    }
    let mut set_parts = Vec::new();
    let mut values = Vec::new();
    let mut changed_fields = Vec::new();
    let mut idx = 1;
    macro_rules! push_set {
        ($column:literal, $value:expr) => {{
            set_parts.push(format!("{} = ${}", $column, idx));
            values.push($value);
            idx += 1;
        }};
    }

    if let Some(name) = req.name {
        changed_fields.push("name");
        push_set!("name", required_trimmed(&name, "name")?.into());
    }
    if let Some(view_type) = req.view_type {
        changed_fields.push("view_type");
        push_set!("view_type", normalize_view_type(&view_type)?.into());
    }
    let mut mark_default = false;
    if let Some(config) = req.config {
        changed_fields.push("config");
        let config = normalize_view_config(config)?;
        mark_default = config_marks_default(&config);
        push_set!("config", config.into());
    }
    if set_parts.is_empty() {
        return Ok(ApiResponse::success(view));
    }

    set_parts.push("updated_at = now()".to_string());
    values.push(view_id.into());
    let sql = format!("UPDATE form_views SET {} WHERE id = ${idx}", set_parts.join(", "));
    let tx = state.db.begin().await?;
    if mark_default {
        clear_default_form_views(&tx, form.id, Some(view_id)).await?;
    }
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres, &sql, values))
        .await?;
    let updated = find_view_with_conn(&tx, view_id).await?;
    insert_form_event(
        &tx,
        &form,
        None,
        "form.view.updated",
        if is_bot { None } else { Some(actor_id) },
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({
            "form_id": form.id,
            "view_id": updated.id,
            "key": updated.key,
            "view_type": updated.view_type,
            "changed_fields": changed_fields
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(updated))
}

pub async fn delete_form_view(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(view_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let view = find_view(&state, view_id).await?;
    let form = find_form(&state, view.form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.design").await?;
    ensure_can_manage_view(&view, actor_id, &role, is_bot)?;
    let tx = state.db.begin().await?;
    insert_form_event(
        &tx,
        &form,
        None,
        "form.view.archived",
        if is_bot { None } else { Some(actor_id) },
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({
            "form_id": form.id,
            "view_id": view.id,
            "key": view.key,
            "view_type": view.view_type
        }),
    )
    .await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE form_views SET archived_at = COALESCE(archived_at, now()), updated_at = now() WHERE id = $1",
        vec![view_id.into()],
    ))
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::ok())
}

pub async fn list_form_records(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Query(query): Query<ListRecordsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, _) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.view").await?;
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(50).clamp(1, 200);
    let offset = (page - 1) * per_page;
    let fields = parse_fields(&form.schema).map_err(ApiError::BadRequest)?;
    let mut values: Vec<sea_orm::Value> = vec![form_id.into()];
    let mut joins = Vec::new();
    let mut where_parts = vec![
        "form_records.form_id = $1".to_string(),
        "form_records.archived_at IS NULL".to_string(),
    ];
    let mut next_idx = 2;
    append_record_scope_sql(
        &state,
        form.id,
        &role,
        actor_id,
        &mut where_parts,
        &mut values,
        &mut next_idx,
    )
    .await?;

    let view_query = record_query_config_from_view(&state, form_id, query.view_id).await?;
    let sort = query.sort.as_deref().or(view_query.sort.as_deref());
    let query_filter_expression = query_filter_expression(
        query.filter_field.as_deref(),
        query.filter_op.clone(),
        query.filter_value.clone(),
    )
    .or(view_query.filter_expression);
    append_record_filter_expression_sql(
        &fields,
        query_filter_expression.as_ref(),
        &mut where_parts,
        &mut values,
        &mut next_idx,
    )?;

    let (sort_join, order_by, sort_value) = record_sort_sql(sort, &fields, next_idx)?;
    if let Some(join) = sort_join {
        joins.push(join);
        if let Some(value) = sort_value {
            values.push(value.into());
        }
    }
    let join_sql = if joins.is_empty() {
        String::new()
    } else {
        format!(" {}", joins.join(" "))
    };
    let where_sql = where_parts.join(" AND ");
    let total = count_query(
        &state,
        &format!("SELECT COUNT(*)::bigint AS count FROM form_records{join_sql} WHERE {where_sql}"),
        values.clone(),
    )
    .await?;
    let mut item_values = values;
    item_values.push(per_page.into());
    item_values.push(offset.into());
    let limit_idx = item_values.len() - 1;
    let offset_idx = item_values.len();
    let items = RecordResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &format!(
            r"
            SELECT form_records.id, form_records.workspace_id, form_records.project_id,
                   form_records.form_id, form_records.title, form_records.values,
                   form_records.source, form_records.schema_version, form_records.created_by,
                   form_records.updated_by, form_records.archived_at, form_records.created_at,
                   form_records.updated_at
            FROM form_records
            {join_sql}
            WHERE {where_sql}
            ORDER BY {order_by}
            LIMIT ${limit_idx} OFFSET ${offset_idx}
        "
        ),
        item_values,
    ))
    .all(&state.db)
    .await?;
    let denied_read_fields = denied_read_field_keys(&state, form.id, &role).await?;
    let items = items
        .into_iter()
        .map(|record| filter_record_response_values(record, &denied_read_fields))
        .collect();

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages: total_pages(total, per_page),
    }))
}

pub async fn export_form_records(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Query(query): Query<ExportRecordsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.export").await?;
    let response = export_records_for_form(&state, &form, actor_id, &role, is_bot, &query).await?;
    Ok(ApiResponse::success(response))
}

pub async fn list_form_export_jobs(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.export").await?;
    let mut values: Vec<sea_orm::Value> = vec![form_id.into()];
    let owner_filter = job_listing_owner_filter(actor_id, &role, is_bot, &mut values);
    let jobs = FormExportJobResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r"
            SELECT id, workspace_id, project_id, form_id, status, input, result, error,
                   created_by, started_at, completed_at, created_at, updated_at
            FROM form_export_jobs
            WHERE form_id = $1{owner_filter}
            ORDER BY created_at DESC
            LIMIT 100
        "
        ),
        values,
    ))
    .all(&state.db)
    .await?;
    let jobs = jobs
        .into_iter()
        .map(|mut job| {
            job.result = summarize_job_result(job.result);
            job
        })
        .collect::<Vec<_>>();
    Ok(ApiResponse::success(jobs))
}

pub async fn get_form_export_job(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(job_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let job = find_export_job(&state, job_id).await?;
    let form = find_form(&state, job.form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.export").await?;
    ensure_can_read_job_result(job.created_by, actor_id, &role, is_bot)?;
    Ok(ApiResponse::success(job))
}

pub async fn create_form_export_job(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Json(req): Json<CreateExportJobRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.export").await?;
    let input = add_form_job_worker_context(create_export_job_input(&req)?, actor_id, &role, is_bot)?;
    let query = export_records_query_from_job(req)?;
    let job_id = insert_export_job(&state, &form, actor_id, is_bot, input).await?;
    let job = find_export_job(&state, job_id).await?;
    spawn_form_export_job(state.clone(), form, actor_id, role, is_bot, job_id, query);
    Ok(ApiResponse::success(job))
}

fn spawn_form_export_job(
    state: AppState,
    form: FormResponse,
    actor_id: Uuid,
    role: String,
    is_bot: bool,
    job_id: Uuid,
    query: ExportRecordsQuery,
) {
    tokio::spawn(async move {
        if let Err(error) = run_form_export_job(state, form, actor_id, role, is_bot, job_id, query).await {
            tracing::warn!(job_id = %job_id, error = %error, "form export job failed before status update");
        }
    });
}

async fn run_form_export_job(
    state: AppState,
    form: FormResponse,
    actor_id: Uuid,
    role: String,
    is_bot: bool,
    job_id: Uuid,
    query: ExportRecordsQuery,
) -> Result<(), ApiError> {
    if !claim_export_job_running(&state, job_id).await? {
        return Ok(());
    }
    let export_result = match export_records_for_form(&state, &form, actor_id, &role, is_bot, &query).await {
        Ok(result) => serde_json::to_value(result).map_err(|_| ApiError::Internal),
        Err(error) => Err(error),
    };
    match export_result {
        Ok(result) => {
            update_export_job_completed(&state, job_id, result).await?;
        }
        Err(error) => {
            update_export_job_failed(&state, job_id, error.to_string()).await?;
        }
    }
    Ok(())
}

async fn export_records_for_form(
    state: &AppState,
    form: &FormResponse,
    actor_id: Uuid,
    role: &str,
    is_bot: bool,
    query: &ExportRecordsQuery,
) -> Result<ExportRecordsResponse, ApiError> {
    let fields = parse_fields(&form.schema).map_err(ApiError::BadRequest)?;
    let denied_read_fields = denied_read_field_keys(state, form.id, role).await?;
    let export_scope = export_records_scope(query.scope.as_deref())?;
    let include_archived = export_records_include_archived(
        query.include_archived.unwrap_or(false),
        is_bot || role_is_form_admin(role),
    )?;
    let row_limit = export_records_limit(query.limit)?;
    let selected_keys = export_column_keys(state, form.id, &fields, query)
        .await?
        .into_iter()
        .filter(|key| !denied_read_fields.contains(key))
        .collect::<Vec<_>>();
    let view_query = if export_scope == "view" {
        record_query_config_from_view(state, form.id, query.view_id).await?
    } else {
        if let Some(view_id) = query.view_id {
            ensure_export_view_access(state, form.id, view_id).await?;
        }
        RecordQueryConfig::default()
    };
    let selected_fields = selected_keys
        .iter()
        .map(|key| find_schema_field(&fields, key))
        .collect::<Result<Vec<_>, _>>()?;
    let columns = selected_fields
        .iter()
        .map(|field| ExportColumnResponse {
            key: field.key.clone(),
            label: field.label.clone(),
        })
        .collect::<Vec<_>>();

    let mut values: Vec<sea_orm::Value> = vec![form.id.into()];
    let mut joins = Vec::new();
    let mut where_parts = vec!["form_records.form_id = $1".to_string()];
    if !include_archived {
        where_parts.push("form_records.archived_at IS NULL".to_string());
    }
    let mut next_idx = 2;
    append_record_scope_sql(
        state,
        form.id,
        role,
        actor_id,
        &mut where_parts,
        &mut values,
        &mut next_idx,
    )
    .await?;
    append_record_filter_expression_sql(
        &fields,
        view_query.filter_expression.as_ref(),
        &mut where_parts,
        &mut values,
        &mut next_idx,
    )?;
    let (sort_join, order_by, sort_value) = record_sort_sql(view_query.sort.as_deref(), &fields, next_idx)?;
    if let Some(join) = sort_join {
        joins.push(join);
        if let Some(value) = sort_value {
            values.push(value.into());
        }
    }
    values.push(row_limit.into());
    let limit_idx = values.len();
    let join_sql = if joins.is_empty() {
        String::new()
    } else {
        format!(" {}", joins.join(" "))
    };
    let where_sql = where_parts.join(" AND ");

    let records = RecordResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &format!(
            r"
                SELECT form_records.id, form_records.workspace_id, form_records.project_id,
                       form_records.form_id, form_records.title, form_records.values,
                       form_records.source, form_records.schema_version, form_records.created_by,
                       form_records.updated_by, form_records.archived_at, form_records.created_at,
                       form_records.updated_at
                FROM form_records
                {join_sql}
                WHERE {where_sql}
                ORDER BY {order_by}
                LIMIT ${limit_idx}
            "
        ),
        values,
    ))
    .all(&state.db)
    .await?;

    let rows = records
        .iter()
        .map(|record| {
            let mut values = BTreeMap::new();
            for field in &selected_fields {
                let display = record
                    .values
                    .get(&field.key)
                    .map(export_display_value)
                    .unwrap_or_default();
                values.insert(field.key.clone(), display);
            }
            ExportRecordRowResponse {
                record_id: record.id,
                title: record.title.clone(),
                values,
            }
        })
        .collect::<Vec<_>>();
    let format = export_records_format(query.format.as_deref())?;
    let csv = (format == "csv").then(|| build_records_csv(&columns, &rows));
    let extension = if format == "json" { "json" } else { "csv" };
    let row_count = rows.len();

    Ok(ExportRecordsResponse {
        form_id: form.id,
        format: format.to_string(),
        file_name: format!("{}_records.{extension}", sanitize_file_stem(&form.key)),
        export_policy: ExportRecordsPolicyResponse {
            scope: export_scope.to_string(),
            view_id: query.view_id,
            include_archived,
            row_limit,
            row_count,
            truncated: i64::try_from(row_count).unwrap_or(i64::MAX) >= row_limit,
        },
        columns,
        rows,
        csv,
    })
}

pub async fn export_form_attachment_package(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Query(query): Query<ExportAttachmentPackageQuery>,
) -> Result<Response, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, _) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.export").await?;
    let package = build_attachment_package_for_form(&state, &form, actor_id, &role, query.view_id).await?;
    let content_disposition = HeaderValue::from_str(&format!("attachment; filename=\"{}\"", package.file_name))
        .map_err(|_| ApiError::Internal)?;

    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("application/zip")),
            (header::CONTENT_DISPOSITION, content_disposition),
            (
                HeaderName::from_static("x-openpr-attachment-count"),
                HeaderValue::from_str(&package.attachment_count.to_string()).map_err(|_| ApiError::Internal)?,
            ),
            (
                HeaderName::from_static("x-openpr-attachment-file-count"),
                HeaderValue::from_str(&package.binary_file_count.to_string()).map_err(|_| ApiError::Internal)?,
            ),
        ],
        package.zip,
    )
        .into_response())
}

pub async fn list_form_attachment_package_jobs(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.export").await?;
    let mut values: Vec<sea_orm::Value> = vec![form_id.into()];
    let owner_filter = job_listing_owner_filter(actor_id, &role, is_bot, &mut values);
    let jobs = FormAttachmentPackageJobResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r"
            SELECT id, workspace_id, project_id, form_id, status, input, result, error,
                   created_by, started_at, completed_at, created_at, updated_at
            FROM form_attachment_package_jobs
            WHERE form_id = $1{owner_filter}
            ORDER BY created_at DESC
            LIMIT 100
        "
        ),
        values,
    ))
    .all(&state.db)
    .await?;
    let jobs = jobs
        .into_iter()
        .map(|mut job| {
            job.result = summarize_job_result(job.result);
            job
        })
        .collect::<Vec<_>>();
    Ok(ApiResponse::success(jobs))
}

pub async fn get_form_attachment_package_job(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(job_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let job = find_attachment_package_job(&state, job_id).await?;
    let form = find_form(&state, job.form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.export").await?;
    ensure_can_read_job_result(job.created_by, actor_id, &role, is_bot)?;
    Ok(ApiResponse::success(job))
}

pub async fn download_form_attachment_package_job(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(job_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let job = find_attachment_package_job(&state, job_id).await?;
    let form = find_form(&state, job.form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.export").await?;
    ensure_can_read_job_result(job.created_by, actor_id, &role, is_bot)?;
    if job.status != "completed" {
        return Err(ApiError::BadRequest(
            "attachment package job is not completed".to_string(),
        ));
    }
    let result = job
        .result
        .as_ref()
        .ok_or_else(|| ApiError::BadRequest("attachment package job result is missing".to_string()))?;
    let stored_file_name = attachment_package_job_result_string(result, "stored_file_name")?;
    let file_name = attachment_package_job_result_string(result, "file_name")?;
    let expires_at = attachment_package_job_result_string(result, "expires_at")?;
    let expires_at = chrono::DateTime::parse_from_rfc3339(&expires_at)
        .map_err(|_| ApiError::BadRequest("attachment package job expiry is invalid".to_string()))?
        .with_timezone(&Utc);
    let artifact_key = attachment_package_artifact_key(&stored_file_name);
    let object_storage = ObjectStorage::from_runtime_config()?;
    if Utc::now() > expires_at {
        let _ = object_storage.delete(&artifact_key).await;
        return Err(ApiError::BadRequest(
            "attachment package job artifact expired".to_string(),
        ));
    }

    let zip = object_storage
        .get(&artifact_key)
        .await
        .map_err(|_| ApiError::NotFound("attachment package job artifact not found".to_string()))?;
    let content_disposition =
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", file_name)).map_err(|_| ApiError::Internal)?;
    let attachment_count = result
        .get("attachment_count")
        .and_then(Value::as_u64)
        .unwrap_or_default()
        .to_string();
    let binary_file_count = result
        .get("binary_file_count")
        .and_then(Value::as_u64)
        .unwrap_or_default()
        .to_string();

    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("application/zip")),
            (header::CONTENT_DISPOSITION, content_disposition),
            (
                HeaderName::from_static("x-openpr-attachment-count"),
                HeaderValue::from_str(&attachment_count).map_err(|_| ApiError::Internal)?,
            ),
            (
                HeaderName::from_static("x-openpr-attachment-file-count"),
                HeaderValue::from_str(&binary_file_count).map_err(|_| ApiError::Internal)?,
            ),
        ],
        zip,
    )
        .into_response())
}

pub async fn create_form_attachment_package_job(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Json(req): Json<CreateAttachmentPackageJobRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.export").await?;
    let input = add_form_job_worker_context(
        serde_json::to_value(&req).map_err(|_| ApiError::Internal)?,
        actor_id,
        &role,
        is_bot,
    )?;
    let query = ExportAttachmentPackageQuery { view_id: req.view_id };
    let job_id = insert_attachment_package_job(&state, &form, actor_id, is_bot, input).await?;
    let job = find_attachment_package_job(&state, job_id).await?;
    spawn_form_attachment_package_job(state.clone(), form, actor_id, role, job_id, query);
    Ok(ApiResponse::success(job))
}

fn spawn_form_attachment_package_job(
    state: AppState,
    form: FormResponse,
    actor_id: Uuid,
    role: String,
    job_id: Uuid,
    query: ExportAttachmentPackageQuery,
) {
    tokio::spawn(async move {
        if let Err(error) = run_form_attachment_package_job(state, form, actor_id, role, job_id, query).await {
            tracing::warn!(job_id = %job_id, error = %error, "form attachment package job failed before status update");
        }
    });
}

async fn run_form_attachment_package_job(
    state: AppState,
    form: FormResponse,
    actor_id: Uuid,
    role: String,
    job_id: Uuid,
    query: ExportAttachmentPackageQuery,
) -> Result<(), ApiError> {
    if !claim_attachment_package_job_running(&state, job_id).await? {
        return Ok(());
    }
    let package_result = match build_attachment_package_for_form(&state, &form, actor_id, &role, query.view_id).await {
        Ok(package) => write_attachment_package_artifact(job_id, package).await,
        Err(error) => Err(error),
    };
    match package_result {
        Ok(result) => {
            update_attachment_package_job_completed(&state, job_id, result).await?;
        }
        Err(error) => {
            update_attachment_package_job_failed(&state, job_id, error.to_string()).await?;
        }
    }
    Ok(())
}

async fn build_attachment_package_for_form(
    state: &AppState,
    form: &FormResponse,
    actor_id: Uuid,
    role: &str,
    view_id: Option<Uuid>,
) -> Result<AttachmentPackageArtifact, ApiError> {
    let fields = parse_fields(&form.schema).map_err(ApiError::BadRequest)?;
    let denied_read_fields = denied_read_field_keys(state, form.id, role).await?;
    let record_ids = visible_form_record_ids_for_export(state, form, &fields, actor_id, role, view_id).await?;
    let attachments = form_attachments_for_package(state, form.id, Some(&record_ids), &denied_read_fields).await?;
    let (entries, binary_file_count) = attachment_package_entries(form, view_id, &attachments).await?;
    let zip = build_stored_zip(&entries)?;
    let file_name = format!("{}_attachments.zip", sanitize_file_stem(&form.key));
    Ok(AttachmentPackageArtifact {
        file_name,
        zip,
        attachment_count: attachments.len(),
        binary_file_count,
    })
}

pub async fn preview_import_form_records(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Json(req): Json<ImportRecordsRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.create").await?;
    let previews = preview_import_rows(&state, &form, actor_id, &role, is_bot, &req.rows).await?;
    Ok(ApiResponse::success(import_preview_response(&form, previews)))
}

pub async fn import_form_records(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Json(req): Json<ImportRecordsRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.create").await?;
    Ok(ApiResponse::success(
        import_form_record_rows(&state, &form, actor_id, &role, is_bot, req).await?,
    ))
}

pub async fn preview_import_form_records_from_file(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Json(req): Json<ImportRecordsFileRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.create").await?;
    let import_req = import_records_request_from_uploaded_file(&state, &form, req, actor_id, &role, is_bot).await?;
    let previews = preview_import_rows(&state, &form, actor_id, &role, is_bot, &import_req.rows).await?;
    Ok(ApiResponse::success(import_preview_response(&form, previews)))
}

pub async fn import_form_records_from_file(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Json(req): Json<ImportRecordsFileRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.create").await?;
    let import_req = import_records_request_from_uploaded_file(&state, &form, req, actor_id, &role, is_bot).await?;
    Ok(ApiResponse::success(
        import_form_record_rows(&state, &form, actor_id, &role, is_bot, import_req).await?,
    ))
}

pub async fn list_form_import_mapping_templates(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Query(query): Query<ListImportMappingTemplatesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.create").await?;
    let can_view_all = is_bot || role_is_form_admin(&role);
    let mut values: Vec<sea_orm::Value> = vec![form.id.into(), actor_id.into(), can_view_all.into()];
    let mut where_parts = vec![
        "form_id = $1".to_string(),
        "archived_at IS NULL".to_string(),
        "(shared = true OR created_by = $2 OR $3 = true)".to_string(),
    ];
    if let Some(signature) = query
        .header_signature
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        values.push(signature.to_string().into());
        where_parts.push(format!("header_signature = ${}", values.len()));
    }
    let where_sql = where_parts.join(" AND ");
    let items = FormImportMappingTemplateResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &format!(
            r"
                SELECT id, workspace_id, project_id, form_id, name, header_signature,
                       headers, selections, transforms, shared, created_by,
                       archived_at, created_at, updated_at
                FROM form_import_mapping_templates
                WHERE {where_sql}
                ORDER BY updated_at DESC
                LIMIT 100
            "
        ),
        values,
    ))
    .all(&state.db)
    .await?;
    Ok(ApiResponse::success(items))
}

pub async fn save_form_import_mapping_template(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Json(req): Json<SaveImportMappingTemplateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.create").await?;
    let fields = parse_fields(&form.schema).map_err(ApiError::BadRequest)?;
    let normalized = normalize_import_mapping_template_request(req, &fields)?;
    let tx = state.db.begin().await?;
    let template_id = if let Some(template_id) = normalized.id {
        let existing = find_import_mapping_template_with_conn(&tx, template_id).await?;
        if existing.form_id != form.id {
            return Err(ApiError::BadRequest("template_id does not belong to form".to_string()));
        }
        ensure_can_manage_import_mapping_template(&existing, actor_id, &role, is_bot)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE form_import_mapping_templates
                SET name = $1, header_signature = $2, headers = $3, selections = $4,
                    transforms = $5, shared = $6, updated_at = now()
                WHERE id = $7
            ",
            vec![
                normalized.name.into(),
                normalized.header_signature.into(),
                normalized.headers.into(),
                normalized.selections.into(),
                normalized.transforms.into(),
                normalized.shared.into(),
                template_id.into(),
            ],
        ))
        .await?;
        template_id
    } else {
        let template_id = Uuid::new_v4();
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO form_import_mapping_templates (
                    id, workspace_id, project_id, form_id, name, header_signature,
                    headers, selections, transforms, shared, created_by, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, now(), now())
            ",
            vec![
                template_id.into(),
                form.workspace_id.into(),
                form.project_id.into(),
                form.id.into(),
                normalized.name.into(),
                normalized.header_signature.into(),
                normalized.headers.into(),
                normalized.selections.into(),
                normalized.transforms.into(),
                normalized.shared.into(),
                if is_bot { None::<Uuid> } else { Some(actor_id) }.into(),
            ],
        ))
        .await?;
        template_id
    };
    let template = find_import_mapping_template_with_conn(&tx, template_id).await?;
    insert_form_event(
        &tx,
        &form,
        None,
        "form.import_mapping_template.saved",
        if is_bot { None } else { Some(actor_id) },
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({
            "template_id": template.id,
            "header_signature": template.header_signature,
            "shared": template.shared
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(template))
}

pub async fn delete_form_import_mapping_template(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(template_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let template = find_import_mapping_template(&state, template_id).await?;
    if template.archived_at.is_some() {
        return Err(ApiError::NotFound("import mapping template not found".to_string()));
    }
    let form = find_form(&state, template.form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.create").await?;
    ensure_can_manage_import_mapping_template(&template, actor_id, &role, is_bot)?;
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE form_import_mapping_templates SET archived_at = now(), updated_at = now() WHERE id = $1",
        vec![template_id.into()],
    ))
    .await?;
    insert_form_event(
        &tx,
        &form,
        None,
        "form.import_mapping_template.archived",
        if is_bot { None } else { Some(actor_id) },
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({
            "template_id": template.id,
            "header_signature": template.header_signature,
            "shared": template.shared
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(json!({ "id": template_id })))
}

pub async fn list_form_import_jobs(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.create").await?;
    let mut values: Vec<sea_orm::Value> = vec![form_id.into()];
    let owner_filter = job_listing_owner_filter(actor_id, &role, is_bot, &mut values);
    let jobs = FormImportJobResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r"
            SELECT id, workspace_id, project_id, form_id, status, input, result, error,
                   created_by, started_at, completed_at, created_at, updated_at
            FROM form_import_jobs
            WHERE form_id = $1{owner_filter}
            ORDER BY created_at DESC
            LIMIT 100
        "
        ),
        values,
    ))
    .all(&state.db)
    .await?;
    let jobs = jobs
        .into_iter()
        .map(|mut job| {
            job.result = summarize_job_result(job.result);
            job
        })
        .collect::<Vec<_>>();
    Ok(ApiResponse::success(jobs))
}

pub async fn get_form_import_job(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(job_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let job = find_import_job(&state, job_id).await?;
    let form = find_form(&state, job.form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.create").await?;
    ensure_can_read_job_result(job.created_by, actor_id, &role, is_bot)?;
    Ok(ApiResponse::success(job))
}

pub async fn create_form_import_job(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Json(req): Json<CreateImportJobRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.create").await?;
    let input = add_form_job_worker_context(create_import_job_input(&req)?, actor_id, &role, is_bot)?;
    let job_id = insert_import_job(&state, &form, actor_id, is_bot, input).await?;
    let job = find_import_job(&state, job_id).await?;
    spawn_form_import_job(state.clone(), form, actor_id, role, is_bot, job_id, req);
    Ok(ApiResponse::success(job))
}

fn spawn_form_import_job(
    state: AppState,
    form: FormResponse,
    actor_id: Uuid,
    role: String,
    is_bot: bool,
    job_id: Uuid,
    req: CreateImportJobRequest,
) {
    tokio::spawn(async move {
        if let Err(error) = run_form_import_job(state, form, actor_id, role, is_bot, job_id, req).await {
            tracing::warn!(job_id = %job_id, error = %error, "form import job failed before status update");
        }
    });
}

async fn run_form_import_job(
    state: AppState,
    form: FormResponse,
    actor_id: Uuid,
    role: String,
    is_bot: bool,
    job_id: Uuid,
    req: CreateImportJobRequest,
) -> Result<(), ApiError> {
    if !claim_import_job_running(&state, job_id).await? {
        return Ok(());
    }
    let import_req_result = import_records_request_from_job(&state, &form, req, actor_id, &role, is_bot).await;
    let import_result = match import_req_result {
        Ok(import_req) => match import_form_record_rows(&state, &form, actor_id, &role, is_bot, import_req).await {
            Ok(result) => serde_json::to_value(result).map_err(|_| ApiError::Internal),
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    match import_result {
        Ok(result) => {
            update_import_job_completed(&state, job_id, result).await?;
        }
        Err(error) => {
            update_import_job_failed(&state, job_id, error.to_string()).await?;
        }
    }
    Ok(())
}

pub async fn process_pending_form_jobs_from_worker(state: AppState, concurrency: usize) -> Result<usize, ApiError> {
    let limit = i64::try_from(concurrency.max(1)).unwrap_or(i64::MAX) * 3;
    let mut processed = 0;
    processed += process_pending_form_import_jobs_from_worker(&state, limit).await?;
    processed += process_pending_form_export_jobs_from_worker(&state, limit).await?;
    processed += process_pending_form_attachment_package_jobs_from_worker(&state, limit).await?;
    processed += cleanup_expired_attachment_package_artifacts_from_worker(&state, limit).await?;
    processed += cleanup_expired_signature_values_from_worker(&state, limit).await?;
    Ok(processed)
}

async fn process_pending_form_import_jobs_from_worker(state: &AppState, limit: i64) -> Result<usize, ApiError> {
    let jobs = FormImportJobResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, form_id, status, input, result, error,
                   created_by, started_at, completed_at, created_at, updated_at
            FROM form_import_jobs
            WHERE status = 'queued'
            ORDER BY created_at
            LIMIT $1
        ",
        vec![limit.into()],
    ))
    .all(&state.db)
    .await?;
    let mut processed = 0;
    for job in jobs {
        if process_form_import_job_from_worker(state, job).await? {
            processed += 1;
        }
    }
    Ok(processed)
}

async fn process_form_import_job_from_worker(state: &AppState, job: FormImportJobResponse) -> Result<bool, ApiError> {
    let req = match serde_json::from_value::<CreateImportJobRequest>(job.input.clone()) {
        Ok(req) => req,
        Err(error) => {
            return fail_import_job_if_claimed(state, job.id, format!("invalid import job input: {error}")).await;
        }
    };
    let (actor_id, role, is_bot) = match form_job_worker_context(&job.input, job.created_by) {
        Ok(context) => context,
        Err(error) => return fail_import_job_if_claimed(state, job.id, error.to_string()).await,
    };
    let form = match find_form(state, job.form_id).await {
        Ok(form) => form,
        Err(error) => return fail_import_job_if_claimed(state, job.id, error.to_string()).await,
    };
    run_form_import_job(state.clone(), form, actor_id, role, is_bot, job.id, req).await?;
    Ok(true)
}

async fn process_pending_form_export_jobs_from_worker(state: &AppState, limit: i64) -> Result<usize, ApiError> {
    let jobs = FormExportJobResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, form_id, status, input, result, error,
                   created_by, started_at, completed_at, created_at, updated_at
            FROM form_export_jobs
            WHERE status = 'queued'
            ORDER BY created_at
            LIMIT $1
        ",
        vec![limit.into()],
    ))
    .all(&state.db)
    .await?;
    let mut processed = 0;
    for job in jobs {
        if process_form_export_job_from_worker(state, job).await? {
            processed += 1;
        }
    }
    Ok(processed)
}

async fn process_form_export_job_from_worker(state: &AppState, job: FormExportJobResponse) -> Result<bool, ApiError> {
    let req = match serde_json::from_value::<CreateExportJobRequest>(job.input.clone()) {
        Ok(req) => req,
        Err(error) => {
            return fail_export_job_if_claimed(state, job.id, format!("invalid export job input: {error}")).await;
        }
    };
    let query = match export_records_query_from_job(req) {
        Ok(query) => query,
        Err(error) => return fail_export_job_if_claimed(state, job.id, error.to_string()).await,
    };
    let (actor_id, role, is_bot) = match form_job_worker_context(&job.input, job.created_by) {
        Ok(context) => context,
        Err(error) => return fail_export_job_if_claimed(state, job.id, error.to_string()).await,
    };
    let form = match find_form(state, job.form_id).await {
        Ok(form) => form,
        Err(error) => return fail_export_job_if_claimed(state, job.id, error.to_string()).await,
    };
    run_form_export_job(state.clone(), form, actor_id, role, is_bot, job.id, query).await?;
    Ok(true)
}

async fn process_pending_form_attachment_package_jobs_from_worker(
    state: &AppState,
    limit: i64,
) -> Result<usize, ApiError> {
    let jobs = FormAttachmentPackageJobResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, form_id, status, input, result, error,
                   created_by, started_at, completed_at, created_at, updated_at
            FROM form_attachment_package_jobs
            WHERE status = 'queued'
            ORDER BY created_at
            LIMIT $1
        ",
        vec![limit.into()],
    ))
    .all(&state.db)
    .await?;
    let mut processed = 0;
    for job in jobs {
        if process_form_attachment_package_job_from_worker(state, job).await? {
            processed += 1;
        }
    }
    Ok(processed)
}

async fn process_form_attachment_package_job_from_worker(
    state: &AppState,
    job: FormAttachmentPackageJobResponse,
) -> Result<bool, ApiError> {
    let req = match serde_json::from_value::<CreateAttachmentPackageJobRequest>(job.input.clone()) {
        Ok(req) => req,
        Err(error) => {
            return fail_attachment_package_job_if_claimed(
                state,
                job.id,
                format!("invalid attachment package job input: {error}"),
            )
            .await;
        }
    };
    let (actor_id, role, _) = match form_job_worker_context(&job.input, job.created_by) {
        Ok(context) => context,
        Err(error) => return fail_attachment_package_job_if_claimed(state, job.id, error.to_string()).await,
    };
    let form = match find_form(state, job.form_id).await {
        Ok(form) => form,
        Err(error) => return fail_attachment_package_job_if_claimed(state, job.id, error.to_string()).await,
    };
    run_form_attachment_package_job(
        state.clone(),
        form,
        actor_id,
        role,
        job.id,
        ExportAttachmentPackageQuery { view_id: req.view_id },
    )
    .await?;
    Ok(true)
}

async fn fail_import_job_if_claimed(state: &AppState, job_id: Uuid, error: String) -> Result<bool, ApiError> {
    if !claim_import_job_running(state, job_id).await? {
        return Ok(false);
    }
    update_import_job_failed(state, job_id, error).await?;
    Ok(true)
}

async fn fail_export_job_if_claimed(state: &AppState, job_id: Uuid, error: String) -> Result<bool, ApiError> {
    if !claim_export_job_running(state, job_id).await? {
        return Ok(false);
    }
    update_export_job_failed(state, job_id, error).await?;
    Ok(true)
}

async fn fail_attachment_package_job_if_claimed(
    state: &AppState,
    job_id: Uuid,
    error: String,
) -> Result<bool, ApiError> {
    if !claim_attachment_package_job_running(state, job_id).await? {
        return Ok(false);
    }
    update_attachment_package_job_failed(state, job_id, error).await?;
    Ok(true)
}

async fn cleanup_expired_attachment_package_artifacts_from_worker(
    state: &AppState,
    limit: i64,
) -> Result<usize, ApiError> {
    let jobs = FormAttachmentPackageJobResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, form_id, status, input, result, error,
                   created_by, started_at, completed_at, created_at, updated_at
            FROM form_attachment_package_jobs
            WHERE status = 'completed'
              AND result IS NOT NULL
              AND result ? 'stored_file_name'
              AND result ? 'expires_at'
              AND NOT (result ? 'artifact_deleted_at')
              AND (result->>'expires_at')::timestamptz <= now()
            ORDER BY completed_at NULLS LAST, created_at
            LIMIT $1
        ",
        vec![limit.into()],
    ))
    .all(&state.db)
    .await?;
    let mut cleaned = 0;
    for job in jobs {
        cleanup_expired_attachment_package_artifact(state, job).await?;
        cleaned += 1;
    }
    Ok(cleaned)
}

async fn cleanup_expired_attachment_package_artifact(
    state: &AppState,
    job: FormAttachmentPackageJobResponse,
) -> Result<(), ApiError> {
    let mut result = job
        .result
        .ok_or_else(|| ApiError::BadRequest("attachment package job result is missing".to_string()))?;
    let stored_file_name = attachment_package_job_result_string(&result, "stored_file_name")?;
    let cleanup_error = ObjectStorage::from_runtime_config()?
        .delete(&attachment_package_artifact_key(&stored_file_name))
        .await
        .err()
        .map(|error| error.to_string());
    let object = result
        .as_object_mut()
        .ok_or_else(|| ApiError::BadRequest("attachment package job result must be an object".to_string()))?;
    object.insert("artifact_deleted_at".to_string(), json!(Utc::now()));
    object.insert("artifact_deleted_by".to_string(), json!("worker"));
    if let Some(error) = cleanup_error {
        object.insert("artifact_cleanup_error".to_string(), json!(error));
    } else {
        object.remove("artifact_cleanup_error");
    }
    update_attachment_package_job_result(state, job.id, result).await
}

async fn cleanup_expired_signature_values_from_worker(state: &AppState, limit: i64) -> Result<usize, ApiError> {
    let candidates = SignatureRetentionCleanupCandidate::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
                forms.id AS form_id,
                forms.workspace_id,
                forms.project_id,
                forms.key AS form_key,
                forms.name AS form_name,
                forms.description AS form_description,
                forms.icon AS form_icon,
                forms.color AS form_color,
                forms.title_template,
                forms.schema,
                forms.detail_layout,
                forms.schema_version,
                forms.created_by AS form_created_by,
                forms.archived_at AS form_archived_at,
                forms.created_at AS form_created_at,
                forms.updated_at AS form_updated_at,
                records.id AS record_id,
                records.values AS record_values,
                records.source AS record_source,
                records.updated_at AS record_updated_at
            FROM form_records records
            JOIN project_forms forms ON forms.id = records.form_id
            WHERE records.archived_at IS NULL
              AND forms.archived_at IS NULL
              AND forms.schema::text LIKE '%"signature"%'
              AND forms.schema::text LIKE '%"retention_days"%'
              AND (
                    records.values::text LIKE '%/uploads/signatures/%'
                 OR records.source::text LIKE '%"previous_object_key"%'
              )
            ORDER BY records.updated_at
            LIMIT $1
        "#,
        vec![limit.into()],
    ))
    .all(&state.db)
    .await?;

    let mut cleaned = 0;
    for candidate in candidates {
        cleaned += cleanup_expired_signature_record(state, candidate).await?;
    }
    Ok(cleaned)
}

async fn cleanup_expired_signature_record(
    state: &AppState,
    candidate: SignatureRetentionCleanupCandidate,
) -> Result<usize, ApiError> {
    let form = FormResponse {
        id: candidate.form_id,
        workspace_id: candidate.workspace_id,
        project_id: candidate.project_id,
        key: candidate.form_key,
        name: candidate.form_name,
        description: candidate.form_description,
        icon: candidate.form_icon,
        color: candidate.form_color,
        title_template: candidate.title_template,
        schema: candidate.schema,
        detail_layout: candidate.detail_layout,
        schema_version: candidate.schema_version,
        created_by: candidate.form_created_by,
        archived_at: candidate.form_archived_at,
        created_at: candidate.form_created_at,
        updated_at: candidate.form_updated_at,
    };
    let now = Utc::now();
    let cleanup =
        cleanup_expired_signature_values(&form.schema, candidate.record_values, candidate.record_updated_at, now)
            .await?;
    let source = append_signature_audit_source(candidate.record_source, cleanup.audit_entries)?;
    let audit_cleanup =
        cleanup_expired_replaced_signature_audit_entries(&form.schema, &cleanup.values, source, now).await?;
    let source = append_signature_audit_source(audit_cleanup.source, audit_cleanup.audit_entries)?;
    let deleted_count = cleanup.deleted_count + audit_cleanup.deleted_count;
    if deleted_count == 0 {
        return Ok(0);
    }

    let title = render_title(&form.title_template, candidate.record_id, &cleanup.values);
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE form_records
            SET title = $1, values = $2, source = $3, schema_version = $4,
                updated_by = NULL, updated_at = now()
            WHERE id = $5
        ",
        vec![
            title.into(),
            cleanup.values.clone().into(),
            source.clone().into(),
            form.schema_version.into(),
            candidate.record_id.into(),
        ],
    ))
    .await?;
    refresh_record_projection(
        &tx,
        form.project_id,
        form.id,
        candidate.record_id,
        &form.schema,
        &cleanup.values,
    )
    .await?;
    insert_form_event(
        &tx,
        &form,
        Some(candidate.record_id),
        "form.record.signature.retention_deleted",
        None,
        source,
        json!({
            "record_id": candidate.record_id,
            "values": cleanup.values,
            "deleted_count": deleted_count,
            "value_deleted_count": cleanup.deleted_count,
            "replacement_deleted_count": audit_cleanup.deleted_count
        }),
    )
    .await?;
    tx.commit().await?;

    Ok(deleted_count)
}

async fn import_form_record_rows(
    state: &AppState,
    form: &FormResponse,
    actor_id: Uuid,
    role: &str,
    is_bot: bool,
    req: ImportRecordsRequest,
) -> Result<ImportRecordsResponse, ApiError> {
    let previews = preview_import_rows(state, form, actor_id, role, is_bot, &req.rows).await?;
    let invalid_rows = previews.iter().filter(|row| !row.valid).count();
    if invalid_rows > 0 {
        return Ok(ImportRecordsResponse {
            form_id: form.id,
            schema_version: form.schema_version,
            total_rows: previews.len(),
            created_count: 0,
            invalid_rows,
            rows: previews,
            records: Vec::new(),
        });
    }

    let mut created = Vec::new();
    let batch_idempotency_key = normalize_optional_idempotency_key(req.idempotency_key)?;
    for (index, row) in req.rows.into_iter().enumerate() {
        let row_number = row.row_number.unwrap_or(index + 1);
        let source = row.source.unwrap_or_else(|| {
            json!({
                "type": if is_bot { "bot" } else { "user" },
                "actor_id": actor_id,
                "origin": "import",
                "row_number": row_number
            })
        });
        let row_idempotency_key = normalize_optional_idempotency_key(row.idempotency_key)?.or_else(|| {
            batch_idempotency_key
                .as_ref()
                .map(|key| format!("{key}:row:{row_number}"))
        });
        let record = create_record_for_form(
            state,
            form,
            actor_id,
            role,
            is_bot,
            CreateRecordRequest {
                values: row.values,
                title: row.title,
                source: Some(source),
                idempotency_key: row_idempotency_key,
            },
        )
        .await?;
        let denied_read_fields = denied_read_field_keys(state, form.id, role).await?;
        created.push(filter_record_response_values(record, &denied_read_fields));
    }

    Ok(ImportRecordsResponse {
        form_id: form.id,
        schema_version: form.schema_version,
        total_rows: previews.len(),
        created_count: created.len(),
        invalid_rows: 0,
        rows: previews,
        records: created,
    })
}

pub async fn get_form_schema_summary(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.design").await?;
    Ok(ApiResponse::success(
        form_schema_summary(&state, form_id, form.schema_version, &form.schema).await?,
    ))
}

pub async fn get_form_field_usage(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.design").await?;
    let input = schema_insight_input(&form);
    Ok(ApiResponse::success(form_field_usage(&state, &input).await?))
}

pub async fn get_form_field_dependencies(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.design").await?;
    let input = schema_insight_input(&form);
    Ok(ApiResponse::success(form_field_dependencies(&state, &input).await?))
}

pub async fn list_form_schema_versions(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.design").await?;
    let items = FormSchemaVersionResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, form_id, version, schema, detail_layout, changed_by, change_summary, created_at
            FROM form_schema_versions
            WHERE form_id = $1
            ORDER BY version DESC
        ",
        vec![form_id.into()],
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(items))
}

pub async fn get_form_schema_version(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((form_id, version)): Path<(Uuid, i32)>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.design").await?;
    let item = FormSchemaVersionResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, form_id, version, schema, detail_layout, changed_by, change_summary, created_at
            FROM form_schema_versions
            WHERE form_id = $1 AND version = $2
        ",
        vec![form_id.into(), version.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("form schema version not found".to_string()))?;

    Ok(ApiResponse::success(item))
}

pub async fn get_form_permissions(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (_, role, is_bot) =
        require_workspace_access_from_auth(&state, &claims, bot.as_ref().map(|b| &b.0), form.workspace_id).await?;
    let policies = list_form_permission_policies(&state, form.id).await?;
    Ok(ApiResponse::success(form_permissions_response(
        form.id, role, is_bot, policies,
    )))
}

pub async fn update_form_permissions(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Json(req): Json<UpdateFormPermissionsRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, is_bot) =
        require_workspace_access_from_auth(&state, &claims, bot.as_ref().map(|b| &b.0), form.workspace_id).await?;
    ensure_form_permission_manager(&role)?;
    if req.policies.len() > 20 {
        return Err(ApiError::BadRequest(
            "permissions policy update cannot exceed 20 rows".to_string(),
        ));
    }

    let tx = state.db.begin().await?;
    let mut updated_subjects = Vec::new();
    for policy in req.policies {
        let subject_type = normalize_permission_subject_type(&policy.subject_type)?;
        let subject_id = normalize_permission_subject_id(&policy.subject_id)?;
        let policy = validate_permission_policy(policy.policy)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO form_permissions (
                    workspace_id, project_id, form_id, subject_type, subject_id, policy, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, now(), now())
                ON CONFLICT (form_id, subject_type, subject_id)
                DO UPDATE SET policy = EXCLUDED.policy, updated_at = now()
            ",
            vec![
                form.workspace_id.into(),
                form.project_id.into(),
                form.id.into(),
                subject_type.clone().into(),
                subject_id.clone().into(),
                policy.into(),
            ],
        ))
        .await?;
        updated_subjects.push(format!("{subject_type}:{subject_id}"));
    }
    insert_form_event(
        &tx,
        &form,
        None,
        "form.permissions.updated",
        if is_bot { None } else { Some(actor_id) },
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({ "form_id": form.id, "updated_subjects": updated_subjects }),
    )
    .await?;
    tx.commit().await?;

    let policies = list_form_permission_policies(&state, form.id).await?;
    Ok(ApiResponse::success(form_permissions_response(
        form.id, role, is_bot, policies,
    )))
}

/// Object keys an attachment registration would bind to `workspace_id`.
///
/// `storage_key`, `url` and `thumbnail_url` arrive straight from the client and are exactly the
/// columns the upload download check reads back to decide which workspace owns an object. Without
/// this check a member could register an attachment row pointing at another tenant's object and
/// make the download check resolve to their own workspace ("claiming" someone else's file).
fn attachment_claimed_object_keys(storage_key: &str, url: &str, thumbnail_url: Option<&str>) -> BTreeSet<String> {
    let mut claimed = BTreeSet::new();
    claimed.insert(storage_key.to_string());
    if let Some(object_key) = upload_object_key_from_path(url) {
        claimed.insert(object_key);
    }
    if let Some(object_key) = thumbnail_url.and_then(upload_object_key_from_path) {
        claimed.insert(object_key);
    }
    claimed
}

/// Reject an attachment registration that points at an object another workspace already owns.
async fn ensure_attachment_objects_claimable(
    state: &AppState,
    workspace_id: Uuid,
    storage_key: &str,
    url: &str,
    thumbnail_url: Option<&str>,
) -> Result<(), ApiError> {
    for object_key in attachment_claimed_object_keys(storage_key, url, thumbnail_url) {
        match resolve_upload_object_owner(state, &object_key).await? {
            UploadObjectOwner::Unowned => {}
            UploadObjectOwner::Owned(owner_workspace_id) if owner_workspace_id == workspace_id => {}
            UploadObjectOwner::Owned(_) | UploadObjectOwner::Ambiguous => {
                return Err(ApiError::Forbidden(
                    "attachment references an upload object owned by another workspace".to_string(),
                ));
            }
        }
    }
    Ok(())
}

/// Upload objects a record write would newly bind to the form's workspace.
///
/// Attachment registration is not the only way to claim an upload object: a materialized signature
/// is owned by whichever record stores its URL, so writing somebody else's signature URL into a
/// record of your own makes the ownership lookup see two workspaces. That resolves to `Ambiguous`,
/// which fails closed for everyone — the real owner is permanently locked out of what is, in a
/// compliance form, the evidence itself. Screening record writes the same way attachment
/// registrations are screened removes the only way to reach that state.
///
/// Only values that differ from what is already stored are collected. Re-saving a record keeps
/// working, the same object may be referenced by as many rows of one workspace as it likes, and a
/// record whose object already went ambiguous is still editable by its owner.
fn record_claimed_object_keys(values: &Value, existing_values: Option<&Value>) -> BTreeSet<String> {
    let mut claimed = BTreeSet::new();
    let Some(object) = values.as_object() else {
        return claimed;
    };
    for (field_key, value) in object {
        let Some(value) = value.as_str() else {
            continue;
        };
        if existing_values
            .and_then(|existing| existing.get(field_key))
            .and_then(Value::as_str)
            == Some(value)
        {
            continue;
        }
        if let Some(object_key) = signature_object_key_claimed_by_record_value(field_key, value) {
            claimed.insert(object_key);
        }
    }
    claimed
}

/// Reject a record write that points at an upload object another workspace already owns.
async fn ensure_record_objects_claimable(
    state: &AppState,
    workspace_id: Uuid,
    values: &Value,
    existing_values: Option<&Value>,
) -> Result<(), ApiError> {
    for object_key in record_claimed_object_keys(values, existing_values) {
        match resolve_upload_object_owner(state, &object_key).await? {
            UploadObjectOwner::Unowned => {}
            UploadObjectOwner::Owned(owner_workspace_id) if owner_workspace_id == workspace_id => {}
            UploadObjectOwner::Owned(_) | UploadObjectOwner::Ambiguous => {
                return Err(ApiError::Forbidden(
                    "record references an upload object owned by another workspace".to_string(),
                ));
            }
        }
    }
    Ok(())
}

pub async fn create_form_attachment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Json(req): Json<CreateAttachmentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, _role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.update").await?;
    let field_key = normalize_key(&req.field_key).map_err(ApiError::BadRequest)?;
    let field = parse_fields(&form.schema)
        .map_err(ApiError::BadRequest)?
        .into_iter()
        .find(|field| field.key == field_key)
        .ok_or_else(|| ApiError::BadRequest("field_key does not exist in form schema".to_string()))?;
    if !matches!(field.field_type.as_str(), "attachment" | "image") {
        return Err(ApiError::BadRequest(
            "field_key must reference an attachment or image field".to_string(),
        ));
    }
    if let Some(record_id) = req.record_id {
        let record = find_record(&state, record_id).await?;
        if record.form_id != form.id {
            return Err(ApiError::BadRequest("record_id does not belong to form".to_string()));
        }
    }
    let file_name = required_trimmed(&req.file_name, "file_name")?;
    let storage_key = required_trimmed(&req.storage_key, "storage_key")?;
    let content_type = req.content_type.as_deref().map(str::trim).unwrap_or("").to_string();
    let url = req.url.as_deref().map(str::trim).unwrap_or("").to_string();
    let byte_size = req.byte_size.unwrap_or_default();
    if byte_size < 0 {
        return Err(ApiError::BadRequest("byte_size must be non-negative".to_string()));
    }
    validate_attachment_create_policy(&form.schema, &field.key, &file_name, &content_type, byte_size, &url)?;
    ensure_attachment_objects_claimable(
        &state,
        form.workspace_id,
        &storage_key,
        &url,
        req.thumbnail_url.as_deref(),
    )
    .await?;
    let media = ensure_attachment_media(
        &form.schema,
        &field.key,
        &file_name,
        &content_type,
        &url,
        req.thumbnail_url,
    )
    .await?;
    let attachment_id = Uuid::new_v4();
    let created_by = if is_bot { None } else { Some(actor_id) };
    let tx = state.db.begin().await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO form_attachments (
                id, workspace_id, project_id, form_id, record_id, field_id, field_key,
                file_name, content_type, byte_size, storage_key, url, thumbnail_url, media_metadata, created_by,
                created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, now(), now())
        ",
        vec![
            attachment_id.into(),
            form.workspace_id.into(),
            form.project_id.into(),
            form.id.into(),
            req.record_id.into(),
            field.field_id.into(),
            field.key.into(),
            file_name.into(),
            content_type.into(),
            byte_size.into(),
            storage_key.into(),
            url.into(),
            media.thumbnail_url.into(),
            media.media_metadata.into(),
            created_by.into(),
        ],
    ))
    .await?;
    insert_form_event(
        &tx,
        &form,
        req.record_id,
        "form.attachment.created",
        created_by,
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({ "attachment_id": attachment_id, "field_key": field_key }),
    )
    .await?;
    tx.commit().await?;

    let attachment = find_attachment(&state, attachment_id).await?;
    Ok(ApiResponse::success(attachment))
}

pub async fn list_form_attachments(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Query(query): Query<ListAttachmentsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.view").await?;
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(50).clamp(1, 200);
    let offset = (page - 1) * per_page;
    let mut where_parts = vec!["form_id = $1".to_string()];
    let mut values = vec![form_id.into()];
    let mut idx = 2;
    if !query.include_archived.unwrap_or(false) {
        where_parts.push("archived_at IS NULL".to_string());
    }
    if let Some(record_id) = query.record_id {
        where_parts.push(format!("record_id = ${idx}"));
        values.push(record_id.into());
        idx += 1;
    }
    if let Some(field_key) = query.field_key {
        let field_key = normalize_key(&field_key).map_err(ApiError::BadRequest)?;
        where_parts.push(format!("field_key = ${idx}"));
        values.push(field_key.into());
        idx += 1;
    }
    let where_sql = where_parts.join(" AND ");
    let total = count_query(
        &state,
        &format!("SELECT COUNT(*)::bigint AS count FROM form_attachments WHERE {where_sql}"),
        values.clone(),
    )
    .await?;
    values.push(per_page.into());
    values.push(offset.into());
    let items = FormAttachmentResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &format!(
            r"
                SELECT id, workspace_id, project_id, form_id, record_id, field_id, field_key,
                       file_name, content_type, byte_size, storage_key, url, thumbnail_url,
                       COALESCE(media_metadata, '{{}}'::jsonb) AS media_metadata, created_by,
                       archived_at, created_at, updated_at
                FROM form_attachments
                WHERE {where_sql}
                ORDER BY created_at DESC
                LIMIT ${idx} OFFSET ${}
            ",
            idx + 1
        ),
        values,
    ))
    .all(&state.db)
    .await?;
    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages: total_pages(total, per_page),
    }))
}

pub async fn create_form_attachment_signed_url(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(attachment_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let attachment = find_attachment(&state, attachment_id).await?;
    if attachment.archived_at.is_some() {
        return Err(ApiError::NotFound("form attachment not found".to_string()));
    }
    let form = find_form(&state, attachment.form_id).await?;
    require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.view").await?;
    let ttl_minutes = attachment_signed_url_ttl_minutes(&form.schema, &attachment.field_key);
    let expires_at = Utc::now() + Duration::minutes(ttl_minutes);
    let expires = expires_at.timestamp();
    let signature = sign_attachment_download(state.cfg.jwt_secret.expose(), attachment_id, expires)?;
    Ok(ApiResponse::success(AttachmentSignedDownloadResponse {
        url: format!("/api/v1/form-attachments/{attachment_id}/download?expires={expires}&signature={signature}"),
        expires_at,
    }))
}

pub async fn download_signed_form_attachment(
    State(state): State<AppState>,
    Path(attachment_id): Path<Uuid>,
    Query(query): Query<AttachmentDownloadQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let attachment = find_attachment(&state, attachment_id).await?;
    if attachment.archived_at.is_some() {
        return Err(ApiError::NotFound("form attachment not found".to_string()));
    }
    if query.expires < Utc::now().timestamp() {
        return Err(ApiError::Unauthorized("attachment download link expired".to_string()));
    }
    let expected = sign_attachment_download(state.cfg.jwt_secret.expose(), attachment_id, query.expires)?;
    if !constant_time_eq(expected.as_bytes(), query.signature.as_bytes()) {
        return Err(ApiError::Unauthorized(
            "invalid attachment download signature".to_string(),
        ));
    }
    if attachment.url.trim().is_empty() {
        return Err(ApiError::NotFound("attachment URL not found".to_string()));
    }
    Ok(Redirect::temporary(&attachment.url))
}

pub async fn archive_form_attachment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(attachment_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let attachment = find_attachment(&state, attachment_id).await?;
    let form = find_form(&state, attachment.form_id).await?;
    let (actor_id, _role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.update").await?;
    let actor_id = if is_bot { None } else { Some(actor_id) };
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE form_attachments SET archived_at = COALESCE(archived_at, now()), updated_at = now() WHERE id = $1",
        vec![attachment_id.into()],
    ))
    .await?;
    insert_form_event(
        &tx,
        &form,
        attachment.record_id,
        "form.attachment.archived",
        actor_id,
        json!({ "type": if is_bot { "bot" } else { "user" } }),
        json!({ "attachment_id": attachment_id, "field_key": attachment.field_key }),
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::ok())
}

pub async fn restore_form_attachment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(attachment_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let attachment = find_attachment(&state, attachment_id).await?;
    let form = find_form(&state, attachment.form_id).await?;
    let (actor_id, _role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.update").await?;
    let actor_id = if is_bot { None } else { Some(actor_id) };
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE form_attachments SET archived_at = NULL, updated_at = now() WHERE id = $1",
        vec![attachment_id.into()],
    ))
    .await?;
    insert_form_event(
        &tx,
        &form,
        attachment.record_id,
        "form.attachment.restored",
        actor_id,
        json!({ "type": if is_bot { "bot" } else { "user" } }),
        json!({ "attachment_id": attachment_id, "field_key": attachment.field_key }),
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(find_attachment(&state, attachment_id).await?))
}

pub async fn aggregate_form_records(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Query(query): Query<AggregateQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, _) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.view").await?;
    let field_key = normalize_key(&query.field_key).map_err(ApiError::BadRequest)?;
    ensure_field_read_policy_allows(&state, form.id, &role, &field_key).await?;
    let aggregate = normalize_aggregate(query.aggregate.as_deref().unwrap_or("sum"))?;
    let field = parse_fields(&form.schema)
        .map_err(ApiError::BadRequest)?
        .into_iter()
        .find(|field| field.key == field_key)
        .ok_or_else(|| ApiError::BadRequest("field_key does not exist in form schema".to_string()))?;
    if !matches!(field.field_type.as_str(), "amount" | "number" | "integer") {
        return Err(ApiError::BadRequest(
            "aggregate field must be amount, number, or integer".to_string(),
        ));
    }

    let aggregate_expr = match aggregate.as_str() {
        "sum" => "SUM(value_decimal)::text",
        "avg" => "AVG(value_decimal)::text",
        "min" => "MIN(value_decimal)::text",
        "max" => "MAX(value_decimal)::text",
        "count" => "COUNT(value_decimal)::text",
        _ => return Err(ApiError::BadRequest("unsupported aggregate".to_string())),
    };
    let amount = field.amount.as_ref();
    let mut values: Vec<sea_orm::Value> = vec![
        form_id.into(),
        field_key.into(),
        field.field_type.into(),
        aggregate.into(),
        amount.map(|config| config.currency.clone()).into(),
        amount
            .map(|config| i32::try_from(config.scale).unwrap_or(i32::MAX))
            .into(),
    ];
    // Aggregates must honour the same row-level scope as the record list, otherwise `owned` scope
    // leaks other members' values through sums and counts.
    let scope_sql = if form_record_scope(&state, form.id, &role).await? == "owned" {
        values.push(actor_id.into());
        format!("\n              AND form_records.created_by = ${}", values.len())
    } else {
        String::new()
    };
    let sql = format!(
        r"
            SELECT
                $1::uuid AS form_id,
                $2::text AS field_key,
                $3::text AS field_type,
                $4::text AS aggregate,
                {aggregate_expr} AS decimal,
                COUNT(value_decimal)::bigint AS count,
                $5::text AS currency,
                $6::integer AS scale
            FROM form_record_field_index
            JOIN form_records ON form_records.id = form_record_field_index.record_id
            WHERE form_record_field_index.form_id = $1
              AND form_record_field_index.field_key = $2
              AND form_record_field_index.value_decimal IS NOT NULL
              AND form_records.archived_at IS NULL{scope_sql}
        "
    );
    let row =
        FormAggregateResponse::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, &sql, values))
            .one(&state.db)
            .await?
            .ok_or_else(|| ApiError::Internal)?;

    Ok(ApiResponse::success(row))
}

pub async fn create_form_record(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Json(req): Json<CreateRecordRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.create").await?;
    let record = create_record_for_form(&state, &form, actor_id, &role, is_bot, req).await?;
    let denied_read_fields = denied_read_field_keys(&state, form.id, &role).await?;
    Ok(ApiResponse::success(filter_record_response_values(
        record,
        &denied_read_fields,
    )))
}

async fn create_record_for_form(
    state: &AppState,
    form: &FormResponse,
    actor_id: Uuid,
    role: &str,
    is_bot: bool,
    req: CreateRecordRequest,
) -> Result<RecordResponse, ApiError> {
    let idempotency_key = normalize_optional_idempotency_key(req.idempotency_key)?;
    if let Some(record) = find_idempotent_record(state, form, idempotency_key.as_deref(), "form.record.created").await?
    {
        return Ok(record);
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
    let record_id = Uuid::new_v4();
    let created_by = if is_bot { None } else { Some(actor_id) };
    let source = ensure_json_object(
        req.source
            .unwrap_or_else(|| json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id })),
        "source",
    )?;
    let tx = state.db.begin().await?;
    let calculated = calculate_values(state, form, None, values_with_formula).await?;
    let with_autonumber = apply_autonumber_values(&tx, form, None, calculated).await?;
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
        r"
            INSERT INTO form_records (
                id, workspace_id, project_id, form_id, title, values, source,
                schema_version, created_by, updated_by, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9, now(), now())
        ",
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
    refresh_record_projection(&tx, form.project_id, form.id, record_id, &form.schema, &normalized).await?;
    let event_payload = json!({ "record_id": record_id, "values": normalized });
    insert_form_event_with_idempotency(
        &tx,
        &form,
        Some(record_id),
        "form.record.created",
        created_by,
        source,
        event_payload.clone(),
        idempotency_key,
    )
    .await?;
    tx.commit().await?;
    run_event_handler_hooks(
        state,
        form.workspace_id,
        form.project_id,
        form.id,
        &form.key,
        Some(record_id),
        "form.record.created",
        event_payload,
    )
    .await?;

    recalculate_parent_records_for_child(state, record_id, created_by).await?;

    find_record(state, record_id).await
}

pub async fn preview_form_record_recalculation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Json(req): Json<RecalculatePreviewRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (_, role, _) = require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.create").await?;
    ensure_field_write_policy_allows(&state, form.id, &role, &req.values).await?;
    let calculated = calculate_values(&state, &form, None, req.values).await?;
    let normalized = validate_and_normalize_values(&form.schema, calculated.clone()).map_err(ApiError::BadRequest)?;
    let denied_read_fields = denied_read_field_keys(&state, form.id, &role).await?;
    Ok(ApiResponse::success(RecalculatePreviewResponse {
        form_id,
        schema_version: form.schema_version,
        values: filter_values_for_read_policy(calculated, &denied_read_fields),
        normalized_values: filter_values_for_read_policy(normalized, &denied_read_fields),
    }))
}

pub async fn get_form_record(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(record_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let record = find_record(&state, record_id).await?;
    let form = find_form(&state, record.form_id).await?;
    let (actor_id, role, _) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.view").await?;
    ensure_record_scope_allows_created_by(&state, form.id, &role, actor_id, record.created_by).await?;
    let denied_read_fields = denied_read_field_keys(&state, form.id, &role).await?;
    Ok(ApiResponse::success(filter_record_response_values(
        record,
        &denied_read_fields,
    )))
}

pub async fn verify_form_record_signature_audit(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(record_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let record = find_record(&state, record_id).await?;
    let form = find_form(&state, record.form_id).await?;
    let (actor_id, role, _) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.view").await?;
    ensure_record_scope_allows_created_by(&state, form.id, &role, actor_id, record.created_by).await?;
    let verification = verify_signature_audit_source(&record.source).await?;
    let lifecycle = signature_lifecycle_summary(&form.schema, &record.values, &record.source)?;
    let workflow = signature_workflow_verification_summary(&verification, &lifecycle);
    Ok(ApiResponse::success(json!({
        "form_id": form.id,
        "record_id": record.id,
        "signature_audit": verification.result,
        "signature_lifecycle": lifecycle,
        "signature_workflow_verification": workflow
    })))
}

pub async fn recalculate_form_record(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(record_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let record = find_record(&state, record_id).await?;
    let form = find_form(&state, record.form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.update").await?;
    ensure_record_scope_allows_created_by(&state, form.id, &role, actor_id, record.created_by).await?;
    let updated_by = if is_bot { None } else { Some(actor_id) };
    let source =
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id, "operation": "recalculate" });
    let tx = state.db.begin().await?;
    let calculated = calculate_values(&state, &form, Some(record_id), record.values.clone()).await?;
    let with_autonumber = apply_autonumber_values(&tx, &form, Some(&record.values), calculated).await?;
    let values = validate_and_normalize_values_with_existing(&form.schema, with_autonumber, &record.values)
        .map_err(ApiError::BadRequest)?;
    let title = render_title(&form.title_template, record_id, &values);

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE form_records
            SET title = $1, values = $2, source = $3, schema_version = $4, updated_by = $5, updated_at = now()
            WHERE id = $6
        ",
        vec![
            title.into(),
            values.clone().into(),
            source.clone().into(),
            form.schema_version.into(),
            updated_by.into(),
            record_id.into(),
        ],
    ))
    .await?;
    refresh_record_projection(&tx, form.project_id, form.id, record_id, &form.schema, &values).await?;
    let event_payload = json!({ "record_id": record_id, "values": values });
    insert_form_event(
        &tx,
        &form,
        Some(record_id),
        "form.record.recalculated",
        updated_by,
        source,
        event_payload.clone(),
    )
    .await?;
    tx.commit().await?;
    run_event_handler_hooks(
        &state,
        form.workspace_id,
        form.project_id,
        form.id,
        &form.key,
        Some(record_id),
        "form.record.recalculated",
        event_payload,
    )
    .await?;
    recalculate_parent_records_for_child(&state, record_id, updated_by).await?;

    let record = find_record(&state, record_id).await?;
    let denied_read_fields = denied_read_field_keys(&state, form.id, &role).await?;
    Ok(ApiResponse::success(filter_record_response_values(
        record,
        &denied_read_fields,
    )))
}

pub async fn list_form_events(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Query(query): Query<ListEventsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    let (actor_id, role, _) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.view").await?;
    let owned_by = if form_record_scope(&state, form.id, &role).await? == "owned" {
        Some(actor_id)
    } else {
        None
    };
    let denied_read_fields = denied_read_field_keys(&state, form.id, &role).await?;
    list_business_events(
        &state,
        EventScope::Form {
            project_id: form.project_id,
            form_id,
            owned_by,
        },
        query,
        &denied_read_fields,
    )
    .await
}

pub async fn list_form_record_events(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(record_id): Path<Uuid>,
    Query(query): Query<ListEventsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let record = find_record(&state, record_id).await?;
    let form = find_form(&state, record.form_id).await?;
    let (actor_id, role, _) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.view").await?;
    ensure_record_scope_allows_created_by(&state, form.id, &role, actor_id, record.created_by).await?;
    let denied_read_fields = denied_read_field_keys(&state, form.id, &role).await?;
    list_business_events(
        &state,
        EventScope::Record {
            project_id: record.project_id,
            record_id,
        },
        query,
        &denied_read_fields,
    )
    .await
}

pub async fn list_form_record_comments(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(record_id): Path<Uuid>,
    Query(query): Query<ListRecordCommentsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let record = find_record(&state, record_id).await?;
    let form = find_form(&state, record.form_id).await?;
    let (actor_id, role, _) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.view").await?;
    ensure_record_scope_allows_created_by(&state, form.id, &role, actor_id, record.created_by).await?;
    let page = record_comment_page(&query);
    let total = count_record_comments(&state, record_id).await?;
    let items = list_record_comments(&state, record_id, &page).await?;

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page: page.page,
        per_page: page.per_page,
        total_pages: total_pages(total, page.per_page),
    }))
}

pub async fn create_form_record_comment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(record_id): Path<Uuid>,
    Json(req): Json<CreateRecordCommentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let record = find_record(&state, record_id).await?;
    let form = find_form(&state, record.form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.update").await?;
    ensure_record_scope_allows_created_by(&state, form.id, &role, actor_id, record.created_by).await?;
    let body = required_record_comment_body(&req)?;
    let metadata = ensure_json_object(req.metadata.unwrap_or_else(|| json!({})), "metadata")?;
    let comment_id = Uuid::new_v4();
    let author_id = if is_bot { None } else { Some(actor_id) };
    let tx = state.db.begin().await?;

    insert_record_comment(
        &tx,
        InsertRecordCommentInput {
            comment_id,
            workspace_id: form.workspace_id,
            project_id: form.project_id,
            form_id: form.id,
            record_id,
            author_id,
            body: body.clone(),
            metadata: metadata.clone(),
        },
    )
    .await?;
    insert_form_event(
        &tx,
        &form,
        Some(record_id),
        "form.record.comment.created",
        author_id,
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({ "record_id": record_id, "comment_id": comment_id, "body": body }),
    )
    .await?;
    tx.commit().await?;

    let comment = find_record_comment(&state, comment_id).await?;
    Ok(ApiResponse::success(comment))
}

pub async fn update_form_record(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(record_id): Path<Uuid>,
    Json(req): Json<UpdateRecordRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let record = find_record(&state, record_id).await?;
    let form = find_form(&state, record.form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.update").await?;
    ensure_record_scope_allows_created_by(&state, form.id, &role, actor_id, record.created_by).await?;
    let record = update_record_for_form(&state, &form, record, actor_id, &role, is_bot, req).await?;
    let denied_read_fields = denied_read_field_keys(&state, form.id, &role).await?;
    Ok(ApiResponse::success(filter_record_response_values(
        record,
        &denied_read_fields,
    )))
}

async fn update_record_for_form(
    state: &AppState,
    form: &FormResponse,
    record: RecordResponse,
    actor_id: Uuid,
    role: &str,
    is_bot: bool,
    req: UpdateRecordRequest,
) -> Result<RecordResponse, ApiError> {
    let idempotency_key = normalize_optional_idempotency_key(req.idempotency_key)?;
    if let Some(record) = find_idempotent_record(state, form, idempotency_key.as_deref(), "form.record.updated").await?
    {
        return Ok(record);
    }
    let record_id = record.id;
    let updated_by = if is_bot { None } else { Some(actor_id) };
    let source = ensure_json_object(
        req.source
            .unwrap_or_else(|| json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id })),
        "source",
    )?;
    let previous_values = record.values.clone();
    let tx = state.db.begin().await?;
    let mut signature_audit_entries = Vec::new();
    let values = match req.values {
        Some(values) => {
            ensure_field_write_policy_allows(state, form.id, role, &values).await?;
            let values_with_formula =
                run_formula_hooks(&state, form.workspace_id, form.project_id, form.id, &form.key, values).await?;
            let calculated = calculate_values_with_existing(
                &state,
                &form,
                Some(record_id),
                Some(&record.values),
                values_with_formula,
            )
            .await?;
            let with_autonumber = apply_autonumber_values(&tx, &form, Some(&record.values), calculated).await?;
            let normalized =
                validate_and_normalize_values_with_existing_report(&form.schema, with_autonumber, &record.values)
                    .map_err(ApiError::BadRequest)?;
            if !normalized.unmet_required_fields.is_empty() {
                // Required fields that were already empty before this write. The update is allowed
                // through so the record stays repairable, but the gap is recorded rather than
                // silently accepted as a satisfied constraint.
                tracing::warn!(
                    form_id = %form.id,
                    record_id = %record_id,
                    fields = %normalized.unmet_required_fields.join(","),
                    "record updated while required fields stay unmet from before this write"
                );
            }
            let signature_materialization =
                materialize_signature_values_with_existing_audit(&form.schema, normalized.values, &record.values)
                    .await?;
            signature_audit_entries = annotate_signature_audit_entries(
                signature_materialization.audit_entries,
                if is_bot { "bot" } else { "user" },
                actor_id,
                "record.update",
                form.id,
                record_id,
                form.schema_version,
            );
            signature_materialization.values
        }
        None => record.values,
    };
    ensure_record_objects_claimable(state, form.workspace_id, &values, Some(&previous_values)).await?;
    let source = append_signature_audit_source(source, signature_audit_entries)?;
    run_field_validator_hooks(
        &state,
        form.workspace_id,
        form.project_id,
        form.id,
        &form.key,
        &form.schema,
        &values,
    )
    .await?;
    let title = req
        .title
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| render_title(&form.title_template, record_id, &values));

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE form_records
            SET title = $1, values = $2, source = $3, schema_version = $4, updated_by = $5, updated_at = now()
            WHERE id = $6
        ",
        vec![
            title.into(),
            values.clone().into(),
            source.clone().into(),
            form.schema_version.into(),
            updated_by.into(),
            record_id.into(),
        ],
    ))
    .await?;
    refresh_record_projection(&tx, form.project_id, form.id, record_id, &form.schema, &values).await?;
    let event_payload = json!({ "record_id": record_id, "values": values });
    insert_form_event_with_idempotency(
        &tx,
        &form,
        Some(record_id),
        "form.record.updated",
        updated_by,
        source,
        event_payload.clone(),
        idempotency_key,
    )
    .await?;
    let table_change_payload = order_table_change_payload(&form.key, record_id, &previous_values, &values);
    if let Some(payload) = table_change_payload {
        insert_form_event(
            &tx,
            &form,
            Some(record_id),
            "order.table_changed",
            updated_by,
            json!({ "type": "system", "origin_event": "form.record.updated" }),
            payload,
        )
        .await?;
    }
    tx.commit().await?;
    run_event_handler_hooks(
        &state,
        form.workspace_id,
        form.project_id,
        form.id,
        &form.key,
        Some(record_id),
        "form.record.updated",
        event_payload,
    )
    .await?;
    recalculate_parent_records_for_child(&state, record_id, updated_by).await?;

    find_record(state, record_id).await
}

fn order_table_change_payload(
    form_key: &str,
    record_id: Uuid,
    previous_values: &Value,
    values: &Value,
) -> Option<Value> {
    if form_key != "order" {
        return None;
    }
    let previous_table = previous_values.get("table_id")?;
    let next_table = values.get("table_id")?;
    if previous_table == next_table {
        return None;
    }
    Some(json!({
        "record_id": record_id,
        "previous_table_id": previous_table,
        "table_id": next_table,
        "values": values
    }))
}

pub async fn delete_form_record(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(record_id): Path<Uuid>,
    Query(query): Query<IdempotencyQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let record = find_record(&state, record_id).await?;
    let form = find_form(&state, record.form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.delete").await?;
    ensure_record_scope_allows_created_by(&state, form.id, &role, actor_id, record.created_by).await?;
    archive_record_for_form(&state, &form, record, actor_id, is_bot, query.idempotency_key).await?;
    Ok(ApiResponse::ok())
}

async fn archive_record_for_form(
    state: &AppState,
    form: &FormResponse,
    record: RecordResponse,
    actor_id: Uuid,
    is_bot: bool,
    idempotency_key: Option<String>,
) -> Result<RecordResponse, ApiError> {
    let idempotency_key = normalize_optional_idempotency_key(idempotency_key)?;
    if let Some(record) =
        find_idempotent_record(state, form, idempotency_key.as_deref(), "form.record.archived").await?
    {
        return Ok(record);
    }
    if record.archived_at.is_none() {
        ensure_required_child_tables_keep_a_row(state, record.id).await?;
    }
    let created_by = if is_bot { None } else { Some(actor_id) };
    let tx = state.db.begin().await?;
    insert_form_event_with_idempotency(
        &tx,
        form,
        Some(record.id),
        "form.record.archived",
        created_by,
        json!({ "type": if is_bot { "bot" } else { "user" } }),
        json!({ "record_id": record.id, "title": record.title }),
        idempotency_key,
    )
    .await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE form_records SET archived_at = COALESCE(archived_at, now()), updated_at = now() WHERE id = $1",
        vec![record.id.into()],
    ))
    .await?;
    tx.commit().await?;
    recalculate_parent_records_for_child(state, record.id, created_by).await?;
    find_record(state, record.id).await
}

pub async fn restore_form_record(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(record_id): Path<Uuid>,
    Query(query): Query<IdempotencyQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let record = find_record(&state, record_id).await?;
    let form = find_form(&state, record.form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.delete").await?;
    ensure_record_scope_allows_created_by(&state, form.id, &role, actor_id, record.created_by).await?;
    let restored = restore_record_for_form(&state, &form, record, actor_id, is_bot, query.idempotency_key).await?;
    let denied_read_fields = denied_read_field_keys(&state, form.id, &role).await?;
    Ok(ApiResponse::success(filter_record_response_values(
        restored,
        &denied_read_fields,
    )))
}

async fn restore_record_for_form(
    state: &AppState,
    form: &FormResponse,
    record: RecordResponse,
    actor_id: Uuid,
    is_bot: bool,
    idempotency_key: Option<String>,
) -> Result<RecordResponse, ApiError> {
    let idempotency_key = normalize_optional_idempotency_key(idempotency_key)?;
    if let Some(record) =
        find_idempotent_record(state, form, idempotency_key.as_deref(), "form.record.restored").await?
    {
        return Ok(record);
    }
    let created_by = if is_bot { None } else { Some(actor_id) };
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE form_records SET archived_at = NULL, updated_at = now() WHERE id = $1",
        vec![record.id.into()],
    ))
    .await?;
    insert_form_event_with_idempotency(
        &tx,
        form,
        Some(record.id),
        "form.record.restored",
        created_by,
        json!({ "type": if is_bot { "bot" } else { "user" } }),
        json!({ "record_id": record.id, "title": record.title }),
        idempotency_key,
    )
    .await?;
    tx.commit().await?;
    recalculate_parent_records_for_child(state, record.id, created_by).await?;
    find_record(state, record.id).await
}

pub async fn create_record_link(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(record_id): Path<Uuid>,
    Json(req): Json<CreateLinkRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let record = find_record(&state, record_id).await?;
    let form = find_form(&state, record.form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "record.update").await?;
    ensure_record_scope_allows_created_by(&state, form.id, &role, actor_id, record.created_by).await?;
    let target_type = normalize_target_type(&req.target_type)?;
    let relation_type = normalize_relation_type(&req.relation_type)?;
    let relation_key = normalize_key(&req.relation_key).map_err(ApiError::BadRequest)?;
    let metadata = ensure_json_object(req.metadata.unwrap_or_else(|| json!({})), "metadata")?;
    let target_id = if target_type == "form_record" {
        let target_record_id = Uuid::parse_str(req.target_id.trim())
            .map_err(|_| ApiError::BadRequest("target_id must be a form record id".to_string()))?;
        let target_record = find_record(&state, target_record_id).await?;
        ensure_link_target_scope(
            record.workspace_id,
            record.project_id,
            target_record.workspace_id,
            target_record.project_id,
        )?;
        let target_form = find_form(&state, target_record.form_id).await?;
        let (_, target_role, _) =
            require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &target_form, "form.view").await?;
        ensure_record_scope_allows_created_by(&state, target_form.id, &target_role, actor_id, target_record.created_by)
            .await?;
        if relation_type == "parent_child" {
            validate_optional_parent_child_relation(&form.schema, &target_form.key, &relation_key)?;
        }
        target_record_id.to_string()
    } else {
        req.target_id
    };
    let link_id = Uuid::new_v4();
    let created_by = if is_bot { None } else { Some(actor_id) };
    let tx = state.db.begin().await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
                INSERT INTO form_record_links (
                    id, workspace_id, project_id, source_record_id, target_type,
                    target_id, relation_key, relation_type, metadata, created_by, created_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now())
            ",
        vec![
            link_id.into(),
            record.workspace_id.into(),
            record.project_id.into(),
            record_id.into(),
            target_type.clone().into(),
            target_id.into(),
            relation_key.into(),
            relation_type.clone().into(),
            metadata.into(),
            created_by.into(),
        ],
    ))
    .await?;

    insert_form_event(
        &tx,
        &form,
        Some(record_id),
        "form.record.linked",
        created_by,
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({ "link_id": link_id }),
    )
    .await?;
    tx.commit().await?;
    let link = find_link(&state, link_id).await?;
    if relation_type == "parent_child" && target_type == "form_record" {
        recalculate_record_values(&state, record_id, created_by, "form.record.child_aggregate.updated").await?;
    }
    Ok(ApiResponse::success(link))
}

pub async fn list_record_links(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(record_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let record = find_record(&state, record_id).await?;
    let form = find_form(&state, record.form_id).await?;
    let (actor_id, role, _) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.view").await?;
    ensure_record_scope_allows_created_by(&state, form.id, &role, actor_id, record.created_by).await?;
    let links = LinkResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, source_record_id, target_type,
                   target_id, relation_key, relation_type, metadata, created_by, created_at
            FROM form_record_links
            WHERE source_record_id = $1
            ORDER BY created_at DESC
        ",
        vec![record_id.into()],
    ))
    .all(&state.db)
    .await?;
    Ok(ApiResponse::success(links))
}

pub async fn list_relation_targets(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Query(query): Query<RelationTargetsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form(&state, form_id).await?;
    require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.view").await?;

    let target_config = relation_target_config(&form.schema, query.field_key.as_deref(), query.form_key.as_deref())?;
    let target_form_key = target_config.form_key;
    let display_field = target_config.display_field;
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    let target_form = FormResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, key, name, description, icon, color,
                   title_template, schema, detail_layout, schema_version, created_by, archived_at, created_at, updated_at
            FROM project_forms
            WHERE workspace_id = $1 AND project_id = $2 AND key = $3 AND archived_at IS NULL
        ",
        vec![form.workspace_id.into(), form.project_id.into(), target_form_key.clone().into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("relation target form not found".to_string()))?;
    let (target_actor_id, target_role, _) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &target_form, "form.view").await?;
    let denied_read_fields = denied_read_field_keys(&state, target_form.id, &target_role).await?;

    let mut where_parts = vec!["form_id = $1".to_string(), "archived_at IS NULL".to_string()];
    let mut values: Vec<sea_orm::Value> = vec![target_form.id.into()];
    if form_record_scope(&state, target_form.id, &target_role).await? == "owned" {
        values.push(target_actor_id.into());
        where_parts.push(format!("created_by = ${}", values.len()));
    }
    if let Some(q) = query
        .q
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        values.push(format!("%{q}%").into());
        let idx = values.len();
        // Searching the raw value document would let an actor probe fields they cannot read.
        if denied_read_fields.is_empty() {
            where_parts.push(format!("(title ILIKE ${idx} OR values::text ILIKE ${idx})"));
        } else {
            where_parts.push(format!("title ILIKE ${idx}"));
        }
    }

    let total_sql = format!(
        "SELECT COUNT(*)::bigint AS count FROM form_records WHERE {}",
        where_parts.join(" AND ")
    );
    let total = count_query(&state, &total_sql, values.clone()).await?;
    values.push(target_form.key.clone().into());
    let form_key_idx = values.len();
    values.push(target_form.name.clone().into());
    let form_name_idx = values.len();
    values.push(per_page.into());
    let limit_idx = values.len();
    values.push(offset.into());
    let offset_idx = values.len();
    let sql = format!(
        r"
            SELECT id AS record_id, form_id, ${form_key_idx} AS form_key, ${form_name_idx} AS form_name, title,
                   title AS display, values, updated_at
            FROM form_records
            WHERE {}
            ORDER BY updated_at DESC
            LIMIT ${limit_idx} OFFSET ${offset_idx}
        ",
        where_parts.join(" AND ")
    );

    let mut items =
        RelationTargetResponse::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .all(&state.db)
            .await?;
    if let Some(display_field) = display_field {
        for item in &mut items {
            if !denied_read_fields.contains(&display_field) {
                if let Some(display) = item.values.get(&display_field).map(display_value) {
                    item.display = display;
                }
            }
        }
    }
    let items = items
        .into_iter()
        .map(|item| filter_relation_target_values(item, &denied_read_fields))
        .collect();

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages: total_pages(total, per_page),
    }))
}

pub async fn list_record_children(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(record_id): Path<Uuid>,
    Query(query): Query<ListChildrenQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let record = find_record(&state, record_id).await?;
    let form = find_form(&state, record.form_id).await?;
    let (actor_id, role, is_bot) =
        require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &form, "form.view").await?;
    ensure_record_scope_allows_created_by(&state, form.id, &role, actor_id, record.created_by).await?;
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(50).clamp(1, 200);
    let offset = (page - 1) * per_page;

    let mut where_parts = vec![
        "links.source_record_id = $1".to_string(),
        "links.target_type = 'form_record'".to_string(),
        "links.relation_type = 'parent_child'".to_string(),
        "child.archived_at IS NULL".to_string(),
    ];
    let mut values: Vec<sea_orm::Value> = vec![record_id.into()];
    // A forged link row must never expose a record from another tenant.
    values.push(record.workspace_id.into());
    where_parts.push(format!("child.workspace_id = ${}", values.len()));
    values.push(record.project_id.into());
    where_parts.push(format!("child.project_id = ${}", values.len()));
    if !is_bot && !role_is_form_admin(&role) {
        values.push(actor_id.into());
        let actor_idx = values.len();
        values.push(normalize_permission_subject_id(&role)?.into());
        let role_idx = values.len();
        where_parts.push(role_record_scope_predicate_sql("child", actor_idx, role_idx)?);
    }
    if let Some(relation_key) = normalize_optional_relation_key(query.relation_key.as_deref())? {
        values.push(relation_key.into());
        where_parts.push(format!("links.relation_key = ${}", values.len()));
    }
    if let Some(child_form_key) = normalize_optional_child_form_key(query.child_form_key.as_deref())? {
        values.push(child_form_key.into());
        where_parts.push(format!("child_form.key = ${}", values.len()));
    }

    let total_sql = format!(
        r"
            SELECT COUNT(*)::bigint AS count
            FROM form_record_links links
            JOIN form_records child ON child.id::text = links.target_id
            JOIN project_forms child_form ON child_form.id = child.form_id
            WHERE {}
        ",
        where_parts.join(" AND ")
    );
    let total = count_query(&state, &total_sql, values.clone()).await?;
    values.push(per_page.into());
    values.push(offset.into());
    let limit_idx = values.len() - 1;
    let offset_idx = values.len();
    let sql = format!(
        r"
            SELECT links.id AS link_id, links.relation_key, links.relation_type,
                   child.id AS record_id, child.workspace_id, child.project_id, child.form_id,
                   child.title, child.values, child.source, child.schema_version,
                   child.created_by, child.updated_by, child.archived_at, child.created_at, child.updated_at
            FROM form_record_links links
            JOIN form_records child ON child.id::text = links.target_id
            JOIN project_forms child_form ON child_form.id = child.form_id
            WHERE {}
            ORDER BY links.created_at DESC
            LIMIT ${limit_idx} OFFSET ${offset_idx}
        ",
        where_parts.join(" AND ")
    );
    let rows = ChildRecordRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .all(&state.db)
        .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let child_form = find_form(&state, row.form_id).await?;
        let (_, child_role, _) =
            require_form_action(&state, &claims, bot.as_ref().map(|b| &b.0), &child_form, "form.view").await?;
        let denied_read_fields = denied_read_field_keys(&state, child_form.id, &child_role).await?;
        let record = RecordResponse {
            id: row.record_id,
            workspace_id: row.workspace_id,
            project_id: row.project_id,
            form_id: row.form_id,
            title: row.title,
            values: row.values,
            source: row.source,
            schema_version: row.schema_version,
            created_by: row.created_by,
            updated_by: row.updated_by,
            archived_at: row.archived_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        };
        items.push(ChildRecordResponse {
            link_id: row.link_id,
            relation_key: row.relation_key,
            relation_type: row.relation_type,
            record: filter_record_response_values(record, &denied_read_fields),
        });
    }

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages: total_pages(total, per_page),
    }))
}

pub async fn create_child_record(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(record_id): Path<Uuid>,
    Json(req): Json<CreateChildRecordRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let parent_record = find_record(&state, record_id).await?;
    let parent_form = find_form(&state, parent_record.form_id).await?;
    let (actor_id, parent_role, is_bot) = require_form_action(
        &state,
        &claims,
        bot.as_ref().map(|b| &b.0),
        &parent_form,
        "record.update",
    )
    .await?;
    ensure_record_scope_allows_created_by(&state, parent_form.id, &parent_role, actor_id, parent_record.created_by)
        .await?;
    let child_form = resolve_child_form(
        &state,
        parent_form.workspace_id,
        parent_form.project_id,
        req.child_form_id,
        req.child_form_key.as_deref(),
    )
    .await?;
    let (_, child_role, _) = require_form_action(
        &state,
        &claims,
        bot.as_ref().map(|b| &b.0),
        &child_form,
        "record.create",
    )
    .await?;
    let relation_key = validate_parent_child_relation(&parent_form.schema, &child_form.key, &req.relation_key)?;
    let metadata = ensure_json_object(req.metadata.unwrap_or_else(|| json!({})), "metadata")?;
    let child_record = create_record_for_form(
        &state,
        &child_form,
        actor_id,
        &child_role,
        is_bot,
        CreateRecordRequest {
            values: req.values,
            title: req.title,
            source: req.source,
            idempotency_key: None,
        },
    )
    .await?;
    let link = create_parent_child_link(
        &state,
        &parent_record,
        &parent_form,
        child_record.id,
        &relation_key,
        metadata,
        actor_id,
        is_bot,
    )
    .await?;
    let denied_read_fields = denied_read_field_keys(&state, child_form.id, &child_role).await?;

    Ok(ApiResponse::success(ChildRecordMutationResponse {
        link,
        record: filter_record_response_values(child_record, &denied_read_fields),
    }))
}

pub async fn update_child_record(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((record_id, child_record_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateChildRecordRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let parent_record = find_record(&state, record_id).await?;
    let parent_form = find_form(&state, parent_record.form_id).await?;
    let (actor_id, _, is_bot) = require_form_action(
        &state,
        &claims,
        bot.as_ref().map(|b| &b.0),
        &parent_form,
        "record.update",
    )
    .await?;
    let child_record = find_record(&state, child_record_id).await?;
    let child_form = find_form(&state, child_record.form_id).await?;
    let (_, child_role, _) = require_form_action(
        &state,
        &claims,
        bot.as_ref().map(|b| &b.0),
        &child_form,
        "record.update",
    )
    .await?;
    ensure_record_scope_allows_created_by(&state, child_form.id, &child_role, actor_id, child_record.created_by)
        .await?;
    let relation_key = match req.relation_key.as_deref() {
        Some(relation_key) => Some(validate_parent_child_relation(
            &parent_form.schema,
            &child_form.key,
            relation_key,
        )?),
        None => None,
    };
    let link = find_parent_child_link(&state, record_id, child_record_id, relation_key.as_deref()).await?;
    let updated = update_record_for_form(
        &state,
        &child_form,
        child_record,
        actor_id,
        &child_role,
        is_bot,
        UpdateRecordRequest {
            values: req.values,
            title: req.title,
            source: req.source,
            idempotency_key: None,
        },
    )
    .await?;
    let denied_read_fields = denied_read_field_keys(&state, child_form.id, &child_role).await?;

    Ok(ApiResponse::success(ChildRecordMutationResponse {
        link,
        record: filter_record_response_values(updated, &denied_read_fields),
    }))
}

pub async fn archive_child_record(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((record_id, child_record_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<ChildRecordRelationQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let parent_record = find_record(&state, record_id).await?;
    let parent_form = find_form(&state, parent_record.form_id).await?;
    let (actor_id, parent_role, is_bot) = require_form_action(
        &state,
        &claims,
        bot.as_ref().map(|b| &b.0),
        &parent_form,
        "record.update",
    )
    .await?;
    ensure_record_scope_allows_created_by(&state, parent_form.id, &parent_role, actor_id, parent_record.created_by)
        .await?;
    let child_record = find_record(&state, child_record_id).await?;
    let child_form = find_form(&state, child_record.form_id).await?;
    let (_, child_role, _) = require_form_action(
        &state,
        &claims,
        bot.as_ref().map(|b| &b.0),
        &child_form,
        "record.delete",
    )
    .await?;
    ensure_record_scope_allows_created_by(&state, child_form.id, &child_role, actor_id, child_record.created_by)
        .await?;
    let relation_key = match query.relation_key.as_deref() {
        Some(relation_key) => Some(validate_parent_child_relation(
            &parent_form.schema,
            &child_form.key,
            relation_key,
        )?),
        None => None,
    };
    let link = find_parent_child_link(&state, record_id, child_record_id, relation_key.as_deref()).await?;
    let archived = archive_record_for_form(&state, &child_form, child_record, actor_id, is_bot, None).await?;
    let denied_read_fields = denied_read_field_keys(&state, child_form.id, &child_role).await?;

    Ok(ApiResponse::success(ChildRecordMutationResponse {
        link,
        record: filter_record_response_values(archived, &denied_read_fields),
    }))
}

pub async fn restore_child_record(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((record_id, child_record_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<ChildRecordRelationQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let parent_record = find_record(&state, record_id).await?;
    let parent_form = find_form(&state, parent_record.form_id).await?;
    let (actor_id, parent_role, is_bot) = require_form_action(
        &state,
        &claims,
        bot.as_ref().map(|b| &b.0),
        &parent_form,
        "record.update",
    )
    .await?;
    ensure_record_scope_allows_created_by(&state, parent_form.id, &parent_role, actor_id, parent_record.created_by)
        .await?;
    let child_record = find_record(&state, child_record_id).await?;
    let child_form = find_form(&state, child_record.form_id).await?;
    let (_, child_role, _) = require_form_action(
        &state,
        &claims,
        bot.as_ref().map(|b| &b.0),
        &child_form,
        "record.delete",
    )
    .await?;
    ensure_record_scope_allows_created_by(&state, child_form.id, &child_role, actor_id, child_record.created_by)
        .await?;
    let relation_key = match query.relation_key.as_deref() {
        Some(relation_key) => Some(validate_parent_child_relation(
            &parent_form.schema,
            &child_form.key,
            relation_key,
        )?),
        None => None,
    };
    let link = find_parent_child_link(&state, record_id, child_record_id, relation_key.as_deref()).await?;
    let restored = restore_record_for_form(&state, &child_form, child_record, actor_id, is_bot, None).await?;
    let denied_read_fields = denied_read_field_keys(&state, child_form.id, &child_role).await?;

    Ok(ApiResponse::success(ChildRecordMutationResponse {
        link,
        record: filter_record_response_values(restored, &denied_read_fields),
    }))
}

async fn resolve_child_form(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Uuid,
    child_form_id: Option<Uuid>,
    child_form_key: Option<&str>,
) -> Result<FormResponse, ApiError> {
    if let Some(child_form_id) = child_form_id {
        let form = find_form(state, child_form_id).await?;
        if form.workspace_id != workspace_id || form.project_id != project_id || form.archived_at.is_some() {
            return Err(ApiError::NotFound("child form not found".to_string()));
        }
        return Ok(form);
    }
    let child_form_key = child_form_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::BadRequest("child_form_id or child_form_key is required".to_string()))?;
    let child_form_key = normalize_key(child_form_key).map_err(ApiError::BadRequest)?;
    FormResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, key, name, description, icon, color,
                   title_template, schema, detail_layout, schema_version, created_by, archived_at, created_at, updated_at
            FROM project_forms
            WHERE workspace_id = $1 AND project_id = $2 AND key = $3 AND archived_at IS NULL
        ",
        vec![workspace_id.into(), project_id.into(), child_form_key.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("child form not found".to_string()))
}

async fn create_parent_child_link(
    state: &AppState,
    parent_record: &RecordResponse,
    parent_form: &FormResponse,
    child_record_id: Uuid,
    relation_key: &str,
    metadata: Value,
    actor_id: Uuid,
    is_bot: bool,
) -> Result<LinkResponse, ApiError> {
    let link_id = Uuid::new_v4();
    let created_by = if is_bot { None } else { Some(actor_id) };
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO form_record_links (
                id, workspace_id, project_id, source_record_id, target_type,
                target_id, relation_key, relation_type, metadata, created_by, created_at
            )
            VALUES ($1, $2, $3, $4, 'form_record', $5, $6, 'parent_child', $7, $8, now())
        ",
        vec![
            link_id.into(),
            parent_record.workspace_id.into(),
            parent_record.project_id.into(),
            parent_record.id.into(),
            child_record_id.to_string().into(),
            relation_key.to_string().into(),
            metadata.into(),
            created_by.into(),
        ],
    ))
    .await?;
    insert_form_event(
        &tx,
        parent_form,
        Some(parent_record.id),
        "form.record.child_created",
        created_by,
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({
            "link_id": link_id,
            "parent_record_id": parent_record.id,
            "child_record_id": child_record_id,
            "relation_key": relation_key
        }),
    )
    .await?;
    tx.commit().await?;
    recalculate_record_values(
        state,
        parent_record.id,
        created_by,
        "form.record.child_aggregate.updated",
    )
    .await?;
    find_link(state, link_id).await
}

async fn calculate_and_normalize_values(
    state: &AppState,
    form: &FormResponse,
    record_id: Option<Uuid>,
    values: Value,
) -> Result<Value, ApiError> {
    let calculated = calculate_values(state, form, record_id, values).await?;
    validate_and_normalize_values(&form.schema, calculated).map_err(ApiError::BadRequest)
}

async fn calculate_values(
    state: &AppState,
    form: &FormResponse,
    record_id: Option<Uuid>,
    values: Value,
) -> Result<Value, ApiError> {
    calculate_values_with_existing(state, form, record_id, None, values).await
}

/// Runs the calculation pipeline against the record as it will look after the write.
///
/// Formula arguments are resolved from the stored record merged with the caller's payload, so a
/// partial update — the only kind a caller with `read:false` on a formula input can send — no longer
/// fails with `formula field 'x' is missing`. Only the values the calculation changed are folded
/// back onto the caller's payload, so nothing downstream mistakes a stored value for a submitted
/// one: the field-level write policy is checked before this against the raw payload, autonumber
/// still resolves its own carry-over from `existing_values`, and normalization still sees exactly
/// the keys the caller mentioned plus the calculated fields.
async fn calculate_values_with_existing(
    state: &AppState,
    form: &FormResponse,
    record_id: Option<Uuid>,
    existing_values: Option<&Value>,
    values: Value,
) -> Result<Value, ApiError> {
    let seeded = merge_values_for_calculation(&form.schema, &values, existing_values).map_err(ApiError::BadRequest)?;
    let calculated = evaluate_calculated_values(&form.schema, seeded.clone()).map_err(ApiError::BadRequest)?;
    let calculated = apply_child_aggregates(state, form, record_id, calculated).await?;
    overlay_calculated_values(values, &seeded, &calculated).map_err(ApiError::BadRequest)
}

async fn apply_autonumber_values(
    tx: &DatabaseTransaction,
    form: &FormResponse,
    existing_values: Option<&Value>,
    values: Value,
) -> Result<Value, ApiError> {
    let mut output = values
        .as_object()
        .cloned()
        .ok_or_else(|| ApiError::BadRequest("values must be a JSON object".to_string()))?;
    let autonumber_fields = autonumber_schema_fields(&form.schema);
    if autonumber_fields.is_empty() {
        return Ok(Value::Object(output));
    }

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtext($1::text))",
        vec![format!("forms.autonumber:{}", form.id).into()],
    ))
    .await?;

    for field in autonumber_fields {
        if let Some(existing) = existing_values.and_then(|item| item.get(&field.key)) {
            if !existing.is_null() {
                output.insert(field.key, existing.clone());
                continue;
            }
        }
        let next_number = next_autonumber_ordinal(tx, form.id, &field.key).await? + 1;
        output.insert(
            field.key,
            Value::String(format!("{}{:0width$}", field.prefix, next_number, width = field.width)),
        );
    }

    Ok(Value::Object(output))
}

#[derive(Debug, Clone)]
struct AutonumberSchemaField {
    key: String,
    prefix: String,
    width: usize,
}

fn autonumber_schema_fields(schema: &Value) -> Vec<AutonumberSchemaField> {
    schema
        .get("fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|field| {
            let object = field.as_object()?;
            if object.get("type").and_then(Value::as_str) != Some("autonumber") {
                return None;
            }
            let key = object.get("key").and_then(Value::as_str)?.trim();
            if key.is_empty() {
                return None;
            }
            let config = object.get("autonumber").and_then(Value::as_object);
            let prefix = config
                .and_then(|item| item.get("prefix"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| "AUTO-".to_string());
            let width = config
                .and_then(|item| item.get("width"))
                .and_then(Value::as_u64)
                .unwrap_or(6)
                .clamp(1, 18) as usize;
            Some(AutonumberSchemaField {
                key: key.to_string(),
                prefix,
                width,
            })
        })
        .collect()
}

async fn next_autonumber_ordinal(tx: &DatabaseTransaction, form_id: Uuid, field_key: &str) -> Result<i64, ApiError> {
    tx.query_one(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT COALESCE(
                MAX(NULLIF(regexp_replace(values ->> $2, '\D', '', 'g'), '')::bigint),
                0
            )::bigint AS count
            FROM form_records
            WHERE form_id = $1
              AND values ? $2
        ",
        vec![form_id.into(), field_key.to_string().into()],
    ))
    .await?
    .ok_or_else(|| ApiError::Internal)?
    .try_get::<i64>("", "count")
    .map_err(ApiError::from)
}

async fn apply_child_aggregates(
    state: &AppState,
    form: &FormResponse,
    record_id: Option<Uuid>,
    values: Value,
) -> Result<Value, ApiError> {
    let Some(parent_record_id) = record_id else {
        return Ok(values);
    };
    let mut output = values
        .as_object()
        .cloned()
        .ok_or_else(|| ApiError::BadRequest("values must be a JSON object".to_string()))?;
    let fields = parse_fields(&form.schema).map_err(ApiError::BadRequest)?;
    let schema_fields = form
        .schema
        .get("fields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    for field in fields {
        let Some(field_schema) = schema_fields
            .iter()
            .find(|item| item.get("key").and_then(Value::as_str) == Some(field.key.as_str()))
        else {
            continue;
        };
        let Some(formula) = field_schema.get("formula").and_then(Value::as_object) else {
            continue;
        };
        let Some(op) = formula.get("op").and_then(Value::as_str) else {
            continue;
        };
        if !op.starts_with("child_") {
            continue;
        }
        let relation_key = formula
            .get("relation_key")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::BadRequest("child aggregate formula requires relation_key".to_string()))
            .and_then(|value| normalize_key(value).map_err(ApiError::BadRequest))?;
        let child_field = formula.get("field").and_then(Value::as_str).map(str::to_string);
        let aggregate = child_aggregate_decimal(
            state,
            (form.workspace_id, form.project_id),
            parent_record_id,
            &relation_key,
            child_field.as_deref(),
            op,
        )
        .await?;
        let field_value = child_aggregate_field_value(&field, aggregate)?;
        output.insert(field.key.clone(), field_value);
    }

    Ok(Value::Object(output))
}

/// Child rows are restricted to the parent's workspace and project so a forged link row can never
/// pull a decimal value out of another tenant.
const CHILD_AGGREGATE_COUNT_SQL: &str = r"
    SELECT COUNT(*)::bigint AS count
    FROM form_record_links links
    JOIN form_records child
      ON links.target_id ~ '^[0-9a-fA-F-]{36}$'
     AND child.id = links.target_id::uuid
    WHERE links.source_record_id = $1
      AND links.target_type = 'form_record'
      AND links.relation_type = 'parent_child'
      AND links.relation_key = $2
      AND child.workspace_id = $3
      AND child.project_id = $4
      AND child.archived_at IS NULL
";

const CHILD_AGGREGATE_DECIMAL_SQL: &str = r"
    SELECT field_index.value_decimal::text AS decimal
    FROM form_record_links links
    JOIN form_records child
      ON links.target_id ~ '^[0-9a-fA-F-]{36}$'
     AND child.id = links.target_id::uuid
    JOIN form_record_field_index field_index
      ON field_index.record_id = child.id
     AND field_index.field_key = $3
    WHERE links.source_record_id = $1
      AND links.target_type = 'form_record'
      AND links.relation_type = 'parent_child'
      AND links.relation_key = $2
      AND child.workspace_id = $4
      AND child.project_id = $5
      AND child.archived_at IS NULL
      AND field_index.value_decimal IS NOT NULL
";

async fn child_aggregate_decimal(
    state: &AppState,
    tenant: (Uuid, Uuid),
    parent_record_id: Uuid,
    relation_key: &str,
    child_field: Option<&str>,
    op: &str,
) -> Result<Decimal, ApiError> {
    let (workspace_id, project_id) = tenant;
    if op == "child_count" && child_field.is_none() {
        let count = count_query(
            state,
            CHILD_AGGREGATE_COUNT_SQL,
            vec![
                parent_record_id.into(),
                relation_key.to_string().into(),
                workspace_id.into(),
                project_id.into(),
            ],
        )
        .await?;
        return Ok(Decimal::from(count));
    }
    let child_field = child_field
        .ok_or_else(|| ApiError::BadRequest("child aggregate formula requires field".to_string()))
        .and_then(|value| normalize_key(value).map_err(ApiError::BadRequest))?;
    let rows = ChildDecimalRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        CHILD_AGGREGATE_DECIMAL_SQL,
        vec![
            parent_record_id.into(),
            relation_key.to_string().into(),
            child_field.into(),
            workspace_id.into(),
            project_id.into(),
        ],
    ))
    .all(&state.db)
    .await?;
    let decimals = rows
        .into_iter()
        .filter_map(|row| row.decimal)
        .map(|value| Decimal::from_str(value.trim()).map_err(|_| ApiError::Internal))
        .collect::<Result<Vec<_>, _>>()?;

    match op {
        "child_sum" => Ok(decimals.into_iter().sum()),
        "child_count" => Ok(Decimal::from(decimals.len())),
        "child_min" => Ok(decimals.into_iter().min().unwrap_or(Decimal::ZERO)),
        "child_max" => Ok(decimals.into_iter().max().unwrap_or(Decimal::ZERO)),
        _ => Err(ApiError::BadRequest("unsupported child aggregate op".to_string())),
    }
}

fn child_aggregate_field_value(field: &FormField, value: Decimal) -> Result<Value, ApiError> {
    match field.field_type.as_str() {
        "amount" => Ok(json!(value.normalize().to_string())),
        "number" | "formula" => Ok(json!(value.normalize().to_string())),
        "integer" => {
            let raw = value.normalize().to_string();
            let parsed = raw
                .parse::<i64>()
                .map_err(|_| ApiError::BadRequest("child aggregate result must be integer".to_string()))?;
            Ok(json!(parsed))
        }
        _ => Ok(json!(value.normalize().to_string())),
    }
}

#[derive(Debug, FromQueryResult)]
struct ParentRequiredChildTableRow {
    relation_key: String,
    parent_record_id: Uuid,
    parent_schema: Value,
    remaining_children: i64,
}

/// Every parent this record hangs under as a `parent_child` row, with the parent's schema and the
/// number of *other* live child rows sharing the same `relation_key`. Sibling counting excludes the
/// record being archived and already archived rows, so `remaining_children = 0` means archiving
/// empties that child table.
const PARENT_REQUIRED_CHILD_TABLES_SQL: &str = r"
    SELECT links.relation_key,
           links.source_record_id AS parent_record_id,
           parent_form.schema AS parent_schema,
           (
               SELECT COUNT(*)::bigint
               FROM form_record_links sibling_links
               JOIN form_records sibling
                 ON sibling_links.target_id ~ '^[0-9a-fA-F-]{36}$'
                AND sibling.id = sibling_links.target_id::uuid
               WHERE sibling_links.source_record_id = links.source_record_id
                 AND sibling_links.relation_key = links.relation_key
                 AND sibling_links.target_type = 'form_record'
                 AND sibling_links.relation_type = 'parent_child'
                 AND sibling.id <> child.id
                 AND sibling.archived_at IS NULL
                 AND sibling.workspace_id = parent.workspace_id
                 AND sibling.project_id = parent.project_id
           ) AS remaining_children
    FROM form_record_links links
    JOIN form_records parent ON parent.id = links.source_record_id
    JOIN project_forms parent_form ON parent_form.id = parent.form_id
    JOIN form_records child
      ON links.target_id ~ '^[0-9a-fA-F-]{36}$'
     AND child.id = links.target_id::uuid
    WHERE links.target_type = 'form_record'
      AND links.relation_type = 'parent_child'
      AND links.target_id = $1
      AND parent.archived_at IS NULL
      AND parent.workspace_id = child.workspace_id
      AND parent.project_id = child.project_id
";

/// Whether `relation_key` names a required `child_table` on the parent schema that archiving would
/// leave with no rows. Kept separate from the query so the rule itself is testable without a
/// database.
fn required_child_table_would_be_emptied(
    parent_schema: &Value,
    relation_key: &str,
    remaining_children: i64,
) -> Result<bool, ApiError> {
    if remaining_children > 0 {
        return Ok(false);
    }
    let fields = parse_fields(parent_schema).map_err(ApiError::BadRequest)?;
    Ok(fields
        .iter()
        .any(|field| field.key == relation_key && field.required && field.field_type == "child_table"))
}

/// Enforces required `child_table` fields at the only operation that can violate them.
///
/// A child table's rows are separate records linked by `form_record_links`, never values on the
/// parent, so "this child table has at least one row" cannot be checked while normalizing the
/// parent's values — a parent has no rows the instant it is created, and normalization runs again
/// on every later write including the recalculation triggered by adding the first row. What can be
/// checked is the row count, and the only request that can drive it to zero is archiving the last
/// row, so that request is where the constraint is enforced.
async fn ensure_required_child_tables_keep_a_row(state: &AppState, child_record_id: Uuid) -> Result<(), ApiError> {
    let parents = ParentRequiredChildTableRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        PARENT_REQUIRED_CHILD_TABLES_SQL,
        vec![child_record_id.to_string().into()],
    ))
    .all(&state.db)
    .await?;
    for parent in parents {
        if required_child_table_would_be_emptied(
            &parent.parent_schema,
            &parent.relation_key,
            parent.remaining_children,
        )? {
            return Err(ApiError::BadRequest(format!(
                "field '{}' requires at least one child row on record {}",
                parent.relation_key, parent.parent_record_id
            )));
        }
    }
    Ok(())
}

/// Parents to recalculate after a child record changed. The child is joined on its uuid primary key
/// (guarded by a uuid shaped `target_id`) like the child aggregates are, so the lookup stays
/// indexable instead of casting every `form_records.id` to text.
const RECALCULATE_PARENT_RECORDS_SQL: &str = r"
    SELECT links.source_record_id
    FROM form_record_links links
    JOIN form_records parent ON parent.id = links.source_record_id
    JOIN form_records child
      ON links.target_id ~ '^[0-9a-fA-F-]{36}$'
     AND child.id = links.target_id::uuid
    WHERE links.target_type = 'form_record'
      AND links.relation_type = 'parent_child'
      AND links.target_id = $1
      AND parent.workspace_id = child.workspace_id
      AND parent.project_id = child.project_id
";

async fn recalculate_parent_records_for_child(
    state: &AppState,
    child_record_id: Uuid,
    actor_id: Option<Uuid>,
) -> Result<(), ApiError> {
    let parents = ParentRecordRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        RECALCULATE_PARENT_RECORDS_SQL,
        vec![child_record_id.to_string().into()],
    ))
    .all(&state.db)
    .await?;
    for parent in parents {
        recalculate_record_values(
            state,
            parent.source_record_id,
            actor_id,
            "form.record.child_aggregate.updated",
        )
        .await?;
    }
    Ok(())
}

async fn recalculate_record_values(
    state: &AppState,
    record_id: Uuid,
    actor_id: Option<Uuid>,
    event_type: &str,
) -> Result<(), ApiError> {
    let record = find_record(state, record_id).await?;
    let form = find_form(state, record.form_id).await?;
    let source = json!({ "type": "system", "operation": event_type });
    let tx = state.db.begin().await?;
    let calculated = calculate_values(state, &form, Some(record_id), record.values.clone()).await?;
    let with_autonumber = apply_autonumber_values(&tx, &form, Some(&record.values), calculated).await?;
    let values = validate_and_normalize_values_with_existing(&form.schema, with_autonumber, &record.values)
        .map_err(ApiError::BadRequest)?;
    let title = render_title(&form.title_template, record_id, &values);
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE form_records
            SET title = $1, values = $2, source = $3, schema_version = $4, updated_by = $5, updated_at = now()
            WHERE id = $6
        ",
        vec![
            title.into(),
            values.clone().into(),
            source.clone().into(),
            form.schema_version.into(),
            actor_id.into(),
            record_id.into(),
        ],
    ))
    .await?;
    refresh_record_projection(&tx, form.project_id, form.id, record_id, &form.schema, &values).await?;
    let event_payload = json!({ "record_id": record_id, "values": values });
    insert_form_event(&tx, &form, Some(record_id), event_type, actor_id, source, event_payload).await?;
    tx.commit().await?;
    Ok(())
}

fn schema_insight_input(form: &FormResponse) -> SchemaInsightInput<'_> {
    SchemaInsightInput {
        form_id: form.id,
        form_key: &form.key,
        schema_version: form.schema_version,
        schema: &form.schema,
        detail_layout: &form.detail_layout,
        title_template: &form.title_template,
    }
}

async fn export_column_keys(
    state: &AppState,
    form_id: Uuid,
    fields: &[FormField],
    query: &ExportRecordsQuery,
) -> Result<Vec<String>, ApiError> {
    let mut raw_keys = Vec::new();
    if let Some(view_id) = query.view_id {
        let view = find_view(state, view_id).await?;
        if view.form_id != form_id {
            return Err(ApiError::BadRequest("view_id does not belong to form".to_string()));
        }
        if view.archived_at.is_some() {
            return Err(ApiError::BadRequest("view is archived".to_string()));
        }
        raw_keys.extend(export_columns_from_config(&view.config));
    }
    if raw_keys.is_empty() {
        if let Some(columns) = query.columns.as_deref() {
            raw_keys.extend(
                columns
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            );
        }
    }
    if raw_keys.is_empty() {
        raw_keys.extend(fields.iter().take(8).map(|field| field.key.clone()));
    }

    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for key in raw_keys {
        let field = find_schema_field(fields, &key)?;
        if seen.insert(field.key.clone()) {
            normalized.push(field.key.clone());
        }
    }
    Ok(normalized)
}

async fn ensure_export_view_access(state: &AppState, form_id: Uuid, view_id: Uuid) -> Result<(), ApiError> {
    let view = find_view(state, view_id).await?;
    if view.form_id != form_id {
        return Err(ApiError::BadRequest("view_id does not belong to form".to_string()));
    }
    if view.archived_at.is_some() {
        return Err(ApiError::BadRequest("view is archived".to_string()));
    }
    Ok(())
}

async fn record_query_config_from_view(
    state: &AppState,
    form_id: Uuid,
    view_id: Option<Uuid>,
) -> Result<RecordQueryConfig, ApiError> {
    let Some(view_id) = view_id else {
        return Ok(RecordQueryConfig::default());
    };
    let view = find_view(state, view_id).await?;
    if view.form_id != form_id {
        return Err(ApiError::BadRequest("view_id does not belong to form".to_string()));
    }
    if view.archived_at.is_some() {
        return Err(ApiError::BadRequest("view is archived".to_string()));
    }

    let mut config = RecordQueryConfig {
        filter_expression: record_filter_expression_from_config(&view.config),
        sort: None,
    };
    if let Some(sort) = view.config.get("sort").and_then(Value::as_object) {
        if let Some(field) = sort.get("field").and_then(Value::as_str).map(str::trim) {
            if !field.is_empty() {
                let direction = match sort.get("direction").and_then(Value::as_str) {
                    Some("desc") => "desc",
                    _ => "asc",
                };
                config.sort = Some(format!("{field}:{direction}"));
            }
        }
    }
    Ok(config)
}

async fn visible_form_record_ids_for_export(
    state: &AppState,
    form: &FormResponse,
    fields: &[FormField],
    actor_id: Uuid,
    role: &str,
    view_id: Option<Uuid>,
) -> Result<Vec<Uuid>, ApiError> {
    let view_query = record_query_config_from_view(state, form.id, view_id).await?;
    let mut values: Vec<sea_orm::Value> = vec![form.id.into()];
    let mut joins = Vec::new();
    let mut where_parts = vec![
        "form_records.form_id = $1".to_string(),
        "form_records.archived_at IS NULL".to_string(),
    ];
    let mut next_idx = 2;
    append_record_scope_sql(
        state,
        form.id,
        role,
        actor_id,
        &mut where_parts,
        &mut values,
        &mut next_idx,
    )
    .await?;
    append_record_filter_expression_sql(
        fields,
        view_query.filter_expression.as_ref(),
        &mut where_parts,
        &mut values,
        &mut next_idx,
    )?;
    let (sort_join, order_by, sort_value) = record_sort_sql(view_query.sort.as_deref(), fields, next_idx)?;
    if let Some(join) = sort_join {
        joins.push(join);
        if let Some(value) = sort_value {
            values.push(value.into());
        }
    }
    values.push(5000_i64.into());
    let limit_idx = values.len();
    let join_sql = if joins.is_empty() {
        String::new()
    } else {
        format!(" {}", joins.join(" "))
    };
    let where_sql = where_parts.join(" AND ");
    let rows = RecordIdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &format!(
            r"
                SELECT form_records.id
                FROM form_records
                {join_sql}
                WHERE {where_sql}
                ORDER BY {order_by}
                LIMIT ${limit_idx}
            "
        ),
        values,
    ))
    .all(&state.db)
    .await?;
    Ok(rows.into_iter().map(|row| row.id).collect())
}

async fn form_attachments_for_package(
    state: &AppState,
    form_id: Uuid,
    record_ids: Option<&[Uuid]>,
    denied_read_fields: &BTreeSet<String>,
) -> Result<Vec<FormAttachmentResponse>, ApiError> {
    if record_ids.is_some_and(<[Uuid]>::is_empty) {
        return Ok(Vec::new());
    }

    let mut values: Vec<sea_orm::Value> = vec![form_id.into()];
    let mut where_parts = vec!["form_id = $1".to_string(), "archived_at IS NULL".to_string()];
    let mut idx = 2;
    if let Some(record_ids) = record_ids {
        let placeholders = record_ids
            .iter()
            .map(|record_id| {
                let placeholder = format!("${idx}");
                values.push((*record_id).into());
                idx += 1;
                placeholder
            })
            .collect::<Vec<_>>()
            .join(", ");
        where_parts.push(format!("record_id IN ({placeholders})"));
    }
    if !denied_read_fields.is_empty() {
        let placeholders = denied_read_fields
            .iter()
            .map(|field_key| {
                let placeholder = format!("${idx}");
                values.push(field_key.clone().into());
                idx += 1;
                placeholder
            })
            .collect::<Vec<_>>()
            .join(", ");
        where_parts.push(format!("field_key NOT IN ({placeholders})"));
    }
    values.push(5000_i64.into());
    let limit_idx = values.len();
    let where_sql = where_parts.join(" AND ");

    FormAttachmentResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &format!(
            r"
                SELECT id, workspace_id, project_id, form_id, record_id, field_id, field_key,
                       file_name, content_type, byte_size, storage_key, url, thumbnail_url,
                       COALESCE(media_metadata, '{{}}'::jsonb) AS media_metadata, created_by,
                       archived_at, created_at, updated_at
                FROM form_attachments
                WHERE {where_sql}
                ORDER BY created_at DESC
                LIMIT ${limit_idx}
            "
        ),
        values,
    ))
    .all(&state.db)
    .await
    .map_err(ApiError::from)
}

async fn attachment_package_entries(
    form: &FormResponse,
    view_id: Option<Uuid>,
    attachments: &[FormAttachmentResponse],
) -> Result<(Vec<ZipEntry>, usize), ApiError> {
    let mut entries = Vec::new();
    let mut manifest_attachments = Vec::new();
    let mut binary_file_count = 0;
    let object_storage = ObjectStorage::from_runtime_config()?;

    for attachment in attachments {
        let safe_name = sanitize_zip_file_name(&attachment.file_name);
        let file_path = format!("files/{}-{safe_name}", attachment.id);
        let link_path = format!("links/{}.url.txt", attachment.id);
        let mut package_path = None;
        let package_status;
        if let Some(upload_file_name) = server_owned_upload_file_name(&attachment.url) {
            match object_storage.get(upload_file_name).await {
                Ok(data) => {
                    entries.push(ZipEntry {
                        path: file_path.clone(),
                        data,
                    });
                    package_path = Some(file_path);
                    package_status = "included";
                    binary_file_count += 1;
                }
                Err(_) => {
                    entries.push(ZipEntry {
                        path: link_path.clone(),
                        data: format!("{}\n", attachment.url).into_bytes(),
                    });
                    package_path = Some(link_path);
                    package_status = "missing_server_file";
                }
            }
        } else if attachment.url.trim().is_empty() {
            package_status = "metadata_only";
        } else {
            entries.push(ZipEntry {
                path: link_path.clone(),
                data: format!("{}\n", attachment.url).into_bytes(),
            });
            package_path = Some(link_path);
            package_status = "external_url";
        }

        manifest_attachments.push(json!({
            "id": attachment.id,
            "record_id": attachment.record_id,
            "field_key": attachment.field_key,
            "file_name": attachment.file_name,
            "content_type": attachment.content_type,
            "byte_size": attachment.byte_size,
            "storage_key": attachment.storage_key,
            "url": attachment.url,
            "thumbnail_url": attachment.thumbnail_url,
            "media_metadata": attachment.media_metadata,
            "package_path": package_path,
            "package_status": package_status,
            "created_at": attachment.created_at,
        }));
    }

    let manifest = json!({
        "bundle_format": "openpr.form.attachments.package.v1",
        "form_id": form.id,
        "form_key": form.key,
        "view_id": view_id,
        "exported_at": Utc::now(),
        "attachment_count": attachments.len(),
        "binary_file_count": binary_file_count,
        "attachments": manifest_attachments,
    });
    entries.insert(
        0,
        ZipEntry {
            path: "manifest.json".to_string(),
            data: serde_json::to_vec_pretty(&manifest).map_err(|_| ApiError::Internal)?,
        },
    );
    Ok((entries, binary_file_count))
}

struct AttachmentPackageArtifact {
    file_name: String,
    zip: Vec<u8>,
    attachment_count: usize,
    binary_file_count: usize,
}

async fn write_attachment_package_artifact(
    job_id: Uuid,
    package: AttachmentPackageArtifact,
) -> Result<Value, ApiError> {
    let object_storage = ObjectStorage::from_runtime_config()?;
    let stored_file_name = format!("{}-{}", Uuid::new_v4(), package.file_name);
    let artifact_storage_key = attachment_package_artifact_key(&stored_file_name);
    let byte_size = package.zip.len();
    let artifact_storage = object_storage.put(&artifact_storage_key, &package.zip).await?;
    let generated_at = Utc::now();
    let expires_at = generated_at + Duration::hours(ATTACHMENT_PACKAGE_JOB_RETENTION_HOURS);
    Ok(json!({
        "file_name": package.file_name,
        "stored_file_name": stored_file_name,
        "artifact_storage_key": artifact_storage_key,
        "artifact_storage": artifact_storage,
        "download_url": format!("/api/v1/form-attachment-package-jobs/{job_id}/download"),
        "content_type": "application/zip",
        "byte_size": byte_size,
        "attachment_count": package.attachment_count,
        "binary_file_count": package.binary_file_count,
        "generated_at": generated_at,
        "expires_at": expires_at,
    }))
}

/// Result keys that are safe to expose in a job listing. Everything else (exported rows, rendered
/// CSV, imported records, per row previews) stays out of list responses.
const JOB_RESULT_SUMMARY_KEYS: [&str; 16] = [
    "form_id",
    "format",
    "file_name",
    "export_policy",
    "schema_version",
    "total_rows",
    "created_count",
    "invalid_rows",
    "attachment_count",
    "binary_file_count",
    "byte_size",
    "content_type",
    "download_url",
    "expires_at",
    "stored_file_name",
    "artifact_storage",
];

/// A record link may only point at a record inside the same tenant boundary as its source record.
/// Cross tenant targets are reported as missing so the endpoint cannot be used to probe foreign ids.
fn ensure_link_target_scope(
    source_workspace_id: Uuid,
    source_project_id: Uuid,
    target_workspace_id: Uuid,
    target_project_id: Uuid,
) -> Result<(), ApiError> {
    if source_workspace_id == target_workspace_id && source_project_id == target_project_id {
        return Ok(());
    }
    Err(ApiError::NotFound("link target record not found".to_string()))
}

/// Async job results are computed with the creating actor's field and record permissions, so they may
/// only be read back by that actor (or by a form administrator).
fn ensure_can_read_job_result(
    created_by: Option<Uuid>,
    actor_id: Uuid,
    role: &str,
    is_bot: bool,
) -> Result<(), ApiError> {
    if is_bot || role_is_form_admin(role) || created_by == Some(actor_id) {
        return Ok(());
    }
    Err(ApiError::NotFound("form job not found".to_string()))
}

/// Reduces a job result to its metadata so that listing jobs never ships bulk record data.
fn summarize_job_result(result: Option<Value>) -> Option<Value> {
    let result = result?;
    let Some(entries) = result.as_object() else {
        return Some(json!({ "result_omitted": true }));
    };
    let mut summary = serde_json::Map::new();
    let mut omitted = false;
    for (key, value) in entries {
        if JOB_RESULT_SUMMARY_KEYS.contains(&key.as_str()) {
            summary.insert(key.clone(), value.clone());
        } else {
            omitted = true;
        }
    }
    summary.insert("result_omitted".to_string(), json!(omitted));
    Some(Value::Object(summary))
}

/// Restricts a job listing to the jobs the actor created, unless the actor administers the form.
fn job_listing_owner_filter(actor_id: Uuid, role: &str, is_bot: bool, values: &mut Vec<sea_orm::Value>) -> String {
    if is_bot || role_is_form_admin(role) {
        return String::new();
    }
    values.push(actor_id.into());
    format!(" AND created_by = ${}", values.len())
}

fn ensure_can_manage_import_mapping_template(
    template: &FormImportMappingTemplateResponse,
    actor_id: Uuid,
    role: &str,
    is_bot: bool,
) -> Result<(), ApiError> {
    if is_bot || role_is_form_admin(role) || template.created_by == Some(actor_id) {
        return Ok(());
    }
    Err(ApiError::Forbidden("cannot manage import mapping template".to_string()))
}

async fn insert_export_job(
    state: &AppState,
    form: &FormResponse,
    actor_id: Uuid,
    is_bot: bool,
    input: Value,
) -> Result<Uuid, ApiError> {
    let job_id = Uuid::new_v4();
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO form_export_jobs (
                    id, workspace_id, project_id, form_id, status, input,
                    created_by, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, 'queued', $5, $6, now(), now())
            ",
            vec![
                job_id.into(),
                form.workspace_id.into(),
                form.project_id.into(),
                form.id.into(),
                input.into(),
                if is_bot { None::<Uuid> } else { Some(actor_id) }.into(),
            ],
        ))
        .await?;
    Ok(job_id)
}

async fn claim_export_job_running(state: &AppState, job_id: Uuid) -> Result<bool, ApiError> {
    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE form_export_jobs
                SET status = 'running', started_at = COALESCE(started_at, now()),
                    updated_at = now()
                WHERE id = $1
                  AND status = 'queued'
            ",
            vec![job_id.into()],
        ))
        .await?;
    Ok(result.rows_affected() > 0)
}

async fn update_export_job_completed(state: &AppState, job_id: Uuid, result: Value) -> Result<(), ApiError> {
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE form_export_jobs
                SET status = 'completed', result = $1, error = NULL,
                    completed_at = now(), updated_at = now()
                WHERE id = $2
            ",
            vec![result.into(), job_id.into()],
        ))
        .await?;
    Ok(())
}

async fn update_export_job_failed(state: &AppState, job_id: Uuid, error: String) -> Result<(), ApiError> {
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE form_export_jobs
                SET status = 'failed', error = $1, completed_at = now(), updated_at = now()
                WHERE id = $2
            ",
            vec![error.into(), job_id.into()],
        ))
        .await?;
    Ok(())
}

async fn insert_attachment_package_job(
    state: &AppState,
    form: &FormResponse,
    actor_id: Uuid,
    is_bot: bool,
    input: Value,
) -> Result<Uuid, ApiError> {
    let job_id = Uuid::new_v4();
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO form_attachment_package_jobs (
                    id, workspace_id, project_id, form_id, status, input,
                    created_by, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, 'queued', $5, $6, now(), now())
            ",
            vec![
                job_id.into(),
                form.workspace_id.into(),
                form.project_id.into(),
                form.id.into(),
                input.into(),
                if is_bot { None::<Uuid> } else { Some(actor_id) }.into(),
            ],
        ))
        .await?;
    Ok(job_id)
}

async fn claim_attachment_package_job_running(state: &AppState, job_id: Uuid) -> Result<bool, ApiError> {
    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE form_attachment_package_jobs
                SET status = 'running', started_at = COALESCE(started_at, now()),
                    updated_at = now()
                WHERE id = $1
                  AND status = 'queued'
            ",
            vec![job_id.into()],
        ))
        .await?;
    Ok(result.rows_affected() > 0)
}

async fn update_attachment_package_job_completed(
    state: &AppState,
    job_id: Uuid,
    result: Value,
) -> Result<(), ApiError> {
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE form_attachment_package_jobs
                SET status = 'completed', result = $1, error = NULL,
                    completed_at = now(), updated_at = now()
                WHERE id = $2
            ",
            vec![result.into(), job_id.into()],
        ))
        .await?;
    Ok(())
}

async fn update_attachment_package_job_result(state: &AppState, job_id: Uuid, result: Value) -> Result<(), ApiError> {
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE form_attachment_package_jobs
                SET result = $1, updated_at = now()
                WHERE id = $2
                  AND status = 'completed'
                  AND NOT (COALESCE(result, '{}'::jsonb) ? 'artifact_deleted_at')
            ",
            vec![result.into(), job_id.into()],
        ))
        .await?;
    Ok(())
}

async fn update_attachment_package_job_failed(state: &AppState, job_id: Uuid, error: String) -> Result<(), ApiError> {
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE form_attachment_package_jobs
                SET status = 'failed', error = $1, completed_at = now(), updated_at = now()
                WHERE id = $2
            ",
            vec![error.into(), job_id.into()],
        ))
        .await?;
    Ok(())
}

async fn import_records_request_from_job(
    state: &AppState,
    form: &FormResponse,
    req: CreateImportJobRequest,
    actor_id: Uuid,
    role: &str,
    is_bot: bool,
) -> Result<ImportRecordsRequest, ApiError> {
    if let Some(rows) = req.rows {
        if rows.is_empty() {
            return Err(ApiError::BadRequest("import rows are required".to_string()));
        }
        return Ok(ImportRecordsRequest {
            rows,
            idempotency_key: req.idempotency_key,
        });
    }
    import_records_request_from_uploaded_file(
        state,
        form,
        ImportRecordsFileRequest {
            file_url: req.file_url,
            upload_url: req.upload_url,
            mapping_template_id: req.mapping_template_id,
            idempotency_key: req.idempotency_key,
        },
        actor_id,
        role,
        is_bot,
    )
    .await
}

async fn insert_import_job(
    state: &AppState,
    form: &FormResponse,
    actor_id: Uuid,
    is_bot: bool,
    input: Value,
) -> Result<Uuid, ApiError> {
    let job_id = Uuid::new_v4();
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO form_import_jobs (
                    id, workspace_id, project_id, form_id, status, input,
                    created_by, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, 'queued', $5, $6, now(), now())
            ",
            vec![
                job_id.into(),
                form.workspace_id.into(),
                form.project_id.into(),
                form.id.into(),
                input.into(),
                if is_bot { None::<Uuid> } else { Some(actor_id) }.into(),
            ],
        ))
        .await?;
    Ok(job_id)
}

async fn claim_import_job_running(state: &AppState, job_id: Uuid) -> Result<bool, ApiError> {
    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE form_import_jobs
                SET status = 'running', started_at = COALESCE(started_at, now()),
                    updated_at = now()
                WHERE id = $1
                  AND status = 'queued'
            ",
            vec![job_id.into()],
        ))
        .await?;
    Ok(result.rows_affected() > 0)
}

async fn update_import_job_completed(state: &AppState, job_id: Uuid, result: Value) -> Result<(), ApiError> {
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE form_import_jobs
                SET status = 'completed', result = $1, error = NULL,
                    completed_at = now(), updated_at = now()
                WHERE id = $2
            ",
            vec![result.into(), job_id.into()],
        ))
        .await?;
    Ok(())
}

async fn update_import_job_failed(state: &AppState, job_id: Uuid, error: String) -> Result<(), ApiError> {
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE form_import_jobs
                SET status = 'failed', error = $1, completed_at = now(), updated_at = now()
                WHERE id = $2
            ",
            vec![error.into(), job_id.into()],
        ))
        .await?;
    Ok(())
}

async fn import_records_request_from_uploaded_file(
    state: &AppState,
    form: &FormResponse,
    req: ImportRecordsFileRequest,
    actor_id: Uuid,
    role: &str,
    is_bot: bool,
) -> Result<ImportRecordsRequest, ApiError> {
    if req.mapping_template_id.is_none() {
        return import_records_request_from_uploaded_file_without_mapping(form, req).await;
    }
    let file_url = req
        .file_url
        .or(req.upload_url)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::BadRequest("file_url is required".to_string()))?;
    let file_name = server_owned_upload_file_name(&file_url)
        .ok_or_else(|| ApiError::BadRequest("file_url must reference a server-owned upload".to_string()))?;
    let data = ObjectStorage::from_runtime_config()?
        .get(file_name)
        .await
        .map_err(|_| ApiError::NotFound("import file not found".to_string()))?;
    let mapping = req.mapping_template_id.map(|template_id| async move {
        import_mapping_template_config_for_import(state, form, template_id, actor_id, role, is_bot).await
    });
    let mapping = if let Some(mapping) = mapping {
        Some(mapping.await?)
    } else {
        None
    };
    import_records_request_from_file_data(form, file_url, data, mapping.as_ref(), req.idempotency_key)
}

async fn import_records_request_from_uploaded_file_without_mapping(
    form: &FormResponse,
    req: ImportRecordsFileRequest,
) -> Result<ImportRecordsRequest, ApiError> {
    let file_url = req
        .file_url
        .or(req.upload_url)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::BadRequest("file_url is required".to_string()))?;
    let file_name = server_owned_upload_file_name(&file_url)
        .ok_or_else(|| ApiError::BadRequest("file_url must reference a server-owned upload".to_string()))?;
    let data = ObjectStorage::from_runtime_config()?
        .get(file_name)
        .await
        .map_err(|_| ApiError::NotFound("import file not found".to_string()))?;
    import_records_request_from_file_data(form, file_url, data, None, req.idempotency_key)
}

fn import_records_request_from_file_data(
    form: &FormResponse,
    file_url: String,
    data: Vec<u8>,
    mapping: Option<&ImportMappingTemplateConfig>,
    idempotency_key: Option<String>,
) -> Result<ImportRecordsRequest, ApiError> {
    if data.len() > MAX_IMPORT_FILE_BYTES {
        return Err(ApiError::BadRequest("import file must be 2MB or smaller".to_string()));
    }
    let text = String::from_utf8(data)
        .map_err(|_| ApiError::BadRequest("import file must be valid UTF-8 text".to_string()))?;
    let fields = parse_fields(&form.schema).map_err(ApiError::BadRequest)?;
    let rows = import_rows_from_file_text(&fields, &text, &file_url, mapping)?;
    Ok(ImportRecordsRequest { rows, idempotency_key })
}

async fn import_mapping_template_config_for_import(
    state: &AppState,
    form: &FormResponse,
    template_id: Uuid,
    actor_id: Uuid,
    role: &str,
    is_bot: bool,
) -> Result<ImportMappingTemplateConfig, ApiError> {
    let template = find_import_mapping_template(state, template_id).await?;
    if template.archived_at.is_some() {
        return Err(ApiError::NotFound("import mapping template not found".to_string()));
    }
    if template.form_id != form.id {
        return Err(ApiError::BadRequest(
            "mapping_template_id does not belong to form".to_string(),
        ));
    }
    if !template.shared && !is_bot && !role_is_form_admin(role) && template.created_by != Some(actor_id) {
        return Err(ApiError::Forbidden(
            "cannot use private import mapping template".to_string(),
        ));
    }
    Ok(ImportMappingTemplateConfig {
        header_signature: template.header_signature,
        headers: json_string_array(&template.headers, "headers")?,
        selections: json_string_array(&template.selections, "selections")?,
        transforms: json_string_array(&template.transforms, "transforms")?,
    })
}

async fn preview_import_rows(
    state: &AppState,
    form: &FormResponse,
    actor_id: Uuid,
    role: &str,
    is_bot: bool,
    rows: &[ImportRecordInput],
) -> Result<Vec<ImportRecordPreviewRow>, ApiError> {
    if rows.is_empty() {
        return Err(ApiError::BadRequest("import rows are required".to_string()));
    }
    if rows.len() > 500 {
        return Err(ApiError::BadRequest(
            "import rows cannot exceed 500 per request".to_string(),
        ));
    }

    let denied_read_fields = denied_read_field_keys(state, form.id, role).await?;
    let mut previews = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let row_number = row.row_number.unwrap_or(index + 1);
        let preview = match preview_import_row(state, form, actor_id, role, is_bot, row).await {
            Ok((title, values)) => ImportRecordPreviewRow {
                row_number,
                valid: true,
                title: Some(title),
                normalized_values: Some(filter_values_for_read_policy(values, &denied_read_fields)),
                error: None,
            },
            Err(error) => ImportRecordPreviewRow {
                row_number,
                valid: false,
                title: row.title.clone(),
                normalized_values: None,
                error: Some(error),
            },
        };
        previews.push(preview);
    }
    Ok(previews)
}

async fn preview_import_row(
    state: &AppState,
    form: &FormResponse,
    actor_id: Uuid,
    role: &str,
    is_bot: bool,
    row: &ImportRecordInput,
) -> Result<(String, Value), String> {
    ensure_field_write_policy_allows(state, form.id, role, &row.values)
        .await
        .map_err(|error| error.to_string())?;
    let source = ensure_json_object(
        row.source.clone().unwrap_or_else(
            || json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id, "origin": "import_preview" }),
        ),
        "source",
    )
    .map_err(|error| error.to_string())?;
    let values_with_formula = run_formula_hooks(
        state,
        form.workspace_id,
        form.project_id,
        form.id,
        &form.key,
        row.values.clone(),
    )
    .await
    .map_err(|error| error.to_string())?;
    let calculated = calculate_and_normalize_values(state, form, None, values_with_formula)
        .await
        .map_err(|error| error.to_string())?;
    run_field_validator_hooks(
        state,
        form.workspace_id,
        form.project_id,
        form.id,
        &form.key,
        &form.schema,
        &calculated,
    )
    .await
    .map_err(|error| error.to_string())?;
    let preview_id = Uuid::new_v4();
    let title = row
        .title
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| render_title(&form.title_template, preview_id, &calculated));
    let _ = source;
    Ok((title, calculated))
}

fn import_preview_response(form: &FormResponse, rows: Vec<ImportRecordPreviewRow>) -> ImportRecordsPreviewResponse {
    let valid_rows = rows.iter().filter(|row| row.valid).count();
    let total_rows = rows.len();
    ImportRecordsPreviewResponse {
        form_id: form.id,
        schema_version: form.schema_version,
        total_rows,
        valid_rows,
        invalid_rows: total_rows.saturating_sub(valid_rows),
        rows,
    }
}

async fn require_form_action(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&BotAuthContext>,
    form: &FormResponse,
    action: &str,
) -> Result<(Uuid, String, bool), ApiError> {
    validate_permission_action(action)?;
    let (actor_id, role, is_bot) = require_workspace_access_from_auth(state, claims, bot, form.workspace_id).await?;
    if !form_action_allowed(state, form.id, &role, action).await? {
        return Err(ApiError::Forbidden(format!("form permission denied: {action}")));
    }
    Ok((actor_id, role, is_bot))
}

fn ensure_form_permission_manager(role: &str) -> Result<(), ApiError> {
    if role_is_form_admin(role) {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "only workspace owners and admins can manage form permissions".to_string(),
        ))
    }
}

fn filter_record_response_values(mut record: RecordResponse, denied_read_fields: &BTreeSet<String>) -> RecordResponse {
    record.values = filter_values_for_read_policy(record.values, denied_read_fields);
    record
}

/// Applies field level read permissions to an event before it leaves the API.
///
/// The payload is pruned to what its event type declares in `forms::event_redaction`; source and
/// metadata are pruned to the keys that are generated server side. Anything undeclared is withheld
/// because event payloads carry field values in shapes a key name list cannot enumerate (rendered
/// titles, bare values at the top level, signature audit trails).
fn filter_event_response_values(
    mut event: BusinessEventResponse,
    denied_read_fields: &BTreeSet<String>,
) -> BusinessEventResponse {
    if denied_read_fields.is_empty() {
        return event;
    }
    event.payload = redact_event_payload(&event.event_type, event.payload, denied_read_fields);
    event.metadata = redact_event_metadata(event.metadata, denied_read_fields);
    event.source = redact_event_source(event.source, denied_read_fields);
    event
}

fn filter_relation_target_values(
    mut target: RelationTargetResponse,
    denied_read_fields: &BTreeSet<String>,
) -> RelationTargetResponse {
    if denied_read_fields.is_empty() {
        return target;
    }
    if let Some(values) = target.values.as_object_mut() {
        for field_key in denied_read_fields {
            values.remove(field_key);
        }
    }
    target
}

async fn ensure_project_actor(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&BotAuthContext>,
    project_id: Uuid,
) -> Result<(Uuid, Option<Uuid>), ApiError> {
    let project = find_project_workspace(state, project_id).await?;
    let (actor_id, _, is_bot) = require_workspace_access_from_auth(state, claims, bot, project.workspace_id).await?;
    Ok((project.workspace_id, if is_bot { None } else { Some(actor_id) }))
}

async fn find_project_workspace(state: &AppState, project_id: Uuid) -> Result<ProjectWorkspace, ApiError> {
    ProjectWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))
}

async fn find_form(state: &AppState, form_id: Uuid) -> Result<FormResponse, ApiError> {
    find_form_with_conn(&state.db, form_id).await
}

async fn find_form_with_conn<C>(db: &C, form_id: Uuid) -> Result<FormResponse, ApiError>
where
    C: ConnectionTrait,
{
    FormResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, key, name, description, icon, color,
                   title_template, schema, detail_layout, schema_version, created_by, archived_at, created_at, updated_at
            FROM project_forms
            WHERE id = $1
        ",
        vec![form_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("form not found".to_string()))
}

async fn find_record(state: &AppState, record_id: Uuid) -> Result<RecordResponse, ApiError> {
    RecordResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, form_id, title, values, source,
                   schema_version, created_by, updated_by, archived_at, created_at, updated_at
            FROM form_records
            WHERE id = $1
        ",
        vec![record_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("form record not found".to_string()))
}

async fn find_view(state: &AppState, view_id: Uuid) -> Result<ViewResponse, ApiError> {
    find_view_with_conn(&state.db, view_id).await
}

async fn find_view_with_conn<C>(db: &C, view_id: Uuid) -> Result<ViewResponse, ApiError>
where
    C: ConnectionTrait,
{
    ViewResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, form_id, key, name, view_type,
                   config, created_by, archived_at, created_at, updated_at
            FROM form_views
            WHERE id = $1
        ",
        vec![view_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("form view not found".to_string()))
}

async fn find_attachment(state: &AppState, attachment_id: Uuid) -> Result<FormAttachmentResponse, ApiError> {
    FormAttachmentResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, form_id, record_id, field_id, field_key,
                   file_name, content_type, byte_size, storage_key, url, thumbnail_url,
                   COALESCE(media_metadata, '{}'::jsonb) AS media_metadata, created_by,
                   archived_at, created_at, updated_at
            FROM form_attachments
            WHERE id = $1
        ",
        vec![attachment_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("form attachment not found".to_string()))
}

async fn find_import_mapping_template(
    state: &AppState,
    template_id: Uuid,
) -> Result<FormImportMappingTemplateResponse, ApiError> {
    find_import_mapping_template_with_conn(&state.db, template_id).await
}

async fn find_import_mapping_template_with_conn<C>(
    db: &C,
    template_id: Uuid,
) -> Result<FormImportMappingTemplateResponse, ApiError>
where
    C: ConnectionTrait,
{
    FormImportMappingTemplateResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, form_id, name, header_signature,
                   headers, selections, transforms, shared, created_by,
                   archived_at, created_at, updated_at
            FROM form_import_mapping_templates
            WHERE id = $1
        ",
        vec![template_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("import mapping template not found".to_string()))
}

async fn find_import_job(state: &AppState, job_id: Uuid) -> Result<FormImportJobResponse, ApiError> {
    FormImportJobResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, form_id, status, input, result, error,
                   created_by, started_at, completed_at, created_at, updated_at
            FROM form_import_jobs
            WHERE id = $1
        ",
        vec![job_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("import job not found".to_string()))
}

async fn find_export_job(state: &AppState, job_id: Uuid) -> Result<FormExportJobResponse, ApiError> {
    FormExportJobResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, form_id, status, input, result, error,
                   created_by, started_at, completed_at, created_at, updated_at
            FROM form_export_jobs
            WHERE id = $1
        ",
        vec![job_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("export job not found".to_string()))
}

async fn find_attachment_package_job(
    state: &AppState,
    job_id: Uuid,
) -> Result<FormAttachmentPackageJobResponse, ApiError> {
    FormAttachmentPackageJobResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, form_id, status, input, result, error,
                   created_by, started_at, completed_at, created_at, updated_at
            FROM form_attachment_package_jobs
            WHERE id = $1
        ",
        vec![job_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("attachment package job not found".to_string()))
}

type HmacSha256 = Hmac<Sha256>;

fn sign_attachment_download(secret: &str, attachment_id: Uuid, expires: i64) -> Result<String, ApiError> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| ApiError::Internal)?;
    mac.update(format!("{attachment_id}:{expires}").as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right.iter())
        .fold(0_u8, |diff, (left, right)| diff | (left ^ right))
        == 0
}

fn normalize_optional_idempotency_key(value: Option<String>) -> Result<Option<String>, ApiError> {
    match value {
        Some(value) => {
            let value = value.trim().to_string();
            if value.is_empty() {
                return Err(ApiError::BadRequest("idempotency_key cannot be empty".to_string()));
            }
            if value.len() > 255 {
                return Err(ApiError::BadRequest(
                    "idempotency_key cannot exceed 255 characters".to_string(),
                ));
            }
            Ok(Some(value))
        }
        None => Ok(None),
    }
}

async fn find_idempotent_record(
    state: &AppState,
    form: &FormResponse,
    idempotency_key: Option<&str>,
    expected_event_type: &str,
) -> Result<Option<RecordResponse>, ApiError> {
    let Some(idempotency_key) = idempotency_key else {
        return Ok(None);
    };
    let receipt = IdempotencyReceiptRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT event_type, aggregate_id
            FROM business_events
            WHERE workspace_id = $1 AND idempotency_key = $2
            ORDER BY created_at ASC
            LIMIT 1
        ",
        vec![form.workspace_id.into(), idempotency_key.to_string().into()],
    ))
    .one(&state.db)
    .await?;
    let Some(receipt) = receipt else {
        return Ok(None);
    };
    if receipt.event_type != expected_event_type {
        return Err(ApiError::Conflict(format!(
            "idempotency key already used for {}",
            receipt.event_type
        )));
    }
    let record_id = Uuid::from_str(&receipt.aggregate_id)
        .map_err(|_| ApiError::Conflict("idempotency receipt does not reference a form record".to_string()))?;
    let record = find_record(state, record_id).await?;
    if record.form_id != form.id {
        return Err(ApiError::Conflict(
            "idempotency key already used for another form".to_string(),
        ));
    }
    Ok(Some(record))
}

#[derive(Debug, Clone, Copy)]
enum EventScope {
    Form {
        project_id: Uuid,
        form_id: Uuid,
        /// Set when the actor's record scope is `owned`: form level events stay visible, record
        /// events are restricted to records the actor created.
        owned_by: Option<Uuid>,
    },
    Record {
        project_id: Uuid,
        record_id: Uuid,
    },
}

/// Restricts a form scoped event listing to records the actor created. Events that carry no record
/// (form and view lifecycle events) stay visible because they expose no record data.
const OWNED_RECORD_EVENT_SQL: &str = r"
    (metadata->>'record_id' IS NULL
     OR EXISTS (
        SELECT 1
          FROM form_records owned_record
         WHERE owned_record.form_id = $FORM_ID
           AND owned_record.created_by = $ACTOR_ID
           AND owned_record.id::text = metadata->>'record_id'
     ))
";

/// Binds `OWNED_RECORD_EVENT_SQL` to concrete placeholder positions. Only code generated
/// placeholder numbers are interpolated; the form id and actor id stay bound values.
fn owned_record_event_predicate(form_id_idx: usize, actor_id_idx: usize) -> String {
    OWNED_RECORD_EVENT_SQL
        .replace("$FORM_ID", &format!("${form_id_idx}"))
        .replace("$ACTOR_ID", &format!("${actor_id_idx}"))
}

/// Translate an event listing scope into bound where clauses.
///
/// A form scope with `owned_by` set also restricts record events to the actor's own records, which
/// the record scoped listing enforces through `ensure_record_scope_allows_created_by`.
fn push_event_scope_filters(
    scope: EventScope,
    where_parts: &mut Vec<String>,
    values: &mut Vec<sea_orm::Value>,
    idx: &mut usize,
) {
    match scope {
        EventScope::Form {
            project_id,
            form_id,
            owned_by,
        } => {
            where_parts.push(format!("project_id = ${idx}"));
            values.push(project_id.into());
            *idx += 1;
            where_parts.push(format!("metadata->>'form_id' = ${idx}"));
            values.push(form_id.to_string().into());
            *idx += 1;
            if let Some(actor_id) = owned_by {
                where_parts.push(owned_record_event_predicate(*idx, *idx + 1));
                values.push(form_id.into());
                values.push(actor_id.into());
                *idx += 2;
            }
        }
        EventScope::Record { project_id, record_id } => {
            where_parts.push(format!("project_id = ${idx}"));
            values.push(project_id.into());
            *idx += 1;
            where_parts.push(format!("aggregate_id = ${idx}"));
            values.push(record_id.to_string().into());
            *idx += 1;
        }
    }
}

async fn list_business_events(
    state: &AppState,
    scope: EventScope,
    query: ListEventsQuery,
    denied_read_fields: &BTreeSet<String>,
) -> Result<Json<ApiResponse<PaginatedData<BusinessEventResponse>>>, ApiError> {
    if let Some(event_type) = query.event_type.as_deref() {
        normalize_event_type(event_type)?;
    }
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(50).clamp(1, 200);
    let offset = (page - 1) * per_page;
    let mut where_parts = Vec::new();
    let mut values = Vec::new();
    let mut idx = 1;

    push_event_scope_filters(scope, &mut where_parts, &mut values, &mut idx);

    if let Some(event_type) = query.event_type {
        where_parts.push(format!("event_type = ${idx}"));
        values.push(normalize_event_type(&event_type)?.into());
        idx += 1;
    }

    let where_sql = format!("WHERE {}", where_parts.join(" AND "));
    let total = count_query(
        state,
        &format!("SELECT COUNT(*)::bigint AS count FROM business_events {where_sql}"),
        values.clone(),
    )
    .await?;

    values.push(per_page.into());
    values.push(offset.into());
    let sql = format!(
        r"
            SELECT id, workspace_id, project_id, event_type, aggregate_type, aggregate_id,
                   actor_id, source, payload, metadata, correlation_id, causation_id,
                   idempotency_key, created_at
            FROM business_events
            {where_sql}
            ORDER BY created_at DESC
            LIMIT ${idx} OFFSET ${}
        ",
        idx + 1
    );
    let items =
        BusinessEventResponse::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, &sql, values))
            .all(&state.db)
            .await?;
    let items = items
        .into_iter()
        .map(|event| filter_event_response_values(event, denied_read_fields))
        .collect();

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages: total_pages(total, per_page),
    }))
}

async fn insert_form_event<C>(
    db: &C,
    form: &FormResponse,
    record_id: Option<Uuid>,
    event_type: &str,
    actor_id: Option<Uuid>,
    source: Value,
    payload: Value,
) -> Result<(), ApiError>
where
    C: ConnectionTrait,
{
    insert_form_event_with_idempotency(db, form, record_id, event_type, actor_id, source, payload, None).await
}

async fn insert_form_event_with_idempotency<C>(
    db: &C,
    form: &FormResponse,
    record_id: Option<Uuid>,
    event_type: &str,
    actor_id: Option<Uuid>,
    source: Value,
    payload: Value,
    idempotency_key: Option<String>,
) -> Result<(), ApiError>
where
    C: ConnectionTrait,
{
    let aggregate_id = record_id.unwrap_or(form.id).to_string();
    let aggregate_type = if record_id.is_some() {
        format!("form.{}", form.key)
    } else {
        "form".to_string()
    };

    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO form_events (
                id, workspace_id, project_id, form_id, record_id, event_type,
                actor_id, source, payload, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now())
        ",
        vec![
            Uuid::new_v4().into(),
            form.workspace_id.into(),
            form.project_id.into(),
            form.id.into(),
            record_id.into(),
            event_type.to_string().into(),
            actor_id.into(),
            source.clone().into(),
            payload.clone().into(),
        ],
    ))
    .await?;
    insert_business_event(
        db,
        BusinessEventInput {
            workspace_id: form.workspace_id,
            project_id: Some(form.project_id),
            event_type: event_type.to_string(),
            aggregate_type: aggregate_type.clone(),
            aggregate_id: aggregate_id.clone(),
            actor_id,
            source,
            payload: payload.clone(),
            metadata: json!({
                "form_id": form.id,
                "form_key": form.key,
                "record_id": record_id,
                "legacy_form_events": true
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: idempotency_key.clone(),
        },
    )
    .await?;
    if form.key == "print_job" && event_type == "form.record.created" {
        insert_business_event(
            db,
            BusinessEventInput {
                workspace_id: form.workspace_id,
                project_id: Some(form.project_id),
                event_type: "print_job.created".to_string(),
                aggregate_type,
                aggregate_id,
                actor_id,
                source: json!({
                    "type": "system",
                    "origin_event": event_type
                }),
                payload,
                metadata: json!({
                    "form_id": form.id,
                    "form_key": form.key,
                    "record_id": record_id,
                    "derived_from": event_type
                }),
                correlation_id: None,
                causation_id: None,
                idempotency_key: idempotency_key.map(|key| format!("{key}:print_job.created")),
            },
        )
        .await?;
    }
    Ok(())
}

async fn insert_schema_version<C>(
    db: &C,
    form: &FormResponse,
    changed_by: Option<Uuid>,
    change_summary: &str,
) -> Result<(), ApiError>
where
    C: ConnectionTrait,
{
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO form_schema_versions (
                form_id, version, schema, detail_layout, changed_by, change_summary, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, now())
            ON CONFLICT (form_id, version) DO UPDATE
            SET schema = EXCLUDED.schema,
                detail_layout = EXCLUDED.detail_layout,
                changed_by = EXCLUDED.changed_by,
                change_summary = EXCLUDED.change_summary
        ",
        vec![
            form.id.into(),
            form.schema_version.into(),
            form.schema.clone().into(),
            form.detail_layout.clone().into(),
            changed_by.into(),
            change_summary.to_string().into(),
        ],
    ))
    .await?;
    Ok(())
}

async fn count_query(state: &AppState, sql: &str, values: Vec<sea_orm::Value>) -> Result<i64, ApiError> {
    state
        .db
        .query_one(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await?
        .ok_or_else(|| ApiError::Internal)?
        .try_get::<i64>("", "count")
        .map_err(ApiError::from)
}

async fn duplicate_form_views(
    tx: &DatabaseTransaction,
    source_form: &FormResponse,
    duplicated_form: &FormResponse,
    created_by: Option<Uuid>,
) -> Result<i64, ApiError> {
    let views = ViewResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, form_id, key, name, view_type,
                   config, created_by, archived_at, created_at, updated_at
            FROM form_views
            WHERE form_id = $1
              AND archived_at IS NULL
            ORDER BY created_at ASC
        ",
        vec![source_form.id.into()],
    ))
    .all(tx)
    .await?;
    for view in &views {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO form_views (
                    id, workspace_id, project_id, form_id, key, name, view_type,
                    config, created_by, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now(), now())
            ",
            vec![
                Uuid::new_v4().into(),
                duplicated_form.workspace_id.into(),
                duplicated_form.project_id.into(),
                duplicated_form.id.into(),
                view.key.clone().into(),
                view.name.clone().into(),
                view.view_type.clone().into(),
                view.config.clone().into(),
                created_by.into(),
            ],
        ))
        .await?;
    }
    Ok(views.len() as i64)
}

async fn next_duplicate_form_key(state: &AppState, project_id: Uuid, source_key: &str) -> Result<String, ApiError> {
    let base = normalize_key(&format!("{source_key}_copy")).map_err(ApiError::BadRequest)?;
    if !form_key_exists(state, project_id, &base).await? {
        return Ok(base);
    }
    for suffix in 2..1000 {
        let candidate = format!("{base}_{suffix}");
        if !form_key_exists(state, project_id, &candidate).await? {
            return Ok(candidate);
        }
    }
    Err(ApiError::BadRequest(
        "could not allocate a duplicate form key".to_string(),
    ))
}

async fn ensure_form_key_available(state: &AppState, project_id: Uuid, key: &str) -> Result<(), ApiError> {
    if form_key_exists(state, project_id, key).await? {
        Err(ApiError::BadRequest("form key already exists".to_string()))
    } else {
        Ok(())
    }
}

async fn form_key_exists(state: &AppState, project_id: Uuid, key: &str) -> Result<bool, ApiError> {
    Ok(count_query(
        state,
        "SELECT COUNT(*)::bigint AS count FROM project_forms WHERE project_id = $1 AND key = $2",
        vec![project_id.into(), key.to_string().into()],
    )
    .await?
        > 0)
}

async fn load_form_scenario_template(
    state: &AppState,
    workspace_id: Uuid,
    template_key: &str,
) -> Result<ScenarioFormTemplateRow, ApiError> {
    ScenarioFormTemplateRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT key, name, description, industry, field_schema
            FROM scenario_templates
            WHERE key = $1
              AND (workspace_id IS NULL OR workspace_id = $2)
            ORDER BY (workspace_id IS NULL) ASC
            LIMIT 1
        ",
        vec![template_key.to_string().into(), workspace_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("scenario template not found".to_string()))
}

fn default_form_key_from_template(template_key: &str) -> String {
    template_key
        .trim()
        .strip_suffix("_default")
        .unwrap_or(template_key.trim())
        .to_string()
}

fn normalize_scenario_field_schema(schema: Value) -> Result<Value, ApiError> {
    let mut schema = ensure_json_object(schema, "template field_schema")?;
    let object = schema
        .as_object_mut()
        .ok_or_else(|| ApiError::BadRequest("template field_schema must be a JSON object".to_string()))?;
    object
        .entry("version".to_string())
        .or_insert_with(|| json!("openpr.form.schema.v1"));
    if let Some(fields) = object.get_mut("fields").and_then(Value::as_array_mut) {
        for field in fields {
            if let Some(field_object) = field.as_object_mut()
                && field_object.get("type").and_then(Value::as_str) == Some("select")
            {
                field_object.insert("type".to_string(), json!("single_select"));
            }
        }
    }
    let schema = ensure_schema_field_ids(schema).map_err(ApiError::BadRequest)?;
    validate_schema(&schema).map_err(ApiError::BadRequest)?;
    Ok(schema)
}

fn schema_field_keys(schema: &Value) -> Vec<String> {
    schema
        .get("fields")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field.get("key").and_then(Value::as_str))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn default_title_template_from_schema(schema: &Value) -> String {
    let fields = schema.get("fields").and_then(Value::as_array);
    let title_key = fields
        .and_then(|fields| {
            fields
                .iter()
                .find(|field| field.get("required").and_then(Value::as_bool).unwrap_or(false))
                .and_then(|field| field.get("key").and_then(Value::as_str))
                .or_else(|| fields.iter().find_map(|field| field.get("key").and_then(Value::as_str)))
        })
        .unwrap_or("id");
    format!("{{{title_key}}}")
}

fn default_detail_layout_for_schema(schema: &Value) -> Value {
    let fields = schema_field_keys(schema);
    json!({
        "sections": [
            {
                "key": "main",
                "title": "Main",
                "fields": fields
            }
        ]
    })
}

async fn create_default_template_views(
    tx: &DatabaseTransaction,
    form: &FormResponse,
    schema: &Value,
    created_by: Option<Uuid>,
) -> Result<(), ApiError> {
    let columns = schema_field_keys(schema).into_iter().take(8).collect::<Vec<_>>();
    let views = [
        json!({"key": "grid", "name": "Grid", "view_type": "grid", "config": {"columns": columns.clone()}}),
        json!({"key": "detail", "name": "Detail", "view_type": "detail", "config": {"sections": [{"key": "main", "fields": columns}]}}),
    ];
    for view in views {
        let view_id = Uuid::new_v4();
        let key = view.get("key").and_then(Value::as_str).unwrap_or("grid");
        let name = view.get("name").and_then(Value::as_str).unwrap_or(key);
        let view_type = view.get("view_type").and_then(Value::as_str).unwrap_or("grid");
        let config = view.get("config").cloned().unwrap_or_else(|| json!({}));
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO form_views (
                    id, workspace_id, project_id, form_id, key, name, view_type,
                    config, created_by, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now(), now())
            ",
            vec![
                view_id.into(),
                form.workspace_id.into(),
                form.project_id.into(),
                form.id.into(),
                key.to_string().into(),
                name.to_string().into(),
                view_type.to_string().into(),
                config.into(),
                created_by.into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

fn required_trimmed(value: &str, field: &str) -> Result<String, ApiError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ApiError::BadRequest(format!("{field} is required")));
    }
    Ok(trimmed.to_string())
}

fn ensure_json_object(value: Value, field: &str) -> Result<Value, ApiError> {
    if value.is_object() {
        Ok(value)
    } else {
        Err(ApiError::BadRequest(format!("{field} must be a JSON object")))
    }
}

fn normalize_view_type(raw: &str) -> Result<String, ApiError> {
    match raw.trim() {
        "grid" | "detail" | "kanban" | "calendar" | "card" | "timeline" | "pivot" | "gantt" => {
            Ok(raw.trim().to_string())
        }
        _ => Err(ApiError::BadRequest("unsupported view_type".to_string())),
    }
}

fn config_marks_default(config: &Value) -> bool {
    config.get("is_default").and_then(Value::as_bool).unwrap_or(false)
}

fn normalize_view_config(value: Value) -> Result<Value, ApiError> {
    let mut config = ensure_json_object(value, "config")?;
    let visibility = match config
        .get("visibility")
        .and_then(Value::as_str)
        .unwrap_or("shared")
        .trim()
    {
        "" | "shared" => "shared",
        "private" => "private",
        _ => {
            return Err(ApiError::BadRequest(
                "view config visibility must be shared or private".to_string(),
            ));
        }
    };
    if let Some(object) = config.as_object_mut() {
        object.insert("visibility".to_string(), Value::String(visibility.to_string()));
    }
    Ok(config)
}

fn view_config_is_private(config: &Value) -> bool {
    config
        .get("visibility")
        .and_then(Value::as_str)
        .map(|value| value.trim() == "private")
        .unwrap_or(false)
}

fn ensure_can_manage_view(view: &ViewResponse, actor_id: Uuid, role: &str, is_bot: bool) -> Result<(), ApiError> {
    if !view_config_is_private(&view.config) || is_bot || role_is_form_admin(role) || view.created_by == Some(actor_id)
    {
        return Ok(());
    }
    Err(ApiError::Forbidden(
        "private view can only be changed by its creator or a workspace admin".to_string(),
    ))
}

async fn clear_default_form_views(
    tx: &DatabaseTransaction,
    form_id: Uuid,
    except_view_id: Option<Uuid>,
) -> Result<(), ApiError> {
    let mut sql = r"
        UPDATE form_views
        SET config = jsonb_set(config, '{is_default}', 'false'::jsonb, true),
            updated_at = now()
        WHERE form_id = $1
          AND archived_at IS NULL
    "
    .to_string();
    let mut values = vec![form_id.into()];
    if let Some(view_id) = except_view_id {
        sql.push_str(" AND id <> $2");
        values.push(view_id.into());
    }
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres, &sql, values))
        .await?;
    Ok(())
}

fn normalize_aggregate(raw: &str) -> Result<String, ApiError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "sum" | "avg" | "min" | "max" | "count" => Ok(raw.trim().to_ascii_lowercase()),
        _ => Err(ApiError::BadRequest("unsupported aggregate".to_string())),
    }
}

fn normalize_event_type(raw: &str) -> Result<String, ApiError> {
    let event_type = raw.trim().to_ascii_lowercase();
    if event_type.is_empty() || event_type.len() > 160 {
        return Err(ApiError::BadRequest("invalid event_type".to_string()));
    }
    if !event_type.split('.').all(|part| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            && part.chars().next().is_some_and(|first| first.is_ascii_lowercase())
    }) {
        return Err(ApiError::BadRequest("invalid event_type".to_string()));
    }
    Ok(event_type)
}

fn render_title(template: &str, record_id: Uuid, values: &Value) -> String {
    let mut title = template.replace("{id}", &record_id.to_string());
    if let Some(object) = values.as_object() {
        for (key, value) in object {
            let replacement = display_value(value);
            title = title.replace(&format!("{{{key}}}"), &replacement);
        }
    }
    if title.trim().is_empty() || title == "{id}" {
        record_id.to_string()
    } else {
        title
    }
}

fn display_value(value: &Value) -> String {
    if let Some(value) = value.as_str() {
        return value.to_string();
    }
    if let Some(decimal) = value.get("decimal").and_then(Value::as_str) {
        if let Some(currency) = value.get("currency").and_then(Value::as_str) {
            return format!("{decimal} {currency}");
        }
        return decimal.to_string();
    }
    if let Some(value) = value.get("value").and_then(Value::as_i64) {
        return value.to_string();
    }
    value.to_string()
}

fn total_pages(total: i64, per_page: i64) -> i64 {
    if total == 0 {
        0
    } else {
        ((total as f64) / (per_page as f64)).ceil() as i64
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::{
        AttachmentPackageArtifact, BusinessEventResponse, CHILD_AGGREGATE_COUNT_SQL, CHILD_AGGREGATE_DECIMAL_SQL,
        EventScope, ImportRecordsFileRequest, PARENT_REQUIRED_CHILD_TABLES_SQL, RECALCULATE_PARENT_RECORDS_SQL,
        attachment_claimed_object_keys, default_form_key_from_template, default_title_template_from_schema,
        ensure_can_read_job_result, ensure_link_target_scope, filter_event_response_values,
        import_records_request_from_uploaded_file_without_mapping, job_listing_owner_filter,
        normalize_scenario_field_schema, owned_record_event_predicate, push_event_scope_filters,
        record_claimed_object_keys, required_child_table_would_be_emptied, summarize_job_result,
        write_attachment_package_artifact,
    };
    use crate::{
        error::ApiError,
        forms::attachment_package::{ZipEntry, attachment_package_artifact_key, build_stored_zip},
        services::object_storage::{test_object_storage, test_should_create_bucket},
    };
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use uuid::Uuid;

    fn denied_fields(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|key| (*key).to_string()).collect()
    }

    fn parent_schema_with_child_table(required: bool) -> Value {
        json!({
            "fields": [
                {
                    "field_id": "fld_line_items",
                    "key": "line_items",
                    "type": "child_table",
                    "required": required
                },
                {"field_id": "fld_notes", "key": "notes", "type": "child_table"}
            ]
        })
    }

    #[test]
    fn archiving_the_last_row_of_a_required_child_table_is_refused() {
        let schema = parent_schema_with_child_table(true);

        assert!(
            required_child_table_would_be_emptied(&schema, "line_items", 0).expect("a well formed schema must parse"),
            "the required child table would be left with no rows"
        );
    }

    #[test]
    fn archiving_a_child_row_is_allowed_while_siblings_or_requiredness_remain_absent() {
        let required = parent_schema_with_child_table(true);
        let optional = parent_schema_with_child_table(false);

        // A sibling row keeps the constraint satisfied.
        assert!(!required_child_table_would_be_emptied(&required, "line_items", 1).expect("schema must parse"));
        // An optional child table may be emptied.
        assert!(!required_child_table_would_be_emptied(&optional, "line_items", 0).expect("schema must parse"));
        // A relation key that is not a required child table on this parent is none of our business.
        assert!(!required_child_table_would_be_emptied(&required, "notes", 0).expect("schema must parse"));
    }

    #[test]
    fn the_required_child_table_query_counts_live_siblings_only() {
        // The guard must not count the record being archived, already archived rows, rows under a
        // different relation key, or rows belonging to a different parent.
        assert!(PARENT_REQUIRED_CHILD_TABLES_SQL.contains("sibling.id <> child.id"));
        assert!(PARENT_REQUIRED_CHILD_TABLES_SQL.contains("sibling.archived_at IS NULL"));
        assert!(PARENT_REQUIRED_CHILD_TABLES_SQL.contains("sibling_links.relation_key = links.relation_key"));
        assert!(PARENT_REQUIRED_CHILD_TABLES_SQL.contains("sibling_links.source_record_id = links.source_record_id"));
        // Tenancy: a forged link must not drag in another workspace's rows.
        assert!(PARENT_REQUIRED_CHILD_TABLES_SQL.contains("parent.workspace_id = child.workspace_id"));
        assert!(PARENT_REQUIRED_CHILD_TABLES_SQL.contains("sibling.workspace_id = parent.workspace_id"));
    }

    #[test]
    fn rejects_record_link_targets_outside_the_source_tenant() {
        let workspace_id = Uuid::new_v4();
        let project_id = Uuid::new_v4();

        assert!(ensure_link_target_scope(workspace_id, project_id, workspace_id, project_id).is_ok());
        assert!(matches!(
            ensure_link_target_scope(workspace_id, project_id, Uuid::new_v4(), project_id),
            Err(ApiError::NotFound(_))
        ));
        assert!(matches!(
            ensure_link_target_scope(workspace_id, project_id, workspace_id, Uuid::new_v4()),
            Err(ApiError::NotFound(_))
        ));
    }

    #[test]
    fn child_aggregate_queries_stay_inside_the_parent_tenant() {
        for sql in [CHILD_AGGREGATE_COUNT_SQL, CHILD_AGGREGATE_DECIMAL_SQL] {
            assert!(
                sql.contains("child.workspace_id = $"),
                "missing workspace filter: {sql}"
            );
            assert!(sql.contains("child.project_id = $"), "missing project filter: {sql}");
        }
    }

    #[test]
    fn record_link_joins_never_cast_a_record_id_to_text() {
        for sql in [
            CHILD_AGGREGATE_COUNT_SQL,
            CHILD_AGGREGATE_DECIMAL_SQL,
            RECALCULATE_PARENT_RECORDS_SQL,
        ] {
            assert!(
                !sql.contains("child.id::text"),
                "casting the record id to text drops the primary key index: {sql}"
            );
            assert!(
                sql.contains("child.id = links.target_id::uuid"),
                "child records must be joined on their uuid primary key: {sql}"
            );
            assert!(
                sql.contains("links.target_id ~ '^[0-9a-fA-F-]{36}$'"),
                "the uuid cast must stay guarded: {sql}"
            );
        }
    }

    #[test]
    fn job_results_are_readable_only_by_their_creator_or_a_form_admin() {
        let creator = Uuid::new_v4();
        let other_member = Uuid::new_v4();

        assert!(ensure_can_read_job_result(Some(creator), creator, "member", false).is_ok());
        assert!(ensure_can_read_job_result(Some(creator), other_member, "admin", false).is_ok());
        assert!(ensure_can_read_job_result(Some(creator), other_member, "owner", false).is_ok());
        assert!(ensure_can_read_job_result(Some(creator), other_member, "member", true).is_ok());
        assert!(matches!(
            ensure_can_read_job_result(Some(creator), other_member, "member", false),
            Err(ApiError::NotFound(_))
        ));
        assert!(matches!(
            ensure_can_read_job_result(None, other_member, "member", false),
            Err(ApiError::NotFound(_))
        ));
    }

    #[test]
    fn job_listings_are_restricted_to_jobs_the_member_created() {
        let actor_id = Uuid::new_v4();
        let mut member_values: Vec<sea_orm::Value> = vec![Uuid::new_v4().into()];
        assert_eq!(
            job_listing_owner_filter(actor_id, "member", false, &mut member_values),
            " AND created_by = $2"
        );
        assert_eq!(member_values.len(), 2);

        let mut admin_values: Vec<sea_orm::Value> = vec![Uuid::new_v4().into()];
        assert!(job_listing_owner_filter(actor_id, "admin", false, &mut admin_values).is_empty());
        assert_eq!(admin_values.len(), 1);
    }

    #[test]
    fn job_listing_results_drop_bulk_export_payloads() {
        let summary = summarize_job_result(Some(json!({
            "form_id": "01234567-89ab-cdef-0123-456789abcdef",
            "file_name": "records.csv",
            "export_policy": {"row_count": 2, "truncated": false},
            "columns": [{"key": "salary"}],
            "rows": [{"values": {"salary": "9000"}}],
            "csv": "salary\n9000\n"
        })))
        .expect("completed jobs keep a summary");

        assert!(summary.get("rows").is_none());
        assert!(summary.get("csv").is_none());
        assert!(summary.get("columns").is_none());
        assert_eq!(summary.get("file_name").and_then(Value::as_str), Some("records.csv"));
        assert_eq!(summary.get("result_omitted").and_then(Value::as_bool), Some(true));
        assert!(summarize_job_result(None).is_none());

        let metadata_only =
            summarize_job_result(Some(json!({"file_name": "records.csv"}))).expect("completed jobs keep a summary");
        assert_eq!(
            metadata_only.get("result_omitted").and_then(Value::as_bool),
            Some(false)
        );
    }

    fn business_event(event_type: &str, payload: Value, source: Value) -> BusinessEventResponse {
        BusinessEventResponse {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            project_id: Some(Uuid::new_v4()),
            event_type: event_type.to_string(),
            aggregate_type: "form_record".to_string(),
            aggregate_id: Uuid::new_v4().to_string(),
            actor_id: None,
            source,
            payload,
            metadata: json!({"form_id": "f1", "values": {"salary": "9000"}}),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn event_responses_apply_field_level_read_permissions() {
        let event = business_event(
            "form.record.updated",
            json!({"record_id": "r1", "values": {"salary": "9000", "name": "Ada"}}),
            json!({"type": "user", "values": {"salary": "9000"}}),
        );

        let filtered = filter_event_response_values(event, &denied_fields(&["salary"]));

        assert!(filtered.payload.pointer("/values/salary").is_none());
        assert_eq!(
            filtered.payload.pointer("/values/name").and_then(Value::as_str),
            Some("Ada")
        );
        assert!(filtered.metadata.pointer("/values/salary").is_none());
        assert!(filtered.source.pointer("/values/salary").is_none());
    }

    #[test]
    fn archived_record_events_do_not_leak_the_rendered_title() {
        let event = business_event(
            "form.record.archived",
            json!({"record_id": "r1", "title": "Ada earns 9000"}),
            json!({"type": "user"}),
        );

        let filtered = filter_event_response_values(event, &denied_fields(&["salary"]));

        assert!(filtered.payload.get("title").is_none());
        assert_eq!(filtered.payload.get("record_id").and_then(Value::as_str), Some("r1"));
    }

    #[test]
    fn signature_audit_sources_do_not_leak_signature_values() {
        let event = business_event(
            "form.record.updated",
            json!({"record_id": "r1", "values": {"name": "Ada"}}),
            json!({
                "type": "user",
                "signature_media": {"entries": [{
                    "field_key": "signature",
                    "url": "/api/v1/uploads/signatures/signature-signature-1.png",
                    "reason": "approved"
                }]}
            }),
        );

        let filtered = filter_event_response_values(event, &denied_fields(&["signature"]));

        assert!(filtered.source.get("signature_media").is_none());
        assert!(!filtered.source.to_string().contains("approved"));
    }

    #[test]
    fn order_table_change_events_do_not_leak_top_level_field_values() {
        let event = business_event(
            "order.table_changed",
            json!({
                "record_id": "r1",
                "previous_table_id": "T1",
                "table_id": "T2",
                "values": {"table_id": "T2"}
            }),
            json!({"type": "system"}),
        );

        let filtered = filter_event_response_values(event, &denied_fields(&["table_id"]));

        assert!(filtered.payload.get("table_id").is_none());
        assert!(filtered.payload.get("previous_table_id").is_none());
        assert!(filtered.payload.pointer("/values/table_id").is_none());
    }

    #[test]
    fn form_event_listings_are_restricted_to_records_an_owned_scope_member_created() {
        let actor_id = Uuid::new_v4();
        let mut where_parts = Vec::new();
        let mut values = Vec::new();
        let mut idx = 1;

        push_event_scope_filters(
            EventScope::Form {
                project_id: Uuid::new_v4(),
                form_id: Uuid::new_v4(),
                owned_by: Some(actor_id),
            },
            &mut where_parts,
            &mut values,
            &mut idx,
        );

        assert_eq!(where_parts.len(), 3, "owned scope must add an ownership predicate");
        assert!(
            where_parts
                .iter()
                .any(|part| part.contains("owned_record.created_by = $4"))
        );
        assert_eq!(values.len(), 4);
        assert_eq!(idx, 5);
    }

    #[test]
    fn form_event_listings_stay_unfiltered_for_members_that_may_read_every_record() {
        let mut where_parts = Vec::new();
        let mut values = Vec::new();
        let mut idx = 1;

        push_event_scope_filters(
            EventScope::Form {
                project_id: Uuid::new_v4(),
                form_id: Uuid::new_v4(),
                owned_by: None,
            },
            &mut where_parts,
            &mut values,
            &mut idx,
        );

        assert_eq!(where_parts.len(), 2);
        assert!(!where_parts.iter().any(|part| part.contains("owned_record")));
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn owned_record_event_predicate_binds_the_expected_placeholders() {
        let predicate = owned_record_event_predicate(3, 4);

        assert!(predicate.contains("owned_record.form_id = $3"));
        assert!(predicate.contains("owned_record.created_by = $4"));
        assert!(predicate.contains("metadata->>'record_id' IS NULL"));
        assert!(!predicate.contains("$FORM_ID"));
        assert!(!predicate.contains("$ACTOR_ID"));
    }

    #[test]
    fn attachment_claims_cover_every_column_the_download_check_reads_back() {
        let claimed = attachment_claimed_object_keys(
            "form-records/r1/photo/1-photo.png",
            "/api/v1/uploads/victim-object.png",
            Some("/uploads/thumbnails/victim-thumb.png"),
        );

        assert!(claimed.contains("form-records/r1/photo/1-photo.png"));
        assert!(claimed.contains("victim-object.png"));
        assert!(claimed.contains("thumbnails/victim-thumb.png"));
    }

    #[test]
    fn attachment_claims_ignore_external_urls() {
        let claimed = attachment_claimed_object_keys("storage/key.png", "https://cdn.example.com/key.png", None);

        assert_eq!(claimed.len(), 1);
        assert!(claimed.contains("storage/key.png"));
    }

    #[test]
    fn normalizes_scenario_template_field_schema_for_form_use() {
        let schema = normalize_scenario_field_schema(json!({
            "fields": [
                {"key": "name", "label": "Name", "type": "text", "required": true},
                {"key": "status", "label": "Status", "type": "select", "options": ["open", "closed"]}
            ]
        }))
        .unwrap();

        assert_eq!(schema["version"], "openpr.form.schema.v1");
        assert_eq!(schema["fields"][1]["type"], "single_select");
        assert!(schema["fields"][0]["field_id"].as_str().is_some());
        assert_eq!(default_title_template_from_schema(&schema), "{name}");
    }

    #[test]
    fn derives_default_form_key_from_template_key() {
        assert_eq!(
            default_form_key_from_template("restaurant_ordering_default"),
            "restaurant_ordering"
        );
        assert_eq!(default_form_key_from_template("custom_ops"), "custom_ops");
    }

    #[tokio::test]
    #[ignore = "requires a reachable S3-compatible service such as MinIO and OPENPR_TEST_CONFIG pointing at an s3 configuration file"]
    async fn attachment_package_artifact_round_trips_against_minio_when_configured() {
        let storage = test_object_storage().expect("OPENPR_TEST_CONFIG should describe the s3 backend");
        assert_eq!(storage.reference("acceptance/probe.zip").backend, "s3");
        if test_should_create_bucket() {
            storage
                .create_bucket_for_test()
                .await
                .expect("bucket create should succeed");
        }

        let job_id = Uuid::new_v4();
        let zip = build_stored_zip(&[
            ZipEntry {
                path: "manifest.json".to_string(),
                data: br#"{"bundle_format":"openpr.form.attachments.package.v1","attachment_count":1}"#.to_vec(),
            },
            ZipEntry {
                path: "files/example.txt".to_string(),
                data: b"package artifact acceptance".to_vec(),
            },
        ])
        .expect("zip package should build");
        let package = AttachmentPackageArtifact {
            file_name: "minio_package_acceptance.zip".to_string(),
            zip: zip.clone(),
            attachment_count: 1,
            binary_file_count: 1,
        };

        let result = write_attachment_package_artifact(job_id, package)
            .await
            .expect("package artifact should write");
        assert_eq!(result["file_name"], "minio_package_acceptance.zip");
        assert_eq!(result["content_type"], "application/zip");
        assert_eq!(result["byte_size"], zip.len());
        assert_eq!(result["attachment_count"], 1);
        assert_eq!(result["binary_file_count"], 1);
        assert_eq!(
            result["download_url"],
            format!("/api/v1/form-attachment-package-jobs/{job_id}/download")
        );
        assert_eq!(result["artifact_storage"]["backend"], "s3");
        let stored_file_name = result["stored_file_name"]
            .as_str()
            .expect("stored file name should be recorded");
        let artifact_key = result["artifact_storage_key"]
            .as_str()
            .expect("artifact key should be recorded");
        assert_eq!(artifact_key, attachment_package_artifact_key(stored_file_name));
        assert_eq!(result["artifact_storage"]["key"], artifact_key);
        assert!(result["expires_at"].as_str().is_some());

        let stored = storage
            .get(artifact_key)
            .await
            .expect("package artifact should be readable from MinIO");
        assert_eq!(stored, zip);
        storage
            .delete(artifact_key)
            .await
            .expect("package artifact cleanup should succeed");
    }

    #[tokio::test]
    #[ignore = "requires a reachable S3-compatible service such as MinIO and OPENPR_TEST_CONFIG pointing at an s3 configuration file"]
    async fn import_file_artifact_round_trips_against_minio_when_configured() {
        let storage = test_object_storage().expect("OPENPR_TEST_CONFIG should describe the s3 backend");
        assert_eq!(storage.reference("acceptance/probe.csv").backend, "s3");
        if test_should_create_bucket() {
            storage
                .create_bucket_for_test()
                .await
                .expect("bucket create should succeed");
        }

        let file_name = format!("minio-import-{}.csv", Uuid::new_v4());
        let file_url = format!("/api/v1/uploads/{file_name}");
        let csv = b"title,sku_name,quantity\nFirst line,Apple,2\nSecond line,Banana,3\n";
        storage
            .put(&file_name, csv)
            .await
            .expect("import file upload should succeed");

        let form = test_form_response(json!({
            "version": "openpr.form.schema.v1",
            "fields": [
                {"field_id": "fld_sku_name_001", "key": "sku_name", "label": "SKU", "type": "text"},
                {"field_id": "fld_quantity_001", "key": "quantity", "label": "Quantity", "type": "number"}
            ]
        }));
        let request = import_records_request_from_uploaded_file_without_mapping(
            &form,
            ImportRecordsFileRequest {
                file_url: Some(file_url.clone()),
                upload_url: None,
                mapping_template_id: None,
                idempotency_key: Some("minio-import-acceptance".to_string()),
            },
        )
        .await
        .expect("import file should be read and parsed");

        assert_eq!(request.idempotency_key.as_deref(), Some("minio-import-acceptance"));
        assert_eq!(request.rows.len(), 2);
        assert_eq!(request.rows[0].row_number, Some(1));
        assert_eq!(request.rows[0].title.as_deref(), Some("First line"));
        assert_eq!(request.rows[0].values["sku_name"], "Apple");
        assert_eq!(request.rows[0].values["quantity"], "2");
        assert_eq!(request.rows[0].source.as_ref().unwrap()["origin"], "import_file");
        assert_eq!(request.rows[0].source.as_ref().unwrap()["file_url"], file_url);
        assert_eq!(request.rows[1].title.as_deref(), Some("Second line"));
        assert_eq!(request.rows[1].values["sku_name"], "Banana");

        storage
            .delete(&file_name)
            .await
            .expect("import artifact cleanup should succeed");
    }

    #[test]
    fn a_record_write_claiming_a_signature_object_is_screened() {
        let file_name = format!("signature-approval-{}.png", Uuid::new_v4());
        let url = format!("/api/v1/uploads/signatures/{file_name}");

        let claimed = record_claimed_object_keys(&json!({"approval": url, "notes": "signed"}), None);

        assert_eq!(claimed, denied_fields(&[&format!("signatures/{file_name}")]));
    }

    #[test]
    fn re_saving_an_unchanged_signature_value_claims_nothing() {
        let file_name = format!("signature-approval-{}.png", Uuid::new_v4());
        let url = format!("/api/v1/uploads/signatures/{file_name}");
        let stored = json!({"approval": url.clone()});

        assert!(record_claimed_object_keys(&json!({"approval": url}), Some(&stored)).is_empty());
    }

    #[test]
    fn replacing_a_signature_value_screens_only_the_new_object() {
        let previous = format!("signature-approval-{}.png", Uuid::new_v4());
        let replacement = format!("signature-approval-{}.png", Uuid::new_v4());
        let stored = json!({"approval": format!("/api/v1/uploads/signatures/{previous}")});

        let claimed = record_claimed_object_keys(
            &json!({"approval": format!("/api/v1/uploads/signatures/{replacement}")}),
            Some(&stored),
        );

        assert_eq!(claimed, denied_fields(&[&format!("signatures/{replacement}")]));
    }

    #[test]
    fn ordinary_record_values_are_not_treated_as_object_claims() {
        let claimed = record_claimed_object_keys(
            &json!({"notes": "/api/v1/uploads/signatures/signature-approval-not-a-uuid.png", "amount": 12}),
            None,
        );

        assert!(claimed.is_empty());
    }

    fn test_form_response(schema: serde_json::Value) -> super::FormResponse {
        let now = chrono::Utc::now();
        super::FormResponse {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            key: "order_line".to_string(),
            name: "Order Line".to_string(),
            description: String::new(),
            icon: None,
            color: None,
            title_template: "{title}".to_string(),
            schema,
            detail_layout: json!({}),
            schema_version: 1,
            created_by: None,
            archived_at: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// The generic record link endpoint against a real `PostgreSQL` server.
///
/// The endpoint used to require the parent form to declare a field named after the link's
/// `relation_key`, which rejected the shipped restaurant scenario: its `order` form keeps no child
/// table field because the `order_line` form holds the parent id. Only a real database shows that
/// the link is now written, that a declared field is still honoured, and that the tenant, target and
/// permission checks the same code path carries were not relaxed with it.
///
/// Set `OPENPR_TEST_DATABASE_URL` to a maintenance connection string, for example
/// `postgres://user:pw@127.0.0.1:5432/postgres`. Without it these tests report that they were
/// skipped instead of pretending to pass.
#[cfg(test)]
#[allow(clippy::indexing_slicing)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]
mod record_link_database_tests {
    use super::create_record_link;
    use crate::{error::ApiError, forms::relations::CreateLinkRequest};
    use axum::extract::{Extension, Json, Path, State};
    use platform::{
        app::AppState,
        auth::{JwtClaims, TokenType},
        config::{AppConfig, Secret},
    };
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
    use serde_json::{Value, json};
    use uuid::Uuid;

    const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

    /// A throwaway database that drops itself when the test is done.
    struct Scratch {
        db: DatabaseConnection,
        url: String,
        name: String,
        admin_url: String,
    }

    impl Scratch {
        async fn drop_self(self) {
            let Self {
                db, name, admin_url, ..
            } = self;
            drop(db);
            let Ok(admin) = Database::connect(&admin_url).await else {
                return;
            };
            if let Err(err) = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await
            {
                eprintln!("could not drop scratch database {name}: {err}");
            }
        }
    }

    /// Creates an empty database named after the test and applies every migration file on disk in
    /// name order, which is the order the runner uses. Returns `None` when the environment does not
    /// offer a server, so the suite stays runnable without one.
    async fn scratch(label: &str) -> Option<Scratch> {
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
        let admin = Database::connect(&admin_url)
            .await
            .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));

        let name = format!("openpr_link_{label}");
        let quoted = format!("\"{name}\"");
        admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS {quoted} WITH (FORCE)"))
            .await
            .unwrap_or_else(|err| panic!("could not reset scratch database {name}: {err}"));
        admin
            .execute_unprepared(&format!("CREATE DATABASE {quoted}"))
            .await
            .unwrap_or_else(|err| panic!("could not create scratch database {name}: {err}"));

        let (prefix, _) = admin_url.rsplit_once('/')?;
        let url = format!("{prefix}/{name}");
        let db = Database::connect(&url)
            .await
            .unwrap_or_else(|err| panic!("could not connect to scratch database {name}: {err}"));

        migrate(&db).await;

        Some(Scratch {
            db,
            url,
            name,
            admin_url,
        })
    }

    /// Applies `migrations/*.sql` in name order. The files are read from disk rather than copied
    /// here so a new migration cannot leave this schema behind.
    async fn migrate(db: &DatabaseConnection) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .expect("migrations directory is readable")
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "no migration file was found in {dir}");

        for path in files {
            let sql = std::fs::read_to_string(&path).expect("a migration file is readable");
            db.execute_unprepared(&sql)
                .await
                .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
        }
    }

    macro_rules! scratch_or_skip {
        ($label:expr) => {
            match scratch($label).await {
                Some(scratch) => scratch,
                None => {
                    eprintln!("skipped: {} is not set", TEST_DATABASE_URL_ENV);
                    return;
                }
            }
        };
    }

    fn state_for(scratch: &Scratch) -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "api-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new(scratch.url.clone()),
                jwt_secret: Secret::new("record-link-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
            },
            db: scratch.db.clone(),
        }
    }

    fn claims_for(user_id: Uuid) -> JwtClaims {
        JwtClaims {
            sub: user_id.to_string(),
            email: format!("{user_id}@example.test"),
            token_type: TokenType::Access,
            iat: 0,
            exp: 0,
        }
    }

    /// The restaurant `order` form: no child table field, so nothing declares `order_lines`.
    fn order_schema() -> Value {
        json!({
            "fields": [
                {"field_id": "fld_order_no", "key": "order_no", "type": "text"},
                {"field_id": "fld_order_status", "key": "status", "type": "text"}
            ]
        })
    }

    /// The same form once it does declare a child table, which the link then has to agree with.
    fn order_schema_with_child_table() -> Value {
        json!({
            "fields": [
                {"field_id": "fld_order_no", "key": "order_no", "type": "text"},
                {
                    "field_id": "fld_order_lines",
                    "key": "order_lines",
                    "type": "child_table",
                    "relation": {"relation_type": "parent_child", "form_key": "order_line"}
                }
            ]
        })
    }

    async fn create_user(state: &AppState, label: &str) -> Uuid {
        let user_id = Uuid::new_v4();
        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO users (id, email, name, password_hash, role) VALUES ($1, $2, $3, 'x', 'user')",
                vec![
                    user_id.into(),
                    format!("{label}-{user_id}@example.test").into(),
                    label.into(),
                ],
            ))
            .await
            .expect("the user is created");
        user_id
    }

    /// One workspace with one project, and the caller joined at `role`.
    async fn create_tenant(state: &AppState, label: &str, user_id: Uuid, role: &str) -> (Uuid, Uuid) {
        let workspace_id = Uuid::new_v4();
        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO workspaces (id, name, slug, created_by) VALUES ($1, $2, $3, $4)",
                vec![
                    workspace_id.into(),
                    label.into(),
                    format!("{label}-{workspace_id}").into(),
                    user_id.into(),
                ],
            ))
            .await
            .expect("the workspace is created");
        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, $3)",
                vec![workspace_id.into(), user_id.into(), role.into()],
            ))
            .await
            .expect("the membership is created");
        let project_id = create_project(state, workspace_id, user_id, label).await;
        (workspace_id, project_id)
    }

    async fn create_project(state: &AppState, workspace_id: Uuid, user_id: Uuid, label: &str) -> Uuid {
        let project_id = Uuid::new_v4();
        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, $3, $4, $5)",
                vec![
                    project_id.into(),
                    workspace_id.into(),
                    format!("P{}", &project_id.simple().to_string()[..6]).into(),
                    label.into(),
                    user_id.into(),
                ],
            ))
            .await
            .expect("the project is created");
        project_id
    }

    async fn create_form(state: &AppState, tenant: (Uuid, Uuid), key: &str, schema: Value) -> Uuid {
        let form_id = Uuid::new_v4();
        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    INSERT INTO project_forms (id, workspace_id, project_id, key, name, title_template, schema)
                    VALUES ($1, $2, $3, $4, $5, '{title}', $6)
                ",
                vec![
                    form_id.into(),
                    tenant.0.into(),
                    tenant.1.into(),
                    key.into(),
                    key.into(),
                    schema.into(),
                ],
            ))
            .await
            .expect("the form is created");
        form_id
    }

    async fn create_record(state: &AppState, tenant: (Uuid, Uuid), form_id: Uuid, created_by: Uuid) -> Uuid {
        let record_id = Uuid::new_v4();
        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    INSERT INTO form_records (id, workspace_id, project_id, form_id, title, values, created_by)
                    VALUES ($1, $2, $3, $4, $5, '{}'::jsonb, $6)
                ",
                vec![
                    record_id.into(),
                    tenant.0.into(),
                    tenant.1.into(),
                    form_id.into(),
                    format!("record {record_id}").into(),
                    created_by.into(),
                ],
            ))
            .await
            .expect("the record is created");
        record_id
    }

    async fn set_member_policy(state: &AppState, tenant: (Uuid, Uuid), form_id: Uuid, policy: Value) {
        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    INSERT INTO form_permissions (workspace_id, project_id, form_id, subject_type, subject_id, policy)
                    VALUES ($1, $2, $3, 'role', 'member', $4)
                ",
                vec![tenant.0.into(), tenant.1.into(), form_id.into(), policy.into()],
            ))
            .await
            .expect("the permission policy is stored");
    }

    #[derive(Debug, FromQueryResult)]
    struct LinkCountRow {
        count: i64,
    }

    #[derive(Debug, FromQueryResult)]
    struct RecordValuesRow {
        values: Value,
    }

    async fn link_count(state: &AppState, source_record_id: Uuid) -> i64 {
        LinkCountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*)::bigint AS count FROM form_record_links WHERE source_record_id = $1",
            vec![source_record_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("the link count is readable")
        .map_or(-1, |row| row.count)
    }

    fn link_request(target_id: &Uuid, relation_key: &str) -> CreateLinkRequest {
        CreateLinkRequest {
            target_type: "form_record".to_string(),
            target_id: target_id.to_string(),
            relation_key: relation_key.to_string(),
            relation_type: "parent_child".to_string(),
            metadata: None,
        }
    }

    async fn link(state: &AppState, user_id: Uuid, record_id: Uuid, req: CreateLinkRequest) -> Result<(), ApiError> {
        create_record_link(
            State(state.clone()),
            Extension(claims_for(user_id)),
            None,
            Path(record_id),
            Json(req),
        )
        .await
        .map(|_| ())
    }

    /// The shipped restaurant scenario models order lines on the child form, so the `order` schema
    /// declares no `order_lines` field and the link used to be rejected with a 400.
    #[tokio::test]
    async fn a_parent_child_link_is_written_when_the_parent_schema_declares_no_relation_field() {
        let scratch = scratch_or_skip!("undeclared");
        let state = state_for(&scratch);

        let user_id = create_user(&state, "owner").await;
        let tenant = create_tenant(&state, "restaurant", user_id, "owner").await;
        let order_form = create_form(&state, tenant, "order", order_schema()).await;
        let line_form = create_form(&state, tenant, "order_line", json!({"fields": []})).await;
        let order = create_record(&state, tenant, order_form, user_id).await;
        let line = create_record(&state, tenant, line_form, user_id).await;

        let result = link(&state, user_id, order, link_request(&line, "order_lines")).await;

        assert!(result.is_ok(), "the link must be accepted: {:?}", result.err());
        assert_eq!(link_count(&state, order).await, 1, "the link row must be stored");

        scratch.drop_self().await;
    }

    /// A declared field still binds: the link may not claim a key the parent form publishes for a
    /// different child form.
    #[tokio::test]
    async fn a_parent_child_link_that_contradicts_a_declared_relation_field_is_rejected() {
        let scratch = scratch_or_skip!("declared");
        let state = state_for(&scratch);

        let user_id = create_user(&state, "owner").await;
        let tenant = create_tenant(&state, "restaurant", user_id, "owner").await;
        let order_form = create_form(&state, tenant, "order", order_schema_with_child_table()).await;
        let line_form = create_form(&state, tenant, "order_line", json!({"fields": []})).await;
        let print_form = create_form(&state, tenant, "print_job", json!({"fields": []})).await;
        let order = create_record(&state, tenant, order_form, user_id).await;
        let line = create_record(&state, tenant, line_form, user_id).await;
        let print_job = create_record(&state, tenant, print_form, user_id).await;

        let mismatched = link(&state, user_id, order, link_request(&print_job, "order_lines")).await;
        assert!(
            matches!(mismatched, Err(ApiError::BadRequest(_))),
            "a link must not contradict the declared child table: {mismatched:?}"
        );
        assert_eq!(
            link_count(&state, order).await,
            0,
            "the rejected link must not be stored"
        );

        let matching = link(&state, user_id, order, link_request(&line, "order_lines")).await;
        assert!(
            matching.is_ok(),
            "the declared child form must still link: {:?}",
            matching.err()
        );
        assert_eq!(link_count(&state, order).await, 1, "the accepted link must be stored");

        scratch.drop_self().await;
    }

    /// Relaxing the schema check must not let a link reach out of its tenant, even for a caller who
    /// belongs to both workspaces.
    #[tokio::test]
    async fn a_parent_child_link_cannot_reach_another_tenant() {
        let scratch = scratch_or_skip!("tenant");
        let state = state_for(&scratch);

        let user_id = create_user(&state, "owner").await;
        let alpha = create_tenant(&state, "alpha", user_id, "owner").await;
        let beta = create_tenant(&state, "beta", user_id, "owner").await;
        let sibling_project = (alpha.0, create_project(&state, alpha.0, user_id, "second").await);

        let order_form = create_form(&state, alpha, "order", order_schema()).await;
        let order = create_record(&state, alpha, order_form, user_id).await;
        let beta_form = create_form(&state, beta, "order_line", json!({"fields": []})).await;
        let beta_line = create_record(&state, beta, beta_form, user_id).await;
        let sibling_form = create_form(&state, sibling_project, "order_line", json!({"fields": []})).await;
        let sibling_line = create_record(&state, sibling_project, sibling_form, user_id).await;

        let across_workspaces = link(&state, user_id, order, link_request(&beta_line, "order_lines")).await;
        assert!(
            matches!(across_workspaces, Err(ApiError::NotFound(_))),
            "another workspace's record must stay unreachable: {across_workspaces:?}"
        );

        let across_projects = link(&state, user_id, order, link_request(&sibling_line, "order_lines")).await;
        assert!(
            matches!(across_projects, Err(ApiError::NotFound(_))),
            "another project's record must stay unreachable: {across_projects:?}"
        );
        assert_eq!(link_count(&state, order).await, 0, "no cross tenant link may be stored");

        scratch.drop_self().await;
    }

    /// A target that does not exist, or that is not a record id at all, is still refused.
    #[tokio::test]
    async fn a_parent_child_link_to_a_missing_target_is_rejected() {
        let scratch = scratch_or_skip!("missing");
        let state = state_for(&scratch);

        let user_id = create_user(&state, "owner").await;
        let tenant = create_tenant(&state, "restaurant", user_id, "owner").await;
        let order_form = create_form(&state, tenant, "order", order_schema()).await;
        let order = create_record(&state, tenant, order_form, user_id).await;

        let unknown = link(&state, user_id, order, link_request(&Uuid::new_v4(), "order_lines")).await;
        assert!(
            matches!(unknown, Err(ApiError::NotFound(_))),
            "an unknown record id must be refused: {unknown:?}"
        );

        let malformed = CreateLinkRequest {
            target_type: "form_record".to_string(),
            target_id: "not-a-record-id".to_string(),
            relation_key: "order_lines".to_string(),
            relation_type: "parent_child".to_string(),
            metadata: None,
        };
        let malformed = link(&state, user_id, order, malformed).await;
        assert!(
            matches!(malformed, Err(ApiError::BadRequest(_))),
            "a target that is not a record id must be refused: {malformed:?}"
        );
        assert_eq!(
            link_count(&state, order).await,
            0,
            "no link may be stored for a missing target"
        );

        scratch.drop_self().await;
    }

    /// The caller still needs `form.view` on the target form, and still has to pass the target
    /// form's `record_scope` policy.
    #[tokio::test]
    async fn a_parent_child_link_still_needs_permission_on_the_target_form() {
        let scratch = scratch_or_skip!("permission");
        let state = state_for(&scratch);

        let member = create_user(&state, "member").await;
        let other = create_user(&state, "other").await;
        let tenant = create_tenant(&state, "restaurant", member, "member").await;
        let order_form = create_form(&state, tenant, "order", order_schema()).await;
        let hidden_form = create_form(&state, tenant, "order_line", json!({"fields": []})).await;
        let scoped_form = create_form(&state, tenant, "print_job", json!({"fields": []})).await;
        let order = create_record(&state, tenant, order_form, member).await;
        let hidden_line = create_record(&state, tenant, hidden_form, member).await;
        let foreign_record = create_record(&state, tenant, scoped_form, other).await;

        set_member_policy(&state, tenant, hidden_form, json!({"actions": {"form.view": false}})).await;
        set_member_policy(&state, tenant, scoped_form, json!({"record_scope": "owned"})).await;

        let without_view = link(&state, member, order, link_request(&hidden_line, "order_lines")).await;
        assert!(
            matches!(without_view, Err(ApiError::Forbidden(_))),
            "form.view on the target form must still be required: {without_view:?}"
        );

        let outside_scope = link(&state, member, order, link_request(&foreign_record, "order_lines")).await;
        assert!(
            matches!(outside_scope, Err(ApiError::Forbidden(_))),
            "the target record scope must still be enforced: {outside_scope:?}"
        );
        assert_eq!(link_count(&state, order).await, 0, "no unauthorized link may be stored");

        scratch.drop_self().await;
    }

    /// A link whose key no formula mentions must leave the parent's child aggregates alone rather
    /// than failing the write or feeding the wrong rows into a total.
    #[tokio::test]
    async fn an_undeclared_link_key_leaves_child_aggregates_untouched() {
        let scratch = scratch_or_skip!("aggregate");
        let state = state_for(&scratch);

        let user_id = create_user(&state, "owner").await;
        let tenant = create_tenant(&state, "restaurant", user_id, "owner").await;
        let order_form = create_form(
            &state,
            tenant,
            "order",
            json!({
                "fields": [
                    {"field_id": "fld_order_no", "key": "order_no", "type": "text"},
                    {
                        "field_id": "fld_lines_total",
                        "key": "lines_total",
                        "type": "number",
                        "formula": {"op": "child_sum", "relation_key": "lines", "field": "line_total"}
                    }
                ]
            }),
        )
        .await;
        let line_form = create_form(
            &state,
            tenant,
            "order_line",
            json!({"fields": [{"field_id": "fld_line_total", "key": "line_total", "type": "number"}]}),
        )
        .await;
        let order = create_record(&state, tenant, order_form, user_id).await;
        let line = create_record(&state, tenant, line_form, user_id).await;

        let result = link(&state, user_id, order, link_request(&line, "order_lines")).await;
        assert!(
            result.is_ok(),
            "an undeclared link key must not break the recalculation: {:?}",
            result.err()
        );

        let values = RecordValuesRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT values FROM form_records WHERE id = $1",
            vec![order.into()],
        ))
        .one(&state.db)
        .await
        .expect("the parent record is readable")
        .expect("the parent record exists")
        .values;
        assert_eq!(
            values
                .get("lines_total")
                .and_then(|total| total.get("decimal"))
                .and_then(Value::as_str),
            Some("0"),
            "a formula on another relation key must not pick the link up: {values}"
        );

        scratch.drop_self().await;
    }
}
