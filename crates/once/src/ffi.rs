use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_uchar};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Command, Once};

#[derive(Debug, Deserialize)]
struct RunCommandRequest {
    workspace_root: PathBuf,
    cache_root: PathBuf,
    argv: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    outputs: Vec<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    cache_failures: bool,
    #[serde(default)]
    stream: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum FfiResponse<T> {
    Ok { value: T },
    Error { message: String },
}

#[derive(Debug, Serialize)]
struct RunCommandResponse {
    action_digest: String,
    cache: &'static str,
    exit_code: i32,
    stdout_digest: Option<String>,
    stderr_digest: Option<String>,
    outputs: BTreeMap<String, String>,
}

static VERSION: &[u8] = concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes();

/// Return the linked Once version as a borrowed nul-terminated string.
#[no_mangle]
pub extern "C" fn once_version() -> *const c_char {
    VERSION.as_ptr().cast()
}

/// Free strings returned by other `once_*` FFI functions.
#[no_mangle]
pub extern "C" fn once_string_free(value: *mut c_char) {
    if value.is_null() {
        return;
    }
    unsafe {
        drop(CString::from_raw(value));
    }
}

/// Compute a BLAKE3 digest for a byte buffer.
#[no_mangle]
pub extern "C" fn once_digest_bytes(data: *const c_uchar, len: usize) -> *mut c_char {
    let Some(bytes) = bytes_from_raw(data, len) else {
        return response_error("data cannot be null when len is non-zero");
    };
    response_ok(once_cas::Digest::of_bytes(bytes).to_hex())
}

/// Compute the Once action digest for a JSON-encoded `once_core::Action`.
#[no_mangle]
pub extern "C" fn once_action_digest_json(action_json: *const c_char) -> *mut c_char {
    let Some(action_json) = str_from_raw(action_json) else {
        return response_error("action_json cannot be null");
    };
    let action = match serde_json::from_str::<once_core::Action>(action_json) {
        Ok(action) => action,
        Err(error) => return response_error(error.to_string()),
    };
    response_ok(action.digest().to_hex())
}

/// Execute a JSON command request through the high-level Once SDK.
#[no_mangle]
pub extern "C" fn once_run_command_json(request_json: *const c_char) -> *mut c_char {
    let Some(request_json) = str_from_raw(request_json) else {
        return response_error("request_json cannot be null");
    };
    let request = match serde_json::from_str::<RunCommandRequest>(request_json) {
        Ok(request) => request,
        Err(error) => return response_error(error.to_string()),
    };
    match run_command(request) {
        Ok(response) => response_ok(response),
        Err(error) => response_error(error),
    }
}

fn run_command(request: RunCommandRequest) -> Result<RunCommandResponse, String> {
    let command = Command::from_argv(request.argv)
        .timeout_ms_opt(request.timeout_ms)
        .cwd_opt(request.cwd)
        .outputs(request.outputs)
        .envs(request.env);
    let once = Once::new(request.workspace_root, request.cache_root)
        .streaming(request.stream)
        .cache_failures(request.cache_failures);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let outcome = runtime
        .block_on(once.run_command(command))
        .map_err(|error| error.to_string())?;
    Ok(RunCommandResponse {
        action_digest: outcome.action_digest.to_hex(),
        cache: match outcome.cache {
            once_core::CacheState::Hit => "hit",
            once_core::CacheState::Miss => "miss",
        },
        exit_code: outcome.exit_code,
        stdout_digest: outcome.result.stdout.map(|digest| digest.to_hex()),
        stderr_digest: outcome.result.stderr.map(|digest| digest.to_hex()),
        outputs: outcome
            .result
            .outputs
            .into_iter()
            .map(|(path, digest)| (path, digest.to_hex()))
            .collect(),
    })
}

trait FfiCommandExt {
    fn cwd_opt(self, path: Option<String>) -> Self;
    fn envs(self, env: BTreeMap<String, String>) -> Self;
    fn outputs(self, outputs: Vec<String>) -> Self;
    fn timeout_ms_opt(self, timeout_ms: Option<u64>) -> Self;
}

impl FfiCommandExt for Command {
    fn cwd_opt(self, path: Option<String>) -> Self {
        match path {
            Some(path) => self.cwd(path),
            None => self,
        }
    }

    fn envs(mut self, env: BTreeMap<String, String>) -> Self {
        for (key, value) in env {
            self = self.env(key, value);
        }
        self
    }

    fn outputs(mut self, outputs: Vec<String>) -> Self {
        for output in outputs {
            self = self.output(output);
        }
        self
    }

    fn timeout_ms_opt(self, timeout_ms: Option<u64>) -> Self {
        match timeout_ms {
            Some(timeout_ms) => self.timeout_ms(timeout_ms),
            None => self,
        }
    }
}

fn response_ok<T: Serialize>(value: T) -> *mut c_char {
    response(&FfiResponse::Ok { value })
}

fn response_error(message: impl Into<String>) -> *mut c_char {
    response(&FfiResponse::<()>::Error {
        message: message.into(),
    })
}

fn response<T: Serialize>(value: &FfiResponse<T>) -> *mut c_char {
    let json = serde_json::to_string(&value)
        .unwrap_or_else(|error| format!(r#"{{"status":"error","message":"{error}"}}"#));
    CString::new(json)
        .expect("JSON response cannot contain interior nul bytes")
        .into_raw()
}

fn str_from_raw(value: *const c_char) -> Option<&'static str> {
    if value.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(value).to_str().ok() }
}

fn bytes_from_raw(data: *const c_uchar, len: usize) -> Option<&'static [u8]> {
    if len == 0 {
        return Some(&[]);
    }
    if data.is_null() {
        return None;
    }
    unsafe { Some(std::slice::from_raw_parts(data, len)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_bytes_returns_json_response() {
        let response = once_digest_bytes(b"hello".as_ptr(), 5);
        let json = unsafe { CStr::from_ptr(response).to_string_lossy().into_owned() };
        once_string_free(response);
        assert!(json.contains(r#""status":"ok""#));
        assert!(json.contains(&once_cas::Digest::of_bytes(b"hello").to_hex()));
    }
}
