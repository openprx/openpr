//! Fractional-index sibling ordering, shared so both adapters resolve concurrent
//! insert/reorder positions with the identical algorithm.
//!
//! Loro's own `TreeHandler` has a built-in fractional index (`enable_fractional_index`) and does
//! not need this module. The yrs adapter has no native moveable-tree/ordered-children primitive,
//! so it emulates ordering with plain LWW registers holding these keys; this module is the
//! (engine-agnostic) key algebra it needs.

const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
const BASE: u32 = 36;
const MAX_DIGITS: usize = 128;

fn digit_value(c: char) -> u32 {
    // ALPHABET has exactly 36 entries, so any found position fits in a u32 with room to spare.
    ALPHABET
        .iter()
        .position(|candidate| *candidate == c as u8)
        .and_then(|pos| u32::try_from(pos).ok())
        .unwrap_or(0)
}

fn digit_char(value: u32) -> char {
    // `value.min(BASE - 1)` is always in `0..36`, well within usize's range on every target.
    #[allow(clippy::cast_possible_truncation)]
    let clamped = value.min(BASE - 1) as usize;
    ALPHABET.get(clamped).copied().unwrap_or(b'0') as char
}

/// Generates a key that sorts strictly between `lower` (exclusive, `None` = no lower bound) and
/// `upper` (exclusive, `None` = no upper bound).
///
/// Ties (both bounds absent) return the alphabet's midpoint digit.
#[must_use]
pub fn between(lower: Option<&str>, upper: Option<&str>) -> String {
    let lower_digits: Vec<u32> = lower.unwrap_or("").chars().map(digit_value).collect();
    let upper_digits: Option<Vec<u32>> = upper.map(|s| s.chars().map(digit_value).collect());

    let mut result = String::new();
    for position in 0..MAX_DIGITS {
        let lo = lower_digits.get(position).copied().unwrap_or(0);
        let hi = upper_digits
            .as_ref()
            .map_or(BASE, |digits| digits.get(position).copied().unwrap_or(BASE));
        if hi > lo + 1 {
            let mid = lo + (hi - lo) / 2;
            result.push(digit_char(mid));
            return result;
        }
        result.push(digit_char(lo));
    }
    // Astronomically unlikely (128 digits of exhausted precision); keep the key strictly
    // orderable by extending it rather than panicking.
    result.push('1');
    result
}

/// Appends a short, deterministic, replica-distinguishing suffix so two replicas that compute the
/// same base key concurrently (e.g. both inserting into the same empty gap) still produce distinct
/// keys.
///
/// The suffix never changes ordering relative to any *other* sibling's key because it only
/// applies after a full `between()` key that already differs in its own digits from any key
/// generated against different bounds.
#[must_use]
pub fn with_replica_tiebreak(base_key: &str, replica_token: &str) -> String {
    let mut out = String::with_capacity(base_key.len() + 1 + replica_token.len());
    out.push_str(base_key);
    out.push('~');
    out.push_str(replica_token);
    out
}

#[cfg(test)]
mod tests {
    use super::{between, with_replica_tiebreak};

    #[test]
    fn between_none_none_is_a_midpoint() {
        let key = between(None, None);
        assert_eq!(key, "i");
    }

    #[test]
    fn between_respects_ordering_against_bounds() {
        let key = between(Some("a"), Some("z"));
        assert!(key.as_str() > "a");
        assert!(key.as_str() < "z");
    }

    #[test]
    fn repeated_bisection_stays_ordered() {
        let mut lower: Option<String> = None;
        let upper = Some("z".to_string());
        let mut keys = Vec::new();
        for _ in 0..40 {
            let key = between(lower.as_deref(), upper.as_deref());
            keys.push(key.clone());
            lower = Some(key);
        }
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "keys must already be produced in ascending order");
    }

    #[test]
    fn tiebreak_keeps_keys_distinct_on_exact_collision() {
        let base = between(Some("m"), Some("o"));
        let a = with_replica_tiebreak(&base, "peer-a");
        let b = with_replica_tiebreak(&base, "peer-b");
        assert_ne!(a, b);
        assert!(a.starts_with(&base));
        assert!(b.starts_with(&base));
    }
}
