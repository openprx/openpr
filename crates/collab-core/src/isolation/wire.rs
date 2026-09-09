//! Request/response wire codec between `isolation::host` (parent) and
//! `src/bin/isolated_apply_worker.rs` (child). Pure, `unsafe`-free encode/decode -- no I/O and no
//! process/signal handling live here; those belong to `isolation::host` (parent side) and
//! `src/bin/isolated_apply_worker.rs` (child side).

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

use crate::error::CollabError;
use crate::{SemanticDiff, SemanticSnapshot};

/// Child-only operation selector set explicitly by the trusted host for every spawn.
pub const OPERATION_ENV: &str = "COLLAB_ISOLATION_OPERATION";
pub const OPERATION_APPLY: &str = "apply";
pub const OPERATION_DIFF: &str = "diff";

/// Response payload cap. Sized well above `bootstrap_response_bytes_max` (12 MiB,
/// `contracts/limits-v1.md`) with headroom for a real exported document snapshot -- unlike
/// `spikes/collab-shared`'s calibration frame (1 MiB, sized for a tiny synthetic report), this
/// frame's success case carries the whole isolated candidate document.
pub const MAX_RESPONSE_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;

/// Request field cap (each of `base_snapshot`/`update`), generous relative to
/// `bootstrap_decoded_bytes_max` (8 MiB) and `update_bytes_max` (64 KiB) -- exists only so a
/// corrupt length prefix cannot make either side attempt an unbounded allocation.
pub const MAX_REQUEST_FIELD_BYTES: usize = 32 * 1024 * 1024;

fn write_u32<W: Write>(writer: &mut W, value: u32) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn read_u32<R: Read>(reader: &mut R) -> io::Result<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn write_u64<W: Write>(writer: &mut W, value: u64) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn read_u64<R: Read>(reader: &mut R) -> io::Result<u64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn write_field<W: Write>(writer: &mut W, bytes: &[u8]) -> io::Result<()> {
    let len = u32::try_from(bytes.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "field too large"))?;
    write_u32(writer, len)?;
    writer.write_all(bytes)
}

fn read_field<R: Read>(reader: &mut R, max_len: usize) -> io::Result<Vec<u8>> {
    let len_u32 = read_u32(reader)?;
    let len = usize::try_from(len_u32).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "field length"))?;
    if len > max_len {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "field exceeds cap"));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

fn write_str<W: Write>(writer: &mut W, value: &str) -> io::Result<()> {
    write_field(writer, value.as_bytes())
}

