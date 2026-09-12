//! Scoped, checksum-driven Flow maintenance primitives.

use collab_core::{CollabEngine, LoroCollabEngine};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::collab::bootstrap;
use super::projection;
use crate::error::ApiError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectionRebuildResult {
    pub workspace_id: Uuid,
    pub object_id: Uuid,
    pub document_id: Uuid,
    pub head_seq: i64,
    pub before_hash: Option<String>,
    pub after_hash: String,
    pub changed: bool,
    pub executed: bool,
}

#[derive(FromQueryResult)]
struct DocumentIdentity {
    workspace: Uuid,
    object: Uuid,
    document: Uuid,
}

#[derive(FromQueryResult)]
struct LockedHead {
    head_seq: i64,
    head_frontier: Vec<u8>,
}

#[derive(Debug, FromQueryResult, Serialize)]
struct ProjectionRow {
    document_seq: i64,
    document_frontier: Vec<u8>,
    title: String,
    state: Value,
    plain_text: String,
}

fn projection_hash(row: &ProjectionRow) -> Result<String, ApiError> {
    serde_json::to_vec(row)
        .map(|bytes| bootstrap::content_hash(&bytes))
        .map_err(|_| ApiError::Internal)
}

async fn projection_row<C: ConnectionTrait>(conn: &C, object_id: Uuid) -> Result<Option<ProjectionRow>, ApiError> {
    Ok(ProjectionRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT document_seq, document_frontier, title, state, plain_text \
         FROM flow_object_projections WHERE object_id = $1",
        vec![object_id.into()],
    ))
    .one(conn)
    .await?)
}

/// Plans or executes an exact-head projection rebuild for one object.
///
/// Decode and semantic projection happen before the transaction. Execute then locks the document,
/// rechecks both head sequence and frontier, and replaces the rebuildable row atomically. A stale
/// plan is rejected rather than applying a projection for the wrong head.
pub async fn rebuild_projection(
    db: &DatabaseConnection,
    object_id: Uuid,
    expected_head_seq: Option<i64>,
    execute: bool,
) -> Result<ProjectionRebuildResult, ApiError> {
    let identity = DocumentIdentity::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT fo.workspace_id AS workspace, fo.id AS object, cd.id AS document \
         FROM flow_objects fo JOIN collab_documents cd ON cd.object_id = fo.id WHERE fo.id = $1",
        vec![object_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    let boot = bootstrap::load(db, identity.document).await?;
    if let Some(expected) = expected_head_seq
        && expected != boot.head_seq
    {
        return Err(ApiError::Conflict(
            "expected_head_seq does not match current head".to_string(),
        ));
    }
    if execute && expected_head_seq.is_none() {
        return Err(ApiError::BadRequest(
            "expected_head_seq is required for execute".to_string(),
        ));
    }

    let mut engine = LoroCollabEngine::load(&boot.snapshot).map_err(|_| ApiError::Internal)?;
    for update in &boot.tail_updates {
        engine.import_update(&update.bytes).map_err(|_| ApiError::Internal)?;
    }
    if engine.frontier().as_bytes() != boot.head_frontier.as_slice() {
        return Err(ApiError::Conflict("resync_required".to_string()));
    }
    let semantic = engine.semantic_snapshot().map_err(|_| ApiError::Internal)?;
    let desired = ProjectionRow {
        document_seq: boot.head_seq,
        document_frontier: boot.head_frontier.clone(),
        title: engine.title().map_err(|_| ApiError::Internal)?,
        state: projection::state_json(&semantic).map_err(|_| ApiError::Internal)?,
        plain_text: projection::plain_text(&semantic),
    };
    let before = projection_row(db, object_id).await?;
    let before_hash = before.as_ref().map(projection_hash).transpose()?;
    let after_hash = projection_hash(&desired)?;
    let changed = before_hash.as_deref() != Some(after_hash.as_str());

    if execute && changed {
        let tx = db.begin().await?;
        super::collab::write::set_locked_phase_statement_budgets(&tx, 1).await?;
        let locked = LockedHead::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_seq, head_frontier FROM collab_documents WHERE id = $1 FOR UPDATE",
            vec![identity.document.into()],
        ))
        .one(&tx)
        .await?
        .ok_or_else(|| ApiError::NotFound("collab document not found".to_string()))?;
        if locked.head_seq != boot.head_seq || locked.head_frontier != boot.head_frontier {
            tx.rollback().await?;
            return Err(ApiError::Conflict(
                "document head changed during projection rebuild".to_string(),
            ));
        }
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_object_projections \
               (object_id, document_seq, document_frontier, title, state, plain_text, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, now()) \
             ON CONFLICT (object_id) DO UPDATE SET \
               document_seq = EXCLUDED.document_seq, document_frontier = EXCLUDED.document_frontier, \
               title = EXCLUDED.title, state = EXCLUDED.state, plain_text = EXCLUDED.plain_text, \
               updated_at = now()",
            vec![
                object_id.into(),
                desired.document_seq.into(),
                desired.document_frontier.clone().into(),
                desired.title.clone().into(),
                desired.state.clone().into(),
                desired.plain_text.clone().into(),
            ],
        ))
        .await?;
        tx.commit().await?;
        let persisted = projection_row(db, object_id)
            .await?
            .ok_or_else(|| ApiError::Conflict("projection missing after rebuild".to_string()))?;
        if projection_hash(&persisted)? != after_hash {
            return Err(ApiError::Conflict(
                "projection checksum mismatch after rebuild".to_string(),
            ));
        }
    }

    Ok(ProjectionRebuildResult {
        workspace_id: identity.workspace,
        object_id: identity.object,
        document_id: identity.document,
        head_seq: boot.head_seq,
        before_hash,
        after_hash,
        changed,
        executed: execute,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stderr)]
