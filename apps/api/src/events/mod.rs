#![allow(clippy::too_long_first_doc_paragraph)]

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;

pub mod dispatcher;

pub struct BusinessEventInput {
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
}

pub async fn insert_business_event<C>(db: &C, input: BusinessEventInput) -> Result<Uuid, ApiError>
where
    C: ConnectionTrait,
{
    let event_id = Uuid::new_v4();

    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO business_events (
                id, workspace_id, project_id, event_type, aggregate_type, aggregate_id,
                actor_id, source, payload, metadata, correlation_id, causation_id,
                idempotency_key, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, now())
        ",
        vec![
            event_id.into(),
            input.workspace_id.into(),
            input.project_id.into(),
            input.event_type.into(),
            input.aggregate_type.into(),
            input.aggregate_id.into(),
            input.actor_id.into(),
            input.source.into(),
            input.payload.into(),
            input.metadata.into(),
            input.correlation_id.into(),
            input.causation_id.into(),
            input.idempotency_key.into(),
        ],
    ))
    .await?;

    Ok(event_id)
}

/// What [`insert_flow_event`] fills into the single, domain-transaction `event_dispatch` row it
/// writes alongside a `delivery_class=business` event. Both fields stay `None` for every event
/// type except `flow.content.accepted`, whose own `event_dispatch_document_id_fill_check` /
/// `event_dispatch_accepted_seq_fill_check` CHECK constraints require both filled together
/// (`migrations/0054_flow_data_layer.sql`, `events-v1.md` "投递").
pub struct FlowDispatchSpec {
    pub max_attempts: i32,
    pub document_id: Option<Uuid>,
    pub accepted_seq: Option<i64>,
}

/// [`insert_flow_event`]'s result: the row's identity, and whether this call actually inserted a
/// new `business_events` row (`true`) or the idempotency key matched an already-committed row and
/// this call is an idempotent replay (`false`) -- `events-v1.md` "same-key replay event id 不变".
#[derive(Debug, Clone, Copy)]
pub struct FlowEventOutcome {
    pub event_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub was_new: bool,
}

#[derive(Debug, FromQueryResult)]
struct BusinessEventIdentityRow {
    id: Uuid,
    created_at: DateTime<Utc>,
}

