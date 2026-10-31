use super::{assert_stalled, client, command, Duration};

#[tokio::test]
async fn upload_body_stalls_even_when_control_output_is_active() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input");
    std::fs::write(&input, vec![0_u8; 2 * 1024 * 1024]).unwrap();
    let stall = Duration::from_millis(200);
    let result = client(stall, Duration::from_secs(3))
        .transfer(
            &command("i=0; while test $i -lt 40; do printf log >&2; sleep 0.05; i=$((i+1)); done"),
            Some(&input),
            None,
        )
        .await;
    assert_stalled(result, "upload", stall);
}

#[tokio::test]
async fn upload_acknowledgement_has_a_shorter_independent_bound() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input");
    std::fs::write(&input, b"archive").unwrap();
    let ack = Duration::from_millis(200);
    let result = client(Duration::from_secs(3), ack)
        .transfer(&command("cat >/dev/null; exec sleep 2"), Some(&input), None)
        .await;
    assert_stalled(result, "upload acknowledgement", ack);
}

#[tokio::test]
async fn empty_upload_starts_the_acknowledgement_bound() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input");
    std::fs::write(&input, []).unwrap();
    let ack = Duration::from_millis(200);
    let result = client(Duration::from_secs(3), ack)
        .transfer(&command("exec sleep 2"), Some(&input), None)
        .await;
    assert_stalled(result, "upload acknowledgement", ack);
}

#[tokio::test]
async fn upload_acknowledgement_may_outlast_the_body_stall_bound() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input");
    std::fs::write(&input, b"archive").unwrap();
    client(Duration::from_millis(100), Duration::from_secs(3))
        .transfer(
            &command("cat >/dev/null; exec sleep 0.5"),
            Some(&input),
            None,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn upload_acknowledgement_does_not_start_before_the_body_finishes() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input");
    std::fs::write(&input, vec![0_u8; 2 * 1024 * 1024]).unwrap();
    client(Duration::from_secs(3), Duration::from_millis(200))
        .transfer(&command("sleep 0.5; cat >/dev/null"), Some(&input), None)
        .await
        .unwrap();
}
