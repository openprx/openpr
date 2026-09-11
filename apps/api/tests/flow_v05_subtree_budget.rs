//! `move_subtree_nodes_max` cost-curve measurement (v0.5 WP-07c).
//!
//! `limits-v1.md`'s `move_subtree_nodes_max` is `status: unset`, and the subtree cascade
//! (`ADR-0013` §2.2 R17 step two) is blocked on it. This binary measures the cost curve the value
//! has to be derived from, instead of picking a number.
//!
//! # What a cascade costs, and where
//!
//! A cascading `move_object` over a subtree of `N` nodes has to, for one command:
//!
//! * rewrite `N` navigator ordering entries — `N` `DeleteNode` in the source navigator's CRDT
//!   document and `N` `CreateNode` in the target navigator's (`flow::move_object`'s
//!   `build_navigator_update`, generalised from one entry to `N`); and
//! * rewrite `N` rows of `flow_objects.project_id`, under the composite FK migration `0056` added.
//!
//! `ADR-0010` puts the CRDT apply strictly **outside** the lock and only the fixed persistence
//! statements inside it, so this harness measures the two sides separately and reports the split
//! rather than one blended number:
//!
//! * `crdt_*` — prepare-phase, out of lock. Its output that matters to the contract is the
//!   **exported update byte count**, which is what `update_bytes_max` (64 KiB) bounds.
//! * `hold_ms` — the locked phase: the epoch `FOR UPDATE`, the two `collab_documents` row locks,
//!   the two staged document writes (whose `flow_object_projections.state` JSON is itself `O(N)`,
//!   because `flow::projection::state_json` serialises the whole `SemanticSnapshot`), the `N`-row
//!   `flow_objects` lock and update, the events and the epoch advance, through `COMMIT`. This is
//!   what `document_lock_hold_ms_p95_max` (25 ms) and `document_lock_hold_ms_max` (100 ms) bound.
//!
//! # Why this is a separate test binary
//!
//! Measured in this repository: the identical workload run as an in-crate `#[test]` alongside the
//! crate's other library tests reported `p95 = 419 ms`; the same workload as a `tests/*.rs` binary
//! reported `83.6 ms`. Process-level co-residency dominates; a dedicated `PostgreSQL` instance is
//! worth about 10 ms on top. So: separate binary, mandatory; dedicated instance, declared and
//! checked.
//!
//! # The environment gate, and why `cargo`'s "1 passed" is not the authority
//!
//! With no declared dedicated instance this binary emits an evidence document whose `passed` is
//! `false` and returns — `cargo test` still prints `1 passed`. The authority is the evidence
//! document's `passed` field, exactly as in `flow_collab_load_harness.rs`.
//!
//! ```text
//! OPENPR_TEST_DATABASE_URL=postgres://flowtest:flowtest@127.0.0.1:25434/openpr \
//! OPENPR_FLOW_DEDICATED_PG_CONTAINER=flow-load-pg \
//! OPENPR_FLOW_SUBTREE_BUDGET_OUT=/tmp/subtree-budget.json \
//!   cargo test -p api --all-features --test flow_v05_subtree_budget \
//!     move_subtree_nodes_max_cost_curve -- --exact --ignored --nocapture
//! ```

#![allow(
    // Test target, same set `flow_collab_load_harness.rs` declares: `clippy.toml` already allows
    // unwrap/expect/panic in tests; `indexing_slicing` and the print lints are not covered by that
    // config and are needed to report a measured distribution at all.
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::print_stderr,
    // Percentiles over sample counts and millisecond arithmetic.
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]

use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use collab_core::{CollabEngine, LoroCollabEngine, NodeId, NodeKind, Operation};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use api::events::{BusinessEventInput, FlowDispatchSpec, insert_flow_event};
use api::flow::projection;

// ---------------------------------------------------------------------------------------------
// Frozen targets — every number transcribed from `contracts/limits-v1.md`, none invented here.
// ---------------------------------------------------------------------------------------------

/// `document_lock_hold_ms_p95_max`.
const LOCK_HOLD_P95_MS_MAX: f64 = 25.0;
/// `document_lock_hold_ms_max`.
const LOCK_HOLD_SINGLE_MS_MAX: f64 = 100.0;
/// `document_lock_hold_ms_max_per_extra_document`.
const LOCK_HOLD_PER_EXTRA_DOCUMENT_MS: f64 = 100.0;
/// This harness always contends two navigator documents.
const LOCK_HOLD_TWO_DOCUMENT_MS_MAX: f64 = LOCK_HOLD_SINGLE_MS_MAX + LOCK_HOLD_PER_EXTRA_DOCUMENT_MS;
/// `update_bytes_max`.
const UPDATE_BYTES_MAX: usize = 65_536;
/// `websocket_frame_bytes_max` — the outbound broadcast of a navigator update has to fit a frame
/// after base64 (4/3) plus the JSON envelope.
const WEBSOCKET_FRAME_BYTES_MAX: usize = 131_072;
/// `container_count_max` — live `NavigatorNode`s per document.
const CONTAINER_COUNT_MAX: usize = 10_000;
/// `snapshot_tail_bytes_soft_max`.
const SNAPSHOT_TAIL_BYTES_SOFT_MAX: usize = 1_048_576;

/// `ADR-0010`'s official-run protocol: 5 warmups, at least 30 measured samples.
const WARMUP_ROUNDS: usize = 5;
const MIN_SAMPLES: usize = 30;

/// The subtree sizes measured. Anything that cannot be run is reported as such, never skipped
/// silently.
const LADDER: &[usize] = &[1, 10, 100, 1_000, 5_000, 10_000];

/// A comma-separated override for [`LADDER`], so the same binary can bisect a region of the curve
/// without a source edit changing what was measured.
const LADDER_OVERRIDE_ENV: &str = "OPENPR_FLOW_SUBTREE_LADDER";

fn ladder() -> Vec<usize> {
    match std::env::var(LADDER_OVERRIDE_ENV) {
        Ok(raw) if !raw.trim().is_empty() => raw
            .split(',')
            .filter_map(|part| part.trim().parse::<usize>().ok())
            .filter(|n| *n > 0)
            .collect(),
        _ => LADDER.to_vec(),
    }
}

/// Above this `N` the literal per-node `build_navigator_update` shape (one `semantic_snapshot()`
/// plus one `check_operation` per node, both `O(nodes)`) is `O(N^2)` and takes minutes. It is
/// measured up to here and reported as not-run above, rather than omitted.
const NAIVE_CRDT_MAX_N: usize = 1_000;

/// The anti-proof: a delay injected inside the locked phase. The instrument has to see it.
const INJECTED_DELAY_MS: u64 = 40;

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";
const DEDICATED_PG_CONTAINER_ENV: &str = "OPENPR_FLOW_DEDICATED_PG_CONTAINER";
const EVIDENCE_OUT_ENV: &str = "OPENPR_FLOW_SUBTREE_BUDGET_OUT";

