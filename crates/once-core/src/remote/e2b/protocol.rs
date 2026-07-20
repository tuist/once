use base64::Engine;
use serde::Deserialize;

use super::client::api_error;
use crate::Result;

pub(super) struct CommandResponse {
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
    exit_code: i32,
    #[serde(default)]
    error: Option<String>,
}

pub(super) fn decode_command_stream(body: &[u8]) -> Result<CommandResponse> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_code = None;
    if body.first() == Some(&b'{') {
        for line in body
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            decode_event(line, &mut stdout, &mut stderr, &mut exit_code)?;
        }
    } else {
        decode_envelopes(body, &mut stdout, &mut stderr, &mut exit_code)?;
    }
    let exit_code = exit_code
        .ok_or_else(|| api_error("command stream ended without an exit code".to_string()))?;
    Ok(CommandResponse {
        exit_code,
        stdout,
        stderr,
    })
}

fn decode_envelopes(
    body: &[u8],
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    exit_code: &mut Option<i32>,
) -> Result<()> {
    let mut offset = 0;
    while offset < body.len() {
        if body.len() - offset < 5 {
            return Err(api_error("truncated Connect response envelope".to_string()));
        }
        let flags = body[offset];
        let len = u32::from_be_bytes(
            body[offset + 1..offset + 5]
                .try_into()
                .expect("Connect envelope length is four bytes"),
        ) as usize;
        offset += 5;
        if body.len() - offset < len {
            return Err(api_error("truncated Connect response payload".to_string()));
        }
        let payload = &body[offset..offset + len];
        offset += len;
        if flags & 0b10 != 0 {
            decode_end_stream(payload)?;
        } else {
            decode_event(payload, stdout, stderr, exit_code)?;
        }
    }
    Ok(())
}

fn decode_end_stream(payload: &[u8]) -> Result<()> {
    let value: serde_json::Value =
        serde_json::from_slice(payload).map_err(|source| api_error(source.to_string()))?;
    if let Some(error) = value.get("error") {
        return Err(api_error(format!("Connect stream failed: {error}")));
    }
    Ok(())
}

fn decode_event(
    payload: &[u8],
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    exit_code: &mut Option<i32>,
) -> Result<()> {
    let response: StartResponse =
        serde_json::from_slice(payload).map_err(|source| api_error(source.to_string()))?;
    let Some(event) = response.event else {
        return Ok(());
    };
    if let Some(data) = event.data {
        let engine = base64::engine::general_purpose::STANDARD;
        if let Some(value) = data.stdout {
            stdout.extend(
                engine
                    .decode(value)
                    .map_err(|source| api_error(source.to_string()))?,
            );
        }
        if let Some(value) = data.stderr {
            stderr.extend(
                engine
                    .decode(value)
                    .map_err(|source| api_error(source.to_string()))?,
            );
        }
    }
    if let Some(end) = event.end {
        match end.error.filter(|error| !error.is_empty()) {
            // An error string with no usable exit code means the process
            // never ran to completion (for example an exec failure), so
            // surfacing it as a hard error avoids reporting a false success.
            Some(error) if end.exit_code == 0 => return Err(api_error(error)),
            // The process terminated with a real exit code and an
            // accompanying diagnostic (for example a signal). Keep the exit
            // code and preserve the diagnostic on stderr rather than
            // discarding both.
            Some(error) => {
                stderr.extend_from_slice(error.as_bytes());
                if !error.ends_with('\n') {
                    stderr.push(b'\n');
                }
                *exit_code = Some(end.exit_code);
            }
            None => *exit_code = Some(end.exit_code),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(flags: u8, value: &serde_json::Value) -> Vec<u8> {
        let payload = serde_json::to_vec(value).unwrap();
        let mut result = vec![flags];
        result.extend(u32::try_from(payload.len()).unwrap().to_be_bytes());
        result.extend(payload);
        result
    }

    #[test]
    fn decodes_connect_command_stream() {
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

        let result = decode_command_stream(&body).unwrap();

        assert_eq!(result.exit_code, 7);
        assert_eq!(result.stdout, b"hello");
        assert_eq!(result.stderr, b"warning");
    }

    #[test]
    fn end_error_with_exit_code_preserves_output() {
        let mut body = envelope(
            0,
            &serde_json::json!({"event":{"data":{"stdout":"aGVsbG8="}}}),
        );
        body.extend(envelope(
            0,
            &serde_json::json!({"event":{"end":{"exitCode":137,"error":"killed by signal"}}}),
        ));
        body.extend(envelope(2, &serde_json::json!({})));

        let result = decode_command_stream(&body).unwrap();

        assert_eq!(result.exit_code, 137);
        assert_eq!(result.stdout, b"hello");
        assert_eq!(result.stderr, b"killed by signal\n");
    }

    #[test]
    fn end_error_without_exit_code_is_a_hard_error() {
        let mut body = envelope(
            0,
            &serde_json::json!({"event":{"end":{"exitCode":0,"error":"exec: node not found"}}}),
        );
        body.extend(envelope(2, &serde_json::json!({})));

        let error = decode_command_stream(&body).map(|_| ()).unwrap_err();

        assert!(error.to_string().contains("node not found"));
    }
}
