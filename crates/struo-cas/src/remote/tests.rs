use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use object_store::memory::InMemory;
use object_store::path::Path as ObjectPath;
use object_store::ObjectStoreExt;
use tempfile::TempDir;

use super::*;
use crate::CacheProvider;

#[derive(Debug, Default)]
struct Recorded {
    puts: Vec<String>,
    fail_reads: Option<ErrorKind>,
}

/// An in-memory bucket that records writes and can refuse reads.
#[derive(Debug)]
struct Bucket {
    inner: ObjectStoreRemote,
    state: Mutex<Recorded>,
}

impl Bucket {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: ObjectStoreRemote::new(
                Arc::new(InMemory::new()),
                ObjectPath::from("p"),
                "memory",
            ),
            state: Mutex::default(),
        })
    }

    fn puts(&self) -> Vec<String> {
        self.state.lock().unwrap().puts.clone()
    }

    fn check_reads(&self) -> Result<()> {
        match self.state.lock().unwrap().fail_reads {
            Some(kind) => Err(Error::remote("memory", "get", kind, "injected")),
            None => Ok(()),
        }
    }
}

#[async_trait::async_trait]
impl RemoteStore for Bucket {
    fn provider(&self) -> &'static str {
        "memory"
    }
    async fn head(&self, key: &str) -> Result<Option<u64>> {
        self.check_reads()?;
        self.inner.head(key).await
    }
    async fn get(&self, key: &str) -> Result<Option<RemoteBody>> {
        self.check_reads()?;
        self.inner.get(key).await
    }
    async fn put_bytes(&self, key: &str, bytes: Bytes) -> Result<()> {
        self.state.lock().unwrap().puts.push(key.to_string());
        self.inner.put_bytes(key, bytes).await
    }
    async fn put_file(&self, key: &str, path: &Path, size: u64) -> Result<()> {
        self.state.lock().unwrap().puts.push(key.to_string());
        self.inner.put_file(key, path, size).await
    }
    fn upload_memory(&self, size: u64) -> u64 {
        self.inner.upload_memory(size)
    }
}

fn client(root: &TempDir, bucket: &Arc<Bucket>, access: RemoteAccess) -> CacheProvider {
    CacheProvider::remote(
        root.path(),
        Arc::clone(bucket) as Arc<dyn RemoteStore>,
        access,
    )
}

fn result_with(stdout: Digest, output: Digest) -> ActionResult {
    ActionResult {
        exit_code: 3,
        stdout: Some(stdout),
        stderr: None,
        outputs: BTreeMap::from([("out/bin".to_string(), output)]),
    }
}

#[tokio::test]
async fn published_records_remain_readable_at_the_size_limit() {
    // One output with a 64-character digest, without escaped path characters.
    let overhead = r#"{"exit_code":0,"outputs":{"":""}}"#.len() + 64;
    for size in [
        MAX_ACTION_RECORD_BYTES - 1,
        MAX_ACTION_RECORD_BYTES,
        MAX_ACTION_RECORD_BYTES + 1,
    ] {
        let bucket = Bucket::new();
        let (first_root, second_root) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let writer = client(&first_root, &bucket, RemoteAccess::ReadWrite);
        let reader = client(&second_root, &bucket, RemoteAccess::ReadWrite);
        let result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::from([(
                "x".repeat(usize::try_from(size).unwrap() - overhead),
                Digest::of_bytes(&[]),
            )]),
        };
        let action = Digest::of_bytes(&size.to_le_bytes());
        writer.put_action_result(&action, &result).await.unwrap();
        assert!(writer
            .lookup_action_result(&action)
            .await
            .unwrap()
            .is_some());
        let hit = reader.lookup_action_result(&action).await.unwrap();
        if size <= MAX_ACTION_RECORD_BYTES {
            assert_eq!(bucket.head(&action_key(&action)).await.unwrap(), Some(size));
            let hit = hit.expect("published record must be readable by a fresh client");
            assert!(hit.blobs_available);
            assert_eq!(hit.result, result);
        } else {
            assert!(
                hit.is_none(),
                "oversized record must stay local, not poison remote reads"
            );
            assert!(bucket.puts().is_empty());
        }
    }
}

#[tokio::test]
async fn a_second_client_reuses_published_results_and_content() {
    let bucket = Bucket::new();
    let (first_root, second_root) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let first = client(&first_root, &bucket, RemoteAccess::ReadWrite);
    // Larger than one multipart part, so the streamed upload path runs.
    let large: Vec<u8> = (0..9 * 1024 * 1024 + 17_u32)
        .map(|i| (i % 251) as u8)
        .collect();
    let output = first.put_stream(&large[..]).await.unwrap();
    let stdout = first.put_blob(b"log line").await.unwrap();
    let action = Digest::of_bytes(b"action");
    first
        .put_action_result(&action, &result_with(stdout, output))
        .await
        .unwrap();

    let puts = bucket.puts();
    assert_eq!(
        puts.last(),
        Some(&action_key(&action)),
        "record must be last: {puts:?}"
    );
    assert!(puts.contains(&blob_key(&output)), "{puts:?}");

    let second = client(&second_root, &bucket, RemoteAccess::ReadWrite);
    let hit = second.lookup_action_result(&action).await.unwrap().unwrap();
    assert!(hit.blobs_available);
    assert_eq!(hit.result, result_with(stdout, output));
    let restored = second_root.path().join("restored");
    second.copy_blob_to_file(&output, &restored).await.unwrap();
    assert_eq!(std::fs::read(&restored).unwrap(), large);
    assert_eq!(second.get_blob(&stdout).await.unwrap(), b"log line");
    assert!(second.has_local_blob(&output).await.unwrap());
}

