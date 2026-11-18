use super::*;
use crate::blob;

#[tokio::test]
async fn deferred_raw_suffix_is_reconstructed_when_late_padding_requires_raw_storage() {
    let mut bytes = vec![0; STREAM_CHUNK * 4];
    let mut body = zstd::stream::encode_all(bytes.as_slice(), 0).unwrap();
    let padding = vec![0; STREAM_CHUNK * 8];
    body.extend_from_slice(&[0x50, 0x2a, 0x4d, 0x18]);
    body.extend_from_slice(&u32::try_from(padding.len()).unwrap().to_le_bytes());
    body.extend_from_slice(&padding);
    let suffix = vec![42; 32 * 1024];
    body.extend(zstd::stream::encode_all(suffix.as_slice(), 0).unwrap());
    bytes.extend(suffix);
    let temp = tempfile::tempdir().unwrap();
    let (path, digest) = ingest_remote(
        temp.path(),
        body.as_slice(),
        u64::try_from(bytes.len()).unwrap(),
        true,
        (),
    )
    .await
    .unwrap();
    assert_eq!(digest, Digest::of_bytes(&bytes));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    drop(path);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn decoded_batches_preserve_empty_exact_and_partial_boundaries() {
    for size in [
        0,
        1,
        STREAM_CHUNK * 2 - 1,
        STREAM_CHUNK * 2,
        STREAM_CHUNK * 2 + 1,
        STREAM_CHUNK * 4 + 3,
    ] {
        let bytes = vec![42; size];
        let body = zstd::stream::encode_all(bytes.as_slice(), 0).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let (path, digest) = ingest_remote(
            temp.path(),
            body.as_slice(),
            u64::try_from(size).unwrap(),
            true,
            (),
        )
        .await
        .unwrap();
        assert_eq!(digest, Digest::of_bytes(&bytes));
        assert_eq!(
            blob::decode_bytes(std::fs::read(&path).unwrap()).unwrap(),
            bytes
        );
        drop(path);
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn final_partial_batch_rejects_declared_size_mismatches_without_publication() {
    let bytes = vec![42; STREAM_CHUNK * 2 + 1];
    let body = zstd::stream::encode_all(bytes.as_slice(), 0).unwrap();
    let size = u64::try_from(bytes.len()).unwrap();
    for expected in [size - 1, size + 1] {
        let temp = tempfile::tempdir().unwrap();
        let error = ingest_remote(temp.path(), body.as_slice(), expected, true, ())
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Io { .. }));
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}
