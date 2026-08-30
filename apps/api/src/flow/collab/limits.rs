//! Transport/server-side numeric ceilings from `contracts/limits-v1.md`, frozen v0.4 values only.
//!
//! Deliberately plain constants, not configuration: `ADR-0010` requires these to be enforced as
//! real LRU eviction / real byte checks, "不得通过关闭 eviction 或无界提高配置来过 gate" — making
//! them configurable would be exactly that escape hatch.

#![allow(clippy::too_long_first_doc_paragraph)]

/// `websocket_frame_bytes_max` — checked on the raw frame before any decode.
pub const WEBSOCKET_FRAME_BYTES_MAX: usize = 131_072;

/// `presence_ttl_seconds_max`; the server default when a `presence` frame omits `ttl_seconds`.
pub const PRESENCE_TTL_SECONDS_MAX: u32 = 30;
pub const PRESENCE_TTL_SECONDS_DEFAULT: u32 = 30;

/// `presence_entries_per_connection_max` / `presence_entries_per_document_max`.
pub const PRESENCE_ENTRIES_PER_CONNECTION_MAX: usize = 8;
pub const PRESENCE_ENTRIES_PER_DOCUMENT_MAX: usize = 100;

/// `warm_cache_documents_per_instance_max`.
pub const WARM_CACHE_DOCUMENTS_MAX: usize = 64;
/// `warm_cache_decoded_bytes_per_instance_max` (512 MiB).
pub const WARM_CACHE_DECODED_BYTES_MAX: u64 = 536_870_912;
/// `warm_cache_entry_decoded_bytes_max` (128 MiB).
pub const WARM_CACHE_ENTRY_DECODED_BYTES_MAX: u64 = 134_217_728;
/// `warm_cache_idle_ttl_seconds`.
pub const WARM_CACHE_IDLE_TTL_SECONDS: u64 = 120;

/// `document_lock_wait_ms_max` — `SET LOCAL lock_timeout` budget for the row lock acquisition.
pub const DOCUMENT_LOCK_WAIT_MS_MAX: u64 = 100;
/// `document_lock_hold_ms_max` — hard per-transaction deadline (`SET LOCAL statement_timeout`
/// budget for the locked portion of the write transaction).
pub const DOCUMENT_LOCK_HOLD_MS_MAX: u64 = 100;

/// Bounded rebase retry count (`collab-protocol-v1.md`: "最多 3 次锁外 rebase"). Also the
/// `document_prepare_rebase_attempts_max` budget snapshot advancement's own boundary/head-mismatch
/// retry reuses (`flow::collab::snapshot::advance`) — both are the identical "head mismatch 后在
/// 锁外重建...三次仍竞争则临时退避" shape.
pub const MAX_REBASE_ATTEMPTS: u32 = 3;

// ---- Snapshot advancement (gate 7 `minimal_snapshot_advancement_bounds_tail`) ----
//
// `snapshot_tail_updates_soft_max` / `snapshot_tail_bytes_soft_max` /
// `snapshot_rebuild_wall_ms_p95_soft_max` / `snapshot_tail_updates_hard_max` /
// `snapshot_tail_bytes_hard_max` — server persistence path budgets, not part of the
// `FlowLimitsV1` wire schema (`limits-v1.md`: "这些是 ADR-0010 的内部架构预算,不是 caller
// payload validity"). `flow::collab::snapshot` is the only module that reads these.

/// `snapshot_tail_updates_soft_max`.
pub const SNAPSHOT_TAIL_UPDATES_SOFT_MAX: i64 = 256;
/// `snapshot_tail_bytes_soft_max` (1 MiB).
pub const SNAPSHOT_TAIL_BYTES_SOFT_MAX: i64 = 1_048_576;
/// `snapshot_rebuild_wall_ms_p95_soft_max` — here applied per-measurement (the most recent
/// [`crate::flow::collab::snapshot::Candidate::rebuild_wall_ms`] for a document), not as a
/// computed p95 series; `flow::collab::snapshot::evaluate`'s doc comment explains why a single
/// slow rebuild is treated as sufficient signal to schedule the next one.
pub const SNAPSHOT_REBUILD_WALL_MS_SOFT_MAX: u64 = 100;
/// `snapshot_tail_updates_hard_max` — "接受下一 update 前必须先成功推进 snapshot,不能继续扩大
/// tail".
pub const SNAPSHOT_TAIL_UPDATES_HARD_MAX: i64 = 1_024;
/// `snapshot_tail_bytes_hard_max` (4 MiB).
pub const SNAPSHOT_TAIL_BYTES_HARD_MAX: i64 = 4_194_304;

