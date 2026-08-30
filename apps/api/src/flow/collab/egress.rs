//! Per-session outbound `accepted` sequencing (`collab-protocol-v1.md` "accepted 出站顺序").
//!
//! "每条 WebSocket subscription 的 document egress 必须严格按 seq 递增。`snapshot.head_seq=H` 后
//! 第一条 accepted 只能是 `H+1`；sequencer 发现 notice `N>next_seq` 时必须 ... 补齐/确认 gap，补不齐
//! 则发送 `resync(reason="outbound_gap")`，禁止先发 `N` 或重新 apply update 拼 receipt。"
//!
//! This is the pure decision core of that sequencer: given the seq a session's outbound channel
//! just handed it and the seq that session is next expecting, decide whether to forward it
//! in-order, drop it as a stale duplicate, or treat it as a gap this session's caller
//! (`flow::collab::session::run`) must resolve — by backfilling the missing range from
//! `collab_updates` (persisted receipt metadata, never by re-applying CRDT bytes) or, failing
//! that, sending `resync`. No I/O happens here; [`EgressSequencer`] only tracks the one integer
//! `collab-protocol-v1.md` defines this invariant over, so the seq arithmetic itself is testable
//! without a database or a socket.

#![allow(clippy::too_long_first_doc_paragraph)]

use std::cmp::Ordering;

/// What a session's outbound loop should do with one `accepted.head_seq` it just received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqDecision {
    /// `seq == next_expected_seq`: forward this notice (and its paired `update`, if any) as-is.
    /// The sequencer has already advanced past it.
    InOrder,
    /// `seq < next_expected_seq`: a stale repeat of something this session already forwarded
    /// (`collab-protocol-v1.md`: "fan-out/commit notice 在实例间可以重复或乱序" — tolerated at the
    /// source, filtered here). Never forwarded, never advances the sequencer.
    Duplicate,
    /// `seq > next_expected_seq`: a gap. `missing_from..=missing_to` is the persisted range the
    /// caller must try to backfill from `collab_updates` before forwarding `seq` itself; on
    /// success call [`EgressSequencer::resolve_gap`], on failure call
    /// [`EgressSequencer::give_up_and_resync`].
    Gap { missing_from: i64, missing_to: i64 },
}

/// One WebSocket session's outbound sequencing state. Lives for the lifetime of one connection
/// (`flow::collab::session::run` owns it), seeded from that session's own `snapshot.head_seq` —
/// deliberately per-session, not per-document: two sessions for the same document can bootstrap
/// at different moments and therefore reasonably observe different `next_expected_seq` starting
/// points before they both converge onto the same live stream.
#[derive(Debug, Clone, Copy)]
pub struct EgressSequencer {
    next_expected_seq: i64,
}

impl EgressSequencer {
    /// `snapshot.head_seq=H` 后第一条 accepted 只能是 `H+1`.
    #[must_use]
    pub const fn after_snapshot(head_seq: i64) -> Self {
        Self {
            next_expected_seq: head_seq + 1,
        }
    }

    #[must_use]
    pub const fn next_expected_seq(&self) -> i64 {
        self.next_expected_seq
    }

    /// Evaluates one incoming `accepted.head_seq` against this session's expectation. Does not by
    /// itself advance the sequencer past a [`SeqDecision::Gap`] — the caller must resolve it first
    /// via [`Self::resolve_gap`] or [`Self::give_up_and_resync`], exactly once per gap, so a
    /// caller cannot accidentally skip that step and silently advance past missing data.
    pub fn evaluate(&mut self, seq: i64) -> SeqDecision {
        match seq.cmp(&self.next_expected_seq) {
            Ordering::Less => SeqDecision::Duplicate,
            Ordering::Equal => {
                self.next_expected_seq += 1;
                SeqDecision::InOrder
            }
            Ordering::Greater => SeqDecision::Gap {
                missing_from: self.next_expected_seq,
                missing_to: seq - 1,
            },
        }
    }

    /// The caller successfully backfilled every seq in `missing_from..=missing_to` (a
    /// [`SeqDecision::Gap`]'s exact range — enforced by requiring the just-filled upper bound,
    /// not letting the caller pass an arbitrary jump) from `collab_updates`, and is about to
    /// forward those synthesized frames followed by the original notice that revealed the gap.
    /// Advances the sequencer to expect the seq right after that original notice.
    pub const fn resolve_gap(&mut self, filled_through_original_seq: i64) {
        self.next_expected_seq = filled_through_original_seq + 1;
    }

