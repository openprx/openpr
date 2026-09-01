//! Sylvode Flow v0.4 — **degradation magnitude** harness for `ADR-0010` item 5's second half.
//!
//! `ADR-0010`'s 2026-08-30 second review left three named gaps. This file addresses the third:
//!
//! > 第 5 项「只退化性能」**只有方向没有量级** —— 顺序写 slowdown 两次运行 3.56x / 4.66x、
//! > 尾段 1.51x / 4.34x，方差过大 […] 需要定义可接受的退化量级并稳定复现。
//!
//! `flow_collab_cache_evidence.rs`'s `sequential_degradation` block answers *whether* deleting the
//! warm cache costs anything (it does) but reports the cost as **one arithmetic mean per arm over
//! 40 writes**. A mean over 40 samples of a latency distribution with a heavy right tail is not a
//! reproducible quantity, which is exactly what the review observed: the published run has
//! `cache_deleted_mean_ms = 16.57` while *both* its own quarter means are 4.87 and 7.41 ms — the
//! mean is dominated by a handful of outliers that live in neither quarter.
//!
//! # What this harness measures differently
//!
//! 1. **Order statistics, not means.** Every write is sampled individually and reported as
//!    p50/p75/p90/p95/max per arm, so a magnitude claim can be made about a statistic that is not
//!    hostage to one 300 ms outlier.
//! 2. **Repetitions, not a single run.** The whole warm-vs-deleted comparison is repeated
//!    `OPENPR_FLOW_DEGRADATION_REPS` times (default 5) on freshly created documents, and the
//!    run-to-run spread of every ratio is reported as a coefficient of variation. A magnitude that
//!    cannot be reproduced across repetitions is reported as *not freezable* rather than averaged
//!    into looking stable.
//! 3. **Tail depth as the independent variable.** The deleted-cache arm replays the whole tail on
//!    every write, so its cost is a function of how deep the tail already is. A single scalar
//!    "slowdown" is therefore the wrong *shape* for this quantity regardless of how many samples
//!    back it. Samples are bucketed by tail depth and a least-squares slope (ms per additional
//!    tail update) is fitted per arm, so the honest claim — "warm is flat in tail depth, deleted
//!    grows linearly at S ms/update" — can be stated and checked.
//! 4. **Interleaved arms.** Warm write *i* and deleted write *i* are issued back to back on two
//!    independent documents, so any drift in the database or the host is charged to both arms.
//!
//! # What this harness deliberately does not do
//!
//! It does not propose a threshold. It reports whether the measured quantity is stable enough that
//! a threshold *could* be frozen, and at which statistic. The ADR is explicit that a fabricated
//! number is worse than a `Proposed` status.
//!
//! # Honest failure
//!
//! Correctness is still asserted on both arms (all writes accepted, `seq` contiguous from 1) and
//! any breach lands in `violations`. Timing results are never asserted against a budget: this file
//! measures a magnitude, it does not gate on one.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::too_many_lines,
    clippy::similar_names
)]

use std::time::Instant;

use collab_core::{CollabEngine, LoroCollabEngine};
use platform::{
    app::AppState,
    config::{AppConfig, Secret},
};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use api::flow::collab::cache::WarmCache;
use api::flow::collab::coordinator::DocumentCoordinator;
use api::flow::collab::registry::SessionRegistry;
use api::flow::collab::snapshot::SnapshotAdvancer;
use api::flow::collab::write::{AcceptOutcome, UpdateRequest, accept_update};
use api::flow::collab::{authz, bootstrap};
use api::flow::event_origin::{CommandOrigin, EventSurface};

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";
const OUT_ENV: &str = "OPENPR_FLOW_DEGRADATION_OUT";
const REPS_ENV: &str = "OPENPR_FLOW_DEGRADATION_REPS";
const WRITES_ENV: &str = "OPENPR_FLOW_DEGRADATION_WRITES";