/// `events-v1.md`'s "必须在同一 events module 增加 transaction-scoped `insert_flow_event(tx,
/// input, delivery_class)`" clause.
///
/// Unlike [`insert_business_event`] (a bare `INSERT`, no `ON CONFLICT` -- documented there, per
/// `events-v1.md`, as the anti-pattern Flow must not repeat, and the reason
/// `find_idempotent_record`'s pre-transaction check in `routes::form` races a concurrent second
/// request today), this idempotently writes `business_events` via
/// `INSERT ... ON CONFLICT (workspace_id, idempotency_key) DO NOTHING RETURNING id`: the
/// correctness comes from the database's own unique index, not from a caller-side check that runs
/// before the transaction even opens. `dispatch: Some(..)` is the `delivery_class=business` half
/// of the same clause -- exactly one `event_dispatch` row is appended in the same transaction, but
/// only when this call actually inserted a new row; `dispatch: None` is the `delivery_class=
/// audit_only` half: the event is written but never dispatched.
///
/// `input.idempotency_key` being `None` never conflicts (the backing index is partial, `WHERE
/// idempotency_key IS NOT NULL` -- see `migrations/0031_business_events_outbox_inbox.sql`), so a
/// caller with no natural idempotency key (e.g. a content-write's `flow.content.accepted`, which
/// is deduplicated by `collab_updates.update_id` instead) gets a plain insert on every call,
/// exactly like [`insert_business_event`] today.
///
/// On a real conflict -- a concurrent request already inserted and committed the same
/// `(workspace_id, idempotency_key)` row -- `RETURNING` yields no row, and this reads the
/// existing row back **in the same transaction** instead of erroring, returning `was_new=false`
/// with its `event_id`. `event_dispatch` is not touched on that path, preserving the "恰好一行"
/// invariant even under a race the caller's own pre-check missed.
pub async fn insert_flow_event<C>(
    tx: &C,
    input: BusinessEventInput,
    dispatch: Option<FlowDispatchSpec>,
) -> Result<FlowEventOutcome, ApiError>
where
    C: ConnectionTrait,
{
    let inserted = BusinessEventIdentityRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO business_events (
                id, workspace_id, project_id, event_type, aggregate_type, aggregate_id,
                actor_id, source, payload, metadata, correlation_id, causation_id,
                idempotency_key, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, now())
            ON CONFLICT (workspace_id, idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING
            RETURNING id, created_at
        ",
        vec![
            Uuid::new_v4().into(),
            input.workspace_id.into(),
            input.project_id.into(),
            input.event_type.clone().into(),
            input.aggregate_type.clone().into(),
            input.aggregate_id.clone().into(),
            input.actor_id.into(),
            input.source.clone().into(),
            input.payload.clone().into(),
            input.metadata.clone().into(),
            input.correlation_id.into(),
            input.causation_id.into(),
            input.idempotency_key.clone().into(),
        ],
    ))
    .one(tx)
    .await?;

    let (event_id, created_at, was_new) = if let Some(row) = inserted {
        (row.id, row.created_at, true)
    } else {
        // Conflict: a `NULL` idempotency_key can never match the partial unique index, so this
        // branch is only reachable when the caller supplied a real key that another, already
        // committed transaction won the race on.
        let Some(idempotency_key) = input.idempotency_key.as_deref() else {
            return Err(ApiError::Internal);
        };
        let existing = BusinessEventIdentityRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, created_at FROM business_events WHERE workspace_id = $1 AND idempotency_key = $2",
            vec![input.workspace_id.into(), idempotency_key.into()],
        ))
        .one(tx)
        .await?
        .ok_or(ApiError::Internal)?;
        (existing.id, existing.created_at, false)
    };

    if was_new && let Some(spec) = dispatch {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO event_dispatch
                    (id, event_id, workspace_id, event_type, document_id, accepted_seq, max_attempts)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
            ",
            vec![
                Uuid::new_v4().into(),
                event_id.into(),
                input.workspace_id.into(),
                input.event_type.into(),
                spec.document_id.into(),
                spec.accepted_seq.into(),
                spec.max_attempts.into(),
            ],
        ))
        .await?;
    }

    Ok(FlowEventOutcome {
        event_id,
        created_at,
        was_new,
    })
}

