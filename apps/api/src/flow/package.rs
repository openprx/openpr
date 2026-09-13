//! Strict ZIP64 codec for the frozen Flow export-package v1 format.
//!
//! The ZIP reader is intentionally stricter than a general-purpose extractor: it cross-checks
//! local and central headers, rejects data descriptors, and streams every expanded byte through
//! [`super::import::BoundedImportStager`] before trusting checksums. Package validation never
//! writes canonical Flow tables; promotion is a separate application-service transaction.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_casefold::UnicodeCaseFold as _;
use unicode_normalization::UnicodeNormalization as _;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::ApiError;

use super::collab::limits::IMPORT_ENTRY_COUNT_MAX;
use super::import::BoundedImportStager;

pub const PACKAGE_SCHEMA: &str = "sylvode.flow.export-package.v1";
pub const FLOW_SCHEMA_VERSION: u32 = 1;
pub const ENGINE_NAME: &str = "loro";
pub const ENGINE_CRATE_VERSION: &str = "1.13.9";
pub const ENGINE_WIRE_FORMAT_VERSION: u32 = 1;

const MANIFEST_PATH: &str = "manifest.json";
const CHECKSUMS_PATH: &str = "checksums.sha256";
const LOCAL_HEADER_SIGNATURE: u32 = 0x0403_4b50;
const CENTRAL_HEADER_SIGNATURE: u32 = 0x0201_4b50;
const END_OF_CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0605_4b50;
const ZIP64_END_OF_CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0606_4b50;
const ZIP64_END_LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
const FLAG_ENCRYPTED: u16 = 1;
const FLAG_DATA_DESCRIPTOR: u16 = 1 << 3;
const ZIP64_EXTRA_ID: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageProducer {
    pub product: String,
    pub version: String,
    pub source_head: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSource {
    pub workspace_id: String,
    pub scope: String,
    pub root_object_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageEngine {
    pub name: String,
    pub crate_version: String,
    pub wire_format_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageHistory {
    pub included: bool,
    pub through_seq_by_document: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageCounts {
    pub objects: u64,
    pub documents: u64,
    pub updates: u64,
    pub relations: u64,
    pub lineage: u64,
    pub events: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageMember {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportPolicy {
    pub complete: bool,
    pub permission_snapshot_at: String,
}

/// Frozen manifest fields. Unknown fields are accepted by Serde so a v1 reader remains compatible
/// with future optional v1 fields; required fields below are still strongly typed and validated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportPackageManifest {
    pub schema: String,
    pub package_id: String,
    pub created_at: String,
    pub producer: PackageProducer,
    pub source: PackageSource,
    pub flow_schema_version: u32,
    pub engine: PackageEngine,
    pub history: PackageHistory,
    pub counts: PackageCounts,
    pub members: Vec<PackageMember>,
    pub export_policy: ExportPolicy,
}

#[derive(Debug, Clone)]
pub struct PackageMemberInput {
    pub path: String,
    pub kind: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltPackage {
    pub bytes: Vec<u8>,
    pub package_sha256: String,
    pub manifest: ExportPackageManifest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPackage {
    pub package_sha256: String,
    pub manifest: ExportPackageManifest,
}

/// Produces a deterministic, stored ZIP64 archive. The function derives `manifest.members`; a
/// caller cannot forge a size or digest that disagrees with the bytes being exported.
pub fn build_package(
    mut manifest: ExportPackageManifest,
    mut inputs: Vec<PackageMemberInput>,
) -> Result<BuiltPackage, ApiError> {
    inputs.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
    validate_input_paths(&inputs)?;
    manifest.members = inputs
        .iter()
        .map(|input| PackageMember {
            path: input.path.clone(),
            bytes: u64::try_from(input.bytes.len()).unwrap_or(u64::MAX),
            sha256: sha256_hex(&input.bytes),
            kind: input.kind.clone(),
        })
        .collect();
    validate_manifest_compatibility(&manifest)?;
    validate_member_layout(&manifest, &manifest.members)?;

    let manifest_bytes = serde_jcs::to_vec(&manifest).map_err(|_| ApiError::Internal)?;
    let checksums = render_checksums(&manifest_bytes, &manifest.members);
    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .large_file(true)
        .unix_permissions(0o600);
    writer
        .start_file(MANIFEST_PATH, options)
        .map_err(|_| ApiError::Internal)?;
    writer.write_all(&manifest_bytes).map_err(|_| ApiError::Internal)?;
    writer
        .start_file(CHECKSUMS_PATH, options)
        .map_err(|_| ApiError::Internal)?;
    writer.write_all(&checksums).map_err(|_| ApiError::Internal)?;
    for input in inputs {
        writer
            .start_file(&input.path, options)
            .map_err(|_| ApiError::Internal)?;
        writer.write_all(&input.bytes).map_err(|_| ApiError::Internal)?;
    }
    let bytes = writer.finish().map_err(|_| ApiError::Internal)?.into_inner();
    let package_sha256 = sha256_hex(&bytes);
    Ok(BuiltPackage {
        bytes,
        package_sha256,
        manifest,
    })
}

/// Validates an archive without materializing expanded member bodies. Each body is decompressed in
/// 64 KiB chunks, counted by the shared import limiter, and hashed before the manifest is trusted.
pub fn verify_package<R: Read + Seek>(
    mut reader: R,
    expected_package_sha256: Option<&str>,
) -> Result<VerifiedPackage, ApiError> {
    reader.seek(SeekFrom::Start(0)).map_err(|_| invalid_archive())?;
    let mut archive_hasher = Sha256::new();
    let mut stager = BoundedImportStager::new(io::sink(), io::sink());
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut chunk).map_err(|_| invalid_archive())?;
        if read == 0 {
            break;
        }
        let bytes = chunk.get(..read).ok_or_else(invalid_archive)?;
        archive_hasher.update(bytes);
        stager.write_archive_chunk(bytes)?;
    }
    stager.finish_archive()?;
    let package_sha256 = digest_hex(archive_hasher.finalize().as_slice());
    if expected_package_sha256.is_some_and(|expected| expected != package_sha256) {
        return Err(ApiError::checksum_mismatch(
            "archive SHA-256 does not match the uploaded artifact",
        ));
    }

    reader.seek(SeekFrom::Start(0)).map_err(|_| invalid_archive())?;
    let mut raw = Vec::new();
    reader.read_to_end(&mut raw).map_err(|_| invalid_archive())?;
    let declared_entries = preflight_entry_count(&raw)?;
    if declared_entries > IMPORT_ENTRY_COUNT_MAX {
        return Err(ApiError::limit_exceeded(
            "import archive contains too many entries",
            "import_entry_count",
            Some(serde_json::json!(IMPORT_ENTRY_COUNT_MAX)),
            Some(serde_json::json!(declared_entries)),
            None,
        ));
    }
    let mut archive = ZipArchive::new(Cursor::new(raw.as_slice())).map_err(|_| invalid_archive())?;
    if archive.offset() != 0
        || !archive.comment().is_empty()
        || archive.has_overlapping_files().map_err(|_| invalid_archive())?
        || u64::try_from(archive.len()).unwrap_or(u64::MAX) != declared_entries
    {
        return Err(invalid_archive());
    }

    let mut exact_paths = HashSet::new();
    let mut folded_paths = HashSet::new();
    let mut observed = BTreeMap::new();
    let mut manifest_bytes = None;
    let mut checksum_bytes = None;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|_| invalid_archive())?;
        let path = strict_entry_path(file.name_raw())?;
        if !exact_paths.insert(path.clone()) || !folded_paths.insert(path.case_fold().collect::<String>()) {
            return Err(ApiError::unsupported_format(
                "duplicate or case-fold-colliding archive path",
            ));
        }
        validate_member_path(&path)?;
        validate_raw_headers(raw.as_slice(), &file, &path)?;
        if file.encrypted() || file.is_dir() || file.is_symlink() || !file.is_file() {
            return Err(ApiError::unsupported_format(
                "archive entries must be unencrypted regular files",
            ));
        }
        if !matches!(
            file.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err(ApiError::unsupported_format(
                "archive entry uses an unsupported compression method",
            ));
        }
        let declared_size = file.size();
        let declared_compressed = file.compressed_size();
        stager.begin_entry(declared_compressed)?;
        let mut hasher = Sha256::new();
        let mut body = if matches!(path.as_str(), MANIFEST_PATH | CHECKSUMS_PATH) {
            Some(Vec::new())
        } else {
            None
        };
        let mut expanded = 0u64;
        loop {
            let read = file.read(&mut chunk).map_err(|_| invalid_archive())?;
            if read == 0 {
                break;
            }
            let bytes = chunk.get(..read).ok_or_else(invalid_archive)?;
            expanded = expanded.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
            stager.write_expanded_chunk(bytes)?;
            hasher.update(bytes);
            if let Some(control) = &mut body {
                control.extend_from_slice(bytes);
            }
        }
        stager.finish_entry()?;
        if expanded != declared_size {
            return Err(ApiError::checksum_mismatch(
                "expanded entry size disagrees with its ZIP header",
            ));
        }
        let member_hash = digest_hex(hasher.finalize().as_slice());
        observed.insert(path.clone(), (expanded, member_hash));
        match path.as_str() {
            MANIFEST_PATH => manifest_bytes = body,
            CHECKSUMS_PATH => checksum_bytes = body,
            _ => {}
        }
    }

    let manifest_bytes = manifest_bytes.ok_or_else(|| ApiError::unsupported_format("manifest.json is missing"))?;
    let checksum_bytes = checksum_bytes.ok_or_else(|| ApiError::unsupported_format("checksums.sha256 is missing"))?;
    let manifest_value: serde_json::Value = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| ApiError::unsupported_format("manifest is not valid UTF-8 JSON"))?;
    let canonical = serde_jcs::to_vec(&manifest_value)
        .map_err(|_| ApiError::unsupported_format("manifest cannot be canonicalized"))?;
    if canonical != manifest_bytes {
        return Err(ApiError::unsupported_format(
            "manifest.json is not RFC 8785 canonical JSON",
        ));
    }
    let manifest: ExportPackageManifest = serde_json::from_value(manifest_value)
        .map_err(|_| ApiError::unsupported_format("manifest is missing required v1 fields"))?;
    validate_manifest_compatibility(&manifest)?;
    validate_member_layout(&manifest, &manifest.members)?;
    verify_member_table(&manifest, &observed)?;
    let expected_checksums = render_checksums(&manifest_bytes, &manifest.members);
    if checksum_bytes != expected_checksums {
        return Err(ApiError::checksum_mismatch(
            "checksum file does not exactly cover manifest members",
        ));
    }
    Ok(VerifiedPackage {
        package_sha256,
        manifest,
    })
}

fn validate_input_paths(inputs: &[PackageMemberInput]) -> Result<(), ApiError> {
    let mut folded = HashSet::new();
    let mut previous: Option<&[u8]> = None;
    for input in inputs {
        validate_member_path(&input.path)?;
        if matches!(input.path.as_str(), MANIFEST_PATH | CHECKSUMS_PATH) {
            return Err(ApiError::unsupported_format(
                "package control entries are generated by the codec",
            ));
        }
        if previous == Some(input.path.as_bytes()) || !folded.insert(input.path.case_fold().collect::<String>()) {
            return Err(ApiError::unsupported_format(
                "duplicate or case-fold-colliding archive path",
            ));
        }
        previous = Some(input.path.as_bytes());
    }
    Ok(())
}

fn strict_entry_path(raw: &[u8]) -> Result<String, ApiError> {
    let path = std::str::from_utf8(raw).map_err(|_| ApiError::unsupported_format("archive entry path is not UTF-8"))?;
    if !path.chars().eq(path.nfc()) {
        return Err(ApiError::unsupported_format("archive entry path is not Unicode NFC"));
    }
    Ok(path.to_string())
}

fn validate_member_path(path: &str) -> Result<(), ApiError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(ApiError::unsupported_format(
            "archive entry is not a relative POSIX path",
        ));
    }
    let known = matches!(
        path,
        MANIFEST_PATH | CHECKSUMS_PATH | "relations/relations.jsonl" | "lineage/lineage.jsonl" | "history/events.jsonl"
    ) || is_object_path(path)
        || is_snapshot_path(path)
        || is_update_path(path);
    if !known {
        return Err(ApiError::unsupported_format(
            "archive contains an unknown top-level or member entry",
        ));
    }
    Ok(())
}

