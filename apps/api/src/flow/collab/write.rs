//! The server write algorithm: `ADR-0010`'s "写入算法" / `collab-protocol-v1.md`'s "服务端写入顺序",
//! including the commit-time `authz_epoch` fencing barrier (`ADR-0012` §3.1) and the document row
//! lock (`ADR-0010`).
//!
//! ```text
//! validate update bytes under limits (before any engine state is allocated)
//!   -> [layer 0] acquire the instance-local document coordinator
//!   -> hydrate/rebase the warm cache to the observed DB head, outside any lock
//!   -> isolated apply (spawned worker: import_update + semantic_snapshot + check_snapshot,
//!      `collab_core::isolation::isolated_apply`) + projection prepare, outside any lock
//!   -> begin transaction, SET LOCAL lock_timeout/statement_timeout
//!   -> [layer 1] SELECT authz_epoch ... FOR SHARE, held to commit; CAS against checked_epoch
//!   -> SELECT collab_documents ... FOR UPDATE
//!   -> if locked head != prepared head: rollback, bounded rebase outside the lock
//!   -> allocate seq; insert update + head + projection + business event + one event_dispatch row
//!   -> commit
//!   -> update the warm cache to the committed head
//! ```
//!
//! Lock-content discipline (`collab-protocol-v1.md`: "锁内严禁 snapshot/tail load、CRDT apply、
//! semantic diff、projection compute、网络 I/O 或等待 async mutex"): every decode, isolated apply,
//! `semantic_snapshot`, and projection JSON/plain-text render happens in [`hydrate_and_apply`],
//! which returns *before* [`run_locked_phase`] ever calls `db.begin()`. The decode/apply/shape-
//! validate step itself (`import_update` + `semantic_snapshot` + `collab_core::limits::
//! check_snapshot`) runs inside a resource-ceilinged worker process spawned by
//! `collab_core::isolation::isolated_apply` (`ADR-0014`; see `crates/collab-core/src/isolation/
//! host.rs`'s module doc), not in this process at all. The only work [`run_locked_phase`] does is
//! the epoch fence, the row lock, the head-match recheck, and the five fixed, parameterized
//! inserts/updates — no engine call, no cache call, and no broadcast happen inside it or between
//! its `begin`/`commit`.
//!
//! Broadcasting `accepted` happens in [`accept_update`] itself, strictly after commit but still
//! before this function returns — i.e. still while the caller's [`DocumentCoordinator`] permit is
//! held (`accept_update`'s own doc comment on why `_permit` stays alive for its whole body). This
//! is deliberate, not incidental: `collab-protocol-v1.md`'s "accepted 出站顺序" requires broadcasts
//! for one document to reach [`super::registry::SessionRegistry`] in commit order, and the
//! coordinator permit is the only thing in this instance that actually serializes writers for one
//! `document_id`. Broadcasting *after* `accept_update` returns — the shape this module shipped
//! with through v0.4's early builds — drops that serialization exactly where it matters: two
//! commits for the same document can each finish (commit + release the permit) before either one
//! reaches the registry, and normal async scheduling gives no guarantee the one that committed
//! first also broadcasts first. Enqueuing to [`super::registry::SessionRegistry`]'s
//! `mpsc::UnboundedSender` is a fast, synchronous, in-memory operation, not the network I/O or
//! blocking work the lock-content discipline above forbids inside the *database* row lock — the
//! coordinator permit is a separate, lighter-weight admission gate `coordinator.rs`'s own doc
//! comment already says exists "purely to serialize same-instance writers", so extending its hold
//! this far is within its documented purpose.

#![allow(clippy::items_after_statements, clippy::too_long_first_doc_paragraph)]

use std::time::Duration;

use collab_core::{CollabEngine, CollabError, InputLimits, LoroCollabEngine};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;
use crate::events::{BusinessEventInput, FlowDispatchSpec, insert_flow_event};
use crate::flow::projection;

use super::authz::fence_epoch_for_share;
use super::bootstrap::{self, content_hash};
use super::cache::WarmCache;
use super::coordinator::DocumentCoordinator;
use super::frame::{Frame, PROTOCOL_VERSION, RejectedCode, encode_bytes};
use super::limits::{DOCUMENT_LOCK_HOLD_MS_MAX, DOCUMENT_LOCK_WAIT_MS_MAX, MAX_REBASE_ATTEMPTS};
use super::registry::SessionRegistry;
use super::snapshot::{self, SnapshotAdvancer, Trigger};

pub struct UpdateRequest {
    pub document_id: Uuid,
    pub update_id: Uuid,
    pub bytes: Vec<u8>,
    pub idempotency_key: Option<String>,
    pub origin_client_id: Option<String>,
    pub message: Option<String>,
    pub actor_id: Uuid,
    pub workspace_id: Uuid,
    /// The `authz_epoch` the caller's effective permission was last verified against (`open` time,
    /// or the most recent successful write). See [`fence_epoch_for_share`].
    pub checked_epoch: i64,
    /// Optimistic-concurrency guard some callers (the `POST .../commands` REST surface) supply:
    /// when `Some`, the update is rejected `stale_frontier` unless it equals the document's
    /// observed `head_frontier` at hydrate time (re-checked on every bounded-rebase attempt, so a
    /// caller cannot straddle a concurrent commit the way a single pre-check would). WebSocket
    /// `update` frames never set this — a directly-typed CRDT edit merges commutatively regardless
    /// of the frontier it was locally based on, which is the whole point of shipping a CRDT; this
    /// guard exists only for callers that explicitly want strict optimistic locking instead.
    pub expected_frontier: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct Accepted {
    pub update_id: Uuid,
    pub head_seq: i64,
    pub head_frontier: Vec<u8>,
    pub projection_seq: i64,
    pub event_id: Uuid,
    /// The document's `head_frontier` immediately before this update was applied
    /// (`collab_updates.before_frontier`). Callers that relay this update's `bytes` to other
    /// sessions (`collab-protocol-v1.md`'s `update` frame) need it for that frame's
    /// `base_frontier` field.
    pub before_frontier: Vec<u8>,
    /// `true` when this document's tail crossed a soft snapshot-advancement threshold
    /// (`flow::collab::snapshot::Trigger::Soft`) as observed *before* this write was applied —
    /// callers that hold a `'static`-reachable [`SnapshotAdvancer`] (`flow::collab::session`,
    /// `flow::command`) should call [`snapshot::spawn_background`] when this is `true`. Always
    /// `false` on an idempotent replay ([`find_prior_update`]): replaying an already-accepted
    /// update observes nothing new about the tail.
    pub should_advance_snapshot: bool,
}

#[derive(Debug, Clone)]
pub struct Rejected {
    pub update_id: Option<Uuid>,
    pub code: RejectedCode,
    pub recoverable: bool,
    pub details: Option<Value>,
    pub current_seq: Option<i64>,
    pub current_frontier: Option<Vec<u8>>,
}

pub enum AcceptOutcome {
    Accepted(Accepted),
    Rejected(Rejected),
}

const fn rejected(code: RejectedCode, recoverable: bool, update_id: Option<Uuid>) -> AcceptOutcome {
    AcceptOutcome::Rejected(Rejected {
        update_id,
        code,
        recoverable,
        details: None,
        current_seq: None,
        current_frontier: None,
    })
}

fn contention(update_id: Option<Uuid>, reason: &str) -> AcceptOutcome {
    AcceptOutcome::Rejected(Rejected {
        update_id,
        code: RejectedCode::ServerDraining,
        recoverable: true,
        details: Some(serde_json::json!({"reason": "contention", "retry_after_ms": 200})),
        current_seq: None,
        current_frontier: None,
    })
    .tap_reason(reason)
}

// Small local extension so `contention`'s `tracing` call reads naturally at the call site without
// a second statement.
trait TapReason {
    fn tap_reason(self, reason: &str) -> Self;
}
impl TapReason for AcceptOutcome {
    fn tap_reason(self, reason: &str) -> Self {
        tracing::warn!(reason, "collab write: contention, returning a recoverable rejection");
        self
    }
}

fn reject_from_collab_error(update_id: Option<Uuid>, err: &CollabError) -> AcceptOutcome {
    // `error-mapping-v1.md`'s `limit_exceeded` row: `details={limit_kind,limit,observed?,...}` --
    // `limit_kind` alone is not enough, `limit`/`observed` must also be recoverable from the
    // caller-visible rejection, not just logged server-side.
    let details = match err {
        CollabError::LimitExceeded {
            limit_kind,
            limit,
            observed,
        } => Some(serde_json::json!({"limit_kind": limit_kind, "limit": limit, "observed": observed})),
        CollabError::InputTooLarge {
            input: "update",
            actual_bytes,
            max_bytes,
        } => Some(serde_json::json!({"limit_kind": "update_bytes", "limit": max_bytes, "observed": actual_bytes})),
        _ => None,
    };
    if let Some(details) = details {
        return AcceptOutcome::Rejected(Rejected {
            update_id,
            code: RejectedCode::LimitExceeded,
            recoverable: false,
            details: Some(details),
            current_seq: None,
            current_frontier: None,
        });
    }
    rejected(RejectedCode::InvalidUpdate, false, update_id)
}

struct ObservedHead {
    object_id: Uuid,
    format_version: String,
    head_seq: i64,
    head_frontier: Vec<u8>,
}

/// `collab_documents.engine` is not selected here: the `collab_documents_engine_check` CHECK
/// constraint already guarantees it is always `'loro'` for every row this package can ever read
/// (`INSERT`s all hardcode it, see `flow::repository::insert_collab_document`), so a runtime
/// branch on it here would be dead code the moment it was written, not defensive programming.
async fn read_observed_head<C: ConnectionTrait>(conn: &C, document_id: Uuid) -> Result<Option<ObservedHead>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        object_id: Uuid,
        format_version: String,
        head_seq: i64,
        head_frontier: Vec<u8>,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT object_id, format_version, head_seq, head_frontier FROM collab_documents WHERE id = $1",
        vec![document_id.into()],
    ))
    .one(conn)
    .await?;
    Ok(row.map(|r| ObservedHead {
        object_id: r.object_id,
        format_version: r.format_version,
        head_seq: r.head_seq,
        head_frontier: r.head_frontier,
    }))
}

/// A prior accepted update with this `update_id`, if any (idempotent replay: `collab_updates`'s
/// `(document_id, update_id)` unique constraint is what this pre-empts hitting as a raw conflict).
async fn find_prior_update<C: ConnectionTrait>(
    conn: &C,
    document_id: Uuid,
    update_id: Uuid,
) -> Result<Option<Accepted>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        seq: i64,
        before_frontier: Vec<u8>,
        after_frontier: Vec<u8>,
        projection_seq: i64,
        event_id: Uuid,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT seq, before_frontier, after_frontier, projection_seq, event_id FROM collab_updates \
         WHERE document_id = $1 AND update_id = $2",
        vec![document_id.into(), update_id.into()],
    ))
    .one(conn)
    .await?;
    Ok(row.map(|r| Accepted {
        update_id,
        head_seq: r.seq,
        head_frontier: r.after_frontier,
        projection_seq: r.projection_seq,
        event_id: r.event_id,
        before_frontier: r.before_frontier,
        should_advance_snapshot: false,
    }))
}

