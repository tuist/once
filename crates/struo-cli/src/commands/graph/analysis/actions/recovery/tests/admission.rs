use std::fs::File;
use std::io::{ErrorKind, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::time::Instant;

use super::*;

mod archives;
mod legacy_capture;
mod repair;

struct Reader(Option<File>);

impl Reader {
    fn open(path: &Path) -> Self {
        Self(Some(
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(path)
                .unwrap(),
        ))
    }

    async fn started(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut byte = [0];
        loop {
            match self.0.as_mut().unwrap().read(&mut byte) {
                Ok(1) => return,
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) => panic!("reading writer rendezvous: {error}"),
            }
            assert!(Instant::now() < deadline, "writer never started");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn drain(file: &mut File, mut seen: bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut buffer = vec![0; 64 * 1024];
        while Instant::now() < deadline {
            match file.read(&mut buffer) {
                Ok(0) if seen => return,
                Ok(0) => std::thread::sleep(Duration::from_millis(1)),
                Ok(_) => seen = true,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(_) => return,
            }
        }
    }

    async fn finish(mut self) {
        let mut file = self.0.take().unwrap();
        tokio::task::spawn_blocking(move || Self::drain(&mut file, true))
            .await
            .unwrap();
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        if let Some(mut file) = self.0.take() {
            std::thread::spawn(move || Self::drain(&mut file, false));
        }
    }
}

async fn cancelled_graph_write(cacheable: bool) {
    let mut workspace = Workspace::new();
    workspace.resources = Arc::new(ResourcePool::new(struo_core::ResourceLimits::new(
        1,
        1 << 30,
    )));
    let path = workspace.path().join("out.fifo");
    assert!(std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .unwrap()
        .success());
    let mut reader = Reader::open(&path);
    let producers = Producers::default();
    let mut declared = action(serde_json::json!({
        "operation": {"kind": "write_file", "path": "out.fifo", "bytes": []},
        "outputs": ["out.fifo"], "cacheable": cacheable,
    }));
    let Some(struo_frontend::analysis::DeclaredActionOperation::WriteFile { bytes, .. }) =
        &mut declared.operation
    else {
        panic!("wrong operation")
    };
    *bytes = vec![b'x'; 8 * 1024 * 1024];
    let analysis = AnalysisResult {
        actions: vec![declared],
        provider: serde_json::json!({}),
        declared_outputs: Vec::new(),
        observations: AnalysisObservations::default(),
    };
    let mut run = Box::pin(workspace.build_with(analysis, &producers, None));
    tokio::select! {
        result = &mut run => panic!("write finished before cancellation: {result:?}"),
        () = reader.started() => {},
    }
    drop(run);
    let lock = cacheable.then(|| {
        let entry = std::fs::read_dir(workspace.path().join(".struo/locks/actions"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(entry.path())
            .unwrap()
    });
    let locked = lock
        .as_ref()
        .is_none_or(|lock| matches!(lock.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
    let held = tokio::time::timeout(
        Duration::from_millis(100),
        workspace.resources.acquire(ResourceRequest::default()),
    )
    .await
    .is_err();
    reader.finish().await;
    assert!(
        held,
        "graph observation released admission while the physical write continued"
    );
    assert!(
        locked,
        "graph observation released the action lock while the physical write continued"
    );
    let _permit = tokio::time::timeout(
        Duration::from_secs(5),
        workspace.resources.acquire(ResourceRequest::default()),
    )
    .await
    .unwrap();
    if let Some(lock) = lock {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match lock.try_lock() {
                    Ok(()) => return,
                    Err(std::fs::TryLockError::WouldBlock) => tokio::task::yield_now().await,
                    Err(error) => panic!("probing action lock: {error}"),
                }
            }
        })
        .await
        .expect("action lock outlived physical work");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cacheable_graph_write_retains_admission_and_lock_after_cancellation() {
    cancelled_graph_write(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uncacheable_graph_write_retains_admission_after_cancellation() {
    cancelled_graph_write(false).await;
}

async fn wait_for_lock(lock: &File) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match lock.try_lock() {
                Ok(()) => return,
                Err(std::fs::TryLockError::WouldBlock) => tokio::task::yield_now().await,
                Err(error) => panic!("probing action lock: {error}"),
            }
        }
    })
    .await
    .expect("action lock outlived physical work");
}

#[test]
fn registry_clones_share_exactly_the_callers_host_pool() {
    let workspace = Workspace::new();
    let producers = Producers::default();
    let clone = producers.clone();
    assert!(Arc::ptr_eq(
        producers.executors(&workspace.resources).unwrap().host(),
        &workspace.resources
    ));
    assert!(std::ptr::eq(
        producers.executors(&workspace.resources).unwrap(),
        clone.executors(&workspace.resources).unwrap()
    ));
    assert!(producers.executors(&Workspace::resources()).is_err());
}
