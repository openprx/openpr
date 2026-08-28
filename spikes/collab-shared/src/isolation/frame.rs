//! Result frame protocol (ADR-0014 section 2): `[u32 length][u32 crc32][payload]`, little-endian,
//! payload capped at 1 MiB.
//!
//! No `unsafe` here — this is pure byte-slice encode/decode, independent of how the bytes are
//! actually transported (the caller in `zygote.rs` reads them off a pipe).

/// Payload cap from ADR-0014 section 2: "长度上限 1 MiB".
pub const MAX_FRAME_PAYLOAD_BYTES: usize = 1024 * 1024;
const HEADER_BYTES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameEncodeError {
    PayloadTooLarge { actual_bytes: usize, max_bytes: usize },
}

/// Encodes `payload` into a length-prefixed, CRC32-checked frame.
///
/// # Errors
/// Returns [`FrameEncodeError::PayloadTooLarge`] if `payload` exceeds [`MAX_FRAME_PAYLOAD_BYTES`].
pub fn encode(payload: &[u8]) -> Result<Vec<u8>, FrameEncodeError> {
    if payload.len() > MAX_FRAME_PAYLOAD_BYTES {
        return Err(FrameEncodeError::PayloadTooLarge {
            actual_bytes: payload.len(),
            max_bytes: MAX_FRAME_PAYLOAD_BYTES,
        });
    }
    // `payload.len() <= MAX_FRAME_PAYLOAD_BYTES` (1 MiB), which fits comfortably in a u32.
    #[allow(clippy::cast_possible_truncation)]
    let length = payload.len() as u32;
    let crc = crc32fast::hash(payload);
    let mut out = Vec::with_capacity(HEADER_BYTES + payload.len());
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Why a received frame was rejected. ADR-0014 section 2: "`read` 到 EOF 与 `wait4` 退出状态双重确认，
/// 不一致判 `crashed`".
///
/// Every variant here is a reason the caller should treat the case as `crashed` rather than
/// trusting any payload bytes it managed to parse out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDecodeError {
    /// Fewer than 8 bytes were read at all — no usable header.
    TooShortForHeader { received_bytes: usize },
    /// The header declared a length larger than [`MAX_FRAME_PAYLOAD_BYTES`].
    DeclaredLengthTooLarge { declared_bytes: u32, max_bytes: usize },
    /// The header declared more payload bytes than were actually received (truncated write,
    /// typically because the child was killed mid-write).
    Truncated { declared_bytes: u32, received_bytes: usize },
    /// The full frame was received but its CRC32 did not match the declared payload.
    CrcMismatch { declared_crc32: u32, computed_crc32: u32 },
    /// Bytes remained after the declared payload ended (the writer sent more than one frame, or
    /// garbage trailed a valid-looking header).
    TrailingBytes { extra_bytes: usize },
}

/// Decodes a full frame that was read from a pipe (or any other exact byte source) up to EOF.
///
/// # Errors
/// See [`FrameDecodeError`] for every rejection reason; all of them mean "treat this case as
/// `crashed`", not "trust a partially-parsed payload".
pub fn decode(bytes: &[u8]) -> Result<Vec<u8>, FrameDecodeError> {
    if bytes.len() < HEADER_BYTES {
        return Err(FrameDecodeError::TooShortForHeader {
            received_bytes: bytes.len(),
        });
    }
    // `.get(..)` + `try_into()` rather than direct indexing (`indexing_slicing` is a deny-level
    // workspace lint): the length check above guarantees these are in bounds, but this avoids a
    // panicking index expression entirely instead of relying on that invariant holding here too.
    let (Some(length_bytes), Some(crc_bytes)) = (
        bytes.get(0..4).and_then(|slice| <[u8; 4]>::try_from(slice).ok()),
        bytes.get(4..8).and_then(|slice| <[u8; 4]>::try_from(slice).ok()),
    ) else {
        return Err(FrameDecodeError::TooShortForHeader {
            received_bytes: bytes.len(),
        });
    };
    let declared_length = u32::from_le_bytes(length_bytes);
    let declared_crc32 = u32::from_le_bytes(crc_bytes);

    if declared_length as usize > MAX_FRAME_PAYLOAD_BYTES {
        return Err(FrameDecodeError::DeclaredLengthTooLarge {
            declared_bytes: declared_length,
            max_bytes: MAX_FRAME_PAYLOAD_BYTES,
        });
    }

    let payload_start = HEADER_BYTES;
    let payload_end = payload_start + declared_length as usize;
    if bytes.len() < payload_end {
        return Err(FrameDecodeError::Truncated {
            declared_bytes: declared_length,
            received_bytes: bytes.len(),
        });
    }
    if bytes.len() > payload_end {
        return Err(FrameDecodeError::TrailingBytes {
            extra_bytes: bytes.len() - payload_end,
        });
    }

    let Some(payload) = bytes.get(payload_start..payload_end) else {
        return Err(FrameDecodeError::Truncated {
            declared_bytes: declared_length,
            received_bytes: bytes.len(),
        });
    };
    let computed_crc32 = crc32fast::hash(payload);
    if computed_crc32 != declared_crc32 {
        return Err(FrameDecodeError::CrcMismatch {
            declared_crc32,
            computed_crc32,
        });
    }

    Ok(payload.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_valid_payload() {
        let payload = b"hello isolation host".to_vec();
        let framed = encode(&payload).unwrap_or_default();
        let decoded = decode(&framed).unwrap_or_default();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn rejects_oversized_payload_at_encode_time() {
        let payload = vec![0u8; MAX_FRAME_PAYLOAD_BYTES + 1];
        let result = encode(&payload);
        assert!(matches!(result, Err(FrameEncodeError::PayloadTooLarge { .. })));
    }

    #[test]
    fn rejects_truncated_frame() {
        let payload = b"hello isolation host".to_vec();
        let mut framed = encode(&payload).unwrap_or_default();
        framed.truncate(framed.len() - 3);
        let result = decode(&framed);
        assert!(matches!(result, Err(FrameDecodeError::Truncated { .. })));
    }

    #[test]
    #[allow(clippy::indexing_slicing)]
    fn rejects_corrupted_crc() {
        let payload = b"hello isolation host".to_vec();
        let mut framed = encode(&payload).unwrap_or_default();
        let last = framed.len() - 1;
        framed[last] ^= 0xFF;
        let result = decode(&framed);
        assert!(matches!(result, Err(FrameDecodeError::CrcMismatch { .. })));
    }

    #[test]
    fn rejects_too_short_header() {
        let result = decode(&[1, 2, 3]);
        assert!(matches!(result, Err(FrameDecodeError::TooShortForHeader { .. })));
    }

    #[test]
    fn rejects_declared_length_over_cap() {
        let mut framed = Vec::new();
        framed.extend_from_slice(&(u32::try_from(MAX_FRAME_PAYLOAD_BYTES + 1).unwrap_or(u32::MAX)).to_le_bytes());
        framed.extend_from_slice(&0u32.to_le_bytes());
        let result = decode(&framed);
        assert!(matches!(result, Err(FrameDecodeError::DeclaredLengthTooLarge { .. })));
    }
}