/// `collab_tickets` TTL (`ADR-0007`: "TTL 固定 60 秒,不可续期").
pub const TICKET_TTL_SECONDS: i64 = 60;

/// Coordinator acquisition timeout.
///
/// Not itself a frozen `limits-v1.md` row (the coordinator is explicitly "not part of the DB lock
/// rank", `ADR-0010`'s 第 0 层); bounded so a stuck peer holder cannot wedge every other writer for
/// this document forever. Kept below the DB lock-wait ceiling so a coordinator timeout always
/// surfaces before a DB-level one could.
pub const COORDINATOR_ACQUIRE_TIMEOUT_MS: u64 = 500;

// ---- `Bootstrap.limits` wire schema (`limits-v1.md` "Bootstrap.limits wire schema") ----
//
// The rest of this file's constants back the pieces of `limits-v1.md` already enforced
// elsewhere (WebSocket frame codec, presence, warm cache, lock budgets); the constants below are
// the remaining frozen v0.4 ceilings that only exist, until now, as this table's own rows — added
// so `Bootstrap.limits` can report the "effective、完整且不可空" `FlowLimitsV1` structure the
// contract requires, not an endpoint-local partial reconstruction of it.

/// `update_bytes_max` — mirrors `collab_core::error::InputLimits::default().update_bytes_max`
/// (that crate cannot itself depend on this one), kept here only for `FlowLimitsV1`'s wire report.
pub const UPDATE_BYTES_MAX: u64 = 65_536;
/// `presence_payload_bytes_max`.
pub const PRESENCE_PAYLOAD_BYTES_MAX: u64 = 8_192;
/// `bootstrap_decoded_bytes_max` — checked against `snapshot.len() + sum(tail update bytes)`
/// before a bootstrap response is built.
pub const BOOTSTRAP_DECODED_BYTES_MAX: u64 = 8_388_608;
/// `bootstrap_response_bytes_max`.
pub const BOOTSTRAP_RESPONSE_BYTES_MAX: u64 = 12_582_912;
/// `tree_depth_max`.
pub const TREE_DEPTH_MAX: u64 = 32;
/// `container_count_max`.
pub const CONTAINER_COUNT_MAX: u64 = 10_000;
/// `document_block_count_max`.
pub const DOCUMENT_BLOCK_COUNT_MAX: u64 = 10_000;
/// `text_block_chars_max`.
pub const TEXT_BLOCK_CHARS_MAX: u64 = 100_000;
/// `document_text_chars_max`.
pub const DOCUMENT_TEXT_CHARS_MAX: u64 = 1_000_000;
/// `semantic_patch_operations_max`.
pub const SEMANTIC_PATCH_OPERATIONS_MAX: u64 = 100;
/// `semantic_patch_json_bytes_max`.
pub const SEMANTIC_PATCH_JSON_BYTES_MAX: u64 = 1_048_576;
/// `decode_apply_cpu_ms_max`.
pub const DECODE_APPLY_CPU_MS_MAX: u64 = 50;
/// `decode_apply_wall_ms_max`.
pub const DECODE_APPLY_WALL_MS_MAX: u64 = 100;
/// `isolated_apply_memory_bytes_max`.
pub const ISOLATED_APPLY_MEMORY_BYTES_MAX: u64 = 134_217_728;
/// `open_documents_per_connection_max`.
pub const OPEN_DOCUMENTS_PER_CONNECTION_MAX: u64 = 8;
/// `connections_per_user_max`.
pub const CONNECTIONS_PER_USER_MAX: u64 = 16;
/// `connections_per_document_max`.
pub const CONNECTIONS_PER_DOCUMENT_MAX: u64 = 100;
/// `connections_per_workspace_max`.
pub const CONNECTIONS_PER_WORKSPACE_MAX: u64 = 500;
/// `frames_per_connection_per_second` / `frame_burst_max`.
pub const FRAMES_PER_CONNECTION_PER_SECOND: u64 = 30;
pub const FRAME_BURST_MAX: u64 = 60;
/// `updates_per_connection_per_second` / `update_burst_max`.
pub const UPDATES_PER_CONNECTION_PER_SECOND: u64 = 10;
pub const UPDATE_BURST_MAX: u64 = 20;
/// `slow_consumer_queue_frames_max` / `slow_consumer_queue_bytes_max`.
pub const SLOW_CONSUMER_QUEUE_FRAMES_MAX: u64 = 256;
pub const SLOW_CONSUMER_QUEUE_BYTES_MAX: u64 = 8_388_608;
/// `page_limit_default` / `page_limit_max` — matches `flow::query::DEFAULT_LIST_LIMIT`/
/// `MAX_LIST_LIMIT` (kept as separate constants here so this module does not depend on `query`).
pub const PAGE_LIMIT_DEFAULT: u64 = 50;
pub const PAGE_LIMIT_MAX: u64 = 100;
/// `authorized_scan_rows_max`.
pub const AUTHORIZED_SCAN_ROWS_MAX: u64 = 1_000;
/// `import_archive_bytes_max`.
pub const IMPORT_ARCHIVE_BYTES_MAX: u64 = 134_217_728;
/// `import_expanded_bytes_max`.
pub const IMPORT_EXPANDED_BYTES_MAX: u64 = 536_870_912;
/// `import_entry_count_max`.
pub const IMPORT_ENTRY_COUNT_MAX: u64 = 100_000;
/// `import_compression_ratio_max`.
pub const IMPORT_COMPRESSION_RATIO_MAX: u64 = 100;

