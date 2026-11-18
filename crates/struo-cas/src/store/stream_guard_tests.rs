use super::*;
use std::sync::Arc;
use std::time::Duration;

struct Guard {
    _permit: tokio::sync::OwnedSemaphorePermit,
    entered: Option<tokio::sync::oneshot::Sender<()>>,
    release: std::sync::mpsc::Receiver<()>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.entered.take().unwrap().send(());
        let _ = self.release.recv_timeout(Duration::from_secs(2));
    }
}

#[tokio::test]
async fn cancelling_ingest_keeps_the_worker_reservation_until_worker_drop_finishes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let resources = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = Arc::clone(&resources).acquire_owned().await.unwrap();
    let (entered, dropping) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let guard = Guard {
        _permit: permit,
        entered: Some(entered),
        release: wait,
    };
    let (_writer, reader) = tokio::io::duplex(64);
    let task = tokio::spawn(async move { ingest_remote(&root, reader, 0, false, guard).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if std::fs::read_dir(temp.path()).unwrap().count() == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(2), dropping)
        .await
        .unwrap()
        .unwrap();
    assert!(resources.try_acquire().is_err());
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    release.send(()).unwrap();
    let permit = tokio::time::timeout(Duration::from_secs(2), resources.acquire())
        .await
        .unwrap()
        .unwrap();
    drop(permit);
}
