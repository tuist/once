use std::io::Write as _;
use std::path::Path;

use super::{preflight, CentralDirectory};
use crate::{Error, RemoteIoLimits};

fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, contents) in entries {
        archive.start_file(*name, options).unwrap();
        archive.write_all(contents).unwrap();
    }
    archive.finish().unwrap().into_inner()
}

fn check(bytes: &[u8], limits: RemoteIoLimits) -> crate::Result<CentralDirectory> {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    preflight(&mut std::fs::File::open(file.path()).unwrap(), limits)
}

fn eocd_offset(bytes: &[u8]) -> usize {
    bytes
        .windows(4)
        .rposition(|window| window == [0x50, 0x4b, 0x05, 0x06])
        .unwrap()
}

fn is_limit(result: &crate::Result<CentralDirectory>, expected: &str) -> bool {
    matches!(result, Err(Error::ExecutorResourceLimit { resource, .. }) if *resource == expected)
}

fn is_refused(result: &crate::Result<CentralDirectory>, expected: &str) -> bool {
    matches!(result, Err(Error::InvalidDownloadAndExtract { reason }) if reason.contains(expected))
}

#[test]
fn an_ordinary_archive_passes_with_its_entry_count() {
    let bytes = zip_bytes(&[("a", b"1"), ("b/c", b"2")]);
    let directory = check(&bytes, RemoteIoLimits::default()).unwrap();
    assert_eq!(directory.entries, 2);
}

#[test]
fn metadata_heavy_archives_are_refused_before_decoding() {
    let name = "n".repeat(2000);
    // About 140 KiB of central directory: over the 64 KiB metadata limit
    // but under the decoder's read budget, so only the preflight refuses it.
    let names = (0..70)
        .map(|index| format!("{name}{index}"))
        .collect::<Vec<_>>();
    let entries = names
        .iter()
        .map(|name| (name.as_str(), b"" as &[u8]))
        .collect::<Vec<_>>();
    let bytes = zip_bytes(&entries);
    let limits = RemoteIoLimits {
        max_metadata_bytes: 64 * 1024,
        ..RemoteIoLimits::default()
    };
    // The decoder itself would open it, allocating every name.
    assert!(zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).is_ok());
    assert!(is_limit(&check(&bytes, limits), "archive metadata bytes"));
    assert!(bytes.len() as u64 > limits.max_metadata_bytes);
    assert!((bytes.len() as u64) < super::decoder_read_budget(limits));

    let workspace = tempfile::tempdir().unwrap();
    let archive = workspace.path().join("heavy.zip");
    std::fs::write(&archive, &bytes).unwrap();
    let extracted = super::super::extract_zip_archive(
        &archive,
        &workspace.path().join("output"),
        limits,
        limits.max_archive_bytes,
    );
    assert!(matches!(
        extracted,
        Err(Error::ExecutorResourceLimit {
            resource: "archive metadata bytes",
            ..
        })
    ));
    assert!(!Path::new(&workspace.path().join("output/n")).exists());
}

#[test]
fn declared_entries_beyond_the_limit_are_refused() {
    let bytes = zip_bytes(&[("a", b""), ("b", b""), ("c", b"")]);
    let limits = RemoteIoLimits {
        max_archive_entries: 2,
        ..RemoteIoLimits::default()
    };
    assert!(is_limit(&check(&bytes, limits), "archive entries"));
}

#[test]
fn multi_disk_archives_are_refused() {
    let mut bytes = zip_bytes(&[("a", b"1")]);
    let eocd = eocd_offset(&bytes);
    bytes[eocd + 4] = 1;
    assert!(is_refused(
        &check(&bytes, RemoteIoLimits::default()),
        "multi-disk"
    ));
}

#[test]
fn trailing_bytes_and_missing_records_are_refused() {
    let mut bytes = zip_bytes(&[("a", b"1")]);
    bytes.extend_from_slice(b"trailing");
    assert!(is_refused(
        &check(&bytes, RemoteIoLimits::default()),
        "end of central directory record is missing"
    ));
    assert!(is_refused(
        &check(b"not a zip", RemoteIoLimits::default()),
        "end of central directory record is missing"
    ));
}

