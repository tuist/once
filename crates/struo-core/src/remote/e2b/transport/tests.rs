use super::*;
use crate::remote::e2b::client::Config;
use crate::remote::test_server::{self, Response};

#[tokio::test]
async fn uploaded_content_length_switches_to_acknowledgement_deadline() {
    let (response, mut hold) = Response::empty(200).held();
    let (url, server) = test_server::spawn(vec![response]).await;
    let mut client = Client::new(Config::for_test(url.clone(), url)).unwrap();
    client.limits.stall_timeout = Duration::from_secs(30);
    client.limits.upload_ack_timeout = Duration::from_mins(5);
    let sandbox = Sandbox {
        id: "sandbox-test".into(),
        access_token: None,
    };
    let source = tempfile::NamedTempFile::new().unwrap();
    let bytes = vec![b'x'; 256 * 1024];
    std::fs::write(source.path(), &bytes).unwrap();
    let mut upload = Box::pin(client.upload(
        &sandbox,
        source.path(),
        u64::try_from(bytes.len()).unwrap(),
        "/input.tar",
    ));
    tokio::select! {
        result = &mut upload => panic!("upload returned before acknowledgement: {result:?}"),
        arrived = &mut hold.arrived => arrived.unwrap(),
        () = tokio::time::sleep(Duration::from_secs(5)) => panic!("upload did not reach test server"),
    }
    tokio::time::pause();
    tokio::time::advance(Duration::from_mins(1)).await;
    let pending = futures::poll!(&mut upload).is_pending();
    tokio::time::resume();
    let _ = hold.release.send(());
    assert!(
        pending,
        "body stall clock remained active after the declared length arrived"
    );
    tokio::time::timeout(Duration::from_secs(5), upload)
        .await
        .unwrap()
        .unwrap();
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].body, bytes);
}
