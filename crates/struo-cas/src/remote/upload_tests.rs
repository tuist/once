use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::Notify;

use super::*;

#[derive(Debug)]
struct BlockingStore {
    started: Arc<Notify>,
    release: Arc<Mutex<std::sync::mpsc::Receiver<()>>>,
    payload: Arc<Mutex<Option<Vec<u8>>>>,
}

impl BlockingStore {
    async fn write(&self, read: impl FnOnce() -> Vec<u8> + Send + 'static) -> Result<()> {
        let started = Arc::clone(&self.started);
        let release = Arc::clone(&self.release);
        let payload = Arc::clone(&self.payload);
        tokio::task::spawn_blocking(move || {
            started.notify_one();
            release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            *payload.lock().unwrap() = Some(read());
        })
        .await
        .unwrap();
        Ok(())
    }
}

#[async_trait::async_trait]
impl RemoteStore for BlockingStore {
    fn provider(&self) -> &'static str {
        "blocking"
    }
    async fn head(&self, _: &str) -> Result<Option<u64>> {
        Ok(None)
    }
    async fn get(&self, _: &str) -> Result<Option<RemoteBody>> {
        Ok(None)
    }
    async fn put_bytes(&self, _: &str, bytes: Bytes) -> Result<()> {
        self.write(move || bytes.to_vec()).await
    }
    async fn put_file(&self, _: &str, path: &Path, _: u64) -> Result<()> {
        let path = path.to_path_buf();
        self.write(move || std::fs::read(path).unwrap()).await
    }
    fn upload_memory(&self, size: u64) -> u64 {
        size
    }
    fn max_concurrent_transfers(&self) -> usize {
        1
    }
}

#[tokio::test]
async fn cancelled_uploads_retain_admission_and_files_until_backend_completion() {
    for kind in ["bytes", "file", "record"] {
        let root = tempfile::tempdir().unwrap();
        let (release, wait) = std::sync::mpsc::channel();
        let store = Arc::new(BlockingStore {
            started: Arc::default(),
            release: Arc::new(Mutex::new(wait)),
            payload: Arc::default(),
        });
        let budget = TransferBudget::new(DOWNLOAD_MEMORY_BYTES, 1024 * 1024);
        let mut cache = RemoteCache::new(
            Cas::open(root.path()),
            store.clone(),
            RemoteAccess::ReadWrite,
        );
        cache.set_transfer_budget(budget.clone());
        let digest = cache.local.put_blob(b"hello").await.unwrap();
        let worker = cache.clone();
        let task = tokio::spawn(async move {
            match kind {
                "bytes" => {
                    worker.put_blob(b"hello").await.unwrap();
                }
                "file" => worker.publish_local_blob(&digest).await,
                "record" => worker
                    .put_action_result(
                        &digest,
                        &ActionResult {
                            exit_code: 7,
                            stdout: None,
                            stderr: None,
                            outputs: std::collections::BTreeMap::new(),
                        },
                    )
                    .await
                    .unwrap(),
                _ => unreachable!(),
            }
        });
        tokio::time::timeout(Duration::from_secs(5), store.started.notified())
            .await
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(budget.available().0 < budget.memory_bytes(), "{kind}");
        assert_eq!(cache.transfers.available_permits(), 0, "{kind}");
        if kind == "file" {
            assert!(budget.available().1 < budget.staging_bytes());
        }
        release.send(()).unwrap();
        let memory = tokio::time::timeout(
            Duration::from_secs(5),
            budget.reserve_memory(budget.memory_bytes()),
        )
        .await
        .unwrap()
        .unwrap();
        drop(memory);
        assert_eq!(cache.transfers.available_permits(), 1);
        assert_eq!(budget.available().1, budget.staging_bytes());
        let expected = if kind == "record" {
            &b"{\"exit_code\":7}"[..]
        } else {
            &b"hello"[..]
        };
        assert_eq!(store.payload.lock().unwrap().as_deref(), Some(expected));
    }
}
