#![allow(clippy::nursery, clippy::pedantic)]

use api::{middleware, response::ApiResponse, routes};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    middleware as axum_middleware,
    response::IntoResponse,
    routing::{delete, get, patch, post, put},
};
use clap::Parser;
use platform::{
    app::{AppState, connect_db},
    config::{AppConfig, MigrationsConfig, OpenPrConfig},
    logging,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    service: String,
}

/// Command line surface of the API binary.
///
/// The only thing the process accepts from outside the configuration file is where that file is.
#[derive(Parser)]
#[command(name = "api", about = "OpenPR API server")]
struct Args {
    /// Path to the configuration file. Defaults to config/openpr.toml
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let config = OpenPrConfig::load(args.config.as_deref())?;
    // Installed before the logger so that a configuration this process cannot publish aborts
    // startup rather than leaving half the service reading the fallback settings.
    api::config::install(&config).map_err(|err| anyhow::anyhow!("{err}"))?;
    logging::init(&config.logging, "api")?;

    // from_config first: it reports a missing database url and signing key together, so the
    // operator fixes both in one pass instead of being walked through them one at a time.
    let cfg = AppConfig::from_config(&config, "api", "0.0.0.0:8081")?;
    let db = connect_db(&config.database_runtime()?).await?;
    run_migrations(&db, config.migrations).await?;
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
    (
        "0050_proposal_workspace_scope.sql",
        include_str!("../../../migrations/0050_proposal_workspace_scope.sql"),
    ),
];

/// Newest migration an existing database may claim without executing it.
///
/// Frozen on purpose. Everything past it is executed on every database that has no ledger row for
/// it, which is what keeps a future migration from being adopted by accident because its probe
/// happens to match an object some other file created. The two files past the cutoff are
/// idempotent and a test enforces that they stay that way.
const MIGRATION_ADOPTION_CUTOFF: &str = "0047_form_attachment_media_metadata.sql";

/// Configuration key of the escape hatch that re-executes every migration once, reporting
/// failures without aborting.
///
/// Safe to use on a live database. A failure never downgrades a ledger row that already records
/// a success, so a replay of the non idempotent early migrations ("relation already exists") no
/// longer turns the next ordinary start into a permanent failure. Named here only so the warning
/// it prints tells the operator which line of the file to turn back off.
const MIGRATION_REPLAY_KEY: &str = "migrations.replay";

/// Configuration key of the escape hatch that starts the service even though a migration failed
/// or the schema check found a gap.
///
/// The degraded path an operator needs to get the service up and inspect it. The failure is
/// recorded and logged; nothing is skipped silently, and the next ordinary start retries.
const MIGRATION_CONTINUE_ON_ERROR_KEY: &str = "migrations.continue_on_error";

/// Advisory lock namespace for the migration runner. The first key separates this lock from any
/// other advisory lock in the system, the second one names the runner within that namespace.
const MIGRATION_LOCK_NAMESPACE: i32 = 0x0F09_2026;
const MIGRATION_LOCK_ID: i32 = 1;

/// Evidence that one migration is already present in a database created before the ledger.
///
/// The previous revision took this decision once for the whole list with
/// `to_regclass('public.users')`: every database that had a `users` table claimed all migrations
/// up to the cutoff. A half built database - 0001 succeeded years ago, 0030 kept failing and the
/// pre-ledger runner only warned - was therefore marked complete and its missing tables were
/// never created again. Adoption is now decided per file, so a gap heals by executing that one
/// file, and the same probes double as the post-run schema assertion.
///
/// A probe that wrongly reports "absent" only costs one execution of an idempotent file. A probe
/// that wrongly reports "present" would skip a migration forever, so every probe names an object
/// that its own migration creates.
#[derive(Clone, Copy)]
enum SchemaProbe {
    /// Table, index or view. Resolved through `search_path`, so a deployment that does not use
    /// the `public` schema is judged against its own schema instead of being told nothing exists.
    Relation(&'static str),
    /// Column on a table, by table and column name.
    Column(&'static str, &'static str),
    /// Label on an enum type.
    EnumLabel(&'static str, &'static str),
    /// Named constraint whose definition contains a marker the migration introduced.
    ConstraintContains(&'static str, &'static str, &'static str),
    /// No object is created that could be probed, but re-executing the file is a no-op, so it is
    /// executed rather than adopted.
    Rerunnable,
}

impl SchemaProbe {
    /// Statement returning a single `present` boolean, or `None` for [`SchemaProbe::Rerunnable`].
    ///
    /// Every identifier travels as a bind parameter, so nothing here is concatenated into SQL.
    fn statement(self) -> Option<Statement> {
        match self {
            Self::Relation(name) => Some(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT to_regclass($1) IS NOT NULL AS present",
                vec![name.into()],
            )),
            Self::Column(table, column) => Some(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    SELECT EXISTS (
                        SELECT 1 FROM pg_attribute
                        WHERE attrelid = to_regclass($1)
                          AND attname = $2
                          AND attnum > 0
                          AND NOT attisdropped
                    ) AS present
                ",
                vec![table.into(), column.into()],
            )),
            Self::EnumLabel(type_name, label) => Some(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    SELECT EXISTS (
                        SELECT 1 FROM pg_enum
                        WHERE enumtypid = to_regtype($1)
                          AND enumlabel = $2
                    ) AS present
                ",
                vec![type_name.into(), label.into()],
            )),
            Self::ConstraintContains(table, constraint, marker) => Some(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    SELECT EXISTS (
                        SELECT 1 FROM pg_constraint
                        WHERE conrelid = to_regclass($1)
                          AND conname = $2
                          AND position($3 IN pg_get_constraintdef(oid)) > 0
                    ) AS present
                ",
                vec![table.into(), constraint.into(), marker.into()],
            )),
            Self::Rerunnable => None,
        }
    }
}

