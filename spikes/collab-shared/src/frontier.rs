/// An opaque, engine-owned version marker.
///
/// `Frontier` deliberately exposes nothing but bytes: no peer id, no client id, no lamport
/// clock structure. Business-layer code (the corpus runner, the benchmark runner, future
/// authorization code) can compare, hash, and pass frontiers around, but it can never reach
/// into an engine's peer/actor identity through this type. Each adapter is responsible for
/// picking a serialization for its own internal version representation and wrapping it here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Frontier(Vec<u8>);

impl Frontier {
    #[must_use]
    pub const fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
