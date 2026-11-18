//! Credentials in endpoint URLs and anything a server sends back never reach
//! an error or a log line, while failures keep their categories.

use super::fault_server::{connected_cache, FaultServer};
use super::*;
use crate::tuist::log_capture;
use crate::tuist::redact::SENTINEL;

fn assert_clean(error: &Error, logs: &str) {
    assert!(!error.to_string().contains(SENTINEL), "error: {error}");
    assert!(!format!("{error:?}").contains(SENTINEL), "error: {error:?}");
    assert!(!logs.contains(SENTINEL), "logs: {logs}");
}

/// Answers each connection with the next canned HTTP response.
async fn http_responses(responses: Vec<String>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for response in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let _ = socket.read(&mut request).await.unwrap();
            socket.write_all(response.as_bytes()).await.unwrap();
            let _ = socket.shutdown().await;
        }
    });
    format!("http://{address}")
}

fn http(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

async fn discovering_cache(temp: &tempfile::TempDir, discovery: &str) -> TuistCache {
    let mut cache =
        connected_cache(temp, "discovering", &FaultServer::default().serve().await).await;
    cache.grpc_channel_cache = Arc::default();
    cache.endpoint_override = None;
    cache.config.url = discovery.to_string();
    cache
}

#[tokio::test]
async fn server_status_messages_are_not_repeated_but_categories_are_kept() {
    for (status, kind) in [
        (Status::unavailable(SENTINEL), ErrorKind::Unavailable),
        (Status::permission_denied(SENTINEL), ErrorKind::Refused),
        (Status::invalid_argument(SENTINEL), ErrorKind::Protocol),
    ] {
        let temp = tempfile::TempDir::new().unwrap();
        let server = FaultServer::default();
        let url = server.serve().await;
        let cache = connected_cache(&temp, "reader", &url).await;
        server.fail("get_action", vec![status.clone(); 5]).await;
        let (logs, _guard) = log_capture::capture();

        let error = cache
            .get_action_result(&Digest::of_bytes(b"action"))
            .await
            .unwrap_err();

        assert_eq!(error.kind(), kind);
        assert!(error.to_string().contains("gRPC"), "{error}");
        assert_clean(&error, &logs.text());
    }
}

#[tokio::test]
async fn batch_item_status_messages_are_not_repeated() {
    let rpc = bazel_remote_apis::google::rpc::Status {
        code: Code::PermissionDenied as i32,
        message: SENTINEL.into(),
        details: vec![],
    };
    let error = status::rpc_status_error("put blob", &rpc);
    assert_eq!(error.kind(), ErrorKind::Refused);
    assert_clean(&error, "");
}

#[tokio::test]
async fn endpoint_credentials_are_not_logged_or_reported() {
    let temp = tempfile::TempDir::new().unwrap();
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    let mut cache = connected_cache(&temp, "override", &FaultServer::default().serve().await).await;
    cache.grpc_channel_cache = Arc::default();
    cache.endpoint_override = Some(format!(
        "http://user:{SENTINEL}@{address}/{SENTINEL}?token={SENTINEL}"
    ));
    let (logs, _guard) = log_capture::capture();

    let error = cache
        .has_blob(&Digest::of_bytes(b"probe"))
        .await
        .unwrap_err();

    assert!(error.is_unavailable(), "{error}");
    assert!(error.to_string().contains(&address.to_string()), "{error}");
    assert_clean(&error, &logs.text());
}

#[tokio::test]
async fn discovery_bodies_and_endpoint_candidates_are_not_repeated() {
    let unreachable = |port: u16| format!("http://user:{SENTINEL}@127.0.0.1:{port}/?k={SENTINEL}");
    let candidates = format!(
        r#"{{"endpoints":["{}","{}"]}}"#,
        unreachable(1),
        unreachable(2)
    );
    for (response, kind) in [
        (http("403 Forbidden", SENTINEL), ErrorKind::Refused),
        (
            http("503 Service Unavailable", SENTINEL),
            ErrorKind::Unavailable,
        ),
        (
            http("200 OK", &format!(r#"{{"endpoints":"{SENTINEL}"}}"#)),
            ErrorKind::Protocol,
        ),
        (http("200 OK", &candidates), ErrorKind::Unavailable),
    ] {
        let temp = tempfile::TempDir::new().unwrap();
        let discovery = http_responses(vec![response]).await;
        let cache = discovering_cache(&temp, &format!("{discovery}/?q={SENTINEL}")).await;
        let (logs, _guard) = log_capture::capture();

        let error = cache
            .has_blob(&Digest::of_bytes(b"probe"))
            .await
            .unwrap_err();

        assert_eq!(error.kind(), kind, "{error}");
        assert_clean(&error, &logs.text());
    }
}

#[tokio::test]
async fn a_selected_endpoint_is_logged_without_its_credentials() {
    let temp = tempfile::TempDir::new().unwrap();
    let server = FaultServer::default();
    let data_plane = server.serve().await;
    let with_credentials = data_plane.replace("http://", &format!("http://user:{SENTINEL}@"));
    let discovery = http_responses(vec![http(
        "200 OK",
        &format!(r#"{{"endpoints":["{with_credentials}?k={SENTINEL}","http://127.0.0.1:1"]}}"#),
    )])
    .await;
    let cache = discovering_cache(&temp, &discovery).await;
    let (logs, _guard) = log_capture::capture();

    assert!(!cache.has_blob(&Digest::of_bytes(b"probe")).await.unwrap());

    let logs = logs.text();
    assert!(logs.contains("selected Tuist cache endpoint"), "{logs}");
    assert!(!logs.contains(SENTINEL), "{logs}");
}
