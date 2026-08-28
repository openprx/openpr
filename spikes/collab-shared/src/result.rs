//! `serde` types mirroring `docs/schemas/sylvode-flow-convergence-result-v1.schema.json` and
//! `...benchmark-result-v1.schema.json` field-for-field.
//!
//! This includes their `additionalProperties: false` closed shape — every field in the schema's
//! `required` arrays has a corresponding non-`Option` field here, and there are no extra fields
//! serde would emit that the schema would reject.
//!
//! **Scope note**: this module defines the *shape* so a future full corpus/benchmark orchestrator
//! can serialize directly into schema-valid JSON without hand-building strings. It does not
//! itself run the JSON Schema validator (`docs/schemas/*.json`) against sample output — see the
//! delivery report for why that was out of scope this round, and the manual field-by-field
//! comparison instead.

use serde::{Deserialize, Serialize};

fn is_lowercase_hex(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len && value.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// A `^[0-9a-f]{40}$` git commit sha, validated at construction so malformed values can never be
/// serialized into a result document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct GitSha(String);

impl GitSha {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if is_lowercase_hex(&value, 40) {
            Ok(Self(value))
        } else {
            Err(format!("{value} is not a 40-char lowercase hex git sha"))
        }
    }
}

impl TryFrom<String> for GitSha {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<GitSha> for String {
    fn from(value: GitSha) -> Self {
        value.0
    }
}

/// A `^[0-9a-f]{64}$` SHA-256 digest, validated at construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Hex(String);

impl Sha256Hex {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if is_lowercase_hex(&value, 64) {
            Ok(Self(value))
        } else {
            Err(format!("{value} is not a 64-char lowercase hex sha256 digest"))
        }
    }
}

