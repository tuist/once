//! C ABI boundary for the embeddable Once cache API.
//!
//! The exported functions are safe to call from any host thread. Strings
//! returned from this module are owned by Rust and must be released with
//! `once_string_free`. JSON cache functions share a lazily initialized
//! Tokio runtime; its worker count defaults to 2 and can be overridden
//! with `ONCE_FFI_WORKER_THREADS`. Raw pointer inputs are checked for
//! null before dereferencing. A null byte buffer is accepted only when
//! its length is zero.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_uchar};
use std::path::PathBuf;
use std::sync::OnceLock;

use once_cas::{ActionResult, Digest};
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;

use crate::{digest_from_hex, Cache};

#[derive(Debug, Deserialize)]
struct CacheRootRequest {
    #[serde(alias = "cache_root")]
    local_cache_root: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct BlobPutRequest {
    #[serde(alias = "cache_root")]
    local_cache_root: Option<PathBuf>,
    bytes: Vec<u8>,
}

#[derive(Debug, Deserialize)]
struct DigestRequest {
    #[serde(alias = "cache_root")]
    local_cache_root: Option<PathBuf>,
    digest: String,
}

#[derive(Debug, Deserialize)]
struct ActionResultPutRequest {
    #[serde(alias = "cache_root")]
    local_cache_root: Option<PathBuf>,
    action_digest: String,
    result: ActionResult,
}

#[derive(Debug, Deserialize)]
struct ActionDigestRequest {
    #[serde(alias = "cache_root")]
    local_cache_root: Option<PathBuf>,
    action_digest: String,
}

#[derive(Debug, Serialize)]
struct BlobResponse {
    bytes: Vec<u8>,
}

#[derive(Debug, Serialize)]
struct StatsResponse {
    blob_count: u64,
    blob_bytes: u64,
    action_count: u64,
    action_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum FfiResponse<T> {
    Ok { value: T },
    Error { message: String },
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
const RESPONSE_SERIALIZATION_ERROR: &str =
    r#"{"status":"error","message":"response serialization failed"}"#;
const DEFAULT_WORKER_THREADS: usize = 2;
const WORKER_THREADS_ENV: &str = "ONCE_FFI_WORKER_THREADS";

/// Return the linked Once version.
#[no_mangle]
pub extern "C" fn once_version() -> *mut c_char {
    string_to_raw(env!("CARGO_PKG_VERSION"))
}

/// Free strings returned by `once_*` FFI functions.
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
    if len == 0 {
        return response_ok(Digest::of_bytes(&[]).to_hex());
    }
    if data.is_null() {
        return response_error("data cannot be null when len is non-zero");
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    response_ok(Digest::of_bytes(bytes).to_hex())
}

/// Compute the Once action digest for a JSON-encoded `once_core::Action`.
#[no_mangle]
pub extern "C" fn once_action_digest_json(action_json: *const c_char) -> *mut c_char {
    let action_json = match str_from_raw(action_json, "action_json") {
        Ok(value) => value,
        Err(message) => return response_error(message),
    };
    let action = match serde_json::from_str::<once_core::Action>(&action_json) {
        Ok(action) => action,
        Err(error) => return response_error(error.to_string()),
    };
    response_ok(action.digest().to_hex())
}

/// Store a blob in the local cache.
#[no_mangle]
pub extern "C" fn once_cache_put_blob_json(request_json: *const c_char) -> *mut c_char {
    run_json::<BlobPutRequest, _>(request_json, |request| {
        let cache = cache_from_root(request.local_cache_root);
        block_on(async {
            cache
                .put_blob(&request.bytes)
                .await
                .map(|digest| digest.to_hex())
        })
    })
}

/// Read a blob from the local cache.
#[no_mangle]
pub extern "C" fn once_cache_get_blob_json(request_json: *const c_char) -> *mut c_char {
    run_json::<DigestRequest, _>(request_json, |request| {
        let cache = cache_from_root(request.local_cache_root);
        let digest = digest_from_hex(&request.digest)?;
        block_on(async {
            cache
                .get_blob(digest)
                .await
                .map(|bytes| BlobResponse { bytes })
        })
    })
}

/// Return whether a blob exists in the local cache.
#[no_mangle]
pub extern "C" fn once_cache_has_blob_json(request_json: *const c_char) -> *mut c_char {
    run_json::<DigestRequest, _>(request_json, |request| {
        let cache = cache_from_root(request.local_cache_root);
        let digest = digest_from_hex(&request.digest)?;
        block_on(async { cache.has_blob(digest).await })
    })
}

/// Store an action result in the local cache.
#[no_mangle]
pub extern "C" fn once_cache_put_action_result_json(request_json: *const c_char) -> *mut c_char {
    run_json::<ActionResultPutRequest, _>(request_json, |request| {
        let cache = cache_from_root(request.local_cache_root);
        let action = digest_from_hex(&request.action_digest)?;
        block_on(async {
            cache
                .put_action_result(action, &request.result)
                .await
                .map(|()| true)
        })
    })
}

/// Read an action result from the local cache.
#[no_mangle]
pub extern "C" fn once_cache_get_action_result_json(request_json: *const c_char) -> *mut c_char {
    run_json::<ActionDigestRequest, _>(request_json, |request| {
        let cache = cache_from_root(request.local_cache_root);
        let action = digest_from_hex(&request.action_digest)?;
        block_on(async { cache.get_action_result(action).await })
    })
}

/// Remove an action result from the local cache.
#[no_mangle]
pub extern "C" fn once_cache_forget_action_json(request_json: *const c_char) -> *mut c_char {
    run_json::<ActionDigestRequest, _>(request_json, |request| {
        let cache = cache_from_root(request.local_cache_root);
        let action = digest_from_hex(&request.action_digest)?;
        block_on(async { cache.forget_action(action).await })
    })
}

/// Return local cache statistics.
#[no_mangle]
pub extern "C" fn once_cache_stats_json(request_json: *const c_char) -> *mut c_char {
    run_json::<CacheRootRequest, _>(request_json, |request| {
        let cache = cache_from_root(request.local_cache_root);
        block_on(async {
            cache.stats().await.map(|stats| StatsResponse {
                blob_count: stats.blob_count,
                blob_bytes: stats.blob_bytes,
                action_count: stats.action_count,
                action_bytes: stats.action_bytes,
            })
        })
    })
}

fn cache_from_root(local_cache_root: Option<PathBuf>) -> Cache {
    local_cache_root.map_or_else(Cache::new, Cache::local)
}

fn run_json<Request, Value>(
    request_json: *const c_char,
    operation: impl FnOnce(Request) -> crate::Result<Value>,
) -> *mut c_char
where
    Request: for<'de> Deserialize<'de>,
    Value: Serialize,
{
    let request_json = match str_from_raw(request_json, "request_json") {
        Ok(value) => value,
        Err(message) => return response_error(message),
    };
    let request = match serde_json::from_str::<Request>(&request_json) {
        Ok(request) => request,
        Err(error) => return response_error(error.to_string()),
    };
    match operation(request) {
        Ok(value) => response_ok(value),
        Err(error) => response_error(error.to_string()),
    }
}

fn block_on<T>(work: impl std::future::Future<Output = crate::Result<T>>) -> crate::Result<T> {
    let runtime = if let Some(runtime) = RUNTIME.get() {
        runtime
    } else {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(worker_threads())
            .enable_all()
            .build()
            .map_err(|source| once_cas::Error::Remote {
                provider: "local",
                operation: "runtime",
                message: source.to_string(),
            })?;
        let _ = RUNTIME.set(runtime);
        RUNTIME.get().ok_or_else(|| once_cas::Error::Remote {
            provider: "local",
            operation: "runtime",
            message: "runtime initialization completed without caching a runtime".to_string(),
        })?
    };
    runtime.block_on(work)
}

fn worker_threads() -> usize {
    std::env::var(WORKER_THREADS_ENV)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_WORKER_THREADS)
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
    let json =
        serde_json::to_string(&value).unwrap_or_else(|_| RESPONSE_SERIALIZATION_ERROR.to_string());
    string_to_raw(json)
}

fn string_to_raw(value: impl Into<String>) -> *mut c_char {
    CString::new(value.into().replace('\0', "\\0"))
        .expect("FFI string cannot contain interior nul bytes")
        .into_raw()
}

fn str_from_raw(value: *const c_char, name: &str) -> std::result::Result<String, String> {
    if value.is_null() {
        return Err(format!("{name} cannot be null"));
    }
    unsafe {
        CStr::from_ptr(value)
            .to_str()
            .map_err(|_| format!("{name} must be valid UTF-8"))
            .map(std::borrow::ToOwned::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response_json(response: *mut c_char) -> serde_json::Value {
        let json = unsafe { CStr::from_ptr(response).to_string_lossy().into_owned() };
        once_string_free(response);
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn version_returns_owned_string() {
        let response = once_version();
        let version = unsafe { CStr::from_ptr(response).to_string_lossy().into_owned() };
        once_string_free(response);

        assert_eq!(version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn digest_bytes_returns_json_response() {
        let response = once_digest_bytes(b"hello".as_ptr(), 5);
        let json = response_json(response);

        assert_eq!(json["status"], "ok");
        assert_eq!(json["value"], Digest::of_bytes(b"hello").to_hex());
    }

    #[test]
    fn digest_bytes_rejects_null_pointer_with_non_zero_length() {
        let response = once_digest_bytes(std::ptr::null(), 1);
        let json = response_json(response);

        assert_eq!(json["status"], "error");
        assert_eq!(json["message"], "data cannot be null when len is non-zero");
    }

    #[test]
    fn action_digest_rejects_null_pointer() {
        let response = once_action_digest_json(std::ptr::null());
        let json = response_json(response);

        assert_eq!(json["status"], "error");
        assert_eq!(json["message"], "action_json cannot be null");
    }

    #[test]
    fn cache_request_rejects_invalid_utf8() {
        let request = [0xff, 0x00];
        let response = once_cache_stats_json(request.as_ptr().cast());
        let json = response_json(response);

        assert_eq!(json["status"], "error");
        assert_eq!(json["message"], "request_json must be valid UTF-8");
    }

    #[test]
    fn put_blob_returns_digest() {
        let tmp = tempfile::TempDir::new().unwrap();
        let request = serde_json::json!({
            "local_cache_root": tmp.path().to_string_lossy(),
            "bytes": [104, 101, 108, 108, 111]
        })
        .to_string();

        let response = once_cache_put_blob_json(CString::new(request).unwrap().as_ptr());
        let json = response_json(response);

        assert_eq!(json["status"], "ok");
        assert_eq!(json["value"], Digest::of_bytes(b"hello").to_hex());
    }

    #[test]
    fn cache_request_rejects_malformed_json() {
        let request = CString::new("not json").unwrap();
        let response = once_cache_stats_json(request.as_ptr());
        let json = response_json(response);

        assert_eq!(json["status"], "error");
        assert!(json["message"].as_str().unwrap().contains("expected ident"));
    }

    #[test]
    fn cache_request_rejects_invalid_digest() {
        let tmp = tempfile::TempDir::new().unwrap();
        let request = serde_json::json!({
            "local_cache_root": tmp.path().to_string_lossy(),
            "digest": "not-a-digest"
        })
        .to_string();

        let response = once_cache_get_blob_json(CString::new(request).unwrap().as_ptr());
        let json = response_json(response);

        assert_eq!(json["status"], "error");
        assert_eq!(json["message"], "invalid digest: not-a-digest");
    }
}
