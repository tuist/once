use super::*;

#[tokio::test]
async fn daemon_replacement_before_or_during_listing_cannot_release_retained_admission() {
    for switch_during_listing in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let client = mock_client(root.path()).await;
        let original_scope = client.scope();
        let host = ExecutorResources::new(Arc::new(ResourcePool::new(ResourceLimits::new(1, 100))));
        let workers = Arc::new(ResourcePool::new(ResourceLimits::new(1, 100)));
        let request = ResourceRequest::new(1, 100).with_slot("exclusive", 1);
        let worker = workers.acquire(ResourceRequest::new(1, 100)).await;
        let lease = host.acquire(Executor::Docker, &request).await;
        let (container, cleanup) = create(
            client.clone(),
            lifecycle::Journal::new(root.path()),
            "image".into(),
            lease,
            worker,
            None,
        )
        .await
        .unwrap();
        std::fs::write(root.path().join("release"), b"").unwrap();
        std::fs::write(root.path().join("fail"), b"").unwrap();
        assert!(cleanup.run().await.is_err());
        let nonce = std::fs::read_to_string(root.path().join("nonce")).unwrap();
        std::fs::remove_file(root.path().join("alive")).unwrap();
        let key = (original_scope.clone(), nonce.clone());
        retained().lock().unwrap().get_mut(&key).unwrap().credential = "another-credential".into();
        assert_eq!(client.find(&nonce).await.unwrap(), Lookup::Inconclusive);
        retained().lock().unwrap().get_mut(&key).unwrap().credential =
            Provider::credential(&client);
        let switch = if switch_during_listing {
            "switch-on-list"
        } else {
            "daemon"
        };
        std::fs::write(root.path().join(switch), b"replacement-daemon").unwrap();
        assert!(client.find(&nonce).await.is_err());
        assert_eq!(client.scope(), original_scope);
        let replacement = Client::new(client.binary.clone(), client.limits)
            .identify()
            .await
            .unwrap();
        assert_ne!(replacement.scope(), original_scope);
        assert_eq!(replacement.find(&nonce).await.unwrap(), Lookup::Absent);
        assert!(client.delete(&container.0).await.is_err());
        assert!(tokio::time::timeout(
            Duration::from_millis(50),
            host.acquire(Executor::Local, &request)
        )
        .await
        .is_err());
        assert!(tokio::time::timeout(
            Duration::from_millis(50),
            workers.acquire(ResourceRequest::new(1, 100))
        )
        .await
        .is_err());
        if switch_during_listing {
            std::fs::remove_file(root.path().join("switch-on-list")).unwrap();
        }
        std::fs::remove_file(root.path().join("daemon")).unwrap();
        std::fs::write(root.path().join("alive"), b"").unwrap();
        std::fs::remove_file(root.path().join("fail")).unwrap();
        assert!(matches!(
            client.find(&nonce).await.unwrap(),
            Lookup::Found(_)
        ));
        client.delete(&container.0).await.unwrap();
        let _host = tokio::time::timeout(
            Duration::from_secs(1),
            host.acquire(Executor::Local, &request),
        )
        .await
        .unwrap();
        let _worker = tokio::time::timeout(
            Duration::from_secs(1),
            workers.acquire(ResourceRequest::new(1, 100)),
        )
        .await
        .unwrap();
    }
}
