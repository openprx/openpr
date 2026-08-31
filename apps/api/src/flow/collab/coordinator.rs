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

/// The one definition of `ADR-0013` §2.1's lock order for a *contended existing document set*:
/// ascending `document_id`, de-duplicated.
///
/// Both layers of that ADR's two-layer order are derived from this single function —
/// [`DocumentCoordinator::acquire_many`] for the instance-local layer 0, and
/// `flow::move_object`'s locked phase for the layer-3 `collab_documents` row locks. That is the
/// point: R16's correction says the two layers must use *the same* order, and the only way to
/// keep that true under later edits is for there to be exactly one place that decides it. A test
/// that captures the ids a real move locked in the database can then be compared against this
/// function's output directly.
#[must_use]
pub fn ascending_document_lock_order(document_ids: &[Uuid]) -> Vec<Uuid> {
    let mut ordered = document_ids.to_vec();
    ordered.sort_unstable();
    ordered.dedup();
    ordered
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

    /// Acquires the admission slots for a whole *contended existing document set* in one call,
    /// **sorted ascending by `document_id` and de-duplicated inside this function** — the layer-0
    /// half of `ADR-0013` §2.1's two-layer lock order.
    ///
    /// This exists so the ordering cannot be gotten wrong at a call site. `ADR-0013` R16's whole
    /// correction is that ordering the *database* row locks is not enough: the instance-local
    /// coordinator sits above the database lock rank, is taken before the transaction is even
    /// opened, and two same-instance requests taking `(A,B)` and `(B,A)` deadlock in **tokio**,
    /// where `PostgreSQL`'s deadlock detector cannot see them and `limits-v1`'s DB lock wait does
    /// not apply. Sorting here, in the only function a multi-document command can use to get more
    /// than one permit, is what makes "coordinator order == database order" a property of the type
    /// rather than a rule every caller has to remember.
    ///
    /// The returned permits are released when the returned `Vec` is dropped; hold it for the whole
    /// command (including any bounded rebase retry), exactly as [`super::write::accept_update`]
    /// holds its single permit.
    ///
    /// # Errors
    /// `ApiError::Conflict` when any one slot's [`COORDINATOR_ACQUIRE_TIMEOUT_MS`] elapses first.
    /// Permits already taken by this call are dropped (released) on that path, so a timed-out
    /// acquisition never leaves a partially-held set behind.
    pub async fn acquire_many(&self, document_ids: &[Uuid]) -> Result<Vec<DocumentPermit>, ApiError> {
        let ordered = ascending_document_lock_order(document_ids);
        let mut permits = Vec::with_capacity(ordered.len());
        for document_id in ordered {
            // `?` drops `permits` on the error path, releasing everything taken so far.
            permits.push(self.acquire(document_id).await?);
        }
        Ok(permits)
    }

    /// Acquires the admission slot for one document, bounded by
    /// [`COORDINATOR_ACQUIRE_TIMEOUT_MS`].
    ///
    /// Callers that need more than one document's slot (multi-document governance commands —
    /// `move_object`, `ADR-0012`/`ADR-0013` v0.5) must go through [`Self::acquire_many`], which
    /// does the `document_id`-ascending ordering itself. This function only ever locks one
    /// document at a time, so it cannot get that ordering wrong on its own.
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
    use super::{DocumentCoordinator, ascending_document_lock_order};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use uuid::Uuid;

    /// Two ids whose byte order is known, so "ascending" is a fact of the fixture rather than of
    /// whatever `Uuid::new_v4` happened to produce.
    fn ordered_pair() -> (Uuid, Uuid) {
        (Uuid::from_u128(0x1111_1111), Uuid::from_u128(0x2222_2222))
    }

    #[test]
    fn ascending_lock_order_sorts_and_deduplicates() {
        let (low, high) = ordered_pair();
        assert_eq!(ascending_document_lock_order(&[high, low]), vec![low, high]);
        assert_eq!(ascending_document_lock_order(&[low, high]), vec![low, high]);
        assert_eq!(ascending_document_lock_order(&[high, low, high]), vec![low, high]);
        assert_eq!(ascending_document_lock_order(&[]), Vec::<Uuid>::new());
    }

    /// `ADR-0013` §2.1 layer 0, and the falsifiable half of it.
    ///
    /// Two multi-document commands whose *request* order is reversed — `(low, high)` and
    /// `(high, low)` — are choreographed deterministically rather than raced: this test holds both
    /// permits itself, lets both waiters queue up behind them, and then releases the two blockers
    /// one at a time. `tokio::sync::Semaphore` is FIFO, so the interleaving below is fixed, not
    /// probabilistic.
    ///
    /// With [`DocumentCoordinator::acquire_many`]'s sort in place, both requests ask for `low`
    /// first, so releasing `low` admits the first waiter, releasing `high` lets it finish, and the
    /// second waiter then runs to completion: **both succeed**.
    ///
    /// Delete the sort (make `acquire_many` follow request order) and the same choreography
    /// produces the tokio-layer deadlock `ADR-0013` R16 names: A holds `low` waiting on `high`, B
    /// holds `high` waiting on `low`, neither is a database lock, `PostgreSQL`'s deadlock detector
    /// never sees it, and both come back as `Err` once `COORDINATOR_ACQUIRE_TIMEOUT_MS` elapses.
    /// That is why this asserts `is_ok()` on both: it is red exactly when the ordering discipline
    /// is gone.
    #[tokio::test]
    #[allow(clippy::significant_drop_tightening)]
    async fn reversed_multi_document_requests_both_succeed_because_acquire_many_sorts() {
        let (low, high) = ordered_pair();
        let coordinator = Arc::new(DocumentCoordinator::new());

        // Both blockers held by the test: every waiter below queues, nothing races.
        let block_low = coordinator.acquire(low).await.expect("test holds low");
        let block_high = coordinator.acquire(high).await.expect("test holds high");

        let first = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.acquire_many(&[low, high]).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        let second = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.acquire_many(&[high, low]).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;

        let started = Instant::now();
        drop(block_low);
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(block_high);

        let first_permits = first.await.expect("first task joins");
        assert!(
            first_permits.is_ok(),
            "the first reversed-order request must acquire the whole set, got {:?}",
            first_permits.err()
        );
        // Release before awaiting the second: the second is queued behind these exact permits.
        drop(first_permits);
        let second_permits = second.await.expect("second task joins");
        assert!(
            second_permits.is_ok(),
            "the second reversed-order request must acquire the whole set, got {:?}",
            second_permits.err()
        );
        assert!(
            started.elapsed() < Duration::from_millis(super::COORDINATOR_ACQUIRE_TIMEOUT_MS),
            "both requests must complete well inside one acquisition timeout; \
             taking longer means they were serialized by a stall, not by the ordering"
        );
    }

    #[tokio::test]
    #[allow(clippy::significant_drop_tightening)]
    async fn acquire_many_releases_everything_it_took_when_one_slot_times_out() {
        let (low, high) = ordered_pair();
        let coordinator = Arc::new(DocumentCoordinator::new());
        let block_high = coordinator.acquire(high).await.expect("test holds high");

        // Asks for both: takes `low`, then times out on `high`.
        let result = coordinator.acquire_many(&[low, high]).await;
        assert!(result.is_err(), "the blocked slot must time out");

        // `low` must be free again — a partially-held set would wedge every later writer.
        let low_again = coordinator.acquire(low).await;
        assert!(
            low_again.is_ok(),
            "a timed-out acquire_many must not leave the slots it already took held"
        );
        drop(low_again);
        drop(block_high);
    }

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
