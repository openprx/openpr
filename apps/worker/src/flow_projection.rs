//! Accepted-only Flow full-text projection worker.
//!
//! `flow_object_projections` is the sole content source. The search table deliberately does not
//! read snapshots, updates, pending client data, or event payloads. A rebuild may replace an
//! accepted projection with an older sequence, so equality (not monotonic `>`) determines whether
//! a row needs rebuilding.

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use uuid::Uuid;

#[derive(Debug, FromQueryResult)]
struct ProjectionCandidate {
    object_id: Uuid,
    indexed_seq: i64,
    indexed_frontier: Vec<u8>,
    title: String,
    plain_text: String,
}

/// Maximum source rows one worker tick copies. Replicas may run concurrently: the source read is
/// immutable for the duration of the statement and the upsert is idempotent.
const MAX_BATCH_SIZE: u64 = 1_000;

/// Copies one bounded batch from the accepted projection read model and removes archived rows.
///
/// Returns the number of rows inserted/replaced plus rows removed for archival. Object deletion
/// is handled immediately by `flow_search_index.object_id ... ON DELETE CASCADE` and therefore is
/// intentionally not included in this tick-local count.
pub async fn run_tick(db: &DatabaseConnection, requested_batch_size: usize) -> anyhow::Result<u64> {
    let bounded = u64::try_from(requested_batch_size.max(1))
        .unwrap_or(MAX_BATCH_SIZE)
        .min(MAX_BATCH_SIZE);
    let tx = db.begin().await?;

    let candidates = ProjectionCandidate::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT p.object_id,
                   p.document_seq AS indexed_seq,
                   p.document_frontier AS indexed_frontier,
                   p.title,
                   p.plain_text
              FROM flow_object_projections p
              JOIN flow_objects fo ON fo.id = p.object_id
         LEFT JOIN flow_search_index si ON si.object_id = p.object_id
             WHERE fo.lifecycle_status = 'active'
               AND (
                    si.object_id IS NULL
                    OR si.indexed_seq <> p.document_seq
                    OR si.indexed_frontier <> p.document_frontier
                    OR si.title <> p.title
                    OR si.plain_text <> p.plain_text
               )
          ORDER BY p.updated_at ASC, p.object_id ASC
             LIMIT $1
        ",
        vec![i64::try_from(bounded).unwrap_or(i64::MAX).into()],
    ))
    .all(&tx)
    .await?;

    for candidate in &candidates {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO flow_search_index
                    (object_id, indexed_seq, indexed_frontier, title, plain_text, updated_at)
                VALUES ($1, $2, $3, $4, $5, now())
                ON CONFLICT (object_id) DO UPDATE
                   SET indexed_seq = EXCLUDED.indexed_seq,
                       indexed_frontier = EXCLUDED.indexed_frontier,
                       title = EXCLUDED.title,
                       plain_text = EXCLUDED.plain_text,
                       updated_at = now()
            ",
            vec![
                candidate.object_id.into(),
                candidate.indexed_seq.into(),
                candidate.indexed_frontier.clone().into(),
                candidate.title.clone().into(),
                candidate.plain_text.clone().into(),
            ],
        ))
        .await?;
    }

    let archived = tx
        .execute(Statement::from_string(
            DbBackend::Postgres,
            r"
                DELETE FROM flow_search_index si
                 USING flow_objects fo
                 WHERE fo.id = si.object_id
                   AND fo.lifecycle_status = 'archived'
            "
            .to_string(),
        ))
        .await?
        .rows_affected();
    tx.commit().await?;

    Ok(u64::try_from(candidates.len())
        .unwrap_or(u64::MAX)
        .saturating_add(archived))
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
    use uuid::Uuid;

    use super::run_tick;

    const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

    struct Scratch {
        db: DatabaseConnection,
        name: String,
        admin_url: String,
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

    async fn scratch(label: &str) -> Option<Scratch> {
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
        let admin = Database::connect(&admin_url)
            .await
            .unwrap_or_else(|error| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {error}"));
        let name = format!("openpr_worker_{label}");
        let quoted = format!("\"{name}\"");
        admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS {quoted} WITH (FORCE)"))
            .await
            .unwrap_or_else(|error| panic!("could not reset scratch database {name}: {error}"));
        admin
            .execute_unprepared(&format!("CREATE DATABASE {quoted}"))
            .await
            .unwrap_or_else(|error| panic!("could not create scratch database {name}: {error}"));
        let (prefix, _) = admin_url.rsplit_once('/')?;
        let db = Database::connect(format!("{prefix}/{name}"))
            .await
            .unwrap_or_else(|error| panic!("could not connect to scratch database {name}: {error}"));
        migrate(&db).await;
        Some(Scratch { db, name, admin_url })
    }

    async fn migrate(db: &DatabaseConnection) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
        let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .expect("migrations directory is readable")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
            .collect();
        paths.sort();
        for path in paths {
            let sql = std::fs::read_to_string(&path).expect("migration is readable");
            db.execute_unprepared(&sql)
                .await
                .unwrap_or_else(|error| panic!("applying {} failed: {error}", path.display()));
        }
    }

    macro_rules! scratch_or_skip {
        ($label:expr) => {
            match scratch($label).await {
                Some(scratch) => scratch,
                None => {
                    eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
                    return;
                }
            }
        };
    }

    async fn execute(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
        db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .unwrap_or_else(|error| panic!("fixture statement failed: {error}"));
    }

    async fn seed_projection(db: &DatabaseConnection) -> (Uuid, Uuid, Uuid) {
        let user_id = Uuid::new_v4();
        let workspace_id = Uuid::new_v4();
        let object_id = Uuid::new_v4();
        execute(
            db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) VALUES ($1, $2, '!', 'worker', 'user', true)",
            vec![user_id.into(), format!("{user_id}@flow.test").into()],
        )
        .await;
        execute(
            db,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'worker', $3)",
            vec![workspace_id.into(), format!("ws-{workspace_id}").into(), user_id.into()],
        )
        .await;
        execute(
            db,
            "INSERT INTO flow_objects (id, workspace_id, object_type, created_by) VALUES ($1, $2, 'page', $3)",
            vec![object_id.into(), workspace_id.into(), user_id.into()],
        )
        .await;
        execute(
            db,
            "INSERT INTO collab_documents (id, object_id, format_version, snapshot, snapshot_frontier, head_frontier) VALUES ($1, $2, '1', ''::bytea, ''::bytea, ''::bytea)",
            vec![Uuid::new_v4().into(), object_id.into()],
        )
        .await;
        execute(
            db,
            "INSERT INTO flow_object_projections (object_id, document_seq, document_frontier, title, state, plain_text) VALUES ($1, 3, $2, 'Accepted title', '{}'::jsonb, 'accepted body token')",
            vec![object_id.into(), vec![3_u8].into()],
        )
        .await;
        (workspace_id, object_id, user_id)
    }

    #[derive(FromQueryResult)]
    struct IndexedRow {
        indexed_seq: i64,
        indexed_frontier: Vec<u8>,
        title: String,
        plain_text: String,
    }

    async fn indexed(db: &DatabaseConnection, object_id: Uuid) -> Option<IndexedRow> {
        IndexedRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT indexed_seq, indexed_frontier, title, plain_text FROM flow_search_index WHERE object_id = $1",
            vec![object_id.into()],
        ))
        .one(db)
        .await
        .expect("index query succeeds")
    }

    #[tokio::test]
    async fn flow_projection_indexes_only_the_accepted_projection_and_tracks_lag() {
        let scratch = scratch_or_skip!("flow_projection_accepted");
        let (_, object_id, _) = seed_projection(&scratch.db).await;

        assert_eq!(run_tick(&scratch.db, 10).await.expect("worker tick succeeds"), 1);
        let row = indexed(&scratch.db, object_id).await.expect("index row exists");
        assert_eq!(row.indexed_seq, 3);
        assert_eq!(row.indexed_frontier, vec![3]);
        assert_eq!(row.title, "Accepted title");
        assert_eq!(row.plain_text, "accepted body token");

        execute(
            &scratch.db,
            "UPDATE collab_documents SET head_seq = 9, head_frontier = $2 WHERE object_id = $1",
            vec![object_id.into(), vec![9_u8].into()],
        )
        .await;
        assert_eq!(run_tick(&scratch.db, 10).await.expect("lagged tick succeeds"), 0);
        assert_eq!(
            indexed(&scratch.db, object_id)
                .await
                .expect("index remains")
                .indexed_seq,
            3
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_projection_replaces_the_index_when_the_accepted_projection_rolls_back() {
        let scratch = scratch_or_skip!("flow_projection_rollback");
        let (_, object_id, _) = seed_projection(&scratch.db).await;
        run_tick(&scratch.db, 10).await.expect("initial tick succeeds");
        execute(
            &scratch.db,
            "UPDATE flow_object_projections SET document_seq = 1, document_frontier = $2, title = 'Rolled back', plain_text = 'older accepted body' WHERE object_id = $1",
            vec![object_id.into(), vec![1_u8].into()],
        )
        .await;

        assert_eq!(run_tick(&scratch.db, 10).await.expect("rollback tick succeeds"), 1);
        let row = indexed(&scratch.db, object_id).await.expect("index row exists");
        assert_eq!(row.indexed_seq, 1);
        assert_eq!(row.indexed_frontier, vec![1]);
        assert_eq!(row.title, "Rolled back");
        assert_eq!(row.plain_text, "older accepted body");

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_projection_removes_archived_and_deleted_objects_and_restores_from_accepted_source() {
        let scratch = scratch_or_skip!("flow_projection_lifecycle");
        let (_, object_id, _) = seed_projection(&scratch.db).await;
        run_tick(&scratch.db, 10).await.expect("initial tick succeeds");

        execute(
            &scratch.db,
            "UPDATE flow_objects SET lifecycle_status = 'archived', archived_at = now() WHERE id = $1",
            vec![object_id.into()],
        )
        .await;
        assert_eq!(run_tick(&scratch.db, 10).await.expect("archive tick succeeds"), 1);
        assert!(indexed(&scratch.db, object_id).await.is_none());

        execute(
            &scratch.db,
            "UPDATE flow_objects SET lifecycle_status = 'active', archived_at = NULL WHERE id = $1",
            vec![object_id.into()],
        )
        .await;
        assert_eq!(run_tick(&scratch.db, 10).await.expect("restore tick succeeds"), 1);
        assert!(indexed(&scratch.db, object_id).await.is_some());

        execute(
            &scratch.db,
            "DELETE FROM flow_objects WHERE id = $1",
            vec![object_id.into()],
        )
        .await;
        assert!(
            indexed(&scratch.db, object_id).await.is_none(),
            "ON DELETE CASCADE removes the index row"
        );

        scratch.drop_self().await;
    }
}