/// One probe per migration, in the same order as [`MIGRATIONS`], including the ledger bootstrap.
/// A test keeps the two lists aligned so a new migration cannot be added without deciding how an
/// existing database recognises it.
const MIGRATION_PROBES: &[(&str, SchemaProbe)] = &[
    ("0000_schema_migrations.sql", SchemaProbe::Relation("schema_migrations")),
    ("0001_init.sql", SchemaProbe::Relation("scheduled_jobs")),
    ("0002_users.sql", SchemaProbe::Column("users", "name")),
    ("0003_labels.sql", SchemaProbe::Relation("work_item_labels")),
    ("0004_sprints.sql", SchemaProbe::Column("work_items", "sprint_id")),
    ("0005_webhooks.sql", SchemaProbe::Relation("webhook_deliveries")),
    (
        "0006_notifications.sql",
        SchemaProbe::Column("notifications", "metadata"),
    ),
    (
        "0007_fulltext_search.sql",
        SchemaProbe::Column("comments", "search_vector"),
    ),
    ("0008_admin_user_fields.sql", SchemaProbe::Column("users", "is_active")),
    (
        "0009_notifications_schema_compat.sql",
        SchemaProbe::Relation("idx_notifications_user_unread"),
    ),
    (
        "0010_issue_activity_notifications_compat.sql",
        SchemaProbe::Column("notifications", "workspace_id"),
    ),
    (
        "0011_bot_user_and_webhook_fields.sql",
        SchemaProbe::Column("webhooks", "bot_user_id"),
    ),
    (
        "0012_governance_phase1.sql",
        SchemaProbe::Relation("proposal_issue_links"),
    ),
    (
        "0013_governance_work_items_fields.sql",
        SchemaProbe::Column("work_items", "proposal_id"),
    ),
    // Reshapes columns 0012 may already have created, so every object it could be probed on
    // exists before it runs. Every branch is guarded and the unguarded statements are
    // `SET NOT NULL` and idempotent updates, so it is executed rather than adopted.
    ("0014_proposal_comments_schema_compat.sql", SchemaProbe::Rerunnable),
    ("0015_governance_phase2.sql", SchemaProbe::Relation("appeals")),
    (
        "0016_governance_phase3.sql",
        SchemaProbe::Relation("governance_audit_logs"),
    ),
    (
        "0017_governance_phase3_hardening.sql",
        SchemaProbe::Relation("uq_trust_score_logs_event_scope"),
    ),
    (
        "0018_governance_phase3_templates.sql",
        SchemaProbe::Column("proposals", "template_id"),
    ),
    (
        "0019_governance_cycle_template_rapid.sql",
        SchemaProbe::EnumLabel("cycle_template", "rapid"),
    ),
    ("0020_ai_tasks.sql", SchemaProbe::Relation("ai_task_events")),
    // Only rewrites foreign keys that are not already cascading, and every branch is guarded, so
    // there is nothing to probe and nothing to lose by executing it again.
    ("0021_fix_cascade.sql", SchemaProbe::Rerunnable),
    ("0022_bot_tokens.sql", SchemaProbe::Relation("workspace_bots")),
    (
        "0023_work_item_identifier.sql",
        SchemaProbe::Column("work_items", "sequence_number"),
    ),
    ("0024_workflow_config.sql", SchemaProbe::Relation("workflow_states")),
    (
        "0025_project_types_resources.sql",
        SchemaProbe::Relation("project_resources"),
    ),
    (
        "0026_connectors_invocations.sql",
        SchemaProbe::Relation("agent_invocations"),
    ),
    ("0027_check_results.sql", SchemaProbe::Relation("check_results")),
    (
        "0028_invocation_tool_calls.sql",
        SchemaProbe::Relation("agent_invocation_tool_calls"),
    ),
    (
        "0029_scenario_templates.sql",
        SchemaProbe::Relation("scenario_templates"),
    ),
    (
        "0030_universal_forms.sql",
        SchemaProbe::Relation("form_record_field_index"),
    ),
    (
        "0031_business_events_outbox_inbox.sql",
        SchemaProbe::Relation("event_inbox"),
    ),
    (
        "0032_print_device_connector_kinds.sql",
        SchemaProbe::ConstraintContains("connectors", "connectors_kind_check", "device"),
    ),
    ("0033_plugins_wasm.sql", SchemaProbe::Relation("plugin_invocations")),
    (
        "0034_user_profile_preferences.sql",
        SchemaProbe::Column("users", "notification_prefs"),
    ),
    // Seeds the two product lanes with ON CONFLICT DO UPDATE and retires the legacy keys; running
    // it again on a database that already has them changes nothing.
    ("0035_project_type_two_lanes.sql", SchemaProbe::Rerunnable),
    (
        "0036_universal_forms_schema_version_archive.sql",
        SchemaProbe::Relation("idx_form_field_index_field_id"),
    ),
    ("0037_form_attachments.sql", SchemaProbe::Relation("form_attachments")),
    ("0038_form_permissions.sql", SchemaProbe::Relation("form_permissions")),
    (
        "0039_universal_forms_pivot_view_type.sql",
        SchemaProbe::ConstraintContains("form_views", "form_views_type_check", "pivot"),
    ),
    (
        "0040_universal_forms_gantt_view_type.sql",
        SchemaProbe::ConstraintContains("form_views", "form_views_type_check", "gantt"),
    ),
    (
        "0041_form_record_comments.sql",
        SchemaProbe::Relation("form_record_comments"),
    ),
    (
        "0042_form_attachment_thumbnail_url.sql",
        SchemaProbe::Column("form_attachments", "thumbnail_url"),
    ),
    (
        "0043_form_import_mapping_templates.sql",
        SchemaProbe::Relation("form_import_mapping_templates"),
    ),
    ("0044_form_import_jobs.sql", SchemaProbe::Relation("form_import_jobs")),
    ("0045_form_export_jobs.sql", SchemaProbe::Relation("form_export_jobs")),
    (
        "0046_form_attachment_package_jobs.sql",
        SchemaProbe::Relation("form_attachment_package_jobs"),
    ),
    (
        "0047_form_attachment_media_metadata.sql",
        SchemaProbe::Column("form_attachments", "media_metadata"),
    ),
    (
        "0048_delivery_lease_and_pickup_indexes.sql",
        SchemaProbe::Column("event_outbox", "lease_token"),
    ),
    (
        "0049_agent_invocation_duplicate_tagging.sql",
        SchemaProbe::Column("agent_invocations", "duplicate_of"),
    ),
    (
        "0050_proposal_workspace_scope.sql",
        SchemaProbe::Column("proposals", "workspace_id"),
    ),
];

/// One recorded migration outcome.
struct RecordedMigration {
    checksum: String,
    status: String,
}

/// What the runner did with one migration.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MigrationOutcome {
    /// The ledger already records a success.
    Skipped,
    /// Recognised in a database that predates the ledger and recorded without executing it.
    Adopted,
    /// Executed and recorded in the same transaction.
    Applied,
}

/// Recovery switches for one migration run.
///
/// Kept as a value rather than read from the environment deep inside the runner, so both branches
/// can be exercised by a test without mutating process wide state.
#[derive(Clone, Copy, Default)]
struct MigrationOptions {
    /// Re-execute every migration, including those the ledger already records as successful.
    replay: bool,
    /// Start even though a migration failed or the post-run schema check found a gap.
    continue_on_error: bool,
}

