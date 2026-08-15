// Pagination retains the established floating-point ceiling and signed wire representation.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use axum::{
    Extension,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};

use crate::{
    error::ApiError,
    response::{ApiResponse, PaginatedData},
    routes::proposal::{ProposalScope, proposal_scope},
};

#[derive(Debug, Serialize, FromQueryResult)]
pub struct DecisionRow {
    pub id: String,
    pub proposal_id: String,
    pub result: String,
    pub approval_rate: Option<f64>,
    pub total_votes: i32,
    pub yes_votes: i32,
    pub no_votes: i32,
    pub abstain_votes: i32,
    pub decided_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct ListDecisionsQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    count: i64,
}

/// The projection every decision read returns, aliased to the join used below.
const DECISION_COLUMNS: &str = "d.id, d.proposal_id, d.result::text AS result, d.approval_rate, \
                                d.total_votes, d.yes_votes, d.no_votes, d.abstain_votes, d.decided_at";

/// The join every decision read goes through, because the tenant lives on the proposal.
const DECISION_FROM: &str = "FROM decisions d INNER JOIN proposals p ON p.id = d.proposal_id";

/// Renders the tenant predicate a decision read is filtered by.
///
/// `decisions` has no tenant column of its own: `proposal_id` is `NOT NULL` and references
/// `proposals`, so a decision belongs wherever its proposal belongs and the predicate is applied
/// to the joined `proposals.workspace_id` that migration 0050 added.
///
/// A NULL workspace is a proposal that migration 0050 could not attribute. `IN` never matches
/// NULL, so those rows stay readable by instance administrators only, which is the same rule the
/// proposal routes already follow.
///
/// Returns the SQL fragment plus the values it binds, numbered from `$first_placeholder`.
fn workspace_predicate(scope: &ProposalScope, first_placeholder: usize) -> (String, Vec<sea_orm::Value>) {
    match scope {
        ProposalScope::Unrestricted => ("TRUE".to_string(), Vec::new()),
        // A caller that belongs to no workspace gets a predicate that matches nothing, rather
        // than no predicate at all.
        ProposalScope::Workspaces(workspaces) if workspaces.is_empty() => ("FALSE".to_string(), Vec::new()),
        ProposalScope::Workspaces(workspaces) => {
            let placeholders: Vec<String> = (first_placeholder..first_placeholder + workspaces.len())
                .map(|n| format!("${n}"))
                .collect();
            (
                format!("p.workspace_id IN ({})", placeholders.join(", ")),
                workspaces.iter().map(|id| (*id).into()).collect(),
            )
        }
    }
}

pub async fn list_decisions(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(query): Query<ListDecisionsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let scope = proposal_scope(&state, &claims).await?;
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    // The count is filtered by the same predicate as the page. An unfiltered `COUNT(*)` would
    // still report how many decisions the other tenants hold.
    let (tenant_sql, tenant_values) = workspace_predicate(&scope, 1);

    let total = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT COUNT(*)::bigint AS count {DECISION_FROM} WHERE {tenant_sql}"),
        tenant_values.clone(),
    ))
    .one(&state.db)
    .await?
    .map_or(0, |r| r.count);

    let limit_idx = tenant_values.len() + 1;
    let offset_idx = limit_idx + 1;
    let mut list_values = tenant_values;
    list_values.push(per_page.into());
    list_values.push(offset.into());

    let rows = DecisionRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r"
            SELECT {DECISION_COLUMNS}
            {DECISION_FROM}
            WHERE {tenant_sql}
            ORDER BY d.decided_at DESC
            LIMIT ${limit_idx} OFFSET ${offset_idx}
        "
        ),
        list_values,
    ))
    .all(&state.db)
    .await?;

    let total_pages = if total == 0 {
        1
    } else {
        ((total as f64) / (per_page as f64)).ceil() as i64
    };

    Ok(ApiResponse::success(PaginatedData {
        items: rows,
        total,
        page,
        per_page,
        total_pages,
    }))
}