/// Everything `ADR-0010`'s write algorithm requires to happen *outside* any lock: hydrate the
/// warm cache to the observed DB head (or rebuild from the bootstrap loader on a miss/stale
/// entry), fork an isolated candidate, apply the update, and prepare the projection. Never opens a
/// database transaction and never touches `WarmCache::put` for anything but re-seeding the
/// observed-head base it just built (not the post-update candidate — that only happens after
/// commit, in [`accept_update`]).
struct Prepared {
    observed: ObservedHead,
    candidate: LoroCollabEngine,
    after_frontier: Vec<u8>,
    content_hash_hex: String,
    title: String,
    state_json: Value,
    plain_text: String,
    decoded_bytes_hint: u64,
}

enum HydrateOutcome {
    Prepared(Box<Prepared>),
    Rejected(AcceptOutcome),
}

async fn hydrate_and_apply(
    db: &DatabaseConnection,
    cache: &WarmCache,
    document_id: Uuid,
    update_id: Uuid,
    bytes: &[u8],
    expected_frontier: Option<&[u8]>,
) -> Result<HydrateOutcome, ApiError> {
    let Some(observed) = read_observed_head(db, document_id).await? else {
        return Ok(HydrateOutcome::Rejected(rejected(
            RejectedCode::NotFound,
            false,
            Some(update_id),
        )));
    };

    // Optimistic-concurrency guard (`UpdateRequest::expected_frontier`'s doc comment): checked
    // against the *observed* head on every hydrate/rebase attempt, never a value read once
    // before the coordinator permit or before a rebase — so a caller cannot straddle a concurrent
    // commit the way a single pre-check would.
    if let Some(expected) = expected_frontier
        && expected != observed.head_frontier.as_slice()
    {
        return Ok(HydrateOutcome::Rejected(AcceptOutcome::Rejected(Rejected {
            update_id: Some(update_id),
            code: RejectedCode::StaleFrontier,
            recoverable: true,
            details: None,
            current_seq: Some(observed.head_seq),
            current_frontier: Some(observed.head_frontier.clone()),
        })));
    }

    let base_engine = match cache.fork_matching(document_id, observed.head_seq, &observed.format_version) {
        Ok(Some(engine)) => engine,
        Ok(None) => {
            let boot = match bootstrap::load(db, document_id).await {
                Ok(boot) => boot,
                Err(ApiError::Conflict(_)) => {
                    return Ok(HydrateOutcome::Rejected(rejected(
                        RejectedCode::ResyncRequired,
                        true,
                        Some(update_id),
                    )));
                }
                Err(other) => return Err(other),
            };
            let mut engine = LoroCollabEngine::load(&boot.snapshot).map_err(|err| {
                tracing::error!(error = %err, "collab write: bootstrap snapshot failed to decode");
                ApiError::Internal
            })?;
            for tail in &boot.tail_updates {
                engine.import_update(&tail.bytes).map_err(|err| {
                    tracing::error!(error = %err, "collab write: bootstrap tail update failed to re-apply");
                    ApiError::Internal
                })?;
            }
            engine
        }
        Err(err) => {
            return Ok(HydrateOutcome::Rejected(reject_from_collab_error(
                Some(update_id),
                &err,
            )));
        }
    };

    // Seed the observed-head base back into the cache (the base, not the post-update candidate
    // below) so a hot document's next write hits cache even after this one started from a miss.
    let base_snapshot_bytes = base_engine.export_snapshot().map_err(|err| {
        tracing::error!(error = %err, "collab write: base engine export_snapshot failed before isolated apply");
        ApiError::Internal
    })?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let decoded_bytes_hint = base_snapshot_bytes.len() as u64;
    if let Ok(seed) = base_engine.fork() {
        cache.put(
            document_id,
            seed,
            observed.format_version.clone(),
            observed.head_seq,
            observed.head_frontier.clone(),
            decoded_bytes_hint,
        );
    }

    // `ADR-0010`'s write algorithm: decode -> limit -> diff -> policy -> projection prepare, all
    // outside any lock and before the DB transaction begins. Unlike `hydrate_and_apply`'s
    // predecessor (which called `LoroCollabEngine::import_update`/`semantic_snapshot`/
    // `collab_core::limits::check_snapshot` directly, in-process, with no resource ceiling
    // enforced at all), decode, apply, and shape validation now happen inside
    // [`collab_core::isolation::isolated_apply`] -- a freshly spawned, CPU/wall/memory-ceilinged
    // worker process (`ADR-0014`, `contracts/limits-v1.md`'s "Isolated decode/apply" table; see
    // `crates/collab-core/src/isolation/host.rs`'s module doc for the full design). It performs
    // the identical `import_update` + `semantic_snapshot` + `check_snapshot` sequence this
    // function used to run directly, and returns either the resulting document snapshot or the
    // same `CollabError` shape a direct in-process call would have produced; only the resource
    // ceilings (CPU/wall/memory) are new outcomes, mapped below to the matching `limit_kind`.
    //
    // `spawn_blocking`: `isolated_apply` performs blocking process spawn/pipe I/O/`wait`
    // synchronously, which must never run directly on a Tokio worker thread.
    let update_bytes_owned = bytes.to_vec();
    let isolated_result = tokio::task::spawn_blocking(move || {
        collab_core::isolation::isolated_apply(&base_snapshot_bytes, &update_bytes_owned)
    })
    .await
    .map_err(|err| {
        tracing::error!(error = %err, "collab write: isolated apply task panicked or was cancelled");
        ApiError::Internal
    })?;

    let candidate_snapshot = match isolated_result {
        Ok(success) => success.snapshot,
        Err(collab_core::isolation::IsolatedApplyError::Collab(err)) => {
            return Ok(HydrateOutcome::Rejected(reject_from_collab_error(
                Some(update_id),
                &err,
            )));
        }
        Err(collab_core::isolation::IsolatedApplyError::CpuCeiling) => {
            let err = CollabError::LimitExceeded {
                limit_kind: "decode_apply_cpu_ms",
                limit: collab_core::isolation::DECODE_APPLY_CPU_MS_MAX,
                observed: collab_core::isolation::DECODE_APPLY_CPU_MS_MAX + 1,
            };
            return Ok(HydrateOutcome::Rejected(reject_from_collab_error(
                Some(update_id),
                &err,
            )));
        }
        Err(collab_core::isolation::IsolatedApplyError::WallCeiling) => {
            let err = CollabError::LimitExceeded {
                limit_kind: "decode_apply_wall_ms",
                limit: collab_core::isolation::DECODE_APPLY_WALL_MS_MAX,
                observed: collab_core::isolation::DECODE_APPLY_WALL_MS_MAX + 1,
            };
            return Ok(HydrateOutcome::Rejected(reject_from_collab_error(
                Some(update_id),
                &err,
            )));
        }
        Err(collab_core::isolation::IsolatedApplyError::MemoryCeiling) => {
            let err = CollabError::LimitExceeded {
                limit_kind: "isolated_apply_memory_bytes",
                limit: collab_core::isolation::ISOLATED_APPLY_MEMORY_BYTES_MAX,
                observed: collab_core::isolation::ISOLATED_APPLY_MEMORY_BYTES_MAX + 1,
            };
            return Ok(HydrateOutcome::Rejected(reject_from_collab_error(
                Some(update_id),
                &err,
            )));
        }
        Err(collab_core::isolation::IsolatedApplyError::HostFailure(reason)) => {
            tracing::error!(
                reason,
                document_id = %document_id,
                "collab write: isolated apply host failure"
            );
            return Err(ApiError::Internal);
        }
    };

    // Reconstructing `LoroCollabEngine` from the isolated worker's already-shape-validated result
    // is cheap and safe: `check_snapshot` already bounded this document's tree depth/container
    // count/block count/text volume inside the metered window above, so this load and the
    // `semantic_snapshot`/`title` reads below operate on state with a known worst-case size, not
    // on attacker-controlled input directly.
    let candidate = LoroCollabEngine::load(&candidate_snapshot).map_err(|err| {
        tracing::error!(error = %err, "collab write: reload of isolated-apply result snapshot failed");
        ApiError::Internal
    })?;
    let after_frontier = candidate.frontier().as_bytes().to_vec();

    let semantic = candidate.semantic_snapshot().map_err(|err| {
        tracing::error!(error = %err, "collab write: semantic_snapshot failed after a successful isolated apply");
        ApiError::Internal
    })?;

    let title = candidate.title().map_err(|err| {
        tracing::error!(error = %err, "collab write: title read failed after a successful isolated apply");
        ApiError::Internal
    })?;
    let state_json = projection::state_json(&semantic).map_err(|_| ApiError::Internal)?;
    let plain_text = projection::plain_text(&semantic);
    let content_hash_hex = content_hash(bytes);

    Ok(HydrateOutcome::Prepared(Box::new(Prepared {
        observed,
        candidate,
        after_frontier,
        content_hash_hex,
        title,
        state_json,
        plain_text,
        decoded_bytes_hint,
    })))
}

enum LockedOutcome {
    Committed(Accepted),
    Rebase,
    EpochMismatch,
}