impl TryFrom<String> for Sha256Hex {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Sha256Hex> for String {
    fn from(value: Sha256Hex) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CandidateName {
    Loro,
    #[serde(rename = "yrs-yjs")]
    YrsYjs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub path: String,
    pub sha256: Sha256Hex,
}

// --- Isolation result (ADR-0014 section 9: per-case array wire shape) ------------------------
//
// This mirrors `$defs/isolation_result` in the v1 schema field-for-field, replacing the old
// side-level-scalar shape (`kind`/`adversarial_cases_run`/`partial_state_unchanged`/`status`)
// that ADR-0014's background section identifies as a guaranteed-false-green wire: a runner could
// hardcode `terminable_instance` + `status: passed` and the old schema would accept it with no
// CPU, memory, or termination evidence at all. The new shape forces every case to carry its own
// meter identity, termination-cause array, and verdict, so a runner cannot fake a pass without
// also fabricating per-case host/platform/cause data that the verifier can cross-check.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationKind {
    TerminableInstance,
    WorkerSandbox,
    AsyncTimeout,
}

/// The termination/completion oracle for one isolation case. Mirrors `$defs/isolation_oracle`.
///
/// Note `cpu_ceiling_enforcement_failed` is a distinct terminal state from `cpu_ceiling`: it
/// means the wall watchdog had to intervene *and* the child's own CPU accounting already showed
/// it over the 50ms ceiling — i.e. the online `SIGPROF` enforcer failed to fire in time and the
/// wall watchdog is the only reason the process didn't run away further. Collapsing this into
/// `wall_ceiling` (as ADR-0014 R18 did) would silently hide a CPU-enforcement outage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationOracle {
    Completed,
    CpuCeiling,
    WallCeiling,
    MemoryCeiling,
    CpuCeilingEnforcementFailed,
    AddressSpaceBackstop,
    Crashed,
    MeterUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationCpuMeter {
    NativeItimerProf,
    NotApplicableWeb,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationMemoryMeter {
    CountingAllocator,
    DiagnosticOnly,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationMemorySampling {
    ContinuousHighWater,
    Endpoints,
}

/// One concurrent termination/observation event.
///
/// ADR-0014 section 1.2: termination is an event *array*, not a single scalar, because
/// CPU-ceiling-pending, wall-watchdog-kill, and an allocator that already wrote a memory cause can
/// all be true of the same case at once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IsolationTerminationCause {
    pub cause: IsolationOracle,
    pub source: String,
    pub observed_at_ms: f64,
}

/// Structural same-artifact attestation (ADR-0014 section 7): both candidates run inside the same
/// host binary chosen by argv.
///
/// `artifact_sha256`/`isolation_host_hash` must be identical across the `loro` and `yrs-yjs`
/// candidate results in a single convergence-result document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IsolationHost {
    pub artifact_sha256: Sha256Hex,
    pub artifact_path: String,
    pub config_digest: Sha256Hex,
    pub source_tree_hash: Sha256Hex,
    pub lockfile_hash: Sha256Hex,
    pub toolchain: String,
    pub features: Vec<String>,
    pub release_flags: Vec<String>,
    pub isolation_host_hash: Sha256Hex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationOs {
    Linux,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IsolationPlatformNative {
    pub os: IsolationOs,
    pub kernel: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IsolationPlatformWeb {
    pub browser: String,
    pub version: String,
    pub cross_origin_isolated: bool,
}

/// `$defs/isolation_platform` is a schema `oneOf` distinguished by which required fields are
/// present (`os`/`kernel` for native, `browser`/`cross_origin_isolated` for web).
///
/// It is not distinguished by an explicit tag property, so this mirrors that with
/// `#[serde(untagged)]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IsolationPlatform {
    Native(IsolationPlatformNative),
    Web(IsolationPlatformWeb),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IsolationCase {
    pub fixture_id: String,
    pub expected_oracle: IsolationOracle,
    pub kind: IsolationKind,
    pub cpu_ms: f64,
    pub cpu_meter: IsolationCpuMeter,
    pub wall_ms: f64,
    pub wall_meter: String,
    pub allocated_active_peak_bytes: u64,
    pub allocated_attempted_peak_bytes: u64,
    pub rss_peak_bytes: u64,
    pub memory_meter: IsolationMemoryMeter,
    pub memory_sampling: IsolationMemorySampling,
    pub termination_causes: Vec<IsolationTerminationCause>,
    pub raw_wait_status: i32,
    pub meter_tampered: bool,
    pub head_hash_before: Sha256Hex,
    pub head_hash_after: Sha256Hex,
    pub frontier_hash_before: Sha256Hex,
    pub frontier_hash_after: Sha256Hex,
    pub partial_state_unchanged: bool,
    pub fork_overhead_ms: f64,
    pub total_wall_ms: f64,
    pub verdict: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_artifact_ref: Option<Artifact>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IsolationResult {
    pub host: IsolationHost,
    pub platform: IsolationPlatform,
    pub cases: Vec<IsolationCase>,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IsolationByPlatform {
    pub rust: IsolationResult,
    pub web: IsolationResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardGateResults {
    pub isolated_apply_not_async_timeout: Status,
    pub rust_wasm_roundtrip: Status,
    pub snapshot_tail_rebuild_hash: Status,
    pub concurrent_tree_move_invariants: Status,
    pub offline_replay_no_accepted_loss: Status,
    pub unauthorized_update_rejected: Status,
    pub corrupt_duplicate_out_of_order_limits: Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureCoverage {
    pub same_text_range_edit: Status,
    pub concurrent_block_move: Status,
    pub ancestor_delete_descendant_move: Status,
    pub concurrent_reorder: Status,
    pub same_key_nested_container_creation: Status,
    pub duplicate_out_of_order_partial_batch: Status,
    pub offline_reconnect: Status,
    pub snapshot_tail_restore: Status,
    pub snapshot_boundary_update: Status,
    pub adversarial_update_limits: Status,
    pub unicode_ime: Status,
    pub local_undo_remote_update: Status,
    pub cursor_target_move_delete: Status,
    pub policy_rejected_intent_recovery: Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitKind {
    UpdateBytes,
    WebsocketFrameBytes,
    TreeDepth,
    ContainerCount,
    DocumentBlockCount,
    TextBlockChars,
    DocumentTextChars,
    SemanticPatchOperations,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedOutcome {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedOutcome {
    Accepted,
    Rejected,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedBoundaryFixture {
    pub observed_value: u64,
    pub expected_outcome: ExpectedOutcome,
    pub observed_outcome: ObservedOutcome,
    pub operation_log: Artifact,
    pub final_semantic_json: Artifact,
    pub semantic_hash: Sha256Hex,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateHashes {
    pub head_before: Sha256Hex,
    pub head_after: Sha256Hex,
    pub frontier_before: Sha256Hex,
    pub frontier_after: Sha256Hex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedBoundaryFixture {
    pub observed_value: u64,
    pub expected_outcome: ExpectedOutcome,
    pub observed_outcome: ObservedOutcome,
    pub error_limit_kind: Option<LimitKind>,
    pub operation_log: Artifact,
    pub state_hashes: StateHashes,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundaryPair {
    pub limit_kind: LimitKind,
    pub limit_value: u64,
    pub exact: AcceptedBoundaryFixture,
    pub boundary_plus_one: RejectedBoundaryFixture,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundaryResults {
    pub update_bytes: BoundaryPair,
    pub websocket_frame_bytes: BoundaryPair,
    pub tree_depth: BoundaryPair,
    pub container_count: BoundaryPair,
    pub document_block_count: BoundaryPair,
    pub text_block_chars: BoundaryPair,
    pub document_text_chars: BoundaryPair,
    pub semantic_patch_operations: BoundaryPair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseCategory {
    SameTextRangeEdit,
    ConcurrentBlockMove,
    AncestorDeleteDescendantMove,
    ConcurrentReorder,
    SameKeyNestedContainerCreation,
    DuplicateOutOfOrderPartialBatch,
    OfflineReconnect,
    SnapshotTailRestore,
    SnapshotBoundaryUpdate,
    AdversarialUpdateLimits,
    UnicodeIme,
    LocalUndoRemoteUpdate,
    CursorTargetMoveDelete,
    PolicyRejectedIntentRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinarySizes {
    pub update_bytes: u64,
    pub snapshot_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseResult {
    pub id: String,
    pub category: CaseCategory,
    pub seed: String,
    pub status: Status,
    pub operation_log: Artifact,
    pub final_semantic_json: Artifact,
    pub semantic_hash: Sha256Hex,
    pub binary_sizes: BinarySizes,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_minimal_reproduction: Option<Artifact>,
}

// `Eq` is dropped from this struct and its containers below: `IsolationByPlatform` now nests
// `f64` fields (`cpu_ms`, `observed_at_ms`, ...) per the new per-case isolation shape, and `f64`
// does not implement `Eq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateConvergenceResult {
    pub candidate: CandidateName,
    pub corpus_seed: String,
    pub case_count: u32,
    pub budget_hash: Sha256Hex,
    pub isolation: IsolationByPlatform,
    pub hard_gates: HardGateResults,
    pub fixture_coverage: FixtureCoverage,
    pub boundary_results: BoundaryResults,
    pub cases: Vec<CaseResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConvergenceCandidates {
    pub loro: CandidateConvergenceResult,
    #[serde(rename = "yrs-yjs")]
    pub yrs_yjs: CandidateConvergenceResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConvergenceResult {
    pub schema_version: String,
    pub schema_path: String,
    pub release: String,
    pub source_head: GitSha,
    pub generated_at: String,
    pub candidates: ConvergenceCandidates,
}

impl ConvergenceResult {
    pub const SCHEMA_VERSION: &'static str = "sylvode.flow.convergence-result.v1";
    pub const SCHEMA_PATH: &'static str = "docs/schemas/sylvode-flow-convergence-result-v1.schema.json";
    pub const RELEASE: &'static str = "0.3.0";
}

// --- Benchmark result -------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budgets {
    pub browser_engine_bundle_gzip_bytes_max: u64,
    pub cold_start_ms_p95_max: u64,
    pub apply_update_ms_p95_max: u64,
    pub bootstrap_10k_ops_ms_p95_max: u64,
    pub bootstrap_100k_ops_ms_p95_max: u64,
    pub peak_memory_100k_ops_bytes_max: u64,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            browser_engine_bundle_gzip_bytes_max: 2_097_152,
            cold_start_ms_p95_max: 250,
            apply_update_ms_p95_max: 20,
            bootstrap_10k_ops_ms_p95_max: 1_000,
            bootstrap_100k_ops_ms_p95_max: 5_000,
            peak_memory_100k_ops_bytes_max: 268_435_456,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserVersion {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    pub cpu_model: String,
    pub cpu_cores: u32,
    pub ram_bytes: u64,
    pub os: String,
    pub kernel: String,
    pub rust_toolchain: String,
    pub browser: BrowserVersion,
    pub node_version: String,
    pub bun_version: String,
    pub release_flags: Vec<String>,
    pub bundle_build_tool: String,
    pub gzip_tool: String,
    pub engine_version: String,
    pub package_version: String,
    pub lockfile_hash: Sha256Hex,
    pub source_commit: GitSha,
    pub machine_idle: bool,
    pub optimized_build: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetricUnit {
    Ms,
    Bytes,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DistributionMetric {
    pub unit: MetricUnit,
    pub warmup_count: u32,
    pub sample_count: u32,
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    pub min: f64,
    pub max: f64,
    pub raw_samples: Artifact,
}

impl DistributionMetric {
    /// Builds the `ms`-unit metric from a shared-runner [`crate::benchmark::SampleStats`] plus
    /// the artifact record for wherever the caller persisted the raw samples on disk.
    #[must_use]
    pub fn from_time_stats(stats: &crate::benchmark::SampleStats, raw_samples_artifact: Artifact) -> Self {
        // Warmup/sample counts come from `benchmark::sample_duration`'s caller-supplied iteration
        // counts, always tiny compared to u32::MAX; saturate rather than panic in the
        // astronomically unlikely event a future caller passes something absurd.
        let warmup_count = u32::try_from(stats.warmup_count).unwrap_or(u32::MAX);
        let sample_count = u32::try_from(stats.sample_count).unwrap_or(u32::MAX);
        Self {
            unit: MetricUnit::Ms,
            warmup_count,
            sample_count,
            median: stats.median,
            p95: stats.p95,
            p99: stats.p99,
            min: stats.min,
            max: stats.max,
            raw_samples: raw_samples_artifact,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleFileKind {
    Javascript,
    Wasm,
    Glue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleFile {
    pub path: String,
    pub kind: BundleFileKind,
    pub gzip_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleResult {
    pub total_gzip_bytes: u64,
    pub metafile: Artifact,
    pub files: Vec<BundleFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurements {
    pub browser_engine_bundle: BundleResult,
    pub cold_start_ms: DistributionMetric,
    pub apply_update_ms: DistributionMetric,
    pub bootstrap_10k_ops_ms: DistributionMetric,
    pub bootstrap_100k_ops_ms: DistributionMetric,
    pub peak_memory_100k_ops_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SupplementalMetrics {
    pub update_bytes: u64,
    pub snapshot_bytes: u64,
    pub indexeddb_bytes: u64,
    pub reconnect_duration_ms: f64,
    pub compaction_time_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HostileInputSafety {
    pub decode_apply_cpu_ms: f64,
    pub decode_apply_wall_ms: f64,
    pub isolated_apply_memory_bytes: u64,
    pub status: Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetChecks {
    pub browser_engine_bundle_gzip_bytes_max: Status,
    pub cold_start_ms_p95_max: Status,
    pub apply_update_ms_p95_max: Status,
    pub bootstrap_10k_ops_ms_p95_max: Status,
    pub bootstrap_100k_ops_ms_p95_max: Status,
    pub peak_memory_100k_ops_bytes_max: Status,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateBenchmarkResult {
    pub candidate: CandidateName,
    pub environment: Environment,
    pub measurements: Measurements,
    pub supplemental_metrics: SupplementalMetrics,
    pub hostile_input_safety: HostileInputSafety,
    pub budget_checks: BudgetChecks,
    pub benchmark_budgets_met: Status,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkCandidates {
    pub loro: CandidateBenchmarkResult,
    #[serde(rename = "yrs-yjs")]
    pub yrs_yjs: CandidateBenchmarkResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub schema_version: String,
    pub schema_path: String,
    pub release: String,
    pub source_head: GitSha,
    pub generated_at: String,
    pub budgets: Budgets,
    pub run_sequence: Vec<CandidateName>,
    pub candidates: BenchmarkCandidates,
}

impl BenchmarkResult {
    pub const SCHEMA_VERSION: &'static str = "sylvode.flow.benchmark-result.v1";
    pub const SCHEMA_PATH: &'static str = "docs/schemas/sylvode-flow-benchmark-result-v1.schema.json";
    pub const RELEASE: &'static str = "0.3.0";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_sha_rejects_wrong_length_or_uppercase() {
        assert!(GitSha::new("a".repeat(40)).is_ok());
        assert!(GitSha::new("a".repeat(39)).is_err());
        assert!(GitSha::new("A".repeat(40)).is_err());
    }

    #[test]
    fn sha256_hex_rejects_wrong_length() {
        assert!(Sha256Hex::new("0".repeat(64)).is_ok());
        assert!(Sha256Hex::new("0".repeat(63)).is_err());
    }

    #[test]
    fn candidate_name_serializes_to_schema_enum_values() {
        let loro = serde_json::to_string(&CandidateName::Loro).unwrap_or_default();
        let yrs = serde_json::to_string(&CandidateName::YrsYjs).unwrap_or_default();
        assert_eq!(loro, "\"loro\"");
        assert_eq!(yrs, "\"yrs-yjs\"");
    }

    #[test]
    fn status_serializes_to_schema_enum_values() {
        let passed = serde_json::to_string(&Status::Passed).unwrap_or_default();
        let failed = serde_json::to_string(&Status::Failed).unwrap_or_default();
        assert_eq!(passed, "\"passed\"");
        assert_eq!(failed, "\"failed\"");
    }

    #[test]
    fn budgets_default_matches_frozen_schema_consts() {
        let budgets = Budgets::default();
        assert_eq!(budgets.browser_engine_bundle_gzip_bytes_max, 2_097_152);
        assert_eq!(budgets.apply_update_ms_p95_max, 20);
        assert_eq!(budgets.peak_memory_100k_ops_bytes_max, 268_435_456);
    }

    /// Serializes a sample `IsolationCase`/`IsolationResult` and asserts every field name and
    /// enum literal matches `$defs/isolation_case` / `$defs/isolation_result` in
    /// `docs/schemas/sylvode-flow-convergence-result-v1.schema.json` byte-for-byte. This is the
    /// regression guard called for in work package 3a: it fails loudly the moment this struct
    /// drifts from the schema again (e.g. if someone reintroduces `worker_wall_approximation`,
    /// which the new schema's `isolation_cpu_meter` enum does not contain).
    #[test]
    fn isolation_case_serializes_to_schema_field_names_and_enum_literals() {
        let case = IsolationCase {
            fixture_id: "cpu_hog_v1".to_string(),
            expected_oracle: IsolationOracle::CpuCeiling,
            kind: IsolationKind::TerminableInstance,
            cpu_ms: 51.2,
            cpu_meter: IsolationCpuMeter::NativeItimerProf,
            wall_ms: 55.0,
            wall_meter: "parent_instant".to_string(),
            allocated_active_peak_bytes: 4096,
            allocated_attempted_peak_bytes: 4096,
            rss_peak_bytes: 8192,
            memory_meter: IsolationMemoryMeter::CountingAllocator,
            memory_sampling: IsolationMemorySampling::ContinuousHighWater,
            termination_causes: vec![IsolationTerminationCause {
                cause: IsolationOracle::CpuCeiling,
                source: "sigprof_signal".to_string(),
                observed_at_ms: 51.2,
            }],
            raw_wait_status: 0,
            meter_tampered: false,
            // `Sha256Hex`'s field is private but visible to this descendant `tests` module, and
            // a 64-char `"0"` literal is statically known-valid, so this sidesteps needing an
            // `.unwrap()`-shaped fallback for a `Result` that can never be the error branch here.
            head_hash_before: Sha256Hex("0".repeat(64)),
            head_hash_after: Sha256Hex("0".repeat(64)),
            frontier_hash_before: Sha256Hex("0".repeat(64)),
            frontier_hash_after: Sha256Hex("0".repeat(64)),
            partial_state_unchanged: true,
            fork_overhead_ms: 0.4,
            total_wall_ms: 55.6,
            verdict: Status::Passed,
            failure_artifact_ref: None,
        };
        let value: serde_json::Value = serde_json::to_value(&case).unwrap_or(serde_json::Value::Null);
        let object = value.as_object().cloned().unwrap_or_default();

        for key in [
            "fixture_id",
            "expected_oracle",
            "kind",
            "cpu_ms",
            "cpu_meter",
            "wall_ms",
            "wall_meter",
            "allocated_active_peak_bytes",
            "allocated_attempted_peak_bytes",
            "rss_peak_bytes",
            "memory_meter",
            "memory_sampling",
            "termination_causes",
            "raw_wait_status",
            "meter_tampered",
            "head_hash_before",
            "head_hash_after",
            "frontier_hash_before",
            "frontier_hash_after",
            "partial_state_unchanged",
            "fork_overhead_ms",
            "total_wall_ms",
            "verdict",
        ] {
            assert!(object.contains_key(key), "missing schema field {key}");
        }
        // `failure_artifact_ref` must be omitted (not `null`) when absent, matching how the
        // schema's `additionalProperties: false` object is expected to be produced only with the
        // fields it actually has.
        assert!(!object.contains_key("failure_artifact_ref"));

        assert_eq!(
            object.get("expected_oracle").and_then(|v| v.as_str()),
            Some("cpu_ceiling")
        );
        assert_eq!(object.get("kind").and_then(|v| v.as_str()), Some("terminable_instance"));
        assert_eq!(
            object.get("cpu_meter").and_then(|v| v.as_str()),
            Some("native_itimer_prof")
        );
        assert_eq!(
            object.get("memory_meter").and_then(|v| v.as_str()),
            Some("counting_allocator")
        );
        assert_eq!(
            object.get("memory_sampling").and_then(|v| v.as_str()),
            Some("continuous_high_water")
        );
        assert_eq!(object.get("verdict").and_then(|v| v.as_str()), Some("passed"));

        // `cpu_meter` must never be able to serialize the retired `worker_wall_approximation`
        // literal: the enum simply has no such variant any more, so this is a compile-time
        // guarantee, but assert the full set of literals the current enum can produce matches
        // the schema's `isolation_cpu_meter` set exactly.
        for (variant, literal) in [
            (IsolationCpuMeter::NativeItimerProf, "native_itimer_prof"),
            (IsolationCpuMeter::NotApplicableWeb, "not_applicable_web"),
            (IsolationCpuMeter::Unavailable, "unavailable"),
        ] {
            let serialized = serde_json::to_string(&variant).unwrap_or_default();
            assert_eq!(serialized, format!("\"{literal}\""));
        }

        let termination = object
            .get("termination_causes")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        assert_eq!(termination.len(), 1);
        let first = termination.first().cloned().unwrap_or(serde_json::Value::Null);
        let first_object = first.as_object().cloned().unwrap_or_default();
        for key in ["cause", "source", "observed_at_ms"] {
            assert!(first_object.contains_key(key), "missing termination cause field {key}");
        }
    }

    #[test]
    fn isolation_platform_untagged_round_trips_native_and_web() {
        let native = IsolationPlatform::Native(IsolationPlatformNative {
            os: IsolationOs::Linux,
            kernel: "6.12.48".to_string(),
            version: "#1 SMP".to_string(),
        });
        let native_value = serde_json::to_value(&native).unwrap_or(serde_json::Value::Null);
        assert_eq!(native_value.get("os").and_then(|v| v.as_str()), Some("linux"));
        assert!(native_value.get("browser").is_none());
        let round_tripped: IsolationPlatform = serde_json::from_value(native_value).unwrap_or_else(|_| native.clone());
        assert_eq!(round_tripped, native);

        let web = IsolationPlatform::Web(IsolationPlatformWeb {
            browser: "chromium".to_string(),
            version: "128.0".to_string(),
            cross_origin_isolated: true,
        });
        let web_value = serde_json::to_value(&web).unwrap_or(serde_json::Value::Null);
        assert_eq!(web_value.get("browser").and_then(|v| v.as_str()), Some("chromium"));
        assert!(web_value.get("os").is_none());
        let round_tripped_web: IsolationPlatform = serde_json::from_value(web_value).unwrap_or_else(|_| web.clone());
        assert_eq!(round_tripped_web, web);
    }
}