#[test]
fn a_central_directory_that_overruns_its_records_is_refused() {
    let mut bytes = zip_bytes(&[("a", b"1")]);
    let eocd = eocd_offset(&bytes);
    let size = u32::from_le_bytes(bytes[eocd + 12..eocd + 16].try_into().unwrap());
    bytes[eocd + 12..eocd + 16].copy_from_slice(&(size + 1).to_le_bytes());
    assert!(is_refused(
        &check(&bytes, RemoteIoLimits::default()),
        "does not end where"
    ));
}

#[test]
fn a_fallback_record_declaring_huge_counts_is_refused() {
    // A stored entry carrying a ZIP64 end-of-central-directory record the
    // decoder could retry with if the real one failed to parse.
    let mut fake = vec![0x50, 0x4b, 0x06, 0x06];
    fake.extend_from_slice(&44_u64.to_le_bytes());
    fake.extend_from_slice(&[45, 0, 45, 0]);
    fake.extend_from_slice(&[0; 8]);
    fake.extend_from_slice(&(u64::MAX / 2).to_le_bytes());
    fake.extend_from_slice(&(u64::MAX / 2).to_le_bytes());
    fake.extend_from_slice(&[0; 16]);
    let bytes = zip_bytes(&[("payload", &fake)]);
    assert!(is_limit(
        &check(&bytes, RemoteIoLimits::default()),
        "archive metadata bytes"
    ));
}

/// Rewrite a small archive's trailer in ZIP64 form.
fn as_zip64(bytes: &[u8]) -> Vec<u8> {
    let eocd = eocd_offset(bytes);
    let entries = u64::from(u16::from_le_bytes([bytes[eocd + 10], bytes[eocd + 11]]));
    let size = u64::from(u32::from_le_bytes(
        bytes[eocd + 12..eocd + 16].try_into().unwrap(),
    ));
    let offset = u64::from(u32::from_le_bytes(
        bytes[eocd + 16..eocd + 20].try_into().unwrap(),
    ));
    let mut out = bytes[..eocd].to_vec();
    let record = out.len() as u64;
    out.extend_from_slice(&[0x50, 0x4b, 0x06, 0x06]);
    out.extend_from_slice(&44_u64.to_le_bytes());
    out.extend_from_slice(&[45, 0, 45, 0]);
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&entries.to_le_bytes());
    out.extend_from_slice(&entries.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(&[0x50, 0x4b, 0x06, 0x07]);
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&record.to_le_bytes());
    out.extend_from_slice(&1_u32.to_le_bytes());
    out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
    out.extend_from_slice(&[0xff; 4]);
    out.extend_from_slice(&[0xff; 8]);
    out.extend_from_slice(&[0, 0]);
    out
}

#[test]
fn zip64_archives_pass_and_still_decode() {
    let bytes = as_zip64(&zip_bytes(&[("a", b"1"), ("b", b"2")]));
    let directory = check(&bytes, RemoteIoLimits::default()).unwrap();
    assert_eq!(directory.entries, 2);
    assert_eq!(
        zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn a_multi_disk_zip64_locator_is_refused() {
    let mut bytes = as_zip64(&zip_bytes(&[("a", b"1")]));
    let locator = bytes
        .windows(4)
        .rposition(|window| window == [0x50, 0x4b, 0x06, 0x07])
        .unwrap();
    bytes[locator + 16] = 2;
    assert!(is_refused(
        &check(&bytes, RemoteIoLimits::default()),
        "multi-disk"
    ));
}

#[test]
fn the_decoder_read_budget_stops_reads_until_lifted() {
    use std::io::Read as _;

    let (mut reader, budget) = super::BudgetedReader::new(std::io::Cursor::new(vec![7_u8; 10]), 4);
    let mut buffer = [0_u8; 10];
    assert_eq!(reader.read(&mut buffer).unwrap(), 4);
    assert!(reader.read(&mut buffer).is_err());
    assert!(budget.exhausted());
    budget.lift();
    assert_eq!(reader.read(&mut buffer).unwrap(), 6);
}
