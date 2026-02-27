mod entities;
mod error;
mod middleware;
mod response;
mod routes;
mod services;
mod webhook_trigger;

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
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde::Serialize;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};
use crate::response::ApiResponse;

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
    let state = AppState {
        cfg: cfg.clone(),
        db,
    };
    let auth_state = state.clone();
    routes::proposal::start_governance_watcher(state.clone());

    let app = Router::new()
        .route("/uploads/{file_name}", get(routes::upload::get_uploaded_file))
        .route(
            "/api/v1/uploads/{file_name}",
            get(routes::upload::get_uploaded_file),
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
                middleware::auth::auth_middleware,
            )),
        )
        .route(
            "/api/v1/auth/me",
            get(routes::auth::me).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::auth_middleware,
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
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{id}",
            get(routes::workspace::get_workspace)
                .put(routes::workspace::update_workspace)
                .delete(routes::workspace::delete_workspace)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        // Workspace member routes (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/members",
            post(routes::member::add_member)
                .get(routes::member::list_members)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/members/{user_id}",
            delete(routes::member::remove_member)
                .patch(routes::member::update_member_role)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/users",
            get(routes::member::search_users).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::auth_middleware,
            )),
        )
        // Project routes (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/projects",
            post(routes::project::create_project)
                .get(routes::project::list_projects)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{id}",
            get(routes::project::get_project)
                .put(routes::project::update_project)
                .delete(routes::project::delete_project)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/domains",
            get(routes::decision_domain::list_decision_domains)
                .post(routes::decision_domain::create_decision_domain)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/domains/{id}",
            patch(routes::decision_domain::update_decision_domain)
                .delete(routes::decision_domain::delete_decision_domain)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/domains/{id}/members",
            get(routes::decision_domain::list_domain_members).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/decision-domains",
            get(routes::decision_domain::list_decision_domains_global).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/projects/{project_id}/ai-participants",
            get(routes::ai_agent::list_ai_agents)
                .post(routes::ai_agent::create_ai_agent)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/ai-participants/{id}/stats",
            get(routes::ai_agent::get_ai_agent_stats).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::auth_middleware,
            )),
        )
        .route(
            "/api/v1/projects/{project_id}/ai-participants/{id}",
            get(routes::ai_agent::get_ai_agent)
                .patch(routes::ai_agent::update_ai_agent)
                .delete(routes::ai_agent::delete_ai_agent)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/ai/tasks",
            get(routes::ai_callback::list_project_ai_tasks)
                .post(routes::ai_callback::create_project_ai_task)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/ai/callbacks/task/{task_id}/complete",
            post(routes::ai_callback::complete_task).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/ai/callbacks/task/{task_id}/fail",
            post(routes::ai_callback::fail_task).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/ai/callbacks/task/{task_id}/progress",
            post(routes::ai_callback::report_progress).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/trust-scores",
            get(routes::trust_score::list_trust_scores).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/users/{id}/trust",
            get(routes::trust_score::get_user_trust).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/trust-scores/{user_id}",
            get(routes::trust_score::get_user_trust).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/trust-scores/{user_id}/history",
            get(routes::trust_score::list_user_trust_history).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/trust-scores/{user_id}/{domain}",
            get(routes::trust_score::get_user_trust_by_domain).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
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
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/issues/{id}",
            get(routes::issue::get_issue)
                .put(routes::issue::update_issue)
                .delete(routes::issue::delete_issue)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        // Comment routes (protected)
        .route(
            "/api/v1/issues/{issue_id}/comments",
            post(routes::comment::create_comment)
                .get(routes::comment::list_comments)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/comments/{id}",
            axum::routing::put(routes::comment::update_comment)
                .delete(routes::comment::delete_comment)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        // Governance proposal routes
        .route(
            "/api/v1/proposals",
            get(routes::proposal::list_proposals),
        )
        .route(
            "/api/v1/proposals",
            post(routes::proposal::create_proposal).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposals/{id}",
            get(routes::proposal::get_proposal),
        )
        .route(
            "/api/v1/proposals/{id}",
            patch(routes::proposal::update_proposal)
                .delete(routes::proposal::delete_proposal)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/proposals/{id}/submit",
            post(routes::proposal::submit_proposal).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposals/{id}/start-voting",
            post(routes::proposal::start_voting).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposals/{id}/archive",
            post(routes::proposal::archive_proposal).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposals/{id}/votes",
            get(routes::proposal::list_votes),
        )
        .route(
            "/api/v1/proposals/{id}/votes",
            post(routes::proposal::create_vote).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::auth_middleware,
            )),
        )
        .route(
            "/api/v1/proposals/{id}/votes/mine",
            delete(routes::proposal::delete_my_vote).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
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
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/proposals/{id}/veto/escalation",
            post(routes::veto::start_escalation).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposals/{id}/veto/escalation/vote",
            post(routes::veto::vote_escalation).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/vetoers",
            get(routes::veto::list_vetoers)
                .post(routes::veto::create_vetoer)
                .delete(routes::veto::delete_vetoer)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/trust-scores/appeals",
            get(routes::appeal::list_appeals)
                .post(routes::appeal::create_appeal)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/trust-scores/appeals/{id}",
            get(routes::appeal::get_appeal)
                .patch(routes::appeal::update_appeal)
                .delete(routes::appeal::delete_appeal)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/proposals/{id}/comments",
            get(routes::proposal::list_proposal_comments),
        )
        .route(
            "/api/v1/proposals/{id}/comments",
            post(routes::proposal::create_proposal_comment).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposal-comments/{id}",
            delete(routes::proposal::delete_proposal_comment).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposals/{id}/comments/{comment_id}",
            delete(routes::proposal::delete_proposal_comment_under_proposal).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposals/{id}/issues",
            post(routes::proposal::link_issue)
                .get(routes::proposal::list_linked_issues)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/proposals/{proposal_id}/issues/{issue_id}",
            delete(routes::proposal::unlink_issue).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposal-templates",
            get(routes::proposal_template::list_proposal_templates)
                .post(routes::proposal_template::create_proposal_template)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/proposal-templates/{id}",
            get(routes::proposal_template::get_proposal_template)
                .put(routes::proposal_template::update_proposal_template)
                .delete(routes::proposal_template::delete_proposal_template)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/governance/config",
            get(routes::governance::get_governance_config).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
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
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/decisions",
            get(routes::decision::list_decisions).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/decisions/{id}",
            get(routes::decision::get_decision).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::auth_middleware,
            )),
        )
        .route(
            "/api/v1/proposals/{id}/decision",
            get(routes::decision::get_proposal_decision).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
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
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/impact-reviews",
            get(routes::impact_review::list_impact_reviews).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/impact-reviews/{id}/participants",
            get(routes::impact_review::list_review_participants)
                .post(routes::impact_review::upsert_review_participant)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/impact-reviews/{id}/participants/{user_id}",
            patch(routes::impact_review::update_review_participant)
                .delete(routes::impact_review::delete_review_participant)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/proposals/{id}/chain",
            get(routes::governance_ext::get_proposal_chain).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/proposals/{id}/timeline",
            get(routes::governance_ext::get_proposal_timeline).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/decisions/analytics",
            get(routes::governance_ext::get_decision_analytics).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/projects/{pid}/audit-reports",
            post(routes::governance_ext::create_project_audit_report)
                .get(routes::governance_ext::list_project_audit_reports)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{pid}/audit-reports/{id}",
            get(routes::governance_ext::get_project_audit_report).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/ai-learning/{review_id}/feedback",
            get(routes::governance_ext::get_ai_review_feedback).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/ai-participants/{id}/learning",
            get(routes::governance_ext::get_ai_participant_learning).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/ai-participants/{id}/alignment-stats",
            get(routes::governance_ext::get_ai_participant_alignment_stats).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
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
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/labels/{id}",
            axum::routing::put(routes::label::update_label)
                .delete(routes::label::delete_label)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        // Issue label association routes (protected)
        .route(
            "/api/v1/issues/{issue_id}/labels/{label_id}",
            post(routes::label::add_label_to_issue)
                .delete(routes::label::remove_label_from_issue)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/issues/{issue_id}/labels",
            get(routes::label::get_issue_labels).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::auth_middleware,
            )),
        )
        // Bot token management routes (protected — JWT only)
        .route(
            "/api/v1/workspaces/{workspace_id}/bots",
            post(routes::bot::create_bot)
                .get(routes::bot::list_bots)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/bots/{bot_id}",
            axum::routing::delete(routes::bot::revoke_bot).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        // Activity routes (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/activities",
            get(routes::activity::get_workspace_activities).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/projects/{project_id}/activities",
            get(routes::activity::get_project_activities).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/issues/{issue_id}/activities",
            get(routes::activity::get_issue_activities).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        // My routes (protected)
        .route(
            "/api/v1/my/issues",
            get(routes::my::get_my_issues).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::auth_middleware,
            )),
        )
        .route(
            "/api/v1/my/activities",
            get(routes::my::get_my_activities).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::auth_middleware,
            )),
        )
        // Board routes (protected)
        .route(
            "/api/v1/projects/{project_id}/board",
            get(routes::board::get_project_board).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::auth_middleware,
            )),
        )
        // Sprint routes (protected)
        .route(
            "/api/v1/projects/{project_id}/sprints",
            post(routes::sprint::create_sprint)
                .get(routes::sprint::list_sprints)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/sprints/{id}",
            axum::routing::put(routes::sprint::update_sprint)
                .delete(routes::sprint::delete_sprint)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        // Webhook routes (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/webhooks",
            post(routes::webhook::create_webhook)
                .get(routes::webhook::list_webhooks)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}",
            get(routes::webhook::get_webhook)
                .patch(routes::webhook::update_webhook)
                .delete(routes::webhook::delete_webhook)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/deliveries",
            get(routes::webhook::list_deliveries).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/deliveries/{delivery_id}",
            get(routes::webhook::get_delivery).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        // Notification routes (protected)
        .route(
            "/api/v1/notifications",
            get(routes::notification::list_notifications).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/notifications/unread-count",
            get(routes::notification::get_unread_count).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/notifications/{id}/read",
            patch(routes::notification::mark_notification_read)
                .put(routes::notification::mark_notification_read)
                .route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/notifications/read-all",
            patch(routes::notification::mark_all_read)
                .put(routes::notification::mark_all_read)
                .route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/notifications/{id}",
            delete(routes::notification::delete_notification).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::auth_middleware,
                ),
            ),
        )
        // Search route (protected)
        .route(
            "/api/v1/search",
            get(routes::search::search).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::auth_middleware,
            )),
        )
        // Export route (protected)
        .route(
            "/api/v1/export/project/{id}",
            get(routes::export::export_project).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::auth_middleware,
            )),
        )
        // Import route (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/import/project",
            post(routes::import::import_project).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::auth_middleware,
            )),
        )
        // Upload route (protected)
        .route(
            "/api/v1/upload",
            post(routes::upload::upload_file).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::auth_middleware,
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

async fn run_migrations(db: &DatabaseConnection) -> anyhow::Result<()> {
    const MIGRATIONS: &[(&str, &str)] = &[
        (
            "0001_init.sql",
            include_str!("../../../migrations/0001_init.sql"),
        ),
        (
            "0002_users.sql",
            include_str!("../../../migrations/0002_users.sql"),
        ),
        (
            "0003_labels.sql",
            include_str!("../../../migrations/0003_labels.sql"),
        ),
        (
            "0004_sprints.sql",
            include_str!("../../../migrations/0004_sprints.sql"),
        ),
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
    ];

    for (name, sql) in MIGRATIONS {
        match db.execute_unprepared(sql).await {
            Ok(_) => tracing::info!(migration = %name, "migration applied"),
            Err(e) => tracing::warn!(
                migration = %name,
                error = %e,
                "migration execution failed (likely already applied)"
            ),
        }
    }

    Ok(())
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
        (
            "review_participants table",
            "SELECT 1 FROM review_participants LIMIT 1",
        ),
        (
            "ai_learning_records table",
            "SELECT 1 FROM ai_learning_records LIMIT 1",
        ),
        (
            "decision_audit_reports table",
            "SELECT 1 FROM decision_audit_reports LIMIT 1",
        ),
        (
            "feedback_loop_links table",
            "SELECT 1 FROM feedback_loop_links LIMIT 1",
        ),
        (
            "governance_configs table",
            "SELECT 1 FROM governance_configs LIMIT 1",
        ),
        (
            "governance_audit_logs table",
            "SELECT 1 FROM governance_audit_logs LIMIT 1",
        ),
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
            return Err(anyhow::anyhow!(
                "governance schema check failed for {}: {}",
                name,
                e
            ));
        }
    }

    Ok(())
}
