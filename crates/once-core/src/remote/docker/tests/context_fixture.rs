use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

pub(super) async fn original(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(std::env::var_os("ONCE_DOCKER_REAL_BIN").unwrap());
    clear_connection_environment(&mut command);
    command
        .arg("--config")
        .arg(root.join("config"))
        .args(["--context", "original"])
        .args(args)
        .output()
        .await
        .unwrap()
}

pub(super) fn clear_connection_environment(command: &mut Command) {
    for key in [
        "DOCKER_HOST",
        "DOCKER_CONTEXT",
        "DOCKER_TLS",
        "DOCKER_TLS_VERIFY",
        "DOCKER_CERT_PATH",
    ] {
        command.env_remove(key);
    }
}

pub(super) struct Fixture {
    pub root: tempfile::TempDir,
    requests: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    pub async fn new(binary: &OsStr, exported: &[u8]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("host=tcp://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                count.fetch_add(1, Ordering::Relaxed);
                let mut header = [0_u8; 8192];
                let len = socket.read(&mut header).await.unwrap();
                let request = String::from_utf8_lossy(&header[..len]);
                let body = if request.contains("/info ") {
                    r#"{"ID":"empty-daemon","NCPU":2,"MemTotal":1073741824}"#
                } else if request.contains("/containers/json") {
                    "[]"
                } else {
                    "OK"
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nApi-Version: 1.47\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        let archive = root.path().join("context.tar");
        std::fs::write(&archive, exported).unwrap();
        for args in [
            vec!["context", "import", "original", archive.to_str().unwrap()],
            vec!["context", "create", "empty", "--docker", &endpoint],
            vec!["context", "use", "original"],
        ] {
            let mut command = Command::new(binary);
            clear_connection_environment(&mut command);
            let output = command
                .arg("--config")
                .arg(root.path().join("config"))
                .args(args)
                .output()
                .await
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::fs::remove_file(archive).unwrap();
        let wrapper = root.path().join("docker-wrapper");
        std::fs::write(&wrapper, include_str!("context-wrapper.py")).unwrap();
        std::fs::set_permissions(wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            root,
            requests,
            server,
        }
    }

    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
