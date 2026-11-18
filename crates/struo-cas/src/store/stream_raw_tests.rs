use super::*;

fn captured(bytes: &[u8]) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(blob::ZSTD_BLOB_MAGIC).unwrap();
    file.write_all(&u64::try_from(bytes.len()).unwrap().to_le_bytes())
        .unwrap();
    file.write_all(&zstd::stream::encode_all(bytes, 0).unwrap())
        .unwrap();
    file
}

#[test]
fn raw_deferral_requires_initial_progress_and_substantial_compression() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut writer = Writer::raw(file.reopen().unwrap(), None);
    writer.push(&vec![0; 256 * 1024]).unwrap();
    writer.consider_captured(1);
    assert!(!writer.defer_raw);
    writer.push(&vec![0; 256 * 1024]).unwrap();
    writer.consider_captured(512 * 1024);
    assert!(!writer.defer_raw);
    writer.consider_captured(256 * 1024);
    assert!(writer.defer_raw);
    writer.push(&vec![0; 512 * 1024]).unwrap();
    assert_eq!(writer.size, 1024 * 1024);
    assert_eq!(writer.raw_size, 512 * 1024);
}

#[test]
fn raw_suffix_preserves_existing_prefix_and_reports_truncated_captures() {
    let bytes = vec![42; 512 * 1024 + 13];
    let mut encoded = captured(&bytes);
    let mut raw = tempfile::NamedTempFile::new().unwrap();
    raw.write_all(&bytes[..13]).unwrap();
    complete_raw(
        raw.as_file_mut(),
        encoded.as_file_mut(),
        13,
        u64::try_from(bytes.len()).unwrap(),
    )
    .unwrap();
    assert_eq!(std::fs::read(raw.path()).unwrap(), bytes);
    let length = encoded.as_file().metadata().unwrap().len();
    encoded.as_file().set_len(length - 1).unwrap();
    let mut raw = tempfile::NamedTempFile::new().unwrap();
    assert!(complete_raw(
        raw.as_file_mut(),
        encoded.as_file_mut(),
        0,
        u64::try_from(bytes.len()).unwrap()
    )
    .is_err());
}

#[test]
fn raw_suffix_propagates_destination_write_failures() {
    let bytes = vec![42; 64 * 1024];
    let mut encoded = captured(&bytes);
    let raw = tempfile::NamedTempFile::new().unwrap();
    let mut readonly = std::fs::File::open(raw.path()).unwrap();
    assert!(complete_raw(
        &mut readonly,
        encoded.as_file_mut(),
        0,
        u64::try_from(bytes.len()).unwrap()
    )
    .is_err());
    assert_eq!(raw.as_file().metadata().unwrap().len(), 0);
}