pub async fn get_decision(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let scope = proposal_scope(&state, &claims).await?;
    let (tenant_sql, tenant_values) = workspace_predicate(&scope, 2);
    let mut values: Vec<sea_orm::Value> = vec![id.into()];
    values.extend(tenant_values);

    // A decision outside the caller's workspaces answers "not found" rather than "forbidden", so
    // the endpoint does not confirm that an id exists in someone else's tenant.
    let decision = DecisionRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {DECISION_COLUMNS} {DECISION_FROM} WHERE d.id = $1 AND {tenant_sql}"),
        values,
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("decision not found".to_string()))?;

    Ok(ApiResponse::success(decision))
}

pub async fn get_proposal_decision(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(proposal_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let scope = proposal_scope(&state, &claims).await?;
    let (tenant_sql, tenant_values) = workspace_predicate(&scope, 2);
    let mut values: Vec<sea_orm::Value> = vec![proposal_id.into()];
    values.extend(tenant_values);

    let decision = DecisionRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {DECISION_COLUMNS} {DECISION_FROM} WHERE d.proposal_id = $1 AND {tenant_sql}"),
        values,
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("decision not found".to_string()))?;

    Ok(ApiResponse::success(decision))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn an_instance_administrator_is_not_filtered() {
        let (sql, values) = workspace_predicate(&ProposalScope::Unrestricted, 1);
        assert_eq!(sql, "TRUE");
        assert!(values.is_empty());
    }

    #[test]
    fn a_caller_without_a_workspace_matches_nothing() {
        let (sql, values) = workspace_predicate(&ProposalScope::Workspaces(Vec::new()), 1);
        assert_eq!(sql, "FALSE");
        assert!(values.is_empty());
    }

    #[test]
    fn workspaces_are_bound_as_parameters_from_the_requested_offset() {
        let workspaces = vec![Uuid::new_v4(), Uuid::new_v4()];
        let (sql, values) = workspace_predicate(&ProposalScope::Workspaces(workspaces.clone()), 2);
        assert_eq!(sql, "p.workspace_id IN ($2, $3)");
        assert_eq!(values.len(), workspaces.len());
    }
}

