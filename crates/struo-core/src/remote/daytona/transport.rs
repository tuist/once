use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;

use super::client::{expect_success, http_error, Client, Sandbox};
use crate::remote::PhasePermit;
use crate::{Error, ExecutionPermit, Result};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ExecuteResponse {
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    stdout: Option<String>,
    #[serde(default)]
    stderr: Option<String>,
    #[serde(default)]
    artifacts: Option<Artifacts>,
}

// Daytona has returned command output in several shapes across toolbox
// versions. Prefer the structured artifact streams, then the dedicated
// stdout/stderr fields, then the combined `result` fallback.
#[derive(Deserialize)]
struct Artifacts {
    #[serde(default)]
    stdout: Option<String>,
    #[serde(default)]
    stderr: Option<String>,
}

impl ExecuteResponse {
    /// The reported process exit code, or an error when the toolbox
    /// omitted it entirely. A missing exit code must never be treated as a
    /// success, otherwise a failed command would be cached as if it passed.
    pub(super) fn exit_code(&self) -> Result<i32> {
        self.exit_code.ok_or_else(|| Error::RemoteProviderApi {
            provider: "daytona".to_string(),
            message: "Daytona response did not include an exit code".to_string(),
        })
    }

    /// Splits the response into stdout and stderr byte streams, preferring
    /// the richest representation the toolbox provided.
    pub(super) fn output_streams(&self) -> (&[u8], &[u8]) {
        let stdout = self
            .artifacts
            .as_ref()
            .and_then(|artifacts| artifacts.stdout.as_deref())
            .or(self.stdout.as_deref())
            .or(self.result.as_deref())
            .unwrap_or_default()
            .as_bytes();
        let stderr = self
            .artifacts
            .as_ref()
            .and_then(|artifacts| artifacts.stderr.as_deref())
            .or(self.stderr.as_deref())
            .unwrap_or_default()
            .as_bytes();
        (stdout, stderr)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteRequest<'a> {
    command: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<&'a str>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    envs: &'a BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<u64>,
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
        let form = Form::new().part(
            "file",
            Part::stream_with_length(body, len).file_name("struo-inputs.tar"),
        );
        let response = progress
            .watch_upload("daytona", self.limits, async {
                self.http
                    .post(format!("{}/files/upload", self.toolbox_base(sandbox)?))
                    .bearer_auth(&self.config.api_key)
                    .query(&[("path", destination)])
                    .multipart(form)
                    .send()
                    .await
                    .map_err(http_error)
            })
            .await?;
        expect_success(response)
    }

    pub(super) async fn execute(
        &self,
        sandbox: &Sandbox,
        command: &str,
        cwd: Option<&str>,
        envs: &BTreeMap<String, String>,
        timeout: Option<u64>,
    ) -> Result<ExecuteResponse> {
        let mut request = self
            .http
            .post(format!("{}/process/execute", self.toolbox_base(sandbox)?))
            .bearer_auth(&self.config.api_key)
            .header("Content-Type", "application/json")
            .body(super::super::http::encode(
                &ExecuteRequest {
                    command,
                    cwd,
                    envs,
                    timeout,
                },
                "daytona",
                self.limits.max_response_bytes,
            )?);
        if let Some(timeout) = timeout {
            request = request.timeout(Duration::from_secs(timeout.saturating_add(5)));
        }
        let response = request.send().await.map_err(http_error)?;
        super::super::http::decode(
            response,
            "daytona",
            self.limits.max_response_bytes,
            self.limits,
        )
        .await
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
                "daytona",
                "download headers",
                self.limits.stall_timeout,
                async {
                    self.http
                        .get(format!("{}/files/download", self.toolbox_base(sandbox)?))
                        .bearer_auth(&self.config.api_key)
                        .query(&[("path", source)])
                        .send()
                        .await
                        .map_err(http_error)
                },
            )
            .await?;
        super::super::http::download(response, destination, "daytona", self.limits, lease, copies)
            .await
    }
}

fn file_error(action: &'static str, source: std::io::Error) -> Error {
    Error::FileAction {
        action,
        path: ".struo/tmp".to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> ExecuteResponse {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn missing_exit_code_is_an_error() {
        let response = parse(r#"{"result":"ok"}"#);
        let error = response.exit_code().unwrap_err();
        assert!(
            matches!(error, Error::RemoteProviderApi { ref provider, .. } if provider == "daytona")
        );
        assert!(error.to_string().contains("exit code"));
    }

    #[test]
    fn streams_prefer_dedicated_fields_over_result() {
        let response = parse(r#"{"exitCode":0,"result":"combined","stdout":"out","stderr":"err"}"#);
        let (stdout, stderr) = response.output_streams();
        assert_eq!(stdout, b"out");
        assert_eq!(stderr, b"err");
    }

    #[test]
    fn streams_prefer_artifacts_over_dedicated_fields() {
        let response = parse(
            r#"{"exitCode":0,"stdout":"out","artifacts":{"stdout":"a-out","stderr":"a-err"}}"#,
        );
        let (stdout, stderr) = response.output_streams();
        assert_eq!(stdout, b"a-out");
        assert_eq!(stderr, b"a-err");
    }

    #[test]
    fn stdout_falls_back_to_result() {
        let response = parse(r#"{"exitCode":0,"result":"combined"}"#);
        let (stdout, stderr) = response.output_streams();
        assert_eq!(stdout, b"combined");
        assert!(stderr.is_empty());
    }
}
