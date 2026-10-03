use super::*;
use std::sync::Arc;
use tempfile::TempDir;

#[test]
fn open_does_no_io() {
    let tmp = TempDir::new().unwrap();
    let nested = tmp.path().join("not/yet/created");
    let _cas = Cas::open(&nested);
    assert!(!nested.exists(), "open must not touch disk");
}

#[tokio::test]
async fn put_get_blob_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let d = cas.put_blob(b"hello").await.unwrap();
    assert_eq!(cas.get_blob(&d).await.unwrap(), b"hello");
}

#[tokio::test]
async fn put_blob_compresses_repetitive_payload_at_rest() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let payload = b"cache line\n".repeat(4096);

    let d = cas.put_blob(&payload).await.unwrap();
    let stored = fs::read(cas.blob_path(&d)).await.unwrap();

    assert!(stored.starts_with(blob::ZSTD_BLOB_MAGIC));
    assert!(stored.len() < payload.len());
    assert_eq!(cas.blob_size(&d).await.unwrap(), payload.len() as u64);
    assert_eq!(cas.get_blob(&d).await.unwrap(), payload);
}

#[tokio::test]
async fn get_blob_reads_legacy_raw_blob() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let payload = b"legacy raw blob";
    let d = Digest::of_bytes(payload);
    let path = cas.blob_path(&d);
    fs::create_dir_all(path.parent().unwrap()).await.unwrap();
    fs::write(&path, payload).await.unwrap();

    assert_eq!(cas.get_blob(&d).await.unwrap(), payload);
    assert_eq!(cas.blob_size(&d).await.unwrap(), payload.len() as u64);
}

#[tokio::test]
async fn put_blob_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let d1 = cas.put_blob(b"hello").await.unwrap();
    let d2 = cas.put_blob(b"hello").await.unwrap();
    assert_eq!(d1, d2);
}

#[tokio::test]
async fn mirrored_blob_roundtrips() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let payload = b"mirrored content";
    let digest = Digest::of_bytes(payload);

    let mirrored = cas.mirror_blob(&digest, payload).await.unwrap();

    assert_eq!(mirrored, digest);
    assert_eq!(cas.get_blob(&digest).await.unwrap(), payload);
}

#[tokio::test]
async fn mismatched_mirrored_blob_is_not_written() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let expected = Digest::of_bytes(b"expected");
    let actual = Digest::of_bytes(b"actual");

    let mirrored = cas.mirror_blob(&expected, b"actual").await.unwrap();

    assert_eq!(mirrored, actual);
    assert!(!cas.blob_path(&actual).exists());
    assert!(!cas.blobs_dir().exists());
}

#[tokio::test]
async fn concurrent_writers_of_identical_blob_do_not_race() {
    let tmp = TempDir::new().unwrap();
    let cas = Arc::new(Cas::open(tmp.path()));
    let mut handles = Vec::new();
    for _ in 0..16 {
        let cas = Arc::clone(&cas);
        handles.push(tokio::spawn(async move {
            cas.put_blob(b"shared content").await
        }));
    }
    let mut digests = Vec::new();
    for h in handles {
        digests.push(h.await.unwrap().unwrap());
    }
    assert!(digests.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(cas.get_blob(&digests[0]).await.unwrap(), b"shared content");
}

#[tokio::test]
async fn action_result_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let stdout = cas.put_blob(b"out").await.unwrap();
    let stderr = cas.put_blob(b"err").await.unwrap();
    let action = Digest::of_bytes(b"action-key");
    let result = ActionResult {
        exit_code: 0,
        stdout: Some(stdout),
        stderr: Some(stderr),
        outputs: std::collections::BTreeMap::new(),
    };
    cas.put_action_result(&action, &result).await.unwrap();
    assert_eq!(cas.get_action_result(&action).await.unwrap(), Some(result));
}

#[tokio::test]
async fn mirrored_action_result_roundtrips() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let action = Digest::of_bytes(b"mirrored-action");
    let result = ActionResult {
        exit_code: 0,
        stdout: None,
        stderr: None,
        outputs: std::collections::BTreeMap::new(),
    };

    cas.mirror_action_result(&action, &result).await.unwrap();

    assert_eq!(cas.get_action_result(&action).await.unwrap(), Some(result));
}

#[tokio::test]
async fn action_result_without_stdout_or_stderr_roundtrips() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let action = Digest::of_bytes(b"silent-action");
    let result = ActionResult {
        exit_code: 0,
        stdout: None,
        stderr: None,
        outputs: std::collections::BTreeMap::new(),
    };
    cas.put_action_result(&action, &result).await.unwrap();
    assert_eq!(cas.get_action_result(&action).await.unwrap(), Some(result));
}

