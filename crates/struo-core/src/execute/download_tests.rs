use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{
    Error, ExecutionPermit, Executor, ExecutorResources, RemoteIoLimits, ResourceLimits,
    ResourcePool, ResourceRequest, WorkspacePath,
};

const SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Serve one response: `head` (status line and headers), then `body`, then
/// hold the connection open until the client goes away.
async fn serve_once(head: &'static str, body: &'static [u8]) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 4096];
        let _ = socket.read(&mut request).await;
        socket.write_all(head.as_bytes()).await.unwrap();
        socket.write_all(body).await.unwrap();
        let _ = socket.read(&mut request).await;
    });
    format!("http://{address}/archive.zip")
}

async fn lease(limits: RemoteIoLimits) -> ExecutionPermit {
    ExecutorResources::new(Arc::new(ResourcePool::new(ResourceLimits::new(1, 1 << 30))))
        .with_remote_io_limits(limits)
        .acquire(Executor::Local, &ResourceRequest::default())
        .await
}

async fn download(url: &str, lease: &ExecutionPermit) -> crate::Result<()> {
    let workspace = tempfile::tempdir().unwrap();
    super::download_and_extract(
        url,
        SHA256,
        &WorkspacePath::try_from("vendor/out").unwrap(),
        None,
        workspace.path(),
        Some(lease),
    )
    .await
}

#[tokio::test]
async fn a_declared_length_over_the_limit_is_refused_before_reading() {
    let url = serve_once("HTTP/1.1 200 OK\r\ncontent-length: 1000\r\n\r\n", b"").await;
    let lease = lease(RemoteIoLimits {
        max_archive_bytes: 16,
        ..RemoteIoLimits::default()
    })
    .await;
    let error = download(&url, &lease).await.unwrap_err();
    assert!(
        matches!(
            error,
            Error::ExecutorResourceLimit {
                resource: "download bytes",
                limit: 16,
                ..
            }
        ),
        "{error}"
    );
}

#[tokio::test]
async fn an_undeclared_body_over_the_limit_stops_the_download() {
    let url = serve_once(
        "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n",
        b"20\r\n01234567890123456789012345678901\r\n0\r\n\r\n",
    )
    .await;
    let lease = lease(RemoteIoLimits {
        max_archive_bytes: 16,
        ..RemoteIoLimits::default()
    })
    .await;
    let error = download(&url, &lease).await.unwrap_err();
    assert!(
        matches!(
            error,
            Error::ExecutorResourceLimit {
                resource: "download bytes",
                ..
            }
        ),
        "{error}"
    );
}

#[tokio::test]
async fn a_body_that_stops_arriving_fails_after_the_stall_timeout() {
    let url = serve_once(
        "HTTP/1.1 200 OK\r\ncontent-length: 1000\r\n\r\n",
        b"partial",
    )
    .await;
    let lease = lease(RemoteIoLimits {
        stall_timeout: Duration::from_millis(100),
        ..RemoteIoLimits::default()
    })
    .await;
    let error = tokio::time::timeout(Duration::from_secs(10), download(&url, &lease))
        .await
        .expect("the stalled download was not bounded")
        .unwrap_err();
    assert!(
        matches!(
            error,
            Error::ExecutorStalled {
                phase: "download",
                ..
            }
        ),
        "{error}"
    );
}

#[tokio::test]
async fn a_download_that_keeps_trickling_fails_at_the_absolute_deadline() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/archive.zip", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 4096];
        let _ = socket.read(&mut request).await;
        socket
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 100000\r\n\r\n")
            .await
            .unwrap();
        // One byte every 20 ms: always progress, never complete.
        while socket.write_all(b"x").await.is_ok() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    let budget = Duration::from_millis(300);
    let lease = lease(RemoteIoLimits {
        stall_timeout: Duration::from_secs(5),
        artifact_download_timeout: budget,
        ..RemoteIoLimits::default()
    })
    .await;
    let started = std::time::Instant::now();
    let error = tokio::time::timeout(Duration::from_secs(10), download(&url, &lease))
        .await
        .expect("the trickling download was not bounded")
        .unwrap_err();
    assert!(
        matches!(error, Error::Timeout(timeout) if timeout == budget),
        "{error}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the stall timeout fired instead of the deadline"
    );
}

#[tokio::test]
async fn staging_below_the_archive_and_expansion_bound_fails_closed() {
    let limits = RemoteIoLimits {
        max_archive_bytes: 1024 * 1024,
        max_staging_bytes: 1024 * 1024,
        ..RemoteIoLimits::default()
    };
    let lease = lease(limits).await;
    // Undeclared: the archive limit twice over.
    let undeclared = serve_once(
        "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n",
        b"a\r\n0123456789\r\n0\r\n\r\n",
    )
    .await;
    // Declared: 64 KiB, which may expand to the whole 1 MiB limit.
    let declared = serve_once("HTTP/1.1 200 OK\r\ncontent-length: 65536\r\n\r\n", b"").await;
    for url in [undeclared, declared] {
        let error = download(&url, &lease).await.unwrap_err();
        assert!(
            matches!(error, Error::ResourceRequirementUnsatisfied { .. }),
            "{error}"
        );
    }
}

