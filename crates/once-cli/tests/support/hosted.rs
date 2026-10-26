use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::process::Command;

use super::cache_miss;

pub const ID: &str = "01900000-0000-7000-8000-000000000001";

pub struct State {
    pub modules: Vec<String>,
    pub assigned: Option<Vec<String>>,
    pub artifact: Vec<u8>,
    pub reports: Vec<Value>,
    pub requests: Vec<(String, Value)>,
    pub protocol_errors: Vec<String>,
    pub reject_upload: bool,
    pub reject_report: bool,
}

pub struct Fixture {
    pub workspace: tempfile::TempDir,
    pub state: Arc<Mutex<State>>,
}

impl Fixture {
    #[allow(clippy::too_many_lines)]
    pub async fn new() -> Self {
        let state = Arc::new(Mutex::new(State {
            modules: vec![],
            assigned: None,
            artifact: vec![],
            reports: vec![],
            requests: vec![],
            protocol_errors: vec![],
            reject_upload: false,
            reject_report: false,
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server_url = url.clone();
        let shared = state.clone();
        let cache = cache_miss::endpoint().await;
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let shared = shared.clone();
                let url = server_url.clone();
                let cache = cache.clone();
                tokio::spawn(async move {
                    let mut request = vec![];
                    let header_end = loop {
                        let mut bytes = [0; 8192];
                        let size = socket.read(&mut bytes).await.unwrap();
                        if size == 0 {
                            return;
                        }
                        request.extend_from_slice(&bytes[..size]);
                        if let Some(index) =
                            request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                        {
                            break index + 4;
                        }
                    };
                    let header = String::from_utf8_lossy(&request[..header_end]).to_string();
                    let length: usize = header
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    while request.len() < header_end + length {
                        let mut bytes = [0; 8192];
                        let size = socket.read(&mut bytes).await.unwrap();
                        if size == 0 {
                            return;
                        }
                        request.extend_from_slice(&bytes[..size]);
                    }
                    let route = header
                        .lines()
                        .next()
                        .unwrap()
                        .split_whitespace()
                        .nth(1)
                        .unwrap();
                    let body = &request[header_end..header_end + length];
                    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
                    let (status, response, etag) = {
                        let mut state = shared.lock().unwrap();
                        state.requests.push((route.to_string(), value.clone()));
                        if route.starts_with("/storage") {
                            if header.to_ascii_lowercase().contains("authorization:") {
                                state
                                    .protocol_errors
                                    .push("credentials sent to storage".into());
                            }
                        } else if route.starts_with("/api/projects/")
                            && !header.contains("Bearer fixture-token")
                        {
                            state
                                .protocol_errors
                                .push("missing API authentication".into());
                        }
                        if route == "/.well-known/once" {
                            (200, b"{\"events\":[]}".to_vec(), false)
                        } else if route.starts_with("/api/cache/endpoints") {
                            (
                                200,
                                json!({"endpoints": [cache]}).to_string().into_bytes(),
                                false,
                            )
                        } else if route.ends_with("/tests/shards") {
                            state.modules =
                                serde_json::from_value(value["modules"].clone()).unwrap();
                            (
                                200,
                                json!({"id": ID, "shard_count": 2}).to_string().into_bytes(),
                                false,
                            )
                        } else if route.ends_with("upload/start") {
                            (
                                200,
                                b"{\"data\":{\"upload_id\":\"upload-1\"}}".to_vec(),
                                false,
                            )
                        } else if route.ends_with("upload/generate-url") {
                            (
                                200,
                                json!({"data":{"url":format!("{url}/storage")}})
                                    .to_string()
                                    .into_bytes(),
                                false,
                            )
                        } else if route == "/storage" && header.starts_with("PUT ") {
                            state.artifact = body.to_vec();
                            (if state.reject_upload { 500 } else { 200 }, vec![], true)
                        } else if route.ends_with("upload/complete") {
                            (200, b"{}".to_vec(), false)
                        } else if route.contains("/tests/shards/") {
                            let assigned = state.assigned.as_ref().unwrap_or(&state.modules);
                            (200, json!({"shard_plan_id": ID, "modules": assigned, "suites": {}, "skip": [], "download_urls": [format!("{url}/storage")]}).to_string().into_bytes(), false)
                        } else if route == "/storage" {
                            (200, state.artifact.clone(), false)
                        } else if route.ends_with("/tests/") {
                            state.reports.push(value);
                            (
                                if state.reject_report { 503 } else { 200 },
                                b"{}".to_vec(),
                                false,
                            )
                        } else {
                            (404, vec![], false)
                        }
                    };
                    let headers = format!("HTTP/1.1 {status} Response\r\ncontent-length: {}\r\nconnection: close\r\n{}\r\n", response.len(), if etag {"etag: \"etag-1\"\r\n"} else {""});
                    socket.write_all(headers.as_bytes()).await.unwrap();
                    socket.write_all(&response).await.unwrap();
                });
            }
        });
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("modules")).unwrap();
        std::fs::write(
            workspace.path().join("modules/fixture.star"),
            include_str!("hosted.star"),
        )
        .unwrap();
        std::fs::write(
            workspace.path().join("runner.sh"),
            r#"printf '%s\n' "$3" >> ran
mode=$(cat mode 2>/dev/null)
if [ "$mode" = sleep ]; then sleep 60; fi
cp "$2" "$1"
if [ "$mode" = case-failed ]; then cp "$4" "$1"; fi
if [ "$mode" = incomplete ]; then rm "$1"; fi
if [ "$mode" = failed ]; then exit 23; fi
"#,
        )
        .unwrap();
        std::fs::write(
            workspace.path().join("once.toml"),
            format!(
                r#"
[infrastructure.cache]
provider = "tuist"
[infrastructures.tuist]
kind = "tuist"
url = "{url}"
account = "acme"
project = "app"
[modules]
paths = ["modules/*.star"]
[[target]]
name = "native"
kind = "fixture"
srcs = ["runner.sh"]
"#
            ),
        )
        .unwrap();
        Self { workspace, state }
    }

    pub fn command(&self, home: &Path, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_once"));
        command
            .current_dir(self.workspace.path())
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("TUIST_TOKEN", "fixture-token")
            .args(["--format", "json"])
            .args(arguments)
            .kill_on_drop(true);
        command
    }

    pub async fn invoke(&self, arguments: &[&str]) -> std::process::Output {
        let home = tempfile::tempdir().unwrap();
        self.command(home.path(), arguments).output().await.unwrap()
    }

    pub async fn plan(&self) -> Value {
        let discovery = self.invoke(&["test", "native"]).await;
        assert!(
            discovery.status.success(),
            "{}",
            String::from_utf8_lossy(&discovery.stderr)
        );
        let plan = self
            .invoke(&[
                "query",
                "test-plan",
                "--shards",
                "2",
                "--reference",
                "ci-reference",
            ])
            .await;
        assert!(
            plan.status.success(),
            "{}",
            String::from_utf8_lossy(&plan.stderr)
        );
        serde_json::from_slice(&plan.stdout).unwrap()
    }
}
