use super::*;
use crate::blob::ZSTD_BLOB_MAGIC;
use std::io;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn produced_raw_compressed_magic_and_empty_blobs_match_streamed_identity() {
    let temp = tempfile::tempdir().unwrap();
    let cas = Cas::open(temp.path());
    let mut random = vec![0; 1024 * 1024 + 17];
    let mut state = 0x1234_5678_u64;
    for byte in &mut random {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state.to_le_bytes()[0];
    }
    let mut magic = ZSTD_BLOB_MAGIC.to_vec();
    magic.extend_from_slice(&random);
    for bytes in [vec![], vec![b'x'; 1024 * 1024 + 1], random, magic] {
        let expected = Digest::of_bytes(&bytes);
        let produced = bytes.clone();
        let digest = cas
            .put_produced((), move |writer| {
                for chunk in produced.chunks(8191) {
                    writer.write_all(chunk)?;
                }
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(digest, expected);
        assert_eq!(cas.get_blob(&digest).await.unwrap(), bytes);
        assert_eq!(cas.put_stream(bytes.as_slice()).await.unwrap(), digest);
        assert_eq!(std::fs::read_dir(cas.scratch_dir()).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn pipelined_producers_hash_large_writes_and_reject_partial_failures() {
    let temp = tempfile::tempdir().unwrap();
    let cas = Cas::open(temp.path());
    let resources = Arc::new(tokio::sync::Semaphore::new(2));
    let mut magic = ZSTD_BLOB_MAGIC.to_vec();
    magic.extend(vec![31; 3 * 512 * 1024 + 9]);
    for bytes in [vec![], vec![17; 3 * 512 * 1024 + 7], magic] {
        let expected = Digest::of_bytes(&bytes);
        let produced = bytes.clone();
        let guard = Arc::clone(&resources).acquire_many_owned(2).await.unwrap();
        let digest = cas
            .put_produced_pipelined(guard, move |writer| writer.write_all(&produced))
            .await
            .unwrap();
        assert_eq!(digest, expected);
        assert_eq!(cas.get_blob(&digest).await.unwrap(), bytes);
    }
    let bytes = vec![13; 2 * 512 * 1024 + 1];
    let digest = Digest::of_bytes(&bytes);
    let guard = Arc::clone(&resources).acquire_many_owned(2).await.unwrap();
    cas.put_produced_pipelined(guard, move |writer| {
        writer.write_all(&bytes)?;
        Err(io::Error::other("producer stopped"))
    })
    .await
    .unwrap_err();
    assert!(!cas.has_blob(&digest).await.unwrap());
    assert_eq!(resources.available_permits(), 2);
    assert_eq!(std::fs::read_dir(cas.scratch_dir()).unwrap().count(), 0);
}

struct DropGate {
    _permit: tokio::sync::OwnedSemaphorePermit,
    entered: Option<tokio::sync::oneshot::Sender<()>>,
    release: std::sync::mpsc::Receiver<()>,
}

impl Drop for DropGate {
    fn drop(&mut self) {
        let _ = self.entered.take().unwrap().send(());
        let _ = self.release.recv_timeout(Duration::from_secs(5));
    }
}

#[tokio::test]
async fn successful_publication_retains_guard_until_worker_drop_finishes() {
    let temp = tempfile::tempdir().unwrap();
    let cas = Cas::open(temp.path());
    let resources = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = Arc::clone(&resources).acquire_owned().await.unwrap();
    let (entered, dropping) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let guard = DropGate {
        _permit: permit,
        entered: Some(entered),
        release: wait,
    };
    let cache = cas.clone();
    let task = tokio::spawn(async move {
        cache
            .put_produced(guard, |writer| writer.write_all(b"durable payload"))
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), dropping)
        .await
        .unwrap()
        .unwrap();
    let digest = Digest::of_bytes(b"durable payload");
    assert_eq!(cas.get_blob(&digest).await.unwrap(), b"durable payload");
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(resources.try_acquire().is_err());
    assert_eq!(std::fs::read_dir(cas.scratch_dir()).unwrap().count(), 0);
    release.send(()).unwrap();
    let permit = tokio::time::timeout(Duration::from_secs(5), resources.acquire())
        .await
        .unwrap()
        .unwrap();
    drop(permit);
}

#[tokio::test]
async fn producer_failure_never_publishes_partial_content() {
    let temp = tempfile::tempdir().unwrap();
    let cas = Cas::open(temp.path());
    let payload = vec![7; 512 * 1024];
    let digest = Digest::of_bytes(&payload);
    let error = cas
        .put_produced((), move |writer| {
            writer.write_all(&payload)?;
            Err(io::Error::other("producer failed after writing"))
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("producer failed after writing"));
    assert!(!cas.has_blob(&digest).await.unwrap());
    assert_eq!(std::fs::read_dir(cas.scratch_dir()).unwrap().count(), 0);
}

#[tokio::test]
async fn publication_failure_cleans_scratch_without_replacing_a_blocking_shard_file() {
    let temp = tempfile::tempdir().unwrap();
    let cas = Cas::open(temp.path());
    let bytes = b"complete but not published";
    let digest = Digest::of_bytes(bytes);
    let destination = cas.blob_path(&digest);
    let shard = destination.parent().unwrap();
    std::fs::create_dir_all(shard.parent().unwrap()).unwrap();
    std::fs::write(shard, b"existing file").unwrap();
    let error = cas
        .put_produced((), move |writer| writer.write_all(bytes))
        .await
        .unwrap_err();
    assert!(matches!(error, crate::Error::Io { .. }));
    assert_eq!(std::fs::read(shard).unwrap(), b"existing file");
    assert!(destination.metadata().is_err());
    assert_eq!(std::fs::read_dir(cas.scratch_dir()).unwrap().count(), 0);
}

#[tokio::test]
async fn cancelling_after_actual_writes_retains_worker_guard_and_cleans_scratch() {
    let temp = tempfile::tempdir().unwrap();
    let cas = Cas::open(temp.path());
    let resources = Arc::new(tokio::sync::Semaphore::new(1));
    let guard = Arc::clone(&resources).acquire_owned().await.unwrap();
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let cache = cas.clone();
    let task = tokio::spawn(async move {
        cache
            .put_produced(guard, move |writer| {
                writer.write_all(&vec![9; 512 * 1024])?;
                started.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
                writer.write_all(b"must fail after cancellation")
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    assert!(std::fs::read_dir(cas.scratch_dir()).unwrap().any(|entry| {
        std::fs::File::open(entry.unwrap().path())
            .unwrap()
            .metadata()
            .unwrap()
            .len()
            >= 512 * 1024
    }));
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(resources.try_acquire().is_err());
    assert_eq!(std::fs::read_dir(cas.scratch_dir()).unwrap().count(), 0);
    release.send(()).unwrap();
    let permit = tokio::time::timeout(Duration::from_secs(5), resources.acquire())
        .await
        .unwrap()
        .unwrap();
    drop(permit);
    assert_eq!(cas.stats().await.unwrap().blob_count, 0);
}
