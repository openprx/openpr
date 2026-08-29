//! The bounded per-document warm cache (`ADR-0010`, decision B).
//!
//! Budgets are the frozen [`super::limits`] numbers, enforced as real LRU eviction — never by
//! disabling eviction or raising a config knob (`ADR-0010`: "不能通过关闭 eviction 或无界提高配置来
//! 过 gate").
//!
//! Concurrency shape (this crate's iron rules + `ADR-0010`'s "写入算法"): the registry itself is a
//! `parking_lot::Mutex`, a short synchronous critical section per call — [`WarmCache::fork_matching`]
//! and [`WarmCache::put`] never hold the guard across an `.await` (there is no `.await` inside
//! either; [`collab_core::LoroCollabEngine::fork`] is CPU-only). The cache is an accelerator, not
//! an authority (`ADR-0010`): [`WarmCache::fork_matching`] hands the caller an independent forked
//! copy while leaving the registry's own entry untouched, so the caller can do the
//! (unbounded-duration) isolated-apply work below with no lock held at all; only a *successful,
//! committed* write ever replaces the registry's entry, via [`WarmCache::put`].

#![allow(clippy::too_long_first_doc_paragraph)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use collab_core::LoroCollabEngine;
use parking_lot::Mutex;
use uuid::Uuid;

use super::limits::{
    WARM_CACHE_DECODED_BYTES_MAX, WARM_CACHE_DOCUMENTS_MAX, WARM_CACHE_ENTRY_DECODED_BYTES_MAX,
    WARM_CACHE_IDLE_TTL_SECONDS,
};

/// One warm document (`ADR-0010`'s frozen entry shape): `document_id`/`engine`=`decoded_doc` are
/// the map key and the field below; `format_version`, `prepared_head_seq`,
/// `prepared_head_frontier`, `projection_input` (this package's write path recomputes projection
/// input from `engine` directly rather than caching it separately — see `write.rs`) and
/// `last_used` round out the rest.
pub struct CacheEntry {
    pub engine: LoroCollabEngine,
    pub format_version: String,
    pub prepared_head_seq: i64,
    pub prepared_head_frontier: Vec<u8>,
    /// Caller-supplied size accounting (production: the entry's `export_snapshot().len()`-order
    /// footprint). Not measured by this module — see [`WarmCache::put`]'s doc comment for why
    /// that is a deliberate, documented choice.
    decoded_bytes: u64,
    last_used: Instant,
}

struct Registry {
    entries: HashMap<Uuid, CacheEntry>,
    total_bytes: u64,
}

pub struct WarmCache {
    inner: Mutex<Registry>,
}

impl Default for WarmCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Outcome of [`WarmCache::put`], letting a caller (and a test) observe whether the entry it just
/// tried to cache actually ended up resident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutOutcome {
    Cached,
    /// The entry's own `decoded_bytes` exceeds [`WARM_CACHE_ENTRY_DECODED_BYTES_MAX`]
    /// (`ADR-0010`: "超过即不驻留并 fail closed/要求缩小或 snapshot,不得常驻更大 state"). The write
    /// this entry came from is unaffected — the cache is an accelerator, not an authority; a
    /// document just too large to cache always still bootstraps from the database on the next
    /// write.
    TooLargeToCache,
}