fn is_object_path(path: &str) -> bool {
    path.strip_prefix("objects/")
        .and_then(|rest| rest.strip_suffix("/object.json"))
        .is_some_and(valid_uuid_segment)
}

fn is_snapshot_path(path: &str) -> bool {
    path.strip_prefix("documents/")
        .and_then(|rest| rest.strip_suffix("/snapshot.bin"))
        .is_some_and(valid_uuid_segment)
}

fn is_update_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("documents/") else {
        return false;
    };
    let Some((document, sequence)) = rest.split_once("/updates/") else {
        return false;
    };
    valid_uuid_segment(document)
        && sequence
            .strip_suffix(".bin")
            .and_then(|value| value.parse::<i64>().ok())
            .is_some_and(|value| value > 0)
}

fn valid_uuid_segment(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok() && !value.contains('/')
}

fn expected_kind(path: &str) -> Option<&'static str> {
    if is_object_path(path) {
        Some("object")
    } else if is_snapshot_path(path) {
        Some("snapshot")
    } else if is_update_path(path) {
        Some("update")
    } else {
        match path {
            "relations/relations.jsonl" => Some("relation"),
            "lineage/lineage.jsonl" => Some("lineage"),
            "history/events.jsonl" => Some("event"),
            _ => None,
        }
    }
}