#[tokio::test]
async fn missing_action_returns_none() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let d = Digest::of_bytes(b"nope");
    assert_eq!(cas.get_action_result(&d).await.unwrap(), None);
}

#[tokio::test]
async fn forget_action_removes_only_the_target() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let stdout = cas.put_blob(b"x").await.unwrap();
    let key = Digest::of_bytes(b"k");
    let result = ActionResult {
        exit_code: 0,
        stdout: Some(stdout),
        stderr: Some(stdout),
        outputs: std::collections::BTreeMap::new(),
    };
    cas.put_action_result(&key, &result).await.unwrap();
    assert!(cas.forget_action(&key).await.unwrap());
    assert_eq!(cas.get_action_result(&key).await.unwrap(), None);
    // Blob is untouched - multiple actions may share a stdout blob.
    assert_eq!(cas.get_blob(&stdout).await.unwrap(), b"x");
    // Forgetting again is a no-op.
    assert!(!cas.forget_action(&key).await.unwrap());
}

#[tokio::test]
async fn put_stream_roundtrip_small() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let payload = b"streamed payload";
    let d = cas.put_stream(&payload[..]).await.unwrap();
    assert_eq!(d, Digest::of_bytes(payload));
    assert_eq!(cas.get_blob(&d).await.unwrap(), payload);
}

#[tokio::test]
async fn put_stream_handles_empty_input() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let d = cas.put_stream(&b""[..]).await.unwrap();
    assert_eq!(d, Digest::of_bytes(b""));
    assert!(cas.get_blob(&d).await.unwrap().is_empty());
}

#[tokio::test]
async fn put_stream_handles_payload_larger_than_chunk() {
    // STREAM_CHUNK = 64 KiB; use a payload that crosses the
    // boundary several times to exercise the loop.
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let payload: Vec<u8> = (0..(STREAM_CHUNK * 3 + 7))
        .map(|i| u8::try_from(i & 0xff).unwrap())
        .collect();
    let d = cas.put_stream(&payload[..]).await.unwrap();
    assert_eq!(d, Digest::of_bytes(&payload));
    assert_eq!(cas.get_blob(&d).await.unwrap(), payload);
}

#[tokio::test]
async fn put_stream_compresses_repetitive_payload_at_rest() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let payload = vec![b'x'; STREAM_CHUNK * 3];

    let d = cas.put_stream(payload.as_slice()).await.unwrap();
    let stored = fs::read(cas.blob_path(&d)).await.unwrap();

    assert_eq!(d, Digest::of_bytes(&payload));
    assert!(stored.starts_with(blob::ZSTD_BLOB_MAGIC));
    assert!(stored.len() < payload.len());
    assert_eq!(cas.get_blob(&d).await.unwrap(), payload);
}

#[tokio::test]
async fn copy_blob_to_file_streams_raw_and_compressed_storage() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path().join("cas"));
    for (name, payload) in [
        ("raw", b"small payload".to_vec()),
        ("compressed", vec![b'x'; STREAM_CHUNK * 4]),
    ] {
        let digest = cas.put_stream(payload.as_slice()).await.unwrap();
        let destination = tmp.path().join(name);

        cas.copy_blob_to_file(&digest, &destination).await.unwrap();

        assert_eq!(fs::read(destination).await.unwrap(), payload);
    }
}

#[tokio::test]
async fn copy_blob_to_file_preserves_destination_when_decode_fails() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path().join("cas"));
    let payload = vec![b'x'; STREAM_CHUNK * 4];
    let digest = cas.put_blob(&payload).await.unwrap();
    let stored_path = cas.blob_path(&digest);
    let mut stored = fs::read(&stored_path).await.unwrap();
    assert!(stored.starts_with(blob::ZSTD_BLOB_MAGIC));
    stored.truncate(blob::ZSTD_BLOB_HEADER_LEN + 1);
    fs::write(&stored_path, stored).await.unwrap();
    let destination = tmp.path().join("existing-output");
    fs::write(&destination, b"keep this output").await.unwrap();

    assert!(cas.copy_blob_to_file(&digest, &destination).await.is_err());
    assert_eq!(fs::read(&destination).await.unwrap(), b"keep this output");
}

#[tokio::test]
async fn put_stream_cleans_up_scratch_on_read_error() {
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::ReadBuf;

    struct ExplodingReader;
    impl AsyncRead for ExplodingReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::other("synthetic read failure")))
        }
    }

    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let err = cas.put_stream(ExplodingReader).await.unwrap_err();
    assert!(matches!(err, Error::Io { .. }));

    // The scratch directory must contain no `stream-*` leftovers.
    let scratch = cas.scratch_dir();
    if scratch.exists() {
        let mut entries = fs::read_dir(&scratch).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            assert!(
                !name.starts_with("stream-"),
                "leftover scratch file: {name}"
            );
        }
    }
}