/// Set to `1` to add the index migration `0056` does not create, before measuring.
///
/// The referenced side of `flow_objects_parent_project_fk` is checked, once per updated row, by
/// `SELECT 1 FROM ONLY flow_objects x WHERE $1 = x.parent_id AND $2 = x.project_scope_id FOR KEY
/// SHARE`. No index leads with `parent_id, project_scope_id`, so the planner falls back to a full
/// scan of `flow_objects_project_scope_key` filtering on the non-leading column — measured on the
/// `N = 5000` fixture: `Index Scan ... Index Cond: (project_scope_id = ...)`,
/// `Filter: (... = parent_id)`, 131 buffers, 0.531 ms **per probe**. `N` probes over an index that
/// itself grows with `N` makes the cascade `O(N^2)`.
///
/// This flag exists so the frozen value is not derived against a fixable schema defect: the same
/// ladder is measured with and without the index, and the report says which curve the value comes
/// from.
const ADD_FK_INDEX_ENV: &str = "OPENPR_FLOW_SUBTREE_ADD_FK_INDEX";

fn add_fk_index_requested() -> bool {
    std::env::var(ADD_FK_INDEX_ENV).is_ok_and(|value| value == "1")
}

// ---------------------------------------------------------------------------------------------
// Environment gate
// ---------------------------------------------------------------------------------------------

fn environment_problem() -> Option<(&'static str, String)> {
    let dedicated = std::env::var(DEDICATED_PG_CONTAINER_ENV).unwrap_or_default();
    if dedicated.is_empty() {
        return Some((
            "dedicated_container_not_declared",
            format!("{DEDICATED_PG_CONTAINER_ENV} is required; the cost curve was not measured"),
        ));
    }
    let normalized = dedicated.to_ascii_lowercase();
    if normalized == "flow-test-pg" || normalized.contains("shared") {
        return Some((
            "known_shared_postgresql_instance",
            format!("PostgreSQL container {dedicated:?} is shared; the cost curve was not measured"),
        ));
    }
    if std::env::var(TEST_DATABASE_URL_ENV).unwrap_or_default().is_empty() {
        return Some((
            "database_url_not_declared",
            format!("{TEST_DATABASE_URL_ENV} is required"),
        ));
    }
    None
}

/// The host's 1/5/15-minute load average, recorded at the start and the end of the run.
///
/// This host builds other packages while this harness runs, and a loaded host inflates every
/// number here. Recording it is not a disclaimer — it is the field that lets a reader decide
/// whether two runs are comparable, and it is why the official run is the one taken at low load.
fn load_average() -> Value {
    std::fs::read_to_string("/proc/loadavg").map_or(Value::Null, |raw| {
        let parts: Vec<&str> = raw.split_whitespace().take(3).collect();
        json!({
            "one_minute": parts.first().and_then(|v| v.parse::<f64>().ok()),
            "five_minute": parts.get(1).and_then(|v| v.parse::<f64>().ok()),
            "fifteen_minute": parts.get(2).and_then(|v| v.parse::<f64>().ok()),
        })
    })
}

fn emit(value: &Value) {
    let rendered = serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string());
    println!("---- flow v0.5 subtree budget measurement ----\n{rendered}");
    if let Ok(path) = std::env::var(EVIDENCE_OUT_ENV) {
        let _ = std::fs::write(path, rendered);
    }
}

fn emit_not_satisfied(reason_code: &str, detail: &str) {
    emit(&json!({
        "schema_version": "sylvode.flow.subtree-budget-measurement.v1",
        "generated_at": Utc::now().to_rfc3339(),
        "environment_gate": {
            "status": "not_satisfied",
            "reason_code": reason_code,
            "detail": detail,
            "declared_dedicated_pg_container": std::env::var(DEDICATED_PG_CONTAINER_ENV).ok(),
            "known_shared_instances_rejected": ["flow-test-pg"],
        },
        "execution": { "status": "not_run_environment_not_satisfied", "harness_started": false },
        "violations": [detail],
        "passed": false,
    }));
}

// ---------------------------------------------------------------------------------------------
// Scratch database (same shape as `flow_collab_load_harness.rs`)
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
    let name = format!("openpr_flow_subtree_{label}");
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

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
}

// ---------------------------------------------------------------------------------------------
// Percentiles
// ---------------------------------------------------------------------------------------------

