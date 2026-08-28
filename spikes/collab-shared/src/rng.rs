//! Deterministic PRNG for fixture generation.
//!
//! Corpus fixtures must be byte-for-byte reproducible from a `corpus_seed`, which rules out
//! `rand`'s thread-local/OS-entropy sources. `SplitMix64` is a small, well-known, dependency-free
//! generator (Vigna, 2015) that is easy to hand-verify and stable across platforms.

#[derive(Debug, Clone)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Derives a fresh, independent generator from this one's current state, without advancing
    /// this generator's own sequence for anything but the derivation draw.
    ///
    /// Useful for splitting a single `corpus_seed` into independent per-replica or per-case
    /// streams deterministically.
    #[must_use]
    pub const fn derive(&mut self, salt: u64) -> Self {
        let mixed = self.next_u64() ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        Self::new(mixed)
    }

    pub const fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub const fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Returns a value in `[0, bound)`. `bound` of zero returns zero.
    pub const fn next_below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
        self.next_u32() % bound
    }

    pub const fn next_bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }

    /// Picks an index into a slice of the given length, or `None` if the length is zero.
    ///
    /// Lengths beyond `u32::MAX` saturate to the last index rather than panicking or wrapping;
    /// every real caller in this crate picks among corpus/fixture collections many orders of
    /// magnitude smaller than that.
    pub fn choose_index(&mut self, len: usize) -> Option<usize> {
        if len == 0 {
            return None;
        }
        let bound = u32::try_from(len).unwrap_or(u32::MAX);
        Some(self.next_below(bound) as usize)
    }

    /// Generates a short, deterministic base36 token, e.g. for use as a logical node id suffix.
    pub fn token(&mut self, len: usize) -> String {
        const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        // ALPHABET's length (36) fits comfortably in a u32 regardless of target pointer width.
        #[allow(clippy::cast_possible_truncation)]
        let alphabet_len = ALPHABET.len() as u32;
        let mut out = String::with_capacity(len);
        for _ in 0..len {
            let idx = self.next_below(alphabet_len) as usize;
            out.push(ALPHABET.get(idx).copied().unwrap_or(b'0') as char);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::SplitMix64;

    #[test]
    fn same_seed_is_byte_for_byte_reproducible() {
        let mut a = SplitMix64::new(42);
        let mut b = SplitMix64::new(42);
        let seq_a: Vec<u64> = (0..64).map(|_| a.next_u64()).collect();
        let seq_b: Vec<u64> = (0..64).map(|_| b.next_u64()).collect();
        assert_eq!(seq_a, seq_b);
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = SplitMix64::new(1);
        let mut b = SplitMix64::new(2);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn derive_is_deterministic_given_parent_state() {
        let mut a = SplitMix64::new(7);
        let mut b = SplitMix64::new(7);
        let mut derived_a = a.derive(11);
        let mut derived_b = b.derive(11);
        assert_eq!(derived_a.next_u64(), derived_b.next_u64());
    }
}
