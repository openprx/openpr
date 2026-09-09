//! Bounded, per-[`AppState`] effective-permission cache for Flow read paths.
//!
//! This cache is an accelerator, never authorization authority. Every entry is bound to the
//! workspace's database-authoritative `authz_epoch`; [`PermissionCache::get`] requires the caller
//! to supply the current epoch and treats any mismatch as a miss. There is intentionally no read
//! API that omits that comparison. Write paths (`command.rs`, `write.rs`, `grants.rs`, and
//! `move_object.rs`) must never read this cache: they continue to call
//! [`super::authz::effective_permission`] against the database under the required fencing locks.
//!
//! Epoch advancement is the immediate logical invalidation mechanism. The explicit object and
//! workspace invalidators only reclaim memory after a successful commit; correctness never
//! depends on an exhaustive physical sweep.

use std::sync::Arc;
use std::time::{Duration, Instant};

use hashlink::LinkedHashMap;
use parking_lot::Mutex;
use platform::app::AppState;
use uuid::Uuid;

use crate::error::ApiError;

use super::authz::PermissionLevel;
use super::limits::WARM_CACHE_IDLE_TTL_SECONDS;

/// The default database pool has 20 connections. One worst-case authorized read may examine
/// `authorized_scan_rows_max` (1,000) objects, so retaining one such working set per concurrent
/// default-pool request requires 20,000 entries. This is an implementation capacity, not a new
/// contract limit: exceeding it evicts least-recently-used entries and can only add DB reads.
const PERMISSION_CACHE_ENTRIES_MAX: usize = 20_000;

/// Reuse the instance-local document warm cache's frozen 120-second idle horizon. Effective
/// permission entries are cheaper and safe to recompute, and sharing the horizon prevents stale
/// process memory from outliving the related Flow working set. This is eviction policy, not an
/// authorization or caller-visible limit.
const PERMISSION_CACHE_IDLE_TTL: Duration = Duration::from_secs(WARM_CACHE_IDLE_TTL_SECONDS);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PrincipalKind {
    User,
    Bot,
}

impl PrincipalKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Bot => "bot",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CacheKey {
    workspace_id: Uuid,
    principal_kind: PrincipalKind,
    principal_id: Uuid,
    object_id: Uuid,
}

#[derive(Debug, Clone, Copy)]
struct CacheEntry {
    level: PermissionLevel,
    authz_epoch: i64,
    last_access: Instant,
}

#[derive(Default)]
struct Registry {
    /// Oldest at the front, most recently used at the back.
    entries: LinkedHashMap<CacheKey, CacheEntry>,
}

/// A bounded effective-permission cache owned by one [`AppState`].
pub struct PermissionCache {
    registry: Mutex<Registry>,
    max_entries: usize,
    idle_ttl: Duration,
}

impl Default for PermissionCache {
    fn default() -> Self {
        Self {
            registry: Mutex::new(Registry::default()),
            max_entries: PERMISSION_CACHE_ENTRIES_MAX,
            idle_ttl: PERMISSION_CACHE_IDLE_TTL,
        }
    }
}

impl PermissionCache {
    /// Returns the cache attached to `state`, creating it on first use.
    ///
    /// # Errors
    /// `Internal` if some other concrete type was placed in the dedicated `AppState` slot.
    pub fn for_state(state: &AppState) -> Result<Arc<Self>, ApiError> {
        state
            .flow_permission_cache
            .get_or_init(Self::default)
            .ok_or(ApiError::Internal)
    }

    /// Reads one entry only when it was computed at `current_epoch`.
    #[must_use]
    pub(crate) fn get(
        &self,
        workspace_id: Uuid,
        principal_kind: PrincipalKind,
        principal_id: Uuid,
        object_id: Uuid,
        current_epoch: i64,
    ) -> Option<PermissionLevel> {
        let now = Instant::now();
        let key = CacheKey {
            workspace_id,
            principal_kind,
            principal_id,
            object_id,
        };
        let mut registry = self.registry.lock();
        let hit = {
            let entry = registry.entries.to_back(&key)?;
            if entry.authz_epoch == current_epoch && now.saturating_duration_since(entry.last_access) <= self.idle_ttl {
                entry.last_access = now;
                Some(entry.level)
            } else {
                None
            }
        };
        if hit.is_none() {
            registry.entries.remove(&key);
        }
        drop(registry);
        hit
    }

    /// Stores a DB-derived effective permission at the epoch used for that derivation.
    pub(super) fn put(
        &self,
        workspace_id: Uuid,
        principal_kind: PrincipalKind,
        principal_id: Uuid,
        object_id: Uuid,
        level: PermissionLevel,
        authz_epoch: i64,
    ) {
        if self.max_entries == 0 {
            return;
        }
        let now = Instant::now();
        let key = CacheKey {
            workspace_id,
            principal_kind,
            principal_id,
            object_id,
        };
        let mut registry = self.registry.lock();
        if !registry.entries.contains_key(&key) && registry.entries.len() >= self.max_entries {
            registry.entries.pop_front();
        }
        registry.entries.insert(
            key,
            CacheEntry {
                level,
                authz_epoch,
                last_access: now,
            },
        );
    }

    /// Physically removes every principal's cached entry for one object after a grant change.
    pub fn invalidate_object(&self, workspace_id: Uuid, object_id: Uuid) {
        self.registry
            .lock()
            .entries
            .retain(|key, _| key.workspace_id != workspace_id || key.object_id != object_id);
    }

    /// Physically removes all cached permissions for a workspace after a membership or baseline
    /// change.
    pub fn invalidate_workspace(&self, workspace_id: Uuid) {
        self.registry
            .lock()
            .entries
            .retain(|key, _| key.workspace_id != workspace_id);
    }

