use super::*;
use struo_cas::Digest;

#[allow(clippy::needless_pass_by_value)]
fn frame(value: serde_json::Value) -> Vec<u8> {
    let payload = serde_json::to_vec(&value).unwrap();
    let mut frame = vec![0];
    frame.extend(u32::try_from(payload.len()).unwrap().to_be_bytes());
    frame.extend(payload);
    frame
}

#[tokio::test]
async fn output_larger_than_response_budget_streams_with_backpressure() {
    let directory = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(directory.path());
    let (mut writer, reader) = tokio::io::duplex(PIPE_CAPACITY);
    let payload = vec![b'x'; 4096];
    let encoded = base64::engine::general_purpose::STANDARD.encode(&payload);
    let data_frame = frame(serde_json::json!({"event":{"data":{"stdout":encoded}}}));
    let end = frame(serde_json::json!({"event":{"end":{"exitCode":0}}}));
    let mut hasher = blake3::Hasher::new();
    for _ in 0..8192 {
        hasher.update(&payload);
    }
    let expected = Digest::from_bytes(*hasher.finalize().as_bytes());
    let produce = tokio::spawn(async move {
        for _ in 0..8192 {
            writer.write_all(&data_frame).await.unwrap();
        }
        writer.write_all(&end).await.unwrap();
    });
    let limits = RemoteIoLimits {
        max_response_bytes: 8192,
        ..RemoteIoLimits::default()
    };
    let result = capture_command(reader, &cache, false, limits, None)
        .await
        .unwrap();
    produce.await.unwrap();
    assert_eq!(result.stdout, Some(expected));
    assert!(cache.has_blob(&expected).await.unwrap());
    assert_eq!(result.stderr, Some(Digest::of_bytes(b"")));
}

#[tokio::test]
async fn rejects_large_or_truncated_frames_and_missing_completion() {
    let directory = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(directory.path());
    for body in [
        vec![0, 0, 0, 0, 100],
        vec![0, 0, 0],
        frame(serde_json::json!({"event":{"data":{"stdout":"eA=="}}})),
    ] {
        let limits = RemoteIoLimits {
            max_response_bytes: 64,
            ..RemoteIoLimits::default()
        };
        assert!(
            capture_command(std::io::Cursor::new(body), &cache, false, limits, None)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn json_lines_and_binary_preserve_both_streams() {
    let directory = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(directory.path());
    let data = serde_json::json!({"event":{"data":{"stdout":"b3V0","stderr":"ZXJy"}}});
    let end = serde_json::json!({"event":{"end":{"exitCode":7}}});
    let lines = format!("{data}\n{end}\n").into_bytes();
    let mut binary = frame(data);
    binary.extend(frame(end));
    for body in [lines, binary] {
        let result = capture_command(
            std::io::Cursor::new(body),
            &cache,
            false,
            RemoteIoLimits::default(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 7);
        assert_eq!(result.stdout, Some(Digest::of_bytes(b"out")));
        assert_eq!(result.stderr, Some(Digest::of_bytes(b"err")));
    }
}

#[tokio::test]
async fn capture_bytes_are_bounded_independently_of_frame_size() {
    let directory = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(directory.path());
    let data = frame(serde_json::json!({"event":{"data":{"stdout":"b3V0"}}}));
    let limits = RemoteIoLimits {
        max_capture_bytes: 2,
        ..RemoteIoLimits::default()
    };
    assert!(matches!(
        capture_command(std::io::Cursor::new(data), &cache, false, limits, None).await,
        Err(Error::ExecutorResourceLimit {
            resource: "captured output bytes",
            ..
        })
    ));
}

#[tokio::test]
async fn stalled_capture_fails_without_publishing_an_action_result() {
    let directory = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(directory.path());
    let (_writer, reader) = tokio::io::duplex(64);
    let limits = RemoteIoLimits {
        command_idle_timeout: Some(std::time::Duration::from_millis(20)),
        ..RemoteIoLimits::default()
    };
    assert!(matches!(
        capture_command(reader, &cache, false, limits, None).await,
        Err(Error::ExecutorStalled {
            phase: "command output",
            ..
        })
    ));
}

async fn decode(body: Vec<u8>) -> Result<ActionResult> {
    let directory = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(directory.path());
    capture_command(
        std::io::Cursor::new(body),
        &cache,
        false,
        RemoteIoLimits::default(),
        None,
    )
    .await
}

fn trailer(payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![2];
    frame.extend(u32::try_from(payload.len()).unwrap().to_be_bytes());
    frame.extend(payload);
    frame
}

#[tokio::test]
async fn completion_follows_protobuf_json_exit_semantics() {
    let success =
        frame(serde_json::json!({"event":{"end":{"exited":true,"status":"exit status 0"}}}));
    assert_eq!(decode(success).await.unwrap().exit_code, 0);

    let signalled =
        frame(serde_json::json!({"event":{"end":{"exitCode":-1,"status":"signal: killed"}}}));
    assert_eq!(decode(signalled).await.unwrap().exit_code, -1);

    for unproven in [
        serde_json::json!({"event":{"end":{"status":"unknown"}}}),
        serde_json::json!({"event":{"end":{"exited":true,"error":"exec failed"}}}),
    ] {
        assert!(decode(frame(unproven)).await.is_err());
    }
}

#[tokio::test]
async fn the_end_of_stream_trailer_ends_the_stream() {
    let end = frame(serde_json::json!({"event":{"end":{"exitCode":4,"exited":true}}}));
    for clean in [trailer(b""), trailer(b"{}"), trailer(b"{\"error\":null}")] {
        let mut body = end.clone();
        body.extend(clean);
        assert_eq!(decode(body).await.unwrap().exit_code, 4);
    }

    let mut failed = end.clone();
    failed.extend(trailer(b"{\"error\":{\"code\":\"deadline_exceeded\"}}"));
    assert!(decode(failed).await.is_err());

    let mut trailing = end;
    trailing.extend(trailer(b"{}"));
    trailing.extend(frame(
        serde_json::json!({"event":{"data":{"stdout":"eA=="}}}),
    ));
    assert!(decode(trailing).await.is_err());
}

#[tokio::test]
async fn blank_json_lines_are_ignored() {
    let body = format!(
        "\n{}\n\n{}\n",
        serde_json::json!({"event":{"data":{"stdout":"b3V0"}}}),
        serde_json::json!({"event":{"end":{"exited":true}}})
    );
    let result = decode(body.into_bytes()).await.unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, Some(Digest::of_bytes(b"out")));
}