/// Repetitions of the whole warm-vs-deleted comparison, each on fresh documents.
const DEFAULT_REPS: usize = 5;
/// Sequential writes per arm per repetition.
const DEFAULT_WRITES: usize = 100;
/// Tail-depth buckets the per-write samples are grouped into.
const DEPTH_BUCKETS: usize = 5;

// ---------------------------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------------------------

/// See `flow_collab_cache_evidence.rs`: the uplifted worker binary does not survive every cargo
/// invocation against a shared target directory, and without it every `accept_update` below fails
/// as `ApiError::Internal` — a red run whose stated cause would be wrong.
fn require_isolated_apply_worker() {
    let exe = std::env::current_exe().expect("test executable path resolves");
    let candidate = exe
        .parent()
        .and_then(std::path::Path::parent)
        .map(|dir| dir.join("collab-isolated-apply-worker"))
        .expect("test executable has a grandparent directory");
    assert!(
        candidate.is_file(),
        "collab-isolated-apply-worker is not at {}. Every accept_update below would fail as \
         ApiError::Internal and this run would report the wrong cause.",
        candidate.display()
    );
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

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
// Scratch database
// ---------------------------------------------------------------------------------------------

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

    let name = format!("openpr_flow_degradation_{label}");
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

// ---------------------------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------------------------

fn state_for(db: DatabaseConnection) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "collab-degradation-magnitude".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new("collab-degradation-secret"),
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
         VALUES ($1, $2, '!', 'degradation magnitude', 'user', true)",
        vec![owner_id.into(), format!("{owner_id}@degradation.test").into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'degradation magnitude', $3)",
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

async fn create_page(state: &AppState, workspace_id: Uuid, actor_id: Uuid, title: &str) -> Uuid {
    use api::flow::command::{CreateObjectInput, create_object};
    let accepted = create_object(
        state,
        CreateObjectInput {
            origin: CommandOrigin::first_request_from(EventSurface::Rest),
            workspace_id,
            actor_id,
            actor_is_bot: false,
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
    accepted.object.document_id
}

#[derive(Debug, Clone, FromQueryResult)]
struct DocRow {
    snapshot: Vec<u8>,
}

async fn document_snapshot(db: &DatabaseConnection, document_id: Uuid) -> Vec<u8> {
    DocRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT snapshot FROM collab_documents WHERE id = $1",
        vec![document_id.into()],
    ))
    .one(db)
    .await
    .expect("document query runs")
    .expect("document row exists")
    .snapshot
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

/// One simulated API instance: `ADR-0010` makes all four pieces instance-local.
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
            origin: CommandOrigin::first_request_from(EventSurface::Rest),
            document_id,
            update_id: Uuid::new_v4(),
            bytes,
            idempotency_key: None,
            event_idempotency_key: None,
            origin_client_id: Some("degradation-magnitude".to_string()),
            message: None,
            actor_id,
            actor_is_bot: false,
            workspace_id,
            checked_epoch,
            expected_frontier: None,
        },
    )
    .await
    .expect("accept_update does not hit a hard database error")
}

// ---------------------------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------------------------

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = (quantile * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        f64::NAN
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

/// Coefficient of variation: sample standard deviation over the mean. The one number that says
/// whether a magnitude reproduces across runs.
fn coefficient_of_variation(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return f64::NAN;
    }
    let avg = mean(values);
    if avg == 0.0 {
        return f64::NAN;
    }
    let variance = values.iter().map(|value| (value - avg).powi(2)).sum::<f64>() / (values.len() as f64 - 1.0);
    variance.sqrt() / avg
}

#[derive(Debug, Clone)]
struct Summary {
    p50: f64,
    p75: f64,
    p90: f64,
    p95: f64,
    max: f64,
    mean: f64,
}

fn summarize(samples: &[f64]) -> Summary {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    Summary {
        p50: percentile(&sorted, 0.50),
        p75: percentile(&sorted, 0.75),
        p90: percentile(&sorted, 0.90),
        p95: percentile(&sorted, 0.95),
        max: sorted.last().copied().unwrap_or(f64::NAN),
        mean: mean(&sorted),
    }
}

