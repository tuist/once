//! The stream and the clock are driven from the test itself, with no writer
//! task and no automatic clock advance, so timing claims are deterministic.

use std::task::Poll;
use std::time::Duration;

use futures::poll;
use tokio::io::{AsyncWriteExt, DuplexStream};

use super::*;
use crate::Error;

const STALL: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_millis(1);

fn envelope(flags: u8, value: &serde_json::Value) -> Vec<u8> {
    let payload = serde_json::to_vec(value).unwrap();
    let mut frame = vec![flags];
    frame.extend(u32::try_from(payload.len()).unwrap().to_be_bytes());
    frame.extend(payload);
    frame
}

fn limits() -> RemoteIoLimits {
    RemoteIoLimits {
        stall_timeout: STALL,
        command_idle_timeout: None,
        ..RemoteIoLimits::default()
    }
}

fn pipe() -> (DuplexStream, DuplexStream) {
    tokio::io::duplex(1 << 16)
}

/// A finished stream holding exactly `body`.
async fn complete(body: &[u8]) -> DuplexStream {
    let (mut writer, reader) = pipe();
    writer.write_all(body).await.unwrap();
    reader
}

fn started() -> Vec<u8> {
    envelope(0, &serde_json::json!({"event":{"start":{"pid":1}}}))
}

fn ended() -> Vec<u8> {
    let mut end = envelope(0, &serde_json::json!({"event":{"end":{"exited":true}}}));
    end.extend(envelope(2, &serde_json::json!({})));
    end
}

#[tokio::test(start_paused = true)]
async fn a_quiet_setup_command_outlasts_the_stall_timeout() {
    let (mut writer, reader) = pipe();
    writer.write_all(&started()).await.unwrap();
    let mut collect = Box::pin(collect_process(reader, limits()));
    assert!(poll!(&mut collect).is_pending());

    tokio::time::advance(Duration::from_mins(10)).await;
    assert!(
        poll!(&mut collect).is_pending(),
        "a quiet setup step was treated as a stall"
    );

    writer.write_all(&ended()).await.unwrap();
    drop(writer);
    let result = collect.await.unwrap();
    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.is_empty() && result.stderr.is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_partial_frame_still_stalls_at_the_stall_bound() {
    let (mut writer, reader) = pipe();
    writer.write_all(&started()).await.unwrap();
    writer.write_all(&ended()[..4]).await.unwrap();
    let mut collect = Box::pin(collect_process(reader, limits()));
    assert!(poll!(&mut collect).is_pending());

    tokio::time::advance(STALL.saturating_sub(TICK)).await;
    assert!(poll!(&mut collect).is_pending());
    tokio::time::advance(TICK).await;
    match poll!(&mut collect) {
        Poll::Ready(Err(Error::ExecutorStalled { .. })) => {}
        Poll::Ready(other) => panic!("expected a stall, got {:?}", other.map(|_| ())),
        Poll::Pending => panic!("a partial frame outlived the stall bound"),
    }
    drop(writer);
}

#[tokio::test(start_paused = true)]
async fn collects_output_and_exit_status_in_both_encodings() {
    let data = serde_json::json!({"event":{"data":{"stdout":"b3V0","stderr":"ZXJy"}}});
    let end = serde_json::json!({"event":{"end":{"exitCode":7,"exited":true}}});
    let mut binary = envelope(0, &data);
    binary.extend(envelope(0, &end));
    binary.extend(envelope(2, &serde_json::json!({})));
    let lines = format!("{data}\n\n{end}\n").into_bytes();
    for body in [binary, lines] {
        let result = collect_process(complete(&body).await, limits())
            .await
            .unwrap();
        assert_eq!(result.exit_code, 7);
        assert_eq!(result.stdout, b"out");
        assert_eq!(result.stderr, b"err");
    }
}

#[tokio::test(start_paused = true)]
async fn setup_output_is_bounded_by_the_control_budget() {
    let tight = RemoteIoLimits {
        max_response_bytes: 256,
        ..limits()
    };
    let mut body = Vec::new();
    for _ in 0..16 {
        body.extend(envelope(
            0,
            &serde_json::json!({"event":{"data":{"stdout":"eHh4eHh4eHg="}}}),
        ));
    }
    body.extend(ended());
    assert!(matches!(
        collect_process(complete(&body).await, tight).await,
        Err(Error::ExecutorResourceLimit {
            resource: "setup response bytes",
            ..
        })
    ));
}

#[tokio::test(start_paused = true)]
async fn a_stream_without_completion_is_rejected() {
    let error = collect_process(complete(&started()).await, limits())
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("without an exit code"));
}
