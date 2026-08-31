//! Sylvode Flow v0.4 collab **warm-cache / bypass evidence harness** — the measurement facility for
//! the `ADR-0010-collab-server-architecture.md` §"量化接受与推翻门槛" clauses that
//! `evidence/v0.4/collab-architecture-result.json` had **no evidence for at all** as of 2026-08-30
//! (full-text search of that artifact: `eviction` 0 hits, `idle` 0 hits, `bypass` 0 hits,
//! `coordinator` 0 hits):
//!
//! - **item 2, remainder** — "cache hit/miss/eviction、API restart 与三次 rebase exhaustion 都不丢
//!   accepted update". The already-published load harness proved item 2's *budget* half (10-client
//!   round-trip p95); it never varied the cache condition and never drove a rejection.
//! - **item 3, entire** — "warm cache entry/decoded bytes/idle TTL exact boundary 可回收，eviction
//!   后从一致性 bootstrap 重建的 semantic hash 与 head 相同，observer/timer 增量为零".
//! - **item 5, entire** — "任意实例绕过 cache 仍通过 DB 锁得到唯一 seq；cache/协调器删除后只退化
//!   性能，不改变正确性".
//!
//! # Why this is a second harness rather than more assertions in the first one
//!
//! `flow_collab_load_harness.rs` measures *latency distributions* and therefore has hard
//! environmental prerequisites (a dedicated, otherwise-idle `PostgreSQL` instance; a release build;
//! server-side statement logging). Everything measured **here** is a correctness/boundary claim
//! with no timing budget attached, so it is legible and reproducible on any real `PostgreSQL`. Both
//! artifacts are emitted in release for the official run, but only the first one is invalidated by
//! a busy database.
//!
//! It also fixes a judgement defect recorded in the ADR: the existing gate
//! `bounded_warm_cache_lock_hold_and_round_trip_budgets` has `bounded_warm_cache` in its **name**
//! while asserting only lock hold / lock wait / in-lock gap / round-trip / sample count / locked
//! statement inventory. Not one warm-cache boundary was verified by it. Every field this harness
//! emits is named after the thing it actually observed.
//!
//! # Measurement caliber
//!
//! Nothing here is self-reported by the implementation. The three authorities used are:
//!
//! 1. **The `WarmCache` public API itself** (`len`, `total_bytes`, `contains`, `fork_matching`,
//!    `put` → `PutOutcome`) for the boundary claims — the cache is asked what it is holding, at
//!    the exact ceiling and at ceiling+1, rather than trusting an internal counter.
//! 2. **`PostgreSQL` canonical state** (`collab_documents`, `collab_updates`, `business_events`,
//!    `event_dispatch`) for every "no accepted update was lost" claim: the rows are counted after
//!    the fact, not inferred from what the API returned.
//! 3. **`flow::collab::bootstrap::load`**, the protocol's own `REPEATABLE READ READ ONLY`
//!    consistency loader, for the post-eviction rebuild — the same code path a cold instance takes,
//!    not a test-local reconstruction.
//!
//! # Anti-proof knobs
//!
//! `OPENPR_FLOW_CACHE_EVIDENCE_MUTANT` deliberately breaks one measurement at a time so a reviewer
//! can confirm each assertion is load-bearing rather than decorative. See [`Mutant`].
//!
//! # Honest failure
//!
//! Every check appends to `violations` instead of panicking, the whole JSON is printed, and a
//! single assertion at the end fails the run. A red run shows its numbers.

#![allow(
    // Test target: `clippy.toml` already allows unwrap/expect/panic in tests; the print and
    // indexing lints are not covered by that config and are needed to report a measured result.
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    // `Vec::len() as i64` when comparing a measured row count against a `head_seq`.
    clippy::cast_possible_wrap,
    clippy::too_many_lines,
    clippy::similar_names
)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use collab_core::{CollabEngine, LoroCollabEngine};
use platform::{
    app::AppState,
    config::{AppConfig, Secret},
};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use api::flow::collab::bootstrap;
use api::flow::collab::cache::{PutOutcome, WarmCache};
use api::flow::collab::coordinator::DocumentCoordinator;
use api::flow::collab::limits::{
    DOCUMENT_LOCK_WAIT_MS_MAX, MAX_REBASE_ATTEMPTS, WARM_CACHE_DECODED_BYTES_MAX, WARM_CACHE_DOCUMENTS_MAX,
    WARM_CACHE_ENTRY_DECODED_BYTES_MAX, WARM_CACHE_IDLE_TTL_SECONDS,
};
use api::flow::collab::registry::SessionRegistry;
use api::flow::collab::snapshot::SnapshotAdvancer;
use api::flow::collab::write::{AcceptOutcome, UpdateRequest, accept_update};
use api::flow::collab::{authz, frame::RejectedCode};
use api::flow::projection;

// ---------------------------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------------------------

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";
const EVIDENCE_OUT_ENV: &str = "OPENPR_FLOW_CACHE_EVIDENCE_OUT";
const PG_CONTAINER_ENV: &str = "OPENPR_FLOW_PG_LOG_CONTAINER";
const MUTANT_ENV: &str = "OPENPR_FLOW_CACHE_EVIDENCE_MUTANT";
const DEFAULT_PG_CONTAINER: &str = "flow-load-pg";

/// The cache module this harness makes claims about; re-read at runtime for the static half of the
/// "observer/timer 增量为零" claim, so the claim cannot silently go stale if the module grows a
/// background task later.
const CACHE_SOURCE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/flow/collab/cache.rs");
const COORDINATOR_SOURCE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/flow/collab/coordinator.rs");

/// How close to the frozen 120 s idle TTL the "still alive" probe is taken. One second of margin
/// against a `std::time::Instant` comparison whose two sides are read microseconds apart.
const IDLE_TTL_ALIVE_PROBE_SECONDS: u64 = 118;
/// How far past the TTL the reclamation probe is taken.
const IDLE_TTL_RECLAIM_PROBE_SECONDS: u64 = 122;

/// Concurrent writers used by the item-5 uniqueness runs and the row-lock negative control.
const BYPASS_WRITERS: usize = 8;

/// Deliberate breakages, one per assertion family, so the evidence can be shown to be
/// falsifiable. Every one of these must turn the run red.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mutant {
    None,
    /// Rebuild the document after eviction from an inconsistent path (drop the last tail update),
    /// so the semantic hash must stop matching the warm one.
    RebuildSkipLastTail,
    /// Allocate `seq` the way an implementation that trusted its cache instead of the row lock
    /// would: read `head_seq` with no `FOR UPDATE`. Uniqueness must fail.
    BypassRowLock,
    /// Take the idle probe *before* the TTL rather than after it, so a cache that never reclaimed
    /// anything would still look reclaimed. The reclamation assertion must fail.
    IdleProbeTooEarly,
}

impl Mutant {
    fn from_env() -> Self {
        match std::env::var(MUTANT_ENV).unwrap_or_default().as_str() {
            "rebuild_skip_last_tail" => Self::RebuildSkipLastTail,
            "bypass_row_lock" => Self::BypassRowLock,
            "idle_probe_too_early" => Self::IdleProbeTooEarly,
            "" | "none" => Self::None,
            other => panic!("unknown {MUTANT_ENV}: {other}"),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::RebuildSkipLastTail => "rebuild_skip_last_tail",
            Self::BypassRowLock => "bypass_row_lock",
            Self::IdleProbeTooEarly => "idle_probe_too_early",
        }
    }
}

/// Fails the run *before* any measurement with an actionable message when the isolated-apply
/// worker binary is missing.
///
/// Without this, a missing worker surfaces as `accept_update` returning `ApiError::Internal`, and
/// every write-path section reports a failure whose stated cause is wrong -- the run looks like an
/// implementation defect when it is a build gap. Observed twice while building this harness: the
/// uplifted `target/<profile>/collab-isolated-apply-worker` does not survive every cargo
/// invocation against the same target directory (a `cargo clippy --all-features` in between was
/// enough to remove it), so the check has to run per-test-process, not once per session.
fn require_isolated_apply_worker() {
    let exe = std::env::current_exe().expect("test executable path resolves");
    // The test binary lives in `<target>/<profile>/deps/`; the worker is uplifted one level up,
    // which is exactly where `collab_core::isolation::host` looks for it.
    let candidate = exe
        .parent()
        .and_then(std::path::Path::parent)
        .map(|dir| dir.join("collab-isolated-apply-worker"))
        .expect("test executable has a grandparent directory");
    assert!(
        candidate.is_file(),
        "collab-isolated-apply-worker is not at {}. Every accept_update below would fail as \
         ApiError::Internal and this run would report the wrong cause. Build it into the same \
         target directory first:\n    \
         CARGO_TARGET_DIR=<same> cargo build --release -p collab-core --bin collab-isolated-apply-worker\n\
         (and the debug equivalent for a debug run).",
        candidate.display()
    );
}

/// The commit this evidence was produced from, plus whether the tree was dirty. `ADR-0010`'s
/// result schema requires `source_head`; an artifact that cannot say which source it measured is
/// not evidence about anything in particular.
fn source_head() -> (String, bool) {
    let head = std::process::Command::new("git")
        .args(["-C", env!("CARGO_MANIFEST_DIR"), "rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map_or_else(
            || "unknown".to_string(),
            |out| String::from_utf8_lossy(&out.stdout).trim().to_string(),
        );
    let dirty = std::process::Command::new("git")
        .args(["-C", env!("CARGO_MANIFEST_DIR"), "status", "--porcelain"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .is_some_and(|out| !out.stdout.is_empty());
    (head, dirty)
}

const fn build_profile() -> &'static str {
    if cfg!(debug_assertions) { "debug" } else { "release" }
}

// ---------------------------------------------------------------------------------------------
// Scratch database (same shape as flow_collab_load_harness.rs)
// ---------------------------------------------------------------------------------------------

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

    let name = format!("openpr_flow_cache_{label}");
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

// ---------------------------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------------------------

fn state_for(db: DatabaseConnection) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "collab-cache-evidence".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new("collab-cache-evidence-secret"),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            default_author_id: None,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db,
    }
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
}

async fn seed_workspace(db: &DatabaseConnection) -> (Uuid, Uuid) {
    let workspace_id = Uuid::new_v4();
    let owner_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active) \
         VALUES ($1, $2, '!', 'cache evidence', 'user', true)",
        vec![owner_id.into(), format!("{owner_id}@collab-cache.test").into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'collab cache evidence', $3)",
        vec![
            workspace_id.into(),
            format!("ws-{workspace_id}").into(),
            owner_id.into(),
        ],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
        vec![workspace_id.into(), owner_id.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled) VALUES ($1, true)",
        vec![workspace_id.into()],
    )
    .await;
    (workspace_id, owner_id)
}

async fn create_page(state: &AppState, workspace_id: Uuid, actor_id: Uuid, title: &str) -> (Uuid, Uuid) {
    use api::flow::command::{CreateObjectInput, create_object};
    let accepted = create_object(
        state,
        CreateObjectInput {
            workspace_id,
            actor_id,
            object_type: "page".to_string(),
            project_id: None,
            parent_object_id: None,
            title: title.to_string(),
            idempotency_key: Uuid::new_v4().to_string(),
            message: None,
        },
    )
    .await
    .expect("object creation succeeds");
    (accepted.object.id, accepted.object.document_id)
}

#[derive(Debug, Clone, FromQueryResult)]
struct DocRow {
    head_seq: i64,
    head_frontier: Vec<u8>,
    format_version: String,
    snapshot: Vec<u8>,
}

async fn read_document(db: &DatabaseConnection, document_id: Uuid) -> DocRow {
    DocRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT head_seq, head_frontier, format_version, snapshot FROM collab_documents WHERE id = $1",
        vec![document_id.into()],
    ))
    .one(db)
    .await
    .expect("document query runs")
    .expect("document row exists")
}

#[derive(FromQueryResult)]
struct CountRow {
    n: i64,
}

async fn count_scalar(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
    CountRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .one(db)
        .await
        .expect("count query runs")
        .expect("count query returns a row")
        .n
}

#[derive(FromQueryResult)]
struct SeqRow {
    seq: i64,
}

async fn all_seqs(db: &DatabaseConnection, document_id: Uuid) -> Vec<i64> {
    SeqRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT seq FROM collab_updates WHERE document_id = $1 ORDER BY seq",
        vec![document_id.into()],
    ))
    .all(db)
    .await
    .expect("seq query runs")
    .into_iter()
    .map(|row| row.seq)
    .collect()
}

