use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::*;

#[derive(Debug, Default)]
struct Store {
    active: AtomicUsize,
    peak: AtomicUsize,
    gets: AtomicUsize,
    puts: AtomicUsize,
    pending: bool,
    writers: std::sync::Mutex<Vec<tokio::io::DuplexStream>>,
}

impl Store {
    async fn request(&self) {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        tokio::task::yield_now().await;
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl RemoteStore for Store {
    fn provider(&self) -> &'static str {
        "test"
    }
    async fn head(&self, _: &str) -> Result<Option<u64>> {
        self.request().await;
        Ok(Some(3))
    }
    async fn get(&self, _: &str) -> Result<Option<RemoteBody>> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        self.request().await;
        let reader: Pin<Box<dyn AsyncRead + Send>> = if self.pending {
            let (writer, reader) = tokio::io::duplex(64);
            self.writers.lock().unwrap().push(writer);
            Box::pin(reader)
        } else {
            Box::pin(std::io::Cursor::new(b"abc"))
        };
        Ok(Some(RemoteBody { size: 3, reader }))
    }
    async fn put_bytes(&self, _: &str, _: Bytes) -> Result<()> {
        self.puts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn put_file(&self, _: &str, _: &Path, _: u64) -> Result<()> {
        self.puts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn upload_memory(&self, _: u64) -> u64 {
        4096
    }
    fn max_concurrent_transfers(&self) -> usize {
        2
    }
}

fn cache(root: &Path, store: &Arc<Store>, budget: TransferBudget) -> RemoteCache {
    let mut cache = RemoteCache::new(
        Cas::open(root),
        Arc::clone(store) as Arc<dyn RemoteStore>,
        RemoteAccess::ReadWrite,
    );
    cache.set_transfer_budget(budget);
    cache
}

#[tokio::test]
async fn concurrent_reads_share_admission_including_metadata_and_batches() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::default());
    let cache = cache(root.path(), &store, TransferBudget::default());
    let digest = Digest::of_bytes(b"abc");
    let result = ActionResult {
        exit_code: 0,
        stdout: Some(digest),
        stderr: Some(Digest::of_bytes(b"second missing local blob")),
        outputs: BTreeMap::new(),
    };
    let jobs = (0..30).map(|i| {
        let cache = &cache;
        let result = &result;
        async move {
            match i % 4 {
                0 => assert!(cache.has_blob(&digest).await.unwrap()),
                1 => assert!(cache.referenced_blobs_available(result).await.unwrap()),
                2 => assert_eq!(
                    cache
                        .get_action_result_remote(&digest)
                        .await
                        .unwrap_err()
                        .kind(),
                    ErrorKind::Protocol
                ),
                _ => assert_eq!(cache.get_blob(&digest).await.unwrap(), b"abc"),
            }
        }
    });
    futures::future::join_all(jobs).await;
    assert!(store.peak.load(Ordering::SeqCst) <= 2);
    assert!(store.gets.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn inadequate_budgets_reject_before_get_decode_and_record_publication() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::default());
    let cache = cache(root.path(), &store, TransferBudget::new(0, 1024 * 1024));
    let digest = Digest::of_bytes(b"abc");
    assert_eq!(
        cache.get_blob(&digest).await.unwrap_err().kind(),
        ErrorKind::Exhausted
    );
    assert_eq!(store.gets.load(Ordering::SeqCst), 0);
    let digest = cache.local.put_blob(b"abc").await.unwrap();
    // HEAD reports three bytes, so use a different local size to require upload.
    let digest = cache.local.put_blob(digest.as_bytes()).await.unwrap();
    std::fs::write(cache.local.blob_path(&digest), vec![0; 32]).unwrap();
    assert_eq!(
        cache.upload_local_blob(&digest).await.unwrap_err().kind(),
        ErrorKind::Exhausted
    );
    let result = ActionResult {
        exit_code: 0,
        stdout: None,
        stderr: None,
        outputs: BTreeMap::new(),
    };
    assert_eq!(
        cache
            .put_action_result_remote(&digest, &result)
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::Exhausted
    );
    assert_eq!(store.puts.load(Ordering::SeqCst), 0);
}

struct DropGate {
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
async fn cancelled_download_keeps_budget_and_slot_until_worker_finishes() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store {
        pending: true,
        ..Store::default()
    });
    let budget = TransferBudget::new(DOWNLOAD_MEMORY_BYTES, 1024 * 1024);
    let cache = cache(root.path(), &store, budget.clone());
    let (entered, dropping) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let guard = DropGate {
        entered: Some(entered),
        release: wait,
    };
    let worker_cache = cache.clone();
    let task = tokio::spawn(async move {
        worker_cache
            .ensure_blob_local_guarded(&Digest::of_bytes(b"abc"), guard)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while std::fs::read_dir(cache.local.scratch_dir()).map_or(0, Iterator::count) != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(5), dropping)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(budget.available().0, 0);
    assert!(budget.available().1 < budget.staging_bytes());
    assert_eq!(cache.transfers.available_permits(), 1);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while budget.available() != (budget.memory_bytes(), budget.staging_bytes()) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(cache.transfers.available_permits(), 2);
}
