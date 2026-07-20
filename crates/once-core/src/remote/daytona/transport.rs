use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use futures::StreamExt;
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use super::client::{checked, expect_success, http_error, Client, Sandbox};
use crate::{Error, Result};

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
    pub(super) fn output_streams(&self) -> (Vec<u8>, Vec<u8>) {
        let stdout = self
            .artifacts
            .as_ref()
            .and_then(|artifacts| artifacts.stdout.clone())
            .or_else(|| self.stdout.clone())
            .or_else(|| self.result.clone())
            .unwrap_or_default()
            .into_bytes();
        let stderr = self
            .artifacts
            .as_ref()
            .and_then(|artifacts| artifacts.stderr.clone())
            .or_else(|| self.stderr.clone())
            .unwrap_or_default()
            .into_bytes();
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
        let body = reqwest::Body::wrap_stream(ReaderStream::new(file));
        let form = Form::new().part(
            "file",
            Part::stream_with_length(body, len).file_name("once-inputs.tar"),
        );
        let response = self
            .http
            .post(format!("{}/files/upload", self.toolbox_base(sandbox)))
            .bearer_auth(&self.config.api_key)
            .query(&[("path", destination)])
            .multipart(form)
            .send()
            .await
            .map_err(http_error)?;
        expect_success(response).await
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
            .post(format!("{}/process/execute", self.toolbox_base(sandbox)))
            .bearer_auth(&self.config.api_key)
            .json(&ExecuteRequest {
                command,
                cwd,
                envs,
                timeout,
            });
        if let Some(timeout) = timeout {
            request = request.timeout(Duration::from_secs(timeout.saturating_add(5)));
        }
        let response = request.send().await.map_err(http_error)?;
        checked(response).await?.json().await.map_err(http_error)
    }

    pub(super) async fn download(
        &self,
        sandbox: &Sandbox,
        source: &str,
        destination: &Path,
    ) -> Result<()> {
        let response = self
            .http
            .get(format!("{}/files/download", self.toolbox_base(sandbox)))
            .bearer_auth(&self.config.api_key)
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
        let response =
            parse(r#"{"exitCode":0,"stdout":"out","artifacts":{"stdout":"a-out","stderr":"a-err"}}"#);
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