/// Everything between `begin` and `commit`. No engine call, no cache call, no network I/O, no
/// `.await` on anything but the database itself.
#[allow(clippy::too_many_arguments)]
async fn run_locked_phase(
    db: &DatabaseConnection,
    request: &UpdateRequest,
    prepared: &Prepared,
    dispatch_max_attempts: i32,
) -> Result<LockedOutcome, ApiError> {
    let tx = db.begin().await?;
    tx.execute_unprepared(&format!("SET LOCAL lock_timeout = '{DOCUMENT_LOCK_WAIT_MS_MAX}ms'"))
        .await?;
    tx.execute_unprepared(&format!(
        "SET LOCAL statement_timeout = '{DOCUMENT_LOCK_HOLD_MS_MAX}ms'"
    ))
    .await?;

    // [layer 1] the commit-time epoch fence, held to commit. Only a genuine epoch mismatch
    // (`ApiError::Conflict` -- `authz_epoch` really did move past `checked_epoch`) means the
    // caller's permission is stale and must come back as a permanent, non-recoverable
    // `PolicyRejected`. Any other error here (a `lock_timeout`/`statement_timeout` hit while
    // waiting on the `FOR SHARE`, a dropped connection, ...) says nothing about authorization at
    // all -- it must propagate as a real `Err` so the caller's existing rebase/contention retry
    // handles it, exactly like every other database error in this function already does.
    // Conflating the two used to report ordinary transient contention as a false, permanent
    // policy rejection (never retried, since `EpochMismatch` is a terminal branch below).
    match fence_epoch_for_share(&tx, request.workspace_id, request.checked_epoch).await {
        Ok(()) => {}
        Err(ApiError::Conflict(_)) => {
            let _ = tx.rollback().await;
            return Ok(LockedOutcome::EpochMismatch);
        }
        Err(err) => {
            let _ = tx.rollback().await;
            return Err(err);
        }
    }

    #[derive(FromQueryResult)]
    struct LockedHead {
        head_seq: i64,
        byte_count: i64,
        update_count: i64,
    }
    let locked = LockedHead::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT head_seq, byte_count, update_count FROM collab_documents WHERE id = $1 FOR UPDATE",
        vec![request.document_id.into()],
    ))
    .one(&tx)
    .await?
    .ok_or(ApiError::Internal)?;

    if locked.head_seq != prepared.observed.head_seq {
        let _ = tx.rollback().await;
        return Ok(LockedOutcome::Rebase);
    }

    let new_head_seq = locked.head_seq + 1;
    let before_frontier = prepared.observed.head_frontier.clone();
    let after_frontier = prepared.after_frontier.clone();

    let event_id = insert_flow_event(
        &tx,
        BusinessEventInput {
            workspace_id: request.workspace_id,
            project_id: None,
            event_type: "flow.content.accepted".to_string(),
            aggregate_type: "flow_document".to_string(),
            aggregate_id: request.document_id.to_string(),
            actor_id: Some(request.actor_id),
            source: serde_json::json!({ "surface": "web" }),
            payload: serde_json::json!({
                "object_id": prepared.observed.object_id,
                "document_id": request.document_id,
                "accepted_seq": new_head_seq,
                "projection_seq": new_head_seq,
                "changed_block_ids": Vec::<Uuid>::new(),
            }),
            metadata: serde_json::json!({ "message": request.message }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
        Some(FlowDispatchSpec {
            max_attempts: dispatch_max_attempts,
            document_id: Some(request.document_id),
            accepted_seq: Some(new_head_seq),
        }),
    )
    .await?
    .event_id;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO collab_updates
                (document_id, seq, update_id, content_hash, idempotency_key, before_frontier, after_frontier,
                 bytes, actor_id, origin_surface, origin_client_id, projection_seq, event_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'web', $10, $2, $11)
        ",
        vec![
            request.document_id.into(),
            new_head_seq.into(),
            request.update_id.into(),
            prepared.content_hash_hex.clone().into(),
            request.idempotency_key.clone().into(),
            before_frontier.into(),
            after_frontier.clone().into(),
            request.bytes.clone().into(),
            request.actor_id.into(),
            request.origin_client_id.clone().into(),
            event_id.into(),
        ],
    ))
    .await?;

    #[allow(clippy::cast_possible_wrap)]
    let new_byte_count = locked.byte_count + request.bytes.len() as i64;
    let new_update_count = locked.update_count + 1;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE collab_documents
            SET head_seq = $2, head_frontier = $3, byte_count = $4, update_count = $5, updated_at = now()
            WHERE id = $1
        ",
        vec![
            request.document_id.into(),
            new_head_seq.into(),
            after_frontier.clone().into(),
            new_byte_count.into(),
            new_update_count.into(),
        ],
    ))
    .await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE flow_object_projections
            SET document_seq = $2, document_frontier = $3, title = $4, state = $5, plain_text = $6, updated_at = now()
            WHERE object_id = $1
        ",
        vec![
            prepared.observed.object_id.into(),
            new_head_seq.into(),
            after_frontier.clone().into(),
            prepared.title.clone().into(),
            prepared.state_json.clone().into(),
            prepared.plain_text.clone().into(),
        ],
    ))
    .await?;

    tx.commit().await?;

    Ok(LockedOutcome::Committed(Accepted {
        update_id: request.update_id,
        head_seq: new_head_seq,
        head_frontier: after_frontier,
        projection_seq: new_head_seq,
        event_id,
        before_frontier: prepared.observed.head_frontier.clone(),
        // Overwritten by `accept_update` from the tail-trigger reading it took before this
        // locked phase ever ran; `run_locked_phase` has no business computing this itself (it
        // would mean an extra query inside the lock, which lock discipline forbids).
        should_advance_snapshot: false,
    }))
}

/// Runs [`hydrate_and_apply`] + [`run_locked_phase`] with bounded rebase, inside one coordinator
/// permit for `request.document_id`.
///
/// # Errors
/// Only for a database failure the caller cannot recover from by retrying the same request later
/// (everything recoverable — contention, epoch mismatch, limit/decode rejection — comes back as
/// `Ok(AcceptOutcome::Rejected(..))` instead).
// `_permit` is intentionally held for the entire bounded-rebase loop below, not dropped as soon
// as it is last read: releasing it between rebase attempts would let a second writer for the same
// document interleave mid-retry, which is exactly what the coordinator exists to prevent.
#[allow(clippy::significant_drop_tightening)]
pub async fn accept_update(
    db: &DatabaseConnection,
    cache: &WarmCache,
    coordinator: &DocumentCoordinator,
    registry: &SessionRegistry,
    snapshot_advancer: &SnapshotAdvancer,
    dispatch_max_attempts: i32,
    exclude_session_id: Option<Uuid>,
    request: UpdateRequest,
) -> Result<AcceptOutcome, ApiError> {
    if let Err(err) = InputLimits::default().validate_update(&request.bytes) {
        return Ok(reject_from_collab_error(Some(request.update_id), &err));
    }

    // No broadcast on this path: `prior` is an already-committed, already-broadcast seq (this is
    // exactly the request-retried-after-a-lost-response case), so re-broadcasting it would only
    // ever produce a `SeqDecision::Duplicate` a receiving session's `egress::EgressSequencer`
    // drops anyway (`collab-protocol-v1.md`: "seq<=last_applied_seq 是幂等重复,忽略") — wasted work
    // on every other open session's channel for no observable effect.
    if let Some(prior) = find_prior_update(db, request.document_id, request.update_id).await? {
        return Ok(AcceptOutcome::Accepted(prior));
    }

    let Ok(_permit) = coordinator.acquire(request.document_id).await else {
        return Ok(contention(Some(request.update_id), "coordinator acquisition timed out"));
    };

    // Gate 7 `minimal_snapshot_advancement_bounds_tail`: a single, unlocked tail-shape read taken
    // once per accept attempt (not once per rebase iteration below) — never inside a transaction,
    // never blocking a concurrent writer for a *different* document. `limits-v1.md`'s hard
    // trigger ("接受下一 update 前必须先成功推进 snapshot,不能继续扩大 tail") is enforced
    // synchronously right here, before this update is even hydrated: if the document is already
    // at/over a hard boundary, this write waits for a real advancement to succeed (or gives up as
    // recoverable contention) rather than growing the tail further. The soft trigger only marks
    // `should_advance_snapshot` on the eventual `Accepted` result — it must never block this
    // write; `flow::collab::session`/`flow::command` spawn the actual background advancement
    // after they see that flag, once this function has already returned.
    let mut should_advance_snapshot = false;
    if let Some(stats) = snapshot::read_tail_stats(db, request.document_id).await? {
        match snapshot::evaluate(&stats, snapshot_advancer.last_rebuild_wall_ms(request.document_id)) {
            Trigger::Hard => match snapshot::advance(db, request.document_id, snapshot_advancer).await {
                Ok(snapshot::AdvanceOutcome::Advanced | snapshot::AdvanceOutcome::NothingToAdvance) => {}
                Ok(snapshot::AdvanceOutcome::Contended) => {
                    return Ok(contention(
                        Some(request.update_id),
                        "snapshot checkpoint required before this document's tail can grow further",
                    ));
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        document_id = %request.document_id,
                        "collab write: forced snapshot checkpoint failed, treating as recoverable contention"
                    );
                    return Ok(contention(Some(request.update_id), "forced snapshot checkpoint failed"));
                }
            },
            Trigger::Soft => should_advance_snapshot = true,
            Trigger::None => {}
        }
    }

    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let prepared = match hydrate_and_apply(
            db,
            cache,
            request.document_id,
            request.update_id,
            &request.bytes,
            request.expected_frontier.as_deref(),
        )
        .await?
        {
            HydrateOutcome::Prepared(prepared) => prepared,
            HydrateOutcome::Rejected(outcome) => return Ok(outcome),
        };

        let locked = tokio::time::timeout(
            Duration::from_millis(DOCUMENT_LOCK_HOLD_MS_MAX.saturating_mul(4)),
            run_locked_phase(db, &request, &prepared, dispatch_max_attempts),
        )
        .await;

        match locked {
            Ok(Ok(LockedOutcome::Committed(mut accepted))) => {
                let format_version = prepared.observed.format_version.clone();
                cache.put(
                    request.document_id,
                    prepared.candidate,
                    format_version,
                    accepted.head_seq,
                    accepted.head_frontier.clone(),
                    prepared.decoded_bytes_hint,
                );
                accepted.should_advance_snapshot = should_advance_snapshot;
                // Still inside the coordinator permit acquired above -- see this module's doc
                // comment on why that is what makes this broadcast's order match commit order.
                broadcast_committed_update(registry, exclude_session_id, &request, &accepted);
                return Ok(AcceptOutcome::Accepted(accepted));
            }
            Ok(Ok(LockedOutcome::EpochMismatch)) => {
                return Ok(rejected(RejectedCode::PolicyRejected, false, Some(request.update_id)));
            }
            Ok(Ok(LockedOutcome::Rebase)) | Err(_) => {
                if attempts >= MAX_REBASE_ATTEMPTS {
                    return Ok(contention(Some(request.update_id), "rebase attempts exhausted"));
                }
            }
            Ok(Err(db_err)) => {
                tracing::warn!(error = %db_err, "collab write: locked phase failed, treating as recoverable contention");
                if attempts >= MAX_REBASE_ATTEMPTS {
                    return Ok(contention(Some(request.update_id), "locked phase failed repeatedly"));
                }
            }
        }
    }
}

