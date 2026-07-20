use std::collections::BTreeMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    pub(super) fn json(body: &str) -> Self {
        Self {
            status: 200,
            content_type: "application/json",
            body: body.as_bytes().to_vec(),
        }
    }

    pub(super) fn empty(status: u16) -> Self {
        Self {
            status,
            content_type: "application/octet-stream",
            body: Vec::new(),
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
        }
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
        for response in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
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
            requests.push(Request {
                method,
                path,
                body: bytes[body_start..body_end].to_vec(),
            });

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
            stream.write_all(head.as_bytes()).await.unwrap();
            stream.write_all(&response.body).await.unwrap();
            stream.shutdown().await.unwrap();
        }
        requests
    });
    (format!("http://{address}"), task)
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}
