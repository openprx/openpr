use axum::{
    Extension,
    extract::{Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{error::ApiError, middleware::bot_auth::BotAuthContext, response::ApiResponse};

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    #[serde(rename = "type")]
    pub search_type: Option<String>, // 'issue', 'project', 'comment', or empty for all
    /// Optional project filter. The project must belong to a workspace the
    /// caller can already see, otherwise the request is rejected.
    pub project_id: Option<Uuid>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct IssueSearchResult {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub state: String,
    pub project_id: Uuid,
    pub workspace_id: Uuid,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ProjectSearchResult {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub workspace_id: Uuid,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct CommentSearchResult {
    pub id: Uuid,
    pub body: String,
    pub issue_id: Uuid,
    pub project_id: Uuid,
    pub workspace_id: Uuid,
    pub author_id: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum SearchResult {
    #[serde(rename = "issue")]
    Issue(IssueSearchResult),
    #[serde(rename = "project")]
    Project(ProjectSearchResult),
    #[serde(rename = "comment")]
    Comment(CommentSearchResult),
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub total: usize,
}

/// Tenant and project boundary of a single search request.
///
/// Exactly one identity is set:
/// - `member_user`: JWT caller, scoped to the workspaces they are a member of.
/// - `bot_workspace`: bot token, scoped to the one workspace the token was
///   issued for. Bots are never resolved through `workspace_members`, so the
///   scope holds even if a bot also owns a membership row.
///
/// `project_id` narrows the scope further and is validated against the tenant
/// scope before any result query runs.
#[derive(Debug, Clone, Copy)]
struct SearchScope {
    member_user: Option<Uuid>,
    bot_workspace: Option<Uuid>,
    project: Option<Uuid>,
}

impl SearchScope {
    const fn for_user(user_id: Uuid) -> Self {
        Self {
            member_user: Some(user_id),
            bot_workspace: None,
            project: None,
        }
    }

    const fn for_bot(workspace_id: Uuid) -> Self {
        Self {
            member_user: None,
            bot_workspace: Some(workspace_id),
            project: None,
        }
    }

    const fn with_project(mut self, project_id: Option<Uuid>) -> Self {
        self.project = project_id;
        self
    }

    /// Bind values for `$1`, `$2` and `$3` of [`SCOPE_FILTER_SQL`].
    fn params(self) -> Vec<sea_orm::Value> {
        vec![self.member_user.into(), self.bot_workspace.into(), self.project.into()]
    }
}

/// Tenant + project guard shared by every search query.
///
/// The outer query must expose `projects` under the alias `p`. Placeholders:
/// `$1` member user id (NULL for bots), `$2` bot workspace id (NULL for users),
/// `$3` optional project filter (NULL means "every visible project").
const SCOPE_FILTER_SQL: &str = r"
        (
            ($1::uuid IS NOT NULL AND EXISTS (
                SELECT 1
                FROM workspace_members wm
                WHERE wm.workspace_id = p.workspace_id
                  AND wm.user_id = $1
            ))
            OR ($2::uuid IS NOT NULL AND p.workspace_id = $2)
        )
        AND ($3::uuid IS NULL OR p.id = $3)
";

/// GET /api/v1/search
pub async fn search(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Query(query): Query<SearchQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let base_scope = match bot {
        Some(Extension(bot_ctx)) => SearchScope::for_bot(bot_ctx.workspace_id),
        None => SearchScope::for_user(
            Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?,
        ),
    };
    let scope = base_scope.with_project(query.project_id);

    if query.q.trim().is_empty() {
        return Err(ApiError::BadRequest("Search query cannot be empty".to_string()));
    }

    // Reject an out-of-scope project explicitly instead of silently returning
    // an empty result set, so callers cannot probe foreign project ids.
    ensure_project_in_scope(&state.db, scope).await?;

    let limit = i64::from(query.limit.unwrap_or(50).min(100));
    let search_type = query.search_type.as_deref();

    let mut results = Vec::new();

    // Search issues
    if search_type.is_none() || search_type == Some("issue") {
        match search_issues(&state.db, scope, &query.q, limit).await {
            Ok(issues) => results.extend(issues.into_iter().map(SearchResult::Issue)),
            Err(err) => tracing::warn!(error = %err, "issue search failed"),
        }
    }

    // Search projects
    if search_type.is_none() || search_type == Some("project") {
        match search_projects(&state.db, scope, &query.q, limit).await {
            Ok(projects) => results.extend(projects.into_iter().map(SearchResult::Project)),
            Err(err) => tracing::warn!(error = %err, "project search failed"),
        }
    }

    // Search comments
    if search_type.is_none() || search_type == Some("comment") {
        match search_comments(&state.db, scope, &query.q, limit).await {
            Ok(comments) => results.extend(comments.into_iter().map(SearchResult::Comment)),
            Err(err) => tracing::warn!(error = %err, "comment search failed"),
        }
    }

    let total = results.len();

    Ok(ApiResponse::success(SearchResponse {
        query: query.q,
        results,
        total,
    }))
}

/// Verifies that an explicitly requested project exists inside the caller's scope.
async fn ensure_project_in_scope(db: &DatabaseConnection, scope: SearchScope) -> Result<(), ApiError> {
    if scope.project.is_none() {
        return Ok(());
    }

    let sql = format!("SELECT 1 FROM projects p WHERE {SCOPE_FILTER_SQL}");
    let found = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            &sql,
            scope.params(),
        ))
        .await?;

    if found.is_none() {
        return Err(ApiError::NotFound("project not found or access denied".to_string()));
    }

    Ok(())
}

async fn search_issues(
    db: &DatabaseConnection,
    scope: SearchScope,
    query: &str,
    limit: i64,
) -> Result<Vec<IssueSearchResult>, ApiError> {
    let pattern = format!("%{query}%");
    let sql = format!(
        r"
        SELECT wi.id, wi.title, wi.description, wi.state, wi.project_id, p.workspace_id
        FROM work_items wi
        INNER JOIN projects p ON wi.project_id = p.id
        WHERE {SCOPE_FILTER_SQL}
          AND (wi.title ILIKE $4 OR wi.description ILIKE $4)
        ORDER BY wi.updated_at DESC
        LIMIT $5
    "
    );

    let mut values = scope.params();
    values.push(pattern.into());
    values.push(limit.into());

    let rows = db
        .query_all(Statement::from_sql_and_values(DbBackend::Postgres, &sql, values))
        .await?;

    rows.iter()
        .map(|r| IssueSearchResult::from_query_result(r, ""))
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

async fn search_projects(
    db: &DatabaseConnection,
    scope: SearchScope,
    query: &str,
    limit: i64,
) -> Result<Vec<ProjectSearchResult>, ApiError> {
    let pattern = format!("%{query}%");
    let sql = format!(
        r"
        SELECT p.id, p.key, p.name, p.description, p.workspace_id
        FROM projects p
        WHERE {SCOPE_FILTER_SQL}
          AND (p.name ILIKE $4 OR p.key ILIKE $4 OR p.description ILIKE $4)
        ORDER BY p.updated_at DESC
        LIMIT $5
    "
    );

    let mut values = scope.params();
    values.push(pattern.into());
    values.push(limit.into());

    let rows = db
        .query_all(Statement::from_sql_and_values(DbBackend::Postgres, &sql, values))
        .await?;

    rows.iter()
        .map(|r| ProjectSearchResult::from_query_result(r, ""))
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

async fn search_comments(
    db: &DatabaseConnection,
    scope: SearchScope,
    query: &str,
    limit: i64,
) -> Result<Vec<CommentSearchResult>, ApiError> {
    let pattern = format!("%{query}%");
    let sql = format!(
        r"
        SELECT c.id, c.body, c.work_item_id AS issue_id, wi.project_id, p.workspace_id, c.author_id, c.created_at
        FROM comments c
        INNER JOIN work_items wi ON c.work_item_id = wi.id
        INNER JOIN projects p ON wi.project_id = p.id
        WHERE {SCOPE_FILTER_SQL}
          AND c.body ILIKE $4
        ORDER BY c.created_at DESC
        LIMIT $5
    "
    );

    let mut values = scope.params();
    values.push(pattern.into());
    values.push(limit.into());

    let rows = db
        .query_all(Statement::from_sql_and_values(DbBackend::Postgres, &sql, values))
        .await?;

    rows.iter()
        .map(|r| CommentSearchResult::from_query_result(r, ""))
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::nursery
)]
mod tests {
    use super::{BotAuthContext, SCOPE_FILTER_SQL, SearchScope};
    use crate::error::ApiError;
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
    use uuid::Uuid;

    #[test]
    fn user_scope_never_binds_a_bot_workspace() {
        let user_id = Uuid::new_v4();
        let scope = SearchScope::for_user(user_id);
        assert_eq!(scope.member_user, Some(user_id));
        assert_eq!(scope.bot_workspace, None);
        assert_eq!(scope.project, None);
    }

    #[test]
    fn bot_scope_is_bound_to_its_own_workspace_only() {
        let workspace_id = Uuid::new_v4();
        let scope = SearchScope::for_bot(workspace_id);
        // A bot must never be resolved through workspace_members.
        assert_eq!(scope.member_user, None);
        assert_eq!(scope.bot_workspace, Some(workspace_id));
    }

    #[test]
    fn with_project_records_the_filter() {
        let project_id = Uuid::new_v4();
        let scope = SearchScope::for_user(Uuid::new_v4()).with_project(Some(project_id));
        assert_eq!(scope.project, Some(project_id));
        assert_eq!(scope.params().len(), 3);
    }

    #[test]
    fn scope_filter_constrains_both_tenant_and_project() {
        assert!(SCOPE_FILTER_SQL.contains("wm.user_id = $1"));
        assert!(SCOPE_FILTER_SQL.contains("p.workspace_id = $2"));
        assert!(SCOPE_FILTER_SQL.contains("$3::uuid IS NULL OR p.id = $3"));
    }

    // ---- Real-database tests (opt-in via OPENPR_TEST_DATABASE_URL) ----
    //
    // These drive the `search` handler itself, so they fail if the scope guard
    // is dropped from the handler and not only from the SQL fragment.

    use super::{SearchQuery, search};
    use axum::{
        Extension,
        extract::{Query, State},
        response::IntoResponse,
    };
    use platform::{
        app::AppState,
        auth::{JwtClaims, TokenType},
        config::AppConfig,
    };
    use serde_json::Value as JsonValue;

    struct Fixture {
        state: AppState,
        token: String,
        alice_id: Uuid,
        bob_id: Uuid,
        workspace_a: Uuid,
        workspace_b: Uuid,
        project_a1: Uuid,
        project_a2: Uuid,
        project_b1: Uuid,
        bot_id: Uuid,
    }

    /// Caller identity used to invoke the handler.
    #[derive(Clone, Copy)]
    enum Caller {
        User(Uuid),
        /// Bot token: (bot id, workspace the token belongs to).
        Bot(Uuid, Uuid),
    }

    async fn state_from_env() -> Option<AppState> {
        let url = std::env::var("OPENPR_TEST_DATABASE_URL").ok()?;
        let db = Database::connect(&url)
            .await
            .unwrap_or_else(|err| panic!("cannot connect to OPENPR_TEST_DATABASE_URL: {err}"));
        let cfg = AppConfig {
            app_name: "api-test".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: url,
            jwt_secret: "test-secret".to_string(),
            jwt_access_ttl_seconds: 60,
            jwt_refresh_ttl_seconds: 60,
            default_author_id: None,
        };
        Some(AppState { cfg, db })
    }

    async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
        db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
    }

    async fn insert_user(db: &DatabaseConnection, user_id: Uuid) {
        exec(
            db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'test', 'user', true)",
            vec![user_id.into(), format!("{user_id}@search.test").into()],
        )
        .await;
    }

    async fn insert_workspace(db: &DatabaseConnection, workspace_id: Uuid, owner: Uuid) {
        exec(
            db,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'search test', $3)",
            vec![workspace_id.into(), format!("ws-{workspace_id}").into(), owner.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
            vec![workspace_id.into(), owner.into()],
        )
        .await;
    }

    async fn insert_project(db: &DatabaseConnection, project_id: Uuid, workspace_id: Uuid, key: &str, token: &str) {
        exec(
            db,
            "INSERT INTO projects (id, workspace_id, key, name, description) VALUES ($1, $2, $3, $4, $5)",
            vec![
                project_id.into(),
                workspace_id.into(),
                key.into(),
                format!("{token} project {key}").into(),
                format!("{token} description").into(),
            ],
        )
        .await;
    }

    /// Inserts one work item plus one comment, both matching `token`.
    async fn insert_issue_with_comment(db: &DatabaseConnection, project_id: Uuid, author: Uuid, token: &str) {
        let issue_id = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO work_items (id, project_id, title, description, state, sequence_number) \
             VALUES ($1, $2, $3, $4, 'todo', 1)",
            vec![
                issue_id.into(),
                project_id.into(),
                format!("{token} issue").into(),
                format!("{token} body").into(),
            ],
        )
        .await;
        exec(
            db,
            "INSERT INTO comments (id, work_item_id, author_id, body) VALUES ($1, $2, $3, $4)",
            vec![
                Uuid::new_v4().into(),
                issue_id.into(),
                author.into(),
                format!("{token} comment").into(),
            ],
        )
        .await;
    }

    /// Two tenants: workspace A (alice, two projects) and workspace B (bob, one
    /// project). Every seeded row carries the same unique search token, so any
    /// leak across a tenant or project boundary shows up as an extra hit.
    async fn seed(state: AppState) -> Fixture {
        let token = format!("sq{}", Uuid::new_v4().simple());
        let alice_id = Uuid::new_v4();
        let bob_id = Uuid::new_v4();
        let bot_id = Uuid::new_v4();
        let workspace_a = Uuid::new_v4();
        let workspace_b = Uuid::new_v4();
        let project_a1 = Uuid::new_v4();
        let project_a2 = Uuid::new_v4();
        let project_b1 = Uuid::new_v4();

        let db = state.db.clone();
        insert_user(&db, alice_id).await;
        insert_user(&db, bob_id).await;
        insert_workspace(&db, workspace_a, alice_id).await;
        insert_workspace(&db, workspace_b, bob_id).await;

        insert_project(&db, project_a1, workspace_a, "A1", &token).await;
        insert_project(&db, project_a2, workspace_a, "A2", &token).await;
        insert_project(&db, project_b1, workspace_b, "B1", &token).await;

        insert_issue_with_comment(&db, project_a1, alice_id, &token).await;
        insert_issue_with_comment(&db, project_a2, alice_id, &token).await;
        insert_issue_with_comment(&db, project_b1, bob_id, &token).await;

        Fixture {
            state,
            token,
            alice_id,
            bob_id,
            workspace_a,
            workspace_b,
            project_a1,
            project_a2,
            project_b1,
            bot_id,
        }
    }

    async fn cleanup(fx: &Fixture) {
        for workspace_id in [fx.workspace_a, fx.workspace_b] {
            exec(
                &fx.state.db,
                "DELETE FROM workspaces WHERE id = $1",
                vec![workspace_id.into()],
            )
            .await;
        }
        for user_id in [fx.alice_id, fx.bob_id] {
            exec(&fx.state.db, "DELETE FROM users WHERE id = $1", vec![user_id.into()]).await;
        }
    }

    fn auth_for(caller: Caller) -> (Extension<JwtClaims>, Option<Extension<BotAuthContext>>) {
        let (subject, bot) = match caller {
            Caller::User(user_id) => (user_id, None),
            Caller::Bot(bot_id, workspace_id) => (
                bot_id,
                Some(Extension(BotAuthContext {
                    bot_id,
                    workspace_id,
                    permissions: vec!["read".to_string()],
                })),
            ),
        };
        let claims = JwtClaims {
            sub: subject.to_string(),
            email: format!("{subject}@search.test"),
            token_type: TokenType::Access,
            iat: 0,
            exp: 0,
        };
        (Extension(claims), bot)
    }

    fn query_from_uri(uri: &str) -> Query<SearchQuery> {
        let uri = uri
            .parse::<axum::http::Uri>()
            .unwrap_or_else(|err| panic!("bad test uri: {err}"));
        Query::<SearchQuery>::try_from_uri(&uri).unwrap_or_else(|err| panic!("query rejected: {err:?}"))
    }

    /// Calls the handler exactly as the router does and returns the decoded body.
    async fn call_search(fx: &Fixture, caller: Caller, uri: &str) -> Result<JsonValue, ApiError> {
        let (claims, bot) = auth_for(caller);
        let response = search(State(fx.state.clone()), claims, bot, query_from_uri(uri))
            .await?
            .into_response();
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap_or_else(|err| panic!("cannot read response body: {err}"));
        Ok(serde_json::from_slice(&bytes).unwrap_or_else(|err| panic!("response is not json: {err}")))
    }

    /// Collects `field` of every result entry of the given `type`.
    fn ids_of(body: &JsonValue, result_type: &str, field: &str) -> Vec<Uuid> {
        let mut ids: Vec<Uuid> = body
            .get("data")
            .and_then(|data| data.get("results"))
            .and_then(JsonValue::as_array)
            .unwrap_or_else(|| panic!("no results array in {body}"))
            .iter()
            .filter(|entry| entry.get("type").and_then(JsonValue::as_str) == Some(result_type))
            .map(|entry| {
                entry
                    .get(field)
                    .and_then(JsonValue::as_str)
                    .and_then(|raw| Uuid::parse_str(raw).ok())
                    .unwrap_or_else(|| panic!("missing {field} in {entry}"))
            })
            .collect();
        ids.sort_unstable();
        ids
    }

    fn sorted(mut ids: Vec<Uuid>) -> Vec<Uuid> {
        ids.sort_unstable();
        ids
    }

    #[tokio::test]
    async fn db_search_is_scoped_to_workspace_and_project() {
        let Some(state) = state_from_env().await else {
            return;
        };
        let fx = seed(state).await;
        let alice = Caller::User(fx.alice_id);

        // 1. Unfiltered: alice sees both of her projects and nothing from workspace B.
        let body = call_search(&fx, alice, &format!("/api/v1/search?q={}", fx.token))
            .await
            .unwrap_or_else(|err| panic!("search failed: {err}"));
        let expected_a = sorted(vec![fx.project_a1, fx.project_a2]);
        assert_eq!(ids_of(&body, "issue", "project_id"), expected_a);
        assert_eq!(ids_of(&body, "comment", "project_id"), expected_a);
        assert_eq!(ids_of(&body, "project", "id"), expected_a);

        // 2. Project filter narrows every entity type to that single project.
        let body = call_search(
            &fx,
            alice,
            &format!("/api/v1/search?q={}&project_id={}", fx.token, fx.project_a1),
        )
        .await
        .unwrap_or_else(|err| panic!("filtered search failed: {err}"));
        assert_eq!(ids_of(&body, "issue", "project_id"), vec![fx.project_a1]);
        assert_eq!(ids_of(&body, "comment", "project_id"), vec![fx.project_a1]);
        assert_eq!(ids_of(&body, "project", "id"), vec![fx.project_a1]);

        // 3. A project of another workspace is rejected, not silently emptied.
        let denied = call_search(
            &fx,
            alice,
            &format!("/api/v1/search?q={}&project_id={}", fx.token, fx.project_b1),
        )
        .await;
        assert!(
            matches!(denied, Err(ApiError::NotFound(_))),
            "cross-workspace project must be rejected"
        );

        // 4. An unknown project id is rejected as well.
        let denied = call_search(
            &fx,
            alice,
            &format!("/api/v1/search?q={}&project_id={}", fx.token, Uuid::new_v4()),
        )
        .await;
        assert!(matches!(denied, Err(ApiError::NotFound(_))), "unknown project must 404");

        // 5. Bob only ever sees workspace B.
        let body = call_search(&fx, Caller::User(fx.bob_id), &format!("/api/v1/search?q={}", fx.token))
            .await
            .unwrap_or_else(|err| panic!("search failed: {err}"));
        assert_eq!(ids_of(&body, "issue", "project_id"), vec![fx.project_b1]);
        assert_eq!(ids_of(&body, "project", "id"), vec![fx.project_b1]);

        cleanup(&fx).await;
    }

    #[tokio::test]
    async fn db_bot_search_is_scoped_by_token_workspace() {
        let Some(state) = state_from_env().await else {
            return;
        };
        let fx = seed(state).await;

        // The bot id has no workspace_members row at all: its reach comes purely
        // from the workspace its token was issued for.
        let bot_a = Caller::Bot(fx.bot_id, fx.workspace_a);
        let body = call_search(&fx, bot_a, &format!("/api/v1/search?q={}", fx.token))
            .await
            .unwrap_or_else(|err| panic!("bot search failed: {err}"));
        assert_eq!(
            ids_of(&body, "issue", "project_id"),
            sorted(vec![fx.project_a1, fx.project_a2])
        );

        // Project filter applies to bots identically.
        let body = call_search(
            &fx,
            bot_a,
            &format!("/api/v1/search?q={}&project_id={}", fx.token, fx.project_a1),
        )
        .await
        .unwrap_or_else(|err| panic!("filtered bot search failed: {err}"));
        assert_eq!(ids_of(&body, "issue", "project_id"), vec![fx.project_a1]);
        assert_eq!(ids_of(&body, "comment", "project_id"), vec![fx.project_a1]);

        // A project of the neighbouring workspace stays out of reach.
        let denied = call_search(
            &fx,
            bot_a,
            &format!("/api/v1/search?q={}&project_id={}", fx.token, fx.project_b1),
        )
        .await;
        assert!(
            matches!(denied, Err(ApiError::NotFound(_))),
            "bot must not reach a foreign project"
        );

        // A bot token of workspace B never reaches workspace A data, even though
        // alice (a member of A) is not involved in the request at all.
        let body = call_search(
            &fx,
            Caller::Bot(fx.bot_id, fx.workspace_b),
            &format!("/api/v1/search?q={}", fx.token),
        )
        .await
        .unwrap_or_else(|err| panic!("bot search failed: {err}"));
        assert_eq!(ids_of(&body, "issue", "project_id"), vec![fx.project_b1]);

        cleanup(&fx).await;
    }
}