impl MigrationOptions {
    /// Maps the `[migrations]` section onto the runner's switches.
    ///
    /// A separate function so the mapping is testable without a database: the two switches have
    /// the same type and opposite consequences, and swapping them would otherwise only show up
    /// on a deployment that had asked for one of them.
    const fn from_config(migrations: MigrationsConfig) -> Self {
        Self {
            replay: migrations.replay,
            continue_on_error: migrations.continue_on_error,
        }
    }
}

async fn run_migrations(db: &DatabaseConnection, migrations: MigrationsConfig) -> anyhow::Result<()> {
    run_migrations_with(db, MigrationOptions::from_config(migrations)).await
}

async fn run_migrations_with(db: &DatabaseConnection, options: MigrationOptions) -> anyhow::Result<()> {
    let Some((ledger_name, ledger_sql)) = MIGRATIONS.first() else {
        return Err(anyhow::anyhow!("migration list is empty, refusing to start"));
    };

    // The ledger bootstrap is idempotent by construction, so it is the one file that may run on
    // every start: nothing can be read from or written to the ledger before it exists. It still
    // takes the runner lock, because two replicas issuing `CREATE TABLE IF NOT EXISTS` at the
    // same instant can collide on the catalogue instead of one of them seeing the finished table.
    bootstrap_migration_ledger(db, ledger_name, ledger_sql)
        .await
        .map_err(|err| anyhow::anyhow!("migration ledger bootstrap failed: {err}"))?;
    record_migration_if_absent(db, ledger_name, &migration_checksum(ledger_sql)).await?;

    let MigrationOptions {
        replay,
        continue_on_error,
    } = options;
    if replay {
        tracing::warn!(
            setting = MIGRATION_REPLAY_KEY,
            "migration replay requested: every migration is re-executed, failures are reported \
             but not fatal, and a ledger row that already records a success is never downgraded"
        );
    }
    if continue_on_error {
        tracing::warn!(
            setting = MIGRATION_CONTINUE_ON_ERROR_KEY,
            "starting even if a migration fails or the schema check finds a gap; the failures are \
             recorded in schema_migrations and retried on the next start"
        );
    }
    let tolerate_failure = replay || continue_on_error;

    let mut adopted = 0_usize;
    let mut applied = 0_usize;
    let mut failed: Vec<&str> = Vec::new();

    for (name, sql) in MIGRATIONS.iter().skip(1) {
        match run_one_migration(db, name, sql, replay).await {
            Ok(MigrationOutcome::Skipped) => {}
            Ok(MigrationOutcome::Adopted) => {
                adopted += 1;
                tracing::info!(migration = %name, "migration recognised in an existing database and adopted");
            }
            Ok(MigrationOutcome::Applied) => {
                applied += 1;
                tracing::info!(migration = %name, "migration applied");
            }
            Err(err) => {
                failed.push(name);
                tracing::error!(migration = %name, error = %err, "migration failed");
                if !tolerate_failure {
                    return Err(anyhow::anyhow!("migration {name} failed: {err}"));
                }
            }
        }
    }

    if adopted > 0 {
        tracing::warn!(
            adopted,
            "migrations were adopted from an existing database without executing them; the schema \
             check below decides whether that database is actually complete"
        );
    }
    tracing::info!(applied, adopted, failed = failed.len(), "migration run finished");

    verify_recorded_migrations(db, tolerate_failure).await
}

/// Runs one migration under the runner lock, deciding inside the transaction that will also write
/// the ledger row. Decision, execution and bookkeeping are therefore one atomic step: an
/// interrupted run leaves no half claimed ledger, and a second replica re-reads the ledger under
/// the same lock instead of acting on a snapshot taken before it was blocked.
async fn run_one_migration(
    db: &DatabaseConnection,
    name: &str,
    sql: &str,
    replay: bool,
) -> anyhow::Result<MigrationOutcome> {
    let checksum = migration_checksum(sql);
    let txn = db.begin().await?;
    lock_migration_runner(&txn).await?;

    let recorded = load_recorded_migration(&txn, name).await?;
    let succeeded_before = recorded.as_ref().is_some_and(|row| row.status != "failed");

    if !replay && let Some(row) = &recorded {
        if succeeded_before {
            if row.checksum != checksum {
                tracing::warn!(
                    migration = %name,
                    recorded_status = %row.status,
                    "migration file changed after it was recorded; the database still holds the \
                     previously applied version"
                );
            }
            txn.commit().await?;
            return Ok(MigrationOutcome::Skipped);
        }
        tracing::warn!(
            migration = %name,
            "migration is recorded as failed; retrying it"
        );
    }

    if !replay && recorded.is_none() && name <= MIGRATION_ADOPTION_CUTOFF && migration_is_present(&txn, name).await? {
        record_migration_with_conn(&txn, name, &checksum, "adopted", None).await?;
        txn.commit().await?;
        return Ok(MigrationOutcome::Adopted);
    }

    if let Err(err) = txn.execute_unprepared(sql).await {
        if let Err(rollback_err) = txn.rollback().await {
            tracing::warn!(migration = %name, error = %rollback_err, "migration rollback failed");
        }
        let message = err.to_string();
        // A replay re-executes files that already ran, so "relation already exists" is the
        // expected answer there. Downgrading the ledger row to `failed` for those was what turned
        // a single use of the replay switch into a database that could never start normally
        // again, because an ordinary start retries `failed` rows and fails on the same statement.
        if succeeded_before {
            tracing::warn!(
                migration = %name,
                error = %message,
                "re-execution failed; the ledger keeps the recorded success"
            );
        } else if let Err(record_err) = record_migration(db, name, &checksum, "failed", Some(&message)).await {
            tracing::error!(
                migration = %name,
                error = %record_err,
                "could not record the migration failure in the ledger"
            );
        }
        return Err(anyhow::anyhow!("{message}"));
    }

    record_migration_with_conn(&txn, name, &checksum, "applied", None).await?;
    txn.commit().await?;
    Ok(MigrationOutcome::Applied)
}

/// Creates the ledger table itself, under the runner lock so concurrent starts serialise.
async fn bootstrap_migration_ledger(db: &DatabaseConnection, name: &str, sql: &str) -> anyhow::Result<()> {
    let txn = db.begin().await?;
    lock_migration_runner(&txn).await?;
    if let Err(err) = txn.execute_unprepared(sql).await {
        if let Err(rollback_err) = txn.rollback().await {
            tracing::warn!(migration = %name, error = %rollback_err, "migration rollback failed");
        }
        return Err(err.into());
    }
    txn.commit().await?;
    Ok(())
}

