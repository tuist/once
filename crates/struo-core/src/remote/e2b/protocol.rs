use base64::Engine;
use serde::Deserialize;

use super::client::api_error;
use crate::Result;

mod frames;
mod process;
mod stream;
pub(super) use process::collect_process;
pub(super) use stream::capture_command;

/// The exit status and output of a process whose output stays small.
pub(super) struct ProcessResult {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Deserialize)]
struct StartResponse {
    event: Option<ProcessEvent>,
}

#[derive(Deserialize)]
struct ProcessEvent {
    data: Option<DataEvent>,
    end: Option<EndEvent>,
}

#[derive(Deserialize)]
struct DataEvent {
    stdout: Option<String>,
    stderr: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EndEvent {
    /// Protobuf JSON omits this field when the process exited with 0.
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    exited: bool,
    #[serde(default)]
    error: Option<String>,
}

impl EndEvent {
    /// The reported exit status. A missing code means 0 only when the event
    /// also says the process exited; otherwise completion is unproven.
    fn exit_status(&self) -> Result<i32> {
        match (self.exit_code, self.exited) {
            (Some(code), _) => Ok(code),
            (None, true) => Ok(0),
            (None, false) => Err(api_error(
                "command completion did not report an exit status".to_string(),
            )),
        }
    }

    fn diagnostic(&self) -> Option<&str> {
        self.error.as_deref().filter(|error| !error.is_empty())
    }
}

fn decode_end_stream(payload: &[u8]) -> Result<()> {
    if payload.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }
    let value: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|_| api_error("invalid Connect end-of-stream message".to_string()))?;
    if value.get("error").is_some_and(|error| !error.is_null()) {
        return Err(api_error(
            "Connect stream reported a protocol failure".to_string(),
        ));
    }
    Ok(())
}

fn decode_event(
    payload: &[u8],
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    exit_code: &mut Option<i32>,
) -> Result<()> {
    let response: StartResponse = serde_json::from_slice(payload)
        .map_err(|_| api_error("invalid command event".to_string()))?;
    let Some(event) = response.event else {
        return Ok(());
    };
    if let Some(data) = event.data {
        let engine = base64::engine::general_purpose::STANDARD;
        if let Some(value) = data.stdout {
            stdout.extend(
                engine
                    .decode(value)
                    .map_err(|_| api_error("invalid command output encoding".to_string()))?,
            );
        }
        if let Some(value) = data.stderr {
            stderr.extend(
                engine
                    .decode(value)
                    .map_err(|_| api_error("invalid command output encoding".to_string()))?,
            );
        }
    }
    if let Some(end) = event.end {
        if exit_code.is_some() {
            return Err(api_error("duplicate command completion".to_string()));
        }
        // An error string with no usable exit code means the process
        // never ran to completion (for example an exec failure), so
        // surfacing it as a hard error avoids reporting a false success.
        let status = match (end.exit_status(), end.diagnostic()) {
            (Ok(0) | Err(_), Some(_)) => {
                return Err(api_error(
                    "command failed before reporting a usable exit code".to_string(),
                ))
            }
            (status, _) => status?,
        };
        // The process terminated with a real exit code and an accompanying
        // diagnostic (for example a signal). Keep both.
        if let Some(error) = end.diagnostic() {
            stderr.extend_from_slice(error.as_bytes());
            if !error.ends_with('\n') {
                stderr.push(b'\n');
            }
        }
        *exit_code = Some(status);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RemoteIoLimits;

    async fn decode(body: Vec<u8>) -> Result<ProcessResult> {
        collect_process(std::io::Cursor::new(body), RemoteIoLimits::default()).await
    }

    fn envelope(flags: u8, value: &serde_json::Value) -> Vec<u8> {
        let payload = serde_json::to_vec(value).unwrap();
        let mut result = vec![flags];
        result.extend(u32::try_from(payload.len()).unwrap().to_be_bytes());
        result.extend(payload);
        result
    }

    #[tokio::test]
    async fn decodes_connect_command_stream() {
        let mut body = envelope(
            0,
            &serde_json::json!({"event":{"data":{"stdout":"aGVsbG8="}}}),
        );
        body.extend(envelope(
            0,
            &serde_json::json!({"event":{"data":{"stderr":"d2FybmluZw=="}}}),
        ));
        body.extend(envelope(
            0,
            &serde_json::json!({"event":{"end":{"exitCode":7,"exited":true}}}),
        ));
        body.extend(envelope(2, &serde_json::json!({})));

        let result = decode(body.clone()).await.unwrap();

        assert_eq!(result.exit_code, 7);
        assert_eq!(result.stdout, b"hello");
        assert_eq!(result.stderr, b"warning");
    }

    #[tokio::test]
    async fn end_error_with_exit_code_preserves_output() {
        let mut body = envelope(
            0,
            &serde_json::json!({"event":{"data":{"stdout":"aGVsbG8="}}}),
        );
        body.extend(envelope(
            0,
            &serde_json::json!({"event":{"end":{"exitCode":137,"error":"killed by signal"}}}),
        ));
        body.extend(envelope(2, &serde_json::json!({})));

        let result = decode(body.clone()).await.unwrap();

        assert_eq!(result.exit_code, 137);
        assert_eq!(result.stdout, b"hello");
        assert_eq!(result.stderr, b"killed by signal\n");
    }

    #[tokio::test]
    async fn an_omitted_zero_exit_code_is_success_only_when_the_process_exited() {
        // Protobuf JSON leaves out a zero exit code on success.
        let mut body = envelope(
            0,
            &serde_json::json!({"event":{"end":{"exited":true,"status":"exit status 0"}}}),
        );
        body.extend(envelope(2, &serde_json::json!({})));
        assert_eq!(decode(body.clone()).await.unwrap().exit_code, 0);

        let mut unproven = envelope(
            0,
            &serde_json::json!({"event":{"end":{"status":"unknown"}}}),
        );
        unproven.extend(envelope(2, &serde_json::json!({})));
        let error = decode(unproven).await.map(|_| ()).unwrap_err();
        assert!(error.to_string().contains("did not report an exit status"));
    }

    #[tokio::test]
    async fn a_clean_trailer_may_be_empty_or_carry_a_null_error() {
        for trailer in [Vec::new(), b"{\"error\":null}".to_vec()] {
            let mut body = envelope(
                0,
                &serde_json::json!({"event":{"end":{"exitCode":3,"exited":true}}}),
            );
            body.push(2);
            body.extend(u32::try_from(trailer.len()).unwrap().to_be_bytes());
            body.extend(trailer);
            assert_eq!(decode(body.clone()).await.unwrap().exit_code, 3);
        }
    }

    #[tokio::test]
    async fn end_error_without_exit_code_is_a_hard_error() {
        let mut body = envelope(
            0,
            &serde_json::json!({"event":{"end":{"exitCode":0,"error":"exec: node not found"}}}),
        );
        body.extend(envelope(2, &serde_json::json!({})));

        let error = decode(body.clone()).await.map(|_| ()).unwrap_err();

        // The provider's diagnostic text never reaches the error.
        assert!(error.to_string().contains("usable exit code"));
        assert!(!error.to_string().contains("node not found"));
    }
}