impl WarmCache {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Registry {
                entries: HashMap::new(),
                total_bytes: 0,
            }),
        }
    }

    /// Non-destructively forks the entry for `document_id` if it is live (not idle-expired) *and*
    /// still represents the caller's `observed_head_seq`/`observed_format_version` — the exact
    /// "hydrate to observed DB head" check `ADR-0010`'s write algorithm requires before an entry
    /// may be used as an isolated-apply base. A stale or absent entry returns `Ok(None)`, telling
    /// the caller to fall back to the full bootstrap loader.
    ///
    /// Deliberately non-destructive (unlike an earlier `take`-based design this module does not
    /// use): the only caller is the write path, which is itself serialized per document by
    /// [`super::coordinator::DocumentCoordinator`], so there is never a concurrent writer for the
    /// same `document_id` racing this fork — `put` (called once, after a successful commit) is
    /// the only thing that ever replaces this entry, so leaving it in place after a fork can never
    /// go stale mid-attempt in a way that matters: any subsequent attempt re-checks
    /// `observed_head_seq` against the database again before trusting the cache at all.
    ///
    /// # Errors
    /// Propagates [`collab_core::LoroCollabEngine::fork`]'s failure mode.
    #[allow(clippy::significant_drop_tightening)] // guard held through the fork, which is synchronous
    pub fn fork_matching(
        &self,
        document_id: Uuid,
        observed_head_seq: i64,
        observed_format_version: &str,
    ) -> Result<Option<LoroCollabEngine>, collab_core::CollabError> {
        let mut registry = self.inner.lock();
        evict_stale(&mut registry);
        let Some(entry) = registry.entries.get_mut(&document_id) else {
            return Ok(None);
        };
        if entry.prepared_head_seq != observed_head_seq || entry.format_version != observed_format_version {
            return Ok(None);
        }
        entry.last_used = Instant::now();
        Ok(Some(entry.engine.fork()?))
    }

    /// Inserts (or replaces) the entry for `document_id`, evicting by LRU until both the entry
    /// count and total byte budgets are satisfied.
    ///
    /// `decoded_bytes` is a caller-supplied size hint rather than something this module measures
    /// from `engine` itself: `collab_core::LoroCollabEngine` exposes no cheap in-memory size
    /// probe (the only way to learn it is `export_snapshot().len()`, an O(n) encode this hot path
    /// should not pay on every cache write). Production call sites compute it once, right after
    /// the export they already need for `collab_updates`/hydrate bookkeeping; tests may pass a
    /// synthetic value to exercise the byte ceiling without allocating that many real bytes — the
    /// eviction *logic* below is exactly the same code path either way.
    pub fn put(
        &self,
        document_id: Uuid,
        engine: LoroCollabEngine,
        format_version: String,
        prepared_head_seq: i64,
        prepared_head_frontier: Vec<u8>,
        decoded_bytes: u64,
    ) -> PutOutcome {
        if decoded_bytes > WARM_CACHE_ENTRY_DECODED_BYTES_MAX {
            return PutOutcome::TooLargeToCache;
        }
        let mut registry = self.inner.lock();
        evict_stale(&mut registry);

        if let Some(old) = registry.entries.remove(&document_id) {
            registry.total_bytes = registry.total_bytes.saturating_sub(old.decoded_bytes);
        }

        while registry.entries.len() >= WARM_CACHE_DOCUMENTS_MAX {
            if !evict_one_lru(&mut registry) {
                break;
            }
        }
        while registry.total_bytes.saturating_add(decoded_bytes) > WARM_CACHE_DECODED_BYTES_MAX {
            if !evict_one_lru(&mut registry) {
                break;
            }
        }

        registry.entries.insert(
            document_id,
            CacheEntry {
                engine,
                format_version,
                prepared_head_seq,
                prepared_head_frontier,
                decoded_bytes,
                last_used: Instant::now(),
            },
        );
        registry.total_bytes = registry.total_bytes.saturating_add(decoded_bytes);
        PutOutcome::Cached
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.inner.lock().total_bytes
    }

    #[must_use]
    pub fn contains(&self, document_id: Uuid) -> bool {
        self.inner.lock().entries.contains_key(&document_id)
    }
}

/// Removes every entry idle past [`WARM_CACHE_IDLE_TTL_SECONDS`]. Caller must hold `registry`'s
/// lock already (this is a private helper, never called with the lock unheld).
fn evict_stale(registry: &mut Registry) {
    let ttl = Duration::from_secs(WARM_CACHE_IDLE_TTL_SECONDS);
    let now = Instant::now();
    let stale: Vec<Uuid> = registry
        .entries
        .iter()
        .filter(|(_, entry)| now.duration_since(entry.last_used) >= ttl)
        .map(|(id, _)| *id)
        .collect();
    for id in stale {
        if let Some(entry) = registry.entries.remove(&id) {
            registry.total_bytes = registry.total_bytes.saturating_sub(entry.decoded_bytes);
        }
    }
}