fn percentile_ms(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

fn sorted(values: &[f64]) -> Vec<f64> {
    let mut out = values.to_vec();
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out
}

#[derive(Clone, Debug)]
struct Distribution {
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
    samples: usize,
}

impl Distribution {
    fn of(values: &[f64]) -> Self {
        let values = sorted(values);
        Self {
            p50_ms: percentile_ms(&values, 50.0),
            p95_ms: percentile_ms(&values, 95.0),
            p99_ms: percentile_ms(&values, 99.0),
            max_ms: values.last().copied().unwrap_or(0.0),
            samples: values.len(),
        }
    }
    fn to_json(&self) -> Value {
        json!({
            "p50_ms": (self.p50_ms * 1000.0).round() / 1000.0,
            "p95_ms": (self.p95_ms * 1000.0).round() / 1000.0,
            "p99_ms": (self.p99_ms * 1000.0).round() / 1000.0,
            "max_ms": (self.max_ms * 1000.0).round() / 1000.0,
            "samples": self.samples,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// CRDT side — pure, in process, no database. This is the prepare phase, out of lock.
// ---------------------------------------------------------------------------------------------

fn entry_id(object_id: Uuid, generation: u32) -> NodeId {
    if generation == 0 {
        Arc::from(object_id.to_string().as_str())
    } else {
        Arc::from(format!("{object_id}#{generation}").as_str())
    }
}

/// A navigator document holding `n` live root ordering entries, one per subtree object.
fn navigator_with_entries(objects: &[Uuid]) -> LoroCollabEngine {
    let mut engine = LoroCollabEngine::new_empty(7);
    for (index, object_id) in objects.iter().enumerate() {
        engine
            .apply_operation(&Operation::CreateNode {
                id: entry_id(*object_id, 0),
                parent: None,
                index: index as u32,
                kind: NodeKind::NavigatorNode,
            })
            .expect("seeding a navigator ordering entry succeeds");
    }
    engine
}

struct CrdtSample {
    source_ms: f64,
    target_ms: f64,
    source_bytes: usize,
    target_bytes: usize,
    /// The real `flow::projection::state_json` output for each navigator after the cascade. It is
    /// `O(N)` and it is written into `flow_object_projections.state` (JSONB) **inside the lock**,
    /// so the locked phase has to be measured with the real value, not a placeholder.
    source_state: Value,
    target_state: Value,
    source_state_json_bytes: usize,
    target_state_json_bytes: usize,
    source_snapshot_bytes: usize,
    target_snapshot_bytes: usize,
}

/// One cascade's CRDT work, in the **batched** shape a sane implementation would use: one
/// `semantic_snapshot()` per document, then `N` operations, then one `export_from`.
fn crdt_batched(source_base: &[u8], target_base: &[u8], objects: &[Uuid]) -> CrdtSample {
    // ---- source navigator: N entries leave ----
    let mut source = LoroCollabEngine::load(source_base).expect("source navigator loads");
    let source_frontier = source.frontier();
    let started = Instant::now();
    let snapshot = source.semantic_snapshot().expect("source snapshot");
    let live: Vec<NodeId> = objects
        .iter()
        .map(|object_id| {
            let bare = entry_id(*object_id, 0);
            assert!(snapshot.nodes.contains_key(&bare), "the seeded entry is present");
            bare
        })
        .collect();
    for id in live {
        source
            .apply_operation(&Operation::DeleteNode { id })
            .expect("removing an ordering entry succeeds");
    }
    let source_bytes = source.export_from(&source_frontier).expect("source export");
    let source_ms = started.elapsed().as_secs_f64() * 1000.0;

    // ---- target navigator: N entries arrive, each under a fresh generation ----
    let mut target = LoroCollabEngine::load(target_base).expect("target navigator loads");
    let target_frontier = target.frontier();
    let started = Instant::now();
    let target_snapshot = target.semantic_snapshot().expect("target snapshot");
    let first_index = target_snapshot.nodes.values().filter(|node| !node.deleted).count() as u32;
    for (index, object_id) in (first_index..).zip(objects.iter()) {
        target
            .apply_operation(&Operation::CreateNode {
                id: entry_id(*object_id, 1),
                parent: None,
                index,
                kind: NodeKind::NavigatorNode,
            })
            .expect("adding an ordering entry succeeds");
    }
    let target_bytes = target.export_from(&target_frontier).expect("target export");
    let target_ms = started.elapsed().as_secs_f64() * 1000.0;

    // The projection JSON both sides write inside the lock.
    let source_after = source.semantic_snapshot().expect("source post snapshot");
    let target_after = target.semantic_snapshot().expect("target post snapshot");
    let source_state = projection::state_json(&source_after).expect("source state json");
    let target_state = projection::state_json(&target_after).expect("target state json");

    CrdtSample {
        source_ms,
        target_ms,
        source_bytes: source_bytes.len(),
        target_bytes: target_bytes.len(),
        source_state_json_bytes: serde_json::to_vec(&source_state).map_or(0, |v| v.len()),
        target_state_json_bytes: serde_json::to_vec(&target_state).map_or(0, |v| v.len()),
        source_state,
        target_state,
        source_snapshot_bytes: source.export_snapshot().map_or(0, |v| v.len()),
        target_snapshot_bytes: target.export_snapshot().map_or(0, |v| v.len()),
    }
}

/// The same work in the **literal per-node** shape `flow::move_object::build_navigator_update`
/// has today: one `semantic_snapshot()` and one `check_operation` per node. Both are `O(nodes)`,
/// so this is `O(N^2)` — measured so the difference is a number rather than an assertion.
fn crdt_per_node_ms(source_base: &[u8], objects: &[Uuid]) -> f64 {
    let limits = collab_core::DocumentLimits::DEFAULT;
    let mut source = LoroCollabEngine::load(source_base).expect("source navigator loads");
    let started = Instant::now();
    for object_id in objects {
        let snapshot = source.semantic_snapshot().expect("per-node snapshot");
        let id = entry_id(*object_id, 0);
        let operation = Operation::DeleteNode { id };
        collab_core::limits::check_operation(&snapshot, &operation, &limits).expect("delete is within limits");
        source.apply_operation(&operation).expect("per-node delete succeeds");
    }
    started.elapsed().as_secs_f64() * 1000.0
}

// ---------------------------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------------------------

struct Fixture {
    workspace_id: Uuid,
    actor_id: Uuid,
    project_a: Uuid,
    project_b: Uuid,
    /// `(object_id, document_id)` for each navigator.
    navigator_a: (Uuid, Uuid),
    navigator_b: (Uuid, Uuid),
    /// The moved subtree root, a child of navigator A.
    root_id: Uuid,
    /// The subtree: the root plus its `N - 1` children. Ascending by id — the cascade's lock order.
    subtree: Vec<Uuid>,
}

async fn seed_fixture(db: &DatabaseConnection, n: usize) -> Fixture {
    let workspace_id = Uuid::new_v4();
    let actor_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active) \
         VALUES ($1, $2, '!', 'subtree budget', 'user', true)",
        vec![actor_id.into(), format!("{actor_id}@subtree.test").into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'subtree budget', $3)",
        vec![
            workspace_id.into(),
            format!("ws-{workspace_id}").into(),
            actor_id.into(),
        ],
    )
    .await;
    exec(
        db,
        "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled) VALUES ($1, true)",
        vec![workspace_id.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
        vec![workspace_id.into(), actor_id.into()],
    )
    .await;

    let mut projects = Vec::new();
    for label in ["a", "b"] {
        let project_id = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, $3, $4, $5)",
            vec![
                project_id.into(),
                workspace_id.into(),
                format!("SBT{label}{}", &project_id.to_string()[..6]).into(),
                format!("subtree budget {label}").into(),
                actor_id.into(),
            ],
        )
        .await;
        projects.push(project_id);
    }
    let (project_a, project_b) = (projects[0], projects[1]);

    let navigator_a = seed_object(db, workspace_id, actor_id, "navigator", Some(project_a), None).await;
    let navigator_b = seed_object(db, workspace_id, actor_id, "navigator", Some(project_b), None).await;

    // The moved subtree: a root page under navigator A, plus `n - 1` children of that root.
    let (root_id, _) = seed_object(db, workspace_id, actor_id, "page", Some(project_a), Some(navigator_a.0)).await;
    let mut subtree = vec![root_id];
    let children: Vec<Uuid> = (1..n).map(|_| Uuid::new_v4()).collect();
    if !children.is_empty() {
        bulk_seed_children(db, workspace_id, actor_id, project_a, root_id, &children).await;
        subtree.extend(children);
    }
    subtree.sort_unstable();

    Fixture {
        workspace_id,
        actor_id,
        project_a,
        project_b,
        navigator_a,
        navigator_b,
        root_id,
        subtree,
    }
}

/// One `flow_objects` row plus its `collab_documents` and `flow_object_projections` rows, the same
/// three rows `flow::command::create_object` writes. Returns `(object_id, document_id)`.
async fn seed_object(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    actor_id: Uuid,
    object_type: &str,
    project_id: Option<Uuid>,
    parent_id: Option<Uuid>,
) -> (Uuid, Uuid) {
    let object_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO flow_objects (id, workspace_id, project_id, object_type, parent_id, created_by, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $6)",
        vec![
            object_id.into(),
            workspace_id.into(),
            project_id.into(),
            object_type.into(),
            parent_id.into(),
            actor_id.into(),
        ],
    )
    .await;
    let engine = LoroCollabEngine::new_empty(1);
    let snapshot = engine.export_snapshot().expect("empty snapshot exports");
    let frontier = engine.frontier().as_bytes().to_vec();
    exec(
        db,
        "INSERT INTO collab_documents (id, object_id, format_version, snapshot, snapshot_frontier, head_frontier) \
         VALUES ($1, $2, 'loro-1', $3, $4, $4)",
        vec![
            document_id.into(),
            object_id.into(),
            snapshot.into(),
            frontier.clone().into(),
        ],
    )
    .await;
    exec(
        db,
        "INSERT INTO flow_object_projections (object_id, document_seq, document_frontier) VALUES ($1, 0, $2)",
        vec![object_id.into(), frontier.into()],
    )
    .await;
    (object_id, document_id)
}

/// `n` sibling `flow_objects` rows in one statement. Only `flow_objects` — the cascade rewrites
/// `project_id` there and nowhere else, and giving 10,000 fixture rows a `collab_documents` row
/// each would measure snapshot seeding, not the cascade.
async fn bulk_seed_children(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    actor_id: Uuid,
    project_id: Uuid,
    parent_id: Uuid,
    ids: &[Uuid],
) {
    for chunk in ids.chunks(2_000) {
        let list = chunk.iter().map(Uuid::to_string).collect::<Vec<_>>().join(",");
        exec(
            db,
            "INSERT INTO flow_objects (id, workspace_id, project_id, object_type, parent_id, created_by, updated_by) \
             SELECT unnest(string_to_array($1, ',')::uuid[]), $2, $3, 'page', $4, $5, $5",
            vec![
                list.into(),
                workspace_id.into(),
                project_id.into(),
                parent_id.into(),
                actor_id.into(),
            ],
        )
        .await;
    }
}

// ---------------------------------------------------------------------------------------------
// The locked phase, replicated statement for statement
// ---------------------------------------------------------------------------------------------

/// What one measured cascade transaction reports.
#[derive(Default, Clone)]
#[allow(clippy::struct_field_names)]
struct LockedSample {
    hold_ms: f64,
    txn_span_ms: f64,
    /// `SELECT ... FROM flow_objects WHERE id = ANY(..) ORDER BY id FOR UPDATE` — the `N`-row lock.
    cascade_lock_ms: f64,
    /// `UPDATE flow_objects SET project_id = ..` over `N` rows, including the composite FK checks
    /// migration `0056` added.
    cascade_update_ms: f64,
    /// The two `stage_one_document` equivalents (event + dispatch + `collab_updates` + head +
    /// projection), both documents.
    staging_ms: f64,
    /// The recursive-CTE subtree re-derivation, inside the lock.
    subtree_rederive_ms: f64,
}

struct CascadeInput<'a> {
    fixture: &'a Fixture,
    /// The exported navigator update bytes, as the prepare phase produced them.
    source_update: Vec<u8>,
    target_update: Vec<u8>,
    source_state: Value,
    target_state: Value,
    /// `true` moves the subtree A -> B, `false` moves it back.
    forward: bool,
    /// Production `SET LOCAL lock_timeout`/`statement_timeout`, or no statement ceiling at all.
    production_budgets: bool,
    /// The anti-proof: sleep this long inside the lock.
    injected_delay_ms: u64,
}

