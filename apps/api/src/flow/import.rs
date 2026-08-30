//! Bounded, isolated staging for Flow package imports.
//!
//! The v0.4 limits contract freezes the package ceilings ahead of the v0.8 public import
//! surface. This module is the application-service enforcement point a ZIP64 decoder feeds: raw
//! archive bytes and each entry's expanded chunks are admitted incrementally, and every limit is
//! checked before the corresponding staging writer is touched. Canonical Flow repositories are
//! intentionally absent from this module, so no rejection can leave a partial database import.

use std::io::{Read, Write};

use base64::engine::general_purpose::STANDARD;
use base64::read::DecoderReader;
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
    poisoned: bool,
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
            poisoned: false,
            current_entry: None,
        }
    }

    fn ensure_active(&self) -> Result<(), ApiError> {
        if self.poisoned {
            return Err(ApiError::invalid_update(
                "import staging cannot continue after a prior decoder, limit, or storage failure",
            ));
        }
        Ok(())
    }

    const fn fail<T>(&mut self, error: ApiError) -> Result<T, ApiError> {
        self.poisoned = true;
        Err(error)
    }

    /// Streams decoded package bytes into isolated archive staging. The prospective size is
    /// checked before `write_all`, so boundary+1 writes zero bytes from the rejected chunk.
    pub fn write_archive_chunk(&mut self, chunk: &[u8]) -> Result<(), ApiError> {
        self.ensure_active()?;
        if self.archive_finished {
            return Err(ApiError::invalid_update(
                "archive bytes arrived after archive staging finished",
            ));
        }
        let chunk_bytes = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        let observed = self.archive_bytes.saturating_add(chunk_bytes);
        if let Err(error) = check_limit(
            observed,
            IMPORT_ARCHIVE_BYTES_MAX,
            "import_archive_bytes",
            "import archive exceeds the fixed decoded byte ceiling",
        ) {
            return self.fail(error);
        }
        if self.archive_stage.write_all(chunk).is_err() {
            return self.fail(ApiError::Internal);
        }
        self.archive_bytes = observed;
        Ok(())
    }

    /// Pulls multipart file bytes or a trusted staging object through a fixed-size buffer into
    /// isolated archive storage. This is the common decoded-package application-service entry
    /// point; [`Self::stage_inline_base64_archive`] wraps its decoder around the same method.
    pub fn stage_archive<R: Read>(&mut self, mut source: R) -> Result<(), ApiError> {
        self.ensure_active()?;
        let mut chunk = [0u8; 64 * 1024];
        loop {
            let read = match source.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => read,
                Err(_) => return self.fail(ApiError::invalid_update("import archive stream failed")),
            };
            let Some(decoded) = chunk.get(..read) else {
                return self.fail(ApiError::Internal);
            };
            self.write_archive_chunk(decoded)?;
        }
        Ok(())
    }

    /// Strictly decodes an inline base64 package through a fixed-size buffer into the same
    /// archive limiter used by multipart and trusted-staging producers. The decoder never owns a
    /// fully decoded package, and a limit failure poisons this stager so a caller cannot catch the
    /// error and resume after the rejected byte.
    pub fn stage_inline_base64_archive<R: Read>(&mut self, source: R) -> Result<(), ApiError> {
        self.ensure_active()?;
        let decoder = DecoderReader::new(source, &STANDARD);
        self.stage_archive(decoder)
    }

    /// Seals the archive-byte phase. A decoder may only start entries after this call, which makes
    /// the whole-package compression-ratio denominator immutable while expanded bytes stream.
    pub fn finish_archive(&mut self) -> Result<(), ApiError> {
        self.ensure_active()?;
        if self.archive_finished {
            return Err(ApiError::invalid_update("archive staging was already finished"));
        }
        self.archive_finished = true;
        Ok(())
    }

    /// Starts one ZIP entry using the decoder-verified compressed byte count from its central
    /// directory/local-header cross-check. Count enforcement happens before any entry bytes write.
    pub fn begin_entry(&mut self, compressed_bytes: u64) -> Result<(), ApiError> {
        self.ensure_active()?;
        if !self.archive_finished {
            return Err(ApiError::invalid_update(
                "entry expansion started before archive staging finished",
            ));
        }
        if self.current_entry.is_some() {
            return Err(ApiError::invalid_update("the previous import entry is still open"));
        }
        let observed = self.entry_count.saturating_add(1);
        if let Err(error) = check_limit(
            observed,
            IMPORT_ENTRY_COUNT_MAX,
            "import_entry_count",
            "import archive contains too many entries",
        ) {
            return self.fail(error);
        }
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
        self.ensure_active()?;
        let Some(entry) = self.current_entry else {
            return Err(ApiError::invalid_update(
                "expanded bytes arrived outside an import entry",
            ));
        };
        let chunk_bytes = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        let entry_expanded = entry.expanded_bytes.saturating_add(chunk_bytes);
        let total_expanded = self.expanded_bytes.saturating_add(chunk_bytes);
        if let Err(error) = check_limit(
            total_expanded,
            IMPORT_EXPANDED_BYTES_MAX,
            "import_expanded_bytes",
            "import package exceeds the fixed expanded byte ceiling",
        ) {
            return self.fail(error);
        }
        if let Err(error) = check_ratio(entry_expanded, entry.compressed_bytes) {
            return self.fail(error);
        }
        if let Err(error) = check_ratio(total_expanded, self.archive_bytes) {
            return self.fail(error);
        }

        if self.expanded_stage.write_all(chunk).is_err() {
            return self.fail(ApiError::Internal);
        }
        self.expanded_bytes = total_expanded;
        self.current_entry = Some(EntryBudget {
            compressed_bytes: entry.compressed_bytes,
            expanded_bytes: entry_expanded,
        });
        Ok(())
    }

    /// Pulls one decompressor's output through a fixed-size buffer and the three expansion-side
    /// ceilings. A ZIP64 reader supplies the header-verified `compressed_bytes`; this method
    /// supplies the bounded streaming enforcement call site and never buffers the full entry.
    pub fn stage_expanded_entry<R: Read>(&mut self, mut expanded: R, compressed_bytes: u64) -> Result<(), ApiError> {
        self.begin_entry(compressed_bytes)?;
        let mut chunk = [0u8; 64 * 1024];
        loop {
            let read = match expanded.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => read,
                Err(_) => return self.fail(ApiError::invalid_update("import entry decompression failed")),
            };
            let Some(decoded) = chunk.get(..read) else {
                return self.fail(ApiError::Internal);
            };
            self.write_expanded_chunk(decoded)?;
        }
        self.finish_entry()
    }

    pub fn finish_entry(&mut self) -> Result<(), ApiError> {
        self.ensure_active()?;
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
    use std::io::{self, Cursor};

    use super::*;
    use crate::error::{ApiError, ApiErrorKind};
    use base64::Engine as _;

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
        stager
            .stage_archive(io::repeat(0).take(IMPORT_ARCHIVE_BYTES_MAX))
            .expect("exact boundary streams");
        assert_eq!(stager.archive_bytes(), IMPORT_ARCHIVE_BYTES_MAX);
        let before = stager.archive_stage.0;
        let err = stager.write_archive_chunk(&[0]).expect_err("plus one rejects");
        assert_eq!(stager.archive_stage.0, before, "rejected chunk writes zero bytes");
        assert_limit(err, "import_archive_bytes", IMPORT_ARCHIVE_BYTES_MAX);
    }

    #[test]
    fn import_expanded_bytes_exact_boundary_accepted_plus_one_rejected_with_zero_partial_chunk_write() {
        let mut stager = BoundedImportStager::new(CountingSink::default(), CountingSink::default());
        stager
            .stage_archive(io::repeat(0).take(6_291_456))
            .expect("archive stages");
        stager.finish_archive().expect("archive finishes");
        stager
            .begin_entry(6_291_456)
            .expect("entry starts for boundary continuation");
        let chunk = vec![0u8; 1_048_576];
        for _ in 0..512 {
            stager.write_expanded_chunk(&chunk).expect("exact boundary streams");
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
        let poisoned = stager.finish_entry().expect_err("a rejected stream cannot be resumed");
        assert_eq!(poisoned.kind(), ApiErrorKind::InvalidUpdate);
    }

    #[test]
    fn inline_base64_archive_and_expanded_entry_use_bounded_streaming_call_sites() {
        let package = b"compressed-package";
        let encoded = STANDARD.encode(package);
        let mut stager = BoundedImportStager::new(CountingSink::default(), CountingSink::default());
        stager
            .stage_inline_base64_archive(Cursor::new(encoded))
            .expect("strict base64 streams");
        assert_eq!(stager.archive_bytes(), u64::try_from(package.len()).expect("fits"));
        stager.finish_archive().expect("archive finishes");

        let expanded = vec![0u8; package.len() * 2];
        stager
            .stage_expanded_entry(Cursor::new(&expanded), u64::try_from(package.len()).expect("fits"))
            .expect("decoder output streams");
        assert_eq!(stager.entry_count(), 1);
        assert_eq!(stager.expanded_bytes(), u64::try_from(expanded.len()).expect("fits"));
    }

    #[test]
    fn malformed_inline_base64_poisoning_prevents_resume() {
        let mut stager = BoundedImportStager::new(CountingSink::default(), CountingSink::default());
        let malformed = Cursor::new("not canonical base64@@");
        let error = stager
            .stage_inline_base64_archive(malformed)
            .expect_err("malformed base64 rejects");
        assert_eq!(error.kind(), ApiErrorKind::InvalidUpdate);
        let resumed = stager
            .write_archive_chunk(b"later")
            .expect_err("rejected decoder is terminal");
        assert_eq!(resumed.kind(), ApiErrorKind::InvalidUpdate);
    }
}