/// `FlowLimitsV1` (`limits-v1.md` "Bootstrap.limits wire schema"): the complete, non-empty,
/// effective ceiling set every `Bootstrap` response must return verbatim — never partially
/// assembled by an endpoint.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct FlowLimitsV1 {
    pub version: &'static str,
    pub update_bytes_max: u64,
    pub websocket_frame_bytes_max: u64,
    pub presence_payload_bytes_max: u64,
    pub presence_ttl_seconds_max: u32,
    pub bootstrap_decoded_bytes_max: u64,
    pub bootstrap_response_bytes_max: u64,
    pub tree_depth_max: u64,
    pub container_count_max: u64,
    pub document_block_count_max: u64,
    pub text_block_chars_max: u64,
    pub document_text_chars_max: u64,
    pub semantic_patch_operations_max: u64,
    pub semantic_patch_json_bytes_max: u64,
    pub decode_apply_cpu_ms_max: u64,
    pub decode_apply_wall_ms_max: u64,
    pub isolated_apply_memory_bytes_max: u64,
    pub open_documents_per_connection_max: u64,
    pub connections_per_user_max: u64,
    pub connections_per_document_max: u64,
    pub connections_per_workspace_max: u64,
    pub presence_entries_per_connection_max: u64,
    pub presence_entries_per_document_max: u64,
    pub frames_per_connection_per_second: u64,
    pub frame_burst_max: u64,
    pub updates_per_connection_per_second: u64,
    pub update_burst_max: u64,
    pub slow_consumer_queue_frames_max: u64,
    pub slow_consumer_queue_bytes_max: u64,
    pub page_limit_default: u64,
    pub page_limit_max: u64,
    pub authorized_scan_rows_max: u64,
    pub import_archive_bytes_max: u64,
    pub import_expanded_bytes_max: u64,
    pub import_entry_count_max: u64,
    pub import_compression_ratio_max: u64,
}

/// Builds the [`collab_core::DocumentLimits`] structural ceiling set from this module's own
/// frozen constants — the single source both the REST content-command path
/// (`flow::command::apply_content_command`, via [`collab_core::limits::check_operation`] /
/// [`collab_core::limits::check_operation_batch_count`]) and the WebSocket write path
/// (`flow::collab::write::hydrate_and_apply`, via [`collab_core::limits::check_snapshot`]) use, so
/// neither call site can silently drift from the other or from `Bootstrap.limits`'s own wire
/// report above.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub const fn document_limits() -> collab_core::DocumentLimits {
    collab_core::DocumentLimits {
        update_bytes_max: UPDATE_BYTES_MAX as usize,
        tree_depth_max: TREE_DEPTH_MAX as usize,
        container_count_max: CONTAINER_COUNT_MAX as usize,
        document_block_count_max: DOCUMENT_BLOCK_COUNT_MAX as usize,
        text_block_chars_max: TEXT_BLOCK_CHARS_MAX as usize,
        document_text_chars_max: DOCUMENT_TEXT_CHARS_MAX as usize,
        semantic_patch_operations_max: SEMANTIC_PATCH_OPERATIONS_MAX as usize,
    }
}