#[derive(FromQueryResult)]
struct LockedHead {
    head_seq: i64,
    byte_count: i64,
    update_count: i64,
}

#[derive(FromQueryResult)]
struct EpochRow {
    authz_epoch: i64,
}

#[derive(FromQueryResult)]
struct IdRow {
    id: Uuid,
}

impl IdRow {
    const fn id(&self) -> Uuid {
        self.id
    }
}

async fn run_cascade(db: &DatabaseConnection, input: &CascadeInput<'_>) -> Result<LockedSample, String> {
    let fixture = input.fixture;
    let (from_project, to_project) = if input.forward {
        (fixture.project_a, fixture.project_b)
    } else {
        (fixture.project_b, fixture.project_a)
    };
    let (from_nav, to_nav) = if input.forward {
        (fixture.navigator_a, fixture.navigator_b)
    } else {
        (fixture.navigator_b, fixture.navigator_a)
    };
    let mut sample = LockedSample::default();

    let txn_started = Instant::now();
    let tx = db.begin().await.map_err(|err| format!("begin: {err}"))?;

    if input.production_budgets {
        tx.execute_unprepared("SET LOCAL lock_timeout = '180ms'")
            .await
            .map_err(|err| format!("lock_timeout: {err}"))?;
        tx.execute_unprepared("SET LOCAL statement_timeout = '200ms'")
            .await
            .map_err(|err| format!("statement_timeout: {err}"))?;
    }

    // ---- the clock starts at the first row lock (`limits-v1.md`: hold is counted from a
    // successful `SELECT ... FOR UPDATE` to commit/rollback completion) ----
    let hold_started = Instant::now();

    // [layer 1] the epoch row, exclusively — `flow::move_object` takes `lock_epoch_for_update`.
    let epoch = EpochRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT authz_epoch FROM flow_workspace_settings WHERE workspace_id = $1 FOR UPDATE",
        vec![fixture.workspace_id.into()],
    ))
    .one(&tx)
    .await
    .map_err(|err| format!("epoch lock: {err}"))?
    .ok_or_else(|| "epoch row missing".to_string())?;

    // The subtree re-derivation, on this transaction's own snapshot. A cascade cannot trust the
    // out-of-lock membership any more than `derive_contended_set` trusts its own.
    let started = Instant::now();
    let derived = IdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "WITH RECURSIVE subtree AS ( \
             SELECT o.id, 0 AS depth FROM flow_objects o WHERE o.id = $1 AND o.workspace_id = $2 \
             UNION ALL \
             SELECT c.id, s.depth + 1 FROM subtree s \
               JOIN flow_objects c ON c.parent_id = s.id AND c.workspace_id = $2 \
              WHERE s.depth < $3::int \
         ) SELECT id FROM subtree ORDER BY id",
        vec![fixture.root_id.into(), fixture.workspace_id.into(), 33i64.into()],
    ))
    .all(&tx)
    .await
    .map_err(|err| format!("subtree rederive: {err}"))?;
    sample.subtree_rederive_ms = started.elapsed().as_secs_f64() * 1000.0;
    let derived: Vec<Uuid> = derived.iter().map(IdRow::id).collect();
    if derived.len() != fixture.subtree.len() {
        let _ = tx.rollback().await;
        return Err(format!(
            "subtree re-derivation saw {} rows, fixture has {}",
            derived.len(),
            fixture.subtree.len()
        ));
    }

    // [layer 2 / cascade] the `N` moved rows, ascending id.
    let id_list = fixture
        .subtree
        .iter()
        .map(Uuid::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let started = Instant::now();
    let locked_rows = IdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM flow_objects WHERE id = ANY(string_to_array($1, ',')::uuid[]) ORDER BY id FOR UPDATE",
        vec![id_list.clone().into()],
    ))
    .all(&tx)
    .await
    .map_err(|err| format!("cascade row lock: {err}"))?;
    sample.cascade_lock_ms = started.elapsed().as_secs_f64() * 1000.0;
    let locked_row_count = locked_rows.iter().map(IdRow::id).count();
    if locked_row_count != fixture.subtree.len() {
        let _ = tx.rollback().await;
        return Err("the cascade row lock did not see the whole subtree".to_string());
    }

    // [layer 3] the two navigator documents, ascending `document_id`, then their staged writes —
    // the identical statement set `write::stage_one_document` issues.
    let mut documents = [
        (from_nav, input.source_update.clone(), input.source_state.clone()),
        (to_nav, input.target_update.clone(), input.target_state.clone()),
    ];
    documents.sort_by_key(|entry| entry.0.1);

    let started = Instant::now();
    for ((object_id, document_id), bytes, state) in &documents {
        let locked = LockedHead::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_seq, byte_count, update_count FROM collab_documents WHERE id = $1 FOR UPDATE",
            vec![(*document_id).into()],
        ))
        .one(&tx)
        .await
        .map_err(|err| format!("document row lock: {err}"))?
        .ok_or_else(|| "document row missing".to_string())?;

        let new_head_seq = locked.head_seq + 1;
        let frontier = Uuid::new_v4().as_bytes().to_vec();
        let event_id = insert_flow_event(
            &tx,
            BusinessEventInput {
                workspace_id: fixture.workspace_id,
                project_id: None,
                event_type: "flow.content.accepted".to_string(),
                aggregate_type: "flow_document".to_string(),
                aggregate_id: document_id.to_string(),
                actor_id: Some(fixture.actor_id),
                source: json!({ "surface": "rest" }),
                payload: json!({
                    "object_id": object_id,
                    "document_id": document_id,
                    "accepted_seq": new_head_seq,
                    "projection_seq": new_head_seq,
                    "changed_block_ids": Vec::<Uuid>::new(),
                }),
                metadata: json!({ "message": Value::Null }),
                correlation_id: None,
                causation_id: None,
                idempotency_key: None,
            },
            Some(FlowDispatchSpec {
                max_attempts: 10,
                document_id: Some(*document_id),
                accepted_seq: Some(new_head_seq),
            }),
        )
        .await
        .map_err(|err| format!("content event: {err:?}"))?
        .event_id;

        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO collab_updates \
                 (document_id, seq, update_id, content_hash, idempotency_key, before_frontier, after_frontier, \
                  bytes, actor_id, origin_surface, origin_client_id, projection_seq, event_id) \
             VALUES ($1, $2, $3, $4, NULL, $5, $6, $7, $8, 'rest', NULL, $2, $9)",
            vec![
                (*document_id).into(),
                new_head_seq.into(),
                Uuid::new_v4().into(),
                format!("{:064x}", new_head_seq as u128 + document_id.as_u128()).into(),
                frontier.clone().into(),
                frontier.clone().into(),
                bytes.clone().into(),
                fixture.actor_id.into(),
                event_id.into(),
            ],
        ))
        .await
        .map_err(|err| format!("collab_updates insert: {err}"))?;

        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE collab_documents \
             SET head_seq = $2, head_frontier = $3, byte_count = $4, update_count = $5, updated_at = now() \
             WHERE id = $1",
            vec![
                (*document_id).into(),
                new_head_seq.into(),
                frontier.clone().into(),
                (locked.byte_count + i64::try_from(bytes.len()).unwrap_or(i64::MAX)).into(),
                (locked.update_count + 1).into(),
            ],
        ))
        .await
        .map_err(|err| format!("head update: {err}"))?;

        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_object_projections \
             SET document_seq = $2, document_frontier = $3, title = $4, state = $5, plain_text = $6, updated_at = now() \
             WHERE object_id = $1",
            vec![
                (*object_id).into(),
                new_head_seq.into(),
                frontier.into(),
                "".into(),
                state.clone().into(),
                "".into(),
            ],
        ))
        .await
        .map_err(|err| format!("projection update: {err}"))?;
    }
    sample.staging_ms = started.elapsed().as_secs_f64() * 1000.0;

    // ---- the cascade write itself ----
    //
    // **One statement, and it has to be one statement.** Migration `0056`'s
    // `flow_objects_parent_project_fk` is `(parent_id, project_scope_id) REFERENCES
    // flow_objects (id, project_scope_id)` and is `NOT DEFERRABLE`, so `PostgreSQL` validates it
    // at the end of *every* statement. `flow::move_object` today issues the governance write as
    // its own `repository::set_object_parent` statement, which works only because it moves one
    // leaf; splitting a cascade into "re-parent the root" + "rewrite the descendants" fails at
    // the end of the first statement, with the root already in the new scope and its children
    // still in the old one. Measured, not reasoned about: the first version of this harness did
    // exactly that split and every forward attempt came back
    // `violates foreign key constraint "flow_objects_parent_project_fk"`.
    let started = Instant::now();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_objects \
         SET project_id = $2, \
             parent_id = CASE WHEN id = $4 THEN $5 ELSE parent_id END, \
             updated_at = now(), updated_by = $3 \
         WHERE id = ANY(string_to_array($1, ',')::uuid[])",
        vec![
            id_list.into(),
            to_project.into(),
            fixture.actor_id.into(),
            fixture.root_id.into(),
            to_nav.0.into(),
        ],
    ))
    .await
    .map_err(|err| format!("cascade project_id update: {err}"))?;
    sample.cascade_update_ms = started.elapsed().as_secs_f64() * 1000.0;

    // The governance event and the epoch advance.
    insert_flow_event(
        &tx,
        BusinessEventInput {
            workspace_id: fixture.workspace_id,
            project_id: Some(to_project),
            event_type: "flow.object.moved".to_string(),
            aggregate_type: "flow_object".to_string(),
            aggregate_id: fixture.root_id.to_string(),
            actor_id: Some(fixture.actor_id),
            source: json!({ "surface": "rest" }),
            payload: json!({
                "object_id": fixture.root_id,
                "old_parent_id": Value::Null,
                "new_parent_id": to_nav.0,
                "position_key": Value::Null,
                "cascaded_nodes": fixture.subtree.len(),
                "from_project_id": from_project,
            }),
            metadata: json!({ "message": Value::Null }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: Some(Uuid::new_v4().to_string()),
        },
        Some(FlowDispatchSpec {
            max_attempts: 10,
            document_id: None,
            accepted_seq: None,
        }),
    )
    .await
    .map_err(|err| format!("move event: {err:?}"))?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_workspace_settings SET authz_epoch = $2 WHERE workspace_id = $1",
        vec![fixture.workspace_id.into(), (epoch.authz_epoch + 1).into()],
    ))
    .await
    .map_err(|err| format!("epoch advance: {err}"))?;

    if input.injected_delay_ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(input.injected_delay_ms)).await;
    }

    tx.commit().await.map_err(|err| format!("commit: {err}"))?;
    sample.hold_ms = hold_started.elapsed().as_secs_f64() * 1000.0;
    sample.txn_span_ms = txn_started.elapsed().as_secs_f64() * 1000.0;
    Ok(sample)
}

