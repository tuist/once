use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use fabrik_cas::{ActionResult, CacheProvider, Digest};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::{Error, Result};

const ENABLE_ENV: &str = "FABRIK_GITHUB_ACTIONS_CACHE_BRIDGE";
const TOKEN: &str = "fabrik-gha-cache-bridge";

pub(crate) fn enabled() -> bool {
    std::env::var_os(ENABLE_ENV).is_some_and(|value| !value.is_empty() && value != "0")
}

pub(crate) struct Bridge {
    base_url: String,
    task: JoinHandle<()>,
}

impl Bridge {
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    pub(crate) fn token() -> &'static str {
        TOKEN
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) async fn start(cache: CacheProvider) -> Result<Option<Bridge>> {
    if !enabled() {
        return Ok(None);
    }

    start_enabled(cache).await.map(Some)
}

async fn start_enabled(cache: CacheProvider) -> Result<Bridge> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|source| Error::CacheBridge {
            message: format!("binding local listener: {source}"),
        })?;
    let addr = listener.local_addr().map_err(|source| Error::CacheBridge {
        message: format!("reading local listener address: {source}"),
    })?;
    let base_url = format!("http://{addr}/");
    let state = Arc::new(Mutex::new(State::new(cache)));
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                let _ = handle_connection(stream, state).await;
            });
        }
    });

    Ok(Bridge { base_url, task })
}

struct State {
    cache: CacheProvider,
    next_upload_id: u64,
    uploads: HashMap<u64, PendingUpload>,
}

impl State {
    fn new(cache: CacheProvider) -> Self {
        Self {
            cache,
            next_upload_id: 1,
            uploads: HashMap::new(),
        }
    }

    fn reserve(&mut self, key: String, version: String) -> u64 {
        let id = self.next_upload_id;
        self.next_upload_id += 1;
        self.uploads.insert(
            id,
            PendingUpload {
                key,
                version,
                bytes: Vec::new(),
            },
        );
        id
    }
}

struct PendingUpload {
    key: String,
    version: String,
    bytes: Vec<u8>,
}

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

async fn handle_connection(mut stream: TcpStream, state: Arc<Mutex<State>>) -> Result<()> {
    let Some(request) = read_request(&mut stream).await? else {
        return Ok(());
    };
    let response = handle_request(request, state).await;
    write_response(&mut stream, response).await
}

async fn handle_request(request: Request, state: Arc<Mutex<State>>) -> Response {
    if request.method == "GET" && request.path.starts_with("/_apis/artifactcache/cache?") {
        return restore(request, state).await;
    }
    if request.method == "POST" && request.path == "/_apis/artifactcache/caches" {
        return reserve(request, state).await;
    }
    if request.method == "PATCH" && request.path.starts_with("/_apis/artifactcache/caches/") {
        return upload_chunk(request, state).await;
    }
    if request.method == "POST" && request.path.starts_with("/_apis/artifactcache/caches/") {
        return commit(request, state).await;
    }
    if request.method == "GET" && request.path.starts_with("/_fabrik/gha-cache/archive/") {
        return download(request, state).await;
    }
    Response::not_found()
}

async fn restore(request: Request, state: Arc<Mutex<State>>) -> Response {
    let query = request.path.split_once('?').map_or("", |(_, query)| query);
    let keys = query_param(query, "keys")
        .map(|keys| keys.split(',').map(ToString::to_string).collect::<Vec<_>>())
        .unwrap_or_default();
    let Some(version) = query_param(query, "version") else {
        return Response::status(400, "missing version");
    };

    let cache = { state.lock().await.cache.clone() };
    for key in keys {
        let action = cache_key_digest(&key, &version);
        let Ok(Some(result)) = cache.get_action_result(&action).await else {
            continue;
        };
        let digest = result.stdout;
        let body = RestoreResponse {
            cache_key: key,
            archive_location: format!(
                "http://{}/_fabrik/gha-cache/archive/{digest}",
                request
                    .headers
                    .get("host")
                    .map_or("127.0.0.1", String::as_str)
            ),
        };
        return Response::json(200, &body);
    }

    Response::empty(204)
}

async fn reserve(request: Request, state: Arc<Mutex<State>>) -> Response {
    let Ok(body) = serde_json::from_slice::<ReserveRequest>(&request.body) else {
        return Response::status(400, "invalid reserve body");
    };
    let id = state.lock().await.reserve(body.key, body.version);
    Response::json(200, &ReserveResponse { cache_id: id })
}

async fn upload_chunk(request: Request, state: Arc<Mutex<State>>) -> Response {
    let Some(id) = upload_id(&request.path) else {
        return Response::status(404, "unknown upload");
    };
    let start = request
        .headers
        .get("content-range")
        .and_then(|value| content_range_start(value))
        .unwrap_or(0);
    let mut state = state.lock().await;
    let Some(upload) = state.uploads.get_mut(&id) else {
        return Response::status(404, "unknown upload");
    };
    let end = start + request.body.len();
    if upload.bytes.len() < end {
        upload.bytes.resize(end, 0);
    }
    upload.bytes[start..end].copy_from_slice(&request.body);
    Response::empty(204)
}

async fn commit(request: Request, state: Arc<Mutex<State>>) -> Response {
    let Some(id) = upload_id(&request.path) else {
        return Response::status(404, "unknown upload");
    };
    let (cache, upload) = {
        let mut state = state.lock().await;
        let Some(upload) = state.uploads.remove(&id) else {
            return Response::status(404, "unknown upload");
        };
        (state.cache.clone(), upload)
    };
    let Ok(blob) = cache.put_blob(&upload.bytes).await else {
        return Response::status(500, "failed to store archive");
    };
    let Ok(empty) = cache.put_blob(&[]).await else {
        return Response::status(500, "failed to store archive metadata");
    };
    let result = ActionResult {
        exit_code: 0,
        stdout: blob,
        stderr: empty,
        outputs: BTreeMap::default(),
    };
    let action = cache_key_digest(&upload.key, &upload.version);
    if cache.put_action_result(&action, &result).await.is_err() {
        return Response::status(500, "failed to store archive index");
    }

    Response::json(200, &serde_json::json!({}))
}