fn contiguous_from_one(seqs: &[i64]) -> bool {
    seqs.iter()
        .enumerate()
        .all(|(index, seq)| *seq == i64::try_from(index).unwrap_or(i64::MAX) + 1)
}

/// One simulated API instance: its own warm cache, its own document coordinator, its own session
/// registry and snapshot advancer. `ADR-0010` makes all four explicitly instance-local, so N of
/// these against one database is exactly the "任意实例" shape item 5 asks about.
struct Instance {
    cache: WarmCache,
    coordinator: DocumentCoordinator,
    registry: SessionRegistry,
    advancer: SnapshotAdvancer,
}

impl Instance {
    fn new() -> Self {
        Self {
            cache: WarmCache::new(),
            coordinator: DocumentCoordinator::new(),
            registry: SessionRegistry::new(),
            advancer: SnapshotAdvancer::new(),
        }
    }
}

/// Builds a genuine Loro delta against `base_snapshot`, the way a real client does: load the
/// server's snapshot, edit, export the delta from the pre-edit frontier.
fn build_update(base_snapshot: &[u8], title: &str) -> Vec<u8> {
    let mut engine = LoroCollabEngine::load(base_snapshot).expect("base snapshot loads");
    let base_frontier = engine.frontier();
    engine.set_title(title).expect("set_title succeeds");
    engine
        .export_from(&base_frontier)
        .expect("export produces a real delta")
}

#[allow(clippy::too_many_arguments)]
async fn write_once(
    db: &DatabaseConnection,
    instance: &Instance,
    document_id: Uuid,
    workspace_id: Uuid,
    actor_id: Uuid,
    checked_epoch: i64,
    bytes: Vec<u8>,
    update_id: Uuid,
) -> AcceptOutcome {
    accept_update(
        db,
        &instance.cache,
        &instance.coordinator,
        &instance.registry,
        &instance.advancer,
        10,
        None,
        UpdateRequest {
            document_id,
            update_id,
            bytes,
            idempotency_key: None,
            event_idempotency_key: None,
            origin_client_id: Some("cache-evidence".to_string()),
            message: None,
            actor_id,
            workspace_id,
            checked_epoch,
            expected_frontier: None,
        },
    )
    .await
    .expect("accept_update does not hit a hard database error")
}

fn semantic_hash(engine: &LoroCollabEngine) -> String {
    let semantic = engine.semantic_snapshot().expect("semantic snapshot builds");
    let state = projection::state_json(&semantic).expect("projection renders");
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_string(&state).expect("state serializes").as_bytes());
    hasher.update(engine.title().expect("title reads").as_bytes());
    hasher.update(projection::plain_text(&semantic).as_bytes());
    hex::encode(hasher.finalize())
}

/// Rebuilds a document exactly the way a cold instance does after a cache miss:
/// `bootstrap::load`'s `REPEATABLE READ READ ONLY` snapshot + tail, replayed in seq order.
async fn rebuild_from_bootstrap(
    db: &DatabaseConnection,
    document_id: Uuid,
    skip_last_tail: bool,
) -> (LoroCollabEngine, i64, usize) {
    let boot = bootstrap::load(db, document_id).await.expect("bootstrap loader runs");
    let mut engine = LoroCollabEngine::load(&boot.snapshot).expect("bootstrap snapshot loads");
    let total = boot.tail_updates.len();
    for (index, tail) in boot.tail_updates.iter().enumerate() {
        if skip_last_tail && index + 1 == total {
            break;
        }
        engine.import_update(&tail.bytes).expect("tail update replays");
    }
    (engine, boot.head_seq, total)
}

/// The document's **head** state as a loadable snapshot.
///
/// `collab_documents.snapshot` is the *checkpoint* at `snapshot_seq`, not the head, so an update
/// built straight off that column is a concurrent sibling of every other update built the same way
/// -- fine for the contention fixtures below, wrong for a fixture that needs a strictly sequential
/// document history (a later write must supersede an earlier one, or dropping a tail update during
/// a rebuild can be semantically invisible and the parity assertion proves nothing).
async fn head_state_bytes(db: &DatabaseConnection, document_id: Uuid) -> Vec<u8> {
    let (engine, _, _) = rebuild_from_bootstrap(db, document_id, false).await;
    engine.export_snapshot().expect("head state exports a snapshot")
}

fn engine_of(cache: &WarmCache, document_id: Uuid, head_seq: i64, format_version: &str) -> Option<LoroCollabEngine> {
    cache
        .fork_matching(document_id, head_seq, format_version)
        .expect("fork_matching does not error")
}

fn filler_engine() -> LoroCollabEngine {
    LoroCollabEngine::new_empty(1)
}

/// Forces `document_id` out of `cache` by real LRU pressure on the frozen entry-count ceiling —
/// never by a privileged "remove" the production code does not have.
fn evict_by_entry_pressure(cache: &WarmCache, document_id: Uuid) -> usize {
    let mut inserted = 0usize;
    for _ in 0..=WARM_CACHE_DOCUMENTS_MAX {
        cache.put(Uuid::new_v4(), filler_engine(), "loro-1".to_string(), 0, vec![], 1);
        inserted += 1;
        std::thread::sleep(Duration::from_micros(50));
        if !cache.contains(document_id) {
            break;
        }
    }
    inserted
}

fn proc_count(dir: &str) -> Option<usize> {
    std::fs::read_dir(dir).ok().map(std::iter::Iterator::count)
}

// ---------------------------------------------------------------------------------------------
// Section 1 — ADR item 3: exact boundary reclamation
// ---------------------------------------------------------------------------------------------

fn section_cache_boundaries(mutant: Mutant, violations: &mut Vec<String>) -> Value {
    // --- entry-count ceiling, exactly at and exactly one past -------------------------------
    let cache = WarmCache::new();
    let mut ids = Vec::with_capacity(WARM_CACHE_DOCUMENTS_MAX);
    for index in 0..WARM_CACHE_DOCUMENTS_MAX {
        let id = Uuid::new_v4();
        ids.push(id);
        let outcome = cache.put(
            id,
            filler_engine(),
            "loro-1".to_string(),
            i64::try_from(index).unwrap_or(0),
            vec![],
            1,
        );
        assert_eq!(outcome, PutOutcome::Cached);
        // Strictly increasing `last_used` so LRU order is deterministic rather than tie-broken by
        // `Instant` resolution.
        std::thread::sleep(Duration::from_micros(50));
    }
    let entry_exact_count = cache.len();
    if entry_exact_count != WARM_CACHE_DOCUMENTS_MAX {
        violations.push(format!(
            "entry_exact: cache holds {entry_exact_count} at the {WARM_CACHE_DOCUMENTS_MAX}-entry ceiling"
        ));
    }
    let all_resident_at_ceiling = ids.iter().all(|id| cache.contains(*id));
    if !all_resident_at_ceiling {
        violations.push("entry_exact: an entry was evicted before the ceiling was exceeded".to_string());
    }

    let past_ceiling = Uuid::new_v4();
    cache.put(past_ceiling, filler_engine(), "loro-1".to_string(), 999, vec![], 1);
    let entry_plus_one_count = cache.len();
    let lru_evicted = !cache.contains(ids[0]);
    let newest_resident = cache.contains(past_ceiling);
    let survivors = ids[1..].iter().filter(|id| cache.contains(**id)).count();
    if entry_plus_one_count > WARM_CACHE_DOCUMENTS_MAX {
        violations.push(format!(
            "entry_plus_one: cache grew to {entry_plus_one_count}, past the {WARM_CACHE_DOCUMENTS_MAX} ceiling"
        ));
    }
    if !lru_evicted {
        violations.push("entry_plus_one: the least-recently-used entry was not reclaimed".to_string());
    }
    if !newest_resident {
        violations.push("entry_plus_one: the new entry did not become resident".to_string());
    }
    if survivors != WARM_CACHE_DOCUMENTS_MAX - 1 {
        violations.push(format!(
            "entry_plus_one: {survivors} of {} non-LRU entries survived; exactly one entry may be reclaimed",
            WARM_CACHE_DOCUMENTS_MAX - 1
        ));
    }

    // --- decoded-bytes ceiling, exactly at and exactly one past ------------------------------
    let byte_cache = WarmCache::new();
    let per_entry = WARM_CACHE_ENTRY_DECODED_BYTES_MAX;
    let entries_at_ceiling = usize::try_from(WARM_CACHE_DECODED_BYTES_MAX / per_entry).unwrap_or(0);
    let mut byte_ids = Vec::with_capacity(entries_at_ceiling);
    for index in 0..entries_at_ceiling {
        let id = Uuid::new_v4();
        byte_ids.push(id);
        let outcome = byte_cache.put(
            id,
            filler_engine(),
            "loro-1".to_string(),
            i64::try_from(index).unwrap_or(0),
            vec![],
            per_entry,
        );
        assert_eq!(outcome, PutOutcome::Cached);
        std::thread::sleep(Duration::from_micros(50));
    }
    let bytes_exact_total = byte_cache.total_bytes();
    if bytes_exact_total != WARM_CACHE_DECODED_BYTES_MAX {
        violations.push(format!(
            "bytes_exact: total_bytes {bytes_exact_total} != the {WARM_CACHE_DECODED_BYTES_MAX} ceiling"
        ));
    }
    let bytes_exact_count = byte_cache.len();

    let bytes_past = Uuid::new_v4();
    byte_cache.put(bytes_past, filler_engine(), "loro-1".to_string(), 42, vec![], per_entry);
    let bytes_plus_one_total = byte_cache.total_bytes();
    let bytes_lru_evicted = !byte_cache.contains(byte_ids[0]);
    if bytes_plus_one_total > WARM_CACHE_DECODED_BYTES_MAX {
        violations.push(format!(
            "bytes_plus_one: total_bytes {bytes_plus_one_total} exceeded the {WARM_CACHE_DECODED_BYTES_MAX} ceiling"
        ));
    }
    if !bytes_lru_evicted {
        violations.push("bytes_plus_one: the least-recently-used entry was not reclaimed for space".to_string());
    }
    if !byte_cache.contains(bytes_past) {
        violations.push("bytes_plus_one: the new entry did not become resident".to_string());
    }

    // --- single-entry ceiling, exactly at and exactly one past -------------------------------
    let single = WarmCache::new();
    let at_id = Uuid::new_v4();
    let at_outcome = single.put(
        at_id,
        filler_engine(),
        "loro-1".to_string(),
        0,
        vec![],
        WARM_CACHE_ENTRY_DECODED_BYTES_MAX,
    );
    let over_id = Uuid::new_v4();
    let over_outcome = single.put(
        over_id,
        filler_engine(),
        "loro-1".to_string(),
        0,
        vec![],
        WARM_CACHE_ENTRY_DECODED_BYTES_MAX + 1,
    );
    if at_outcome != PutOutcome::Cached {
        violations.push("single_entry_exact: an entry exactly at the ceiling was refused".to_string());
    }
    if over_outcome != PutOutcome::TooLargeToCache || single.contains(over_id) {
        violations.push("single_entry_plus_one: an over-ceiling entry became resident".to_string());
    }

    // --- idle TTL, probed on both sides of the frozen boundary -------------------------------
    let idle = WarmCache::new();
    let idle_id = Uuid::new_v4();
    idle.put(idle_id, filler_engine(), "loro-1".to_string(), 7, vec![1, 2], 4_096);
    let idle_started = Instant::now();

    let alive_probe_at = if mutant == Mutant::IdleProbeTooEarly {
        1
    } else {
        IDLE_TTL_ALIVE_PROBE_SECONDS
    };
    std::thread::sleep(Duration::from_secs(alive_probe_at));
    let alive_elapsed = idle_started.elapsed().as_secs_f64();
    // `fork_matching` is one of the two entry points that runs the idle sweep, so this probe both
    // asks "is it still here" and proves the sweep ran and declined to reclaim it.
    let alive = engine_of(&idle, idle_id, 7, "loro-1").is_some();
    let alive_len = idle.len();
    if !alive || alive_len != 1 {
        violations.push(format!(
            "idle_ttl: entry was already gone at {alive_elapsed:.1}s, inside the \
             {WARM_CACHE_IDLE_TTL_SECONDS}s TTL"
        ));
    }

    let reclaim_probe_at = if mutant == Mutant::IdleProbeTooEarly {
        2
    } else {
        IDLE_TTL_RECLAIM_PROBE_SECONDS
    };
    // `fork_matching` above refreshed `last_used`, so the reclamation clock restarts here.
    let reclaim_started = Instant::now();
    std::thread::sleep(Duration::from_secs(reclaim_probe_at));
    let reclaim_elapsed = reclaim_started.elapsed().as_secs_f64();
    // Any cache entry point runs the sweep; `put` of an unrelated document is the least
    // privileged one available and is what a real instance does constantly.
    idle.put(Uuid::new_v4(), filler_engine(), "loro-1".to_string(), 0, vec![], 1);
    let idle_reclaimed = !idle.contains(idle_id);
    let idle_bytes_after = idle.total_bytes();
    if !idle_reclaimed {
        violations.push(format!(
            "idle_ttl: entry idle for {reclaim_elapsed:.1}s was not reclaimed at the \
             {WARM_CACHE_IDLE_TTL_SECONDS}s TTL"
        ));
    }
    if idle_bytes_after != 1 {
        violations.push(format!(
            "idle_ttl: total_bytes is {idle_bytes_after} after reclamation; the reclaimed entry's \
             4096 bytes were not released"
        ));
    }

    json!({
        "entry_count_ceiling": WARM_CACHE_DOCUMENTS_MAX,
        "entry_exact_resident_count": entry_exact_count,
        "entry_exact_all_resident": all_resident_at_ceiling,
        "entry_plus_one_resident_count": entry_plus_one_count,
        "entry_plus_one_lru_reclaimed": lru_evicted,
        "entry_plus_one_newest_resident": newest_resident,
        "entry_plus_one_non_lru_survivors": survivors,
        "decoded_bytes_ceiling": WARM_CACHE_DECODED_BYTES_MAX,
        "bytes_exact_entries": bytes_exact_count,
        "bytes_exact_total_bytes": bytes_exact_total,
        "bytes_exact_equals_ceiling": bytes_exact_total == WARM_CACHE_DECODED_BYTES_MAX,
        "bytes_plus_one_total_bytes": bytes_plus_one_total,
        "bytes_plus_one_within_ceiling": bytes_plus_one_total <= WARM_CACHE_DECODED_BYTES_MAX,
        "bytes_plus_one_lru_reclaimed": bytes_lru_evicted,
        "entry_decoded_bytes_ceiling": WARM_CACHE_ENTRY_DECODED_BYTES_MAX,
        "single_entry_exact_cached": at_outcome == PutOutcome::Cached,
        "single_entry_plus_one_refused": over_outcome == PutOutcome::TooLargeToCache,
        "idle_ttl_seconds": WARM_CACHE_IDLE_TTL_SECONDS,
        "idle_ttl_alive_probe_seconds": alive_elapsed,
        "idle_ttl_alive_before_expiry": alive,
        "idle_ttl_reclaim_probe_seconds": reclaim_elapsed,
        "idle_ttl_reclaimed": idle_reclaimed,
        "idle_ttl_total_bytes_after_reclaim": idle_bytes_after,
        "idle_reclamation_trigger": "lazy sweep on the next WarmCache::put/fork_matching; the module runs no timer",
        "decoded_bytes_accounting_basis":
            "caller-supplied hint, which flow::collab::write::hydrate_and_apply sets to \
             LoroCollabEngine::export_snapshot().len() -- an encoded-size proxy for resident heap, \
             not a measurement of it",
    })
}

