//! Re-exports the shared, candidate-agnostic error/limit types.
//!
//! Input validation and error typing now live in `collab-shared` so both adapters enforce the
//! identical boundary policy (see `collab_shared::error`); this module keeps the old names alive
//! for anything in this crate that still imports them locally.

pub use collab_shared::{CollabError as InputError, InputLimits};
