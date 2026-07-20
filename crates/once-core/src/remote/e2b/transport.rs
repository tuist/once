use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use futures::StreamExt;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use super::client::{checked, expect_success, http_error, Client, Sandbox};
use super::protocol::{decode_command_stream, CommandResponse};
use crate::{Error, Result};

#[derive(Serialize)]
struct StartRequest<'a> {
    process: ProcessRequest<'a>,
    stdin: bool,
}

#[derive(Serialize)]
struct ProcessRequest<'a> {
    cmd: &'a str,
    args: &'a [String],
    envs: &'a BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<&'a str>,
}

impl Client {
    pub(super) async fn upload(
        &self,
        sandbox: &Sandbox,
        source: &Path,
        len: u64,
        destination: &str,
    ) -> Result<()> {
        let file = tokio::fs::File::open(source)
            .await
            .map_err(|source| file_error("open_remote_input_archive", source))?;
        let body = reqwest::Body::wrap_stream(ReaderStream::new(file));
        let response = self
            .sandbox_request(reqwest::Method::POST, sandbox, "/files")
            .query(&[("path", destination)])
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", len)
            .body(body)
            .send()
            .await
            .map_err(http_error)?;
        expect_success(response).await
    }

    pub(super) async fn execute(
        &self,
        sandbox: &Sandbox,
        program: &str,
        args: &[String],
        cwd: Option<&str>,
        envs: &BTreeMap<String, String>,
        timeout_ms: Option<u64>,
    ) -> Result<CommandResponse> {
        let mut request = self
            .sandbox_request(reqwest::Method::POST, sandbox, "/process.Process/Start")
            .header("Connect-Protocol-Version", "1")
            .header("Content-Type", "application/connect+json")
            .json(&StartRequest {
                process: ProcessRequest {
                    cmd: program,
                    args,
                    envs,
                    cwd,
                },
                stdin: false,
            });
        if let Some(timeout_ms) = timeout_ms {
            request = request
                .header("Connect-Timeout-Ms", timeout_ms)
                .timeout(Duration::from_millis(timeout_ms.saturating_add(5_000)));
        }
        let response = request.send().await.map_err(http_error)?;
        let body = checked(response).await?.bytes().await.map_err(http_error)?;
        decode_command_stream(&body)
    }

    pub(super) async fn download(
        &self,
        sandbox: &Sandbox,
        source: &str,
        destination: &Path,
    ) -> Result<()> {
        let response = self
            .sandbox_request(reqwest::Method::GET, sandbox, "/files")
            .query(&[("path", source)])
            .send()
            .await
            .map_err(http_error)?;
        let response = checked(response).await?;
        let mut file = tokio::fs::File::create(destination)
            .await
            .map_err(|source| file_error("create_remote_output_archive", source))?;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            file.write_all(&chunk.map_err(http_error)?)
                .await
                .map_err(|source| file_error("write_remote_output_archive", source))?;
        }
        file.flush()
            .await
            .map_err(|source| file_error("flush_remote_output_archive", source))
    }
}

fn file_error(action: &'static str, source: std::io::Error) -> Error {
    Error::FileAction {
        action,
        path: ".once/tmp".to_string(),
        source,
    }
}