// ---------------------------------------------------------------------------------------------
// Section 2 — ADR item 3: no observers or timers survive eviction/destroy
// ---------------------------------------------------------------------------------------------

fn section_observers_and_timers(violations: &mut Vec<String>) -> Value {
    // Static half: the modules that own cache entries register nothing that could outlive one.
    // Only the non-test portion is scanned -- `#[cfg(test)]` code below it legitimately sleeps.
    let mut static_hits: Vec<String> = Vec::new();
    for (label, path) in [("cache", CACHE_SOURCE), ("coordinator", COORDINATOR_SOURCE)] {
        let source = std::fs::read_to_string(path).expect("collab source file is readable");
        let production = source.split("#[cfg(test)]").next().unwrap_or(&source).to_string();
        for needle in [
            "tokio::spawn",
            "thread::spawn",
            "tokio::time::interval",
            "tokio::time::sleep",
            "thread::sleep",
            ".subscribe(",
            "Instant::now() +",
        ] {
            if production.contains(needle) {
                static_hits.push(format!("{label}.rs: {needle}"));
            }
        }
    }
    if !static_hits.is_empty() {
        violations.push(format!(
            "observer/timer: the cache/coordinator modules now contain background constructs: {static_hits:?}"
        ));
    }

    // Dynamic half: a full fill-then-reclaim cycle must leave no OS thread and no file descriptor
    // behind. A registered engine observer or a per-entry timer is a task or a handle; neither can
    // be created and abandoned without one of these two counters moving.
    let threads_before = proc_count("/proc/self/task");
    let fds_before = proc_count("/proc/self/fd");

    let cache = WarmCache::new();
    let mut ids = Vec::new();
    for index in 0..WARM_CACHE_DOCUMENTS_MAX {
        let id = Uuid::new_v4();
        ids.push(id);
        cache.put(
            id,
            filler_engine(),
            "loro-1".to_string(),
            i64::try_from(index).unwrap_or(0),
            vec![],
            1,
        );
    }
    // Force every one of them out by LRU pressure, then destroy the cache itself.
    for _ in 0..WARM_CACHE_DOCUMENTS_MAX {
        cache.put(Uuid::new_v4(), filler_engine(), "loro-1".to_string(), 0, vec![], 1);
    }
    let originals_remaining = ids.iter().filter(|id| cache.contains(**id)).count();
    let entries_after_cycle = cache.len();
    drop(cache);

    let threads_after = proc_count("/proc/self/task");
    let fds_after = proc_count("/proc/self/fd");
    let thread_delta = match (threads_before, threads_after) {
        (Some(before), Some(after)) => Some(after as i64 - before as i64),
        _ => None,
    };
    let fd_delta = match (fds_before, fds_after) {
        (Some(before), Some(after)) => Some(after as i64 - before as i64),
        _ => None,
    };
    if originals_remaining != 0 {
        violations.push(format!(
            "observer/timer: {originals_remaining} of the original entries survived a full \
             {WARM_CACHE_DOCUMENTS_MAX}-entry eviction cycle"
        ));
    }
    if thread_delta.is_some_and(|delta| delta > 0) {
        violations.push(format!(
            "observer/timer: OS thread count grew by {} across a fill/evict/destroy cycle",
            thread_delta.unwrap_or(0)
        ));
    }
    if fd_delta.is_some_and(|delta| delta > 0) {
        violations.push(format!(
            "observer/timer: open file descriptors grew by {} across a fill/evict/destroy cycle",
            fd_delta.unwrap_or(0)
        ));
    }

    json!({
        "static_background_constructs_in_cache_and_coordinator": static_hits,
        "static_scan_sources": [CACHE_SOURCE, COORDINATOR_SOURCE],
        "engine_observer_api_exists_in_collab_core": false,
        "cycle_entries_inserted": WARM_CACHE_DOCUMENTS_MAX * 2,
        "cycle_original_entries_remaining": originals_remaining,
        "cycle_entries_after": entries_after_cycle,
        "os_threads_before": threads_before,
        "os_threads_after": threads_after,
        "os_threads_delta": thread_delta,
        "open_fds_before": fds_before,
        "open_fds_after": fds_after,
        "open_fds_delta": fd_delta,
    })
}

// ---------------------------------------------------------------------------------------------
// Section 3 — ADR item 3: post-eviction rebuild is identical to the warm state
// ---------------------------------------------------------------------------------------------

async fn section_rebuild_after_eviction(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    actor_id: Uuid,
    document_id: Uuid,
    epoch: i64,
    mutant: Mutant,
    violations: &mut Vec<String>,
) -> Value {
    let instance = Instance::new();
    let writes = 6usize;
    for index in 0..writes {
        let base = head_state_bytes(db, document_id).await;
        let bytes = build_update(&base, &format!("rebuild-source-{index}"));
        let outcome = write_once(
            db,
            &instance,
            document_id,
            workspace_id,
            actor_id,
            epoch,
            bytes,
            Uuid::new_v4(),
        )
        .await;
        assert!(
            matches!(outcome, AcceptOutcome::Accepted(_)),
            "seed write {index} must be accepted"
        );
    }

    let doc = read_document(db, document_id).await;
    let warm_before_eviction = instance.cache.contains(document_id);
    let warm_engine = engine_of(&instance.cache, document_id, doc.head_seq, &doc.format_version);
    let warm_hash = warm_engine.as_ref().map(semantic_hash);
    if warm_engine.is_none() {
        violations.push(
            "rebuild: the warm cache did not hold this document at its committed head after six \
             consecutive writes; there is no warm state to compare a rebuild against"
                .to_string(),
        );
    }

    let filler_inserted = evict_by_entry_pressure(&instance.cache, document_id);
    let warm_after_eviction = instance.cache.contains(document_id);
    if warm_after_eviction {
        violations.push("rebuild: the document survived LRU pressure past the entry ceiling".to_string());
    }
    // A cache that no longer holds the document must also refuse to serve it.
    let serves_after_eviction = engine_of(&instance.cache, document_id, doc.head_seq, &doc.format_version).is_some();
    if serves_after_eviction {
        violations.push("rebuild: an evicted document was still served by fork_matching".to_string());
    }

    let (rebuilt, rebuilt_head, tail_len) =
        rebuild_from_bootstrap(db, document_id, mutant == Mutant::RebuildSkipLastTail).await;
    let rebuilt_hash = semantic_hash(&rebuilt);
    let hash_equal = warm_hash.as_deref() == Some(rebuilt_hash.as_str());
    let head_equal = rebuilt_head == doc.head_seq;
    if !hash_equal {
        violations.push(format!(
            "rebuild: semantic hash after eviction differs -- warm {warm_hash:?} vs bootstrap-rebuilt \
             {rebuilt_hash}"
        ));
    }
    if !head_equal {
        violations.push(format!(
            "rebuild: rebuilt head {rebuilt_head} != committed head {}",
            doc.head_seq
        ));
    }

    // The point of the clause: the instance keeps working after the eviction.
    let post_base = head_state_bytes(db, document_id).await;
    let bytes = build_update(&post_base, "post-eviction-write");
    let post = write_once(
        db,
        &instance,
        document_id,
        workspace_id,
        actor_id,
        epoch,
        bytes,
        Uuid::new_v4(),
    )
    .await;
    let post_accepted = match &post {
        AcceptOutcome::Accepted(accepted) => accepted.head_seq == doc.head_seq + 1,
        AcceptOutcome::Rejected(_) => false,
    };
    if !post_accepted {
        violations.push("rebuild: the first write after an eviction was not accepted at head+1".to_string());
    }

    json!({
        "seed_writes": writes,
        "seed_write_shape": "strictly sequential: each update is built against the previous head state, \
                             so the last accepted update is the semantic winner and dropping it from a \
                             rebuild is observable",
        "committed_head_seq": doc.head_seq,
        "warm_cache_held_document_before_eviction": warm_before_eviction,
        "eviction_mechanism": "real LRU pressure past warm_cache_documents_per_instance_max",
        "eviction_filler_entries_inserted": filler_inserted,
        "warm_cache_holds_document_after_eviction": warm_after_eviction,
        "fork_matching_serves_after_eviction": serves_after_eviction,
        "rebuild_source": "flow::collab::bootstrap::load (REPEATABLE READ READ ONLY snapshot + tail)",
        "rebuild_tail_updates_replayed": tail_len,
        "warm_semantic_hash": warm_hash,
        "rebuilt_semantic_hash": rebuilt_hash,
        "semantic_hash_equal": hash_equal,
        "rebuilt_head_seq": rebuilt_head,
        "head_equal": head_equal,
        "first_write_after_eviction_accepted_at_head_plus_one": post_accepted,
    })
}

// ---------------------------------------------------------------------------------------------
// Section 4 — ADR item 2 remainder: hit / miss / eviction / restart lose nothing
// ---------------------------------------------------------------------------------------------