fn summary_json(summary: &Summary) -> Value {
    json!({
        "p50_ms": summary.p50,
        "p75_ms": summary.p75,
        "p90_ms": summary.p90,
        "p95_ms": summary.p95,
        "max_ms": summary.max,
        "mean_ms": summary.mean,
    })
}

/// Least-squares slope of `samples` against write index, i.e. milliseconds of extra latency per
/// additional tail update. This is the number that decides whether "slowdown" is a constant
/// multiple at all.
fn slope_per_write(samples: &[f64]) -> f64 {
    let n = samples.len() as f64;
    if n < 2.0 {
        return f64::NAN;
    }
    let mean_x = (n - 1.0) / 2.0;
    let mean_y = mean(samples);
    let mut numerator = 0.0;
    let mut denominator = 0.0;
    for (index, value) in samples.iter().enumerate() {
        let dx = index as f64 - mean_x;
        numerator = dx.mul_add(value - mean_y, numerator);
        denominator = dx.mul_add(dx, denominator);
    }
    if denominator == 0.0 {
        f64::NAN
    } else {
        numerator / denominator
    }
}

// ---------------------------------------------------------------------------------------------
// One repetition
// ---------------------------------------------------------------------------------------------

struct Repetition {
    warm: Vec<f64>,
    deleted: Vec<f64>,
}

#[allow(clippy::too_many_arguments)]
async fn one_repetition(
    db: &DatabaseConnection,
    state: &AppState,
    workspace_id: Uuid,
    actor_id: Uuid,
    epoch: i64,
    writes: usize,
    index: usize,
    violations: &mut Vec<String>,
) -> Repetition {
    let warm_doc = create_page(state, workspace_id, actor_id, &format!("Degradation Warm {index}")).await;
    let deleted_doc = create_page(state, workspace_id, actor_id, &format!("Degradation Deleted {index}")).await;

    // The warm arm keeps one instance for the whole arm: its entry stays resident and every write
    // after the first prepares incrementally. The deleted arm builds a brand new instance per
    // write, which is exactly "cache and coordinator removed" — nothing can ever hit.
    let warm_instance = Instance::new();
    let mut warm_samples = Vec::with_capacity(writes);
    let mut deleted_samples = Vec::with_capacity(writes);
    let mut warm_accepted = 0usize;
    let mut deleted_accepted = 0usize;

    for step in 0..writes {
        let base = document_snapshot(db, warm_doc).await;
        let bytes = build_update(&base, &format!("warm-{index}-{step}"));
        let started = Instant::now();
        let outcome = write_once(db, &warm_instance, warm_doc, workspace_id, actor_id, epoch, bytes).await;
        warm_samples.push(started.elapsed().as_secs_f64() * 1000.0);
        if matches!(outcome, AcceptOutcome::Accepted(_)) {
            warm_accepted += 1;
        }

        let base = document_snapshot(db, deleted_doc).await;
        let bytes = build_update(&base, &format!("deleted-{index}-{step}"));
        let deleted_instance = Instance::new();
        let started = Instant::now();
        let outcome = write_once(db, &deleted_instance, deleted_doc, workspace_id, actor_id, epoch, bytes).await;
        deleted_samples.push(started.elapsed().as_secs_f64() * 1000.0);
        if matches!(outcome, AcceptOutcome::Accepted(_)) {
            deleted_accepted += 1;
        }
    }

    for (label, accepted, doc) in [
        ("warm", warm_accepted, warm_doc),
        ("deleted", deleted_accepted, deleted_doc),
    ] {
        let seqs = all_seqs(db, doc).await;
        if accepted != writes || seqs.len() != writes || !contiguous_from_one(&seqs) {
            violations.push(format!(
                "rep {index} [{label}]: {accepted} of {writes} accepted, {} canonical rows, contiguous={}",
                seqs.len(),
                contiguous_from_one(&seqs)
            ));
        }
        // The deleted arm must still rebuild to the same head through the consistency loader:
        // "只退化性能" is a claim about correctness as much as about latency.
        let boot = bootstrap::load(db, doc).await.expect("bootstrap loader runs");
        if boot.head_seq != seqs.len() as i64 {
            violations.push(format!(
                "rep {index} [{label}]: bootstrap head_seq {} != {} canonical rows",
                boot.head_seq,
                seqs.len()
            ));
        }
    }

    Repetition {
        warm: warm_samples,
        deleted: deleted_samples,
    }
}