/// Takes the transaction scoped runner lock. It is released by the commit or the rollback that
/// ends the transaction, so it can never be left behind on a pooled connection.
async fn lock_migration_runner<C: ConnectionTrait>(conn: &C) -> anyhow::Result<()> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock($1, $2)",
        vec![MIGRATION_LOCK_NAMESPACE.into(), MIGRATION_LOCK_ID.into()],
    ))
    .await?;
    Ok(())
}

/// Content fingerprint used to notice that a migration file changed after it was applied.
fn migration_checksum(sql: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sql.as_bytes());
    hex::encode(hasher.finalize())
}

fn migration_probe(name: &str) -> Option<SchemaProbe> {
    MIGRATION_PROBES
        .iter()
        .find(|(probe_name, _)| *probe_name == name)
        .map(|(_, probe)| *probe)
}

/// Whether the objects a migration creates are already in the database.
///
/// `Rerunnable` migrations and migrations without a probe answer "no", which costs one execution
/// and never skips work.
async fn migration_is_present<C: ConnectionTrait>(conn: &C, name: &str) -> anyhow::Result<bool> {
    let Some(statement) = migration_probe(name).and_then(SchemaProbe::statement) else {
        return Ok(false);
    };
    let Some(row) = conn.query_one(statement).await? else {
        return Ok(false);
    };
    Ok(row.try_get::<bool>("", "present")?)
}

/// Confirms that every migration the ledger reports as successful really left its objects behind.
///
/// This is the assertion the previous revision did not have. `verify_governance_schema` only
/// covers governance and `verify_delivery_schema` only covers the two delivery migrations, so a
/// database that claimed 0001..0047 while the forms tables were missing started up healthy and
/// answered 500 for every forms request. Every migration is checked here instead.
async fn verify_recorded_migrations(db: &DatabaseConnection, tolerate_failure: bool) -> anyhow::Result<()> {
    let ledger = load_migration_ledger(db).await?;
    let mut missing: Vec<&str> = Vec::new();

    for (name, _) in MIGRATIONS {
        let Some(recorded) = ledger.get(*name) else {
            continue;
        };
        if recorded.status == "failed" {
            continue;
        }
        if migration_probe(name).is_some_and(|probe| matches!(probe, SchemaProbe::Rerunnable)) {
            continue;
        }
        if !migration_is_present(db, name).await? {
            missing.push(name);
        }
    }

    if missing.is_empty() {
        return Ok(());
    }

    let names = missing.join(", ");
    if tolerate_failure {
        tracing::error!(
            migrations = %names,
            "the ledger reports these migrations as applied but their objects are missing; \
             starting anyway because a migration escape hatch is set"
        );
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "the migration ledger reports {names} as applied but their objects are missing; re-run \
         with {MIGRATION_REPLAY_KEY} = true in the configuration file to re-execute every \
         migration, or start with {MIGRATION_CONTINUE_ON_ERROR_KEY} = true to inspect the database"
    ))
}

async fn record_migration(
    db: &DatabaseConnection,
    name: &str,
    checksum: &str,
    status: &str,
    error: Option<&str>,
) -> anyhow::Result<()> {
    record_migration_with_conn(db, name, checksum, status, error).await
}

async fn record_migration_with_conn<C: ConnectionTrait>(
    conn: &C,
    name: &str,
    checksum: &str,
    status: &str,
    error: Option<&str>,
) -> anyhow::Result<()> {
    conn.execute(Statement::from_sql_and_values(
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

async fn load_recorded_migration<C: ConnectionTrait>(
    conn: &C,
    name: &str,
) -> anyhow::Result<Option<RecordedMigration>> {
    let row = conn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT checksum, status FROM schema_migrations WHERE name = $1",
            vec![name.into()],
        ))
        .await?;
    match row {
        Some(row) => Ok(Some(RecordedMigration {
            checksum: row.try_get::<String>("", "checksum")?,
            status: row.try_get::<String>("", "status")?,
        })),
        None => Ok(None),
    }
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
#[allow(clippy::indexing_slicing, clippy::print_stderr)]
mod tests {
    use super::{MIGRATION_ADOPTION_CUTOFF, MIGRATION_PROBES, MIGRATIONS, MigrationOptions, migration_checksum};
    use platform::config::MigrationsConfig;

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
                "0049_agent_invocation_duplicate_tagging.sql",
                "0050_proposal_workspace_scope.sql"
            ],
            "everything past the cutoff re-runs on an adopted database and must be idempotent"
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

    /// Every migration needs a probe, otherwise an existing database either replays it (which the
    /// early non idempotent files do not survive) or, worse, a future migration falls back on a
    /// probe belonging to another file. The two lists are kept in lockstep here.
    #[test]
    fn every_migration_has_a_schema_probe() {
        let migrations: Vec<&str> = MIGRATIONS.iter().map(|(name, _)| *name).collect();
        let probed: Vec<&str> = MIGRATION_PROBES.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            migrations, probed,
            "MIGRATIONS and MIGRATION_PROBES must list the same files in the same order"
        );
    }

    /// A probe has to produce a statement, except for the two files that create nothing and are
    /// executed rather than adopted.
    #[test]
    fn only_rerunnable_probes_have_no_statement() {
        let without: Vec<&str> = MIGRATION_PROBES
            .iter()
            .filter(|(_, probe)| probe.statement().is_none())
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(
            without,
            vec![
                "0014_proposal_comments_schema_compat.sql",
                "0021_fix_cascade.sql",
                "0035_project_type_two_lanes.sql"
            ],
            "a migration may only skip its probe when re-executing it is a no-op"
        );
    }

    /// Probes must resolve relations through search_path. A hardcoded `public.` prefix reports
    /// "absent" on a deployment that does not use the public schema, which sends the runner into
    /// replaying the early non idempotent migrations.
    #[test]
    fn probes_do_not_hardcode_the_public_schema() {
        for (name, probe) in MIGRATION_PROBES {
            let Some(statement) = probe.statement() else {
                continue;
            };
            let rendered = format!("{statement:?}");
            assert!(
                !rendered.contains("public."),
                "{name} probe must not hardcode the public schema"
            );
        }
    }

    #[test]
    fn migration_escape_hatches_are_off_until_the_file_asks_for_them() {
        let quiet = MigrationOptions::from_config(MigrationsConfig::default());
        assert!(!quiet.replay);
        assert!(!quiet.continue_on_error);

        // Each switch is read from its own key. Asserted one at a time so a swapped mapping
        // cannot pass by setting both.
        let replaying = MigrationOptions::from_config(MigrationsConfig {
            replay: true,
            continue_on_error: false,
        });
        assert!(replaying.replay);
        assert!(!replaying.continue_on_error);

        let tolerant = MigrationOptions::from_config(MigrationsConfig {
            replay: false,
            continue_on_error: true,
        });
        assert!(!tolerant.replay);
        assert!(tolerant.continue_on_error);
    }

    /// Routes are authenticated one by one with `route_layer`, so forgetting one is invisible
    /// until somebody calls it. Four proposal reads were unauthenticated that way, two of which
    /// finalized voting and wrote decisions. The router source is parsed here and every route
    /// without a layer has to be on the list of endpoints that are public on purpose.
    #[test]
    fn only_the_intended_routes_are_reachable_without_authentication() {
        /// Endpoints that must answer without a session, and why.
        const PUBLIC_ROUTES: &[&str] = &[
            // Signed, expiring download links; the handler verifies the HMAC itself.
            "/api/v1/uploads/signatures/{file_name}/download",
            "/api/v1/form-attachments/{attachment_id}/download",
            // Liveness and readiness probes.
            "/health",
            "/ready",
            // Credential entry points; there is no session to present yet.
            "/api/v1/auth/register",
            "/api/v1/auth/login",
            "/api/v1/auth/refresh",
        ];

        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
            .expect("the router source is readable");

        let mut unauthenticated: Vec<String> = Vec::new();
        let mut block: Option<String> = None;
        let mut in_router = false;

        let finish = |block: &str, out: &mut Vec<String>| {
            if block.contains("route_layer") {
                return;
            }
            if let Some(path) = block.split('"').nth(1) {
                out.push(path.to_string());
            }
        };

        for line in source.lines() {
            if line.contains("let app = Router::new()") {
                in_router = true;
                continue;
            }
            if !in_router {
                continue;
            }
            let starts_route = line.starts_with("        .route(");
            let ends_router = line.starts_with("        .layer(");
            if starts_route || ends_router {
                if let Some(previous) = block.take() {
                    finish(&previous, &mut unauthenticated);
                }
                if ends_router {
                    in_router = false;
                    continue;
                }
                block = Some(String::new());
            }
            if let Some(current) = block.as_mut() {
                current.push_str(line);
                current.push('\n');
            }
        }
        if let Some(previous) = block.take() {
            finish(&previous, &mut unauthenticated);
        }

        assert!(
            !unauthenticated.is_empty(),
            "the router parser found nothing; it no longer matches main.rs"
        );
        let mut expected: Vec<String> = PUBLIC_ROUTES.iter().map(|path| (*path).to_string()).collect();
        expected.sort();
        let mut found = unauthenticated;
        found.sort();
        assert_eq!(
            found, expected,
            "a route was registered without an authentication layer; add the layer, or add the \
             path to PUBLIC_ROUTES with the reason it is public"
        );
    }
}