// -------------------------------------------------------------------------------------------
// Real-database tests (opt-in via `OPENPR_TEST_DATABASE_URL`), matching
// `apps/api/src/events/dispatcher.rs`'s `dispatcher_database_tests` scratch-per-run convention.
// -------------------------------------------------------------------------------------------
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod database_tests {
    use sea_orm::{
        ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
    };
    use serde_json::json;
    use uuid::Uuid;

    use super::{BusinessEventInput, FlowDispatchSpec, insert_flow_event};

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
            .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));

        let name = format!("openpr_events_mod_{label}");
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

        Some(Scratch { db, name, admin_url })
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

    async fn seed_workspace(db: &DatabaseConnection) -> Uuid {
        let workspace_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'test', 'user', true)",
            vec![owner_id.into(), format!("{owner_id}@events-mod.test").into()],
        ))
        .await
        .expect("user insert succeeds");
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'events mod test', $3)",
            vec![
                workspace_id.into(),
                format!("ws-{workspace_id}").into(),
                owner_id.into(),
            ],
        ))
        .await
        .expect("workspace insert succeeds");
        workspace_id
    }

    fn business_input(workspace_id: Uuid, idempotency_key: Option<&str>) -> BusinessEventInput {
        BusinessEventInput {
            workspace_id,
            project_id: None,
            event_type: "flow.object.created".to_string(),
            aggregate_type: "flow_object".to_string(),
            aggregate_id: Uuid::new_v4().to_string(),
            actor_id: None,
            source: json!({ "surface": "system" }),
            payload: json!({}),
            metadata: json!({}),
            correlation_id: None,
            causation_id: None,
            idempotency_key: idempotency_key.map(str::to_string),
        }
    }

    #[derive(FromQueryResult)]
    struct Count {
        n: i64,
    }

    async fn count(db: &DatabaseConnection, sql: &str, params: Vec<sea_orm::Value>) -> i64 {
        Count::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, params))
            .one(db)
            .await
            .expect("count query runs")
            .expect("count query returns exactly one row")
            .n
    }

    #[tokio::test]
    async fn same_idempotency_key_replay_returns_the_original_event_id_and_appends_no_second_dispatch_row() {
        let scratch = scratch_or_skip!("insert-flow-event-idempotent");
        let workspace_id = seed_workspace(&scratch.db).await;

        let tx1 = scratch.db.begin().await.expect("tx1 begins");
        let first = insert_flow_event(
            &tx1,
            business_input(workspace_id, Some("dedupe-key-1")),
            Some(FlowDispatchSpec {
                max_attempts: 5,
                document_id: None,
                accepted_seq: None,
            }),
        )
        .await
        .expect("first insert succeeds");
        tx1.commit().await.expect("tx1 commits");
        assert!(first.was_new);

        let tx2 = scratch.db.begin().await.expect("tx2 begins");
        let second = insert_flow_event(
            &tx2,
            business_input(workspace_id, Some("dedupe-key-1")),
            Some(FlowDispatchSpec {
                max_attempts: 5,
                document_id: None,
                accepted_seq: None,
            }),
        )
        .await
        .expect("replay insert succeeds instead of erroring on the unique index");
        tx2.commit().await.expect("tx2 commits");

        assert!(
            !second.was_new,
            "a same-key replay must not be reported as a new insert"
        );
        assert_eq!(
            second.event_id, first.event_id,
            "same-key replay must return the original event id, not mint a new one"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM business_events WHERE workspace_id = $1 AND idempotency_key = $2",
                vec![workspace_id.into(), "dedupe-key-1".into()],
            )
            .await,
            1,
            "the unique index must have prevented a second business_events row"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_dispatch WHERE event_id = $1",
                vec![first.event_id.into()],
            )
            .await,
            1,
            "a same-key replay must not append a second event_dispatch row for the same event"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn no_idempotency_key_never_conflicts_and_dispatch_none_writes_no_dispatch_row() {
        let scratch = scratch_or_skip!("insert-flow-event-no-key");
        let workspace_id = seed_workspace(&scratch.db).await;

        let tx = scratch.db.begin().await.expect("tx begins");
        let first = insert_flow_event(&tx, business_input(workspace_id, None), None)
            .await
            .expect("insert with no idempotency key succeeds");
        let second = insert_flow_event(&tx, business_input(workspace_id, None), None)
            .await
            .expect("a second insert with no idempotency key also succeeds, not treated as a conflict");
        tx.commit().await.expect("tx commits");

        assert!(first.was_new);
        assert!(second.was_new);
        assert_ne!(first.event_id, second.event_id, "each NULL-key insert is its own row");
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_dispatch WHERE event_id IN ($1, $2)",
                vec![first.event_id.into(), second.event_id.into()],
            )
            .await,
            0,
            "dispatch=None (delivery_class=audit_only) must never write an event_dispatch row"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn business_dispatch_row_carries_document_id_and_accepted_seq_for_content_accepted() {
        let scratch = scratch_or_skip!("insert-flow-event-content");
        let workspace_id = seed_workspace(&scratch.db).await;
        let document_id = Uuid::new_v4();

        let mut input = business_input(workspace_id, None);
        input.event_type = "flow.content.accepted".to_string();
        input.aggregate_type = "flow_document".to_string();

        let tx = scratch.db.begin().await.expect("tx begins");
        let outcome = insert_flow_event(
            &tx,
            input,
            Some(FlowDispatchSpec {
                max_attempts: 5,
                document_id: Some(document_id),
                accepted_seq: Some(7),
            }),
        )
        .await
        .expect("content-accepted insert succeeds");
        tx.commit().await.expect("tx commits");

        let row_document_id: Uuid = {
            #[derive(FromQueryResult)]
            struct Row {
                document_id: Option<Uuid>,
                accepted_seq: Option<i64>,
            }
            let row = Row::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT document_id, accepted_seq FROM event_dispatch WHERE event_id = $1",
                vec![outcome.event_id.into()],
            ))
            .one(&scratch.db)
            .await
            .expect("query runs")
            .expect("exactly one event_dispatch row exists");
            assert_eq!(row.accepted_seq, Some(7));
            row.document_id
                .expect("document_id is filled for flow.content.accepted")
        };
        assert_eq!(row_document_id, document_id);

        scratch.drop_self().await;
    }
}
