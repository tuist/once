//! C foreign-function boundary for the embeddable Once cache interface.
//!
//! The exported functions are safe to call from any host thread. Strings
//! returned from this module are owned by Rust and must be released with
//! `once_string_free`. Cache request functions share a lazily initialized
//! Tokio runtime. Raw pointer inputs are checked before dereferencing:
//! a null byte pointer is valid only with length zero, which is treated
//! as an empty byte buffer.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_uchar};
use std::sync::{Mutex, OnceLock};

use once_cas::Digest;
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;

use crate::{digest_from_hex, ActionKeyBuilder, Cache};

mod protocol;

use protocol::{
    ActionDigestRequest, ActionKeyInputRequest, ActionKeyRequest, ActionResultPutRequest,
    BlobFileRequest, BlobPutRequest, BlobResponse, CacheRequest, CacheSelection, DigestRequest,
    FfiResponse, FilePutRequest, StatsResponse,
};

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static RUNTIME_INIT: Mutex<()> = Mutex::new(());
const RESPONSE_SERIALIZATION_ERROR: &str =
    r#"{"status":"error","message":"response serialization failed"}"#;
const DEFAULT_WORKER_THREADS: usize = 2;

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

/// Build a versioned action key from labeled bytes and digests.
#[no_mangle]
pub extern "C" fn once_action_key_json(request_json: *const c_char) -> *mut c_char {
    run_json::<ActionKeyRequest, _>(request_json, |request| {
        let mut builder = ActionKeyBuilder::new(request.namespace);
        for input in request.inputs {
            match input {
                ActionKeyInputRequest::Bytes { label, bytes } => {
                    builder.push_bytes(label, bytes);
                }
                ActionKeyInputRequest::Digest { label, digest } => {
                    builder.push_digest(label, digest_from_hex(&digest)?);
                }
            }
        }
        Ok(builder.finish().to_hex())
    })
}

/// Store a blob in the selected cache.
#[no_mangle]
pub extern "C" fn once_cache_put_blob_json(request_json: *const c_char) -> *mut c_char {
    run_json::<BlobPutRequest, _>(request_json, |request| {
        let cache = cache_from_selection(request.cache)?;
        block_on(async {
            cache
                .put_blob(&request.bytes)
                .await
                .map(|digest| digest.to_hex())
        })
    })
}

/// Store a file in the selected cache without encoding its bytes in the request.
#[no_mangle]
pub extern "C" fn once_cache_put_file_json(request_json: *const c_char) -> *mut c_char {
    run_json::<FilePutRequest, _>(request_json, |request| {
        let cache = cache_from_selection(request.cache)?;
        block_on(async {
            cache
                .put_file(request.path)
                .await
                .map(|digest| digest.to_hex())
        })
    })
}

/// Read a blob from the selected cache.
#[no_mangle]
pub extern "C" fn once_cache_get_blob_json(request_json: *const c_char) -> *mut c_char {
    run_json::<DigestRequest, _>(request_json, |request| {
        let digest = digest_from_hex(&request.digest)?;
        let cache = cache_from_selection(request.cache)?;
        block_on(async {
            cache
                .get_blob(digest)
                .await
                .map(|bytes| BlobResponse { bytes })
        })
    })
}

/// Materialize a blob at a file path without encoding its bytes in the response.
#[no_mangle]
pub extern "C" fn once_cache_get_blob_to_file_json(request_json: *const c_char) -> *mut c_char {
    run_json::<BlobFileRequest, _>(request_json, |request| {
        let digest = digest_from_hex(&request.digest)?;
        let cache = cache_from_selection(request.cache)?;
        block_on(async { cache.get_blob_to_file(digest, request.path).await })
    })
}

/// Return whether a blob exists in the selected cache.
#[no_mangle]
pub extern "C" fn once_cache_has_blob_json(request_json: *const c_char) -> *mut c_char {
    run_json::<DigestRequest, _>(request_json, |request| {
        let digest = digest_from_hex(&request.digest)?;
        let cache = cache_from_selection(request.cache)?;
        block_on(async { cache.has_blob(digest).await })
    })
}

/// Store an action result in the selected cache.
#[no_mangle]
pub extern "C" fn once_cache_put_action_result_json(request_json: *const c_char) -> *mut c_char {
    run_json::<ActionResultPutRequest, _>(request_json, |request| {
        let action = digest_from_hex(&request.action_digest)?;
        let cache = cache_from_selection(request.cache)?;
        block_on(async {
            cache
                .put_action_result(action, &request.result)
                .await
                .map(|()| true)
        })
    })
}

/// Read an action result from the selected cache.
#[no_mangle]
pub extern "C" fn once_cache_get_action_result_json(request_json: *const c_char) -> *mut c_char {
    run_json::<ActionDigestRequest, _>(request_json, |request| {
        let action = digest_from_hex(&request.action_digest)?;
        let cache = cache_from_selection(request.cache)?;
        block_on(async { cache.get_action_result(action).await })
    })
}

/// Remove an action result from the selected cache.
#[no_mangle]
pub extern "C" fn once_cache_forget_action_json(request_json: *const c_char) -> *mut c_char {
    run_json::<ActionDigestRequest, _>(request_json, |request| {
        let action = digest_from_hex(&request.action_digest)?;
        let cache = cache_from_selection(request.cache)?;
        block_on(async { cache.forget_action(action).await })
    })
}

/// Return local cache statistics.
#[no_mangle]
pub extern "C" fn once_cache_stats_json(request_json: *const c_char) -> *mut c_char {
    run_json::<CacheRequest, _>(request_json, |request| {
        let cache = cache_from_selection(request.cache)?;
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

fn cache_from_selection(selection: CacheSelection) -> crate::Result<Cache> {
    match (selection.local_cache_root, selection.workspace_root) {
        (None, None) => Ok(Cache::new()),
        (Some(root), None) => Ok(Cache::local(root)),
        (None, Some(workspace)) => Cache::from_workspace(workspace),
        (Some(_), Some(_)) => Err(once_cas::Error::InvalidConfig {
            provider: "sdk",
            message: "local_cache_root and workspace_root cannot be used together".to_string(),
        }
        .into()),
    }
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
        let _guard = RUNTIME_INIT
            .lock()
            .map_err(|source| once_cas::Error::Remote {
                provider: "local",
                operation: "runtime",
                message: source.to_string(),
            })?;
        if let Some(runtime) = RUNTIME.get() {
            runtime
        } else {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(DEFAULT_WORKER_THREADS)
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
        }
    };
    runtime.block_on(work)
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
    match CString::new(value.into()) {
        Ok(value) => value.into_raw(),
        Err(_) => fallback_response_serialization_error_raw(),
    }
}

fn fallback_response_serialization_error_raw() -> *mut c_char {
    unsafe { CString::from_vec_unchecked(RESPONSE_SERIALIZATION_ERROR.as_bytes().to_vec()) }
        .into_raw()
}

fn str_from_raw(value: *const c_char, name: &str) -> std::result::Result<String, String> {
    if value.is_null() {
        return Err(format!("{name} cannot be null"));
    }
    unsafe {
        CStr::from_ptr(value)
            .to_str()
            .map_err(|_| format!("{name} must use Unicode Transformation Format, 8-bit"))
            .map(std::borrow::ToOwned::to_owned)
    }
}

#[cfg(test)]
mod tests;