/// Migration runner behaviour against a real PostgreSQL server.
///
/// Adoption, replay and retry only differ on databases that are not empty, so a fresh compose
/// database exercises none of them. Each test here creates its own database, shapes it into the
/// state under test and runs the real runner against it.
///
/// Set `OPENPR_TEST_DATABASE_URL` to a maintenance connection string, for example
/// `postgres://user:pw@127.0.0.1:5432/postgres`. Without it these tests report that they were
/// skipped instead of pretending to pass.
#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::print_stderr)]
mod migration_runner_database_tests {
    use super::{MIGRATIONS, MigrationOptions, migration_is_present, run_migrations_with};
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
    use std::collections::HashMap;

    const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

    /// A throwaway database plus the connection string needed to reach it again.
    pub(super) struct Scratch {
        db: DatabaseConnection,
        url: String,
        name: String,
        admin_url: String,
    }

    impl Scratch {
        /// Connection to the scratch database.
        pub(super) fn connection(&self) -> &DatabaseConnection {
            &self.db
        }

        /// Connection string of the scratch database.
        pub(super) fn url(&self) -> &str {
            &self.url
        }

        /// Second connection to the same database, for the concurrent start test.
        async fn second_connection(&self) -> anyhow::Result<DatabaseConnection> {
            Ok(Database::connect(&self.url).await?)
        }

