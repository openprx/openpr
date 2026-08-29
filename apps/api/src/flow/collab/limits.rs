//! Transport/server-side numeric ceilings from `contracts/limits-v1.md`, frozen v0.4 values only.
//!
//! Deliberately plain constants, not configuration: `ADR-0010` requires these to be enforced as
//! real LRU eviction / real byte checks, "不得通过关闭 eviction 或无界提高配置来过 gate" — making
//! them configurable would be exactly that escape hatch.

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

/// Bounded rebase retry count (`collab-protocol-v1.md`: "最多 3 次锁外 rebase").
pub const MAX_REBASE_ATTEMPTS: u32 = 3;

/// `collab_tickets` TTL (`ADR-0007`: "TTL 固定 60 秒,不可续期").
pub const TICKET_TTL_SECONDS: i64 = 60;

/// Coordinator acquisition timeout.
///
/// Not itself a frozen `limits-v1.md` row (the coordinator is explicitly "not part of the DB lock
/// rank", `ADR-0010`'s 第 0 层); bounded so a stuck peer holder cannot wedge every other writer for
/// this document forever. Kept below the DB lock-wait ceiling so a coordinator timeout always
/// surfaces before a DB-level one could.
pub const COORDINATOR_ACQUIRE_TIMEOUT_MS: u64 = 500;