mod database_tests {
    use collab_core::{CollabEngine, LoroCollabEngine};
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
    use uuid::Uuid;

    use super::rebuild_projection;
    use crate::error::ApiError;

    const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

    struct Scratch {
        db: DatabaseConnection,
        name: String,
        admin_url: String,
    }

    #[derive(FromQueryResult)]
    struct Root {
        id: Uuid,
    }

    impl Scratch {
        async fn drop_self(self) {
            let Self { db, name, admin_url } = self;
            drop(db);
            let Ok(admin) = Database::connect(&admin_url).await else {
                return;
            };
            let _ = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }
    }

    async fn scratch() -> Option<Scratch> {
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
        let admin = Database::connect(&admin_url)
            .await
            .unwrap_or_else(|error| panic!("{TEST_DATABASE_URL_ENV} is unusable: {error}"));
        let name = "openpr_flow_projection_rebuild_v08".to_string();
        admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
            .await
            .expect("old scratch drops");
        admin
            .execute_unprepared(&format!("CREATE DATABASE \"{name}\""))
            .await
            .expect("scratch creates");
        let (prefix, _) = admin_url.rsplit_once('/')?;
        let db = Database::connect(format!("{prefix}/{name}"))
            .await
            .expect("scratch connects");
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .expect("migrations read")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
            .collect();
        paths.sort();
        for path in paths {
            db.execute_unprepared(&std::fs::read_to_string(&path).expect("migration reads"))
                .await
                .unwrap_or_else(|error| panic!("{} failed: {error}", path.display()));
        }
        Some(Scratch { db, name, admin_url })
    }

    #[tokio::test]
    async fn dry_run_changes_nothing_and_execute_restores_the_exact_canonical_projection() {
        let Some(scratch) = scratch().await else {
            eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
            return;
        };
        let user_id = Uuid::new_v4();
        let workspace_id = Uuid::new_v4();
        let object_id = Uuid::new_v4();
        let document_id = Uuid::new_v4();
        scratch
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO users (id,email,password_hash,name,role,is_active) \
                 VALUES ($1,$2,'!','rebuild','user',true)",
                vec![user_id.into(), format!("{user_id}@flow.test").into()],
            ))
            .await
            .expect("user seeds");
        scratch
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO workspaces (id,slug,name,created_by) VALUES ($1,$2,'rebuild',$3)",
                vec![workspace_id.into(), format!("ws-{workspace_id}").into(), user_id.into()],
            ))
            .await
            .expect("workspace seeds");
        let root = Root::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM flow_objects WHERE workspace_id=$1 AND object_type='navigator'",
            vec![workspace_id.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("root query")
        .expect("root exists");
        let mut engine = LoroCollabEngine::new_empty(7);
        engine.set_title("canonical title").expect("title sets");
        let snapshot = engine.export_snapshot().expect("snapshot exports");
        let frontier = engine.frontier().as_bytes().to_vec();
        scratch
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO flow_objects (id,workspace_id,object_type,parent_id,created_by) \
                 VALUES ($1,$2,'page',$3,$4)",
                vec![object_id.into(), workspace_id.into(), root.id.into(), user_id.into()],
            ))
            .await
            .expect("object seeds");
        scratch
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO collab_documents \
                   (id,object_id,format_version,snapshot,snapshot_frontier,head_frontier) \
                 VALUES ($1,$2,'1',$3,$4,$4)",
                vec![
                    document_id.into(),
                    object_id.into(),
                    snapshot.into(),
                    frontier.clone().into(),
                ],
            ))
            .await
            .expect("document seeds");
        scratch
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO flow_object_projections \
                   (object_id,document_seq,document_frontier,title,state,plain_text) \
                 VALUES ($1,0,$2,'corrupt replica','{}'::jsonb,'wrong')",
                vec![object_id.into(), frontier.into()],
            ))
            .await
            .expect("projection seeds");

        let plan = rebuild_projection(&scratch.db, object_id, Some(0), false)
            .await
            .expect("dry run succeeds");
        assert!(plan.changed);
        assert!(!plan.executed);
        let still_wrong: String = scratch
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT title FROM flow_object_projections WHERE object_id=$1",
                vec![object_id.into()],
            ))
            .await
            .expect("title query")
            .expect("projection exists")
            .try_get("", "title")
            .expect("title reads");
        assert_eq!(still_wrong, "corrupt replica", "dry-run must write nothing");

        let executed = rebuild_projection(&scratch.db, object_id, Some(0), true)
            .await
            .expect("execute succeeds");
        assert!(executed.changed);
        assert!(executed.executed);
        assert_eq!(executed.after_hash, plan.after_hash);
        let rebuilt_title: String = scratch
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT title FROM flow_object_projections WHERE object_id=$1",
                vec![object_id.into()],
            ))
            .await
            .expect("rebuilt title query")
            .expect("rebuilt projection exists")
            .try_get("", "title")
            .expect("rebuilt title reads");
        assert_eq!(rebuilt_title, "canonical title");
        let clean = rebuild_projection(&scratch.db, object_id, Some(0), false)
            .await
            .expect("verification dry run succeeds");
        assert!(!clean.changed);
        assert_eq!(clean.before_hash.as_deref(), Some(clean.after_hash.as_str()));
        assert!(matches!(
            rebuild_projection(&scratch.db, object_id, Some(1), true).await,
            Err(ApiError::Conflict(_))
        ));

        scratch.drop_self().await;
    }
}
