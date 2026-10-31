use std::time::Duration;

use super::Client;
use crate::{Error, RemoteIoLimits};

mod upload;

fn client(stall: Duration, ack: Duration) -> Client {
    Client::new(
        "/bin/sh".into(),
        RemoteIoLimits {
            stall_timeout: stall,
            upload_ack_timeout: ack,
            artifact_download_timeout: Duration::from_secs(10),
            ..RemoteIoLimits::default()
        },
    )
}

fn command(script: &str) -> Vec<String> {
    vec!["-c".into(), script.into()]
}

fn assert_stalled(result: crate::Result<()>, phase: &'static str, timeout: Duration) {
    assert!(
        matches!(result, Err(Error::ExecutorStalled { executor, phase: actual, timeout: bound })
            if executor == "docker" && actual == phase && bound == timeout),
        "expected Docker {phase} deadline of {timeout:?}"
    );
}

#[tokio::test]
async fn download_stalls_before_body_between_chunks_and_after_eof_are_bounded() {
    let stall = Duration::from_millis(200);
    for script in [
        "exec sleep 2",
        "printf first; exec sleep 2",
        "printf complete; exec 1>&-; exec sleep 2",
    ] {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("output");
        let result = client(stall, Duration::from_secs(3))
            .transfer(&command(script), None, Some(&output))
            .await;
        assert_stalled(result, "download", stall);
    }
}

#[tokio::test]
async fn download_progress_extends_stall_but_not_total_deadline() {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("output");
    let script = "i=0; while test $i -lt 20; do printf data; sleep 0.05; i=$((i+1)); done";
    let mut client = client(Duration::from_millis(400), Duration::from_millis(20));
    client
        .transfer(&command(script), None, Some(&output))
        .await
        .unwrap();
    assert_eq!(std::fs::read(&output).unwrap(), b"data".repeat(20));
    client.limits.artifact_download_timeout = Duration::from_millis(600);
    assert!(matches!(
        client.transfer(&command(script), None, Some(&output)).await,
        Err(Error::Timeout(bound)) if bound == Duration::from_millis(600)
    ));
}

#[tokio::test]
async fn quiet_execution_and_control_do_not_inherit_transfer_deadlines() {
    let root = tempfile::tempdir().unwrap();
    let cache = once_cas::CacheProvider::open_local(root.path().join("cache"));
    let mut client = client(Duration::from_millis(20), Duration::from_millis(20));
    client.limits.artifact_download_timeout = Duration::from_millis(20);
    let args = command("sleep 0.3; printf done");
    let (control, execution) = tokio::join!(
        client.control(&args),
        client.execute(&args, &cache, false, None)
    );
    assert_eq!(control.unwrap(), b"done");
    assert_eq!(execution.unwrap().exit_code, 0);
}