fn read_str<R: Read>(reader: &mut R, max_len: usize) -> io::Result<String> {
    let bytes = read_field(reader, max_len)?;
    String::from_utf8(bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-utf8 string field"))
}

/// Writes the harness-envelope request: `[base_len][base][update_len][update]`. Reading this back
/// is excluded from the child's metered window (`ADR-0014` section 1.3: "harness envelope 的反序列化"
/// is explicitly not counted toward the CPU/wall ceilings).
///
/// # Errors
/// Any I/O failure writing to `writer`, or [`io::ErrorKind::InvalidInput`] if either field is
/// larger than `u32::MAX` bytes (`update`/`base_snapshot` are already bounded far below that by
/// their own callers' limits, so this never fires in practice).
pub fn write_request<W: Write>(writer: &mut W, base_snapshot: &[u8], update: &[u8]) -> io::Result<()> {
    write_field(writer, base_snapshot)?;
    write_field(writer, update)?;
    writer.flush()
}

/// Reads back a request written by [`write_request`].
///
/// # Errors
/// Any I/O failure, or [`io::ErrorKind::InvalidData`] if a declared field length exceeds
/// `max_field_bytes`.
pub fn read_request<R: Read>(reader: &mut R, max_field_bytes: usize) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let base = read_field(reader, max_field_bytes)?;
    let update = read_field(reader, max_field_bytes)?;
    Ok((base, update))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayDiffUpdate {
    pub bytes: Vec<u8>,
    pub before_frontier: Vec<u8>,
    pub after_frontier: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayDiffRequest {
    pub snapshot_frontier: Vec<u8>,
    pub updates: Vec<ReplayDiffUpdate>,
    pub from_frontier: Vec<u8>,
    pub to_frontier: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayDiffResult {
    pub semantic_diff: SemanticDiff,
    pub to_snapshot: SemanticSnapshot,
    pub from_title: String,
    pub to_title: String,
}

fn json_error(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

pub fn encode_replay_diff_request(request: &ReplayDiffRequest) -> io::Result<Vec<u8>> {
    serde_json::to_vec(request).map_err(json_error)
}

pub fn decode_replay_diff_request(bytes: &[u8]) -> io::Result<ReplayDiffRequest> {
    serde_json::from_slice(bytes).map_err(json_error)
}

pub fn encode_replay_diff_result(result: &ReplayDiffResult) -> io::Result<Vec<u8>> {
    serde_json::to_vec(result).map_err(json_error)
}

pub fn decode_replay_diff_result(bytes: &[u8]) -> io::Result<ReplayDiffResult> {
    serde_json::from_slice(bytes).map_err(json_error)
}

/// What the child reports back on a run that completes without being killed by a ceiling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Success { snapshot: Vec<u8> },
    Rejected(CollabError),
}

const TAG_SUCCESS: u8 = 0;
const TAG_EMPTY_INPUT: u8 = 1;
const TAG_INPUT_TOO_LARGE: u8 = 2;
const TAG_DECODE_FAILED: u8 = 3;
const TAG_UNKNOWN_NODE: u8 = 4;
const TAG_DUPLICATE_NODE: u8 = 5;
const TAG_CYCLE_REJECTED: u8 = 6;
const TAG_OPERATION_FAILED: u8 = 7;
const TAG_LIMIT_EXCEEDED: u8 = 8;

/// `CollabError::EmptyInput`/`InputTooLarge`/`DecodeFailed`'s `input` field, and
/// `LimitExceeded`'s `limit_kind` field, are `&'static str` -- not `String` -- because every
/// producer in this crate only ever constructs them from a small, fixed vocabulary
/// (`InputLimits::validate_snapshot`/`validate_update` only ever pass `"snapshot"`/`"update"`;
/// `check_snapshot` only ever passes one of five frozen `limit_kind` values from
/// `contracts/limits-v1.md`). The wire only ever carries their UTF-8 bytes, so decoding maps back
/// to the matching static string from that same fixed set -- an unrecognized value (which a
/// well-behaved worker process never sends) falls back to a clearly-labeled placeholder rather
/// than panicking or leaking memory to fabricate an arbitrary `&'static str`.
fn static_str_from(value: &str) -> &'static str {
    match value {
        "snapshot" => "snapshot",
        "update" => "update",
        "tree_depth" => "tree_depth",
        "container_count" => "container_count",
        "document_block_count" => "document_block_count",
        "text_block_chars" => "text_block_chars",
        "document_text_chars" => "document_text_chars",
        _ => "unknown",
    }
}

/// Encodes an [`Outcome`] into a raw payload (not yet length/CRC32-framed -- see
/// `isolation::host`'s response frame helpers, which wrap this payload for transport).
///
/// # Errors
/// Any I/O failure writing into the in-memory buffer (only possible if a field exceeds
/// `u32::MAX` bytes, which none of `CollabError`'s fields ever do in practice).
pub fn encode_outcome(outcome: &Outcome) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    match outcome {
        Outcome::Success { snapshot } => {
            buf.push(TAG_SUCCESS);
            write_field(&mut buf, snapshot)?;
        }
        Outcome::Rejected(CollabError::EmptyInput { input }) => {
            buf.push(TAG_EMPTY_INPUT);
            write_str(&mut buf, input)?;
        }
        Outcome::Rejected(CollabError::InputTooLarge {
            input,
            actual_bytes,
            max_bytes,
        }) => {
            buf.push(TAG_INPUT_TOO_LARGE);
            write_str(&mut buf, input)?;
            write_u64(&mut buf, u64::try_from(*actual_bytes).unwrap_or(u64::MAX))?;
            write_u64(&mut buf, u64::try_from(*max_bytes).unwrap_or(u64::MAX))?;
        }
        Outcome::Rejected(CollabError::DecodeFailed { input, reason }) => {
            buf.push(TAG_DECODE_FAILED);
            write_str(&mut buf, input)?;
            write_str(&mut buf, reason)?;
        }
        Outcome::Rejected(CollabError::UnknownNode { id }) => {
            buf.push(TAG_UNKNOWN_NODE);
            write_str(&mut buf, id)?;
        }
        Outcome::Rejected(CollabError::DuplicateNode { id }) => {
            buf.push(TAG_DUPLICATE_NODE);
            write_str(&mut buf, id)?;
        }
        Outcome::Rejected(CollabError::CycleRejected { id }) => {
            buf.push(TAG_CYCLE_REJECTED);
            write_str(&mut buf, id)?;
        }
        Outcome::Rejected(CollabError::OperationFailed { reason }) => {
            buf.push(TAG_OPERATION_FAILED);
            write_str(&mut buf, reason)?;
        }
        Outcome::Rejected(CollabError::LimitExceeded {
            limit_kind,
            limit,
            observed,
        }) => {
            buf.push(TAG_LIMIT_EXCEEDED);
            write_str(&mut buf, limit_kind)?;
            write_u64(&mut buf, *limit)?;
            write_u64(&mut buf, *observed)?;
        }
    }
    Ok(buf)
}

/// Decodes a payload produced by [`encode_outcome`].
///
/// # Errors
/// [`io::ErrorKind::UnexpectedEof`]/`InvalidData` for a truncated or malformed payload (an empty
/// payload, an unrecognized tag byte, a declared field longer than its cap, or non-UTF-8 bytes in
/// a string field) -- the caller (`isolation::host::isolated_apply`) treats any of these as
/// [`super::host::IsolatedApplyError::HostFailure`], never as a business rejection.
pub fn decode_outcome(payload: &[u8]) -> io::Result<Outcome> {
    let mut cursor = payload;
    let mut tag = [0u8; 1];
    cursor.read_exact(&mut tag)?;
    let Some(&tag_byte) = tag.first() else {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "missing tag byte"));
    };
    match tag_byte {
        TAG_SUCCESS => {
            let snapshot = read_field(&mut cursor, MAX_RESPONSE_PAYLOAD_BYTES)?;
            Ok(Outcome::Success { snapshot })
        }
        TAG_EMPTY_INPUT => {
            let input = read_str(&mut cursor, 64)?;
            Ok(Outcome::Rejected(CollabError::EmptyInput {
                input: static_str_from(&input),
            }))
        }
        TAG_INPUT_TOO_LARGE => {
            let input = read_str(&mut cursor, 64)?;
            let actual_bytes = read_u64(&mut cursor)?;
            let max_bytes = read_u64(&mut cursor)?;
            Ok(Outcome::Rejected(CollabError::InputTooLarge {
                input: static_str_from(&input),
                actual_bytes: usize::try_from(actual_bytes).unwrap_or(usize::MAX),
                max_bytes: usize::try_from(max_bytes).unwrap_or(usize::MAX),
            }))
        }
        TAG_DECODE_FAILED => {
            let input = read_str(&mut cursor, 64)?;
            let reason = read_str(&mut cursor, MAX_RESPONSE_PAYLOAD_BYTES)?;
            Ok(Outcome::Rejected(CollabError::DecodeFailed {
                input: static_str_from(&input),
                reason,
            }))
        }
        TAG_UNKNOWN_NODE => {
            let id = read_str(&mut cursor, MAX_RESPONSE_PAYLOAD_BYTES)?;
            Ok(Outcome::Rejected(CollabError::UnknownNode { id }))
        }
        TAG_DUPLICATE_NODE => {
            let id = read_str(&mut cursor, MAX_RESPONSE_PAYLOAD_BYTES)?;
            Ok(Outcome::Rejected(CollabError::DuplicateNode { id }))
        }
        TAG_CYCLE_REJECTED => {
            let id = read_str(&mut cursor, MAX_RESPONSE_PAYLOAD_BYTES)?;
            Ok(Outcome::Rejected(CollabError::CycleRejected { id }))
        }
        TAG_OPERATION_FAILED => {
            let reason = read_str(&mut cursor, MAX_RESPONSE_PAYLOAD_BYTES)?;
            Ok(Outcome::Rejected(CollabError::OperationFailed { reason }))
        }
        TAG_LIMIT_EXCEEDED => {
            let limit_kind = read_str(&mut cursor, 64)?;
            let limit = read_u64(&mut cursor)?;
            let observed = read_u64(&mut cursor)?;
            Ok(Outcome::Rejected(CollabError::LimitExceeded {
                limit_kind: static_str_from(&limit_kind),
                limit,
                observed,
            }))
        }
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "unrecognized outcome tag")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips() {
        let base = b"a base snapshot".to_vec();
        let update = b"an update".to_vec();
        let mut buf = Vec::new();
        write_request(&mut buf, &base, &update).expect("write succeeds");
        let mut cursor = buf.as_slice();
        let (read_base, read_update) = read_request(&mut cursor, MAX_REQUEST_FIELD_BYTES).expect("read succeeds");
        assert_eq!(read_base, base);
        assert_eq!(read_update, update);
    }

    #[test]
    fn replay_diff_request_and_result_round_trip() {
        let request = ReplayDiffRequest {
            snapshot_frontier: vec![1, 2],
            updates: vec![ReplayDiffUpdate {
                bytes: vec![3, 4],
                before_frontier: vec![5],
                after_frontier: vec![6],
            }],
            from_frontier: vec![7],
            to_frontier: vec![8],
        };
        let encoded = encode_replay_diff_request(&request).expect("request encodes");
        assert_eq!(decode_replay_diff_request(&encoded).expect("request decodes"), request);

        let result = ReplayDiffResult {
            semantic_diff: SemanticDiff::default(),
            to_snapshot: SemanticSnapshot::default(),
            from_title: "before".to_string(),
            to_title: "after".to_string(),
        };
        let encoded = encode_replay_diff_result(&result).expect("result encodes");
        assert_eq!(decode_replay_diff_result(&encoded).expect("result decodes"), result);
    }

    #[test]
    fn success_outcome_round_trips() {
        let outcome = Outcome::Success {
            snapshot: b"exported snapshot bytes".to_vec(),
        };
        let payload = encode_outcome(&outcome).expect("encode succeeds");
        let decoded = decode_outcome(&payload).expect("decode succeeds");
        assert_eq!(decoded, outcome);
    }

    #[test]
    fn decode_failed_outcome_round_trips_with_known_input_field() {
        let outcome = Outcome::Rejected(CollabError::DecodeFailed {
            input: "update",
            reason: "bad bytes".to_string(),
        });
        let payload = encode_outcome(&outcome).expect("encode succeeds");
        let decoded = decode_outcome(&payload).expect("decode succeeds");
        assert_eq!(decoded, outcome);
    }

    #[test]
    fn limit_exceeded_outcome_round_trips_for_every_check_snapshot_limit_kind() {
        for limit_kind in [
            "tree_depth",
            "container_count",
            "document_block_count",
            "text_block_chars",
            "document_text_chars",
        ] {
            let outcome = Outcome::Rejected(CollabError::LimitExceeded {
                limit_kind,
                limit: 10,
                observed: 11,
            });
            let payload = encode_outcome(&outcome).expect("encode succeeds");
            let decoded = decode_outcome(&payload).expect("decode succeeds");
            assert_eq!(decoded, outcome, "limit_kind {limit_kind} must round-trip exactly");
        }
    }

    #[test]
    fn decode_rejects_an_empty_payload() {
        let result = decode_outcome(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn decode_rejects_an_unrecognized_tag() {
        let result = decode_outcome(&[255u8]);
        assert!(result.is_err());
    }

    #[test]
    fn read_field_rejects_a_declared_length_over_the_cap() {
        let mut buf = Vec::new();
        write_u32(&mut buf, 100).expect("write succeeds");
        buf.extend_from_slice(b"short");
        let mut cursor = buf.as_slice();
        let result = read_field(&mut cursor, 10);
        assert!(result.is_err());
    }
}