async fn section_no_accepted_update_lost(
    db: &DatabaseConnection,
    scratch_url: &str,
    workspace_id: Uuid,
    actor_id: Uuid,
    document_id: Uuid,
    epoch: i64,
    violations: &mut Vec<String>,
) -> Value {
    let mut ledger: Vec<Value> = Vec::new();
    let shared = Instance::new();

    // Each condition is *established and verified* before its write, so the recorded label is an
    // observation and not an intention.
    // "warmup" is a real write on the shared instance whose only purpose is to leave a resident
    // entry at the committed head, so the "hit" writes below can be *established and verified*
    // rather than hoped for. It is counted in the ledger like any other accepted update.
    let plan: [(&str, usize); 5] = [("miss", 2), ("warmup", 1), ("hit", 2), ("eviction", 2), ("restart", 1)];
    let mut connection: Option<DatabaseConnection> = None;

    for (condition, count) in plan {
        for index in 0..count {
            let active_db: &DatabaseConnection = connection.as_ref().unwrap_or(db);
            let doc = read_document(active_db, document_id).await;
            let update_id = Uuid::new_v4();
            let base = head_state_bytes(active_db, document_id).await;
            let bytes = build_update(&base, &format!("{condition}-{index}"));

            let (outcome, observed_condition) = match condition {
                "miss" => {
                    let cold = Instance::new();
                    assert_eq!(cold.cache.len(), 0, "a fresh instance cache must be empty");
                    let outcome = write_once(
                        active_db,
                        &cold,
                        document_id,
                        workspace_id,
                        actor_id,
                        epoch,
                        bytes,
                        update_id,
                    )
                    .await;
                    (outcome, "miss (fresh empty WarmCache, hydrate via bootstrap loader)")
                }
                "warmup" => {
                    let outcome = write_once(
                        active_db,
                        &shared,
                        document_id,
                        workspace_id,
                        actor_id,
                        epoch,
                        bytes,
                        update_id,
                    )
                    .await;
                    (
                        outcome,
                        "warmup (shared instance, cold on entry; seeds the entry the 'hit' writes need)",
                    )
                }
                "hit" => {
                    let served = engine_of(&shared.cache, document_id, doc.head_seq, &doc.format_version).is_some();
                    if !served {
                        violations.push(format!(
                            "no_lost_updates: the '{condition}' write could not be established -- the \
                             shared cache did not hold the document at head {}",
                            doc.head_seq
                        ));
                    }
                    let outcome = write_once(
                        active_db,
                        &shared,
                        document_id,
                        workspace_id,
                        actor_id,
                        epoch,
                        bytes,
                        update_id,
                    )
                    .await;
                    (outcome, "hit (fork_matching returned the entry at the observed head)")
                }
                "eviction" => {
                    if !shared.cache.contains(document_id) {
                        // Make sure there is something to evict, so the label is honest.
                        shared.cache.put(
                            document_id,
                            filler_engine(),
                            doc.format_version.clone(),
                            doc.head_seq,
                            doc.head_frontier.clone(),
                            1,
                        );
                    }
                    evict_by_entry_pressure(&shared.cache, document_id);
                    if shared.cache.contains(document_id) {
                        violations.push("no_lost_updates: could not evict the document under LRU pressure".to_string());
                    }
                    let outcome = write_once(
                        active_db,
                        &shared,
                        document_id,
                        workspace_id,
                        actor_id,
                        epoch,
                        bytes,
                        update_id,
                    )
                    .await;
                    (outcome, "eviction (entry LRU-reclaimed immediately before the write)")
                }
                _ => {
                    // "restart": destroy every piece of process-local collab state and the
                    // connection pool, then rebuild from nothing but the database.
                    drop(connection.take());
                    let fresh_db = Database::connect(scratch_url)
                        .await
                        .expect("a restarted instance reconnects");
                    let fresh = Instance::new();
                    let base = head_state_bytes(&fresh_db, document_id).await;
                    let bytes = build_update(&base, "restart-0");
                    let outcome = write_once(
                        &fresh_db,
                        &fresh,
                        document_id,
                        workspace_id,
                        actor_id,
                        epoch,
                        bytes,
                        update_id,
                    )
                    .await;
                    connection = Some(fresh_db);
                    (
                        outcome,
                        "restart (all process-local cache/coordinator/registry/advancer state and the \
                         connection pool destroyed and rebuilt from PostgreSQL)",
                    )
                }
            };

            match outcome {
                AcceptOutcome::Accepted(accepted) => ledger.push(json!({
                    "cache_condition": observed_condition,
                    "update_id": update_id,
                    "accepted_seq": accepted.head_seq,
                })),
                AcceptOutcome::Rejected(rejected) => {
                    violations.push(format!(
                        "no_lost_updates: the '{condition}' write was rejected ({:?}); every condition in \
                         this section must accept",
                        rejected.code
                    ));
                    ledger.push(json!({
                        "cache_condition": observed_condition,
                        "update_id": update_id,
                        "rejected": format!("{:?}", rejected.code),
                    }));
                }
            }
        }
    }

    let verify_db: &DatabaseConnection = connection.as_ref().unwrap_or(db);
    let seqs = all_seqs(verify_db, document_id).await;
    let doc = read_document(verify_db, document_id).await;
    let expected = ledger.len() as i64;
    let contiguous = contiguous_from_one(&seqs);
    let distinct_update_ids = count_scalar(
        verify_db,
        "SELECT count(DISTINCT update_id) AS n FROM collab_updates WHERE document_id = $1",
        vec![document_id.into()],
    )
    .await;
    if seqs.len() as i64 != expected {
        violations.push(format!(
            "no_lost_updates: {} accepted updates but {} collab_updates rows",
            expected,
            seqs.len()
        ));
    }
    if !contiguous {
        violations.push(format!("no_lost_updates: seq is not contiguous from 1: {seqs:?}"));
    }
    if doc.head_seq != expected {
        violations.push(format!(
            "no_lost_updates: head_seq {} != {expected} accepted updates",
            doc.head_seq
        ));
    }
    if distinct_update_ids != expected {
        violations.push(format!(
            "no_lost_updates: {distinct_update_ids} distinct update_ids for {expected} accepted updates"
        ));
    }

    // Every accepted update is still reachable through the consistency loader after the restart.
    let (rebuilt, rebuilt_head, _) = rebuild_from_bootstrap(verify_db, document_id, false).await;
    let rebuilt_hash = semantic_hash(&rebuilt);
    let live_cache_hash = engine_of(&shared.cache, document_id, doc.head_seq, &doc.format_version)
        .as_ref()
        .map(semantic_hash);
    if rebuilt_head != doc.head_seq {
        violations.push("no_lost_updates: post-restart rebuild head does not match canonical head".to_string());
    }

    json!({
        "conditions_exercised": ["miss", "warmup", "hit", "eviction", "restart"],
        "writes": ledger,
        "accepted_total": expected,
        "collab_updates_rows": seqs.len(),
        "distinct_update_ids": distinct_update_ids,
        "seq_list": seqs,
        "seq_contiguous_from_one": contiguous,
        "canonical_head_seq": doc.head_seq,
        "post_restart_rebuilt_head_seq": rebuilt_head,
        "post_restart_rebuilt_semantic_hash": rebuilt_hash,
        "live_cache_semantic_hash_after_restart": live_cache_hash,
        "restart_mode":
            "process-local state destroyed in-process (cache, coordinator, registry, snapshot advancer, \
             connection pool). Not a re-exec of the API binary: durability of accepted updates is \
             asserted against PostgreSQL rows, which survive either.",
    })
}

// ---------------------------------------------------------------------------------------------
// Section 5 — ADR item 2 remainder: bounded-retry exhaustion writes nothing
// ---------------------------------------------------------------------------------------------

