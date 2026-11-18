//! These tests drive the stream and the clock from the test itself: bytes are
//! written by the test, the reader is polled exactly once with `poll!`, and
//! time moves only through `advance`. No second task and no automatic clock
//! advance take part, so every claim about what is pending after a given
//! amount of time is deterministic.

use std::future::Future;
use std::task::Poll;
use std::time::Duration;

use futures::poll;
use tokio::io::{AsyncWriteExt, DuplexStream};

use super::*;

const QUIET: Duration = Duration::from_mins(10);
const STALL: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_millis(1);
const BEFORE_STALL: Duration = STALL.saturating_sub(TICK);

fn envelope(flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![flags];
    frame.extend(u32::try_from(payload.len()).unwrap().to_be_bytes());
    frame.extend(payload);
    frame
}

fn message(json_lines: bool, payload: &[u8]) -> Vec<u8> {
    if json_lines {
        let mut line = payload.to_vec();
        line.push(b'\n');
        line
    } else {
        envelope(0, payload)
    }
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

/// Let `elapsed` pass and require `future` to still be waiting.
async fn pending_after<F: Future + Unpin>(future: &mut F, elapsed: Duration) {
    tokio::time::advance(elapsed).await;
    assert!(
        poll!(&mut *future).is_pending(),
        "finished before {elapsed:?} had passed"
    );
}

/// Let `elapsed` pass and require `future` to have finished.
async fn ready_after<F: Future + Unpin>(future: &mut F, elapsed: Duration) -> F::Output {
    tokio::time::advance(elapsed).await;
    match poll!(&mut *future) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("still waiting after {elapsed:?}"),
    }
}

fn is_stall(result: &Result<Option<Vec<u8>>>) -> bool {
    matches!(
        result,
        Err(Error::ExecutorStalled {
            phase: "command output",
            ..
        })
    )
}

#[tokio::test(start_paused = true)]
async fn quiet_gaps_between_messages_are_not_stalls() {
    for json_lines in [false, true] {
        let (mut writer, reader) = pipe();
        let mut frames = FrameReader::new(reader, limits());
        writer
            .write_all(&message(json_lines, b"{\"first\":1}"))
            .await
            .unwrap();
        assert!(frames.next().await.unwrap().is_some());

        let mut second = Box::pin(frames.next());
        pending_after(&mut second, Duration::ZERO).await;
        pending_after(&mut second, QUIET).await;
        writer
            .write_all(&message(json_lines, b"{\"second\":2}"))
            .await
            .unwrap();
        drop(writer);
        assert!(second.await.unwrap().is_some());
        assert!(frames.next().await.unwrap().is_none());
    }
}

#[tokio::test(start_paused = true)]
async fn an_explicit_idle_timeout_bounds_the_wait_between_messages() {
    let idle = Duration::from_mins(1);
    let (mut writer, reader) = pipe();
    let mut frames = FrameReader::new(
        reader,
        RemoteIoLimits {
            command_idle_timeout: Some(idle),
            ..limits()
        },
    );
    writer.write_all(&envelope(0, b"{}")).await.unwrap();
    assert!(frames.next().await.unwrap().is_some());

    let mut next = Box::pin(frames.next());
    pending_after(&mut next, Duration::ZERO).await;
    pending_after(&mut next, idle.saturating_sub(TICK)).await;
    assert!(is_stall(&ready_after(&mut next, TICK).await));
}

#[tokio::test(start_paused = true)]
async fn a_message_that_stops_arriving_stalls_at_the_stall_bound() {
    let complete = envelope(0, b"{\"event\":{}}");
    for partial in [
        complete[..3].to_vec(),
        complete[..complete.len() - 2].to_vec(),
        b"{\"event\":".to_vec(),
    ] {
        let (mut writer, reader) = pipe();
        let mut frames = FrameReader::new(reader, limits());
        writer.write_all(&partial).await.unwrap();
        let mut next = Box::pin(frames.next());
        pending_after(&mut next, Duration::ZERO).await;
        pending_after(&mut next, BEFORE_STALL).await;
        assert!(is_stall(&ready_after(&mut next, TICK).await));
    }
}

#[tokio::test(start_paused = true)]
async fn a_slow_message_that_keeps_arriving_is_not_a_stall() {
    let (mut writer, reader) = pipe();
    let mut frames = FrameReader::new(reader, limits());
    let frame = envelope(0, &[b' '; 64]);
    let mut next = Box::pin(frames.next());
    for chunk in frame.chunks(8) {
        writer.write_all(chunk).await.unwrap();
        // Each chunk arrives within the stall bound; the whole message takes
        // far longer than it.
        pending_after(&mut next, STALL.saturating_sub(Duration::from_secs(10))).await;
    }
    // The whitespace message is skipped; the one after it is returned.
    writer.write_all(&envelope(0, b"{}")).await.unwrap();
    assert_eq!(
        ready_after(&mut next, Duration::ZERO).await.unwrap(),
        Some(b"{}".to_vec())
    );
}

#[tokio::test(start_paused = true)]
async fn oversized_messages_are_rejected_before_they_are_read() {
    let (mut writer, reader) = pipe();
    let mut header = vec![0];
    header.extend(1025_u32.to_be_bytes());
    writer.write_all(&header).await.unwrap();
    let mut frames = FrameReader::new(
        reader,
        RemoteIoLimits {
            max_response_bytes: 1024,
            ..limits()
        },
    );
    let mut next = Box::pin(frames.next());
    assert!(matches!(
        ready_after(&mut next, Duration::ZERO).await,
        Err(Error::ExecutorResourceLimit {
            resource: "command frame bytes",
            ..
        })
    ));
}

#[tokio::test(start_paused = true)]
async fn the_total_budget_counts_every_byte_including_empty_messages() {
    for json_lines in [false, true] {
        let (mut writer, reader) = pipe();
        for _ in 0..64 {
            writer.write_all(&message(json_lines, b" ")).await.unwrap();
        }
        drop(writer);
        let mut frames = FrameReader::new(reader, limits()).with_total(64, "test bytes");
        assert!(matches!(
            frames.next().await,
            Err(Error::ExecutorResourceLimit {
                resource: "test bytes",
                ..
            })
        ));
    }
}

#[tokio::test(start_paused = true)]
async fn the_end_of_stream_message_must_be_last() {
    let (mut writer, reader) = pipe();
    writer.write_all(&envelope(2, b"{}")).await.unwrap();
    writer.write_all(&envelope(0, b"{}")).await.unwrap();
    drop(writer);
    assert!(FrameReader::new(reader, limits()).next().await.is_err());

    let (mut writer, reader) = pipe();
    writer.write_all(&envelope(1, b"{}")).await.unwrap();
    drop(writer);
    assert!(FrameReader::new(reader, limits()).next().await.is_err());
}
