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
fn action_key_matches_rust_builder() {
    let request = serde_json::json!({
        "namespace": "compile",
        "inputs": [
            {"kind": "bytes", "label": "tool", "bytes": [115, 119, 105, 102, 116, 99]},
            {"kind": "digest", "label": "source", "digest": Digest::of_bytes(b"source").to_hex()}
        ]
    })
    .to_string();
    let response = once_action_key_json(CString::new(request).unwrap().as_ptr());
    let json = response_json(response);
    let mut expected = ActionKeyBuilder::new("compile");
    expected
        .push_bytes("tool", "swiftc")
        .push_digest("source", Digest::of_bytes(b"source"));

    assert_eq!(json["status"], "ok");
    assert_eq!(json["value"], expected.finish().to_hex());
}

#[test]
fn cache_request_rejects_invalid_utf8() {
    let request = [0xff, 0x00];
    let response = once_cache_put_blob_json(request.as_ptr().cast());
    let json = response_json(response);

    assert_eq!(json["status"], "error");
    assert_eq!(
        json["message"],
        "request_json must use Unicode Transformation Format, 8-bit"
    );
}

#[test]
fn raw_strings_with_interior_null_bytes_return_error_json() {
    let response = string_to_raw("before\0after");
    let value = unsafe { CStr::from_ptr(response).to_string_lossy().into_owned() };
    once_string_free(response);

    assert_eq!(value, RESPONSE_SERIALIZATION_ERROR);
}

#[test]
fn put_blob_returns_digest() {
    let _env_lock = crate::TEST_ENV_LOCK.lock().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    std::env::set_var("XDG_CACHE_HOME", tmp.path());
    let request = serde_json::json!({
        "bytes": [104, 101, 108, 108, 111]
    })
    .to_string();

    let response = once_cache_put_blob_json(CString::new(request).unwrap().as_ptr());
    let json = response_json(response);

    assert_eq!(json["status"], "ok");
    assert_eq!(json["value"], Digest::of_bytes(b"hello").to_hex());
    std::env::remove_var("XDG_CACHE_HOME");
}

#[test]
fn explicit_root_and_file_operations_roundtrip() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cache_root = tmp.path().join("cache");
    let input = tmp.path().join("input.bin");
    let output = tmp.path().join("nested/output.bin");
    std::fs::write(&input, b"file payload").unwrap();
    let put_request = serde_json::json!({
        "local_cache_root": cache_root,
        "path": input
    })
    .to_string();

    let put = response_json(once_cache_put_file_json(
        CString::new(put_request).unwrap().as_ptr(),
    ));
    let get_request = serde_json::json!({
        "local_cache_root": tmp.path().join("cache"),
        "digest": put["value"],
        "path": output
    })
    .to_string();
    let get = response_json(once_cache_get_blob_to_file_json(
        CString::new(get_request).unwrap().as_ptr(),
    ));

    assert_eq!(put["status"], "ok");
    assert_eq!(get["status"], "ok");
    assert_eq!(get["value"], 12);
    assert_eq!(
        std::fs::read(tmp.path().join("nested/output.bin")).unwrap(),
        b"file payload"
    );
}

#[test]
fn cache_selection_rejects_two_roots() {
    let request = serde_json::json!({
        "local_cache_root": "/tmp/local",
        "workspace_root": "/tmp/workspace",
        "bytes": []
    })
    .to_string();

    let response = response_json(once_cache_put_blob_json(
        CString::new(request).unwrap().as_ptr(),
    ));

    assert_eq!(response["status"], "error");
    assert!(response["message"]
        .as_str()
        .unwrap()
        .contains("cannot be used together"));
}

#[test]
fn cache_request_rejects_malformed_json() {
    let request = CString::new("not json").unwrap();
    let response = once_cache_put_blob_json(request.as_ptr());
    let json = response_json(response);

    assert_eq!(json["status"], "error");
    assert!(!json["message"].as_str().unwrap().is_empty());
}

#[test]
fn cache_request_rejects_invalid_digest() {
    let request = serde_json::json!({
        "digest": "not-a-digest"
    })
    .to_string();

    let response = once_cache_get_blob_json(CString::new(request).unwrap().as_ptr());
    let json = response_json(response);

    assert_eq!(json["status"], "error");
    assert_eq!(json["message"], "invalid digest: not-a-digest");
}

#[test]
fn action_result_request_rejects_invalid_digest() {
    let request = serde_json::json!({
        "action_digest": "not-a-digest",
        "result": {
            "exit_code": 0,
            "stdout": null,
            "stderr": null,
            "outputs": {}
        }
    })
    .to_string();

    let response = once_cache_put_action_result_json(CString::new(request).unwrap().as_ptr());
    let json = response_json(response);

    assert_eq!(json["status"], "error");
    assert_eq!(json["message"], "invalid digest: not-a-digest");
}

#[test]
fn cache_calls_can_initialize_runtime_concurrently() {
    let _env_lock = crate::TEST_ENV_LOCK.lock().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    std::env::set_var("XDG_CACHE_HOME", tmp.path());
    let request = CString::new(r#"{"bytes":[104,101,108,108,111]}"#).unwrap();

    std::thread::scope(|scope| {
        for _ in 0..8 {
            let request = &request;
            scope.spawn(move || {
                let json = response_json(once_cache_put_blob_json(request.as_ptr()));
                assert_eq!(json["status"], "ok");
                assert_eq!(json["value"], Digest::of_bytes(b"hello").to_hex());
            });
        }
    });

    std::env::remove_var("XDG_CACHE_HOME");
}