// ---------------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn cache_removal_degradation_magnitude_evidence() {
    let Some(scratch) = scratch("magnitude").await else {
        eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
        return;
    };
    require_isolated_apply_worker();
    let started = Instant::now();
    let reps = env_usize(REPS_ENV, DEFAULT_REPS);
    let writes = env_usize(WRITES_ENV, DEFAULT_WRITES);
    let mut violations: Vec<String> = Vec::new();

    let db = scratch.db.clone();
    let state = state_for(db.clone());
    let (workspace_id, actor_id) = seed_workspace(&db).await;
    let epoch = authz::read_epoch(&db, workspace_id).await.expect("epoch reads");

    let mut repetitions = Vec::with_capacity(reps);
    for index in 0..reps {
        repetitions.push(
            one_repetition(
                &db,
                &state,
                workspace_id,
                actor_id,
                epoch,
                writes,
                index,
                &mut violations,
            )
            .await,
        );
    }

    // ---- Per repetition, every statistic that could plausibly carry a frozen magnitude.
    let mut per_rep = Vec::with_capacity(reps);
    let mut ratio_p50: Vec<f64> = Vec::with_capacity(reps);
    let mut ratio_p95: Vec<f64> = Vec::with_capacity(reps);
    let mut ratio_mean: Vec<f64> = Vec::with_capacity(reps);
    let mut warm_slopes: Vec<f64> = Vec::with_capacity(reps);
    let mut deleted_slopes: Vec<f64> = Vec::with_capacity(reps);

    for (index, rep) in repetitions.iter().enumerate() {
        let warm = summarize(&rep.warm);
        let deleted = summarize(&rep.deleted);
        let r_p50 = deleted.p50 / warm.p50;
        let r_p95 = deleted.p95 / warm.p95;
        let r_mean = deleted.mean / warm.mean;
        ratio_p50.push(r_p50);
        ratio_p95.push(r_p95);
        ratio_mean.push(r_mean);

        let warm_slope = slope_per_write(&rep.warm);
        let deleted_slope = slope_per_write(&rep.deleted);
        warm_slopes.push(warm_slope);
        deleted_slopes.push(deleted_slope);

        // Tail-depth buckets: the deleted arm replays the whole tail, so its cost should climb
        // bucket over bucket while the warm arm stays flat.
        let bucket_size = writes / DEPTH_BUCKETS;
        let mut buckets = Vec::with_capacity(DEPTH_BUCKETS);
        if bucket_size > 0 {
            for bucket in 0..DEPTH_BUCKETS {
                let lo = bucket * bucket_size;
                let hi = if bucket + 1 == DEPTH_BUCKETS {
                    writes
                } else {
                    (bucket + 1) * bucket_size
                };
                let warm_bucket = summarize(&rep.warm[lo..hi]);
                let deleted_bucket = summarize(&rep.deleted[lo..hi]);
                buckets.push(json!({
                    "tail_depth_range": [lo, hi],
                    "warm_p50_ms": warm_bucket.p50,
                    "deleted_p50_ms": deleted_bucket.p50,
                    "ratio_p50": deleted_bucket.p50 / warm_bucket.p50,
                }));
            }
        }

        per_rep.push(json!({
            "repetition": index,
            "writes_per_arm": writes,
            "warm": summary_json(&warm),
            "cache_and_coordinator_deleted": summary_json(&deleted),
            "ratio_p50": r_p50,
            "ratio_p95": r_p95,
            "ratio_mean": r_mean,
            "warm_slope_ms_per_tail_update": warm_slope,
            "deleted_slope_ms_per_tail_update": deleted_slope,
            "by_tail_depth": buckets,
        }));
    }

    // ---- Pooled view: every sample from every repetition, which is the largest defensible n.
    let pooled_warm: Vec<f64> = repetitions.iter().flat_map(|rep| rep.warm.clone()).collect();
    let pooled_deleted: Vec<f64> = repetitions.iter().flat_map(|rep| rep.deleted.clone()).collect();
    let pooled_warm_summary = summarize(&pooled_warm);
    let pooled_deleted_summary = summarize(&pooled_deleted);

    let cv_p50 = coefficient_of_variation(&ratio_p50);
    let cv_p95 = coefficient_of_variation(&ratio_p95);
    let cv_mean = coefficient_of_variation(&ratio_mean);
    let cv_deleted_slope = coefficient_of_variation(&deleted_slopes);

    let spread = |values: &[f64]| -> Value {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        json!({
            "samples": values,
            "min": sorted.first().copied().unwrap_or(f64::NAN),
            "max": sorted.last().copied().unwrap_or(f64::NAN),
            "mean": mean(values),
            "coefficient_of_variation": coefficient_of_variation(values),
        })
    };

    let artifact = json!({
        "schema_version": "sylvode.flow.collab-degradation-magnitude.v1",
        "source_head": source_head().0,
        "source_tree_dirty": source_head().1,
        "environment": {
            "build_profile": build_profile(),
            "postgres_version": postgres_version(&db).await,
            "scratch_database": scratch.name,
            "repetitions": reps,
            "writes_per_arm_per_repetition": writes,
            "total_samples_per_arm": reps * writes,
            "wall_seconds": started.elapsed().as_secs_f64(),
        },
        "shape": "two independent documents per repetition; warm arm keeps one WarmCache + \
                  DocumentCoordinator for the whole arm, deleted arm constructs a fresh empty \
                  instance for every single write. Arms are interleaved write-by-write so host \
                  drift is charged to both.",
        "per_repetition": per_rep,
        "pooled": {
            "warm": summary_json(&pooled_warm_summary),
            "cache_and_coordinator_deleted": summary_json(&pooled_deleted_summary),
            "ratio_p50": pooled_deleted_summary.p50 / pooled_warm_summary.p50,
            "ratio_p95": pooled_deleted_summary.p95 / pooled_warm_summary.p95,
            "ratio_mean": pooled_deleted_summary.mean / pooled_warm_summary.mean,
        },
        "reproducibility": {
            "note": "coefficient of variation of each ratio across independent repetitions. A \
                     magnitude can only be frozen on a statistic whose CV is small; the ADR's \
                     complaint was recorded against `ratio_mean`.",
            "ratio_p50": spread(&ratio_p50),
            "ratio_p95": spread(&ratio_p95),
            "ratio_mean": spread(&ratio_mean),
            "warm_slope_ms_per_tail_update": spread(&warm_slopes),
            "deleted_slope_ms_per_tail_update": spread(&deleted_slopes),
            "cv_ratio_p50": cv_p50,
            "cv_ratio_p95": cv_p95,
            "cv_ratio_mean": cv_mean,
            "cv_deleted_slope": cv_deleted_slope,
        },
        "violations": violations,
        "passed": violations.is_empty(),
    });

    let rendered = serde_json::to_string_pretty(&artifact).expect("artifact serializes");
    println!("{rendered}");
    if let Ok(path) = std::env::var(OUT_ENV) {
        let temp = format!("{path}.tmp");
        std::fs::write(&temp, format!("{rendered}\n")).expect("artifact temp file writes");
        std::fs::rename(&temp, &path).expect("artifact renames into place");
    }

    scratch.drop_self().await;
    assert!(
        violations.is_empty(),
        "degradation harness recorded violations: {violations:#?}"
    );
}

#[derive(FromQueryResult)]
struct VersionRow {
    version: String,
}

async fn postgres_version(db: &DatabaseConnection) -> String {
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