    /// The caller could not backfill the gap (missing/compacted rows, or a query failure) and is
    /// about to send `resync(reason="outbound_gap")` instead. `collab-protocol-v1.md` forbids
    /// forwarding the notice that revealed the gap in this case ("禁止先发 N"), so this does not
    /// take that seq at all -- it only stops this sequencer from re-flagging the same already-
    /// reported gap on every subsequent notice while the client reconnects, by trusting the
    /// resync itself (not a forwarded frame) to be the client's cue to re-bootstrap.
    pub const fn give_up_and_resync(&mut self, seq_that_revealed_the_gap: i64) {
        self.next_expected_seq = seq_that_revealed_the_gap + 1;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::{EgressSequencer, SeqDecision};

    #[test]
    fn first_accepted_after_snapshot_must_be_head_seq_plus_one() {
        let mut sequencer = EgressSequencer::after_snapshot(10);
        assert_eq!(sequencer.next_expected_seq(), 11);
        assert_eq!(sequencer.evaluate(11), SeqDecision::InOrder);
        assert_eq!(sequencer.next_expected_seq(), 12);
    }

    #[test]
    fn contiguous_notices_all_forward_in_order() {
        let mut sequencer = EgressSequencer::after_snapshot(0);
        for seq in 1..=5 {
            assert_eq!(sequencer.evaluate(seq), SeqDecision::InOrder);
        }
        assert_eq!(sequencer.next_expected_seq(), 6);
    }

    #[test]
    fn a_repeat_of_an_already_forwarded_seq_is_a_duplicate_and_does_not_advance() {
        let mut sequencer = EgressSequencer::after_snapshot(0);
        assert_eq!(sequencer.evaluate(1), SeqDecision::InOrder);
        assert_eq!(sequencer.evaluate(2), SeqDecision::InOrder);
        assert_eq!(
            sequencer.evaluate(1),
            SeqDecision::Duplicate,
            "seq 1 was already forwarded"
        );
        assert_eq!(
            sequencer.evaluate(2),
            SeqDecision::Duplicate,
            "seq 2 was already forwarded"
        );
        assert_eq!(
            sequencer.next_expected_seq(),
            3,
            "evaluating a duplicate must not move the expectation"
        );
    }

    #[test]
    fn a_seq_ahead_of_expectation_is_a_gap_with_the_exact_missing_range() {
        let mut sequencer = EgressSequencer::after_snapshot(10);
        assert_eq!(
            sequencer.evaluate(14),
            SeqDecision::Gap {
                missing_from: 11,
                missing_to: 13
            }
        );
        assert_eq!(
            sequencer.next_expected_seq(),
            11,
            "an unresolved gap must not silently advance the sequencer"
        );
    }

    #[test]
    fn a_single_seq_gap_reports_missing_from_equal_to_missing_to() {
        let mut sequencer = EgressSequencer::after_snapshot(10);
        assert_eq!(
            sequencer.evaluate(12),
            SeqDecision::Gap {
                missing_from: 11,
                missing_to: 11
            }
        );
    }

    #[test]
    fn resolving_a_gap_via_backfill_advances_past_the_notice_that_revealed_it() {
        let mut sequencer = EgressSequencer::after_snapshot(10);
        let SeqDecision::Gap { .. } = sequencer.evaluate(14) else {
            panic!("expected a gap");
        };
        sequencer.resolve_gap(14);
        assert_eq!(sequencer.next_expected_seq(), 15);
        assert_eq!(sequencer.evaluate(15), SeqDecision::InOrder);
    }

    #[test]
    fn giving_up_on_a_gap_advances_past_it_without_ever_forwarding_it() {
        let mut sequencer = EgressSequencer::after_snapshot(10);
        let SeqDecision::Gap { .. } = sequencer.evaluate(20) else {
            panic!("expected a gap");
        };
        sequencer.give_up_and_resync(20);
        assert_eq!(sequencer.next_expected_seq(), 21);
        // The stream continuing normally from 21 onward must not re-flag the resync'd gap.
        assert_eq!(sequencer.evaluate(21), SeqDecision::InOrder);
    }

    #[test]
    fn a_gap_immediately_followed_by_a_second_larger_gap_reports_the_new_missing_range() {
        let mut sequencer = EgressSequencer::after_snapshot(0);
        let SeqDecision::Gap { .. } = sequencer.evaluate(5) else {
            panic!("expected first gap");
        };
        sequencer.give_up_and_resync(5);
        assert_eq!(
            sequencer.evaluate(9),
            SeqDecision::Gap {
                missing_from: 6,
                missing_to: 8
            }
        );
    }
}