async fn section_retry_exhaustion(
    db: &DatabaseConnection,
    scratch_url: &str,
    workspace_id: Uuid,
    actor_id: Uuid,
    document_id: Uuid,
    epoch: i64,
    violations: &mut Vec<String>,
) -> Value {
    // ---- Deterministic arm: the row lock is held by a third party for longer than the whole
    // bounded-retry budget, so each of the three attempts burns its `lock_timeout` and rolls back.
    let blocker_db = Database::connect(scratch_url).await.expect("blocker connects");
    let blocker = blocker_db.begin().await.expect("blocker transaction opens");
    let locked_row = blocker
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_seq FROM collab_documents WHERE id = $1 FOR UPDATE",
            vec![document_id.into()],
        ))
        .await
        .expect("blocker row-lock query runs");
    assert!(locked_row.is_some(), "blocker must take the document row lock");

    let before = read_document(db, document_id).await;
    let before_updates = count_scalar(
        db,
        "SELECT count(*) AS n FROM collab_updates WHERE document_id = $1",
        vec![document_id.into()],
    )
    .await;
    let before_events = count_scalar(
        db,
        "SELECT count(*) AS n FROM business_events WHERE workspace_id = $1",
        vec![workspace_id.into()],
    )
    .await;
    let before_dispatch = count_scalar(db, "SELECT count(*) AS n FROM event_dispatch", vec![]).await;

    let instance = Instance::new();
    let victim_id = Uuid::new_v4();
    let bytes = build_update(&before.snapshot, "exhaustion-victim");
    let started = Instant::now();
    let outcome = write_once(
        db,
        &instance,
        document_id,
        workspace_id,
        actor_id,
        epoch,
        bytes,
        victim_id,
    )
    .await;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let _ = blocker.rollback().await;
    drop(blocker_db);

    let (code, recoverable, write_state, details) = match &outcome {
        AcceptOutcome::Accepted(accepted) => {
            violations.push(format!(
                "exhaustion: the victim write was accepted at seq {} despite the row lock being held \
                 for the entire bounded-retry budget",
                accepted.head_seq
            ));
            ("accepted".to_string(), false, "n/a".to_string(), Value::Null)
        }
        AcceptOutcome::Rejected(rejected) => (
            format!("{:?}", rejected.code),
            rejected.recoverable,
            format!("{:?}", rejected.write_state),
            rejected.details.clone().unwrap_or(Value::Null),
        ),
    };
    if let AcceptOutcome::Rejected(rejected) = &outcome {
        if rejected.code != RejectedCode::ServerDraining {
            violations.push(format!("exhaustion: expected server_draining, got {:?}", rejected.code));
        }
        if !rejected.recoverable {
            violations.push("exhaustion: the rejection was not marked recoverable".to_string());
        }
        if details.get("reason").and_then(Value::as_str) != Some("contention") {
            violations.push(format!("exhaustion: details.reason was not \"contention\": {details}"));
        }
        if details.get("retry_after_ms").is_none() {
            violations.push("exhaustion: no retry_after_ms was supplied with the backoff".to_string());
        }
    }

    let after = read_document(db, document_id).await;
    let after_updates = count_scalar(
        db,
        "SELECT count(*) AS n FROM collab_updates WHERE document_id = $1",
        vec![document_id.into()],
    )
    .await;
    let after_events = count_scalar(
        db,
        "SELECT count(*) AS n FROM business_events WHERE workspace_id = $1",
        vec![workspace_id.into()],
    )
    .await;
    let after_dispatch = count_scalar(db, "SELECT count(*) AS n FROM event_dispatch", vec![]).await;
    let victim_rows = count_scalar(
        db,
        "SELECT count(*) AS n FROM collab_updates WHERE document_id = $1 AND update_id = $2",
        vec![document_id.into(), victim_id.into()],
    )
    .await;

    let canonical_unchanged = after.head_seq == before.head_seq
        && after.head_frontier == before.head_frontier
        && after_updates == before_updates
        && after_events == before_events
        && after_dispatch == before_dispatch
        && victim_rows == 0;
    if !canonical_unchanged {
        violations.push(format!(
            "exhaustion: canonical state moved -- head {}->{}, updates {before_updates}->{after_updates}, \
             events {before_events}->{after_events}, dispatch {before_dispatch}->{after_dispatch}, \
             victim rows {victim_rows}",
            before.head_seq, after.head_seq
        ));
    }
    // Three attempts, each bounded by the 100 ms lock wait, is the floor for this construction.
    let attempts_floor_ms = f64::from(MAX_REBASE_ATTEMPTS) * DOCUMENT_LOCK_WAIT_MS_MAX as f64;
    if elapsed_ms < attempts_floor_ms * 0.8 {
        violations.push(format!(
            "exhaustion: the victim gave up after {elapsed_ms:.0} ms, well under the \
             {MAX_REBASE_ATTEMPTS} x {DOCUMENT_LOCK_WAIT_MS_MAX} ms the bounded retry implies -- it \
             probably did not make all three attempts"
        ));
    }

    // ---- Genuine head-mismatch arm. Nothing here is left to the scheduler: `flow_objects` is the
    // one table `bootstrap::load` reads that no other statement on the accept path touches (see
    // [`HYDRATE_GATE_TABLE`]), so an `ACCESS EXCLUSIVE` lock on it parks a writer *after* it has
    // read the head it will be checked against and *before* the row lock that checks it. Real
    // production writes are committed through that window, so the victim's next `FOR UPDATE`
    // provably finds a moved head. Both sides of the ceiling are exercised, because a bound is
    // only demonstrated by showing where it does *not* fire either.
    let forced_events_before = count_scalar(
        db,
        "SELECT count(*) AS n FROM business_events WHERE workspace_id = $1",
        vec![workspace_id.into()],
    )
    .await;
    let forced_dispatch_before = count_scalar(db, "SELECT count(*) AS n FROM event_dispatch", vec![]).await;

    let recovered = force_head_mismatches(
        db,
        scratch_url,
        workspace_id,
        actor_id,
        document_id,
        epoch,
        MAX_REBASE_ATTEMPTS - 1,
        "rebase_recovers",
        violations,
    )
    .await;
    let recovered_seq = match &recovered.outcome {
        AcceptOutcome::Accepted(accepted) => Some(accepted.head_seq),
        AcceptOutcome::Rejected(rejected) => {
            violations.push(format!(
                "rebase_recovers: a victim that lost {} head races -- one short of the \
                 {MAX_REBASE_ATTEMPTS}-attempt ceiling -- came back {:?} instead of being rebased \
                 onto the moved head and accepted",
                MAX_REBASE_ATTEMPTS - 1,
                rejected.code
            ));
            None
        }
    };
    if let Some(seq) = recovered_seq {
        let expected = recovered.head_before + i64::from(MAX_REBASE_ATTEMPTS - 1) + 1;
        if seq != expected {
            violations.push(format!(
                "rebase_recovers: the recovered write landed at seq {seq}, not {expected} (head \
                 {} + {} forced advances + itself)",
                recovered.head_before,
                MAX_REBASE_ATTEMPTS - 1
            ));
        }
    }
    let recovered_rows = count_scalar(
        db,
        "SELECT count(*) AS n FROM collab_updates WHERE document_id = $1 AND update_id = $2",
        vec![document_id.into(), recovered.update_id.into()],
    )
    .await;
    if recovered_seq.is_some() && recovered_rows != 1 {
        violations.push(format!(
            "rebase_recovers: the accepted write left {recovered_rows} collab_updates rows, not 1"
        ));
    }

    let exhausted = force_head_mismatches(
        db,
        scratch_url,
        workspace_id,
        actor_id,
        document_id,
        epoch,
        MAX_REBASE_ATTEMPTS,
        "exhaustion",
        violations,
    )
    .await;
    let (mismatch_code, mismatch_recoverable, mismatch_write_state, mismatch_details) = match &exhausted.outcome {
        AcceptOutcome::Accepted(accepted) => {
            violations.push(format!(
                "exhaustion: the victim was accepted at seq {} after losing {MAX_REBASE_ATTEMPTS} \
                 head races -- the bounded rebase did not stop at its ceiling",
                accepted.head_seq
            ));
            ("accepted".to_string(), false, "n/a".to_string(), Value::Null)
        }
        AcceptOutcome::Rejected(rejected) => {
            if rejected.code != RejectedCode::ServerDraining {
                violations.push(format!(
                    "exhaustion: head-mismatch exhaustion reported {:?}, not server_draining",
                    rejected.code
                ));
            }
            if !rejected.recoverable {
                violations.push("exhaustion: head-mismatch exhaustion was not marked recoverable".to_string());
            }
            let write_state = format!("{:?}", rejected.write_state);
            if write_state != "NotApplied" {
                violations.push(format!(
                    "exhaustion: head-mismatch exhaustion reported write_state {write_state}, not NotApplied"
                ));
            }
            let details = rejected.details.clone().unwrap_or(Value::Null);
            if details.get("reason").and_then(Value::as_str) != Some("contention") {
                violations.push(format!(
                    "exhaustion: head-mismatch details.reason was not \"contention\": {details}"
                ));
            }
            if details.get("retry_after_ms").is_none() {
                violations
                    .push("exhaustion: no retry_after_ms was supplied with the head-mismatch backoff".to_string());
            }
            (
                format!("{:?}", rejected.code),
                rejected.recoverable,
                write_state,
                details,
            )
        }
    };
    let mismatch_victim_rows = count_scalar(
        db,
        "SELECT count(*) AS n FROM collab_updates WHERE document_id = $1 AND update_id = $2",
        vec![document_id.into(), exhausted.update_id.into()],
    )
    .await;
    // The gate script reads this key: it must stay an empty list.
    let mut rejected_victims_that_left_a_row: Vec<String> = Vec::new();
    if mismatch_victim_rows != 0 {
        rejected_victims_that_left_a_row.push(exhausted.update_id.to_string());
        violations.push(format!(
            "exhaustion: the exhausted victim left {mismatch_victim_rows} collab_updates row(s) behind"
        ));
    }
    let mismatch_head_advanced = exhausted.head_after - exhausted.head_before;
    // Every seq the head gained during the victim's call is accounted for: the forced mismatches,
    // plus the victim's own commit if it made one. Anything else means a write nobody asked for.
    let expected_head_advance =
        i64::from(MAX_REBASE_ATTEMPTS) + i64::from(matches!(exhausted.outcome, AcceptOutcome::Accepted(_)));
    if mismatch_head_advanced != expected_head_advance {
        violations.push(format!(
            "exhaustion: the head advanced {mismatch_head_advanced} times during the victim's call, \
             not the {expected_head_advance} this construction can account for"
        ));
    }

    // `collab-protocol-v1.md` calls this rejection recoverable: the identical bytes under the
    // identical `update_id`, resubmitted once the head has stopped moving, must land. Proven by
    // resubmitting them, not read off the `recoverable` flag.
    let retry_db = Database::connect(scratch_url).await.expect("retry connects");
    let retry_instance = Instance::new();
    let retry_outcome = write_once(
        &retry_db,
        &retry_instance,
        document_id,
        workspace_id,
        actor_id,
        epoch,
        exhausted.bytes.clone(),
        exhausted.update_id,
    )
    .await;
    let retry_seq = match &retry_outcome {
        AcceptOutcome::Accepted(accepted) => Some(accepted.head_seq),
        AcceptOutcome::Rejected(rejected) => {
            violations.push(format!(
                "exhaustion: the recoverable rejection was not recoverable -- the same update_id \
                 resubmitted against a quiet head came back {:?}",
                rejected.code
            ));
            None
        }
    };

    let forced_events_after = count_scalar(
        db,
        "SELECT count(*) AS n FROM business_events WHERE workspace_id = $1",
        vec![workspace_id.into()],
    )
    .await;
    let forced_dispatch_after = count_scalar(db, "SELECT count(*) AS n FROM event_dispatch", vec![]).await;
    // An accepted retry only adds a fact when the victim really was rejected; a retry that
    // replays an already-committed `update_id` is served from the existing row and writes nothing.
    let retry_committed = retry_seq.is_some() && matches!(exhausted.outcome, AcceptOutcome::Rejected(_));
    let forced_commits = recovered.committed_writes + exhausted.committed_writes + i64::from(retry_committed);
    if forced_events_after - forced_events_before != forced_commits {
        violations.push(format!(
            "exhaustion: business_events moved by {} across the forced-mismatch arms while only \
             {forced_commits} writes committed -- a failed attempt left a fact behind",
            forced_events_after - forced_events_before
        ));
    }
    if forced_dispatch_after - forced_dispatch_before != forced_commits {
        violations.push(format!(
            "exhaustion: event_dispatch moved by {} across the forced-mismatch arms while only \
             {forced_commits} writes committed -- a failed attempt left a dispatch row behind",
            forced_dispatch_after - forced_dispatch_before
        ));
    }

    let final_seqs = all_seqs(db, document_id).await;
    let final_doc = read_document(db, document_id).await;
    let final_contiguous = contiguous_from_one(&final_seqs);
    if !final_contiguous {
        violations.push(format!(
            "exhaustion: after the forced head mismatches, seq is not contiguous from 1: {final_seqs:?}"
        ));
    }
    if final_doc.head_seq != final_seqs.len() as i64 {
        violations.push(format!(
            "exhaustion: after the forced head mismatches, head_seq {} != {} collab_updates rows",
            final_doc.head_seq,
            final_seqs.len()
        ));
    }

    json!({
        "max_rebase_attempts": MAX_REBASE_ATTEMPTS,
        "lock_wait_exhaustion": {
            "construction": "a third-party transaction holds SELECT ... FOR UPDATE on collab_documents for \
                             the whole bounded-retry budget, so all three attempts hit SET LOCAL lock_timeout",
            "elapsed_ms": elapsed_ms,
            "three_attempt_floor_ms": attempts_floor_ms,
            "rejected_code": code,
            "recoverable": recoverable,
            "write_state": write_state,
            "details": details,
            "canonical_head_before": before.head_seq,
            "canonical_head_after": after.head_seq,
            "collab_updates_before": before_updates,
            "collab_updates_after": after_updates,
            "business_events_before": before_events,
            "business_events_after": after_events,
            "event_dispatch_before": before_dispatch,
            "event_dispatch_after": after_dispatch,
            "victim_collab_update_rows": victim_rows,
            "canonical_state_unchanged": canonical_unchanged,
        },
        "head_mismatch_exhaustion": {
            "construction": "an ACCESS EXCLUSIVE lock on flow_objects -- the one table bootstrap::load \
                             reads that no other statement on the accept path touches -- parks the \
                             victim between its out-of-lock read_observed_head and its \
                             SELECT ... FOR UPDATE, and a real production write is committed through \
                             that window once per attempt, so every failed attempt is a constructed \
                             head mismatch rather than an observed race",
            "deterministic": true,
            "discriminator": "the statement parked at each rendezvous is asserted to be \
                              bootstrap::load's document read, and zero transactions hold a writer \
                              lock on collab_documents when the gate opens -- so the victim's next \
                              FOR UPDATE cannot be a lock wait, only a moved head",
            "rebase_recovers_one_below_the_ceiling": recovered.evidence,
            "exhausts_at_the_ceiling": exhausted.evidence,
            "rejected_code": mismatch_code,
            "recoverable": mismatch_recoverable,
            "write_state": mismatch_write_state,
            "details": mismatch_details,
            "head_advanced_during_the_exhausted_call": mismatch_head_advanced,
            "victim_collab_update_rows": mismatch_victim_rows,
            "rejected_victims_that_left_a_row": rejected_victims_that_left_a_row,
            "recovered_accepted_seq": recovered_seq,
            "same_update_id_retry_accepted_seq": retry_seq,
            "committed_writes": forced_commits,
            "business_events_delta": forced_events_after - forced_events_before,
            "event_dispatch_delta": forced_dispatch_after - forced_dispatch_before,
        },
        "post_forced_mismatch_seq_contiguous": final_contiguous,
        "post_forced_mismatch_head_seq": final_doc.head_seq,
        "post_forced_mismatch_collab_updates_rows": final_seqs.len(),
    })
}

// ---------------------------------------------------------------------------------------------
// Deterministic head-mismatch construction
// ---------------------------------------------------------------------------------------------

/// The one table `bootstrap::load` reads that no other statement on the accept path touches.
///
/// `accept_update`'s complete statement inventory, in execution order: `find_prior_update`
/// (`collab_updates`, retries only), `read_observed_head` (`collab_documents`), `bootstrap::load`
/// on a cache miss (`collab_documents JOIN flow_objects`, then `collab_updates`),
/// `fence_epoch_for_share` (`flow_workspace_settings`), the locked `SELECT ... FOR UPDATE`
/// (`collab_documents`), `insert_flow_event` (`business_events`, `event_dispatch`), the
/// `collab_updates` insert, and the `collab_documents` / `flow_object_projections` updates.
/// `flow_objects` appears exactly once in that list: in `bootstrap::load`'s first query, which
/// runs **after** `read_observed_head` has pinned the head this attempt will be checked against
/// and **before** the row lock that checks it. (The two `UPDATE`s against tables that carry a
/// foreign key to `flow_objects` never write the referencing column, so `PostgreSQL` re-checks no
/// constraint and takes no lock on it there.)
///
/// Holding `ACCESS EXCLUSIVE` on it therefore parks a writer precisely inside the window a head
/// mismatch needs, for exactly as long as the lock is held, with **no timeout anywhere in the
/// parked path**: `bootstrap::load`'s `REPEATABLE READ READ ONLY` transaction sets none, and the
/// accept path's `SET LOCAL lock_timeout`/`statement_timeout` live inside the later write
/// transaction, which has not been opened yet. That is what makes this construction a sequencing
/// device rather than a race with a wider window.
const HYDRATE_GATE_TABLE: &str = "flow_objects";

/// The opening of `bootstrap::load`'s document read. Asserted against `pg_stat_activity` at every
/// rendezvous, so the construction fails loudly instead of silently gating some other statement if
/// the loader's query inventory ever changes.
const HYDRATE_GATE_STATEMENT_PREFIX: &str = "SELECT fo.workspace_id";