fn validate_manifest_compatibility(manifest: &ExportPackageManifest) -> Result<(), ApiError> {
    if manifest.schema != PACKAGE_SCHEMA {
        return Err(ApiError::unsupported_format("package schema major is not v1"));
    }
    if manifest.flow_schema_version != FLOW_SCHEMA_VERSION {
        return Err(ApiError::unsupported_format(
            "Flow schema version has no tested forward adapter",
        ));
    }
    if manifest.engine.name != ENGINE_NAME
        || manifest.engine.crate_version != ENGINE_CRATE_VERSION
        || manifest.engine.wire_format_version != ENGINE_WIRE_FORMAT_VERSION
    {
        return Err(ApiError::unsupported_format(
            "CRDT engine compatibility matrix has no matching corpus",
        ));
    }
    if !manifest.export_policy.complete {
        return Err(ApiError::unsupported_format("partial export packages are forbidden"));
    }
    if !matches!(manifest.source.scope.as_str(), "object" | "workspace") {
        return Err(ApiError::unsupported_format("manifest source scope is unsupported"));
    }
    if uuid::Uuid::parse_str(&manifest.package_id).is_err()
        || uuid::Uuid::parse_str(&manifest.source.workspace_id).is_err()
        || manifest.source.root_object_ids.is_empty()
        || manifest
            .source
            .root_object_ids
            .iter()
            .any(|id| uuid::Uuid::parse_str(id).is_err())
        || (manifest.source.scope == "object" && manifest.source.root_object_ids.len() != 1)
    {
        return Err(ApiError::unsupported_format("manifest source identity is invalid"));
    }
    let unique_roots: HashSet<&str> = manifest.source.root_object_ids.iter().map(String::as_str).collect();
    if unique_roots.len() != manifest.source.root_object_ids.len() {
        return Err(ApiError::unsupported_format(
            "manifest contains duplicate root object ids",
        ));
    }
    if manifest.producer.product != "sylvode"
        || semver::Version::parse(&manifest.producer.version).is_err()
        || manifest.producer.source_head.len() != 40
        || !is_lower_hex(&manifest.producer.source_head)
        || chrono::DateTime::parse_from_rfc3339(&manifest.created_at).is_err()
        || chrono::DateTime::parse_from_rfc3339(&manifest.export_policy.permission_snapshot_at).is_err()
    {
        return Err(ApiError::unsupported_format(
            "manifest producer or timestamp metadata is invalid",
        ));
    }
    Ok(())
}

