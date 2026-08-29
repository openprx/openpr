use std::error::Error;
use std::fmt::{Display, Formatter};

/// Typed error surface shared by every [`crate::engine::CollabEngine`] adapter.
///
/// Adapter-specific engine errors (loro's `LoroError`, yrs's `Error`/`UpdateError`) are always
/// mapped into one of these variants before crossing the `CollabEngine` boundary, so the corpus
/// runner and benchmark runner never depend on an engine-specific error type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollabError {
    /// A snapshot or update byte slice was empty.
    EmptyInput { input: &'static str },
    /// A snapshot or update byte slice exceeded the configured limit.
    InputTooLarge {
        input: &'static str,
        actual_bytes: usize,
        max_bytes: usize,
    },
    /// The bytes could not be decoded as a valid snapshot/update for this engine.
    DecodeFailed { input: &'static str, reason: String },
    /// An operation referenced a logical node id that does not exist in local state.
    UnknownNode { id: String },
    /// An operation referenced a logical node id that already exists where creation was expected.
    DuplicateNode { id: String },
    /// A move operation was rejected because it would have created a cycle.
    CycleRejected { id: String },
    /// The underlying engine reported a failure applying an otherwise well-formed operation.
    OperationFailed { reason: String },
    /// A structural (non-byte-length) limit from `contracts/limits-v1.md` would have been
    /// exceeded had the operation (or batch) been applied. `limit_kind` is copied verbatim from
    /// that document's `limit_kind` column so it can be compared byte-for-byte against the
    /// frozen contract.
    LimitExceeded {
        limit_kind: &'static str,
        limit: u64,
        observed: u64,
    },
}

impl Display for CollabError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyInput { input } => write!(formatter, "{input} must not be empty"),
            Self::InputTooLarge {
                input,
                actual_bytes,
                max_bytes,
            } => write!(
                formatter,
                "{input} is {actual_bytes} bytes, exceeding the {max_bytes}-byte limit"
            ),
            Self::DecodeFailed { input, reason } => {
                write!(formatter, "failed to decode {input}: {reason}")
            }
            Self::UnknownNode { id } => write!(formatter, "unknown node id {id}"),
            Self::DuplicateNode { id } => write!(formatter, "duplicate node id {id}"),
            Self::CycleRejected { id } => {
                write!(formatter, "move of node {id} rejected: would create a cycle")
            }
            Self::OperationFailed { reason } => write!(formatter, "operation failed: {reason}"),
            Self::LimitExceeded {
                limit_kind,
                limit,
                observed,
            } => write!(
                formatter,
                "limit_exceeded({limit_kind}): observed {observed}, limit {limit}"
            ),
        }
    }
}

impl Error for CollabError {}

impl CollabError {
    /// Maps this error to the frozen `limit_kind` string from `contracts/limits-v1.md`'s table,
    /// when this error represents a safety-ceiling rejection. Returns `None` for errors that are
    /// not a limit rejection at all (e.g. [`Self::DecodeFailed`], [`Self::UnknownNode`]) — a
    /// boundary-fixture assertion that a rejection carries a *specific* `limit_kind` must not be
    /// satisfiable by an unrelated decode failure, so callers should treat `None` as a real test
    /// failure rather than paper over it.
    #[must_use]
    pub fn limit_kind(&self) -> Option<&'static str> {
        match self {
            Self::InputTooLarge { input: "update", .. } => Some("update_bytes"),
            Self::LimitExceeded { limit_kind, .. } => Some(limit_kind),
            _ => None,
        }
    }
}

impl From<crate::limits::LimitViolation> for CollabError {
    fn from(violation: crate::limits::LimitViolation) -> Self {
        Self::LimitExceeded {
            limit_kind: violation.limit_kind,
            limit: violation.limit,
            observed: violation.observed,
        }
    }
}

/// Byte-length limits enforced on every snapshot/update before it ever reaches an engine's
/// decoder. Shared by both adapters so the boundary-validation policy cannot drift between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputLimits {
    pub snapshot_bytes_max: usize,
    pub update_bytes_max: usize,
}

impl Default for InputLimits {
    fn default() -> Self {
        // Matches the update_bytes / (generous) snapshot ceiling from limits-v1.md's boundary
        // fixtures (65,536-byte update exact boundary). Snapshots are allowed much larger since
        // they carry full history, not a single change.
        Self {
            snapshot_bytes_max: 64 * 1024 * 1024,
            update_bytes_max: 65_536,
        }
    }
}

impl InputLimits {
    pub const fn validate_snapshot(self, snapshot: &[u8]) -> Result<(), CollabError> {
        validate_input("snapshot", snapshot, self.snapshot_bytes_max)
    }

    pub const fn validate_update(self, update: &[u8]) -> Result<(), CollabError> {
        validate_input("update", update, self.update_bytes_max)
    }
}

const fn validate_input(input_name: &'static str, input: &[u8], max_bytes: usize) -> Result<(), CollabError> {
    if input.is_empty() {
        return Err(CollabError::EmptyInput { input: input_name });
    }
    if input.len() > max_bytes {
        return Err(CollabError::InputTooLarge {
            input: input_name,
            actual_bytes: input.len(),
            max_bytes,
        });
    }
    Ok(())
}
