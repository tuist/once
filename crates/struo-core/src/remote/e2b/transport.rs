use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use futures::TryStreamExt;
use serde::Serialize;
use struo_cas::{ActionResult, CacheProvider};
use tokio_util::io::ReaderStream;
use tokio_util::io::StreamReader;

#[cfg(test)]
mod tests;

use super::client::{checked, expect_success, http_error, Client, Sandbox};
use super::protocol::{capture_command, collect_process, ProcessResult};
use crate::executor::staging::CapturePermit;
use crate::remote::PhasePermit;
use crate::{Error, ExecutionPermit, Result};

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
        let progress = super::super::http::Progress::new();
        let body = reqwest::Body::wrap_stream(progress.track_bytes(ReaderStream::new(file), len));
        let response = progress
            .watch_upload("e2b", self.limits, async {
                self.sandbox_request(reqwest::Method::POST, sandbox, "/files")
                    .query(&[("path", destination)])
                    .header("Content-Type", "application/octet-stream")
                    .header("Content-Length", len)
                    .body(body)
                    .send()
                    .await
                    .map_err(http_error)
            })
            .await?;
        expect_success(response)
    }

    fn process_request(
        &self,
        sandbox: &Sandbox,
        program: &str,
        args: &[String],
        cwd: Option<&str>,
        envs: &BTreeMap<String, String>,
        timeout_ms: Option<u64>,
    ) -> Result<reqwest::RequestBuilder> {
        let mut request = self
            .sandbox_request(reqwest::Method::POST, sandbox, "/process.Process/Start")
            .header("Connect-Protocol-Version", "1")
            .header("Content-Type", "application/connect+json")
            .body(super::super::http::encode(
                &StartRequest {
                    process: ProcessRequest {
                        cmd: program,
                        args,
                        envs,
                        cwd,
                    },
                    stdin: false,
                },
                "e2b",
                self.limits.max_response_bytes,
            )?);
        if let Some(timeout_ms) = timeout_ms {
            request = request
                .header("Connect-Timeout-Ms", timeout_ms)
                .timeout(Duration::from_millis(timeout_ms.saturating_add(5_000)));
        }
        Ok(request)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn execute_streaming(
        &self,
        sandbox: &Sandbox,
        program: &str,
        args: &[String],
        cwd: Option<&str>,
        envs: &BTreeMap<String, String>,
        timeout_ms: Option<u64>,
        cache: &CacheProvider,
        stream_to_parent: bool,
        budget: Option<CapturePermit>,
    ) -> Result<ActionResult> {
        let response = self
            .process_request(sandbox, program, args, cwd, envs, timeout_ms)?
            .send()
            .await
            .map_err(http_error)?;
        let response = checked(response)?;
        let reader = StreamReader::new(
            response
                .bytes_stream()
                .map_err(|error| std::io::Error::other(http_error(error))),
        );
        capture_command(reader, cache, stream_to_parent, self.limits, budget).await
    }

    pub(super) async fn execute(
        &self,
        sandbox: &Sandbox,
        program: &str,
        args: &[String],
        cwd: Option<&str>,
        envs: &BTreeMap<String, String>,
        timeout_ms: Option<u64>,
    ) -> Result<ProcessResult> {
        let response = self
            .process_request(sandbox, program, args, cwd, envs, timeout_ms)?
            .send()
            .await
            .map_err(http_error)?;
        let reader = StreamReader::new(
            checked(response)?
                .bytes_stream()
                .map_err(std::io::Error::other),
        );
        collect_process(reader, self.limits).await
    }

    pub(super) async fn download(
        &self,
        sandbox: &Sandbox,
        source: &str,
        destination: &Path,
        lease: Option<&ExecutionPermit>,
        copies: u64,
    ) -> Result<PhasePermit> {
        let response = super::super::http::Progress::new()
            .watch(
                "e2b",
                "download headers",
                self.limits.stall_timeout,
                async {
                    self.sandbox_request(reqwest::Method::GET, sandbox, "/files")
                        .query(&[("path", source)])
                        .send()
                        .await
                        .map_err(http_error)
                },
            )
            .await?;
        super::super::http::download(response, destination, "e2b", self.limits, lease, copies).await
    }
}

fn file_error(action: &'static str, source: std::io::Error) -> Error {
    Error::FileAction {
        action,
        path: ".struo/tmp".to_string(),
        source,
    }
}
