#![allow(
    clippy::pedantic,
    clippy::nursery,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_lossless
)]

use api::{middleware, response::ApiResponse, routes};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    middleware as axum_middleware,
    response::IntoResponse,
    routing::{delete, get, patch, post, put},
};
use platform::{
    app::{AppState, connect_db},
    config::AppConfig,
    logging,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    service: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = AppConfig::from_env("api", "0.0.0.0:8081")?;
    logging::init("api");

    let db = connect_db(&cfg.database_url).await?;
    run_migrations(&db).await?;
    verify_governance_schema(&db).await?;
    // A migration that only warned used to leave the delivery pipeline half built. Fail here
    // instead, while the reason is still on screen.
    routes::connector::verify_delivery_schema(&db)
        .await
        .map_err(|err| anyhow::anyhow!("{err}"))?;
    let state = AppState { cfg: cfg.clone(), db };
    let auth_state = state.clone();
    // Proposal settlement deliberately does not run here. It used to run both as an API
    // background task and inline on `GET /api/v1/proposals*`, which made a read request write
    // decisions and trust scores. The worker polling pipeline is now the only writer.
    routes::webhook::spawn_stored_endpoint_audit(state.clone());

    // Uploaded object downloads are gated by `uploads_access_middleware`: it accepts either a
    // signed download URL or a normal session, and the handlers then scope the object to the
    // workspace that owns it.
    let uploads_access_layer =
        || axum_middleware::from_fn_with_state(auth_state.clone(), routes::upload::uploads_access_middleware);

    let app = Router::new()
        .route(
            "/uploads/{file_name}",
            get(routes::upload::get_uploaded_file).route_layer(uploads_access_layer()),
        )
        .route(
            "/uploads/thumbnails/{file_name}",
            get(routes::upload::get_uploaded_thumbnail).route_layer(uploads_access_layer()),
        )
        .route(
            "/uploads/previews/{file_name}",
            get(routes::upload::get_uploaded_preview).route_layer(uploads_access_layer()),
        )
        .route(
            "/uploads/variants/{file_name}",
            get(routes::upload::get_uploaded_variant).route_layer(uploads_access_layer()),
        )
        .route(
            "/uploads/signatures/{file_name}",
            get(routes::upload::get_uploaded_signature).route_layer(uploads_access_layer()),
        )
        .route(
            "/api/v1/uploads/{file_name}",
            get(routes::upload::get_uploaded_file).route_layer(uploads_access_layer()),
        )
        .route(
            "/api/v1/uploads/thumbnails/{file_name}",
            get(routes::upload::get_uploaded_thumbnail).route_layer(uploads_access_layer()),
        )
        .route(
            "/api/v1/uploads/previews/{file_name}",
            get(routes::upload::get_uploaded_preview).route_layer(uploads_access_layer()),
        )
        .route(
            "/api/v1/uploads/variants/{file_name}",
            get(routes::upload::get_uploaded_variant).route_layer(uploads_access_layer()),
        )
        .route(
            "/api/v1/uploads/signatures/{file_name}",
            get(routes::upload::get_uploaded_signature).route_layer(uploads_access_layer()),
        )
        .route(
            "/api/v1/uploads/signatures/{file_name}/download",
            get(routes::upload::download_signed_uploaded_signature),
        )
        .route(
            "/api/v1/uploads/signatures/{file_name}/signed-url",
            post(routes::upload::create_uploaded_signature_signed_url).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/form-attachments/{attachment_id}/download",
            get(routes::form::download_signed_form_attachment),
        )
        .route("/health", get(health))
        .route("/ready", get(ready))
        // Auth routes
        .route("/api/v1/auth/register", post(routes::auth::register))
        .route("/api/v1/auth/login", post(routes::auth::login))
        .route("/api/v1/auth/refresh", post(routes::auth::refresh))
        .route(
            "/api/v1/auth/logout",
            post(routes::auth::logout).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/auth/me",
            get(routes::auth::me).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/auth/me/profile",
            put(routes::auth::update_profile).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/auth/me/password",
            put(routes::auth::update_password).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/auth/me/preferences",
            get(routes::auth::get_preferences)
                .put(routes::auth::update_preferences)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        // Workspace routes (protected)
        .route(
            "/api/v1/admin/users",
            post(routes::admin::create_user)
                .get(routes::admin::list_users)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::admin::admin_middleware,
                )),
        )
        .route(
            "/api/v1/admin/users/{id}",
            get(routes::admin::get_user)
                .put(routes::admin::update_user)
                .delete(routes::admin::delete_user)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::admin::admin_middleware,
                )),
        )
        .route(
            "/api/v1/admin/users/{id}/status",
            patch(routes::admin::toggle_user_status).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::admin::admin_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/admin/users/{id}/password",
            axum::routing::put(routes::admin::reset_user_password).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::admin::admin_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/admin/stats",
            get(routes::admin::get_stats).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::admin::admin_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces",
            post(routes::workspace::create_workspace)
                .get(routes::workspace::list_workspaces)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{id}",
            get(routes::workspace::get_workspace)
                .put(routes::workspace::update_workspace)
                .delete(routes::workspace::delete_workspace)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        // Workspace member routes (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/members",
            post(routes::member::add_member)
                .get(routes::member::list_members)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/members/{user_id}",
            delete(routes::member::remove_member)
                .patch(routes::member::update_member_role)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/users",
            get(routes::member::search_users).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        // Project routes (protected)
        .route(
            "/api/v1/project-types",
            get(routes::project_type::list_project_types).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/project-types/{key}",
            get(routes::project_type::get_project_type)
                .patch(routes::project_type::update_project_type)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/project-types",
            get(routes::project_type::list_workspace_project_types)
                .post(routes::project_type::create_project_type)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/scenario-templates",
            get(routes::scenario_template::list_scenario_templates).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/scenario-templates/{key}",
            get(routes::scenario_template::get_scenario_template).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects",
            post(routes::project::create_project)
                .get(routes::project::list_projects)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{id}",
            get(routes::project::get_project)
                .put(routes::project::update_project)
                .delete(routes::project::delete_project)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/scenario-templates/{template_key}/install",
            post(routes::project::install_project_scenario_template).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/projects/{project_id}/resources",
            get(routes::project_type::list_project_resources)
                .post(routes::project_type::create_project_resource)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/resources/{resource_id}",
            patch(routes::project_type::update_project_resource)
                .delete(routes::project_type::delete_project_resource)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/forms",
            get(routes::form::list_project_forms)
                .post(routes::form::create_project_form)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/forms/from-template",
            post(routes::form::create_project_form_from_template).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/forms/{form_id}",
            get(routes::form::get_form)
                .patch(routes::form::update_form)
                .delete(routes::form::delete_form)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/forms/{form_id}/restore",
            post(routes::form::restore_form).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/duplicate",
            post(routes::form::duplicate_form).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/views",
            get(routes::form::list_form_views)
                .post(routes::form::create_form_view)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/forms/{form_id}/aggregate",
            get(routes::form::aggregate_form_records).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/schema-summary",
            get(routes::form::get_form_schema_summary).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/field-usage",
            get(routes::form::get_form_field_usage).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/field-dependencies",
            get(routes::form::get_form_field_dependencies).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/schema-versions",
            get(routes::form::list_form_schema_versions).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/schema-versions/{version}",
            get(routes::form::get_form_schema_version).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/permissions",
            get(routes::form::get_form_permissions)
                .patch(routes::form::update_form_permissions)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/forms/{form_id}/relation-targets",
            get(routes::form::list_relation_targets).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/records/recalculate-preview",
            post(routes::form::preview_form_record_recalculation).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/attachments",
            get(routes::form::list_form_attachments)
                .post(routes::form::create_form_attachment)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/forms/{form_id}/attachments/package",
            get(routes::form::export_form_attachment_package).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/attachments/package-jobs",
            get(routes::form::list_form_attachment_package_jobs)
                .post(routes::form::create_form_attachment_package_job)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/form-attachment-package-jobs/{job_id}",
            get(routes::form::get_form_attachment_package_job).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/form-attachment-package-jobs/{job_id}/download",
            get(routes::form::download_form_attachment_package_job).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/form-attachments/{attachment_id}",
            delete(routes::form::archive_form_attachment).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/form-attachments/{attachment_id}/signed-url",
            get(routes::form::create_form_attachment_signed_url).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/form-attachments/{attachment_id}/restore",
            post(routes::form::restore_form_attachment).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/events",
            get(routes::form::list_form_events).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/form-views/{view_id}",
            patch(routes::form::update_form_view)
                .delete(routes::form::delete_form_view)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/forms/{form_id}/records",
            get(routes::form::list_form_records)
                .post(routes::form::create_form_record)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/forms/{form_id}/records/export",
            get(routes::form::export_form_records).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/records/export-jobs",
            get(routes::form::list_form_export_jobs)
                .post(routes::form::create_form_export_job)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/form-export-jobs/{job_id}",
            get(routes::form::get_form_export_job).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/records/import-preview",
            post(routes::form::preview_import_form_records).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/records/import-file-preview",
            post(routes::form::preview_import_form_records_from_file).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/forms/{form_id}/records/import",
            post(routes::form::import_form_records).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/records/import-file",
            post(routes::form::import_form_records_from_file).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/records/import-jobs",
            get(routes::form::list_form_import_jobs)
                .post(routes::form::create_form_import_job)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/form-import-jobs/{job_id}",
            get(routes::form::get_form_import_job).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/forms/{form_id}/import-mapping-templates",
            get(routes::form::list_form_import_mapping_templates)
                .post(routes::form::save_form_import_mapping_template)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/form-import-mapping-templates/{template_id}",
            delete(routes::form::delete_form_import_mapping_template).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/form-records/{record_id}",
            get(routes::form::get_form_record)
                .patch(routes::form::update_form_record)
                .delete(routes::form::delete_form_record)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/form-records/{record_id}/signatures/audit-verification",
            get(routes::form::verify_form_record_signature_audit).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/form-records/{record_id}/restore",
            post(routes::form::restore_form_record).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/form-records/{record_id}/recalculate",
            post(routes::form::recalculate_form_record).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/form-records/{record_id}/events",
            get(routes::form::list_form_record_events).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/form-records/{record_id}/comments",
            get(routes::form::list_form_record_comments)
                .post(routes::form::create_form_record_comment)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/form-records/{record_id}/links",
            get(routes::form::list_record_links)
                .post(routes::form::create_record_link)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/form-records/{record_id}/children",
            get(routes::form::list_record_children)
                .post(routes::form::create_child_record)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/form-records/{record_id}/children/{child_record_id}",
            patch(routes::form::update_child_record)
                .delete(routes::form::archive_child_record)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/form-records/{record_id}/children/{child_record_id}/restore",
            post(routes::form::restore_child_record).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/projects/{project_id}/plugins",
            get(routes::plugin::list_project_plugins)
                .post(routes::plugin::install_project_plugin)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/plugins/{plugin_id}",
            get(routes::plugin::get_plugin)
                .patch(routes::plugin::update_plugin)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/plugins/{plugin_id}/invoke",
            post(routes::plugin::invoke_plugin).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/plugins/{plugin_id}/invocations",
            get(routes::plugin::list_plugin_invocations).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/projects/{project_id}/context",
            get(routes::context::get_project_context).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/projects/{project_id}/governance-context",
            get(routes::context::get_project_governance_context).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/projects/{project_id}/agent-policy",
            get(routes::context::get_project_agent_policy).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/projects/{project_id}/release-readiness",
            get(routes::release_readiness::get_project_release_readiness).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/projects/{project_id}/domains",
            get(routes::decision_domain::list_decision_domains)
                .post(routes::decision_domain::create_decision_domain)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/domains/{id}",
            patch(routes::decision_domain::update_decision_domain)
                .delete(routes::decision_domain::delete_decision_domain)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/domains/{id}/members",
            get(routes::decision_domain::list_domain_members).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/decision-domains",
            get(routes::decision_domain::list_decision_domains_global).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/projects/{project_id}/ai-participants",
            get(routes::ai_agent::list_ai_agents)
                .post(routes::ai_agent::create_ai_agent)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/ai-participants/{id}/stats",
            get(routes::ai_agent::get_ai_agent_stats).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/projects/{project_id}/ai-participants/{id}",
            get(routes::ai_agent::get_ai_agent)
                .patch(routes::ai_agent::update_ai_agent)
                .delete(routes::ai_agent::delete_ai_agent)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/ai/tasks",
            get(routes::ai_callback::list_project_ai_tasks)
                .post(routes::ai_callback::create_project_ai_task)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/ai/callbacks/task/{task_id}/complete",
            post(routes::ai_callback::complete_task).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/ai/callbacks/task/{task_id}/fail",
            post(routes::ai_callback::fail_task).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/ai/callbacks/task/{task_id}/progress",
            post(routes::ai_callback::report_progress).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/projects/{project_id}/invocations",
            get(routes::connector::list_project_invocations)
                .post(routes::connector::create_project_invocation)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/invocations/{invocation_id}",
            get(routes::connector::get_invocation).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/invocations/{invocation_id}/tool-calls",
            get(routes::connector::list_invocation_tool_calls)
                .post(routes::connector::report_invocation_tool_call)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/invocations/{invocation_id}/inbox",
            get(routes::connector::list_invocation_inbox).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/invocations/{invocation_id}/inbox/{inbox_id}/replay",
            post(routes::connector::replay_invocation_inbox).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/forms/{form_id}/inbox",
            get(routes::connector::list_form_inbox).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/forms/{form_id}/inbox/{inbox_id}/replay",
            post(routes::connector::replay_form_inbox).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/invocations/{invocation_id}/progress",
            post(routes::connector::report_invocation_progress).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/invocations/{invocation_id}/complete",
            post(routes::connector::complete_invocation).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/invocations/{invocation_id}/fail",
            post(routes::connector::fail_invocation).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/invocations/{invocation_id}/receipt",
            post(routes::connector::report_connector_receipt).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/invocations/{invocation_id}/cancel",
            post(routes::connector::cancel_invocation).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/projects/{project_id}/check-results",
            get(routes::check_result::list_project_check_results)
                .post(routes::check_result::create_project_check_result)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/check-results/{check_result_id}",
            get(routes::check_result::get_check_result).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/check-results/{check_result_id}/proposal",
            post(routes::check_result::create_proposal_from_result).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/trust-scores",
            get(routes::trust_score::list_trust_scores).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/users/{id}/trust",
            get(routes::trust_score::get_user_trust).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/trust-scores/{user_id}",
            get(routes::trust_score::get_user_trust).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/trust-scores/{user_id}/history",
            get(routes::trust_score::list_user_trust_history).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/trust-scores/{user_id}/{domain}",
            get(routes::trust_score::get_user_trust_by_domain).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        // Issue routes (protected)
        .route(
            "/api/v1/projects/{project_id}/issues",
            post(routes::issue::create_issue)
                .get(routes::issue::list_issues)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/issues/{id}",
            get(routes::issue::get_issue)
                .put(routes::issue::update_issue)
                .delete(routes::issue::delete_issue)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        // Comment routes (protected)
        .route(
            "/api/v1/issues/{issue_id}/comments",
            post(routes::comment::create_comment)
                .get(routes::comment::list_comments)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/comments/{id}",
            axum::routing::put(routes::comment::update_comment)
                .delete(routes::comment::delete_comment)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        // Governance proposal routes
        .route(
            "/api/v1/proposals",
            get(routes::proposal::list_proposals).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/proposals",
            post(routes::proposal::create_proposal).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposals/{id}",
            get(routes::proposal::get_proposal).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/proposals/{id}",
            patch(routes::proposal::update_proposal)
                .delete(routes::proposal::delete_proposal)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/proposals/{id}/submit",
            post(routes::proposal::submit_proposal).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposals/{id}/start-voting",
            post(routes::proposal::start_voting).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/proposals/{id}/archive",
            post(routes::proposal::archive_proposal).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposals/{id}/votes",
            get(routes::proposal::list_votes).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/proposals/{id}/votes",
            post(routes::proposal::create_vote).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/proposals/{id}/votes/mine",
            delete(routes::proposal::delete_my_vote).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposals/{id}/veto",
            get(routes::veto::get_veto)
                .post(routes::veto::exercise_veto)
                .delete(routes::veto::withdraw_veto)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/proposals/{id}/veto/escalation",
            post(routes::veto::start_escalation).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/proposals/{id}/veto/escalation/vote",
            post(routes::veto::vote_escalation).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/vetoers",
            get(routes::veto::list_vetoers)
                .post(routes::veto::create_vetoer)
                .delete(routes::veto::delete_vetoer)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/trust-scores/appeals",
            get(routes::appeal::list_appeals)
                .post(routes::appeal::create_appeal)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/trust-scores/appeals/{id}",
            get(routes::appeal::get_appeal)
                .patch(routes::appeal::update_appeal)
                .delete(routes::appeal::delete_appeal)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/proposals/{id}/comments",
            get(routes::proposal::list_proposal_comments).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/proposals/{id}/comments",
            post(routes::proposal::create_proposal_comment).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposal-comments/{id}",
            delete(routes::proposal::delete_proposal_comment).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposals/{id}/comments/{comment_id}",
            delete(routes::proposal::delete_proposal_comment_under_proposal).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposals/{id}/issues",
            post(routes::proposal::link_issue)
                .get(routes::proposal::list_linked_issues)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/proposals/{proposal_id}/issues/{issue_id}",
            delete(routes::proposal::unlink_issue).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposal-templates",
            get(routes::proposal_template::list_proposal_templates)
                .post(routes::proposal_template::create_proposal_template)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/proposal-templates/{id}",
            get(routes::proposal_template::get_proposal_template)
                .put(routes::proposal_template::update_proposal_template)
                .delete(routes::proposal_template::delete_proposal_template)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/governance/config",
            get(routes::governance::get_governance_config).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/governance/config",
            put(routes::governance::update_governance_config).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::admin::admin_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/governance/audit-logs",
            get(routes::governance::list_governance_audit_logs).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::admin::admin_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/decisions",
            get(routes::decision::list_decisions).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/decisions/{id}",
            get(routes::decision::get_decision).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/proposals/{id}/decision",
            get(routes::decision::get_proposal_decision).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposals/{id}/impact-review",
            get(routes::impact_review::get_impact_review_by_proposal)
                .post(routes::impact_review::create_impact_review)
                .patch(routes::impact_review::update_impact_review_by_proposal)
                .delete(routes::impact_review::delete_impact_review_by_proposal)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/impact-reviews",
            get(routes::impact_review::list_impact_reviews).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/impact-reviews/{id}/participants",
            get(routes::impact_review::list_review_participants)
                .post(routes::impact_review::upsert_review_participant)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/impact-reviews/{id}/participants/{user_id}",
            patch(routes::impact_review::update_review_participant)
                .delete(routes::impact_review::delete_review_participant)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/proposals/{id}/chain",
            get(routes::governance_ext::get_proposal_chain).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposals/{id}/timeline",
            get(routes::governance_ext::get_proposal_timeline).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/decisions/analytics",
            get(routes::governance_ext::get_decision_analytics).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/projects/{pid}/audit-reports",
            post(routes::governance_ext::create_project_audit_report)
                .get(routes::governance_ext::list_project_audit_reports)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{pid}/audit-reports/{id}",
            get(routes::governance_ext::get_project_audit_report).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/ai-learning/{review_id}/feedback",
            get(routes::governance_ext::get_ai_review_feedback).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/ai-participants/{id}/learning",
            get(routes::governance_ext::get_ai_participant_learning).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/ai-participants/{id}/alignment-stats",
            get(routes::governance_ext::get_ai_participant_alignment_stats).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        // Label routes (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/labels",
            post(routes::label::create_label)
                .get(routes::label::list_labels)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/labels/{id}",
            axum::routing::put(routes::label::update_label)
                .delete(routes::label::delete_label)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        // Issue label association routes (protected)
        .route(
            "/api/v1/issues/{issue_id}/labels/{label_id}",
            post(routes::label::add_label_to_issue)
                .delete(routes::label::remove_label_from_issue)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/issues/{issue_id}/labels",
            get(routes::label::get_issue_labels).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        // Bot-accessible routes (bot_or_user_auth_middleware)
        .route(
            "/api/v1/projects/{project_id}/labels",
            get(routes::label::list_project_labels).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/issues/by-identifier/{identifier}",
            get(routes::issue::get_issue_by_identifier).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/issues/{issue_id}/labels/batch",
            post(routes::issue::add_labels_to_issue).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        // Bot token management routes (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/bots",
            post(routes::bot::create_bot)
                .get(routes::bot::list_bots)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/bots/{bot_id}",
            axum::routing::delete(routes::bot::revoke_bot).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        // Activity routes (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/activities",
            get(routes::activity::get_workspace_activities).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/projects/{project_id}/activities",
            get(routes::activity::get_project_activities).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/issues/{issue_id}/activities",
            get(routes::activity::get_issue_activities).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        // My routes (protected)
        .route(
            "/api/v1/my/issues",
            get(routes::my::get_my_issues).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/my/activities",
            get(routes::my::get_my_activities).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        // Board routes (protected)
        .route(
            "/api/v1/projects/{project_id}/board",
            get(routes::board::get_project_board).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/projects/{project_id}/workflow/effective",
            get(routes::workflow::get_effective_workflow_by_project).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        // Workflow CRUD routes (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/workflows",
            post(routes::workflow::create_workflow)
                .get(routes::workflow::list_workflows)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workflows/{workflow_id}",
            get(routes::workflow::get_workflow)
                .put(routes::workflow::update_workflow)
                .delete(routes::workflow::delete_workflow)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workflows/{workflow_id}/states",
            get(routes::workflow::list_workflow_states)
                .post(routes::workflow::create_workflow_state)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workflows/{workflow_id}/states/reorder",
            put(routes::workflow::reorder_workflow_states).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/workflow-states/{state_id}",
            put(routes::workflow::update_workflow_state)
                .delete(routes::workflow::delete_workflow_state)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/workflow",
            put(routes::workflow::set_project_workflow).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        // Sprint routes (protected)
        .route(
            "/api/v1/projects/{project_id}/sprints",
            post(routes::sprint::create_sprint)
                .get(routes::sprint::list_sprints)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/sprints/{id}",
            axum::routing::put(routes::sprint::update_sprint)
                .delete(routes::sprint::delete_sprint)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        // Webhook routes (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/webhooks",
            post(routes::webhook::create_webhook)
                .get(routes::webhook::list_webhooks)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}",
            get(routes::webhook::get_webhook)
                .patch(routes::webhook::update_webhook)
                .delete(routes::webhook::delete_webhook)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/deliveries",
            get(routes::webhook::list_deliveries).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/deliveries/{delivery_id}",
            get(routes::webhook::get_delivery).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/connectors",
            post(routes::connector::create_connector)
                .get(routes::connector::list_connectors)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/connectors/{connector_id}",
            get(routes::connector::get_connector)
                .patch(routes::connector::update_connector)
                .delete(routes::connector::delete_connector)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        // Notification routes (protected)
        .route(
            "/api/v1/notifications",
            get(routes::notification::list_notifications).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/notifications/unread-count",
            get(routes::notification::get_unread_count).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/notifications/{id}/read",
            patch(routes::notification::mark_notification_read)
                .put(routes::notification::mark_notification_read)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/notifications/read-all",
            patch(routes::notification::mark_all_read)
                .put(routes::notification::mark_all_read)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/notifications/{id}",
            delete(routes::notification::delete_notification).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        // Search route (protected)
        .route(
            "/api/v1/search",
            get(routes::search::search).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        // Export route (protected)
        .route(
            "/api/v1/export/project/{id}",
            get(routes::export::export_project).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        // Import route (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/import/project",
            post(routes::import::import_project).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        // Upload route (protected)
        .route(
            "/api/v1/upload",
            post(routes::upload::upload_file).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .layer(DefaultBodyLimit::max(200 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .layer(CompressionLayer::new())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr).await?;
    tracing::info!(bind_addr = %cfg.bind_addr, "api server started");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> Json<ApiResponse<HealthResponse>> {
    let body = HealthResponse {
        status: "ok",
        service: state.cfg.app_name,
    };
    ApiResponse::success(body)
}

async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    match state.db.ping().await {
        Ok(_) => ApiResponse::success(serde_json::json!({"status":"ready"})).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "database not ready");
            ApiResponse::error(500, "database not ready").into_response()
        }
    }
}

/// Every migration file, in application order.
///
/// A file that exists on disk but is missing here is silently never applied, so
/// `every_migration_file_is_registered` asserts the two stay in sync.
const MIGRATIONS: &[(&str, &str)] = &[
    // Must stay first: the ledger has to exist before any outcome can be recorded.
    (
        "0000_schema_migrations.sql",
        include_str!("../../../migrations/0000_schema_migrations.sql"),
    ),
    ("0001_init.sql", include_str!("../../../migrations/0001_init.sql")),
    ("0002_users.sql", include_str!("../../../migrations/0002_users.sql")),
    ("0003_labels.sql", include_str!("../../../migrations/0003_labels.sql")),
    ("0004_sprints.sql", include_str!("../../../migrations/0004_sprints.sql")),
    (
        "0005_webhooks.sql",
        include_str!("../../../migrations/0005_webhooks.sql"),
    ),
    (
        "0006_notifications.sql",
        include_str!("../../../migrations/0006_notifications.sql"),
    ),
    (
        "0007_fulltext_search.sql",
        include_str!("../../../migrations/0007_fulltext_search.sql"),
    ),
    (
        "0008_admin_user_fields.sql",
        include_str!("../../../migrations/0008_admin_user_fields.sql"),
    ),
    (
        "0009_notifications_schema_compat.sql",
        include_str!("../../../migrations/0009_notifications_schema_compat.sql"),
    ),
    (
        "0010_issue_activity_notifications_compat.sql",
        include_str!("../../../migrations/0010_issue_activity_notifications_compat.sql"),
    ),
    (
        "0011_bot_user_and_webhook_fields.sql",
        include_str!("../../../migrations/0011_bot_user_and_webhook_fields.sql"),
    ),
    (
        "0012_governance_phase1.sql",
        include_str!("../../../migrations/0012_governance_phase1.sql"),
    ),
    (
        "0013_governance_work_items_fields.sql",
        include_str!("../../../migrations/0013_governance_work_items_fields.sql"),
    ),
    (
        "0014_proposal_comments_schema_compat.sql",
        include_str!("../../../migrations/0014_proposal_comments_schema_compat.sql"),
    ),
    (
        "0015_governance_phase2.sql",
        include_str!("../../../migrations/0015_governance_phase2.sql"),
    ),
    (
        "0016_governance_phase3.sql",
        include_str!("../../../migrations/0016_governance_phase3.sql"),
    ),
    (
        "0017_governance_phase3_hardening.sql",
        include_str!("../../../migrations/0017_governance_phase3_hardening.sql"),
    ),
    (
        "0018_governance_phase3_templates.sql",
        include_str!("../../../migrations/0018_governance_phase3_templates.sql"),
    ),
    (
        "0019_governance_cycle_template_rapid.sql",
        include_str!("../../../migrations/0019_governance_cycle_template_rapid.sql"),
    ),
    (
        "0020_ai_tasks.sql",
        include_str!("../../../migrations/0020_ai_tasks.sql"),
    ),
    (
        "0021_fix_cascade.sql",
        include_str!("../../../migrations/0021_fix_cascade.sql"),
    ),
    (
        "0022_bot_tokens.sql",
        include_str!("../../../migrations/0022_bot_tokens.sql"),
    ),
    (
        "0023_work_item_identifier.sql",
        include_str!("../../../migrations/0023_work_item_identifier.sql"),
    ),
    (
        "0024_workflow_config.sql",
        include_str!("../../../migrations/0024_workflow_config.sql"),
    ),
    (
        "0025_project_types_resources.sql",
        include_str!("../../../migrations/0025_project_types_resources.sql"),
    ),
    (
        "0026_connectors_invocations.sql",
        include_str!("../../../migrations/0026_connectors_invocations.sql"),
    ),
    (
        "0027_check_results.sql",
        include_str!("../../../migrations/0027_check_results.sql"),
    ),
    (
        "0028_invocation_tool_calls.sql",
        include_str!("../../../migrations/0028_invocation_tool_calls.sql"),
    ),
    (
        "0029_scenario_templates.sql",
        include_str!("../../../migrations/0029_scenario_templates.sql"),
    ),
    (
        "0030_universal_forms.sql",
        include_str!("../../../migrations/0030_universal_forms.sql"),
    ),
    (
        "0031_business_events_outbox_inbox.sql",
        include_str!("../../../migrations/0031_business_events_outbox_inbox.sql"),
    ),
    (
        "0032_print_device_connector_kinds.sql",
        include_str!("../../../migrations/0032_print_device_connector_kinds.sql"),
    ),
    (
        "0033_plugins_wasm.sql",
        include_str!("../../../migrations/0033_plugins_wasm.sql"),
    ),
    (
        "0034_user_profile_preferences.sql",
        include_str!("../../../migrations/0034_user_profile_preferences.sql"),
    ),
    (
        "0035_project_type_two_lanes.sql",
        include_str!("../../../migrations/0035_project_type_two_lanes.sql"),
    ),
    (
        "0036_universal_forms_schema_version_archive.sql",
        include_str!("../../../migrations/0036_universal_forms_schema_version_archive.sql"),
    ),
    (
        "0037_form_attachments.sql",
        include_str!("../../../migrations/0037_form_attachments.sql"),
    ),
    (
        "0038_form_permissions.sql",
        include_str!("../../../migrations/0038_form_permissions.sql"),
    ),
    (
        "0039_universal_forms_pivot_view_type.sql",
        include_str!("../../../migrations/0039_universal_forms_pivot_view_type.sql"),
    ),
    (
        "0040_universal_forms_gantt_view_type.sql",
        include_str!("../../../migrations/0040_universal_forms_gantt_view_type.sql"),
    ),
    (
        "0041_form_record_comments.sql",
        include_str!("../../../migrations/0041_form_record_comments.sql"),
    ),
    (
        "0042_form_attachment_thumbnail_url.sql",
        include_str!("../../../migrations/0042_form_attachment_thumbnail_url.sql"),
    ),
    (
        "0043_form_import_mapping_templates.sql",
        include_str!("../../../migrations/0043_form_import_mapping_templates.sql"),
    ),
    (
        "0044_form_import_jobs.sql",
        include_str!("../../../migrations/0044_form_import_jobs.sql"),
    ),
    (
        "0045_form_export_jobs.sql",
        include_str!("../../../migrations/0045_form_export_jobs.sql"),
    ),
    (
        "0046_form_attachment_package_jobs.sql",
        include_str!("../../../migrations/0046_form_attachment_package_jobs.sql"),
    ),
    (
        "0047_form_attachment_media_metadata.sql",
        include_str!("../../../migrations/0047_form_attachment_media_metadata.sql"),
    ),
    (
        "0048_delivery_lease_and_pickup_indexes.sql",
        include_str!("../../../migrations/0048_delivery_lease_and_pickup_indexes.sql"),
    ),
    (
        "0049_agent_invocation_duplicate_tagging.sql",
        include_str!("../../../migrations/0049_agent_invocation_duplicate_tagging.sql"),
    ),
];

async fn run_migrations(db: &DatabaseConnection) -> anyhow::Result<()> {
    let Some((ledger_name, ledger_sql)) = MIGRATIONS.first() else {
        return Err(anyhow::anyhow!("migration list is empty, refusing to start"));
    };

    // The ledger bootstrap is idempotent by construction, so it is the one file that may run on
    // every start: nothing can be read from or written to the ledger before it exists.
    apply_migration(db, ledger_name, ledger_sql)
        .await
        .map_err(|err| anyhow::anyhow!("migration ledger bootstrap failed: {err}"))?;
    record_migration_if_absent(db, ledger_name, &migration_checksum(ledger_sql)).await?;

    let replay = migration_replay_requested();
    if replay {
        tracing::warn!(
            env = MIGRATION_REPLAY_ENV,
            "migration replay requested: every migration is re-executed and failures are reported \
             but not fatal, matching the pre-ledger runner"
        );
    }

    let mut ledger = load_migration_ledger(db).await?;

    // A database created by the pre-ledger runner already carries the whole schema, but the ledger
    // is empty. Replaying 0001..cutoff there would be both slow and unsafe, so those entries are
    // claimed without executing them. The claim is bounded by a frozen cutoff, so migrations added
    // after the ledger shipped are never adopted by accident.
    if !replay && ledger.len() <= 1 && database_predates_ledger(db).await? {
        let mut adopted = 0_usize;
        for (name, sql) in MIGRATIONS
            .iter()
            .skip(1)
            .filter(|(name, _)| *name <= MIGRATION_ADOPTION_CUTOFF)
        {
            record_migration(db, name, &migration_checksum(sql), "adopted", None).await?;
            adopted += 1;
        }
        tracing::warn!(
            adopted,
            cutoff = MIGRATION_ADOPTION_CUTOFF,
            "existing database adopted into the migration ledger without replaying it; the schema \
             assertions that follow decide whether it is actually complete"
        );
        ledger = load_migration_ledger(db).await?;
    }

    for (name, sql) in MIGRATIONS.iter().skip(1) {
        let checksum = migration_checksum(sql);
        if !replay
            && let Some(recorded) = ledger.get(*name)
            && recorded.status != "failed"
        {
            if recorded.checksum != checksum {
                tracing::warn!(
                    migration = %name,
                    recorded_status = %recorded.status,
                    "migration file changed after it was recorded; the database still holds the \
                     previously applied version"
                );
            }
            continue;
        }

        match apply_migration(db, name, sql).await {
            Ok(()) => {
                record_migration(db, name, &checksum, "applied", None).await?;
                tracing::info!(migration = %name, "migration applied");
            }
            Err(err) => {
                let message = err.to_string();
                tracing::error!(
                    migration = %name,
                    error = %message,
                    "migration failed; the ledger keeps it unapplied so the next start retries it"
                );
                if let Err(record_err) = record_migration(db, name, &checksum, "failed", Some(&message)).await {
                    tracing::error!(
                        migration = %name,
                        error = %record_err,
                        "could not record the migration failure in the ledger"
                    );
                }
                if !replay {
                    return Err(anyhow::anyhow!("migration {name} failed: {message}"));
                }
            }
        }
    }

    Ok(())
}

/// Newest migration an existing database may claim without executing it.
///
/// Frozen on purpose. It stops at 0047 rather than at the last pre-ledger file (0048) because
/// 0048 is fully idempotent and is the one whose failure the pre-ledger runner used to hide: a
/// database that never got its delivery lease column re-executes it once and heals, instead of
/// being claimed as applied and failing the schema assertions forever. Never move this forward,
/// or a future migration would silently be skipped on every existing deployment.
const MIGRATION_ADOPTION_CUTOFF: &str = "0047_form_attachment_media_metadata.sql";

/// Escape hatch that restores the pre-ledger behaviour for one start: everything is re-executed
/// and a failure is reported without aborting. Recovery tool for a database whose ledger and
/// actual schema disagree; not meant for normal operation.
const MIGRATION_REPLAY_ENV: &str = "OPENPR_MIGRATIONS_REPLAY";

/// One recorded migration outcome.
struct RecordedMigration {
    checksum: String,
    status: String,
}

fn migration_replay_requested() -> bool {
    std::env::var(MIGRATION_REPLAY_ENV)
        .is_ok_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

/// Content fingerprint used to notice that a migration file changed after it was applied.
fn migration_checksum(sql: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sql.as_bytes());
    hex::encode(hasher.finalize())
}

/// Runs one migration atomically. Partially applied files cannot survive a failure, so the ledger
/// and the schema never drift apart within a single migration.
async fn apply_migration(db: &DatabaseConnection, name: &str, sql: &str) -> anyhow::Result<()> {
    let txn = db.begin().await?;
    if let Err(err) = txn.execute_unprepared(sql).await {
        if let Err(rollback_err) = txn.rollback().await {
            tracing::warn!(migration = %name, error = %rollback_err, "migration rollback failed");
        }
        return Err(err.into());
    }
    txn.commit().await?;
    Ok(())
}

async fn record_migration(
    db: &DatabaseConnection,
    name: &str,
    checksum: &str,
    status: &str,
    error: Option<&str>,
) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO schema_migrations (name, checksum, status, error, applied_at)
            VALUES ($1, $2, $3, $4, now())
            ON CONFLICT (name) DO UPDATE
            SET checksum = EXCLUDED.checksum,
                status = EXCLUDED.status,
                error = EXCLUDED.error,
                applied_at = EXCLUDED.applied_at
        ",
        vec![
            name.into(),
            checksum.into(),
            status.into(),
            error.map(ToString::to_string).into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Records the ledger bootstrap without touching an existing row, so `applied_at` keeps meaning
/// "when this database first got the migration" instead of "when the API last started".
async fn record_migration_if_absent(db: &DatabaseConnection, name: &str, checksum: &str) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO schema_migrations (name, checksum, status, error, applied_at)
            VALUES ($1, $2, 'applied', NULL, now())
            ON CONFLICT (name) DO NOTHING
        ",
        vec![name.into(), checksum.into()],
    ))
    .await?;
    Ok(())
}

async fn load_migration_ledger(db: &DatabaseConnection) -> anyhow::Result<HashMap<String, RecordedMigration>> {
    let rows = db
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT name, checksum, status FROM schema_migrations".to_string(),
        ))
        .await?;

    let mut ledger = HashMap::with_capacity(rows.len());
    for row in rows {
        let name = row.try_get::<String>("", "name")?;
        let checksum = row.try_get::<String>("", "checksum")?;
        let status = row.try_get::<String>("", "status")?;
        ledger.insert(name, RecordedMigration { checksum, status });
    }
    Ok(ledger)
}

/// True when the database already carries the pre-ledger schema. `users` is created by 0001 and
/// never dropped, so its presence separates "existing deployment" from "empty database".
async fn database_predates_ledger(db: &DatabaseConnection) -> anyhow::Result<bool> {
    let row = db
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT to_regclass('public.users') IS NOT NULL AS present".to_string(),
        ))
        .await?;
    match row {
        Some(row) => Ok(row.try_get::<bool>("", "present")?),
        None => Ok(false),
    }
}

async fn verify_governance_schema(db: &DatabaseConnection) -> anyhow::Result<()> {
    const CHECKS: &[(&str, &str)] = &[
        ("proposals table", "SELECT 1 FROM proposals LIMIT 1"),
        ("proposal_type enum", "SELECT 'feature'::proposal_type"),
        ("proposal_status enum", "SELECT 'draft'::proposal_status"),
        ("author_type enum", "SELECT 'human'::author_type"),
        ("trust_scores table", "SELECT 1 FROM trust_scores LIMIT 1"),
        ("trust_score_logs table", "SELECT 1 FROM trust_score_logs LIMIT 1"),
        ("ai_participants table", "SELECT 1 FROM ai_participants LIMIT 1"),
        ("decision_domains table", "SELECT 1 FROM decision_domains LIMIT 1"),
        ("veto_events table", "SELECT 1 FROM veto_events LIMIT 1"),
        ("appeals table", "SELECT 1 FROM appeals LIMIT 1"),
        ("impact_reviews table", "SELECT 1 FROM impact_reviews LIMIT 1"),
        ("review_participants table", "SELECT 1 FROM review_participants LIMIT 1"),
        ("ai_learning_records table", "SELECT 1 FROM ai_learning_records LIMIT 1"),
        (
            "decision_audit_reports table",
            "SELECT 1 FROM decision_audit_reports LIMIT 1",
        ),
        ("feedback_loop_links table", "SELECT 1 FROM feedback_loop_links LIMIT 1"),
        ("governance_configs table", "SELECT 1 FROM governance_configs LIMIT 1"),
        (
            "governance_audit_logs table",
            "SELECT 1 FROM governance_audit_logs LIMIT 1",
        ),
        ("check_results table", "SELECT 1 FROM check_results LIMIT 1"),
        ("trust_level enum", "SELECT 'observer'::trust_level"),
        ("participant_type enum", "SELECT 'human'::participant_type"),
        ("veto_status enum", "SELECT 'active'::veto_status"),
        ("appeal_status enum", "SELECT 'pending'::appeal_status"),
        ("review_status enum", "SELECT 'pending'::review_status"),
        ("review_rating enum", "SELECT 'A'::review_rating"),
    ];

    for (name, sql) in CHECKS {
        if let Err(e) = db
            .query_one(Statement::from_string(DbBackend::Postgres, (*sql).to_string()))
            .await
        {
            return Err(anyhow::anyhow!("governance schema check failed for {}: {}", name, e));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MIGRATION_ADOPTION_CUTOFF, MIGRATIONS, migration_checksum};

    /// A migration file has to be registered in two places: on disk and in [`MIGRATIONS`].
    /// Forgetting the second one makes it silently never run, so the two are compared here.
    #[test]
    fn every_migration_file_is_registered() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .expect("migrations directory is readable")
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".sql"))
            .collect();
        on_disk.sort();

        let registered: Vec<String> = MIGRATIONS.iter().map(|(name, _)| (*name).to_string()).collect();
        assert_eq!(
            registered, on_disk,
            "migrations/ and MIGRATIONS must list the same files"
        );
    }

    #[test]
    fn migrations_are_registered_in_file_name_order() {
        let mut sorted: Vec<&str> = MIGRATIONS.iter().map(|(name, _)| *name).collect();
        let registered = sorted.clone();
        sorted.sort_unstable();
        assert_eq!(registered, sorted, "MIGRATIONS must stay in file name order");
    }

    /// The ledger bootstrap has to be applicable before anything can be recorded, so it must be
    /// the first entry and it must be safe to execute on every start.
    #[test]
    fn the_ledger_bootstrap_is_first_and_idempotent() {
        let (name, sql) = MIGRATIONS.first().expect("at least one migration is registered");
        assert_eq!(*name, "0000_schema_migrations.sql");
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS schema_migrations"));
    }

    /// The cutoff decides which files an existing database claims without executing them.
    /// It must name a real migration, and everything after it must actually run.
    #[test]
    fn the_adoption_cutoff_names_a_registered_migration() {
        assert!(
            MIGRATIONS.iter().any(|(name, _)| *name == MIGRATION_ADOPTION_CUTOFF),
            "adoption cutoff must be a registered migration"
        );
        let replayed: Vec<&str> = MIGRATIONS
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| *name > MIGRATION_ADOPTION_CUTOFF)
            .collect();
        assert_eq!(
            replayed,
            vec![
                "0048_delivery_lease_and_pickup_indexes.sql",
                "0049_agent_invocation_duplicate_tagging.sql"
            ],
            "only the idempotent delivery migrations may re-run on an adopted database"
        );
    }

    /// Adoption records a checksum without executing the file, so the checksum has to be a pure
    /// function of the content and has to separate two different files.
    #[test]
    fn migration_checksums_are_content_addressed() {
        assert_eq!(migration_checksum("select 1;"), migration_checksum("select 1;"));
        assert_ne!(migration_checksum("select 1;"), migration_checksum("select 2;"));
        assert_eq!(migration_checksum("").len(), 64);
    }

    /// 0048 used to delete rows on every API start. Whatever else it grows, it must never remove
    /// data again, and it must stay re-runnable because adopted databases execute it.
    #[test]
    fn the_delivery_migrations_never_delete_rows() {
        for (name, sql) in MIGRATIONS.iter().filter(|(name, _)| *name > MIGRATION_ADOPTION_CUTOFF) {
            let upper = sql.to_uppercase();
            assert!(
                !upper.contains("DELETE FROM"),
                "{name} must not delete rows: it re-runs on every adopted database"
            );
            assert!(
                !upper.contains("DROP TABLE"),
                "{name} must not drop tables: it re-runs on every adopted database"
            );
        }
    }
}
