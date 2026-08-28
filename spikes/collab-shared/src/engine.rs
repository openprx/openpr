use crate::frontier::Frontier;
use crate::operation::Operation;
use crate::semantic::SemanticSnapshot;

/// Summary of what an `import_update` call actually changed, without exposing any engine-internal
/// op/diff representation to callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Diff {
    /// `false` when the update was a byte-for-byte or semantic no-op (e.g. a duplicate replay).
    pub changed: bool,
}

/// The shared-runner sync contract, matching the shape frozen in
/// `versions/v0.3-foundation.md` work package 3:
///
/// ```ignore
/// trait CollabEngine {
///     fn load(snapshot: &[u8]) -> Result<Self>;
///     fn import_update(&mut self, update: &[u8]) -> Result<Diff>;
///     fn export_snapshot(&self) -> Result<Vec<u8>>;
///     fn export_from(&self, frontier: &Frontier) -> Result<Vec<u8>>;
///     fn frontier(&self) -> Frontier;
/// }
/// ```
///
/// Every method returns a typed [`crate::error::CollabError`] (never a boxed/opaque error), every
/// byte-slice input is length-validated before it reaches the underlying engine's decoder (see
/// each adapter's use of [`crate::error::InputLimits`]), and [`Frontier`] is an opaque byte
/// wrapper so no implementation can leak an engine-specific peer/client id through this trait.
pub trait CollabEngine: Sized {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Loads a document from a full snapshot previously produced by [`Self::export_snapshot`].
    fn load(snapshot: &[u8]) -> Result<Self, Self::Error>;

    /// Applies a remote update (full snapshot or incremental delta) produced by
    /// [`Self::export_snapshot`] or [`Self::export_from`]. Must be atomic: on error, local state
    /// (and therefore [`Self::frontier`]) is left byte-for-byte unchanged.
    fn import_update(&mut self, update: &[u8]) -> Result<Diff, Self::Error>;

    /// Exports the full document state (history + current state) as a single portable blob.
    fn export_snapshot(&self) -> Result<Vec<u8>, Self::Error>;

    /// Exports only the changes this replica has that the given remote `frontier` does not.
    fn export_from(&self, frontier: &Frontier) -> Result<Vec<u8>, Self::Error>;

    /// The current version marker for this replica, suitable for passing to a peer's
    /// [`Self::export_from`].
    fn frontier(&self) -> Frontier;
}

/// Harness-only extension used by the corpus/benchmark runners to drive an engine with the shared
/// [`Operation`] vocabulary and read back a comparable [`SemanticSnapshot`].
///
/// This is deliberately kept separate from [`CollabEngine`]: the sync contract above is the
/// frozen v0.3 shape, while this trait is shared-runner plumbing that both adapters implement
/// identically.
pub trait CorpusEngine: CollabEngine {
    /// Creates a fresh, empty replica. `replica_seed` deterministically seeds the engine's
    /// internal peer/client id so two runs of the same corpus produce byte-identical output.
    fn new_empty(replica_seed: u64) -> Self;

    /// Applies one locally-originated operation from the shared vocabulary.
    fn apply_operation(&mut self, operation: &Operation) -> Result<(), Self::Error>;

    /// Exports the current merged state as the engine-independent [`SemanticSnapshot`] used for
    /// hashing and invariant checks.
    fn semantic_snapshot(&self) -> Result<SemanticSnapshot, Self::Error>;
}