/// Sends the `update`+`accepted` frame pair `collab-protocol-v1.md` requires for one committed
/// content write, to every other session with `request.document_id` open. The single call site
/// both `flow::collab::session`'s WebSocket path and `flow::command::execute_content_command`'s
/// REST path route through (via [`accept_update`]) — see this module's top doc comment for why
/// this runs here, still inside the caller's coordinator permit, rather than after
/// [`accept_update`] returns as earlier builds of this module did.
fn broadcast_committed_update(
    registry: &SessionRegistry,
    exclude_session_id: Option<Uuid>,
    request: &UpdateRequest,
    accepted: &Accepted,
) {
    let update_frame = Frame::Update {
        protocol_version: PROTOCOL_VERSION,
        document_id: request.document_id,
        update_id: accepted.update_id,
        base_frontier: encode_bytes(&accepted.before_frontier),
        bytes: encode_bytes(&request.bytes),
        idempotency_key: request.idempotency_key.clone(),
        origin: request.origin_client_id.clone().unwrap_or_default(),
        message: request.message.clone(),
    };
    let accepted_frame = Frame::Accepted {
        protocol_version: PROTOCOL_VERSION,
        document_id: request.document_id,
        update_id: accepted.update_id,
        head_seq: accepted.head_seq,
        head_frontier: encode_bytes(&accepted.head_frontier),
        projection_seq: accepted.projection_seq,
        event_id: accepted.event_id,
    };
    registry.broadcast(request.document_id, &update_frame, exclude_session_id);
    registry.broadcast(request.document_id, &accepted_frame, exclude_session_id);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod isolation_rejection_tests {
    use collab_core::CollabError;
    use uuid::Uuid;

    use super::{AcceptOutcome, reject_from_collab_error};
    use crate::flow::collab::frame::RejectedCode;

    /// `limits-v1.md`'s `limit_exceeded` details rule (`details={limit_kind,limit,observed?,
    /// retry_after_ms?}`) applied to the three "Isolated decode/apply" ceilings.
    ///
    /// What this pins is the *wire shape* of the rejection `hydrate_and_apply` builds for each
    /// `collab_core::isolation::IsolatedApplyError` resource outcome — that all three name a
    /// `limit_kind` from the frozen table and carry a numeric `limit` equal to the constant
    /// `collab_core::isolation` actually enforces, rather than an empty `details` a REST/MCP/CLI
    /// caller could not branch on. It is deliberately *not* a claim that the ceilings fire:
    /// that is proven against the real worker process (SIGPROF kill, wall watchdog, counting
    /// allocator) by `collab-core`'s own
    /// `decode_apply_cpu_ms_ceiling_accepts_just_under_and_kills_with_cpu_ceiling_just_over_the_boundary`
    /// and its two siblings in `crates/collab-core/src/isolation/host.rs`.
    #[test]
    fn every_isolated_apply_resource_ceiling_rejects_with_its_frozen_limit_kind_and_a_numeric_limit() {
        let cases = [
            ("decode_apply_cpu_ms", collab_core::isolation::DECODE_APPLY_CPU_MS_MAX),
            ("decode_apply_wall_ms", collab_core::isolation::DECODE_APPLY_WALL_MS_MAX),
            (
                "isolated_apply_memory_bytes",
                collab_core::isolation::ISOLATED_APPLY_MEMORY_BYTES_MAX,
            ),
        ];
        for (limit_kind, limit) in cases {
            let update_id = Uuid::new_v4();
            let outcome = reject_from_collab_error(
                Some(update_id),
                &CollabError::LimitExceeded {
                    limit_kind,
                    limit,
                    observed: limit + 1,
                },
            );
            let AcceptOutcome::Rejected(rejected) = outcome else {
                panic!("{limit_kind} must reject, not accept");
            };
            assert_eq!(rejected.code, RejectedCode::LimitExceeded);
            assert_eq!(rejected.update_id, Some(update_id));
            let details = rejected
                .details
                .unwrap_or_else(|| panic!("{limit_kind} must carry limit_exceeded details"));
            assert_eq!(details["limit_kind"], limit_kind);
            assert_eq!(
                details["limit"], limit,
                "{limit_kind}'s `limit` must be the constant collab_core::isolation enforces"
            );
            assert_eq!(details["observed"], limit + 1);
        }
    }
}

// ---- Real-database tests (opt-in via `OPENPR_TEST_DATABASE_URL`) ----
//
// Mirrors the scratch-database convention `apps/api/src/routes/flow.rs::flow_database_tests` and
// `apps/api/src/main.rs::migration_runner_database_tests` already use: own throwaway database per
// run, migrated from `migrations/*.sql` on disk, dropped on the way out.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]
mod database_tests {
    use collab_core::{CollabEngine, LoroCollabEngine, NodeId, NodeKind, Operation};
    use platform::{
        app::AppState,
        config::{AppConfig, Secret},
    };
    use sea_orm::{
        ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
    };
    use serde_json::Value;
    use std::time::Duration;
    use uuid::Uuid;

    use super::{AcceptOutcome, SnapshotAdvancer, UpdateRequest, accept_update};
    use crate::flow::collab::authz;
    use crate::flow::collab::bootstrap;
    use crate::flow::collab::bootstrap::fetch_update_range;
    use crate::flow::collab::cache::WarmCache;
    use crate::flow::collab::coordinator::DocumentCoordinator;
    use crate::flow::collab::frame::RejectedCode;
    use crate::flow::collab::registry::SessionRegistry;
    use crate::flow::command::{CreateObjectInput, create_object};

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

        let name = format!("openpr_collab_write_{label}");
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

        Some(Scratch { db, name, admin_url })
    }

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
                    eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
                    return;
                }
            }
        };
    }

    fn state_for(db: DatabaseConnection) -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "collab-write-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("collab-write-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
        }
    }

    async fn exec(state: &AppState, sql: &str, values: Vec<sea_orm::Value>) {
        state
            .db
            .execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
    }

    async fn seed_workspace(state: &AppState) -> (Uuid, Uuid) {
        let workspace_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        exec(
            state,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'test', 'user', true)",
            vec![owner_id.into(), format!("{owner_id}@collab.test").into()],
        )
        .await;
        exec(
            state,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'collab write test', $3)",
            vec![
                workspace_id.into(),
                format!("ws-{workspace_id}").into(),
                owner_id.into(),
            ],
        )
        .await;
        exec(
            state,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
            vec![workspace_id.into(), owner_id.into()],
        )
        .await;
        exec(
            state,
            "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled) VALUES ($1, true)",
            vec![workspace_id.into()],
        )
        .await;
        (workspace_id, owner_id)
    }

    async fn create_page(state: &AppState, workspace_id: Uuid, actor_id: Uuid) -> (Uuid, Uuid) {
        let accepted = create_object(
            state,
            CreateObjectInput {
                workspace_id,
                actor_id,
                object_type: "page".to_string(),
                project_id: None,
                parent_object_id: None,
                title: "Write Path Test Page".to_string(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
            },
        )
        .await
        .expect("object creation succeeds");
        (accepted.object.id, accepted.object.document_id)
    }

    async fn count_collab_updates(state: &AppState, document_id: Uuid, update_id: Uuid) -> i64 {
        #[derive(FromQueryResult)]
        struct Row {
            n: i64,
        }
        let row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM collab_updates WHERE document_id = $1 AND update_id = $2",
            vec![document_id.into(), update_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("count query runs")
        .expect("count query returns a row");
        row.n
    }

    fn a_valid_update_against(document_id_snapshot: &[u8]) -> (Vec<u8>, LoroCollabEngine) {
        let mut engine = LoroCollabEngine::load(document_id_snapshot).expect("loads");
        let base_frontier = engine.frontier();
        engine.set_title("mutated by test").expect("set_title succeeds");
        let update = engine.export_from(&base_frontier).expect("export succeeds");
        (update, engine)
    }

    #[tokio::test]
    async fn accept_update_commits_and_advances_head_seq_against_a_real_database() {
        let scratch = scratch_or_skip!("accept-basic");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        #[derive(FromQueryResult)]
        struct SnapshotRow {
            snapshot: Vec<u8>,
        }
        let snapshot_row = SnapshotRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT snapshot FROM collab_documents WHERE id = $1",
            vec![document_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("query runs")
        .expect("row exists");

        let (update_bytes, _engine) = a_valid_update_against(&snapshot_row.snapshot);
        let checked_epoch = authz::read_epoch(&state.db, workspace_id).await.expect("epoch reads");

        let cache = WarmCache::new();
        let coordinator = DocumentCoordinator::new();
        let registry = SessionRegistry::new();
        let snapshot_advancer = SnapshotAdvancer::new();
        let update_id = Uuid::new_v4();
        let outcome = accept_update(
            &state.db,
            &cache,
            &coordinator,
            &registry,
            &snapshot_advancer,
            10,
            None,
            UpdateRequest {
                document_id,
                update_id,
                bytes: update_bytes,
                idempotency_key: None,
                origin_client_id: Some("test-client".to_string()),
                message: None,
                actor_id: owner_id,
                workspace_id,
                checked_epoch,
                expected_frontier: None,
            },
        )
        .await
        .expect("accept_update does not hit a hard database error");

        let AcceptOutcome::Accepted(accepted) = outcome else {
            panic!("expected Accepted");
        };
        assert_eq!(accepted.head_seq, 1);
        assert_eq!(accepted.update_id, update_id);
        assert_eq!(count_collab_updates(&state, document_id, update_id).await, 1);

        scratch.drop_self().await;
    }

    /// ★ The TOCTOU race `collab-protocol-v1.md` names verbatim: "A 重验 epoch=E → B 提交 E+1 并
    /// 撤权 → A 插入 update 并在 B 之后 commit" must **not** result in "撤权后仍写入成功".
    ///
    /// This is not a unit-level mock: `B` is a real, separate database connection that takes a
    /// genuine `SELECT ... FOR UPDATE` on the workspace's `authz_epoch` row and holds it open
    /// while `A`'s write is in flight, so `A`'s `fence_epoch_for_share` (`SELECT ... FOR SHARE`)
    /// must actually block on Postgres's own row lock — proving the barrier is a lock, not a
    /// pre-insert timestamp check that could race B and lose.
    #[tokio::test]
    async fn epoch_fencing_blocks_a_write_that_straddles_a_concurrent_revocation() {
        let scratch = scratch_or_skip!("epoch-toctou");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        #[derive(FromQueryResult)]
        struct SnapshotRow {
            snapshot: Vec<u8>,
        }
        let snapshot_row = SnapshotRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT snapshot FROM collab_documents WHERE id = $1",
            vec![document_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("query runs")
        .expect("row exists");
        let (update_bytes, _engine) = a_valid_update_against(&snapshot_row.snapshot);

        // `A` reverifies permission/reads the epoch "outside the transaction" here, exactly like
        // `session::reverify_open` does at `open` time.
        let original_epoch = authz::read_epoch(&state.db, workspace_id).await.expect("epoch reads");

        // A second, independent database connection for `B` (a real separate session, not a
        // second handle to the same connection -- otherwise `FOR UPDATE` and `FOR SHARE` would
        // trivially self-block on one connection instead of exercising cross-transaction locking).
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).expect("checked by scratch_or_skip! above");
        let db_url = admin_url
            .rsplit_once('/')
            .map(|(prefix, _)| format!("{prefix}/{}", scratch.name))
            .expect("db url");
        let db_b = Database::connect(&db_url).await.expect("B connects independently");

        let (b_holding_tx, b_holding_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_b_tx, release_b_rx) = tokio::sync::oneshot::channel::<()>();

        let b_task = tokio::spawn(async move {
            let tx = db_b.begin().await.expect("B begins");
            // The real "冲突锁 FOR UPDATE" an authorization-changing transaction takes
            // (`ADR-0012` §3.1) -- held open across the `oneshot` handshake below so `A`'s
            // `FOR SHARE` genuinely has to wait on Postgres, not on test-harness timing.
            tx.query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT authz_epoch FROM flow_workspace_settings WHERE workspace_id = $1 FOR UPDATE",
                vec![workspace_id.into()],
            ))
            .await
            .expect("B locks the epoch row");
            b_holding_tx.send(()).expect("A is still waiting to receive this");

            release_b_rx.await.expect("A releases B");
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE flow_workspace_settings SET authz_epoch = authz_epoch + 1 WHERE workspace_id = $1",
                vec![workspace_id.into()],
            ))
            .await
            .expect("B advances the epoch");
            tx.commit().await.expect("B commits, releasing the row lock");
        });

        b_holding_rx.await.expect("B signals it holds the lock");

        let cache = WarmCache::new();
        let coordinator = DocumentCoordinator::new();
        let registry = SessionRegistry::new();
        let snapshot_advancer = SnapshotAdvancer::new();
        let update_id = Uuid::new_v4();
        let db_for_a = state.db.clone();
        let a_task = tokio::spawn(async move {
            accept_update(
                &db_for_a,
                &cache,
                &coordinator,
                &registry,
                &snapshot_advancer,
                10,
                None,
                UpdateRequest {
                    document_id,
                    update_id,
                    bytes: update_bytes,
                    idempotency_key: None,
                    origin_client_id: Some("test-client-a".to_string()),
                    message: None,
                    actor_id: owner_id,
                    workspace_id,
                    checked_epoch: original_epoch,
                    expected_frontier: None,
                },
            )
            .await
        });

        // Give A a real chance to reach `fence_epoch_for_share` and block on B's `FOR UPDATE`
        // before B is allowed to proceed -- this is what makes the interleaving deterministic:
        // A's write is provably in flight, past its own permission check, when B commits. Kept
        // well under `DOCUMENT_LOCK_WAIT_MS_MAX` (100ms, `run_locked_phase`'s own `lock_timeout`)
        // so this proves A is *blocked* on B's lock, not that A's own lock_timeout fired first.
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(
            !a_task.is_finished(),
            "A must still be blocked on B's row lock at this point"
        );

        release_b_tx.send(()).expect("B is still waiting to receive this");
        b_task.await.expect("B task joins");

        let a_result = a_task.await.expect("A task joins").expect("no hard database error");
        let AcceptOutcome::Rejected(rejected) = a_result else {
            panic!("epoch fencing must reject A's write once B has revoked after A's permission check, got Accepted");
        };
        assert_eq!(
            rejected.code,
            RejectedCode::PolicyRejected,
            "the epoch mismatch must surface as policy_rejected, not any other rejection code"
        );

        // The decisive assertion: "撤权后仍写入成功" must not have happened -- no collab_updates
        // row for this update_id, no matter what the in-memory outcome claimed.
        assert_eq!(
            count_collab_updates(&state, document_id, update_id).await,
            0,
            "epoch fencing failed: a write was persisted after a concurrent revocation committed first"
        );

        let final_epoch = authz::read_epoch(&state.db, workspace_id).await.expect("epoch reads");
        assert_eq!(
            final_epoch,
            original_epoch + 1,
            "B's revocation must still have taken effect"
        );

        scratch.drop_self().await;
    }

    /// Root-cause reproduction for the full-suite flake: `fence_epoch_for_share`'s `SELECT ...
    /// FOR SHARE` can fail for reasons that have nothing to do with `authz_epoch` ever moving --
    /// most concretely, Postgres's own `lock_timeout` (`DOCUMENT_LOCK_WAIT_MS_MAX`, 100ms) firing
    /// while it waits on a row lock some *other* transaction happens to be holding a moment too
    /// long (exactly what many scratch databases hammering one shared Postgres instance under
    /// `cargo test --workspace` produce). `B` here holds a real `FOR UPDATE` on the same row for
    /// 150ms -- past `A`'s 100ms `lock_timeout` -- then rolls back having never touched
    /// `authz_epoch` at all. No authorization ever changed; this is pure transient contention.
    ///
    /// Before the fix this reads as `LockedOutcome::EpochMismatch` (any `Err` from the fence
    /// check was treated as a real mismatch) and surfaces as a permanent, non-recoverable
    /// `PolicyRejected` on `A`'s very first attempt, with no retry. After the fix, only a genuine
    /// `ApiError::Conflict` may do that; every other error (this lock timeout included)
    /// propagates as a real `Err`, which `accept_update`'s existing rebase loop already retries --
    /// so once B's lock is gone, the exact same write goes on to commit normally.
    #[tokio::test]
    async fn epoch_fence_lock_timeout_must_not_surface_as_policy_rejected() {
        let scratch = scratch_or_skip!("epoch-lock-timeout");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        #[derive(FromQueryResult)]
        struct SnapshotRow {
            snapshot: Vec<u8>,
        }
        let snapshot_row = SnapshotRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT snapshot FROM collab_documents WHERE id = $1",
            vec![document_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("query runs")
        .expect("row exists");
        let (update_bytes, _engine) = a_valid_update_against(&snapshot_row.snapshot);

        let original_epoch = authz::read_epoch(&state.db, workspace_id).await.expect("epoch reads");

        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).expect("checked by scratch_or_skip! above");
        let db_url = admin_url
            .rsplit_once('/')
            .map(|(prefix, _)| format!("{prefix}/{}", scratch.name))
            .expect("db url");
        let db_b = Database::connect(&db_url).await.expect("B connects independently");

        let (b_holding_tx, b_holding_rx) = tokio::sync::oneshot::channel::<()>();

        let b_task = tokio::spawn(async move {
            let tx = db_b.begin().await.expect("B begins");
            // The exact row `fence_epoch_for_share` takes `FOR SHARE` on -- but B here stands in
            // for *any* transient holder of this lock, not an authorization change.
            tx.query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT authz_epoch FROM flow_workspace_settings WHERE workspace_id = $1 FOR UPDATE",
                vec![workspace_id.into()],
            ))
            .await
            .expect("B locks the epoch row");
            b_holding_tx.send(()).expect("A is still waiting to receive this");

            // Held well past A's 100ms `lock_timeout` so A's own `FOR SHARE` is guaranteed to be
            // cancelled by Postgres (`55P03 lock_not_available`), not merely to block and then
            // succeed once granted.
            tokio::time::sleep(Duration::from_millis(150)).await;

            // B never advances `authz_epoch` and rolls back: nothing about authorization ever
            // changed here, only the row was momentarily locked.
            tx.rollback()
                .await
                .expect("B rolls back, releasing the row lock without changing authz_epoch");
        });

        b_holding_rx.await.expect("B signals it holds the lock");

        let cache = WarmCache::new();
        let coordinator = DocumentCoordinator::new();
        let registry = SessionRegistry::new();
        let snapshot_advancer = SnapshotAdvancer::new();
        let update_id = Uuid::new_v4();
        let db_for_a = state.db.clone();
        let a_task = tokio::spawn(async move {
            accept_update(
                &db_for_a,
                &cache,
                &coordinator,
                &registry,
                &snapshot_advancer,
                10,
                None,
                UpdateRequest {
                    document_id,
                    update_id,
                    bytes: update_bytes,
                    idempotency_key: None,
                    origin_client_id: Some("test-client-a".to_string()),
                    message: None,
                    actor_id: owner_id,
                    workspace_id,
                    checked_epoch: original_epoch,
                    expected_frontier: None,
                },
            )
            .await
        });

        let (a_joined, b_joined) = tokio::join!(a_task, b_task);
        b_joined.expect("B task joins");
        let a_result = a_joined.expect("A task joins").expect("no hard database error");

        match a_result {
            AcceptOutcome::Accepted(accepted) => {
                assert_eq!(
                    accepted.update_id, update_id,
                    "A's own write must be the one that landed"
                );
            }
            AcceptOutcome::Rejected(rejected) => {
                panic!(
                    "a transient lock_timeout on the epoch fence must never surface as a rejection -- \
                     B never changed authz_epoch, this must retry until B's lock clears instead of \
                     reporting a false permanent rejection, got {rejected:?}"
                );
            }
        }

        let final_epoch = authz::read_epoch(&state.db, workspace_id).await.expect("epoch reads");
        assert_eq!(
            final_epoch, original_epoch,
            "B never touched authz_epoch -- it must be exactly what it started as"
        );
        assert_eq!(
            count_collab_updates(&state, document_id, update_id).await,
            1,
            "A's update must have been persisted exactly once, after B's lock cleared"
        );

        scratch.drop_self().await;
    }

    /// Lock-discipline proof, part 1: the locked phase never calls anything CRDT/cache-shaped.
    /// `run_locked_phase` (private to this module) takes only `&Prepared` (already-applied engine
    /// state) and issues five fixed, parameterized statements between `begin` and `commit` -- there
    /// is no `LoroCollabEngine` method, no `WarmCache` method, and no broadcast call reachable from
    /// its body. This is enforced structurally (see the module's own doc comment for the itemized
    /// trace of which function does what) and is additionally exercised end-to-end by
    /// [`accept_update_commits_and_advances_head_seq_against_a_real_database`]: if any lock-scoped
    /// I/O leaked into `run_locked_phase`, the `SET LOCAL statement_timeout` set at its start
    /// would make that test flaky/slow under contention, which it is not.
    ///
    /// Lock-discipline proof, part 2 (a real assertion, not a comment): the document row lock
    /// timeout budgets are real Postgres `SET LOCAL` values, not aspirational constants -- this
    /// checks the exact frozen `limits-v1.md` numbers `run_locked_phase` sends over the wire.
    #[test]
    fn lock_timeout_budgets_match_the_frozen_limits_v1_numbers() {
        assert_eq!(super::DOCUMENT_LOCK_WAIT_MS_MAX, 100);
        assert_eq!(super::DOCUMENT_LOCK_HOLD_MS_MAX, 100);
    }

    /// `flow::collab::egress::EgressSequencer`'s `SeqDecision::Gap` backfill query
    /// (`collab-protocol-v1.md` "accepted 出站顺序"): proves `fetch_update_range` reads back the
    /// exact `[from_seq, to_seq]` slice of real `collab_updates` rows a gap needs -- same
    /// `before_frontier`/`after_frontier`/`bytes`/`event_id`/`projection_seq` the production write
    /// path (`accept_update`, exercised above) actually committed, not recomputed -- and returns a
    /// short read (not an error, not padding) when part of the requested range does not exist, so
    /// `flow::collab::session::resolve_egress_gap` can tell "fully backfillable" from "must resync"
    /// by row count alone.
    #[tokio::test]
    async fn fetch_update_range_returns_the_exact_persisted_slice_and_a_short_read_past_head() {
        let scratch = scratch_or_skip!("fetch-update-range");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        #[derive(FromQueryResult)]
        struct SnapshotRow {
            snapshot: Vec<u8>,
        }
        let snapshot_row = SnapshotRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT snapshot FROM collab_documents WHERE id = $1",
            vec![document_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("query runs")
        .expect("row exists");
        let mut engine = LoroCollabEngine::load(&snapshot_row.snapshot).expect("loads");

        let cache = WarmCache::new();
        let coordinator = DocumentCoordinator::new();
        let registry = SessionRegistry::new();
        let snapshot_advancer = SnapshotAdvancer::new();

        // Commit 3 real updates (head_seq 1..=3) through the exact production write path.
        let mut committed = Vec::new();
        for label in ["seq-1", "seq-2", "seq-3"] {
            let base_frontier = engine.frontier();
            engine.set_title(label).expect("set_title succeeds");
            let bytes = engine.export_from(&base_frontier).expect("export succeeds");
            let checked_epoch = authz::read_epoch(&state.db, workspace_id).await.expect("epoch reads");
            let outcome = accept_update(
                &state.db,
                &cache,
                &coordinator,
                &registry,
                &snapshot_advancer,
                10,
                None,
                UpdateRequest {
                    document_id,
                    update_id: Uuid::new_v4(),
                    bytes,
                    idempotency_key: None,
                    origin_client_id: Some("range-test".to_string()),
                    message: None,
                    actor_id: owner_id,
                    workspace_id,
                    checked_epoch,
                    expected_frontier: None,
                },
            )
            .await
            .expect("accept_update does not hit a hard database error");
            let AcceptOutcome::Accepted(accepted) = outcome else {
                panic!("expected Accepted for {label}");
            };
            committed.push(accepted);
        }

        // Exact contiguous slice covering the middle two commits (seq 2..=3).
        let rows = fetch_update_range(&state.db, document_id, 2, 3)
            .await
            .expect("range query runs");
        assert_eq!(rows.len(), 2, "must return exactly the two rows in [2,3]");
        assert_eq!(rows[0].seq, 2);
        assert_eq!(rows[1].seq, 3);
        assert_eq!(rows[0].update_id, committed[1].update_id);
        assert_eq!(rows[1].update_id, committed[2].update_id);
        assert_eq!(
            rows[0].before_frontier, committed[0].head_frontier,
            "before_frontier of seq 2 must chain from seq 1's committed head_frontier"
        );
        assert_eq!(rows[0].after_frontier, committed[1].head_frontier);
        assert_eq!(rows[0].event_id, committed[1].event_id);
        assert_eq!(rows[0].projection_seq, committed[1].projection_seq);
        assert_eq!(rows[0].origin_client_id.as_deref(), Some("range-test"));

        // A range extending past the real head (only 3 updates exist) must come back short, not
        // padded and not an error -- this is exactly the signal `resolve_egress_gap` uses to
        // decide "give up and resync" instead of forwarding a partial catch-up.
        let short_rows = fetch_update_range(&state.db, document_id, 2, 10)
            .await
            .expect("range query runs even past head");
        assert_eq!(
            short_rows.len(),
            2,
            "only seq 2 and 3 exist -- a short read, not padding and not an error"
        );

        // A range entirely past head returns empty, not an error.
        let empty_rows = fetch_update_range(&state.db, document_id, 50, 60)
            .await
            .expect("range query runs for a range with no rows");
        assert!(empty_rows.is_empty());

        scratch.drop_self().await;
    }

    // ---- Call-direction proofs for `collab_core::limits::check_snapshot` on the WebSocket write
    // path, i.e. `hydrate_and_apply` reached through the exact `accept_update` a real WebSocket
    // `update` frame (and a REST content command, which shares this same function) goes through.
    // Every case here builds real CRDT update bytes with a locally-owned `LoroCollabEngine` (no
    // discrete `Operation` list is ever handed to the server -- exactly the "opaque update" shape
    // this module's own doc comment on `check_snapshot`'s call site describes) and submits them
    // through the production `accept_update`, against a real Postgres-backed document.

    /// Reconstructs the document's *current* full state exactly the way the production hydrate
    /// path does (`bootstrap::load`'s snapshot + tail replay -- `collab_documents.snapshot` is
    /// only advanced by background snapshot advancement, never on every accepted update, so
    /// reading that column directly after a prior accepted write would silently reload a stale,
    /// pre-write document), runs `mutate` against an isolated fork of it, and exports the
    /// resulting update relative to the pre-mutation frontier -- the exact "isolated fork, mutate,
    /// `export_from(base_frontier)`" shape a real client (or `flow::command`'s own REST relay)
    /// produces, just built directly here instead of through a client SDK.
    async fn build_update_from_current(
        state: &AppState,
        document_id: Uuid,
        mutate: impl FnOnce(&mut LoroCollabEngine),
    ) -> Vec<u8> {
        let boot = bootstrap::load(&state.db, document_id).await.expect("bootstrap loads");
        let mut engine = LoroCollabEngine::load(&boot.snapshot).expect("loads");
        for tail in &boot.tail_updates {
            engine.import_update(&tail.bytes).expect("tail update re-applies");
        }
        let base_frontier = engine.frontier();
        mutate(&mut engine);
        engine.export_from(&base_frontier).expect("export succeeds")
    }

    async fn count_event_dispatch(state: &AppState, document_id: Uuid) -> i64 {
        #[derive(FromQueryResult)]
        struct Row {
            n: i64,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM event_dispatch WHERE document_id = $1",
            vec![document_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("count query runs")
        .expect("count query returns a row")
        .n
    }

    /// The `limit_kind`s of `limits-v1.md`'s "Isolated decode/apply" table — the three ceilings
    /// that bound *how much machine* one apply may consume, not what shape the resulting document
    /// may have.
    ///
    /// They are the only `limit_exceeded` kinds whose verdict depends on the host rather than on
    /// the update under test: the same update that is applied in well under
    /// `decode_apply_cpu_ms_max` on an idle machine really can cross it when 500 other tests are
    /// saturating every core (observed: `{"limit":50,"limit_kind":"decode_apply_cpu_ms",
    /// "observed":51}` rejecting an update that is accepted every time the test runs alone). The
    /// enforcement is correct in both cases — the isolated worker was killed exactly as
    /// `ADR-0014` requires — which is precisely why the boundary fixtures below must not read it
    /// as their own verdict.
    const LOAD_DEPENDENT_LIMIT_KINDS: [&str; 3] = [
        "decode_apply_cpu_ms",
        "decode_apply_wall_ms",
        "isolated_apply_memory_bytes",
    ];

    /// Whether `outcome` is a rejection whose *verdict* depends on concurrent machine load rather
    /// than on the submitted update, and which a contract-compliant caller therefore retries.
    ///
    /// Two families qualify, and nothing else:
    /// * `server_draining` — `error-mapping-v1.md` defines it as recoverable ("客户端保留 intent
    ///   后重试"), and `write::contention` raises it for coordinator/lock timeouts and rebase
    ///   exhaustion, all of which are pure contention artifacts of many scratch databases sharing
    ///   one Postgres.
    /// * `limit_exceeded` with one of [`LOAD_DEPENDENT_LIMIT_KINDS`] — see that constant.
    ///
    /// A `limit_exceeded` naming any *shape* ceiling (`container_count`, `document_block_count`,
    /// `text_block_chars`, `document_text_chars`, `tree_depth`, `update_bytes`, ...) is never
    /// retried: those are the deterministic verdicts every caller of [`submit`] is actually
    /// asserting on, and they are returned on the first attempt, unretried.
    fn is_load_dependent(outcome: &AcceptOutcome) -> bool {
        let AcceptOutcome::Rejected(rejected) = outcome else {
            return false;
        };
        match rejected.code {
            RejectedCode::ServerDraining => true,
            RejectedCode::LimitExceeded => rejected
                .details
                .as_ref()
                .and_then(|details| details.get("limit_kind"))
                .and_then(Value::as_str)
                .is_some_and(|kind| LOAD_DEPENDENT_LIMIT_KINDS.contains(&kind)),
            _ => false,
        }
    }

    /// Submits `bytes` and retries every [`is_load_dependent`] rejection (`server_draining`, and
    /// `limit_exceeded` naming one of the three isolated decode/apply resource ceilings) until
    /// either it stops happening or a wall-clock deadline passes -- `error-mapping-v1.md`: that code is defined
    /// as recoverable, "客户端保留 intent 后重试", the exact behavior a real caller is
    /// contractually expected to have, with no contract-stated upper bound on how long a
    /// compliant caller keeps trying (unlike `limit_exceeded`, which is final and never retried).
    /// This module's own `epoch_fence_lock_timeout_must_not_surface_as_policy_rejected` test
    /// already documents that many scratch databases hammering one shared Postgres instance under
    /// `cargo test --workspace` produces real, transient lock/rebase contention independent of any
    /// application bug; the several structural-limit tests below submit many real transactions in
    /// a tight loop (building up to `container_count_max`/`document_block_count_max` fixture
    /// state) and are exactly the shape most likely to observe it, including sustained multi-
    /// second congestion windows a small fixed attempt count was observed not to outlast.
    /// Retrying here changes nothing about what is under test: a `limit_exceeded` rejection (the
    /// actual assertion every caller of this function cares about) is never `ServerDraining` and
    /// is always returned on the first attempt, unretried; only the recoverable, contract-defined
    /// transient code is retried, and only until `CONTENTION_RETRY_DEADLINE`, so a genuine,
    /// persistent failure still surfaces as a test failure rather than hanging forever.
    #[allow(clippy::too_many_arguments)]
    async fn submit(
        state: &AppState,
        cache: &WarmCache,
        coordinator: &DocumentCoordinator,
        registry: &SessionRegistry,
        snapshot_advancer: &SnapshotAdvancer,
        workspace_id: Uuid,
        document_id: Uuid,
        actor_id: Uuid,
        bytes: Vec<u8>,
        label: &str,
    ) -> AcceptOutcome {
        const CONTENTION_RETRY_DEADLINE: Duration = Duration::from_mins(3);
        const CONTENTION_RETRY_BACKOFF: Duration = Duration::from_millis(150);
        let started = std::time::Instant::now();
        loop {
            let checked_epoch = authz::read_epoch(&state.db, workspace_id).await.expect("epoch reads");
            let outcome = accept_update(
                &state.db,
                cache,
                coordinator,
                registry,
                snapshot_advancer,
                10,
                None,
                UpdateRequest {
                    document_id,
                    update_id: Uuid::new_v4(),
                    bytes: bytes.clone(),
                    idempotency_key: None,
                    origin_client_id: Some(label.to_string()),
                    message: None,
                    actor_id,
                    workspace_id,
                    checked_epoch,
                    expected_frontier: None,
                },
            )
            .await
            .expect("accept_update does not hit a hard database error");
            if is_load_dependent(&outcome) && started.elapsed() < CONTENTION_RETRY_DEADLINE {
                tokio::time::sleep(CONTENTION_RETRY_BACKOFF).await;
                continue;
            }
            return outcome;
        }
    }

    /// Call-direction proof for `check_snapshot`'s `tree_depth` branch: a chain reaching exactly
    /// `tree_depth_max` is accepted; one node deeper is rejected `limit_kind="tree_depth"`, and
    /// the rejection advances neither the document head nor `event_dispatch`.
    #[tokio::test]
    async fn ws_structural_limit_tree_depth_exact_boundary_accepted_plus_one_rejected_zero_side_effects() {
        let scratch = scratch_or_skip!("limit-tree-depth");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        let cache = WarmCache::new();
        let coordinator = DocumentCoordinator::new();
        let registry = SessionRegistry::new();
        let snapshot_advancer = SnapshotAdvancer::new();
        let limits = crate::flow::collab::limits::document_limits();

        let exact_bytes = build_update_from_current(&state, document_id, |engine| {
            let mut parent: Option<NodeId> = None;
            for i in 0..=limits.tree_depth_max {
                let id = NodeId::from(format!("depth-node-{i}"));
                engine
                    .apply_operation(&Operation::CreateNode {
                        id: id.clone(),
                        parent: parent.clone(),
                        index: 0,
                        kind: NodeKind::Block,
                    })
                    .expect("creating within the depth boundary must succeed locally");
                parent = Some(id);
            }
        })
        .await;

        let accepted_exact = match submit(
            &state,
            &cache,
            &coordinator,
            &registry,
            &snapshot_advancer,
            workspace_id,
            document_id,
            owner_id,
            exact_bytes,
            "depth-exact",
        )
        .await
        {
            AcceptOutcome::Accepted(accepted) => accepted,
            AcceptOutcome::Rejected(rejected) => {
                panic!("a chain reaching exactly tree_depth_max must be accepted, got {rejected:?}")
            }
        };
        let dispatch_after_exact = count_event_dispatch(&state, document_id).await;
        assert_eq!(
            dispatch_after_exact, 1,
            "the accepted update must produce exactly one event_dispatch row"
        );

        let plus_one_bytes = build_update_from_current(&state, document_id, |engine| {
            engine
                .apply_operation(&Operation::CreateNode {
                    id: NodeId::from("depth-node-one-too-many"),
                    parent: Some(NodeId::from(format!("depth-node-{}", limits.tree_depth_max))),
                    index: 0,
                    kind: NodeKind::Block,
                })
                .expect("the local candidate applies the op -- the server-side gate must reject it");
        })
        .await;

        let rejected = match submit(
            &state,
            &cache,
            &coordinator,
            &registry,
            &snapshot_advancer,
            workspace_id,
            document_id,
            owner_id,
            plus_one_bytes,
            "depth-plus-one",
        )
        .await
        {
            AcceptOutcome::Rejected(rejected) => rejected,
            AcceptOutcome::Accepted(_) => panic!("one node past tree_depth_max must be rejected"),
        };
        assert_eq!(rejected.code, RejectedCode::LimitExceeded);
        let details = rejected.details.expect("a limit_exceeded rejection must carry details");
        assert_eq!(details["limit_kind"], "tree_depth");
        assert_eq!(details["limit"], limits.tree_depth_max as u64);

        let head_after_rejection = super::read_observed_head(&state.db, document_id)
            .await
            .expect("head reads")
            .expect("document row exists");
        assert_eq!(
            head_after_rejection.head_seq, accepted_exact.head_seq,
            "a rejected update must never advance the document head"
        );
        assert_eq!(
            count_event_dispatch(&state, document_id).await,
            dispatch_after_exact,
            "a rejected update must never produce a new event_dispatch row"
        );

        scratch.drop_self().await;
    }

    /// Submits `total` `CreateNode(kind)` operations against the document's *current* state,
    /// split across as many `accept_update` calls as needed to stay well under `update_bytes_max`
    /// per update (~51-54 measured bytes/op for a bare `CreateNode`, `chunk` is chosen with a
    /// wide safety margin, not tuned to the exact ceiling) -- a single update carrying all
    /// `container_count_max`/`document_block_count_max` (10,000) creates would itself be
    /// rejected `limit_kind="update_bytes"` before ever reaching the check under test. Every
    /// intermediate chunk must itself be `Accepted` (none of them are the case under test);
    /// returns the final chunk's outcome, i.e. the one that reaches exactly `total`.
    #[allow(clippy::too_many_arguments)]
    async fn submit_create_nodes_in_chunks(
        state: &AppState,
        cache: &WarmCache,
        coordinator: &DocumentCoordinator,
        registry: &SessionRegistry,
        snapshot_advancer: &SnapshotAdvancer,
        workspace_id: Uuid,
        document_id: Uuid,
        actor_id: Uuid,
        id_prefix: &str,
        kind: NodeKind,
        total: usize,
        chunk: usize,
        label: &str,
    ) -> AcceptOutcome {
        let mut created = 0usize;
        loop {
            let this_chunk = chunk.min(total - created);
            let start = created;
            let bytes = build_update_from_current(state, document_id, |engine| {
                for i in 0..this_chunk {
                    engine
                        .apply_operation(&Operation::CreateNode {
                            id: NodeId::from(format!("{id_prefix}-{}", start + i)),
                            parent: None,
                            index: 0,
                            kind,
                        })
                        .expect("creating within the boundary must succeed locally");
                }
            })
            .await;
            created += this_chunk;
            let outcome = submit(
                state,
                cache,
                coordinator,
                registry,
                snapshot_advancer,
                workspace_id,
                document_id,
                actor_id,
                bytes,
                label,
            )
            .await;
            if created >= total {
                return outcome;
            }
            match outcome {
                AcceptOutcome::Accepted(_) => {}
                AcceptOutcome::Rejected(rejected) => {
                    panic!(
                        "an intermediate chunk (created {created} of {total}) was unexpectedly rejected: {rejected:?}"
                    )
                }
            }
        }
    }

    /// Submits `total_chars` of `InsertText` into one block, split across as many `accept_update`
    /// calls as needed to stay well under `update_bytes_max` per update (a single update carrying
    /// `text_block_chars_max` (100,000) chars is itself over the 65,536-byte `update_bytes_max`
    /// ceiling, rejected `limit_kind="update_bytes"` before ever reaching the check under test).
    /// `first_chunk_creates_block` controls whether the very first chunk also creates `block_id`
    /// (`false` when appending to a block that already exists). Every intermediate chunk must
    /// itself be `Accepted`; returns the final chunk's outcome, i.e. the one that reaches exactly
    /// `total_chars`.
    #[allow(clippy::too_many_arguments)]
    async fn submit_block_text_in_chunks(
        state: &AppState,
        cache: &WarmCache,
        coordinator: &DocumentCoordinator,
        registry: &SessionRegistry,
        snapshot_advancer: &SnapshotAdvancer,
        workspace_id: Uuid,
        document_id: Uuid,
        actor_id: Uuid,
        block_id: &str,
        first_chunk_creates_block: bool,
        total_chars: usize,
        chunk_chars: usize,
        label: &str,
    ) -> AcceptOutcome {
        let mut inserted = 0usize;
        let mut create_this_chunk = first_chunk_creates_block;
        loop {
            let this_chunk = chunk_chars.min(total_chars - inserted);
            let id = NodeId::from(block_id.to_string());
            let start = inserted;
            #[allow(clippy::cast_possible_truncation)]
            let index = start as u32;
            let bytes = build_update_from_current(state, document_id, |engine| {
                if create_this_chunk {
                    engine
                        .apply_operation(&Operation::CreateNode {
                            id: id.clone(),
                            parent: None,
                            index: 0,
                            kind: NodeKind::Block,
                        })
                        .expect("create must succeed locally");
                }
                engine
                    .apply_operation(&Operation::InsertText {
                        id,
                        index,
                        text: "a".repeat(this_chunk),
                    })
                    .expect("inserting within the boundary must succeed locally");
            })
            .await;
            create_this_chunk = false;
            inserted += this_chunk;
            let outcome = submit(
                state,
                cache,
                coordinator,
                registry,
                snapshot_advancer,
                workspace_id,
                document_id,
                actor_id,
                bytes,
                label,
            )
            .await;
            if inserted >= total_chars {
                return outcome;
            }
            match outcome {
                AcceptOutcome::Accepted(_) => {}
                AcceptOutcome::Rejected(rejected) => panic!(
                    "an intermediate text chunk (inserted {inserted} of {total_chars}) was unexpectedly rejected: {rejected:?}"
                ),
            }
        }
    }

    /// Chunk size for [`submit_create_nodes_in_chunks`]: 1,000 * ~54 bytes/op (measured, see this
    /// module's calibration in `crates/collab-core`'s benchmark notes) is comfortably under the
    /// frozen 65,536-byte `update_bytes_max`.
    const CREATE_NODE_CHUNK: usize = 1_000;
    /// Chunk size for [`submit_block_text_in_chunks`]: comfortably under `update_bytes_max` even
    /// including the `CreateNode` overhead on a chunk that also creates the block.
    const TEXT_CHUNK_CHARS: usize = 50_000;

    /// Call-direction proof for `check_snapshot`'s `container_count` branch (independent of
    /// `document_block_count` -- `NavigatorNode`, not `Block`).
    #[tokio::test]
    async fn ws_structural_limit_container_count_exact_boundary_accepted_plus_one_rejected_zero_side_effects() {
        let scratch = scratch_or_skip!("limit-container-count");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        let cache = WarmCache::new();
        let coordinator = DocumentCoordinator::new();
        let registry = SessionRegistry::new();
        let snapshot_advancer = SnapshotAdvancer::new();
        let limits = crate::flow::collab::limits::document_limits();

        let accepted_exact = match submit_create_nodes_in_chunks(
            &state,
            &cache,
            &coordinator,
            &registry,
            &snapshot_advancer,
            workspace_id,
            document_id,
            owner_id,
            "nav",
            NodeKind::NavigatorNode,
            limits.container_count_max,
            CREATE_NODE_CHUNK,
            "container-exact",
        )
        .await
        {
            AcceptOutcome::Accepted(accepted) => accepted,
            AcceptOutcome::Rejected(rejected) => {
                panic!("exactly container_count_max navigator nodes must be accepted, got {rejected:?}")
            }
        };
        let dispatch_after_exact = count_event_dispatch(&state, document_id).await;

        let plus_one_bytes = build_update_from_current(&state, document_id, |engine| {
            engine
                .apply_operation(&Operation::CreateNode {
                    id: NodeId::from("nav-one-too-many"),
                    parent: None,
                    index: 0,
                    kind: NodeKind::NavigatorNode,
                })
                .expect("the local candidate applies the op -- the server-side gate must reject it");
        })
        .await;

        let rejected = match submit(
            &state,
            &cache,
            &coordinator,
            &registry,
            &snapshot_advancer,
            workspace_id,
            document_id,
            owner_id,
            plus_one_bytes,
            "container-plus-one",
        )
        .await
        {
            AcceptOutcome::Rejected(rejected) => rejected,
            AcceptOutcome::Accepted(_) => panic!("one navigator node past container_count_max must be rejected"),
        };
        assert_eq!(rejected.code, RejectedCode::LimitExceeded);
        let details = rejected.details.expect("a limit_exceeded rejection must carry details");
        assert_eq!(details["limit_kind"], "container_count");
        assert_eq!(details["limit"], limits.container_count_max as u64);

        let head_after_rejection = super::read_observed_head(&state.db, document_id)
            .await
            .expect("head reads")
            .expect("document row exists");
        assert_eq!(head_after_rejection.head_seq, accepted_exact.head_seq);
        assert_eq!(count_event_dispatch(&state, document_id).await, dispatch_after_exact);

        scratch.drop_self().await;
    }

    /// Call-direction proof for `check_snapshot`'s `document_block_count` branch (independent of
    /// `container_count` -- `Block`, not `NavigatorNode`).
    #[tokio::test]
    async fn ws_structural_limit_document_block_count_exact_boundary_accepted_plus_one_rejected_zero_side_effects() {
        let scratch = scratch_or_skip!("limit-block-count");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        let cache = WarmCache::new();
        let coordinator = DocumentCoordinator::new();
        let registry = SessionRegistry::new();
        let snapshot_advancer = SnapshotAdvancer::new();
        let limits = crate::flow::collab::limits::document_limits();

        let accepted_exact = match submit_create_nodes_in_chunks(
            &state,
            &cache,
            &coordinator,
            &registry,
            &snapshot_advancer,
            workspace_id,
            document_id,
            owner_id,
            "blk",
            NodeKind::Block,
            limits.document_block_count_max,
            CREATE_NODE_CHUNK,
            "block-count-exact",
        )
        .await
        {
            AcceptOutcome::Accepted(accepted) => accepted,
            AcceptOutcome::Rejected(rejected) => {
                panic!("exactly document_block_count_max blocks must be accepted, got {rejected:?}")
            }
        };
        let dispatch_after_exact = count_event_dispatch(&state, document_id).await;

        let plus_one_bytes = build_update_from_current(&state, document_id, |engine| {
            engine
                .apply_operation(&Operation::CreateNode {
                    id: NodeId::from("blk-one-too-many"),
                    parent: None,
                    index: 0,
                    kind: NodeKind::Block,
                })
                .expect("the local candidate applies the op -- the server-side gate must reject it");
        })
        .await;

        let rejected = match submit(
            &state,
            &cache,
            &coordinator,
            &registry,
            &snapshot_advancer,
            workspace_id,
            document_id,
            owner_id,
            plus_one_bytes,
            "block-count-plus-one",
        )
        .await
        {
            AcceptOutcome::Rejected(rejected) => rejected,
            AcceptOutcome::Accepted(_) => panic!("one block past document_block_count_max must be rejected"),
        };
        assert_eq!(rejected.code, RejectedCode::LimitExceeded);
        let details = rejected.details.expect("a limit_exceeded rejection must carry details");
        assert_eq!(details["limit_kind"], "document_block_count");
        assert_eq!(details["limit"], limits.document_block_count_max as u64);

        let head_after_rejection = super::read_observed_head(&state.db, document_id)
            .await
            .expect("head reads")
            .expect("document row exists");
        assert_eq!(head_after_rejection.head_seq, accepted_exact.head_seq);
        assert_eq!(count_event_dispatch(&state, document_id).await, dispatch_after_exact);

        scratch.drop_self().await;
    }

    /// Call-direction proof for `check_snapshot`'s `text_block_chars` branch: one block's text at
    /// exactly `text_block_chars_max` chars is accepted; one char more is rejected.
    #[tokio::test]
    async fn ws_structural_limit_text_block_chars_exact_boundary_accepted_plus_one_rejected_zero_side_effects() {
        let scratch = scratch_or_skip!("limit-text-block-chars");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        let cache = WarmCache::new();
        let coordinator = DocumentCoordinator::new();
        let registry = SessionRegistry::new();
        let snapshot_advancer = SnapshotAdvancer::new();
        let limits = crate::flow::collab::limits::document_limits();

        let accepted_exact = match submit_block_text_in_chunks(
            &state,
            &cache,
            &coordinator,
            &registry,
            &snapshot_advancer,
            workspace_id,
            document_id,
            owner_id,
            "blk-text",
            true,
            limits.text_block_chars_max,
            TEXT_CHUNK_CHARS,
            "text-chars-exact",
        )
        .await
        {
            AcceptOutcome::Accepted(accepted) => accepted,
            AcceptOutcome::Rejected(rejected) => {
                panic!("a block at exactly text_block_chars_max must be accepted, got {rejected:?}")
            }
        };
        let dispatch_after_exact = count_event_dispatch(&state, document_id).await;

        let plus_one_bytes = build_update_from_current(&state, document_id, |engine| {
            engine
                .apply_operation(&Operation::InsertText {
                    id: NodeId::from("blk-text"),
                    index: 0,
                    text: "b".to_string(),
                })
                .expect("the local candidate applies the op -- the server-side gate must reject it");
        })
        .await;

        let rejected = match submit(
            &state,
            &cache,
            &coordinator,
            &registry,
            &snapshot_advancer,
            workspace_id,
            document_id,
            owner_id,
            plus_one_bytes,
            "text-chars-plus-one",
        )
        .await
        {
            AcceptOutcome::Rejected(rejected) => rejected,
            AcceptOutcome::Accepted(_) => panic!("one char past text_block_chars_max must be rejected"),
        };
        assert_eq!(rejected.code, RejectedCode::LimitExceeded);
        let details = rejected.details.expect("a limit_exceeded rejection must carry details");
        assert_eq!(details["limit_kind"], "text_block_chars");
        assert_eq!(details["limit"], limits.text_block_chars_max as u64);

        let head_after_rejection = super::read_observed_head(&state.db, document_id)
            .await
            .expect("head reads")
            .expect("document row exists");
        assert_eq!(head_after_rejection.head_seq, accepted_exact.head_seq);
        assert_eq!(count_event_dispatch(&state, document_id).await, dispatch_after_exact);

        scratch.drop_self().await;
    }

    /// Call-direction proof for `check_snapshot`'s `document_text_chars` branch: ten blocks each
    /// holding exactly `text_block_chars_max` chars sum to exactly `document_text_chars_max` (no
    /// individual block ever exceeds `text_block_chars_max`, so that check never fires first).
    /// One char more, in an eleventh block, is rejected `document_text_chars`.
    #[tokio::test]
    async fn ws_structural_limit_document_text_chars_exact_boundary_accepted_plus_one_rejected_zero_side_effects() {
        let scratch = scratch_or_skip!("limit-doc-text-chars");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        let cache = WarmCache::new();
        let coordinator = DocumentCoordinator::new();
        let registry = SessionRegistry::new();
        let snapshot_advancer = SnapshotAdvancer::new();
        let limits = crate::flow::collab::limits::document_limits();
        assert_eq!(
            limits.document_text_chars_max,
            limits.text_block_chars_max * 10,
            "this fixture assumes document_text_chars_max is exactly 10x text_block_chars_max"
        );

        let mut dispatch_after_exact = 0i64;
        let mut accepted_exact = None;
        for block_index in 0..10 {
            let block_id = format!("blk-doc-{block_index}");
            let outcome = submit_block_text_in_chunks(
                &state,
                &cache,
                &coordinator,
                &registry,
                &snapshot_advancer,
                workspace_id,
                document_id,
                owner_id,
                &block_id,
                true,
                limits.text_block_chars_max,
                TEXT_CHUNK_CHARS,
                "doc-chars-exact",
            )
            .await;
            match outcome {
                AcceptOutcome::Accepted(accepted) => {
                    dispatch_after_exact = count_event_dispatch(&state, document_id).await;
                    accepted_exact = Some(accepted);
                }
                AcceptOutcome::Rejected(rejected) => panic!(
                    "block {block_index}/10 at exactly text_block_chars_max must be accepted                      (document total is exactly document_text_chars_max only after the 10th), got {rejected:?}"
                ),
            }
        }
        let accepted_exact = accepted_exact.expect("the loop above always assigns Some on success");

        let plus_one_bytes = build_update_from_current(&state, document_id, |engine| {
            engine
                .apply_operation(&Operation::CreateNode {
                    id: NodeId::from("blk-doc-one-too-many"),
                    parent: None,
                    index: 0,
                    kind: NodeKind::Block,
                })
                .expect("create must succeed locally");
            engine
                .apply_operation(&Operation::InsertText {
                    id: NodeId::from("blk-doc-one-too-many"),
                    index: 0,
                    text: "z".to_string(),
                })
                .expect("the local candidate applies the op -- the server-side gate must reject it");
        })
        .await;

        let rejected = match submit(
            &state,
            &cache,
            &coordinator,
            &registry,
            &snapshot_advancer,
            workspace_id,
            document_id,
            owner_id,
            plus_one_bytes,
            "doc-chars-plus-one",
        )
        .await
        {
            AcceptOutcome::Rejected(rejected) => rejected,
            AcceptOutcome::Accepted(_) => panic!("one char past document_text_chars_max must be rejected"),
        };
        assert_eq!(rejected.code, RejectedCode::LimitExceeded);
        let details = rejected.details.expect("a limit_exceeded rejection must carry details");
        assert_eq!(details["limit_kind"], "document_text_chars");
        assert_eq!(details["limit"], limits.document_text_chars_max as u64);

        let head_after_rejection = super::read_observed_head(&state.db, document_id)
            .await
            .expect("head reads")
            .expect("document row exists");
        assert_eq!(head_after_rejection.head_seq, accepted_exact.head_seq);
        assert_eq!(count_event_dispatch(&state, document_id).await, dispatch_after_exact);

        scratch.drop_self().await;
    }

    /// Builds a real, decodable update whose exported byte length is exactly `target_len` --
    /// used below to prove `update_bytes_max`'s exact-boundary-*accepted* case with genuine CRDT
    /// content (unlike the plus-one case, which uses an arbitrary buffer precisely because
    /// `InputLimits::validate_update` rejects on length before any decode, so its content does
    /// not need to be valid at all). A single `CreateNode` + `InsertText` of `n` plain ASCII
    /// characters grows the exported update by exactly one byte per character (Loro's
    /// column-oriented encoding stores the text content as a contiguous byte span, no
    /// per-character framing), so starting from a conservative overhead estimate and correcting
    /// by the exact remaining delta converges in at most a couple of iterations.
    async fn build_update_of_exact_len(
        state: &AppState,
        document_id: Uuid,
        block_id: &str,
        target_len: usize,
    ) -> Vec<u8> {
        let mut text_len = target_len.saturating_sub(200);
        for _ in 0..8 {
            let bytes = build_update_from_current(state, document_id, |engine| {
                engine
                    .apply_operation(&Operation::CreateNode {
                        id: NodeId::from(block_id.to_string()),
                        parent: None,
                        index: 0,
                        kind: NodeKind::Block,
                    })
                    .expect("create must succeed locally");
                engine
                    .apply_operation(&Operation::InsertText {
                        id: NodeId::from(block_id.to_string()),
                        index: 0,
                        text: "a".repeat(text_len),
                    })
                    .expect("insert must succeed locally");
            })
            .await;
            match bytes.len().cmp(&target_len) {
                std::cmp::Ordering::Equal => return bytes,
                std::cmp::Ordering::Less => text_len += target_len - bytes.len(),
                std::cmp::Ordering::Greater => text_len -= bytes.len() - target_len,
            }
        }
        panic!("could not converge on an update of exactly {target_len} bytes (last attempt used {text_len} chars)");
    }

    /// `update_bytes_max` (65,536 bytes) is checked by `InputLimits::validate_update` -- the very
    /// first thing `accept_update` does, before `find_prior_update`, the coordinator permit, or
    /// any other database access (see this module's `accept_update` doc comment). Proven here
    /// with a real, decodable CRDT update at exactly the ceiling (fully applied, committed, and
    /// counted in `event_dispatch`), and an arbitrary 65,537-byte buffer one byte over it:
    /// rejected on length alone before ever being decoded, and provably a total no-op against the
    /// document (head unchanged, no new `event_dispatch` row).
    #[tokio::test]
    async fn ws_structural_limit_update_bytes_exact_boundary_accepted_plus_one_rejected_zero_side_effects() {
        let scratch = scratch_or_skip!("limit-update-bytes");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        let cache = WarmCache::new();
        let coordinator = DocumentCoordinator::new();
        let registry = SessionRegistry::new();
        let snapshot_advancer = SnapshotAdvancer::new();

        const UPDATE_BYTES_MAX: usize = 65_536;

        let exact_bytes =
            build_update_of_exact_len(&state, document_id, "update-bytes-boundary-block", UPDATE_BYTES_MAX).await;
        assert_eq!(
            exact_bytes.len(),
            UPDATE_BYTES_MAX,
            "the constructed fixture must hit the ceiling exactly"
        );

        let dispatch_before = count_event_dispatch(&state, document_id).await;
        let accepted_exact = match submit(
            &state,
            &cache,
            &coordinator,
            &registry,
            &snapshot_advancer,
            workspace_id,
            document_id,
            owner_id,
            exact_bytes,
            "update-bytes-exact",
        )
        .await
        {
            AcceptOutcome::Accepted(accepted) => accepted,
            AcceptOutcome::Rejected(rejected) => {
                panic!(
                    "an update of exactly update_bytes_max ({UPDATE_BYTES_MAX} bytes) must be accepted: {rejected:?}"
                )
            }
        };
        let dispatch_after_exact = count_event_dispatch(&state, document_id).await;
        assert_eq!(
            dispatch_after_exact,
            dispatch_before + 1,
            "the accepted exact-boundary update must produce exactly one new event_dispatch row"
        );

        // update_bytes_max + 1 = 65537 bytes: arbitrary content is fine here (unlike the exact
        // boundary above) because `InputLimits::validate_update` rejects on length alone, before
        // this buffer is ever decoded.
        let plus_one_bytes = vec![0u8; UPDATE_BYTES_MAX + 1];
        let rejected = match submit(
            &state,
            &cache,
            &coordinator,
            &registry,
            &snapshot_advancer,
            workspace_id,
            document_id,
            owner_id,
            plus_one_bytes,
            "update-bytes-plus-one",
        )
        .await
        {
            AcceptOutcome::Rejected(rejected) => rejected,
            AcceptOutcome::Accepted(_) => {
                panic!(
                    "a {}-byte update (one past update_bytes_max) must be rejected",
                    UPDATE_BYTES_MAX + 1
                )
            }
        };
        assert_eq!(rejected.code, RejectedCode::LimitExceeded);
        let details = rejected.details.expect("a limit_exceeded rejection must carry details");
        assert_eq!(details["limit_kind"], "update_bytes");
        assert_eq!(details["limit"], UPDATE_BYTES_MAX as u64);
        assert_eq!(details["observed"], (UPDATE_BYTES_MAX + 1) as u64);

        let head_after_rejection = super::read_observed_head(&state.db, document_id)
            .await
            .expect("head reads")
            .expect("document row exists");
        assert_eq!(
            head_after_rejection.head_seq, accepted_exact.head_seq,
            "a rejected oversized update must never advance the document head"
        );
        assert_eq!(
            count_event_dispatch(&state, document_id).await,
            dispatch_after_exact,
            "a rejected oversized update must never produce a new event_dispatch row"
        );

        scratch.drop_self().await;
    }
}