/// The plan `PostgreSQL` picks for the referential-integrity check that
/// `flow_objects_parent_project_fk` fires on **every row the cascade updates**.
///
/// The constraint is `FOREIGN KEY (parent_id, project_scope_id) REFERENCES flow_objects (id,
/// project_scope_id)`. Updating a referenced key column (`project_scope_id` is `GENERATED ALWAYS
/// AS (COALESCE(project_id, nil))`, so rewriting `project_id` rewrites it) fires the `ON UPDATE
/// NO ACTION` trigger, whose query is the one below: "does any child still point at the old
/// key?". Migration `0056` adds no index on `(parent_id, project_scope_id)`, and
/// `idx_flow_objects_parent` is `(workspace_id, parent_id)` — the wrong leading column. If the
/// planner answers `Seq Scan`, one cascade of `N` rows costs `N` sequential scans of
/// `flow_objects`, which is a property of the schema, not of the limit.
async fn cascade_ri_check_plan(db: &DatabaseConnection) -> Value {
    let sql = "SELECT 1 FROM ONLY flow_objects x WHERE $1::uuid OPERATOR(pg_catalog.=) x.parent_id AND $2::uuid OPERATOR(pg_catalog.=) x.project_scope_id FOR KEY SHARE";
    // `EXPLAIN`'s single output column is literally named `QUERY PLAN`, which no derive can name
    // and no alias can rename (`EXPLAIN` is not allowed in a subquery), so the rows are read
    // positionally instead of through `FromQueryResult`.
    let plan = db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("EXPLAIN (ANALYZE, BUFFERS) {sql}"),
            vec![Uuid::nil().into(), Uuid::nil().into()],
        ))
        .await
        .map(|rows| {
            rows.into_iter()
                .filter_map(|row| row.try_get_by_index::<String>(0).ok())
                .collect::<Vec<_>>()
                .join("\n")
        });
    match plan {
        Ok(plan) if !plan.is_empty() => {
            // The question is not "is any index used" -- the planner already reaches for
            // `flow_objects_project_scope_key` here, as a full index scan filtered on its
            // *non-leading* column. It is whether an index **leads** with `parent_id`.
            let leads_with_parent_id =
                plan.contains("Index Cond: (parent_id") || plan.contains("idx_flow_objects_parent_project_scope");
            json!({
                "referential_integrity_check_sql": sql,
                "plan": plan,
                "leads_with_parent_id": leads_with_parent_id,
                "indexes_on_flow_objects": [
                    "idx_flow_objects_parent (workspace_id, parent_id)",
                    "flow_objects_workspace_id_key (workspace_id, id)",
                    "flow_objects_project_scope_key (id, project_scope_id)",
                    "idx_flow_objects_project (workspace_id, project_id) WHERE project_id IS NOT NULL",
                    "idx_flow_objects_type_status (workspace_id, object_type, lifecycle_status)",
                ],
            })
        }
        Ok(_) => json!({ "error": "EXPLAIN returned no row" }),
        Err(err) => json!({ "error": format!("{err}") }),
    }
}

