use super::endpoints::{authorized_grpc_request_with_token, cache_control_max_age};
use super::*;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn tuist_cache(temp: &TempDir, project: Option<&str>) -> TuistCache {
    TuistCache::new(
        Cas::open(temp.path().join("cas")),
        temp.path().join("auth"),
        TuistCacheConfig {
            url: "https://tuist.dev".to_string(),
            account: Some("tuist".to_string()),
            project: project.map(str::to_string),
            oauth_client_id: None,
            provider_name: "tuist".to_string(),
        },
    )
    .unwrap()
}

/// Answers each connection with the next response, counting requests.
async fn serve_discovery(responses: Vec<String>) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = requests.clone();
    tokio::spawn(async move {
        for response in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            stream.write_all(response.as_bytes()).await.unwrap();
            let _ = stream.shutdown().await;
        }
    });
    (format!("http://{address}"), requests)
}

fn endpoints_response(endpoint: &str, cache_control: &str) -> String {
    let body = format!(r#"{{"endpoints":["{endpoint}"]}}"#);
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nCache-Control: {cache_control}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn status_response(status: &str) -> String {
    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
}

/// A cache as a fresh Struo process would open it on `temp`.
fn discovery_cache(temp: &TempDir, url: &str) -> TuistCache {
    let mut cache = tuist_cache(temp, Some("struo"));
    cache.config.url = url.to_string();
    cache.auth_token_cache.set("test-token".into());
    cache
}

#[tokio::test]
async fn discovered_endpoints_are_reused_by_later_processes() {
    let temp = TempDir::new().unwrap();
    let (url, requests) = serve_discovery(vec![endpoints_response(
        "https://cache.example.test",
        "max-age=300",
    )])
    .await;

    let first = discovery_cache(&temp, &url)
        .fetch_endpoints()
        .await
        .unwrap();
    let second = discovery_cache(&temp, &url)
        .fetch_endpoints()
        .await
        .unwrap();

    assert_eq!(first, ["https://cache.example.test"]);
    assert_eq!(second, first);
    assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn expired_endpoints_are_discovered_again() {
    let temp = TempDir::new().unwrap();
    let (url, requests) = serve_discovery(vec![
        endpoints_response("https://old.example.test", "max-age=0"),
        endpoints_response("https://new.example.test", "max-age=300"),
    ])
    .await;

    discovery_cache(&temp, &url)
        .fetch_endpoints()
        .await
        .unwrap();
    let refreshed = discovery_cache(&temp, &url)
        .fetch_endpoints()
        .await
        .unwrap();

    assert_eq!(refreshed, ["https://new.example.test"]);
    assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn unreachable_discovery_falls_back_to_the_last_known_endpoints() {
    let temp = TempDir::new().unwrap();
    let (url, _) = serve_discovery(vec![
        endpoints_response("https://cache.example.test", "max-age=0"),
        status_response("503 Service Unavailable"),
    ])
    .await;

    discovery_cache(&temp, &url)
        .fetch_endpoints()
        .await
        .unwrap();
    let endpoints = discovery_cache(&temp, &url)
        .fetch_endpoints()
        .await
        .unwrap();

    assert_eq!(endpoints, ["https://cache.example.test"]);
}

#[tokio::test]
async fn unreachable_discovery_without_a_known_endpoint_is_a_read_miss() {
    let temp = TempDir::new().unwrap();
    let (url, _) = serve_discovery(vec![status_response("503 Service Unavailable")]).await;

    let error = discovery_cache(&temp, &url)
        .fetch_endpoints()
        .await
        .unwrap_err();

    assert!(error.is_unavailable(), "{error}");
}

#[tokio::test]
async fn interrupted_discovery_body_without_known_endpoints_is_a_read_miss() {
    let temp = TempDir::new().unwrap();
    let (url, _) = serve_discovery(vec![
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n".to_string(),
    ])
    .await;

    let error = discovery_cache(&temp, &url)
        .fetch_endpoints()
        .await
        .unwrap_err();

    assert!(error.is_unavailable(), "{error}");
}

#[tokio::test]
async fn interrupted_discovery_body_reuses_stale_endpoints() {
    let temp = TempDir::new().unwrap();
    let (url, _) = serve_discovery(vec![
        endpoints_response("https://cache.example.test", "max-age=0"),
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n".to_string(),
    ])
    .await;

    discovery_cache(&temp, &url)
        .fetch_endpoints()
        .await
        .unwrap();
    let endpoints = discovery_cache(&temp, &url)
        .fetch_endpoints()
        .await
        .unwrap();

    assert_eq!(endpoints, ["https://cache.example.test"]);
}

#[tokio::test]
async fn malformed_discovery_body_stays_fatal() {
    let temp = TempDir::new().unwrap();
    let (url, _) = serve_discovery(vec![
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]".to_string(),
    ])
    .await;

    let error = discovery_cache(&temp, &url)
        .fetch_endpoints()
        .await
        .unwrap_err();

    assert!(!error.is_unavailable(), "{error}");
}

#[tokio::test]
async fn refused_discovery_stays_fatal() {
    let temp = TempDir::new().unwrap();
    let (url, _) = serve_discovery(vec![status_response("403 Forbidden")]).await;

    let error = discovery_cache(&temp, &url)
        .fetch_endpoints()
        .await
        .unwrap_err();

    assert!(!error.is_unavailable(), "{error}");
}

#[test]
fn cache_control_max_age_ignores_other_directives() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::CACHE_CONTROL,
        "private, MAX-AGE=42, must-revalidate".parse().unwrap(),
    );
    assert_eq!(
        cache_control_max_age(&headers),
        Some(Duration::from_secs(42))
    );
    assert_eq!(
        cache_control_max_age(&reqwest::header::HeaderMap::new()),
        None
    );
}

#[tokio::test]
async fn data_plane_endpoint_prefers_the_override_over_discovery() {
    let temp = TempDir::new().unwrap();
    let mut cache = tuist_cache(&temp, Some("struo"));
    cache.config.url = "http://127.0.0.1:9".to_string();
    cache.endpoint_override = Some("http://kura.kura.svc.cluster.local:4000".to_string());

    let endpoint = cache.data_plane_endpoint().await.unwrap();

    assert_eq!(endpoint, "http://kura.kura.svc.cluster.local:4000");
}

#[test]
fn grpc_endpoint_url_normalizes_grpc_schemes() {
    assert_eq!(
        grpc_endpoint_url("grpc://localhost:5101"),
        "http://localhost:5101"
    );
    assert_eq!(
        grpc_endpoint_url("grpcs://cache.example.com"),
        "https://cache.example.com"
    );
    assert_eq!(
        grpc_endpoint_url("https://cache.example.com"),
        "https://cache.example.com"
    );
}

#[test]
fn instance_name_uses_project_namespace_without_account_prefix() {
    let temp = TempDir::new().unwrap();
    let cache = tuist_cache(&temp, Some("gradle-plugin"));

    assert_eq!(cache.instance_name(), "gradle-plugin");
}

#[test]
fn instance_name_defaults_to_default_namespace() {
    let temp = TempDir::new().unwrap();
    let cache = tuist_cache(&temp, None);

    assert_eq!(cache.instance_name(), "default");
}

#[tokio::test]
async fn empty_blob_is_available_without_remote_storage() {
    let temp = TempDir::new().unwrap();
    let cache = tuist_cache(&temp, None);
    let digest = Digest::of_bytes(&[]);

    assert!(!cache.local().has_blob(&digest).await.unwrap());
    assert!(cache.has_blob(&digest).await.unwrap());
    assert!(cache.get_blob(&digest).await.unwrap().is_empty());
    assert!(cache.local().has_blob(&digest).await.unwrap());
}

#[tokio::test]
async fn empty_blob_materializes_without_remote_storage() {
    let temp = TempDir::new().unwrap();
    let cache = tuist_cache(&temp, None);
    let digest = Digest::of_bytes(&[]);

    cache.ensure_blob_local(&digest).await.unwrap();

    assert!(cache.local().get_blob(&digest).await.unwrap().is_empty());
}

#[test]
fn authorized_grpc_request_includes_tuist_account_header() {
    let request =
        authorized_grpc_request_with_token((), "token", Some("acme")).expect("valid metadata");

    assert_eq!(
        request
            .metadata()
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        Some("Bearer token")
    );
    assert_eq!(
        request
            .metadata()
            .get("x-tuist-account-handle")
            .and_then(|value| value.to_str().ok()),
        Some("acme")
    );
}

#[test]
fn sha256_digest_matches_stable_vectors() {
    let cases = [
        (
            b"".as_slice(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            0,
        ),
        (
            b"hello".as_slice(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
            5,
        ),
    ];

    for (bytes, hash, size_bytes) in cases {
        let digest = sha256_digest(bytes).unwrap();
        assert_eq!(digest.hash, hash);
        assert_eq!(digest.size_bytes, size_bytes);
    }
}

#[test]
fn file_digests_match_in_memory_digests() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("blob");
    let bytes = b"streamed remote cache body";
    std::fs::write(&path, bytes).unwrap();

    let mut file = std::fs::File::open(&path).unwrap();
    let mut copied = Vec::new();
    let (once, remote) = copy_blob_digests(&mut file, &mut copied).unwrap();
    assert_eq!(copied, bytes);

    assert_eq!(once, Digest::of_bytes(bytes));
    assert_eq!(remote, sha256_digest(bytes).unwrap());
}

#[test]
fn size_bytes_rejects_values_that_do_not_fit_reapi_digest() {
    if i64::try_from(usize::MAX).is_ok() {
        return;
    }

    let error = size_bytes_i64(usize::MAX).unwrap_err();
    assert!(error.to_string().contains("REAPI size limit"));
}

#[test]
fn blob_and_action_mapping_digests_are_distinct() {
    let digest = Digest::of_bytes(b"payload");
    assert_ne!(
        blob_mapping_digest(&digest).unwrap(),
        action_mapping_digest(&digest).unwrap()
    );
}

#[test]
fn byte_stream_resources_include_instance_digest_and_upload_id() {
    let digest = sha256_digest(b"payload").unwrap();
    let upload_id = Uuid::parse_str("018f22d2-6e6e-7e63-a6cc-06c3d9ca8f22").unwrap();

    assert_eq!(
        byte_stream_read_resource(
            "project",
            &digest,
            reapi::compressor::Value::Identity as i32
        ),
        format!("project/blobs/{}", digest_key(&digest))
    );
    assert_eq!(
        byte_stream_write_resource(
            "project",
            &digest,
            upload_id,
            reapi::compressor::Value::Identity as i32
        ),
        format!("project/uploads/{upload_id}/blobs/{}", digest_key(&digest))
    );
    assert_eq!(
        byte_stream_read_resource("", &digest, reapi::compressor::Value::Zstd as i32),
        format!("compressed-blobs/zstd/{}", digest_key(&digest))
    );
}

#[test]
fn zstd_blob_transfer_roundtrips_and_preserves_digest() {
    let bytes = vec![b'a'; BATCH_BLOB_LIMIT + 1];
    let digest = sha256_digest(&bytes).unwrap();
    let (encoded, compressor) = encode_reapi_blob(&bytes, true, "test").unwrap();

    assert_eq!(compressor, reapi::compressor::Value::Zstd as i32);
    assert!(encoded.len() < bytes.len());
    assert_eq!(
        decode_reapi_blob(encoded, compressor, &digest, "test").unwrap(),
        bytes
    );
}

#[test]
fn native_action_result_metadata_roundtrips_without_output_bodies() {
    let output = Digest::of_bytes(b"output");
    let stdout = Digest::of_bytes(b"stdout");
    let native = ActionResult {
        exit_code: 0,
        stdout: Some(stdout),
        stderr: None,
        outputs: BTreeMap::from([("out/result".to_string(), output)]),
    };
    let remote = reapi::ActionResult {
        execution_metadata: Some(reapi::ExecutedActionMetadata {
            auxiliary_metadata: vec![encode_action_result_metadata(&native).unwrap()],
            ..Default::default()
        }),
        ..Default::default()
    };

    assert_eq!(
        decode_action_result_metadata(&remote).unwrap(),
        Some(native)
    );
}
