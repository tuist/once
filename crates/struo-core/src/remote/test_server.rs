use std::collections::BTreeMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub hold: Option<HeldResponse>,
}

/// Server side of [`Hold`]: report arrival, then wait for the test to release.
pub(super) struct HeldResponse {
    arrived: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

/// Test side of a held response. The request has reached the server once
/// `arrived` resolves; the response is written once `release` fires or drops.
pub(super) struct Hold {
    pub arrived: tokio::sync::oneshot::Receiver<()>,
    pub release: tokio::sync::oneshot::Sender<()>,
}

impl Response {
    pub(super) fn json(body: &str) -> Self {
        Self {
            status: 200,
            content_type: "application/json",
            body: body.as_bytes().to_vec(),
            hold: None,
        }
    }

    pub(super) fn empty(status: u16) -> Self {
        Self {
            status,
            content_type: "application/octet-stream",
            body: Vec::new(),
            hold: None,
        }
    }

    pub(super) fn tar_file(path: &str, contents: &[u8]) -> Self {
        let mut archive = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(u64::try_from(contents.len()).unwrap());
        header.set_mode(0o644);
        header.set_cksum();
        archive.append_data(&mut header, path, contents).unwrap();
        archive.finish().unwrap();
        Self {
            status: 200,
            content_type: "application/x-tar",
            body: archive.into_inner().unwrap(),
            hold: None,
        }
    }

    /// Keep the connection open after reading the request until released.
    pub(super) fn held(mut self) -> (Self, Hold) {
        let (arrived_tx, arrived) = tokio::sync::oneshot::channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        self.hold = Some(HeldResponse {
            arrived: arrived_tx,
            release: release_rx,
        });
        (self, Hold { arrived, release })
    }
}

pub(super) struct Request {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
}

pub(super) async fn spawn(
    responses: Vec<Response>,
) -> (String, tokio::task::JoinHandle<Vec<Request>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for mut response in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            requests.push(read_request(&mut stream).await);
            if let Some(hold) = response.hold.take() {
                let _ = hold.arrived.send(());
                let _ = hold.release.await;
            }
            write_response(&mut stream, &response).await;
        }
        requests
    });
    (format!("http://{address}"), task)
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Request {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(read > 0, "request closed before its headers arrived");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(index) = find_header_end(&bytes) {
            break index;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let mut lines = headers.lines();
    let first = lines.next().unwrap();
    let mut first = first.split_whitespace();
    let method = first.next().unwrap().to_string();
    let path = first.next().unwrap().to_string();
    let header_map = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect::<BTreeMap<_, _>>();
    let content_length = header_map
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < header_end + 4 + content_length {
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk).await.unwrap();
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    let body_start = header_end + 4;
    let body_end = (body_start + content_length).min(bytes.len());
    Request {
        method,
        path,
        body: bytes[body_start..body_end].to_vec(),
    }
}

async fn write_response(stream: &mut tokio::net::TcpStream, response: &Response) {
    let reason = match response.status {
        200 => "OK",
        204 => "No Content",
        500 => "Internal Server Error",
        _ => "Response",
    };
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        reason,
        response.content_type,
        response.body.len()
    );
    // A cancelled client may have closed the connection already.
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(&response.body).await;
    let _ = stream.shutdown().await;
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Fails unless `future` is still pending after a short grace period.
pub(super) async fn assert_pending<F: std::future::Future>(future: F) {
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), future)
            .await
            .is_err(),
        "expected the future to stay pending"
    );
}

/// Fails unless `future` completes promptly.
pub(super) async fn assert_ready<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(std::time::Duration::from_secs(5), future)
        .await
        .expect("expected the future to complete")
}

/// A one-request remote quota, so a retained lease blocks the next admission.
pub(super) fn single_remote_quota() -> crate::ExecutorResources {
    crate::ExecutorResources::new(std::sync::Arc::new(crate::ResourcePool::new(
        crate::ResourceLimits::new(4, 0),
    )))
    .with_remote_concurrency(1)
}