/// Per-statement client/server round-trip cost on this connection, and the cost of an empty
/// `BEGIN`/`COMMIT` pair.
///
/// `hold_ms` here is an **in-process** span: it starts before the client writes the first
/// `FOR UPDATE` on the wire and stops after `COMMIT`'s reply is parsed, so it includes one
/// round-trip per statement that `PostgreSQL`'s own `%m`-derived transaction span (what
/// `flow_collab_load_harness.rs` uses) does not. That makes every number here an **upper bound**
/// on the true lock hold. This function measures the size of that bias instead of hand-waving it.
async fn round_trip_calibration(db: &DatabaseConnection) -> Value {
    const PROBES: usize = 40;
    let mut empty_txn = Vec::new();
    let mut per_statement = Vec::new();
    for _ in 0..PROBES {
        let started = Instant::now();
        let tx = db.begin().await.expect("calibration begin");
        tx.commit().await.expect("calibration commit");
        empty_txn.push(started.elapsed().as_secs_f64() * 1000.0);

        let tx = db.begin().await.expect("calibration begin");
        let started = Instant::now();
        for _ in 0..10 {
            tx.execute(Statement::from_string(DbBackend::Postgres, "SELECT 1"))
                .await
                .expect("calibration select");
        }
        per_statement.push(started.elapsed().as_secs_f64() * 100.0);
        tx.commit().await.expect("calibration commit");
    }
    let empty = Distribution::of(&empty_txn);
    let statement = Distribution::of(&per_statement);
    json!({
        "empty_begin_commit_ms": empty.to_json(),
        "per_statement_round_trip_ms": statement.to_json(),
        "statements_in_the_measured_cascade": "16 + 2 events + 2 dispatch inserts (fixed), independent of N",
        "note": "hold_ms is an in-process span and therefore an upper bound: it carries one client round trip per statement, which a PostgreSQL-log-derived span does not",
    })
}

// ---------------------------------------------------------------------------------------------
// One rung of the ladder
// ---------------------------------------------------------------------------------------------

struct RungResult {
    n: usize,
    crdt_source: Distribution,
    crdt_target: Distribution,
    crdt_total: Distribution,
    crdt_per_node_naive: Option<Distribution>,
    source_update_bytes: usize,
    target_update_bytes: usize,
    source_state_json_bytes: usize,
    target_state_json_bytes: usize,
    source_snapshot_bytes: usize,
    target_snapshot_bytes: usize,
    hold: Option<Distribution>,
    hold_unbudgeted: Option<Distribution>,
    cascade_lock: Option<Distribution>,
    cascade_update: Option<Distribution>,
    staging: Option<Distribution>,
    subtree_rederive: Option<Distribution>,
    production_budget_failures: Vec<String>,
    production_budget_timeouts: usize,
    production_budget_attempts: usize,
    notes: Vec<String>,
}

/// A failure caused by the production `SET LOCAL statement_timeout`/`lock_timeout`, as opposed to
/// anything else (which is a harness defect and must be reported as one, never folded into "the
/// budget rejected it").
fn is_statement_budget_failure(message: &str) -> bool {
    message.contains("statement timeout") || message.contains("lock timeout") || message.contains("due to lock")
}

/// Flushes the write-ahead log the fixture seeding just produced and lets the instance settle.
///
/// Not a way to make the numbers look better: it removes an artefact of *this harness* that a real
/// `move_object` never has behind it. Each rung creates a database, replays every migration and
/// bulk-inserts up to 10,000 rows immediately before measuring, and the checkpoint that burst
/// triggers lands in the middle of the sample window — which is how the first fine-ladder run
/// produced `N=50 → p95 223 ms` sitting between `N=30 → 6.2 ms` and `N=75 → 9.7 ms`. `ANALYZE`
/// is issued for the same reason: a freshly-loaded table has no statistics, so the planner is
/// choosing the cascade's plan blind, which is not the state a production table is in.
async fn checkpoint_and_settle(db: &DatabaseConnection) {
    let _ = db.execute_unprepared("ANALYZE").await;
    let _ = db.execute_unprepared("CHECKPOINT").await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
}