    #[cfg(test)]
    fn with_limits(max_entries: usize, idle_ttl: Duration) -> Self {
        Self {
            registry: Mutex::new(Registry::default()),
            max_entries,
            idle_ttl,
        }
    }

    #[cfg(test)]
    pub(crate) fn len_for_test(&self) -> usize {
        self.registry.lock().entries.len()
    }

    /// Deliberate poison seam for cross-module cache invalidation tests. Production code cannot
    /// inject an arbitrary epoch/level pair because [`Self::put`] is restricted to `collab`.
    #[cfg(test)]
    pub(crate) fn put_for_test(
        &self,
        workspace_id: Uuid,
        principal_kind: PrincipalKind,
        principal_id: Uuid,
        object_id: Uuid,
        level: PermissionLevel,
        authz_epoch: i64,
    ) {
        self.put(
            workspace_id,
            principal_kind,
            principal_id,
            object_id,
            level,
            authz_epoch,
        );
    }
}

/// Best-effort physical cleanup after a committed workspace-wide permission change.
///
/// The committed epoch is already the logical authority, so cache allocation/type errors are
/// logged and must never rewrite a successful mutation response into an error.
pub fn invalidate_workspace_after_commit(state: &AppState, workspace_id: Uuid) {
    match PermissionCache::for_state(state) {
        Ok(cache) => cache.invalidate_workspace(workspace_id),
        Err(error) => tracing::warn!(%workspace_id, %error, "flow permission cache cleanup failed"),
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    fn ids() -> (Uuid, Uuid, Uuid) {
        (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4())
    }

    #[test]
    fn stale_epoch_poison_is_a_miss_and_is_removed() {
        let cache = PermissionCache::default();
        let (workspace_id, principal_id, object_id) = ids();
        cache.put(
            workspace_id,
            PrincipalKind::User,
            principal_id,
            object_id,
            PermissionLevel::FullAccess,
            7,
        );

        assert_eq!(
            cache.get(workspace_id, PrincipalKind::User, principal_id, object_id, 8),
            None
        );
        assert_eq!(cache.len_for_test(), 0);
    }

    #[test]
    fn capacity_evicts_an_old_entry_instead_of_growing() {
        let cache = PermissionCache::with_limits(1, Duration::from_mins(1));
        let (workspace_id, principal_id, first_object) = ids();
        let second_object = Uuid::new_v4();
        cache.put(
            workspace_id,
            PrincipalKind::User,
            principal_id,
            first_object,
            PermissionLevel::View,
            1,
        );
        cache.put(
            workspace_id,
            PrincipalKind::User,
            principal_id,
            second_object,
            PermissionLevel::Edit,
            1,
        );

        assert_eq!(cache.len_for_test(), 1);
        assert_eq!(
            cache.get(workspace_id, PrincipalKind::User, principal_id, first_object, 1),
            None
        );
        assert_eq!(
            cache.get(workspace_id, PrincipalKind::User, principal_id, second_object, 1),
            Some(PermissionLevel::Edit)
        );
    }

    #[test]
    fn a_get_refreshes_lru_recency_before_capacity_eviction() {
        let cache = PermissionCache::with_limits(2, Duration::from_mins(1));
        let (workspace_id, principal_id, first_object) = ids();
        let second_object = Uuid::new_v4();
        let third_object = Uuid::new_v4();
        for object_id in [first_object, second_object] {
            cache.put(
                workspace_id,
                PrincipalKind::User,
                principal_id,
                object_id,
                PermissionLevel::View,
                1,
            );
        }

        assert_eq!(
            cache.get(workspace_id, PrincipalKind::User, principal_id, first_object, 1),
            Some(PermissionLevel::View)
        );
        cache.put(
            workspace_id,
            PrincipalKind::User,
            principal_id,
            third_object,
            PermissionLevel::Edit,
            1,
        );

        assert_eq!(
            cache.get(workspace_id, PrincipalKind::User, principal_id, second_object, 1),
            None,
            "the untouched least-recently-used entry must be evicted"
        );
        assert_eq!(
            cache.get(workspace_id, PrincipalKind::User, principal_id, first_object, 1),
            Some(PermissionLevel::View),
            "the get must have refreshed the first entry"
        );
    }

    #[test]
    fn idle_entries_expire() {
        let cache = PermissionCache::with_limits(2, Duration::from_millis(1));
        let (workspace_id, principal_id, object_id) = ids();
        cache.put(
            workspace_id,
            PrincipalKind::Bot,
            principal_id,
            object_id,
            PermissionLevel::View,
            1,
        );
        thread::sleep(Duration::from_millis(3));

        assert_eq!(
            cache.get(workspace_id, PrincipalKind::Bot, principal_id, object_id, 1),
            None
        );
        assert_eq!(cache.len_for_test(), 0);
    }

    #[test]
    fn object_and_workspace_invalidators_remove_only_their_scope() {
        let cache = PermissionCache::default();
        let (workspace_id, principal_id, object_id) = ids();
        let other_workspace = Uuid::new_v4();
        cache.put(
            workspace_id,
            PrincipalKind::User,
            principal_id,
            object_id,
            PermissionLevel::View,
            1,
        );
        cache.put(
            other_workspace,
            PrincipalKind::User,
            principal_id,
            object_id,
            PermissionLevel::Edit,
            1,
        );

        cache.invalidate_object(workspace_id, object_id);
        assert_eq!(
            cache.get(workspace_id, PrincipalKind::User, principal_id, object_id, 1),
            None
        );
        assert_eq!(
            cache.get(other_workspace, PrincipalKind::User, principal_id, object_id, 1),
            Some(PermissionLevel::Edit)
        );

        cache.invalidate_workspace(other_workspace);
        assert_eq!(cache.len_for_test(), 0);
    }
}
