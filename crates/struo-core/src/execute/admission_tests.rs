use std::fs::File;
use std::io::{ErrorKind, Read};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use struo_cas::{CacheProvider, Cas};

use crate::{
    Action, Executor, ExecutorResources, ResourceLimits, ResourcePool, ResourceRequest,
    WorkspacePath,
};

const SETTLE: Duration = Duration::from_millis(200);
const DEADLINE: Duration = Duration::from_secs(30);

/// Far above any pipe's capacity, so the writer blocks with bytes unread.
const PAYLOAD_BYTES: usize = 8 * 1024 * 1024;

/// The read end of a FIFO, opened without blocking so the writer's open never
/// waits for a reader. Dropping it drains the pipe on a thread until the
/// writer closes it, so a blocked write always finishes and a failing test
/// cannot hang the runtime's shutdown on it.
struct Reader(Option<File>);

impl Reader {
    fn open(fifo: &Path) -> Self {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(fifo)
            .unwrap();
        Self(Some(file))
    }

    /// Wait until the writer has put bytes in the pipe, which means it is
    /// inside its blocking write.
    async fn wait_for_bytes(&mut self) -> bool {
        let file = self.0.as_mut().expect("reader is open");
        let started = Instant::now();
        let mut byte = [0_u8; 1];
        while started.elapsed() < DEADLINE {
            match file.read(&mut byte) {
                Ok(1) => return true,
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) => panic!("reading the FIFO: {error}"),
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        false
    }

    /// Read until the writer closes the pipe, or the deadline passes. Until
    /// a writer has been seen, end of file only means none has opened yet,
    /// so keep waiting for one.
    fn drain(mut file: File, mut seen_data: bool) {
        let started = Instant::now();
        let mut buffer = vec![0_u8; 64 * 1024];
        while started.elapsed() < DEADLINE {
            match file.read(&mut buffer) {
                Ok(0) if seen_data => return,
                Ok(0) => std::thread::sleep(Duration::from_millis(5)),
                Ok(_) => seen_data = true,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(_) => return,
            }
        }
    }

    fn finish(mut self) {
        let file = self.0.take().expect("reader is open");
        std::thread::spawn(move || Self::drain(file, true))
            .join()
            .unwrap();
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        if let Some(file) = self.0.take() {
            std::thread::spawn(move || Self::drain(file, false));
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_file_action_keeps_its_admission_until_its_file_work_stops() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(Cas::open(workspace.path().join(".cas")));
    let fifo = workspace.path().join("out.fifo");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let mut reader = Reader::open(&fifo);
    let resources =
        ExecutorResources::new(Arc::new(ResourcePool::new(ResourceLimits::new(1, 1 << 30))));
    let request = ResourceRequest::default();
    let lease = resources.acquire(Executor::Local, &request).await;
    let action = Action::WriteFile {
        path: WorkspacePath::try_from("out.fifo").unwrap(),
        bytes: vec![b'x'; PAYLOAD_BYTES],
        input_digest: None,
    };

    let mut run = Box::pin(super::run_admitted(
        &action,
        workspace.path(),
        &cache,
        false,
        false,
        None,
        Some(lease),
    ));
    let writing = tokio::select! {
        result = &mut run => panic!("the write finished while the pipe was full: {result:?}"),
        writing = reader.wait_for_bytes() => writing,
    };
    assert!(writing, "the write never reached the pipe");
    drop(run);

    let held = tokio::time::timeout(SETTLE, resources.acquire(Executor::Local, &request))
        .await
        .is_err();
    reader.finish();
    assert!(
        held,
        "the reservation was released while the write was still running"
    );
    tokio::time::timeout(
        Duration::from_secs(5),
        resources.acquire(Executor::Local, &request),
    )
    .await
    .expect("the reservation outlived the write");
}