async fn measure_rung(db: &DatabaseConnection, n: usize) -> RungResult {
    let mut notes = Vec::new();
    let fixture = seed_fixture(db, n).await;
    if add_fk_index_requested() {
        // `IF NOT EXISTS` since migration `0057` now creates this index by this exact name, which
        // is the outcome this counterfactual argued for. The flag is kept so the pre-`0057` curve
        // can still be re-measured by reverting that one migration, and a plain `CREATE INDEX`
        // would now fail against a migrated database rather than measure anything.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_flow_objects_parent_project_scope ON flow_objects (parent_id, project_scope_id)",
        )
        .await
        .expect("the counterfactual foreign-key index is created");
        notes.push(
            "measured WITH the index idx_flow_objects_parent_project_scope \
             (parent_id, project_scope_id), created by migration 0057"
                .to_string(),
        );
    }
    checkpoint_and_settle(db).await;

    // ---- CRDT side ----
    let source_base = navigator_with_entries(&fixture.subtree)
        .export_snapshot()
        .expect("source base exports");
    let target_base = LoroCollabEngine::new_empty(9)
        .export_snapshot()
        .expect("target base exports");

    let mut source_ms = Vec::new();
    let mut target_ms = Vec::new();
    let mut total_ms = Vec::new();
    let mut last: Option<CrdtSample> = None;
    for round in 0..(WARMUP_ROUNDS + MIN_SAMPLES) {
        let sample = crdt_batched(&source_base, &target_base, &fixture.subtree);
        if round >= WARMUP_ROUNDS {
            source_ms.push(sample.source_ms);
            target_ms.push(sample.target_ms);
            total_ms.push(sample.source_ms + sample.target_ms);
        }
        last = Some(sample);
    }
    let last = last.expect("at least one CRDT sample");

    let crdt_per_node_naive = if n <= NAIVE_CRDT_MAX_N {
        let mut values = Vec::new();
        for _ in 0..MIN_SAMPLES.min(10) {
            values.push(crdt_per_node_ms(&source_base, &fixture.subtree));
        }
        Some(Distribution::of(&values))
    } else {
        notes.push(format!(
            "per-node (literal build_navigator_update) CRDT shape not run at N={n}: it is O(N^2) \
             (semantic_snapshot + check_operation per node, both O(nodes)); measured up to N={NAIVE_CRDT_MAX_N}"
        ));
        None
    };

    if n > CONTAINER_COUNT_MAX {
        notes.push(format!(
            "N={n} exceeds container_count_max={CONTAINER_COUNT_MAX}: a navigator document cannot \
             legally hold this many live ordering entries at all"
        ));
    }

    // ---- locked phase, production budgets ----
    let mut production_budget_failures: Vec<String> = Vec::new();
    let mut production_budget_timeouts = 0usize;
    let mut production_budget_attempts_failed = 0usize;
    let mut hold_values = Vec::new();
    let mut lock_values = Vec::new();
    let mut update_values = Vec::new();
    let mut staging_values = Vec::new();
    let mut rederive_values = Vec::new();

    for round in 0..(WARMUP_ROUNDS + MIN_SAMPLES) {
        let input = CascadeInput {
            fixture: &fixture,
            source_update: vec![0u8; last.source_bytes.max(1)],
            target_update: vec![0u8; last.target_bytes.max(1)],
            source_state: last.source_state.clone(),
            target_state: last.target_state.clone(),
            forward: round % 2 == 0,
            production_budgets: true,
            injected_delay_ms: 0,
        };
        match run_cascade(db, &input).await {
            Ok(sample) => {
                if round >= WARMUP_ROUNDS {
                    hold_values.push(sample.hold_ms);
                    lock_values.push(sample.cascade_lock_ms);
                    update_values.push(sample.cascade_update_ms);
                    staging_values.push(sample.staging_ms);
                    rederive_values.push(sample.subtree_rederive_ms);
                }
            }
            Err(err) => {
                production_budget_attempts_failed += 1;
                if is_statement_budget_failure(&err) {
                    production_budget_timeouts += 1;
                }
                if production_budget_failures.len() < 5 {
                    production_budget_failures.push(err);
                }
            }
        }
    }

    // ---- locked phase, statement ceilings removed, to obtain the cost curve even where the
    // production budget aborts the transaction. This makes the verdict worse, never better. ----
    let mut unbudgeted_values = Vec::new();
    if production_budget_attempts_failed > 0 {
        notes.push(format!(
            "N={n}: {production_budget_attempts_failed} of {} production-budget attempts failed, \
             {production_budget_timeouts} of them on SET LOCAL statement_timeout/lock_timeout; \
             the unbudgeted pass below reports the cost the budget refused to pay",
            WARMUP_ROUNDS + MIN_SAMPLES
        ));
        for round in 0..(WARMUP_ROUNDS + MIN_SAMPLES) {
            let input = CascadeInput {
                fixture: &fixture,
                source_update: vec![0u8; last.source_bytes.max(1)],
                target_update: vec![0u8; last.target_bytes.max(1)],
                source_state: last.source_state.clone(),
                target_state: last.target_state.clone(),
                forward: round % 2 == 0,
                production_budgets: false,
                injected_delay_ms: 0,
            };
            match run_cascade(db, &input).await {
                Ok(sample) => {
                    if round >= WARMUP_ROUNDS {
                        unbudgeted_values.push(sample.hold_ms);
                    }
                }
                Err(err) => notes.push(format!("N={n} unbudgeted attempt failed: {err}")),
            }
        }
    }

    RungResult {
        n,
        crdt_source: Distribution::of(&source_ms),
        crdt_target: Distribution::of(&target_ms),
        crdt_total: Distribution::of(&total_ms),
        crdt_per_node_naive,
        source_update_bytes: last.source_bytes,
        target_update_bytes: last.target_bytes,
        source_state_json_bytes: last.source_state_json_bytes,
        target_state_json_bytes: last.target_state_json_bytes,
        source_snapshot_bytes: last.source_snapshot_bytes,
        target_snapshot_bytes: last.target_snapshot_bytes,
        hold: (!hold_values.is_empty()).then(|| Distribution::of(&hold_values)),
        hold_unbudgeted: (!unbudgeted_values.is_empty()).then(|| Distribution::of(&unbudgeted_values)),
        cascade_lock: (!lock_values.is_empty()).then(|| Distribution::of(&lock_values)),
        cascade_update: (!update_values.is_empty()).then(|| Distribution::of(&update_values)),
        staging: (!staging_values.is_empty()).then(|| Distribution::of(&staging_values)),
        subtree_rederive: (!rederive_values.is_empty()).then(|| Distribution::of(&rederive_values)),
        production_budget_failures,
        production_budget_timeouts,
        production_budget_attempts: WARMUP_ROUNDS + MIN_SAMPLES,
        notes,
    }
}

fn rung_json(rung: &RungResult) -> Value {
    json!({
        "n": rung.n,
        "crdt_out_of_lock": {
            "source_navigator_ms": rung.crdt_source.to_json(),
            "target_navigator_ms": rung.crdt_target.to_json(),
            "total_ms": rung.crdt_total.to_json(),
            "per_node_literal_shape_ms": rung.crdt_per_node_naive.as_ref().map(Distribution::to_json),
        },
        "crdt_bytes": {
            "source_update_bytes": rung.source_update_bytes,
            "target_update_bytes": rung.target_update_bytes,
            "largest_single_update_bytes": rung.source_update_bytes.max(rung.target_update_bytes),
            "update_bytes_max": UPDATE_BYTES_MAX,
            "over_update_bytes_max": rung.source_update_bytes.max(rung.target_update_bytes) > UPDATE_BYTES_MAX,
            "largest_update_as_ws_frame_bytes":
                (rung.source_update_bytes.max(rung.target_update_bytes) as f64 * 4.0 / 3.0).ceil() as usize + 256,
            "websocket_frame_bytes_max": WEBSOCKET_FRAME_BYTES_MAX,
            "source_projection_state_json_bytes": rung.source_state_json_bytes,
            "target_projection_state_json_bytes": rung.target_state_json_bytes,
            "source_snapshot_bytes": rung.source_snapshot_bytes,
            "target_snapshot_bytes": rung.target_snapshot_bytes,
            "snapshot_tail_bytes_soft_max": SNAPSHOT_TAIL_BYTES_SOFT_MAX,
        },
        "locked_phase": {
            "hold_ms_production_budgets": rung.hold.as_ref().map(Distribution::to_json),
            "hold_ms_no_statement_ceiling": rung.hold_unbudgeted.as_ref().map(Distribution::to_json),
            "cascade_row_lock_ms": rung.cascade_lock.as_ref().map(Distribution::to_json),
            "cascade_project_id_update_ms": rung.cascade_update.as_ref().map(Distribution::to_json),
            "two_document_staging_ms": rung.staging.as_ref().map(Distribution::to_json),
            "subtree_rederive_ms": rung.subtree_rederive.as_ref().map(Distribution::to_json),
            "production_budget_attempts": rung.production_budget_attempts,
            "production_budget_statement_timeouts": rung.production_budget_timeouts,
            "production_budget_failures": rung.production_budget_failures,
        },
        "notes": rung.notes,
    })
}