        pub(super) async fn drop_self(self) {
            let Self {
                db, name, admin_url, ..
            } = self;
            drop(db);
            let Ok(admin) = Database::connect(&admin_url).await else {
                return;
            };
            let statement = format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)");
            if let Err(err) = admin.execute_unprepared(&statement).await {
                eprintln!("could not drop scratch database {name}: {err}");
            }
        }
    }

    /// Creates an empty database named after the test. Returns `None` when the environment does
    /// not offer a server, so the suite stays runnable without one.
    pub(super) async fn scratch(label: &str) -> Option<Scratch> {
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
        let admin = match Database::connect(&admin_url).await {
            Ok(admin) => admin,
            Err(err) => panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"),
        };

        let name = format!("openpr_mig_{label}");
        let quoted = format!("\"{name}\"");
        if let Err(err) = admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS {quoted} WITH (FORCE)"))
            .await
        {
            panic!("could not reset scratch database {name}: {err}");
        }
        if let Err(err) = admin.execute_unprepared(&format!("CREATE DATABASE {quoted}")).await {
            panic!("could not create scratch database {name}: {err}");
        }

        let (prefix, _) = admin_url.rsplit_once('/')?;
        let url = format!("{prefix}/{name}");
        let db = match Database::connect(&url).await {
            Ok(db) => db,
            Err(err) => panic!("could not connect to scratch database {name}: {err}"),
        };
        Some(Scratch {
            db,
            url,
            name,
            admin_url,
        })
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

    /// Reproduces the pre-ledger runner: every file executed straight against the database with
    /// no ledger to record it. `stop_before` leaves the database half built.
    async fn seed_pre_ledger_schema(db: &DatabaseConnection, stop_before: Option<&str>) {
        for (name, sql) in MIGRATIONS.iter().skip(1) {
            if stop_before.is_some_and(|limit| *name >= limit) {
                break;
            }
            if let Err(err) = db.execute_unprepared(sql).await {
                panic!("seeding {name} failed: {err}");
            }
        }
    }

    async fn ledger(db: &DatabaseConnection) -> HashMap<String, String> {
        let rows = db
            .query_all(Statement::from_string(
                DbBackend::Postgres,
                "SELECT name, status FROM schema_migrations".to_string(),
            ))
            .await
            .expect("the ledger is readable");
        let mut out = HashMap::with_capacity(rows.len());
        for row in rows {
            let name: String = row.try_get("", "name").expect("name column");
            let status: String = row.try_get("", "status").expect("status column");
            out.insert(name, status);
        }
        out
    }

    fn status_of<'a>(ledger: &'a HashMap<String, String>, name: &str) -> &'a str {
        ledger.get(name).map_or("<absent>", String::as_str)
    }

    /// Every object the ledger claims is present has to be there.
    async fn assert_schema_complete(db: &DatabaseConnection) {
        for (name, _) in MIGRATIONS {
            let present = migration_is_present(db, name).await.expect("probe runs");
            let probe_exists = super::migration_probe(name).is_some_and(|probe| probe.statement().is_some());
            assert!(!probe_exists || present, "{name} left no trace in the database");
        }
    }

    #[tokio::test]
    async fn a_fresh_database_applies_every_migration() {
        let scratch = scratch_or_skip!("fresh");

        run_migrations_with(&scratch.db, MigrationOptions::default())
            .await
            .expect("a fresh database migrates cleanly");

        let ledger = ledger(&scratch.db).await;
        assert_eq!(ledger.len(), MIGRATIONS.len(), "every migration is recorded");
        for (name, _) in MIGRATIONS {
            assert_eq!(status_of(&ledger, name), "applied", "{name} should have been executed");
        }
        assert_schema_complete(&scratch.db).await;

        scratch.drop_self().await;
    }

    /// A database built by the pre-ledger runner keeps its schema: the early files, which are not
    /// idempotent, must be recognised rather than replayed.
    #[tokio::test]
    async fn an_existing_database_is_adopted_rather_than_replayed() {
        let scratch = scratch_or_skip!("existing");
        seed_pre_ledger_schema(&scratch.db, None).await;

        run_migrations_with(&scratch.db, MigrationOptions::default())
            .await
            .expect("an existing database migrates cleanly");

        let ledger = ledger(&scratch.db).await;
        assert_eq!(status_of(&ledger, "0001_init.sql"), "adopted");
        assert_eq!(status_of(&ledger, "0005_webhooks.sql"), "adopted");
        assert_eq!(status_of(&ledger, "0022_bot_tokens.sql"), "adopted");
        assert_eq!(status_of(&ledger, "0038_form_permissions.sql"), "adopted");
        // Past the frozen cutoff nothing is ever adopted; both files are idempotent and re-run.
        assert_eq!(
            status_of(&ledger, "0048_delivery_lease_and_pickup_indexes.sql"),
            "applied"
        );
        assert!(
            !ledger.values().any(|status| status == "failed"),
            "no migration may fail on an existing database"
        );
        assert_schema_complete(&scratch.db).await;

        scratch.drop_self().await;
    }

    /// The regression that made the previous revision dangerous: a database whose early files
    /// succeeded and whose forms files never did. The blanket `to_regclass('public.users')`
    /// predicate claimed all of them, so the missing tables were never created again. Per file
    /// probes have to adopt what is there and execute what is not.
    #[tokio::test]
    async fn a_half_built_database_only_adopts_what_it_actually_has() {
        let scratch = scratch_or_skip!("half_built");
        seed_pre_ledger_schema(&scratch.db, Some("0030_universal_forms.sql")).await;

        assert!(
            !migration_is_present(&scratch.db, "0038_form_permissions.sql")
                .await
                .expect("probe runs"),
            "the fixture must start without the forms tables"
        );

        run_migrations_with(&scratch.db, MigrationOptions::default())
            .await
            .expect("a half built database heals");

        let ledger = ledger(&scratch.db).await;
        assert_eq!(status_of(&ledger, "0001_init.sql"), "adopted");
        assert_eq!(status_of(&ledger, "0029_scenario_templates.sql"), "adopted");
        assert_eq!(status_of(&ledger, "0030_universal_forms.sql"), "applied");
        assert_eq!(status_of(&ledger, "0038_form_permissions.sql"), "applied");
        assert!(
            migration_is_present(&scratch.db, "0038_form_permissions.sql")
                .await
                .expect("probe runs"),
            "the forms tables have to exist after the run"
        );
        assert_schema_complete(&scratch.db).await;

        scratch.drop_self().await;
    }

    /// Adoption used to be a bare loop entered only while the ledger was empty, so an
    /// interruption left it half claimed and the next start executed the remaining early files
    /// against a database that already had them. Adoption is now re-derived per file.
    #[tokio::test]
    async fn an_interrupted_adoption_resumes_on_the_next_start() {
        let scratch = scratch_or_skip!("interrupted");
        seed_pre_ledger_schema(&scratch.db, None).await;

        run_migrations_with(&scratch.db, MigrationOptions::default())
            .await
            .expect("first run adopts the database");

        // Same shape as an adoption killed after 0004: a ledger with a handful of rows in it.
        scratch
            .db
            .execute_unprepared("DELETE FROM schema_migrations WHERE name > '0004'")
            .await
            .expect("the ledger is writable");

        run_migrations_with(&scratch.db, MigrationOptions::default())
            .await
            .expect("the interrupted adoption resumes instead of replaying");

        let ledger = ledger(&scratch.db).await;
        assert_eq!(
            status_of(&ledger, "0005_webhooks.sql"),
            "adopted",
            "0005 creates webhooks without IF NOT EXISTS; executing it here would fail"
        );
        assert_eq!(status_of(&ledger, "0022_bot_tokens.sql"), "adopted");
        assert!(!ledger.values().any(|status| status == "failed"));

        scratch.drop_self().await;
    }

    /// The escape hatch used to be a trap: one replay recorded the non idempotent files as
    /// `failed`, and an ordinary start retries `failed` rows, so the API could never start again
    /// without the switch. A replay may now report failures but must never downgrade a recorded
    /// success, and the ordinary start afterwards has to work.
    #[tokio::test]
    async fn a_replay_never_leaves_the_database_unable_to_start() {
        let scratch = scratch_or_skip!("replay");
        seed_pre_ledger_schema(&scratch.db, None).await;
        run_migrations_with(&scratch.db, MigrationOptions::default())
            .await
            .expect("first run adopts the database");

        run_migrations_with(
            &scratch.db,
            MigrationOptions {
                replay: true,
                continue_on_error: false,
            },
        )
        .await
        .expect("a replay reports failures without aborting");

        let after_replay = ledger(&scratch.db).await;
        assert!(
            !after_replay.values().any(|status| status == "failed"),
            "a replay must not downgrade a recorded success to failed"
        );

        run_migrations_with(&scratch.db, MigrationOptions::default())
            .await
            .expect("an ordinary start after a replay still works");
        assert_schema_complete(&scratch.db).await;

        scratch.drop_self().await;
    }

    /// A genuine failure aborts the start, is recorded with its error, and is retried once the
    /// cause is gone.
    #[tokio::test]
    async fn a_failing_migration_aborts_the_start_and_is_retried_later() {
        let scratch = scratch_or_skip!("failing");
        // 0005 creates `webhooks` without IF NOT EXISTS, so an unrelated table of that name is
        // enough to make exactly one migration fail.
        scratch
            .db
            .execute_unprepared("CREATE TABLE webhooks (blocker integer)")
            .await
            .expect("the blocker table is created");

        let err = run_migrations_with(&scratch.db, MigrationOptions::default())
            .await
            .expect_err("a failing migration must abort the start");
        assert!(
            err.to_string().contains("0005_webhooks.sql"),
            "the error has to name the migration, got: {err}"
        );

        let after_failure = ledger(&scratch.db).await;
        assert_eq!(status_of(&after_failure, "0005_webhooks.sql"), "failed");
        assert_eq!(status_of(&after_failure, "0004_sprints.sql"), "applied");

        scratch
            .db
            .execute_unprepared("DROP TABLE webhooks")
            .await
            .expect("the blocker table is dropped");
        run_migrations_with(&scratch.db, MigrationOptions::default())
            .await
            .expect("the retry succeeds once the cause is gone");
        let after_retry = ledger(&scratch.db).await;
        assert_eq!(status_of(&after_retry, "0005_webhooks.sql"), "applied");
        assert_schema_complete(&scratch.db).await;

        scratch.drop_self().await;
    }

    /// The degradation channel an operator needs: start with the failure recorded and visible,
    /// rather than being locked out of a database that cannot be inspected.
    #[tokio::test]
    async fn continue_on_error_starts_despite_a_failed_migration() {
        let scratch = scratch_or_skip!("continue_on_error");
        scratch
            .db
            .execute_unprepared("CREATE TABLE webhooks (blocker integer)")
            .await
            .expect("the blocker table is created");

        run_migrations_with(
            &scratch.db,
            MigrationOptions {
                replay: false,
                continue_on_error: true,
            },
        )
        .await
        .expect("the degraded start comes up");

        let ledger = ledger(&scratch.db).await;
        assert_eq!(status_of(&ledger, "0005_webhooks.sql"), "failed");
        assert_eq!(
            status_of(&ledger, "0030_universal_forms.sql"),
            "applied",
            "the run continues past the failure instead of stopping"
        );

        scratch.drop_self().await;
    }

    /// A ledger row is a claim, not proof. When the object it claims is gone the start has to
    /// stop and name it, because the previous revision started up healthy with the forms tables
    /// missing and answered 500 for every forms request instead.
    #[tokio::test]
    async fn a_ledger_claim_without_the_object_stops_the_start() {
        let scratch = scratch_or_skip!("gap");
        run_migrations_with(&scratch.db, MigrationOptions::default())
            .await
            .expect("a fresh database migrates cleanly");

        scratch
            .db
            .execute_unprepared("DROP TABLE form_permissions CASCADE")
            .await
            .expect("the forms table is dropped");

        let err = run_migrations_with(&scratch.db, MigrationOptions::default())
            .await
            .expect_err("a missing object must stop the start");
        assert!(
            err.to_string().contains("0038_form_permissions.sql"),
            "the error has to name the migration, got: {err}"
        );

        run_migrations_with(
            &scratch.db,
            MigrationOptions {
                replay: false,
                continue_on_error: true,
            },
        )
        .await
        .expect("the operator can still start to inspect the database");

        scratch.drop_self().await;
    }

    /// api and worker start together and both run the migrations. Without the advisory lock two
    /// runners raced on the same DDL and one of them died on "relation already exists"; with
    /// `restart: unless-stopped` in front of them that is a crash loop.
    #[tokio::test]
    async fn two_runners_starting_together_both_succeed() {
        let scratch = scratch_or_skip!("concurrent");
        let second = scratch.second_connection().await.expect("a second connection opens");

        let (first_result, second_result) = tokio::join!(
            run_migrations_with(&scratch.db, MigrationOptions::default()),
            run_migrations_with(&second, MigrationOptions::default()),
        );
        first_result.expect("the first runner succeeds");
        second_result.expect("the second runner succeeds");

        let ledger = ledger(&scratch.db).await;
        assert_eq!(ledger.len(), MIGRATIONS.len());
        assert!(!ledger.values().any(|status| status == "failed"));
        assert_schema_complete(&scratch.db).await;

        drop(second);
        scratch.drop_self().await;
    }

    /// The bootstrap used to resolve the ledger with `'public.schema_migrations'::regclass`,
    /// which raises rather than returning NULL, so a deployment whose search_path does not
    /// include public could not even create the ledger.
    #[tokio::test]
    async fn a_database_outside_the_public_schema_migrates() {
        let scratch = scratch_or_skip!("other_schema");
        scratch
            .db
            .execute_unprepared("CREATE SCHEMA app")
            .await
            .expect("the schema is created");
        scratch
            .db
            .execute_unprepared(&format!("ALTER DATABASE \"{}\" SET search_path TO app", scratch.name))
            .await
            .expect("the search_path is set");

        let relocated = scratch.second_connection().await.expect("a fresh connection opens");
        run_migrations_with(&relocated, MigrationOptions::default())
            .await
            .expect("a non public schema migrates cleanly");
        assert_schema_complete(&relocated).await;

        drop(relocated);
        scratch.drop_self().await;
    }
}