fn validate_member_layout(manifest: &ExportPackageManifest, members: &[PackageMember]) -> Result<(), ApiError> {
    let mut previous: Option<&[u8]> = None;
    let mut folded = HashSet::new();
    let mut objects = 0u64;
    let mut documents = 0u64;
    let mut updates = 0u64;
    let mut snapshot_documents = HashSet::new();
    let mut relation_file = false;
    let mut lineage_file = false;
    let mut event_file = false;
    for member in members {
        validate_member_path(&member.path)?;
        let Some(kind) = expected_kind(&member.path) else {
            return Err(ApiError::unsupported_format("manifest member has an unknown kind"));
        };
        if member.kind != kind || member.sha256.len() != 64 || !is_lower_hex(&member.sha256) {
            return Err(ApiError::unsupported_format("manifest member metadata is invalid"));
        }
        if previous.is_some_and(|value| value >= member.path.as_bytes())
            || !folded.insert(member.path.case_fold().collect::<String>())
        {
            return Err(ApiError::unsupported_format(
                "manifest members are not uniquely bytewise sorted",
            ));
        }
        previous = Some(member.path.as_bytes());
        match kind {
            "object" => objects = objects.saturating_add(1),
            "snapshot" => {
                documents = documents.saturating_add(1);
                if let Some(document_id) = member
                    .path
                    .strip_prefix("documents/")
                    .and_then(|rest| rest.strip_suffix("/snapshot.bin"))
                {
                    snapshot_documents.insert(document_id.to_string());
                }
            }
            "update" => updates = updates.saturating_add(1),
            "relation" => relation_file = true,
            "lineage" => lineage_file = true,
            "event" => event_file = true,
            _ => {}
        }
        if !manifest.history.included && matches!(kind, "update" | "event") {
            return Err(ApiError::unsupported_format(
                "history members are present when history.included is false",
            ));
        }
    }
    if manifest.counts.objects != objects
        || manifest.counts.documents != documents
        || manifest.counts.updates != updates
    {
        return Err(ApiError::unsupported_format(
            "manifest member counts disagree with member paths",
        ));
    }
    if !relation_file || !lineage_file || event_file != manifest.history.included {
        return Err(ApiError::unsupported_format(
            "package is missing a required relation, lineage, or conditional history member",
        ));
    }
    let through_documents: HashSet<&str> = manifest
        .history
        .through_seq_by_document
        .keys()
        .map(String::as_str)
        .collect();
    if through_documents.len() != snapshot_documents.len()
        || snapshot_documents
            .iter()
            .any(|document| !through_documents.contains(document.as_str()))
        || manifest.history.through_seq_by_document.values().any(|seq| *seq < 0)
    {
        return Err(ApiError::unsupported_format(
            "history frontier map does not exactly cover package documents",
        ));
    }
    Ok(())
}

fn verify_member_table(
    manifest: &ExportPackageManifest,
    observed: &BTreeMap<String, (u64, String)>,
) -> Result<(), ApiError> {
    if observed.len() != manifest.members.len().saturating_add(2) {
        return Err(ApiError::checksum_mismatch(
            "manifest does not cover every archive entry",
        ));
    }
    for member in &manifest.members {
        let Some((bytes, sha256)) = observed.get(&member.path) else {
            return Err(ApiError::checksum_mismatch("manifest member is missing from archive"));
        };
        if *bytes != member.bytes || *sha256 != member.sha256 {
            return Err(ApiError::checksum_mismatch(
                "manifest member size or SHA-256 differs from archive bytes",
            ));
        }
    }
    Ok(())
}

fn render_checksums(manifest_bytes: &[u8], members: &[PackageMember]) -> Vec<u8> {
    let mut rendered = String::new();
    let _ = writeln!(rendered, "{}  {MANIFEST_PATH}", sha256_hex(manifest_bytes));
    for member in members {
        let _ = writeln!(rendered, "{}  {}", member.sha256, member.path);
    }
    rendered.into_bytes()
}