#[tokio::test]
async fn evicted_content_makes_a_remote_result_a_miss() {
    let bucket = Bucket::new();
    let (first_root, second_root) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let first = client(&first_root, &bucket, RemoteAccess::ReadWrite);
    let stdout = first.put_blob(b"stdout").await.unwrap();
    let output = first.put_blob(b"output").await.unwrap();
    let action = Digest::of_bytes(b"action");
    first
        .put_action_result(&action, &result_with(stdout, output))
        .await
        .unwrap();
    bucket
        .inner
        .store
        .delete(&bucket.inner.location(&blob_key(&output)))
        .await
        .unwrap();

    let second = client(&second_root, &bucket, RemoteAccess::ReadWrite);
    assert!(second
        .lookup_action_result(&action)
        .await
        .unwrap()
        .is_none());
    let error = second
        .copy_blob_to_file(&output, &second_root.path().join("x"))
        .await
        .unwrap_err();
    assert!(error.is_not_found(), "{error}");
    assert!(!second.has_blob(&output).await.unwrap());
}

#[tokio::test]
async fn content_that_does_not_match_its_key_is_rejected() {
    let bucket = Bucket::new();
    let root = TempDir::new().unwrap();
    let claimed = Digest::of_bytes(b"expected bytes");
    for body in [&b"tampered bytes"[..], &b"expected byte"[..]] {
        bucket
            .inner
            .put_bytes(&blob_key(&claimed), Bytes::copy_from_slice(body))
            .await
            .unwrap();
        let provider = client(&root, &bucket, RemoteAccess::ReadWrite);
        let error = provider.get_blob(&claimed).await.unwrap_err();
        assert!(error.is_corrupt(), "{error}");
        assert!(!provider.has_local_blob(&claimed).await.unwrap());
    }
}

#[tokio::test]
async fn refused_reads_are_errors_not_misses() {
    let bucket = Bucket::new();
    let root = TempDir::new().unwrap();
    let provider = client(&root, &bucket, RemoteAccess::ReadWrite);
    bucket.state.lock().unwrap().fail_reads = Some(ErrorKind::Refused);
    let error = provider
        .lookup_action_result(&Digest::of_bytes(b"action"))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Refused);
    let error = provider
        .has_blob(&Digest::of_bytes(b"blob"))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Refused);
}

#[tokio::test]
async fn read_only_clients_never_publish() {
    let bucket = Bucket::new();
    let root = TempDir::new().unwrap();
    let provider = client(&root, &bucket, RemoteAccess::ReadOnly);
    let stdout = provider.put_blob(b"stdout").await.unwrap();
    let output = provider.put_stream(&b"output"[..]).await.unwrap();
    let action = Digest::of_bytes(b"action");
    provider
        .put_action_result(&action, &result_with(stdout, output))
        .await
        .unwrap();
    assert!(bucket.puts().is_empty());
    assert!(provider
        .lookup_action_result(&action)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn a_record_is_not_published_when_its_content_upload_fails() {
    let bucket = Bucket::new();
    let root = TempDir::new().unwrap();
    let provider = client(&root, &bucket, RemoteAccess::ReadWrite);
    let output = provider.put_blob(b"output").await.unwrap();
    let missing = Digest::of_bytes(b"never stored locally");
    let action = Digest::of_bytes(b"action");
    provider
        .put_action_result(&action, &result_with(missing, output))
        .await
        .unwrap();
    assert!(!bucket.puts().contains(&action_key(&action)));
}

#[tokio::test]
async fn downloads_larger_than_the_staging_budget_are_exhausted() {
    let bucket = Bucket::new();
    let (first_root, second_root) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let digest = client(&first_root, &bucket, RemoteAccess::ReadWrite)
        .put_blob(&vec![7; 64 * 1024])
        .await
        .unwrap();
    let second = client(&second_root, &bucket, RemoteAccess::ReadWrite)
        .with_transfer_budget(TransferBudget::new(64 * 1024 * 1024, 64 * 1024));
    let error = second.get_blob(&digest).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Exhausted);
}

#[tokio::test]
async fn directory_stores_round_trip_multipart_uploads() {
    let directory = TempDir::new().unwrap();
    let url = format!("file://{}/cache", directory.path().display());
    let remote = ObjectStoreRemote::from_url(&url, &BTreeMap::new()).unwrap();
    let source = directory.path().join("source");
    let bytes: Vec<u8> = (0..8 * 1024 * 1024 + 3_u32)
        .map(|i| (i % 13) as u8)
        .collect();
    std::fs::write(&source, &bytes).unwrap();
    remote
        .put_file("cas/abc", &source, bytes.len() as u64)
        .await
        .unwrap();
    assert_eq!(
        remote.head("cas/abc").await.unwrap(),
        Some(bytes.len() as u64)
    );
    assert_eq!(remote.head("cas/missing").await.unwrap(), None);
    assert!(remote.get("cas/missing").await.unwrap().is_none());
    assert_eq!(
        std::fs::read(directory.path().join("cache/v1/cas/abc")).unwrap(),
        bytes
    );
}

#[test]
fn the_environment_can_only_narrow_access() {
    let resolve = |configured, env| RemoteAccess::resolve(configured, env).unwrap();
    assert_eq!(resolve(false, None), RemoteAccess::ReadWrite);
    assert_eq!(resolve(false, Some("1")), RemoteAccess::ReadOnly);
    assert_eq!(resolve(true, Some("0")), RemoteAccess::ReadOnly);
    assert_eq!(
        RemoteAccess::resolve(false, Some("yes"))
            .unwrap_err()
            .kind(),
        ErrorKind::Misconfigured
    );
}
