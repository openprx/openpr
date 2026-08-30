//! Bounded, isolated staging for Flow package imports.
//!
//! The v0.4 limits contract freezes the package ceilings ahead of the v0.8 public import
//! surface. This module is the application-service enforcement point a ZIP64 decoder feeds: raw
//! archive bytes and each entry's expanded chunks are admitted incrementally, and every limit is
//! checked before the corresponding staging writer is touched. Canonical Flow repositories are
//! intentionally absent from this module, so no rejection can leave a partial database import.

use std::io::Write;

use serde_json::json;

use crate::error::ApiError;

use super::collab::limits::{
    IMPORT_ARCHIVE_BYTES_MAX, IMPORT_COMPRESSION_RATIO_MAX, IMPORT_ENTRY_COUNT_MAX, IMPORT_EXPANDED_BYTES_MAX,
};

#[derive(Debug, Clone, Copy)]
struct EntryBudget {
    compressed_bytes: u64,
    expanded_bytes: u64,
}

/// Stateful sink shared by multipart, strict-base64, and trusted-staging package producers.
///
/// `archive_stage` and `expanded_stage` must both be isolated artifact storage, never canonical
/// object/document tables.
pub struct BoundedImportStager<A, E> {
    archive_stage: A,
    expanded_stage: E,
    archive_bytes: u64,
    expanded_bytes: u64,
    entry_count: u64,
    archive_finished: bool,
    current_entry: Option<EntryBudget>,
}

impl<A: Write, E: Write> BoundedImportStager<A, E> {
    pub const fn new(archive_stage: A, expanded_stage: E) -> Self {
        Self {
            archive_stage,
            expanded_stage,
            archive_bytes: 0,
            expanded_bytes: 0,
            entry_count: 0,
            archive_finished: false,
            current_entry: None,
        }
    }

    /// Streams decoded package bytes into isolated archive staging. The prospective size is
    /// checked before `write_all`, so boundary+1 writes zero bytes from the rejected chunk.
    pub fn write_archive_chunk(&mut self, chunk: &[u8]) -> Result<(), ApiError> {
        if self.archive_finished {
            return Err(ApiError::invalid_update(
                "archive bytes arrived after archive staging finished",
            ));
        }
        let chunk_bytes = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        let observed = self.archive_bytes.saturating_add(chunk_bytes);
        check_limit(
            observed,
            IMPORT_ARCHIVE_BYTES_MAX,
            "import_archive_bytes",
            "import archive exceeds the fixed decoded byte ceiling",
        )?;
        self.archive_stage.write_all(chunk).map_err(|_| ApiError::Internal)?;
        self.archive_bytes = observed;
        Ok(())
    }

    /// Seals the archive-byte phase. A decoder may only start entries after this call, which makes
    /// the whole-package compression-ratio denominator immutable while expanded bytes stream.
    pub fn finish_archive(&mut self) -> Result<(), ApiError> {
        if self.archive_finished {
            return Err(ApiError::invalid_update("archive staging was already finished"));
        }
        self.archive_finished = true;
        Ok(())
    }

    /// Starts one ZIP entry using the decoder-verified compressed byte count from its central
    /// directory/local-header cross-check. Count enforcement happens before any entry bytes write.
    pub fn begin_entry(&mut self, compressed_bytes: u64) -> Result<(), ApiError> {
        if !self.archive_finished {
            return Err(ApiError::invalid_update(
                "entry expansion started before archive staging finished",
            ));
        }
        if self.current_entry.is_some() {
            return Err(ApiError::invalid_update("the previous import entry is still open"));
        }
        let observed = self.entry_count.saturating_add(1);
        check_limit(
            observed,
            IMPORT_ENTRY_COUNT_MAX,
            "import_entry_count",
            "import archive contains too many entries",
        )?;
        self.entry_count = observed;
        self.current_entry = Some(EntryBudget {
            compressed_bytes,
            expanded_bytes: 0,
        });
        Ok(())
    }

    /// Streams one bounded decoder output chunk into isolated expanded staging. Expanded bytes,
    /// per-entry ratio, and whole-package ratio are all checked on the prospective counters before
    /// `write_all`; a zip bomb is therefore stopped at the first disallowed byte, not after full
    /// decompression or allocation.
    pub fn write_expanded_chunk(&mut self, chunk: &[u8]) -> Result<(), ApiError> {
        let Some(entry) = self.current_entry else {
            return Err(ApiError::invalid_update(
                "expanded bytes arrived outside an import entry",
            ));
        };
        let chunk_bytes = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        let entry_expanded = entry.expanded_bytes.saturating_add(chunk_bytes);
        let total_expanded = self.expanded_bytes.saturating_add(chunk_bytes);
        check_limit(
            total_expanded,
            IMPORT_EXPANDED_BYTES_MAX,
            "import_expanded_bytes",
            "import package exceeds the fixed expanded byte ceiling",
        )?;
        check_ratio(entry_expanded, entry.compressed_bytes)?;
        check_ratio(total_expanded, self.archive_bytes)?;

        self.expanded_stage.write_all(chunk).map_err(|_| ApiError::Internal)?;
        self.expanded_bytes = total_expanded;
        self.current_entry = Some(EntryBudget {
            compressed_bytes: entry.compressed_bytes,
            expanded_bytes: entry_expanded,
        });
        Ok(())
    }

    pub fn finish_entry(&mut self) -> Result<(), ApiError> {
        if self.current_entry.take().is_none() {
            return Err(ApiError::invalid_update("no import entry is open"));
        }
        Ok(())
    }

    #[must_use]
    pub const fn archive_bytes(&self) -> u64 {
        self.archive_bytes
    }