/// Tripwire on one rendezvous wait. Nothing in the construction is timing-dependent, so this is
/// not a budget: reaching it means the interleaving being waited for can no longer happen at all.
const GATE_RENDEZVOUS_TIMEOUT: Duration = Duration::from_secs(30);

/// Every table-lock mode on `collab_documents` a *writer* takes (`SELECT ... FOR UPDATE` takes
/// `RowShareLock`, the `UPDATE` takes `RowExclusiveLock`). Counted the moment before each gate
/// opens: zero is what proves the victim's next `SELECT ... FOR UPDATE` cannot be a lock wait, and
/// therefore that the rejection it goes on to produce is a head mismatch and nothing else.
const DOCUMENT_WRITER_LOCK_PROBE: &str = "SELECT count(*) AS n FROM pg_locks \
     WHERE locktype = 'relation' AND relation = 'collab_documents'::regclass \
       AND database = (SELECT oid FROM pg_database WHERE datname = current_database()) \
       AND mode IN ('RowShareLock', 'RowExclusiveLock', 'ShareLock', 'ShareRowExclusiveLock', \
                    'ExclusiveLock', 'AccessExclusiveLock')";

/// A `pg_locks` count of one lock mode against [`HYDRATE_GATE_TABLE`], granted or not, in this
/// database only (`pg_locks` is cluster-wide, and a relation OID is only unique per database).
fn gate_lock_probe(mode: &str, granted: bool) -> String {
    format!(
        "SELECT count(*) AS n FROM pg_locks \
         WHERE locktype = 'relation' AND relation = '{HYDRATE_GATE_TABLE}'::regclass \
           AND database = (SELECT oid FROM pg_database WHERE datname = current_database()) \
           AND mode = '{mode}' AND granted IS {}",
        if granted { "TRUE" } else { "FALSE" }
    )
}

/// Polls `probe` until it counts at least one row. Returns how long that took, for the record.
async fn wait_for_lock(db: &DatabaseConnection, probe: &str, what: &str) -> Result<f64, String> {
    let started = Instant::now();
    loop {
        if count_scalar(db, probe, vec![]).await >= 1 {
            return Ok(started.elapsed().as_secs_f64() * 1000.0);
        }
        if started.elapsed() > GATE_RENDEZVOUS_TIMEOUT {
            return Err(format!(
                "waited {:.0} ms for {what} and it never happened",
                started.elapsed().as_secs_f64() * 1000.0
            ));
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// The statement text of whichever backend is currently parked at the gate, read from
/// `pg_stat_activity` -- the database's own account of what it is holding, not the test's.
async fn blocked_gate_statement(db: &DatabaseConnection) -> String {
    #[derive(FromQueryResult)]
    struct QueryRow {
        query: String,
    }
    QueryRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT coalesce(a.query, '') AS query FROM pg_locks l \
             JOIN pg_stat_activity a ON a.pid = l.pid \
             WHERE l.locktype = 'relation' AND l.relation = '{HYDRATE_GATE_TABLE}'::regclass \
               AND l.database = (SELECT oid FROM pg_database WHERE datname = current_database()) \
               AND l.mode = 'AccessShareLock' AND l.granted IS FALSE \
             LIMIT 1"
        ),
        vec![],
    ))
    .one(db)
    .await
    .ok()
    .flatten()
    .map_or_else(String::new, |row| row.query)
}

/// An open transaction holding `ACCESS EXCLUSIVE` on [`HYDRATE_GATE_TABLE`], released on command.
struct HydrateGate {
    releaser: tokio::sync::oneshot::Sender<()>,
    finished: tokio::task::JoinHandle<()>,
}

impl HydrateGate {
    /// Issues the lock request on its own connection. Returns immediately -- whether the request
    /// is granted or queued is read back from `pg_locks`, never assumed.
    fn arm(scratch_url: &str) -> Self {
        let (releaser, wait) = tokio::sync::oneshot::channel::<()>();
        let url = scratch_url.to_string();
        let finished = tokio::spawn(async move {
            let conn = Database::connect(&url).await.expect("hydrate gate connects");
            let tx = conn.begin().await.expect("hydrate gate transaction opens");
            tx.execute_unprepared(&format!("LOCK TABLE {HYDRATE_GATE_TABLE} IN ACCESS EXCLUSIVE MODE"))
                .await
                .expect("hydrate gate takes ACCESS EXCLUSIVE");
            let _ = wait.await;
            let _ = tx.rollback().await;
        });
        Self { releaser, finished }
    }

    /// Releases the gate and waits for the `ROLLBACK` to have been processed, so a caller that
    /// returns from here can rely on the lock actually being gone.
    async fn release(self) {
        let _ = self.releaser.send(());
        let _ = self.finished.await;
    }
}

/// What one run of the deterministic construction produced.
struct ForcedMismatchRun {
    outcome: AcceptOutcome,
    update_id: Uuid,
    /// The victim's exact payload, kept so the protocol's "retry the same `update_id`" can be
    /// exercised with the same bytes rather than a lookalike.
    bytes: Vec<u8>,
    head_before: i64,
    head_after: i64,
    /// Writes that really committed during this run, for the event/dispatch ledger check.
    committed_writes: i64,
    evidence: Value,
}

/// Forces exactly `forced` genuine head mismatches on one victim write, then lets it finish.
///
/// The victim is parked at [`HYDRATE_GATE_TABLE`] once per attempt -- after it has read the head
/// it will be checked against, before it can take the row lock that checks it -- and a real
/// production write is committed through that window each time. Nothing is retried until it
/// happens to interleave: every rendezvous is observed in `pg_locks` before the head is moved, and
/// the next gate is observed to be queued *ahead of the victim* before the current one is
/// released, so the victim cannot slip through between attempts.
#[allow(clippy::too_many_arguments)]
async fn force_head_mismatches(
    db: &DatabaseConnection,
    scratch_url: &str,
    workspace_id: Uuid,
    actor_id: Uuid,
    document_id: Uuid,
    epoch: i64,
    forced: u32,
    label: &str,
    violations: &mut Vec<String>,
) -> ForcedMismatchRun {
    let blocked_share = gate_lock_probe("AccessShareLock", false);
    let queued_exclusive = gate_lock_probe("AccessExclusiveLock", false);
    let granted_exclusive = gate_lock_probe("AccessExclusiveLock", true);

    // One competitor instance, warmed **before** the gate goes up: with its warm cache hit,
    // `hydrate_and_apply` never calls `bootstrap::load`, so this is the one writer that can still
    // commit while the gate is held. (A cold competitor would park at the same gate, and the run
    // would report that instead of deadlocking -- see the `parked_backends` check below.)
    let competitor_db = Database::connect(scratch_url).await.expect("competitor connects");
    let competitor = Instance::new();
    let warmup_bytes = build_update(&head_state_bytes(db, document_id).await, &format!("{label}-warmup"));
    let mut committed_writes = 0i64;
    match write_once(
        &competitor_db,
        &competitor,
        document_id,
        workspace_id,
        actor_id,
        epoch,
        warmup_bytes,
        Uuid::new_v4(),
    )
    .await
    {
        AcceptOutcome::Accepted(_) => committed_writes += 1,
        AcceptOutcome::Rejected(rejected) => violations.push(format!(
            "{label}: the competitor's cache-warming write came back {:?}",
            rejected.code
        )),
    }

    // Every payload is built now: building one reads the document through `bootstrap::load`, which
    // is exactly what the gate blocks.
    let base = head_state_bytes(db, document_id).await;
    let competing_bytes: Vec<Vec<u8>> = (0..forced)
        .map(|round| build_update(&base, &format!("{label}-competitor-{round}")))
        .collect();
    let victim_bytes = build_update(&base, &format!("{label}-victim"));
    let victim_id = Uuid::new_v4();
    let head_before = read_document(db, document_id).await.head_seq;

    let mut gate = Some(HydrateGate::arm(scratch_url));
    if let Err(err) = wait_for_lock(db, &granted_exclusive, "the first hydrate gate to be granted").await {
        violations.push(format!("{label}: {err}"));
    }

    let victim_db = Database::connect(scratch_url).await.expect("victim connects");
    let spawned_bytes = victim_bytes.clone();
    let started = Instant::now();
    let victim = tokio::spawn(async move {
        let instance = Instance::new();
        write_once(
            &victim_db,
            &instance,
            document_id,
            workspace_id,
            actor_id,
            epoch,
            spawned_bytes,
            victim_id,
        )
        .await
    });

    let mut rendezvous: Vec<Value> = Vec::new();
    for (round, bytes) in competing_bytes.into_iter().enumerate() {
        let parked_after_ms = match wait_for_lock(db, &blocked_share, "the victim to park at the hydrate gate").await {
            Ok(ms) => ms,
            Err(err) => {
                violations.push(format!("{label}: rendezvous {round}: {err}"));
                break;
            }
        };
        // Exactly one backend may be parked here. Two would mean the competitor missed its cache
        // and parked too, which would make the next line wait on a writer that cannot run.
        let parked_backends = count_scalar(db, &blocked_share, vec![]).await;
        if parked_backends != 1 {
            violations.push(format!(
                "{label}: rendezvous {round}: {parked_backends} backends are parked at the hydrate \
                 gate, not 1 -- the construction cannot say which one it is sequencing"
            ));
        }
        let held_statement = blocked_gate_statement(db).await;
        if !held_statement.starts_with(HYDRATE_GATE_STATEMENT_PREFIX) {
            violations.push(format!(
                "{label}: rendezvous {round}: the statement parked at the gate is not \
                 bootstrap::load's document read but {held_statement:?} -- the gate no longer sits \
                 between the observed head and the row lock"
            ));
        }

        // A real production write, committed while the victim is provably parked past its own
        // `read_observed_head`.
        let Ok(advance) = tokio::time::timeout(
            GATE_RENDEZVOUS_TIMEOUT,
            write_once(
                &competitor_db,
                &competitor,
                document_id,
                workspace_id,
                actor_id,
                epoch,
                bytes,
                Uuid::new_v4(),
            ),
        )
        .await
        else {
            violations.push(format!(
                "{label}: rendezvous {round}: the competing write never finished -- it is parked at \
                 the gate itself"
            ));
            break;
        };
        let advanced_to = match &advance {
            AcceptOutcome::Accepted(accepted) => {
                committed_writes += 1;
                Some(accepted.head_seq)
            }
            AcceptOutcome::Rejected(rejected) => {
                violations.push(format!(
                    "{label}: rendezvous {round}: the write that was supposed to move the head came \
                     back {:?}",
                    rejected.code
                ));
                None
            }
        };
        let writer_locks = count_scalar(db, DOCUMENT_WRITER_LOCK_PROBE, vec![]).await;
        if writer_locks != 0 {
            violations.push(format!(
                "{label}: rendezvous {round}: {writer_locks} transaction(s) still hold a writer lock \
                 on collab_documents as the gate opens -- the victim's next FOR UPDATE could be a \
                 lock wait rather than a head mismatch"
            ));
        }

        // Chain the next gate *before* releasing this one: once it is queued, the victim's next
        // hydrate necessarily queues behind it, which is what removes the between-attempts race.
        let next_gate = (round + 1 < forced as usize).then(|| HydrateGate::arm(scratch_url));
        if next_gate.is_some()
            && let Err(err) = wait_for_lock(db, &queued_exclusive, "the next hydrate gate to queue").await
        {
            violations.push(format!("{label}: rendezvous {round}: {err}"));
        }
        if let Some(open) = gate.take() {
            open.release().await;
        }
        if let Some(next) = next_gate {
            // Granted means the victim's parked read has been served *and finished*: any parked
            // backend seen after this point is a fresh hydrate, i.e. a fresh attempt.
            if let Err(err) = wait_for_lock(db, &granted_exclusive, "the next hydrate gate to take over").await {
                violations.push(format!("{label}: rendezvous {round}: {err}"));
            }
            gate = Some(next);
        }

        rendezvous.push(json!({
            "round": round,
            "victim_parked_after_ms": parked_after_ms,
            "backends_parked_at_the_gate": parked_backends,
            "statement_parked_at_the_gate": held_statement.chars().take(96).collect::<String>(),
            "competing_write_committed_at_seq": advanced_to,
            "collab_documents_writer_locks_when_the_gate_opened": writer_locks,
        }));
    }
    if let Some(open) = gate.take() {
        open.release().await;
    }

    let outcome = victim.await.expect("the victim write task joins");
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    if matches!(outcome, AcceptOutcome::Accepted(_)) {
        committed_writes += 1;
    }
    let head_after = read_document(db, document_id).await.head_seq;

    let evidence = json!({
        "forced_head_mismatches": forced,
        "head_before": head_before,
        "head_after": head_after,
        "head_advanced": head_after - head_before,
        "victim_elapsed_ms": elapsed_ms,
        "victim_outcome": match &outcome {
            AcceptOutcome::Accepted(accepted) => json!({"accepted_seq": accepted.head_seq}),
            AcceptOutcome::Rejected(rejected) => json!({
                "code": format!("{:?}", rejected.code),
                "recoverable": rejected.recoverable,
                "write_state": format!("{:?}", rejected.write_state),
                "details": rejected.details.clone().unwrap_or(Value::Null),
            }),
        },
        "rendezvous": rendezvous,
    });

    ForcedMismatchRun {
        outcome,
        update_id: victim_id,
        bytes: victim_bytes,
        head_before,
        head_after,
        committed_writes,
        evidence,
    }
}