/// Decision endpoints against a real `PostgreSQL` server.
///
/// The three decision reads took no authentication context at all, so any authenticated caller
/// read every tenant's governance outcomes and the unfiltered `COUNT(*)` leaked how many the
/// other tenants held. Only a real database can show that the join to `proposals.workspace_id`
/// filters, that an unattributed proposal stays administrator only, and that the total agrees
/// with the page.
///
/// Set `OPENPR_TEST_DATABASE_URL` to a maintenance connection string, for example
/// `postgres://user:pw@127.0.0.1:5432/postgres`. Without it these tests report that they were
/// skipped instead of pretending to pass.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    clippy::indexing_slicing,
    clippy::struct_field_names
)]
mod decision_scope_database_tests {
    use super::{ListDecisionsQuery, get_decision, get_proposal_decision, list_decisions};
    use axum::extract::{Extension, Path, Query, State};
    use axum::response::IntoResponse;
    use chrono::Utc;
    use platform::{
        app::AppState,
        auth::{JwtClaims, TokenType},
        config::{AppConfig, Secret},
    };
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
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
    /// name order, which is the order the runner uses. Returns `None` when the environment does
    /// not offer a server, so the suite stays runnable without one.
    async fn scratch(label: &str) -> Option<Scratch> {
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
        let admin = Database::connect(&admin_url)
            .await
            .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));

        let name = format!("openpr_dec_{label}");
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
                jwt_secret: Secret::new("decision-scope-test-secret"),
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

    fn empty_query() -> ListDecisionsQuery {
        ListDecisionsQuery {
            page: None,
            per_page: Some(100),
        }
    }

    async fn body_of(response: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("the response body is readable");
        serde_json::from_slice(&bytes).expect("the response body is JSON")
    }

    /// One workspace with one member, one proposal and the decision that settled it.
    struct Tenant {
        user_id: Uuid,
        proposal_id: String,
        decision_id: String,
    }

    async fn create_user(state: &AppState, label: &str, role: &str) -> Uuid {
        let user_id = Uuid::new_v4();
        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO users (id, email, name, password_hash, role) VALUES ($1, $2, $3, 'x', $4)",
                vec![
                    user_id.into(),
                    format!("{label}-{user_id}@example.test").into(),
                    label.into(),
                    role.into(),
                ],
            ))
            .await
            .expect("the user is created");
        user_id
    }

    /// Inserts a proposal plus the decision that settled it, and returns both ids.
    /// `workspace_id` is `None` for the rows migration 0050 could not attribute.
    async fn seed_decision(
        state: &AppState,
        label: &str,
        author_id: Uuid,
        workspace_id: Option<Uuid>,
    ) -> (String, String) {
        let proposal_id = format!("PROP-{label}-{}", Uuid::new_v4());
        let decision_id = format!("DEC-{label}-{}", Uuid::new_v4());

        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                    INSERT INTO proposals (
                        id, title, proposal_type, status, author_id, author_type, content,
                        domains, voting_rule, cycle_template, workspace_id, created_at
                    ) VALUES (
                        $1, $2, 'feature'::proposal_type, 'approved'::proposal_status, $3,
                        'human'::author_type, $4, '["product"]'::jsonb,
                        'simple_majority'::voting_rule, 'rapid'::cycle_template, $5, $6
                    )
                "#,
                vec![
                    proposal_id.clone().into(),
                    format!("proposal for {label}").into(),
                    author_id.to_string().into(),
                    "content that is comfortably longer than the fifty character minimum".into(),
                    workspace_id.into(),
                    Utc::now().into(),
                ],
            ))
            .await
            .expect("the proposal is created");

        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    INSERT INTO decisions (
                        id, proposal_id, result, approval_rate, total_votes,
                        yes_votes, no_votes, abstain_votes, decided_at
                    ) VALUES ($1, $2, 'approved'::decision_result, 1.0, 1, 1, 0, 0, $3)
                ",
                vec![
                    decision_id.clone().into(),
                    proposal_id.clone().into(),
                    Utc::now().into(),
                ],
            ))
            .await
            .expect("the decision is created");

        (proposal_id, decision_id)
    }

    async fn seed_tenant(state: &AppState, label: &str) -> Tenant {
        let user_id = create_user(state, label, "user").await;
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
                "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
                vec![workspace_id.into(), user_id.into()],
            ))
            .await
            .expect("the membership is created");

        let (proposal_id, decision_id) = seed_decision(state, label, user_id, Some(workspace_id)).await;

        Tenant {
            user_id,
            proposal_id,
            decision_id,
        }
    }

    async fn listed_ids(state: &AppState, user_id: Uuid) -> (Vec<String>, i64) {
        let response = list_decisions(
            State(state.clone()),
            Extension(claims_for(user_id)),
            Query(empty_query()),
        )
        .await
        .expect("the list endpoint answers")
        .into_response();
        let body = body_of(response).await;
        let items = body["data"]["items"].as_array().cloned().unwrap_or_default();
        let ids = items
            .iter()
            .filter_map(|item| item["id"].as_str().map(ToString::to_string))
            .collect();
        let total = body["data"]["total"].as_i64().unwrap_or(-1);
        (ids, total)
    }

    /// Every decision read used to take no claims at all, so one tenant read another tenant's
    /// governance outcomes and the list total counted them too.
    #[tokio::test]
    async fn a_member_of_one_workspace_cannot_read_another_workspace() {
        let scratch = scratch_or_skip!("scope");
        let state = state_for(&scratch);

        let alpha = seed_tenant(&state, "alpha").await;
        let beta = seed_tenant(&state, "beta").await;

        let (ids, total) = listed_ids(&state, alpha.user_id).await;
        assert_eq!(
            ids,
            vec![alpha.decision_id.clone()],
            "the list must contain only the caller's workspace"
        );
        assert_eq!(total, 1, "the total must not count another workspace's decisions");

        assert!(
            get_decision(
                State(state.clone()),
                Extension(claims_for(alpha.user_id)),
                Path(beta.decision_id.clone()),
            )
            .await
            .is_err(),
            "reading another workspace's decision must not succeed"
        );
        assert!(
            get_proposal_decision(
                State(state.clone()),
                Extension(claims_for(alpha.user_id)),
                Path(beta.proposal_id.clone()),
            )
            .await
            .is_err(),
            "reading another workspace's decision by proposal must not succeed"
        );

        // The caller's own decision still reads, by id and by proposal.
        let own = get_decision(
            State(state.clone()),
            Extension(claims_for(alpha.user_id)),
            Path(alpha.decision_id.clone()),
        )
        .await
        .expect("the caller reads their own decision")
        .into_response();
        assert_eq!(body_of(own).await["data"]["id"], alpha.decision_id);

        let own_by_proposal = get_proposal_decision(
            State(state.clone()),
            Extension(claims_for(alpha.user_id)),
            Path(alpha.proposal_id.clone()),
        )
        .await
        .expect("the caller reads their own decision by proposal")
        .into_response();
        assert_eq!(body_of(own_by_proposal).await["data"]["id"], alpha.decision_id);

        scratch.drop_self().await;
    }

    /// Migration 0050 leaves a proposal it cannot attribute at NULL. Those decisions belong to no
    /// tenant, so no tenant may read them, but an operator still has to be able to find them.
    #[tokio::test]
    async fn an_unattributed_decision_is_readable_by_an_instance_administrator_only() {
        let scratch = scratch_or_skip!("unattributed");
        let state = state_for(&scratch);

        let tenant = seed_tenant(&state, "alpha").await;
        let orphan_author = create_user(&state, "orphan", "user").await;
        let (_, orphan_decision) = seed_decision(&state, "orphan", orphan_author, None).await;
        let admin_id = create_user(&state, "root", "admin").await;

        let (member_ids, member_total) = listed_ids(&state, tenant.user_id).await;
        assert_eq!(member_ids, vec![tenant.decision_id.clone()]);
        assert_eq!(member_total, 1);
        assert!(
            get_decision(
                State(state.clone()),
                Extension(claims_for(tenant.user_id)),
                Path(orphan_decision.clone()),
            )
            .await
            .is_err(),
            "an unattributed decision must not be readable by a tenant"
        );

        let (admin_ids, admin_total) = listed_ids(&state, admin_id).await;
        assert_eq!(admin_total, 2, "the administrator sees both decisions");
        assert!(admin_ids.contains(&orphan_decision), "including the unattributed one");
        assert!(
            get_decision(
                State(state.clone()),
                Extension(claims_for(admin_id)),
                Path(orphan_decision.clone()),
            )
            .await
            .is_ok(),
            "an operator can still open the unattributed decision"
        );

        scratch.drop_self().await;
    }

    /// A caller that matches neither a user nor an active bot sees nothing rather than everything.
    #[tokio::test]
    async fn an_unknown_subject_reads_no_decision() {
        let scratch = scratch_or_skip!("unknown");
        let state = state_for(&scratch);

        let tenant = seed_tenant(&state, "alpha").await;
        let stranger = Uuid::new_v4();

        let (ids, total) = listed_ids(&state, stranger).await;
        assert!(ids.is_empty(), "an unknown subject lists nothing");
        assert_eq!(total, 0, "and is told the total is zero");
        assert!(
            get_decision(
                State(state.clone()),
                Extension(claims_for(stranger)),
                Path(tenant.decision_id.clone()),
            )
            .await
            .is_err()
        );

        scratch.drop_self().await;
    }
}