/// Builds the effective `FlowLimitsV1` from this module's own frozen constants — the single
/// source `Bootstrap.limits` (and any future limits-reporting surface) must call, so the wire
/// value can never drift from the constants this package actually enforces.
pub const fn effective_limits() -> FlowLimitsV1 {
    FlowLimitsV1 {
        version: "sylvode.flow.limits.v1",
        update_bytes_max: UPDATE_BYTES_MAX,
        websocket_frame_bytes_max: WEBSOCKET_FRAME_BYTES_MAX as u64,
        presence_payload_bytes_max: PRESENCE_PAYLOAD_BYTES_MAX,
        presence_ttl_seconds_max: PRESENCE_TTL_SECONDS_MAX,
        bootstrap_decoded_bytes_max: BOOTSTRAP_DECODED_BYTES_MAX,
        bootstrap_response_bytes_max: BOOTSTRAP_RESPONSE_BYTES_MAX,
        tree_depth_max: TREE_DEPTH_MAX,
        container_count_max: CONTAINER_COUNT_MAX,
        document_block_count_max: DOCUMENT_BLOCK_COUNT_MAX,
        text_block_chars_max: TEXT_BLOCK_CHARS_MAX,
        document_text_chars_max: DOCUMENT_TEXT_CHARS_MAX,
        semantic_patch_operations_max: SEMANTIC_PATCH_OPERATIONS_MAX,
        semantic_patch_json_bytes_max: SEMANTIC_PATCH_JSON_BYTES_MAX,
        decode_apply_cpu_ms_max: DECODE_APPLY_CPU_MS_MAX,
        decode_apply_wall_ms_max: DECODE_APPLY_WALL_MS_MAX,
        isolated_apply_memory_bytes_max: ISOLATED_APPLY_MEMORY_BYTES_MAX,
        open_documents_per_connection_max: OPEN_DOCUMENTS_PER_CONNECTION_MAX,
        connections_per_user_max: CONNECTIONS_PER_USER_MAX,
        connections_per_document_max: CONNECTIONS_PER_DOCUMENT_MAX,
        connections_per_workspace_max: CONNECTIONS_PER_WORKSPACE_MAX,
        presence_entries_per_connection_max: PRESENCE_ENTRIES_PER_CONNECTION_MAX as u64,
        presence_entries_per_document_max: PRESENCE_ENTRIES_PER_DOCUMENT_MAX as u64,
        frames_per_connection_per_second: FRAMES_PER_CONNECTION_PER_SECOND,
        frame_burst_max: FRAME_BURST_MAX,
        updates_per_connection_per_second: UPDATES_PER_CONNECTION_PER_SECOND,
        update_burst_max: UPDATE_BURST_MAX,
        slow_consumer_queue_frames_max: SLOW_CONSUMER_QUEUE_FRAMES_MAX,
        slow_consumer_queue_bytes_max: SLOW_CONSUMER_QUEUE_BYTES_MAX,
        page_limit_default: PAGE_LIMIT_DEFAULT,
        page_limit_max: PAGE_LIMIT_MAX,
        authorized_scan_rows_max: AUTHORIZED_SCAN_ROWS_MAX,
        import_archive_bytes_max: IMPORT_ARCHIVE_BYTES_MAX,
        import_expanded_bytes_max: IMPORT_EXPANDED_BYTES_MAX,
        import_entry_count_max: IMPORT_ENTRY_COUNT_MAX,
        import_compression_ratio_max: IMPORT_COMPRESSION_RATIO_MAX,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::effective_limits;

    /// `limits-v1.md`: "Bootstrap 必须返回 effective、完整且不可空的结构" — pins the wire field
    /// count and a couple of values so a future edit here cannot silently drop a field without a
    /// test failing.
    #[test]
    fn effective_limits_serializes_every_frozen_field_non_null() {
        let value = serde_json::to_value(effective_limits()).expect("FlowLimitsV1 serializes");
        let object = value.as_object().expect("FlowLimitsV1 serializes as a JSON object");
        assert_eq!(
            object.len(),
            36,
            "one field per limits-v1.md wire schema row plus `version`"
        );
        for (key, field_value) in object {
            assert!(!field_value.is_null(), "field '{key}' must not be null");
        }
        assert_eq!(value["version"], "sylvode.flow.limits.v1");
        assert_eq!(value["update_bytes_max"], 65_536);
        assert_eq!(value["bootstrap_decoded_bytes_max"], 8_388_608);
        assert_eq!(value["import_compression_ratio_max"], 100);
    }
}
