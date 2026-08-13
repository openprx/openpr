//! A string wrapper for configuration values that must never reach a log line.

use std::fmt;

use serde::Deserialize;

/// What a [`Secret`] renders as in `Debug` output.
pub const REDACTED: &str = "[redacted]";

/// A configuration value that carries credential material.
///
/// The type exists so that `#[derive(Debug)]` on the surrounding configuration structs stays
/// safe: every field that holds a password, a token or a signing key is a `Secret`, and a
/// `Secret` prints as [`REDACTED`]. It deliberately implements neither `Display` nor
/// `AsRef<str>`, so `format!("{value}")` and implicit coercions do not compile; reading the
/// plaintext requires the explicit [`Secret::expose`] call, which is greppable in review.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// Wraps a plaintext value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrows the plaintext. Every call site is a deliberate decision to handle the secret.
    pub const fn expose(&self) -> &str {
        self.0.as_str()
    }

    /// Length in bytes. Used by validation rules that impose a minimum strength.
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the secret carries no bytes at all.
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::nursery
)]
mod tests {
    use super::{REDACTED, Secret};

    #[test]
    fn debug_output_never_contains_the_plaintext() {
        let secret = Secret::new("super-secret-signing-key");
        let rendered = format!("{secret:?}");
        assert_eq!(rendered, REDACTED);
        assert!(!rendered.contains("super-secret-signing-key"));
    }

    #[test]
    fn alternate_debug_output_never_contains_the_plaintext() {
        let secret = Secret::new("super-secret-signing-key");
        let rendered = format!("{secret:#?}");
        assert!(!rendered.contains("super-secret-signing-key"), "{rendered}");
    }

    #[test]
    fn expose_returns_the_plaintext() {
        let secret = Secret::new("value");
        assert_eq!(secret.expose(), "value");
        assert_eq!(secret.len(), 5);
        assert!(!secret.is_empty());
        assert!(Secret::new("").is_empty());
    }

    #[test]
    fn deserializes_transparently_from_a_plain_string() {
        let secret: Secret = serde_json::from_str(r#""from-json""#).expect("string should deserialize");
        assert_eq!(secret.expose(), "from-json");
    }
}