#[tokio::test]
async fn put_stream_cleans_up_raw_and_compressed_scratch_on_install_error() {
    for payload in [b"raw".to_vec(), vec![b'x'; STREAM_CHUNK * 3]] {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());
        let digest = Digest::of_bytes(&payload);
        let destination = cas.blob_path(&digest);
        fs::create_dir_all(cas.blobs_dir()).await.unwrap();
        fs::write(destination.parent().unwrap(), b"not a shard directory")
            .await
            .unwrap();

        assert!(cas.put_stream(payload.as_slice()).await.is_err());
        assert_eq!(std::fs::read_dir(cas.scratch_dir()).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn put_stream_dedups_identical_content() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let payload = vec![42u8; STREAM_CHUNK + 100];
    let d1 = cas.put_stream(&payload[..]).await.unwrap();
    let d2 = cas.put_stream(&payload[..]).await.unwrap();
    assert_eq!(d1, d2);
    // Only one blob entry exists despite two puts.
    assert_eq!(cas.stats().await.unwrap().blob_count, 1);
}

#[tokio::test]
async fn concurrent_put_stream_of_identical_content_is_safe() {
    let tmp = TempDir::new().unwrap();
    let cas = Arc::new(Cas::open(tmp.path()));
    let payload: Vec<u8> = (0..(STREAM_CHUNK * 2))
        .map(|i| u8::try_from(i & 0xff).unwrap())
        .collect();
    let payload = Arc::new(payload);
    let mut handles = Vec::new();
    for _ in 0..8 {
        let cas = Arc::clone(&cas);
        let payload = Arc::clone(&payload);
        handles.push(tokio::spawn(async move {
            cas.put_stream(payload.as_slice()).await
        }));
    }
    let mut digests = Vec::new();
    for h in handles {
        digests.push(h.await.unwrap().unwrap());
    }
    assert!(digests.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(cas.stats().await.unwrap().blob_count, 1);
}

#[tokio::test]
async fn concurrent_writers_of_distinct_blobs_all_persist() {
    // Different content goes to different shards; nothing races on
    // the same final path.
    let tmp = TempDir::new().unwrap();
    let cas = Arc::new(Cas::open(tmp.path()));
    let mut handles = Vec::new();
    for i in 0..32u32 {
        let cas = Arc::clone(&cas);
        handles.push(tokio::spawn(async move {
            cas.put_blob(format!("blob-{i}").as_bytes()).await
        }));
    }
    let mut digests = Vec::new();
    for h in handles {
        digests.push(h.await.unwrap().unwrap());
    }
    // All 32 distinct, all readable.
    let unique: std::collections::HashSet<_> = digests.iter().collect();
    assert_eq!(unique.len(), 32);
    for (i, d) in digests.iter().enumerate() {
        assert_eq!(
            cas.get_blob(d).await.unwrap(),
            format!("blob-{i}").as_bytes()
        );
    }
}

#[tokio::test]
async fn has_blob_is_false_for_unknown_digest() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let d = Digest::of_bytes(b"never-stored");
    assert!(!cas.has_blob(&d).await.unwrap());
}

#[tokio::test]
async fn has_blob_finds_content_addressed_blob() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let d = cas.put_blob(b"hello").await.unwrap();
    assert!(cas.has_blob(&d).await.unwrap());
}

#[tokio::test]
async fn get_blob_returns_blob_not_found_for_unknown_digest() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let d = Digest::of_bytes(b"never-stored");
    match cas.get_blob(&d).await {
        Err(Error::BlobNotFound(missing)) => assert_eq!(missing, d),
        other => panic!("expected BlobNotFound, got {other:?}"),
    }
}

#[tokio::test]
async fn get_action_result_surfaces_corruption() {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    // Plant a corrupted action-result file at a known digest path.
    let key = Digest::of_bytes(b"corrupt");
    let stdout = cas.put_blob(b"x").await.unwrap();
    cas.put_action_result(
        &key,
        &ActionResult {
            exit_code: 0,
            stdout: Some(stdout),
            stderr: Some(stdout),
            outputs: std::collections::BTreeMap::new(),
        },
    )
    .await
    .unwrap();
    // Overwrite with non-JSON.
    let action_path = cas.action_path(&key);
    fs::write(&action_path, b"not json").await.unwrap();
    match cas.get_action_result(&key).await {
        Err(Error::Corrupt(path, _)) => assert_eq!(path, action_path),
        other => panic!("expected Corrupt, got {other:?}"),
    }
}