#[test]
fn a_cancelled_archive_write_keeps_its_file_staging_and_admission_until_it_finishes() {
    use std::sync::mpsc;

    const BOUND: Duration = Duration::from_secs(30);

    // One blocking thread, so a write queues behind the blocker below.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let resources =
            ExecutorResources::new(Arc::new(ResourcePool::new(ResourceLimits::new(1, 1 << 30))))
                .with_remote_io_limits(RemoteIoLimits {
                    max_staging_bytes: 1024 * 1024,
                    ..RemoteIoLimits::default()
                });
        let request = ResourceRequest::default();
        let lease = resources.acquire(Executor::Local, &request).await;
        let staging = lease.acquire_staging(1024 * 1024).await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let archive = Arc::new(super::DownloadedArchive {
            file: tempfile::NamedTempFile::new_in(directory.path()).unwrap(),
            _staging: Some(staging),
        });
        let path = archive.file.path().to_path_buf();

        let (started, blocker_started) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let blocker = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            let _ = released.recv_timeout(BOUND);
        });
        blocker_started.recv_timeout(BOUND).unwrap();

        let mut write = Box::pin(super::write_archive_chunk(
            Some(&lease),
            archive,
            "archive",
            b"payload".to_vec(),
        ));
        assert!(futures::poll!(&mut write).is_pending());
        drop(write);
        drop(lease);

        let observer = resources.acquire_materialization().await;
        let mut staging_wait = Box::pin(observer.acquire_staging(64 * 1024));
        let mut admission_wait = Box::pin(resources.acquire(Executor::Local, &request));
        let file_kept = path.exists();
        let staging_kept = futures::poll!(&mut staging_wait).is_pending();
        let admission_kept = futures::poll!(&mut admission_wait).is_pending();

        release.send(()).unwrap();
        blocker.await.unwrap();
        assert!(file_kept, "the archive was deleted under a queued write");
        assert!(staging_kept, "staging was released under a queued write");
        assert!(
            admission_kept,
            "admission was released under a queued write"
        );
        tokio::time::timeout(BOUND, staging_wait)
            .await
            .expect("staging outlived the write")
            .expect("staging fits");
        tokio::time::timeout(BOUND, admission_wait)
            .await
            .expect("admission outlived the write");
        assert!(!path.exists(), "the archive outlived its last writer");
    });
}

fn zip_of(name: &str, contents: &[u8], method: zip::CompressionMethod) -> Vec<u8> {
    use std::io::Write as _;

    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    archive
        .start_file(
            name,
            zip::write::SimpleFileOptions::default().compression_method(method),
        )
        .unwrap();
    archive.write_all(contents).unwrap();
    archive.finish().unwrap().into_inner()
}

/// Serve `body` once with its length declared.
async fn serve_archive(body: Vec<u8>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 4096];
        let _ = socket.read(&mut request).await;
        let head = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", body.len());
        socket.write_all(head.as_bytes()).await.unwrap();
        socket.write_all(&body).await.unwrap();
        let _ = socket.read(&mut request).await;
    });
    format!("http://{address}/archive.zip")
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    super::hex_lower(&sha2::Sha256::digest(bytes))
}

async fn download_into(
    workspace: &std::path::Path,
    url: &str,
    sha256: &str,
    lease: &ExecutionPermit,
) -> crate::Result<()> {
    super::download_and_extract(
        url,
        sha256,
        &WorkspacePath::try_from("vendor/out").unwrap(),
        None,
        workspace,
        Some(lease),
    )
    .await
}

#[tokio::test]
async fn small_known_length_downloads_reserve_side_by_side() {
    // Two 64 KiB staging units each: the declared kibibyte plus its 64 KiB
    // expansion. The pool holds exactly two such reservations, and none
    // sized for an undeclared length.
    let limits = RemoteIoLimits {
        max_staging_bytes: 256 * 1024,
        ..RemoteIoLimits::default()
    };
    assert_eq!(
        super::download_staging_bytes(Some(1024), limits),
        1024 + 64 * 1024
    );
    assert_eq!(
        super::download_staging_bytes(None, limits),
        2 * limits.max_archive_bytes
    );
    let lease = lease(limits).await;
    let first = lease
        .acquire_staging(super::download_staging_bytes(Some(1024), limits))
        .await
        .unwrap();
    let second = tokio::time::timeout(
        Duration::from_secs(5),
        lease.acquire_staging(super::download_staging_bytes(Some(1024), limits)),
    )
    .await
    .expect("a second small reservation waited for the first")
    .unwrap();
    drop((first, second));

    // End to end: two real downloads under that pool at once.
    let bytes = zip_of("file.txt", b"hello", zip::CompressionMethod::Stored);
    let sha256 = sha256_hex(&bytes);
    let (one, two) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (url_one, url_two) = (
        serve_archive(bytes.clone()).await,
        serve_archive(bytes).await,
    );
    let (first, second) = tokio::join!(
        download_into(one.path(), &url_one, &sha256, &lease),
        download_into(two.path(), &url_two, &sha256, &lease),
    );
    first.unwrap();
    second.unwrap();
    for workspace in [&one, &two] {
        assert_eq!(
            std::fs::read(workspace.path().join("vendor/out/file.txt")).unwrap(),
            b"hello"
        );
    }
}

#[tokio::test]
async fn compression_beyond_the_ratio_is_refused_before_writing() {
    let bytes = zip_of(
        "zeros.bin",
        &vec![0_u8; 1024 * 1024],
        zip::CompressionMethod::Deflated,
    );
    let bound = bytes.len() as u64 * 64;
    assert!(
        bound < 1024 * 1024,
        "the fixture must compress past the ratio"
    );
    let sha256 = sha256_hex(&bytes);
    let url = serve_archive(bytes).await;
    let workspace = tempfile::tempdir().unwrap();
    let lease = lease(RemoteIoLimits::default()).await;
    let error = download_into(workspace.path(), &url, &sha256, &lease)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            Error::ExecutorResourceLimit {
                resource: "extracted bytes",
                limit,
                ..
            } if limit == bound
        ),
        "{error}"
    );
    assert!(!workspace.path().join("vendor/out").exists());
    let leftovers = std::fs::read_dir(workspace.path().join("vendor"))
        .unwrap()
        .count();
    assert_eq!(leftovers, 0, "extraction left staging behind");
}