// ---------------------------------------------------------------------------------------------
// Section 6 — ADR item 5: bypassing the cache, and deleting the cache/coordinator entirely
// ---------------------------------------------------------------------------------------------

struct UniquenessRun {
    accepted: usize,
    seqs: Vec<i64>,
    elapsed_ms: f64,
    /// First-pass rejections. `ADR-0010` and `collab-protocol-v1.md` make a bounded-rebase
    /// exhaustion a **recoverable** `server_draining{contention}` the client retries with the same
    /// `update_id`, not a lost write — so a rejection here is a throughput cost, and the claim
    /// under test is that retrying it lands it. Both facts are recorded.
    rejections: Vec<Value>,
    retries_needed: usize,
    accepted_after_retry: usize,
}

/// Runs `BYPASS_WRITERS` concurrent writes at one document. `share_instance` selects between the
/// production shape (one instance-local coordinator and one warm cache serializing/serving the
/// same-instance writers) and the "cache and coordinator deleted" shape (a fresh, empty cache and
/// a fresh coordinator per writer -- no cache can ever hit, and no instance-local serialization
/// exists at all, leaving the database row lock as the only authority).
#[allow(clippy::too_many_arguments)]
async fn uniqueness_run(
    db: &DatabaseConnection,
    scratch_url: &str,
    workspace_id: Uuid,
    actor_id: Uuid,
    document_id: Uuid,
    epoch: i64,
    base_snapshot: &[u8],
    label: &str,
    shared: Option<Arc<Instance>>,
    violations: &mut Vec<String>,
) -> UniquenessRun {
    // Every writer's delta is built up front from the same base snapshot, so a retry can resubmit
    // the identical bytes under the identical `update_id` -- the exact shape
    // `collab-protocol-v1.md` requires of a client that got a recoverable contention.
    let payloads: Vec<(Uuid, Vec<u8>)> = (0..BYPASS_WRITERS)
        .map(|writer| {
            (
                Uuid::new_v4(),
                build_update(base_snapshot, &format!("{label}-writer-{writer}")),
            )
        })
        .collect();

    let mut handles = Vec::new();
    let started = Instant::now();
    for (update_id, bytes) in payloads.clone() {
        let writer_db = Database::connect(scratch_url).await.expect("writer connects");
        let shared = shared.clone();
        handles.push(tokio::spawn(async move {
            let owned;
            let instance: &Instance = if let Some(instance) = shared.as_deref() {
                instance
            } else {
                owned = Instance::new();
                // "Cache deleted": this cache is created empty, used for exactly one write and
                // dropped, so `fork_matching` provably cannot hit for this write.
                assert_eq!(owned.cache.len(), 0);
                &owned
            };
            let outcome = write_once(
                &writer_db,
                instance,
                document_id,
                workspace_id,
                actor_id,
                epoch,
                bytes,
                update_id,
            )
            .await;
            (update_id, outcome)
        }));
    }
    let mut seqs = Vec::new();
    let mut rejections = Vec::new();
    let mut pending: Vec<Uuid> = Vec::new();
    for handle in handles {
        let (update_id, outcome) = handle.await.expect("writer task joins");
        match outcome {
            AcceptOutcome::Accepted(accepted) => seqs.push(accepted.head_seq),
            AcceptOutcome::Rejected(rejected) => {
                let rows = count_scalar(
                    db,
                    "SELECT count(*) AS n FROM collab_updates WHERE document_id = $1 AND update_id = $2",
                    vec![document_id.into(), update_id.into()],
                )
                .await;
                if rows != 0 {
                    violations.push(format!(
                        "item5 [{label}]: a rejected write left {rows} collab_updates row(s) behind"
                    ));
                }
                if !rejected.recoverable {
                    violations.push(format!(
                        "item5 [{label}]: rejection {:?} was not recoverable; contention must be retryable",
                        rejected.code
                    ));
                }
                if format!("{:?}", rejected.write_state) != "NotApplied" {
                    violations.push(format!(
                        "item5 [{label}]: rejection reported write_state {:?}, not NotApplied",
                        rejected.write_state
                    ));
                }
                rejections.push(json!({
                    "update_id": update_id,
                    "code": format!("{:?}", rejected.code),
                    "recoverable": rejected.recoverable,
                    "write_state": format!("{:?}", rejected.write_state),
                    "details": rejected.details,
                    "collab_update_rows_left_behind": rows,
                }));
                pending.push(update_id);
            }
        }
    }
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

    // The contract's own remedy, applied: resubmit each contended write under the same
    // `update_id`, one at a time. "Not lost" means it lands, not that it never had to retry.
    let mut retries_needed = 0usize;
    let mut accepted_after_retry = 0usize;
    for update_id in pending {
        let bytes = payloads
            .iter()
            .find(|(id, _)| *id == update_id)
            .map(|(_, bytes)| bytes.clone())
            .expect("payload for a pending update_id exists");
        let instance = Instance::new();
        let mut landed = false;
        for _ in 0..8 {
            retries_needed += 1;
            let outcome = write_once(
                db,
                &instance,
                document_id,
                workspace_id,
                actor_id,
                epoch,
                bytes.clone(),
                update_id,
            )
            .await;
            if let AcceptOutcome::Accepted(accepted) = outcome {
                seqs.push(accepted.head_seq);
                accepted_after_retry += 1;
                landed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if !landed {
            violations.push(format!(
                "item5 [{label}]: a contended write never landed within 8 protocol-shaped retries"
            ));
        }
    }

    seqs.sort_unstable();
    UniquenessRun {
        accepted: seqs.len(),
        seqs,
        elapsed_ms,
        rejections,
        retries_needed,
        accepted_after_retry,
    }
}

async fn section_bypass_and_removal(
    db: &DatabaseConnection,
    scratch_url: &str,
    state: &AppState,
    workspace_id: Uuid,
    actor_id: Uuid,
    epoch: i64,
    mutant: Mutant,
    violations: &mut Vec<String>,
) -> Value {
    // Separate documents so the two shapes are compared on identical, independent baselines.
    let (_, warm_doc) = create_page(state, workspace_id, actor_id, "Item5 Warm Baseline").await;
    let (_, bypass_doc) = create_page(state, workspace_id, actor_id, "Item5 Bypass").await;

    let warm_snapshot = read_document(db, warm_doc).await.snapshot;
    let shared = Arc::new(Instance::new());
    let warm = uniqueness_run(
        db,
        scratch_url,
        workspace_id,
        actor_id,
        warm_doc,
        epoch,
        &warm_snapshot,
        "warm",
        Some(shared),
        violations,
    )
    .await;

    let bypass_snapshot = read_document(db, bypass_doc).await.snapshot;
    let bypass = uniqueness_run(
        db,
        scratch_url,
        workspace_id,
        actor_id,
        bypass_doc,
        epoch,
        &bypass_snapshot,
        "bypass",
        None,
        violations,
    )
    .await;

    let warm_seqs = all_seqs(db, warm_doc).await;
    let warm_head = read_document(db, warm_doc).await.head_seq;
    let warm_unique = distinct(&warm.seqs) == warm.seqs.len();
    let bypass_seqs = all_seqs(db, bypass_doc).await;
    let bypass_head = read_document(db, bypass_doc).await.head_seq;
    let bypass_unique = distinct(&bypass.seqs) == bypass.seqs.len();

    for (label, run, seqs, head, unique) in [
        ("warm+coordinated", &warm, &warm_seqs, warm_head, warm_unique),
        (
            "cache+coordinator deleted",
            &bypass,
            &bypass_seqs,
            bypass_head,
            bypass_unique,
        ),
    ] {
        if !unique {
            violations.push(format!(
                "item5 [{label}]: accepted seqs were not unique: {:?}",
                run.seqs
            ));
        }
        if run.accepted != BYPASS_WRITERS {
            violations.push(format!(
                "item5 [{label}]: only {} of {BYPASS_WRITERS} writes ever landed, even after the \
                 protocol's own retry",
                run.accepted
            ));
        }
        if !contiguous_from_one(seqs) {
            violations.push(format!("item5 [{label}]: seq is not contiguous from 1: {seqs:?}"));
        }
        if head != seqs.len() as i64 {
            violations.push(format!(
                "item5 [{label}]: head_seq {head} != {} collab_updates rows",
                seqs.len()
            ));
        }
    }

    // ---- Negative control: the same allocation with the row lock removed.
    //
    // The production code cannot be asked to skip its own `FOR UPDATE`, so the *allocation
    // shape* is reproduced here twice against a real `collab_documents` row -- once with the row
    // lock and once without -- to show which of the two makes `seq` unique. This is the only
    // measurement in this file that does not run production code, and it is labelled as such.
    let (_, control_doc) = create_page(state, workspace_id, actor_id, "Item5 Lock Control").await;
    let with_lock = allocate_concurrently(scratch_url, control_doc, true).await;
    let without_lock = allocate_concurrently(scratch_url, control_doc, false).await;
    let with_lock_unique = distinct(&with_lock) == with_lock.len();
    let without_lock_unique = distinct(&without_lock) == without_lock.len();
    let expect_lock_authoritative = mutant != Mutant::BypassRowLock;
    if !with_lock_unique {
        violations.push(format!(
            "item5 negative control: SELECT ... FOR UPDATE did not make allocation unique: {with_lock:?}"
        ));
    }
    if without_lock_unique && expect_lock_authoritative {
        violations.push(format!(
            "item5 negative control: dropping FOR UPDATE still produced unique seqs ({without_lock:?}); \
             the control does not discriminate and therefore proves nothing about the lock"
        ));
    }
    if mutant == Mutant::BypassRowLock && !without_lock_unique {
        violations
            .push("item5 mutant bypass_row_lock: uniqueness failed without the row lock, as designed".to_string());
    }

    let concurrent_ratio = if warm.elapsed_ms > 0.0 {
        bypass.elapsed_ms / warm.elapsed_ms
    } else {
        f64::NAN
    };

    // ---- "只退化性能" needs a measurement where the cache can actually pay off. Eight writers
    // racing one document is dominated by row-lock contention, and deleting the coordinator there
    // makes the wall clock *shorter*, not longer, because the writers stop queueing locally. The
    // cache's job is avoiding a full bootstrap rebuild on a growing tail, so the honest comparison
    // is sequential writes on one document: warm (entry resident, incremental prepare) versus a
    // deleted cache (every write reloads snapshot + replays the whole tail).
    let sequential_writes = 40usize;
    let (_, seq_warm_doc) = create_page(state, workspace_id, actor_id, "Item5 Sequential Warm").await;
    let (_, seq_cold_doc) = create_page(state, workspace_id, actor_id, "Item5 Sequential Cold").await;
    let warm_instance = Instance::new();
    let mut warm_ms = 0.0f64;
    let mut cold_ms = 0.0f64;
    let mut warm_accepted = 0usize;
    let mut cold_accepted = 0usize;
    // Per-write samples, so the divergence as the tail grows is visible rather than averaged away:
    // a warm entry prepares incrementally at any tail depth, a deleted cache replays the whole tail
    // every time.
    let mut warm_samples: Vec<f64> = Vec::with_capacity(sequential_writes);
    let mut cold_samples: Vec<f64> = Vec::with_capacity(sequential_writes);
    for index in 0..sequential_writes {
        let doc = read_document(db, seq_warm_doc).await;
        let bytes = build_update(&doc.snapshot, &format!("seq-warm-{index}"));
        let started = Instant::now();
        let outcome = write_once(
            db,
            &warm_instance,
            seq_warm_doc,
            workspace_id,
            actor_id,
            epoch,
            bytes,
            Uuid::new_v4(),
        )
        .await;
        let sample = started.elapsed().as_secs_f64() * 1000.0;
        warm_ms += sample;
        warm_samples.push(sample);
        if matches!(outcome, AcceptOutcome::Accepted(_)) {
            warm_accepted += 1;
        }

        let doc = read_document(db, seq_cold_doc).await;
        let bytes = build_update(&doc.snapshot, &format!("seq-cold-{index}"));
        let cold_instance = Instance::new();
        let started = Instant::now();
        let outcome = write_once(
            db,
            &cold_instance,
            seq_cold_doc,
            workspace_id,
            actor_id,
            epoch,
            bytes,
            Uuid::new_v4(),
        )
        .await;
        let sample = started.elapsed().as_secs_f64() * 1000.0;
        cold_ms += sample;
        cold_samples.push(sample);
        if matches!(outcome, AcceptOutcome::Accepted(_)) {
            cold_accepted += 1;
        }
    }
    let quarter = sequential_writes / 4;
    let mean = |values: &[f64]| -> f64 {
        if values.is_empty() {
            f64::NAN
        } else {
            values.iter().sum::<f64>() / values.len() as f64
        }
    };
    let warm_first_quarter = mean(&warm_samples[..quarter]);
    let warm_last_quarter = mean(&warm_samples[sequential_writes - quarter..]);
    let cold_first_quarter = mean(&cold_samples[..quarter]);
    let cold_last_quarter = mean(&cold_samples[sequential_writes - quarter..]);
    let seq_warm_seqs = all_seqs(db, seq_warm_doc).await;
    let seq_cold_seqs = all_seqs(db, seq_cold_doc).await;
    for (label, accepted, seqs) in [
        ("sequential warm", warm_accepted, &seq_warm_seqs),
        ("sequential cache-deleted", cold_accepted, &seq_cold_seqs),
    ] {
        if accepted != sequential_writes || !contiguous_from_one(seqs) || seqs.len() != sequential_writes {
            violations.push(format!(
                "item5 [{label}]: {accepted} of {sequential_writes} accepted, canonical seqs {seqs:?}"
            ));
        }
    }

    json!({
        "concurrent_writers": BYPASS_WRITERS,
        "warm_and_coordinated": {
            "document_id": warm_doc,
            "shape": "one shared instance: one WarmCache, one DocumentCoordinator",
            "landed_total": warm.accepted,
            "first_pass_rejections": warm.rejections.len(),
            "rejection_details": warm.rejections,
            "protocol_retries_issued": warm.retries_needed,
            "landed_after_retry": warm.accepted_after_retry,
            "accepted_seqs": warm.seqs,
            "seqs_unique": warm_unique,
            "canonical_seqs": warm_seqs,
            "canonical_head_seq": warm_head,
            "wall_ms": warm.elapsed_ms,
        },
        "cache_and_coordinator_deleted": {
            "document_id": bypass_doc,
            "shape": "one fresh empty WarmCache and one fresh DocumentCoordinator per writer -- no cache \
                      can hit and no instance-local serialization exists; only the DB row lock remains",
            "every_writer_started_with_an_empty_cache": true,
            "landed_total": bypass.accepted,
            "first_pass_rejections": bypass.rejections.len(),
            "rejection_details": bypass.rejections,
            "protocol_retries_issued": bypass.retries_needed,
            "landed_after_retry": bypass.accepted_after_retry,
            "accepted_seqs": bypass.seqs,
            "seqs_unique": bypass_unique,
            "canonical_seqs": bypass_seqs,
            "canonical_head_seq": bypass_head,
            "wall_ms": bypass.elapsed_ms,
        },
        "concurrent_wall_ms_ratio_deleted_over_warm": concurrent_ratio,
        "concurrent_contention_cost_of_deleting_the_coordinator": {
            "note": "the instance-local coordinator's job is keeping same-instance writers from \
                     racing each other into the row lock. Deleting it does not cost correctness \
                     (seqs stay unique and contiguous) -- it costs client-visible recoverable \
                     contentions, which the protocol's retry then resolves.",
            "warm_first_pass_rejections": warm.rejections.len(),
            "deleted_first_pass_rejections": bypass.rejections.len(),
        },
        "sequential_degradation": {
            "note": "same document, one writer at a time, so the row lock is never contended and the \
                     only difference is whether the warm entry exists. This is where deleting the cache \
                     costs something.",
            "writes_per_arm": sequential_writes,
            "warm_total_ms": warm_ms,
            "warm_mean_ms": warm_ms / sequential_writes as f64,
            "warm_accepted": warm_accepted,
            "warm_canonical_seqs": seq_warm_seqs,
            "cache_deleted_total_ms": cold_ms,
            "cache_deleted_mean_ms": cold_ms / sequential_writes as f64,
            "cache_deleted_accepted": cold_accepted,
            "cache_deleted_canonical_seqs": seq_cold_seqs,
            "slowdown_ratio_deleted_over_warm": if warm_ms > 0.0 { cold_ms / warm_ms } else { f64::NAN },
            "tail_depth_samples": {
                "quarter_size": quarter,
                "warm_first_quarter_mean_ms": warm_first_quarter,
                "warm_last_quarter_mean_ms": warm_last_quarter,
                "cache_deleted_first_quarter_mean_ms": cold_first_quarter,
                "cache_deleted_last_quarter_mean_ms": cold_last_quarter,
                "slowdown_ratio_first_quarter": cold_first_quarter / warm_first_quarter,
                "slowdown_ratio_last_quarter": cold_last_quarter / warm_last_quarter,
            },
        },
        "correctness_unchanged_without_cache_or_coordinator": bypass_unique
            && contiguous_from_one(&bypass_seqs)
            && bypass.accepted == BYPASS_WRITERS,
        "row_lock_negative_control": {
            "note": "test-local reproduction of the seq allocation, run twice against a real \
                     collab_documents row; production code is not modified. It answers only \
                     'is the row lock what makes allocation unique'.",
            "writers": BYPASS_WRITERS,
            "with_for_update_allocations": with_lock,
            "with_for_update_unique": with_lock_unique,
            "without_for_update_allocations": without_lock,
            "without_for_update_unique": without_lock_unique,
            "row_lock_is_the_authority": with_lock_unique && !without_lock_unique,
        },
    })
}

fn distinct(values: &[i64]) -> usize {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted.len()
}

/// `BYPASS_WRITERS` connections each allocate "the next seq" for one document at the same time,
/// with or without the row lock, and report what each one allocated.
async fn allocate_concurrently(scratch_url: &str, document_id: Uuid, for_update: bool) -> Vec<i64> {
    let barrier = Arc::new(tokio::sync::Barrier::new(BYPASS_WRITERS));
    let mut handles = Vec::new();
    for _ in 0..BYPASS_WRITERS {
        let db = Database::connect(scratch_url).await.expect("control writer connects");
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            let tx = db.begin().await.expect("control transaction opens");
            barrier.wait().await;
            let sql = if for_update {
                "SELECT head_seq FROM collab_documents WHERE id = $1 FOR UPDATE"
            } else {
                "SELECT head_seq FROM collab_documents WHERE id = $1"
            };
            let row = tx
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    sql,
                    vec![document_id.into()],
                ))
                .await
                .expect("control read runs")
                .expect("control read returns a row");
            let head: i64 = row.try_get("", "head_seq").expect("head_seq reads");
            let allocated = head + 1;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE collab_documents SET head_seq = $2 WHERE id = $1",
                vec![document_id.into(), allocated.into()],
            ))
            .await
            .expect("control write runs");
            tx.commit().await.expect("control transaction commits");
            allocated
        }));
    }
    let mut allocated = Vec::new();
    for handle in handles {
        allocated.push(handle.await.expect("control task joins"));
    }
    allocated.sort_unstable();
    allocated
}