    #[must_use]
    pub const fn expanded_bytes(&self) -> u64 {
        self.expanded_bytes
    }

    #[must_use]
    pub const fn entry_count(&self) -> u64 {
        self.entry_count
    }
}

fn check_limit(observed: u64, limit: u64, limit_kind: &'static str, message: &'static str) -> Result<(), ApiError> {
    if observed > limit {
        return Err(ApiError::limit_exceeded(
            message,
            limit_kind,
            Some(json!(limit)),
            Some(json!(observed)),
            None,
        ));
    }
    Ok(())
}

fn check_ratio(expanded_bytes: u64, compressed_bytes: u64) -> Result<(), ApiError> {
    let allowed = compressed_bytes.saturating_mul(IMPORT_COMPRESSION_RATIO_MAX);
    if expanded_bytes > allowed {
        let observed_ratio = expanded_bytes
            .saturating_add(compressed_bytes.saturating_sub(1))
            .checked_div(compressed_bytes)
            .unwrap_or(u64::MAX);
        return Err(ApiError::limit_exceeded(
            "import entry or package exceeds the fixed compression ratio ceiling",
            "import_compression_ratio",
            Some(json!(IMPORT_COMPRESSION_RATIO_MAX)),
            Some(json!(observed_ratio)),
            None,
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use std::io;

    use super::*;
    use crate::error::{ApiError, ApiErrorKind};

    #[derive(Default)]
    struct CountingSink(u64);

    impl Write for CountingSink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0 = self.0.saturating_add(u64::try_from(buf.len()).unwrap_or(u64::MAX));
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn assert_limit(err: ApiError, kind: &str, limit: u64) {
        assert_eq!(err.kind(), ApiErrorKind::LimitExceeded);
        let ApiError::Typed { details, .. } = err else {
            panic!("expected typed limit error");
        };
        let details = details.expect("details");
        assert_eq!(details["limit_kind"], kind);
        assert_eq!(details["limit"], limit);
    }

    #[test]
    fn import_archive_bytes_exact_boundary_accepted_plus_one_rejected_with_zero_partial_chunk_write() {
        let mut stager = BoundedImportStager::new(CountingSink::default(), CountingSink::default());
        let chunk = vec![0u8; 1_048_576];
        for _ in 0..128 {
            stager.write_archive_chunk(&chunk).expect("exact boundary streams");
        }
        assert_eq!(stager.archive_bytes(), IMPORT_ARCHIVE_BYTES_MAX);
        let before = stager.archive_stage.0;
        let err = stager.write_archive_chunk(&[0]).expect_err("plus one rejects");
        assert_eq!(stager.archive_stage.0, before, "rejected chunk writes zero bytes");
        assert_limit(err, "import_archive_bytes", IMPORT_ARCHIVE_BYTES_MAX);
    }

    #[test]
    fn import_expanded_bytes_exact_boundary_accepted_plus_one_rejected_with_zero_partial_chunk_write() {
        let mut stager = BoundedImportStager::new(CountingSink::default(), CountingSink::default());
        let archive_chunk = vec![0u8; 1_048_576];
        for _ in 0..6 {
            stager.write_archive_chunk(&archive_chunk).expect("archive stages");
        }
        stager.finish_archive().expect("archive finishes");
        stager.begin_entry(6_291_456).expect("entry starts");
        for _ in 0..512 {
            stager
                .write_expanded_chunk(&archive_chunk)
                .expect("exact boundary streams");
        }
        assert_eq!(stager.expanded_bytes(), IMPORT_EXPANDED_BYTES_MAX);
        let before = stager.expanded_stage.0;
        let err = stager.write_expanded_chunk(&[0]).expect_err("plus one rejects");
        assert_eq!(stager.expanded_stage.0, before, "rejected chunk writes zero bytes");
        assert_limit(err, "import_expanded_bytes", IMPORT_EXPANDED_BYTES_MAX);
    }

    #[test]
    fn import_entry_count_exact_boundary_accepted_plus_one_rejected_before_entry_write() {
        let mut stager = BoundedImportStager::new(CountingSink::default(), CountingSink::default());
        stager.write_archive_chunk(&[0]).expect("archive stages");
        stager.finish_archive().expect("archive finishes");
        for _ in 0..IMPORT_ENTRY_COUNT_MAX {
            stager.begin_entry(0).expect("entry within boundary starts");
            stager.finish_entry().expect("entry finishes");
        }
        assert_eq!(stager.entry_count(), IMPORT_ENTRY_COUNT_MAX);
        let err = stager.begin_entry(0).expect_err("plus one rejects");
        assert_limit(err, "import_entry_count", IMPORT_ENTRY_COUNT_MAX);
    }

    #[test]
    fn import_compression_ratio_exact_boundary_accepted_plus_one_rejected_streaming() {
        let mut stager = BoundedImportStager::new(CountingSink::default(), CountingSink::default());
        stager.write_archive_chunk(&[0]).expect("archive stages");
        stager.finish_archive().expect("archive finishes");
        stager.begin_entry(1).expect("entry starts");
        stager
            .write_expanded_chunk(&vec![0; usize::try_from(IMPORT_COMPRESSION_RATIO_MAX).expect("fits")])
            .expect("exact ratio streams");
        let before = stager.expanded_stage.0;
        let err = stager.write_expanded_chunk(&[0]).expect_err("ratio plus one rejects");
        assert_eq!(stager.expanded_stage.0, before, "rejected byte is never staged");
        assert_limit(err, "import_compression_ratio", IMPORT_COMPRESSION_RATIO_MAX);
    }
}
