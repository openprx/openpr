//! The instance-local, per-document coordinator (`ADR-0010`'s "第 0 层"): the admission gate every
//! path that will advance a document's `head_seq` must acquire, in `document_id`-ascending order,
//! before it does anything else. It is explicitly *not* part of the database lock rank
//! (`ADR-0013`'s "workspace/authz epoch → object/ancestor 行 → collab document → event/dispatch/
//! delivery") — it exists purely to serialize same-instance writers so a hot document's isolated
//! apply/hydrate work does not race itself before the row lock is even reached.
//!
//! Implemented as a registry of owned `tokio::sync::Semaphore(1)` permits, one per
//! currently-contended `document_id`, guarded by a short `parking_lot::Mutex` — never a
//! `tokio::sync::Mutex` held across the caller's write work, and the `parking_lot` guard here
//! itself is never held across an `.await` (acquiring the semaphore permit happens *after* the
//! guard is dropped).

#![allow(clippy::too_long_first_doc_paragraph)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use crate::error::ApiError;

use super::limits::COORDINATOR_ACQUIRE_TIMEOUT_MS;

#[derive(Default)]
pub struct DocumentCoordinator {
    semaphores: Mutex<HashMap<Uuid, Arc<Semaphore>>>,
}

/// An acquired admission slot for one document. Dropping it releases the slot; the semaphore
/// entry itself is reference-counted, so it disappears from the registry once no one holds or is
/// waiting on it (checked opportunistically on the next acquire, not eagerly on release, since an
/// eager sweep would need the registry lock held at drop time from a context that cannot await).
pub struct DocumentPermit {
    _permit: OwnedSemaphorePermit,
}

impl DocumentCoordinator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn semaphore_for(&self, document_id: Uuid) -> Arc<Semaphore> {
        let mut semaphores = self.semaphores.lock();
        // Opportunistic cleanup: an entry only this call and no other holder/waiter references
        // can be dropped now instead of growing the map forever.
        semaphores.retain(|_, sem| Arc::strong_count(sem) > 1);
        semaphores
            .entry(document_id)
            .or_insert_with(|| Arc::new(Semaphore::new(1)))
            .clone()
    }

    /// Acquires the admission slot for one document, bounded by
    /// [`COORDINATOR_ACQUIRE_TIMEOUT_MS`].
    ///
    /// Callers that need more than one document's slot (multi-document governance commands —
    /// none exist in this package; `move_object` is `ADR-0012`/`ADR-0013` v0.5 scope) must acquire
    /// them in `document_id`-ascending order themselves by calling this once per id in that order
    /// — this function only ever locks one document at a time, so it cannot itself get that
    /// ordering wrong.
    ///
    /// # Errors
    /// `ApiError::Conflict` (mapped to `server_draining`/`reason="contention"` on the wire by the
    /// caller) when the timeout elapses first.
    pub async fn acquire(&self, document_id: Uuid) -> Result<DocumentPermit, ApiError> {
        let semaphore = self.semaphore_for(document_id);
        let acquire = semaphore.acquire_owned();
        match tokio::time::timeout(Duration::from_millis(COORDINATOR_ACQUIRE_TIMEOUT_MS), acquire).await {
            Ok(Ok(permit)) => Ok(DocumentPermit { _permit: permit }),
            Ok(Err(_closed)) => Err(ApiError::Conflict(
                "document coordinator semaphore was closed".to_string(),
            )),
            Err(_timeout) => Err(ApiError::Conflict(
                "document coordinator acquisition timed out".to_string(),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::DocumentCoordinator;
    use std::sync::Arc;
    use std::time::Duration;
    use uuid::Uuid;

    #[tokio::test]
    async fn two_different_documents_do_not_serialize_each_other() {
        let coordinator = Arc::new(DocumentCoordinator::new());
        let a = coordinator.acquire(Uuid::new_v4()).await.expect("acquires");
        let b = coordinator
            .acquire(Uuid::new_v4())
            .await
            .expect("a different document acquires independently");
        drop(a);
        drop(b);
    }

    #[tokio::test]
    #[allow(clippy::significant_drop_tightening)]
    async fn a_second_writer_for_the_same_document_waits_then_proceeds_after_release() {
        let coordinator = Arc::new(DocumentCoordinator::new());
        let document_id = Uuid::new_v4();
        let first = coordinator.acquire(document_id).await.expect("first acquires");

        let coordinator2 = coordinator.clone();
        let waiter = tokio::spawn(async move { coordinator2.acquire(document_id).await });

        // Give the spawned task a chance to actually start waiting on the held permit.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !waiter.is_finished(),
            "the second acquire must block while the first permit is held"
        );

        drop(first);
        let second = waiter.await.expect("task joins").expect("acquires after release");
        drop(second);
    }

    #[tokio::test]
    #[allow(clippy::significant_drop_tightening)]
    async fn acquisition_times_out_rather_than_waiting_forever() {
        let coordinator = Arc::new(DocumentCoordinator::new());
        let document_id = Uuid::new_v4();
        let _held = coordinator.acquire(document_id).await.expect("first acquires");

        let result = coordinator.acquire(document_id).await;
        assert!(result.is_err(), "acquisition must time out while the permit is held");
    }
}