/// Evicts the single least-recently-used entry. Returns `false` when the registry is empty (so a
/// caller's `while` loop cannot spin forever demanding more eviction than there is anything left
/// to evict).
fn evict_one_lru(registry: &mut Registry) -> bool {
    let oldest = registry
        .entries
        .iter()
        .min_by_key(|(_, entry)| entry.last_used)
        .map(|(id, _)| *id);
    let Some(id) = oldest else { return false };
    if let Some(entry) = registry.entries.remove(&id) {
        registry.total_bytes = registry.total_bytes.saturating_sub(entry.decoded_bytes);
    }
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::{PutOutcome, WARM_CACHE_DOCUMENTS_MAX, WARM_CACHE_ENTRY_DECODED_BYTES_MAX, WarmCache};
    use collab_core::LoroCollabEngine;
    use uuid::Uuid;

    fn engine() -> LoroCollabEngine {
        LoroCollabEngine::new_empty(1)
    }

    #[test]
    fn put_then_fork_matching_returns_an_independent_engine_and_keeps_the_entry() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        let outcome = cache.put(id, engine(), "loro-1".to_string(), 5, vec![9, 9], 128);
        assert_eq!(outcome, PutOutcome::Cached);
        assert!(cache.contains(id));

        let forked = cache
            .fork_matching(id, 5, "loro-1")
            .expect("fork succeeds")
            .expect("head_seq/format_version match");
        drop(forked);
        assert!(cache.contains(id), "fork_matching is non-destructive");
    }

    #[test]
    fn fork_matching_returns_none_on_a_head_seq_mismatch() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        cache.put(id, engine(), "loro-1".to_string(), 5, vec![9, 9], 128);

        let result = cache.fork_matching(id, 6, "loro-1").expect("fork call does not error");
        assert!(
            result.is_none(),
            "a stale prepared_head_seq must miss, not silently serve stale state"
        );
    }

    #[test]
    fn fork_matching_returns_none_on_a_format_version_mismatch() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        cache.put(id, engine(), "loro-1".to_string(), 5, vec![9, 9], 128);

        let result = cache.fork_matching(id, 5, "loro-2").expect("fork call does not error");
        assert!(result.is_none());
    }

    #[test]
    fn an_entry_over_the_single_entry_ceiling_is_not_cached() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        let outcome = cache.put(
            id,
            engine(),
            "loro-1".to_string(),
            0,
            vec![],
            WARM_CACHE_ENTRY_DECODED_BYTES_MAX + 1,
        );
        assert_eq!(outcome, PutOutcome::TooLargeToCache);
        assert!(!cache.contains(id));
        assert_eq!(cache.total_bytes(), 0);
    }

    #[test]
    fn exactly_at_the_single_entry_ceiling_is_cached() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        let outcome = cache.put(
            id,
            engine(),
            "loro-1".to_string(),
            0,
            vec![],
            WARM_CACHE_ENTRY_DECODED_BYTES_MAX,
        );
        assert_eq!(outcome, PutOutcome::Cached);
        assert!(cache.contains(id));
    }

    #[test]
    fn entry_count_ceiling_evicts_the_least_recently_used() {
        let cache = WarmCache::new();
        let mut ids = Vec::new();
        for i in 0..WARM_CACHE_DOCUMENTS_MAX {
            let id = Uuid::new_v4();
            ids.push(id);
            cache.put(id, engine(), "loro-1".to_string(), i64::try_from(i).unwrap(), vec![], 1);
            // Force a strictly increasing `last_used` ordering so eviction order is deterministic
            // (two `Instant::now()` calls back to back can otherwise tie on some platforms).
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        assert_eq!(cache.len(), WARM_CACHE_DOCUMENTS_MAX);

        // One more document past the ceiling: the oldest (`ids[0]`) must be evicted, everything
        // else stays, and the count never exceeds the ceiling.
        let newest = Uuid::new_v4();
        cache.put(newest, engine(), "loro-1".to_string(), 999, vec![], 1);
        assert_eq!(
            cache.len(),
            WARM_CACHE_DOCUMENTS_MAX,
            "count must never exceed the ceiling"
        );
        assert!(
            !cache.contains(ids[0]),
            "the least-recently-used entry must have been evicted"
        );
        assert!(cache.contains(newest));
        assert!(
            cache.contains(ids[1]),
            "everything but the oldest survives a one-over insert"
        );
    }

    #[test]
    fn decoded_bytes_ceiling_evicts_until_the_new_entry_fits() {
        let cache = WarmCache::new();
        // Each entry sits exactly at the single-entry ceiling (128 MiB); the total budget is
        // 512 MiB = 4x that, so a 5th such entry cannot coexist with all 4 previous ones and
        // must force an LRU eviction on bytes alone -- 5 entries is nowhere near the 64-entry
        // count ceiling, so this exercises the byte budget specifically, not the count one.
        let per_entry = WARM_CACHE_ENTRY_DECODED_BYTES_MAX;
        let mut ids = Vec::new();
        for i in 0..4u32 {
            let id = Uuid::new_v4();
            ids.push(id);
            let outcome = cache.put(id, engine(), "loro-1".to_string(), i64::from(i), vec![], per_entry);
            assert_eq!(outcome, PutOutcome::Cached);
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        assert_eq!(cache.total_bytes(), per_entry.saturating_mul(4));
        assert_eq!(cache.total_bytes(), super::WARM_CACHE_DECODED_BYTES_MAX);

        // A 5th entry that would push total bytes over the ceiling must evict the oldest
        // (`ids[0]`) to make room, never simply exceed the byte budget.
        let fifth = Uuid::new_v4();
        let outcome = cache.put(fifth, engine(), "loro-1".to_string(), 4, vec![], per_entry);
        assert_eq!(outcome, PutOutcome::Cached);
        assert!(cache.total_bytes() <= super::WARM_CACHE_DECODED_BYTES_MAX);
        assert!(
            !cache.contains(ids[0]),
            "the least-recently-used entry must have been evicted for space"
        );
        assert!(cache.contains(ids[1]));
        assert!(cache.contains(ids[2]));
        assert!(cache.contains(ids[3]));
        assert!(cache.contains(fifth));
    }

    #[test]
    fn fork_matching_on_a_miss_returns_none_without_panicking() {
        let cache = WarmCache::new();
        assert!(
            cache
                .fork_matching(Uuid::new_v4(), 0, "loro-1")
                .expect("call does not error")
                .is_none()
        );
    }
}