async fn download(request: Request, state: Arc<Mutex<State>>) -> Response {
    let Some(raw) = request.path.rsplit('/').next() else {
        return Response::not_found();
    };
    let Some(digest) = Digest::from_hex(raw) else {
        return Response::not_found();
    };
    let cache = { state.lock().await.cache.clone() };
    match cache.get_blob(&digest).await {
        Ok(bytes) => Response {
            status: 200,
            content_type: "application/octet-stream",
            body: bytes,
        },
        Err(_) => Response::not_found(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReserveRequest {
    key: String,
    version: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReserveResponse {
    cache_id: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RestoreResponse {
    cache_key: String,
    archive_location: String,
}

struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Response {
    fn empty(status: u16) -> Self {
        Self {
            status,
            content_type: "application/octet-stream",
            body: Vec::new(),
        }
    }

    fn status(status: u16, message: &str) -> Self {
        Self {
            status,
            content_type: "text/plain",
            body: message.as_bytes().to_vec(),
        }
    }

    fn not_found() -> Self {
        Self::status(404, "not found")
    }

    fn json(status: u16, value: &impl Serialize) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: serde_json::to_vec(value).expect("response serializes"),
        }
    }
}

async fn read_request(stream: &mut TcpStream) -> Result<Option<Request>> {
    let mut raw = Vec::new();
    let header_end = loop {
        let mut buf = [0_u8; 8192];
        let n = stream.read(&mut buf).await.map_err(|source| Error::Wait {
            program: "github actions cache bridge".to_string(),
            source,
        })?;
        if n == 0 {
            return Ok(None);
        }
        raw.extend_from_slice(&buf[..n]);
        if let Some(pos) = find_header_end(&raw) {
            break pos;
        }
    };

    let header = String::from_utf8_lossy(&raw[..header_end]);
    let mut lines = header.lines();
    let Some(start) = lines.next() else {
        return Ok(None);
    };
    let mut parts = start.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    let mut body = raw[body_start..].to_vec();
    while body.len() < content_length {
        let mut buf = vec![0_u8; content_length - body.len()];
        stream
            .read_exact(&mut buf)
            .await
            .map_err(|source| Error::Wait {
                program: "github actions cache bridge".to_string(),
                source,
            })?;
        body.extend_from_slice(&buf);
    }
    body.truncate(content_length);

    Ok(Some(Request {
        method,
        path,
        headers,
        body,
    }))
}

async fn write_response(stream: &mut TcpStream, response: Response) -> Result<()> {
    let status_text = match response.status {
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        status_text,
        response.content_type,
        response.body.len()
    );
    stream
        .write_all(head.as_bytes())
        .await
        .map_err(|source| Error::Wait {
            program: "github actions cache bridge".to_string(),
            source,
        })?;
    stream
        .write_all(&response.body)
        .await
        .map_err(|source| Error::Wait {
            program: "github actions cache bridge".to_string(),
            source,
        })?;
    Ok(())
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn upload_id(path: &str) -> Option<u64> {
    path.rsplit('/').next()?.parse().ok()
}

fn content_range_start(value: &str) -> Option<usize> {
    let value = value.strip_prefix("bytes ")?;
    let (start, _) = value.split_once('-')?;
    start.parse().ok()
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == name).then(|| percent_decode(value))
    })
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(hex);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn cache_key_digest(key: &str, version: &str) -> Digest {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"fabrik.gha-cache.v1\0");
    bytes.extend_from_slice(key.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(version.as_bytes());
    Digest::of_bytes(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn v1_protocol_round_trips_archive_through_cache_provider() {
        let tmp = TempDir::new().unwrap();
        let cache = CacheProvider::open_local(tmp.path().join("cas"));
        let bridge = start_enabled(cache).await.unwrap();
        let client = reqwest::Client::new();
        let base = bridge.base_url();

        let reserve = client
            .post(format!("{base}_apis/artifactcache/caches"))
            .json(&serde_json::json!({
                "key": "npm-linux-lock",
                "version": "version-1"
            }))
            .send()
            .await
            .unwrap();
        assert!(reserve.status().is_success());
        let reserve: serde_json::Value = reserve.json().await.unwrap();
        let cache_id = reserve["cacheId"].as_u64().unwrap();

        let archive = b"archive-bytes";
        let upload = client
            .patch(format!("{base}_apis/artifactcache/caches/{cache_id}"))
            .header("Content-Range", "bytes 0-12/*")
            .body(archive.as_slice().to_vec())
            .send()
            .await
            .unwrap();
        assert!(upload.status().is_success());

        let commit = client
            .post(format!("{base}_apis/artifactcache/caches/{cache_id}"))
            .json(&serde_json::json!({ "size": archive.len() }))
            .send()
            .await
            .unwrap();
        assert!(commit.status().is_success());

        let restore = client
            .get(format!(
                "{base}_apis/artifactcache/cache?keys=npm-linux-lock&version=version-1"
            ))
            .send()
            .await
            .unwrap();
        assert!(restore.status().is_success());
        let restore: serde_json::Value = restore.json().await.unwrap();
        assert_eq!(restore["cacheKey"], "npm-linux-lock");
        let archive_location = restore["archiveLocation"].as_str().unwrap();

        let downloaded = client
            .get(archive_location)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(&downloaded[..], archive);
    }
}