// ---------------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------------

async fn postgres_version(db: &DatabaseConnection) -> String {
    #[derive(FromQueryResult)]
    struct VersionRow {
        version: String,
    }
    VersionRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT version() AS version",
        vec![],
    ))
    .one(db)
    .await
    .ok()
    .flatten()
    .map_or_else(|| "unknown".to_string(), |row| row.version)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn warm_cache_boundaries_rebuild_bypass_and_retry_exhaustion_evidence() {
    let Some(scratch) = scratch("evidence").await else {
        eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
        return;
    };
    require_isolated_apply_worker();
    let mutant = Mutant::from_env();
    let mut violations: Vec<String> = Vec::new();
    let started = Instant::now();

    let db = scratch.db.clone();
    let state = state_for(db.clone());
    let (workspace_id, actor_id) = seed_workspace(&db).await;
    let epoch = authz::read_epoch(&db, workspace_id).await.expect("epoch reads");

    // Section 1/2: pure cache-boundary claims, no database involvement at all.
    let boundaries = section_cache_boundaries(mutant, &mut violations);
    let observers = section_observers_and_timers(&mut violations);

    // Section 3: post-eviction rebuild parity, on its own document.
    let (_, rebuild_doc) = create_page(&state, workspace_id, actor_id, "Item3 Rebuild").await;
    let rebuild =
        section_rebuild_after_eviction(&db, workspace_id, actor_id, rebuild_doc, epoch, mutant, &mut violations).await;

    // Section 4: hit/miss/eviction/restart, on its own document.
    let (_, ledger_doc) = create_page(&state, workspace_id, actor_id, "Item2 Cache Conditions").await;
    let no_lost = section_no_accepted_update_lost(
        &db,
        &scratch.url,
        workspace_id,
        actor_id,
        ledger_doc,
        epoch,
        &mut violations,
    )
    .await;

    // Section 5: bounded-retry exhaustion, on its own document.
    let (_, exhaustion_doc) = create_page(&state, workspace_id, actor_id, "Item2 Exhaustion").await;
    let exhaustion = section_retry_exhaustion(
        &db,
        &scratch.url,
        workspace_id,
        actor_id,
        exhaustion_doc,
        epoch,
        &mut violations,
    )
    .await;

    // Section 6: item 5.
    let bypass = section_bypass_and_removal(
        &db,
        &scratch.url,
        &state,
        workspace_id,
        actor_id,
        epoch,
        mutant,
        &mut violations,
    )
    .await;

    // A mutant run is *supposed* to be red; a mutant run that stays green means the assertion it
    // broke was not load-bearing, which is the one thing worse than a red gate.
    let mutant_detected = !violations.is_empty();
    if mutant != Mutant::None && !mutant_detected {
        violations.push(format!(
            "anti-proof: mutant {} did not turn any assertion red -- the assertion it targets is not \
             load-bearing",
            mutant.name()
        ));
    }

    let (head, dirty) = source_head();
    let result = json!({
        "schema_version": "sylvode.flow.collab-cache-evidence.v1",
        "source_head": head,
        "source_tree_dirty": dirty,
        "environment": {
            "build_profile": build_profile(),
            "postgres_version": postgres_version(&db).await,
            "pg_container": std::env::var(PG_CONTAINER_ENV).unwrap_or_else(|_| DEFAULT_PG_CONTAINER.to_string()),
            "scratch_database": scratch.name,
            "mutant": mutant.name(),
            "measurement_authority":
                "WarmCache/DocumentCoordinator public API for boundaries; PostgreSQL canonical rows \
                 (collab_documents, collab_updates, business_events, event_dispatch) for every \
                 'nothing was lost' claim; flow::collab::bootstrap::load for every rebuild",
            "wall_seconds": started.elapsed().as_secs_f64(),
        },
        "adr_clauses_covered": {
            "item_2_cache_hit_miss_eviction_restart_no_accepted_update_lost": "no_accepted_update_lost",
            "item_2_three_rebase_exhaustion_no_accepted_update_lost": "retry_exhaustion",
            "item_3_warm_cache_exact_boundary_reclamation": "cache_boundaries",
            "item_3_post_eviction_rebuild_semantic_hash_and_head": "rebuild_after_eviction",
            "item_3_observer_and_timer_delta_zero": "observers_and_timers",
            "item_5_bypass_cache_still_unique_seq_via_db_lock": "bypass_and_removal",
            "item_5_deleting_cache_and_coordinator_degrades_performance_only": "bypass_and_removal",
        },
        "cache_boundaries": boundaries,
        "observers_and_timers": observers,
        "rebuild_after_eviction": rebuild,
        "no_accepted_update_lost": no_lost,
        "retry_exhaustion": exhaustion,
        "bypass_and_removal": bypass,
        "violations": violations,
        "passed": violations.is_empty(),
    });

    let rendered = serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
    println!("---- flow collab cache evidence ----\n{rendered}");
    if let Ok(path) = std::env::var(EVIDENCE_OUT_ENV) {
        let _ = std::fs::write(path, &rendered);
    }

    let passed = violations.is_empty();
    scratch.drop_self().await;
    assert!(
        passed,
        "collab cache evidence run has {} violation(s)",
        violations.len()
    );
}