fn validate_raw_headers<R: Read + ?Sized>(
    raw: &[u8],
    file: &zip::read::ZipFile<'_, R>,
    path: &str,
) -> Result<(), ApiError> {
    let local = usize::try_from(file.header_start()).map_err(|_| invalid_archive())?;
    let central = usize::try_from(file.central_header_start()).map_err(|_| invalid_archive())?;
    if read_u32(raw, local)? != LOCAL_HEADER_SIGNATURE || read_u32(raw, central)? != CENTRAL_HEADER_SIGNATURE {
        return Err(invalid_archive());
    }
    let local_flags = read_u16(raw, local.saturating_add(6))?;
    let central_flags = read_u16(raw, central.saturating_add(8))?;
    let local_method = read_u16(raw, local.saturating_add(8))?;
    let central_method = read_u16(raw, central.saturating_add(10))?;
    if local_flags != central_flags
        || local_method != central_method
        || local_flags & (FLAG_ENCRYPTED | FLAG_DATA_DESCRIPTOR) != 0
    {
        return Err(ApiError::unsupported_format(
            "local and central ZIP headers disagree or use forbidden flags",
        ));
    }
    let local_name_len = usize::from(read_u16(raw, local.saturating_add(26))?);
    let local_extra_len = usize::from(read_u16(raw, local.saturating_add(28))?);
    let central_name_len = usize::from(read_u16(raw, central.saturating_add(28))?);
    let central_extra_len = usize::from(read_u16(raw, central.saturating_add(30))?);
    let local_name_start = local.checked_add(30).ok_or_else(invalid_archive)?;
    let central_name_start = central.checked_add(46).ok_or_else(invalid_archive)?;
    let local_name = slice(raw, local_name_start, local_name_len)?;
    let central_name = slice(raw, central_name_start, central_name_len)?;
    if local_name != path.as_bytes() || central_name != path.as_bytes() {
        return Err(ApiError::unsupported_format("local and central ZIP paths disagree"));
    }
    let local_extra_start = local_name_start
        .checked_add(local_name_len)
        .ok_or_else(invalid_archive)?;
    let local_extra = slice(raw, local_extra_start, local_extra_len)?;
    if !contains_extra_field(local_extra, ZIP64_EXTRA_ID)? {
        return Err(ApiError::unsupported_format(
            "package entry is not encoded with ZIP64 metadata",
        ));
    }
    let central_extra_start = central_name_start
        .checked_add(central_name_len)
        .ok_or_else(invalid_archive)?;
    let central_extra = slice(raw, central_extra_start, central_extra_len)?;
    let local_sizes = resolved_sizes(
        read_u32(raw, local.saturating_add(18))?,
        read_u32(raw, local.saturating_add(22))?,
        local_extra,
    )?;
    let central_sizes = resolved_sizes(
        read_u32(raw, central.saturating_add(20))?,
        read_u32(raw, central.saturating_add(24))?,
        central_extra,
    )?;
    if read_u32(raw, local.saturating_add(14))? != file.crc32()
        || read_u32(raw, central.saturating_add(16))? != file.crc32()
        || local_sizes != (file.compressed_size(), file.size())
        || central_sizes != (file.compressed_size(), file.size())
        || file.data_start()
            != Some(u64::try_from(local_extra_start.saturating_add(local_extra_len)).unwrap_or(u64::MAX))
    {
        return Err(ApiError::unsupported_format(
            "local and central ZIP CRC, size, or data offsets disagree",
        ));
    }
    Ok(())
}

fn resolved_sizes(compressed_32: u32, uncompressed_32: u32, extra: &[u8]) -> Result<(u64, u64), ApiError> {
    let mut compressed = u64::from(compressed_32);
    let mut uncompressed = u64::from(uncompressed_32);
    if compressed_32 != u32::MAX && uncompressed_32 != u32::MAX {
        return Ok((compressed, uncompressed));
    }
    let zip64 = find_extra_field(extra, ZIP64_EXTRA_ID)?
        .ok_or_else(|| ApiError::unsupported_format("ZIP64 sentinel has no ZIP64 extended information"))?;
    let mut offset = 0usize;
    if uncompressed_32 == u32::MAX {
        uncompressed = read_u64(zip64, offset)?;
        offset = offset.saturating_add(8);
    }
    if compressed_32 == u32::MAX {
        compressed = read_u64(zip64, offset)?;
    }
    Ok((compressed, uncompressed))
}

fn contains_extra_field(bytes: &[u8], wanted: u16) -> Result<bool, ApiError> {
    Ok(find_extra_field(bytes, wanted)?.is_some())
}

fn find_extra_field(mut bytes: &[u8], wanted: u16) -> Result<Option<&[u8]>, ApiError> {
    while !bytes.is_empty() {
        if bytes.len() < 4 {
            return Err(invalid_archive());
        }
        let id = read_u16(bytes, 0)?;
        let size = usize::from(read_u16(bytes, 2)?);
        let remaining = bytes.get(4..).ok_or_else(invalid_archive)?;
        let Some((_, tail)) = remaining.split_at_checked(size) else {
            return Err(invalid_archive());
        };
        if id == wanted {
            return Ok(remaining.get(..size));
        }
        bytes = tail;
    }
    Ok(None)
}