// ---------------------------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "environment-gated heavy measurement; run explicitly against dedicated PostgreSQL"]
async fn move_subtree_nodes_max_cost_curve() {
    if let Some((code, detail)) = environment_problem() {
        emit_not_satisfied(code, &detail);
        panic!("ENVIRONMENT NOT SATISFIED [{code}]: {detail}");
    }
    // Every rung gets its **own** scratch database. Sharing one across the ladder makes the last
    // rungs measure the dead tuples and table growth the earlier ones left behind: in the first
    // run of this harness the shared-database anti-proof baseline for N=10 came out at
    // p50 = 53 ms, against 4.3 ms for the identical N=10 rung measured on a fresh database.
    let load_at_start = load_average();
    let mut rungs = Vec::new();
    let ladder = ladder();
    for (index, &n) in ladder.iter().enumerate() {
        let Some(scratch) = scratch(&format!("rung{index}")).await else {
            emit_not_satisfied("scratch_unavailable", "the scratch database could not be created");
            panic!("the scratch database could not be created");
        };
        let started = Instant::now();
        let rung = measure_rung(&scratch.db, n).await;
        eprintln!(
            "rung N={n} finished in {:.1}s (hold p95 {:?})",
            started.elapsed().as_secs_f64(),
            rung.hold.as_ref().map(|d| d.p95_ms)
        );
        rungs.push(rung);
        scratch.drop_self().await;
    }

    // ---- anti-proof: the instrument has to see a delay it did not otherwise have ----
    let Some(scratch) = scratch("antiproof").await else {
        emit_not_satisfied(
            "scratch_unavailable",
            "the anti-proof scratch database could not be created",
        );
        panic!("the anti-proof scratch database could not be created");
    };
    let anti_fixture = seed_fixture(&scratch.db, 10).await;
    checkpoint_and_settle(&scratch.db).await;
    let mut baseline = Vec::new();
    let mut injected = Vec::new();
    for round in 0..(WARMUP_ROUNDS + MIN_SAMPLES) {
        for (delay, sink) in [(0u64, &mut baseline), (INJECTED_DELAY_MS, &mut injected)] {
            let input = CascadeInput {
                fixture: &anti_fixture,
                source_update: vec![0u8; 512],
                target_update: vec![0u8; 512],
                source_state: json!({ "nodes": {} }),
                target_state: json!({ "nodes": {} }),
                forward: round % 2 == 0,
                // The injected delay is deliberately longer than `statement_timeout`; it sits
                // between statements, where `statement_timeout` does not apply, which is exactly
                // the "application work inside the lock" this instrument must be able to see.
                production_budgets: true,
                injected_delay_ms: delay,
            };
            if let Ok(sample) = run_cascade(&scratch.db, &input).await
                && round >= WARMUP_ROUNDS
            {
                sink.push(sample.hold_ms);
            }
        }
    }
    let calibration = round_trip_calibration(&scratch.db).await;
    let ri_plan = cascade_ri_check_plan(&scratch.db).await;
    let baseline_dist = Distribution::of(&baseline);
    let injected_dist = Distribution::of(&injected);
    let observed_shift = injected_dist.p50_ms - baseline_dist.p50_ms;
    let instrument_sees_injection = observed_shift >= INJECTED_DELAY_MS as f64 * 0.9;

    // ---- verdict ----
    let mut violations: Vec<String> = Vec::new();
    let mut largest_n_within_time = 0usize;
    let mut largest_n_within_bytes = 0usize;
    for rung in &rungs {
        if let Some(hold) = &rung.hold {
            if hold.samples < MIN_SAMPLES && rung.production_budget_timeouts == 0 {
                violations.push(format!(
                    "N={}: hold has only {} samples, below the {MIN_SAMPLES} the official-run protocol requires, \
                     and no statement/lock budget abort explains the shortfall",
                    rung.n, hold.samples
                ));
            }
            if hold.p95_ms <= LOCK_HOLD_P95_MS_MAX && hold.max_ms <= LOCK_HOLD_TWO_DOCUMENT_MS_MAX {
                largest_n_within_time = largest_n_within_time.max(rung.n);
            }
        }
        let other_failures = rung
            .production_budget_failures
            .iter()
            .filter(|message| !is_statement_budget_failure(message))
            .count();
        if other_failures > 0 {
            violations.push(format!(
                "N={}: {} production-budget attempt(s) failed for a reason that is not the \
                 statement/lock budget — that is a harness defect, not a measurement: {:?}",
                rung.n, other_failures, rung.production_budget_failures
            ));
        }
        let largest_update = rung.source_update_bytes.max(rung.target_update_bytes);
        if largest_update <= UPDATE_BYTES_MAX {
            largest_n_within_bytes = largest_n_within_bytes.max(rung.n);
        }
    }
    if !instrument_sees_injection {
        violations.push(format!(
            "the instrument did not see a {INJECTED_DELAY_MS} ms injected in-lock delay \
             (p50 shift {observed_shift:.1} ms); the measurement cannot be trusted"
        ));
    }

    let report = json!({
        "schema_version": "sylvode.flow.subtree-budget-measurement.v1",
        "generated_at": Utc::now().to_rfc3339(),
        "environment_gate": {
            "status": "satisfied",
            "declared_dedicated_pg_container": std::env::var(DEDICATED_PG_CONTAINER_ENV).ok(),
            "known_shared_instances_rejected": ["flow-test-pg"],
            "separate_test_binary": true,
            "host_load_average_at_start": load_at_start,
            "host_load_average_at_end": load_average(),
        },
        "schema_variant": if add_fk_index_requested() {
            "with_counterfactual_fk_index"
        } else {
            "as_shipped_migration_0056"
        },
        "protocol": {
            "warmup_rounds": WARMUP_ROUNDS,
            "min_samples": MIN_SAMPLES,
            "ladder": ladder,
            "hold_definition": "from the first successful SELECT ... FOR UPDATE to the return of COMMIT, measured in process",
        },
        "frozen_targets": {
            "document_lock_hold_ms_p95_max": LOCK_HOLD_P95_MS_MAX,
            "document_lock_hold_ms_max": LOCK_HOLD_SINGLE_MS_MAX,
            "document_lock_hold_ms_max_per_extra_document": LOCK_HOLD_PER_EXTRA_DOCUMENT_MS,
            "two_document_lock_hold_ms_max": LOCK_HOLD_TWO_DOCUMENT_MS_MAX,
            "update_bytes_max": UPDATE_BYTES_MAX,
            "websocket_frame_bytes_max": WEBSOCKET_FRAME_BYTES_MAX,
            "container_count_max": CONTAINER_COUNT_MAX,
            "snapshot_tail_bytes_soft_max": SNAPSHOT_TAIL_BYTES_SOFT_MAX,
        },
        "rungs": rungs.iter().map(rung_json).collect::<Vec<_>>(),
        "instrument_self_check": {
            "injected_in_lock_delay_ms": INJECTED_DELAY_MS,
            "baseline_hold": baseline_dist.to_json(),
            "injected_hold": injected_dist.to_json(),
            "observed_p50_shift_ms": (observed_shift * 1000.0).round() / 1000.0,
            "instrument_sees_injection": instrument_sees_injection,
            "round_trip_calibration": calibration,
            "cascade_referential_integrity_plan": ri_plan,
        },
        "derived": {
            "largest_n_within_lock_hold_budget": largest_n_within_time,
            "largest_n_within_update_bytes_max": largest_n_within_bytes,
        },
        "violations": violations,
        "passed": violations.is_empty(),
    });
    emit(&report);
    scratch.drop_self().await;
}