/// Proposal endpoints against a real PostgreSQL server.
///
/// Two properties are checked here that no unit test can reach: a proposal belonging to another
/// workspace is not readable, and reading a proposal whose voting window has expired no longer
/// settles it. Settlement belongs to the worker tick, which is driven explicitly.
///
/// Uses the same `OPENPR_TEST_DATABASE_URL` as the migration tests.
#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::print_stderr)]
mod proposal_scope_database_tests {
    use super::migration_runner_database_tests::{Scratch, scratch};
    use super::{MigrationOptions, run_migrations_with};
    use api::routes::proposal::{ListProposalsQuery, get_proposal, governance_tick, list_proposals};
    use axum::extract::{Extension, Query, State};
    use axum::response::IntoResponse;
    use chrono::{Duration, Utc};
    use platform::{
        app::AppState,
        auth::{JwtClaims, TokenType},
        config::{AppConfig, Secret},
    };
    use sea_orm::{ConnectionTrait, DbBackend, Statement};
    use uuid::Uuid;

    macro_rules! scratch_or_skip {
        ($label:expr) => {
            match scratch($label).await {
                Some(scratch) => scratch,
                None => {
                    eprintln!("skipped: OPENPR_TEST_DATABASE_URL is not set");
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
                database_url: Secret::new(scratch.url()),
                jwt_secret: Secret::new("proposal-scope-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
            },
            db: scratch.connection().clone(),
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

    fn empty_query() -> ListProposalsQuery {
        ListProposalsQuery {
            status: None,
            proposal_type: None,
            domain: None,
            page: None,
            per_page: Some(100),
            sort: None,
        }
    }

    async fn body_of(response: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("the response body is readable");
        serde_json::from_slice(&bytes).expect("the response body is JSON")
    }

    /// One workspace with one member and one proposal in it.
    struct Tenant {
        workspace_id: Uuid,
        user_id: Uuid,
        proposal_id: String,
    }

    async fn seed_tenant(state: &AppState, label: &str, status: &str) -> Tenant {
        let workspace_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let proposal_id = format!("PROP-{label}-{}", Uuid::new_v4());

        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO users (id, email, name, password_hash, role) VALUES ($1, $2, $3, 'x', 'user')",
                vec![user_id.into(), format!("{label}@example.test").into(), label.into()],
            ))
            .await
            .expect("the user is created");
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
                "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
                vec![workspace_id.into(), user_id.into()],
            ))
            .await
            .expect("the membership is created");
        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                    INSERT INTO proposals (
                        id, title, proposal_type, status, author_id, author_type, content,
                        domains, voting_rule, cycle_template, workspace_id,
                        created_at, submitted_at, voting_started_at
                    ) VALUES (
                        $1, $2, 'feature'::proposal_type, $3::proposal_status, $4, 'human'::author_type, $5,
                        '["product"]'::jsonb, 'simple_majority'::voting_rule, 'rapid'::cycle_template, $6,
                        $7, $7, $8
                    )
                "#,
                vec![
                    proposal_id.clone().into(),
                    format!("proposal for {label}").into(),
                    status.into(),
                    user_id.to_string().into(),
                    "content that is comfortably longer than the fifty character minimum".into(),
                    workspace_id.into(),
                    Utc::now().into(),
                    (Utc::now() - Duration::hours(48)).into(),
                ],
            ))
            .await
            .expect("the proposal is created");

        Tenant {
            workspace_id,
            user_id,
            proposal_id,
        }
    }

    #[derive(sea_orm::FromQueryResult)]
    struct CountRow {
        count: i64,
    }

    async fn decision_count(state: &AppState, proposal_id: &str) -> i64 {
        use sea_orm::FromQueryResult;
        CountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*)::bigint AS count FROM decisions WHERE proposal_id = $1",
            vec![proposal_id.to_string().into()],
        ))
        .one(&state.db)
        .await
        .expect("the decision count is readable")
        .map_or(0, |row| row.count)
    }

    /// Governance proposals used to live in one instance wide namespace with no tenant column, so
    /// every authenticated caller could list every other tenant's proposals.
    #[tokio::test]
    async fn a_member_of_one_workspace_cannot_read_another_workspace() {
        let scratch = scratch_or_skip!("proposal_scope");
        let state = state_for(&scratch);
        run_migrations_with(&state.db, MigrationOptions::default())
            .await
            .expect("the scratch database migrates");

        let alpha = seed_tenant(&state, "alpha", "open").await;
        let beta = seed_tenant(&state, "beta", "open").await;

        let listed = list_proposals(
            State(state.clone()),
            Extension(claims_for(alpha.user_id)),
            Query(empty_query()),
        )
        .await
        .expect("the list endpoint answers")
        .into_response();
        let body = body_of(listed).await;
        let items = body["data"]["items"].as_array().cloned().unwrap_or_default();
        let ids: Vec<&str> = items.iter().filter_map(|item| item["id"].as_str()).collect();
        assert_eq!(
            ids,
            vec![alpha.proposal_id.as_str()],
            "the list must contain only the caller's workspace"
        );
        assert_eq!(
            items
                .first()
                .and_then(|item| item["workspace_id"].as_str())
                .and_then(|id| Uuid::parse_str(id).ok()),
            Some(alpha.workspace_id)
        );

        let cross_tenant = get_proposal(
            State(state.clone()),
            Extension(claims_for(alpha.user_id)),
            axum::extract::Path(beta.proposal_id.clone()),
        )
        .await;
        assert!(
            cross_tenant.is_err(),
            "reading another workspace's proposal must not succeed"
        );

        scratch.drop_self().await;
    }

    /// `GET /api/v1/proposals/{id}` used to finalize an expired vote inline: it updated the
    /// proposal, inserted a decision and moved trust scores, all from a request that was not even
    /// authenticated. The read must not write, and the worker tick must do the settling.
    #[tokio::test]
    async fn reading_an_expired_proposal_does_not_settle_it_but_the_worker_tick_does() {
        let scratch = scratch_or_skip!("proposal_settlement");
        let state = state_for(&scratch);
        run_migrations_with(&state.db, MigrationOptions::default())
            .await
            .expect("the scratch database migrates");

        // Voting started 48 hours ago on a `rapid` cycle, whose window is one hour.
        let tenant = seed_tenant(&state, "settle", "voting").await;

        let response = get_proposal(
            State(state.clone()),
            Extension(claims_for(tenant.user_id)),
            axum::extract::Path(tenant.proposal_id.clone()),
        )
        .await
        .expect("the read answers")
        .into_response();
        let body = body_of(response).await;
        assert_eq!(body["data"]["proposal"]["status"], "voting");
        assert_eq!(
            decision_count(&state, &tenant.proposal_id).await,
            0,
            "a read must not create a decision"
        );

        governance_tick(&state).await.expect("the worker tick runs");
        assert_eq!(
            decision_count(&state, &tenant.proposal_id).await,
            1,
            "the worker tick settles the expired proposal"
        );

        // Running it again must not produce a second decision.
        governance_tick(&state).await.expect("the worker tick is idempotent");
        assert_eq!(decision_count(&state, &tenant.proposal_id).await, 1);

        scratch.drop_self().await;
    }
}
