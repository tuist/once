use super::*;

struct FailingWriter;
impl Write for FailingWriter {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("write failed"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn buffered_entry_payload_matches_original_encoding_and_digest_at_boundaries() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("payload");
    for len in [
        0,
        1,
        8191,
        8192,
        CAPTURE_BUFFER - 1,
        CAPTURE_BUFFER,
        CAPTURE_BUFFER + 1,
        CAPTURE_BUFFER * 2 + 17,
    ] {
        let bytes: Vec<u8> = (0..len).map(|index| index.to_le_bytes()[0]).collect();
        std::fs::write(&path, &bytes).unwrap();
        let entry = DirectoryEntryContent::File {
            path: path.clone(),
            len: u64::try_from(len).unwrap(),
        };
        let mut output = Vec::new();
        entry.write_to(&mut output).unwrap();
        assert_eq!(output, bytes);
        let mut original = Vec::new();
        std::io::copy(
            &mut std::fs::File::open(&path)
                .unwrap()
                .take(u64::try_from(len).unwrap()),
            &mut original,
        )
        .unwrap();
        assert_eq!(Digest::of_bytes(&output), Digest::of_bytes(&original));
    }
}

#[test]
fn growing_entry_is_limited_to_its_recorded_length() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("payload");
    std::fs::write(&path, vec![0x42; CAPTURE_BUFFER + 17]).unwrap();
    let entry = DirectoryEntryContent::File {
        path,
        len: u64::try_from(CAPTURE_BUFFER - 1).unwrap(),
    };
    let mut output = Vec::new();
    entry.write_to(&mut output).unwrap();
    assert_eq!(output, vec![0x42; CAPTURE_BUFFER - 1]);
}

#[test]
fn truncated_entry_and_writer_errors_are_not_suppressed() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("payload");
    std::fs::write(&path, vec![0x42; CAPTURE_BUFFER - 1]).unwrap();
    let entry = DirectoryEntryContent::File {
        path: path.clone(),
        len: u64::try_from(CAPTURE_BUFFER).unwrap(),
    };
    assert_eq!(
        entry.write_to(&mut Vec::new()).unwrap_err().kind(),
        std::io::ErrorKind::UnexpectedEof
    );
    let entry = DirectoryEntryContent::File {
        path,
        len: u64::try_from(CAPTURE_BUFFER - 1).unwrap(),
    };
    assert_eq!(
        entry.write_to(&mut FailingWriter).unwrap_err().kind(),
        std::io::ErrorKind::Other
    );
}