fn read_u16(raw: &[u8], offset: usize) -> Result<u16, ApiError> {
    let bytes: [u8; 2] = slice(raw, offset, 2)?.try_into().map_err(|_| invalid_archive())?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(raw: &[u8], offset: usize) -> Result<u32, ApiError> {
    let bytes: [u8; 4] = slice(raw, offset, 4)?.try_into().map_err(|_| invalid_archive())?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(raw: &[u8], offset: usize) -> Result<u64, ApiError> {
    let bytes: [u8; 8] = slice(raw, offset, 8)?.try_into().map_err(|_| invalid_archive())?;
    Ok(u64::from_le_bytes(bytes))
}

/// Reads the exact terminal EOCD (archive comments/trailing bytes are forbidden) before the ZIP
/// library allocates central-directory metadata. This makes the 100,000-entry ceiling an actual
/// parser preflight rather than a check performed after an attacker already forced allocation.
fn preflight_entry_count(raw: &[u8]) -> Result<u64, ApiError> {
    let eocd = raw.len().checked_sub(22).ok_or_else(invalid_archive)?;
    if read_u32(raw, eocd)? != END_OF_CENTRAL_DIRECTORY_SIGNATURE || read_u16(raw, eocd + 20)? != 0 {
        return Err(invalid_archive());
    }
    let count_16 = read_u16(raw, eocd + 10)?;
    if count_16 != u16::MAX {
        return Ok(u64::from(count_16));
    }
    let locator = eocd.checked_sub(20).ok_or_else(invalid_archive)?;
    if read_u32(raw, locator)? != ZIP64_END_LOCATOR_SIGNATURE {
        return Err(invalid_archive());
    }
    let zip64_offset = usize::try_from(read_u64(raw, locator + 8)?).map_err(|_| invalid_archive())?;
    if read_u32(raw, zip64_offset)? != ZIP64_END_OF_CENTRAL_DIRECTORY_SIGNATURE {
        return Err(invalid_archive());
    }
    read_u64(raw, zip64_offset + 32)
}

fn slice(raw: &[u8], offset: usize, len: usize) -> Result<&[u8], ApiError> {
    let end = offset.checked_add(len).ok_or_else(invalid_archive)?;
    raw.get(offset..end).ok_or_else(invalid_archive)
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_hex(bytes: &[u8]) -> String {
    digest_hex(Sha256::digest(bytes).as_slice())
}

fn digest_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn invalid_archive() -> ApiError {
    ApiError::unsupported_format("archive is not a structurally valid Flow ZIP64 package")
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::needless_pass_by_value
)]
mod tests {
    use super::*;
    use crate::error::ApiErrorKind;

    const WORKSPACE_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const OBJECT_ID: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    const DOCUMENT_ID: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

    fn fixture_manifest() -> ExportPackageManifest {
        ExportPackageManifest {
            schema: PACKAGE_SCHEMA.to_string(),
            package_id: "dddddddd-dddd-4ddd-8ddd-dddddddddddd".to_string(),
            created_at: "2026-09-12T12:00:00Z".to_string(),
            producer: PackageProducer {
                product: "sylvode".to_string(),
                version: "0.2.232".to_string(),
                source_head: "0123456789abcdef0123456789abcdef01234567".to_string(),
            },
            source: PackageSource {
                workspace_id: WORKSPACE_ID.to_string(),
                scope: "object".to_string(),
                root_object_ids: vec![OBJECT_ID.to_string()],
            },
            flow_schema_version: FLOW_SCHEMA_VERSION,
            engine: PackageEngine {
                name: ENGINE_NAME.to_string(),
                crate_version: ENGINE_CRATE_VERSION.to_string(),
                wire_format_version: ENGINE_WIRE_FORMAT_VERSION,
            },
            history: PackageHistory {
                included: false,
                through_seq_by_document: BTreeMap::from([(DOCUMENT_ID.to_string(), 7)]),
            },
            counts: PackageCounts {
                objects: 1,
                documents: 1,
                ..PackageCounts::default()
            },
            members: Vec::new(),
            export_policy: ExportPolicy {
                complete: true,
                permission_snapshot_at: "2026-09-12T12:00:00Z".to_string(),
            },
        }
    }

    fn fixture_inputs() -> Vec<PackageMemberInput> {
        vec![
            PackageMemberInput {
                path: format!("documents/{DOCUMENT_ID}/snapshot.bin"),
                kind: "snapshot".to_string(),
                bytes: b"snapshot-v1".to_vec(),
            },
            PackageMemberInput {
                path: format!("objects/{OBJECT_ID}/object.json"),
                kind: "object".to_string(),
                bytes: br#"{"accepted_frontier":"AQ==","document_id":"cccccccc-cccc-4ccc-8ccc-cccccccccccc","object_id":"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb","type":"page"}"#.to_vec(),
            },
            PackageMemberInput {
                path: "relations/relations.jsonl".to_string(),
                kind: "relation".to_string(),
                bytes: Vec::new(),
            },
            PackageMemberInput {
                path: "lineage/lineage.jsonl".to_string(),
                kind: "lineage".to_string(),
                bytes: Vec::new(),
            },
        ]
    }

    fn assert_kind(error: ApiError, expected: ApiErrorKind) {
        assert_eq!(error.kind(), expected, "wrong error: {error:?}");
    }

    fn raw_zip(entries: Vec<(String, Vec<u8>)>, large_file: bool) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .large_file(large_file)
            .unix_permissions(0o600);
        for (path, bytes) in entries {
            writer.start_file(path, options).unwrap();
            writer.write_all(&bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn rewrite_member(package: &[u8], target: &str, replacement: &[u8]) -> Vec<u8> {
        let mut archive = ZipArchive::new(Cursor::new(package)).unwrap();
        let mut entries = Vec::new();
        for index in 0..archive.len() {
            let mut file = archive.by_index(index).unwrap();
            let path = file.name().to_string();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            if path == target {
                bytes = replacement.to_vec();
            }
            entries.push((path, bytes));
        }
        raw_zip(entries, true)
    }

    #[test]
    fn writer_emits_canonical_zip64_and_reader_verifies_exact_archive_and_members() {
        let built = build_package(fixture_manifest(), fixture_inputs()).expect("package builds");
        assert_eq!(built.manifest.members.len(), 4);
        assert!(
            built
                .manifest
                .members
                .windows(2)
                .all(|pair| pair[0].path.as_bytes() < pair[1].path.as_bytes())
        );
        let verified = verify_package(Cursor::new(&built.bytes), Some(&built.package_sha256))
            .expect("self-produced package verifies");
        assert_eq!(verified.package_sha256, built.package_sha256);
        assert_eq!(verified.manifest, built.manifest);

        let mut archive = ZipArchive::new(Cursor::new(&built.bytes)).unwrap();
        for index in 0..archive.len() {
            let file = archive.by_index(index).unwrap();
            let local = usize::try_from(file.header_start()).unwrap();
            let name_len = usize::from(read_u16(&built.bytes, local + 26).unwrap());
            let extra_len = usize::from(read_u16(&built.bytes, local + 28).unwrap());
            let extra = slice(&built.bytes, local + 30 + name_len, extra_len).unwrap();
            assert!(contains_extra_field(extra, ZIP64_EXTRA_ID).unwrap());
        }
    }

    #[test]
    fn package_hash_member_hash_and_checksum_file_are_independent_fail_closed_checks() {
        let built = build_package(fixture_manifest(), fixture_inputs()).unwrap();
        let wrong_archive_hash = "0".repeat(64);
        assert_kind(
            verify_package(Cursor::new(&built.bytes), Some(&wrong_archive_hash)).unwrap_err(),
            ApiErrorKind::ChecksumMismatch,
        );

        let object_path = format!("objects/{OBJECT_ID}/object.json");
        let mut replacement = fixture_inputs()
            .into_iter()
            .find(|input| input.path == object_path)
            .expect("object fixture exists")
            .bytes;
        replacement[0] ^= 1;
        let changed_member = rewrite_member(&built.bytes, &object_path, &replacement);
        assert_kind(
            verify_package(Cursor::new(changed_member), None).unwrap_err(),
            ApiErrorKind::ChecksumMismatch,
        );

        let changed_checksums = rewrite_member(&built.bytes, CHECKSUMS_PATH, b"");
        assert_kind(
            verify_package(Cursor::new(changed_checksums), None).unwrap_err(),
            ApiErrorKind::ChecksumMismatch,
        );
    }

    #[test]
    fn reader_rejects_non_zip64_unknown_traversal_and_unicode_casefold_collisions() {
        let non_zip64 = raw_zip(vec![(MANIFEST_PATH.to_string(), b"{}".to_vec())], false);
        assert_kind(
            verify_package(Cursor::new(non_zip64), None).unwrap_err(),
            ApiErrorKind::UnsupportedFormat,
        );

        for path in [
            "../manifest.json",
            "/manifest.json",
            "objects\\x\\object.json",
            "secrets/token",
        ] {
            let archive = raw_zip(vec![(path.to_string(), b"x".to_vec())], true);
            assert_kind(
                verify_package(Cursor::new(archive), None).unwrap_err(),
                ApiErrorKind::UnsupportedFormat,
            );
        }

        let collision_object = "abababab-abab-4bab-8bab-abababababab";
        let collision = raw_zip(
            vec![
                (format!("objects/{collision_object}/object.json"), Vec::new()),
                (
                    format!("objects/{}/object.json", collision_object.to_ascii_uppercase()),
                    Vec::new(),
                ),
            ],
            true,
        );
        assert_kind(
            verify_package(Cursor::new(collision), None).unwrap_err(),
            ApiErrorKind::UnsupportedFormat,
        );

        let decomposed = format!("objects/{OBJECT_ID}/objec\u{74}\u{301}.json");
        let archive = raw_zip(vec![(decomposed, b"x".to_vec())], true);
        assert_kind(
            verify_package(Cursor::new(archive), None).unwrap_err(),
            ApiErrorKind::UnsupportedFormat,
        );
    }

    #[test]
    fn local_header_descriptor_or_encryption_flag_cannot_hide_from_central_directory() {
        let built = build_package(fixture_manifest(), fixture_inputs()).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(&built.bytes)).unwrap();
        let file = archive.by_index(0).unwrap();
        let local = usize::try_from(file.header_start()).unwrap();
        let central = usize::try_from(file.central_header_start()).unwrap();
        drop(file);
        drop(archive);

        let mut unequal = built.bytes.clone();
        let local_flags = u16::from_le_bytes([unequal[local + 6], unequal[local + 7]]) | (1 << 2);
        unequal[local + 6..local + 8].copy_from_slice(&local_flags.to_le_bytes());
        assert_kind(
            verify_package(Cursor::new(unequal), None).unwrap_err(),
            ApiErrorKind::UnsupportedFormat,
        );

        for flag in [FLAG_DATA_DESCRIPTOR, FLAG_ENCRYPTED] {
            let mut mutant = built.bytes.clone();
            let local_flags = u16::from_le_bytes([mutant[local + 6], mutant[local + 7]]) | flag;
            let central_flags = u16::from_le_bytes([mutant[central + 8], mutant[central + 9]]) | flag;
            mutant[local + 6..local + 8].copy_from_slice(&local_flags.to_le_bytes());
            mutant[central + 8..central + 10].copy_from_slice(&central_flags.to_le_bytes());
            assert_kind(
                verify_package(Cursor::new(mutant), None).unwrap_err(),
                ApiErrorKind::UnsupportedFormat,
            );
        }
    }

    #[test]
    fn raw_header_validator_itself_rejects_flag_disagreement_forbidden_descriptor_and_non_zip64() {
        let built = build_package(fixture_manifest(), fixture_inputs()).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(&built.bytes)).unwrap();
        let file = archive.by_index(0).unwrap();
        let local = usize::try_from(file.header_start()).unwrap();
        let central = usize::try_from(file.central_header_start()).unwrap();
        let path = file.name().to_string();

        let mut unequal = built.bytes.clone();
        unequal[local + 6..local + 8].copy_from_slice(&(1u16 << 2).to_le_bytes());
        assert_kind(
            validate_raw_headers(&unequal, &file, &path).unwrap_err(),
            ApiErrorKind::UnsupportedFormat,
        );

        let mut descriptor = built.bytes.clone();
        descriptor[local + 6..local + 8].copy_from_slice(&FLAG_DATA_DESCRIPTOR.to_le_bytes());
        descriptor[central + 8..central + 10].copy_from_slice(&FLAG_DATA_DESCRIPTOR.to_le_bytes());
        assert_kind(
            validate_raw_headers(&descriptor, &file, &path).unwrap_err(),
            ApiErrorKind::UnsupportedFormat,
        );

        let plain = raw_zip(vec![(MANIFEST_PATH.to_string(), b"{}".to_vec())], false);
        let mut plain_archive = ZipArchive::new(Cursor::new(&plain)).unwrap();
        let plain_file = plain_archive.by_index(0).unwrap();
        assert_kind(
            validate_raw_headers(&plain, &plain_file, MANIFEST_PATH).unwrap_err(),
            ApiErrorKind::UnsupportedFormat,
        );
    }

    #[test]
    fn input_path_validator_rejects_casefold_collision_before_manifest_layout() {
        let lower = "abababab-abab-4bab-8bab-abababababab";
        let inputs = vec![
            PackageMemberInput {
                path: format!("objects/{lower}/object.json"),
                kind: "object".to_string(),
                bytes: Vec::new(),
            },
            PackageMemberInput {
                path: format!("objects/{}/object.json", lower.to_ascii_uppercase()),
                kind: "object".to_string(),
                bytes: Vec::new(),
            },
        ];
        assert_kind(
            validate_input_paths(&inputs).unwrap_err(),
            ApiErrorKind::UnsupportedFormat,
        );
    }

    #[test]
    fn writer_rejects_unknown_paths_casefold_collisions_kind_drift_and_history_leakage() {
        let mut unknown = fixture_inputs();
        unknown.push(PackageMemberInput {
            path: "unknown/state.bin".to_string(),
            kind: "snapshot".to_string(),
            bytes: vec![1],
        });
        assert_kind(
            build_package(fixture_manifest(), unknown).unwrap_err(),
            ApiErrorKind::UnsupportedFormat,
        );

        let collision_object = "abababab-abab-4bab-8bab-abababababab";
        let collision = vec![
            PackageMemberInput {
                path: format!("objects/{collision_object}/object.json"),
                kind: "object".to_string(),
                bytes: Vec::new(),
            },
            PackageMemberInput {
                path: format!("objects/{}/object.json", collision_object.to_ascii_uppercase()),
                kind: "object".to_string(),
                bytes: Vec::new(),
            },
        ];
        assert_kind(
            build_package(fixture_manifest(), collision).unwrap_err(),
            ApiErrorKind::UnsupportedFormat,
        );

        let mut wrong_kind = fixture_inputs();
        wrong_kind[0].kind = "object".to_string();
        assert_kind(
            build_package(fixture_manifest(), wrong_kind).unwrap_err(),
            ApiErrorKind::UnsupportedFormat,
        );

        let mut history = fixture_inputs();
        history.push(PackageMemberInput {
            path: format!("documents/{DOCUMENT_ID}/updates/8.bin"),
            kind: "update".to_string(),
            bytes: vec![1],
        });
        let mut manifest = fixture_manifest();
        manifest.counts.updates = 1;
        assert_kind(
            build_package(manifest, history).unwrap_err(),
            ApiErrorKind::UnsupportedFormat,
        );
    }

    #[test]
    fn noncanonical_manifest_and_unsupported_compatibility_matrix_are_rejected() {
        let built = build_package(fixture_manifest(), fixture_inputs()).unwrap();
        let pretty = serde_json::to_vec_pretty(&built.manifest).unwrap();
        let checksums = render_checksums(&pretty, &built.manifest.members);
        let mut entries = vec![
            (MANIFEST_PATH.to_string(), pretty),
            (CHECKSUMS_PATH.to_string(), checksums),
        ];
        for input in fixture_inputs() {
            entries.push((input.path, input.bytes));
        }
        let noncanonical = raw_zip(entries, true);
        assert_kind(
            verify_package(Cursor::new(noncanonical), None).unwrap_err(),
            ApiErrorKind::UnsupportedFormat,
        );

        let mut unsupported = fixture_manifest();
        unsupported.engine.wire_format_version = 2;
        assert_kind(
            build_package(unsupported, fixture_inputs()).unwrap_err(),
            ApiErrorKind::UnsupportedFormat,
        );
    }

    #[test]
    fn nfc_and_zip64_entry_count_are_checked_before_archive_parsing() {
        assert!(strict_entry_path("é".as_bytes()).is_ok());
        assert_kind(
            strict_entry_path("e\u{301}".as_bytes()).unwrap_err(),
            ApiErrorKind::UnsupportedFormat,
        );

        let zip64_offset = 0usize;
        let mut raw = vec![0u8; 56 + 20 + 22];
        raw[0..4].copy_from_slice(&ZIP64_END_OF_CENTRAL_DIRECTORY_SIGNATURE.to_le_bytes());
        raw[32..40].copy_from_slice(&100_001u64.to_le_bytes());
        let locator = 56usize;
        raw[locator..locator + 4].copy_from_slice(&ZIP64_END_LOCATOR_SIGNATURE.to_le_bytes());
        raw[locator + 8..locator + 16].copy_from_slice(&u64::try_from(zip64_offset).unwrap().to_le_bytes());
        let eocd = locator + 20;
        raw[eocd..eocd + 4].copy_from_slice(&END_OF_CENTRAL_DIRECTORY_SIGNATURE.to_le_bytes());
        raw[eocd + 10..eocd + 12].copy_from_slice(&u16::MAX.to_le_bytes());
        assert_eq!(preflight_entry_count(&raw).unwrap(), 100_001);
        assert_kind(
            verify_package(Cursor::new(raw), None).unwrap_err(),
            ApiErrorKind::LimitExceeded,
        );
    }
}
