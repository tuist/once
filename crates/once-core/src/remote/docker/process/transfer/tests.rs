use std::time::Duration;

use tokio::io::AsyncReadExt;

use super::{upload, Progress};
use crate::RemoteIoLimits;

#[tokio::test(start_paused = true)]
async fn partial_upload_writes_reset_the_stall_deadline() {
    let progress = Progress::new();
    let limits = RemoteIoLimits {
        stall_timeout: Duration::from_secs(2),
        upload_ack_timeout: Duration::from_secs(30),
        ..RemoteIoLimits::default()
    };
    let (writer, mut reader) = tokio::io::duplex(1);
    let body = b"archive";
    let (sent, received) = tokio::join!(
        progress.watch_upload("docker", limits, upload(&body[..], writer, &progress)),
        async {
            let mut received = Vec::new();
            for _ in body {
                tokio::time::sleep(Duration::from_secs(1)).await;
                received.push(reader.read_u8().await.unwrap());
            }
            assert_eq!(reader.read(&mut [0]).await.unwrap(), 0);
            received
        }
    );
    sent.unwrap();
    assert_eq!(received, body);
}
