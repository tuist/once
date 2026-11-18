use super::*;

#[tokio::test]
async fn collections_deduplicate_overlapping_windows_and_stream_oversized_blobs() {
    let (_temp, server, cache, bodies) = fixture(140, 64 * 1024).await;
    let provider = CacheProvider::Tuist(cache.clone());
    let mut digests = bodies
        .iter()
        .map(|body| Digest::of_bytes(body))
        .collect::<Vec<_>>();
    cache.local.put_blob(&bodies[0]).await.unwrap();
    digests.extend_from_within(..10);
    digests.push(Digest::of_bytes(&[]));
    let reversed = digests.iter().copied().rev().collect::<Vec<_>>();
    tokio::time::timeout(Duration::from_secs(10), async {
        let (first, second) = tokio::join!(
            provider.prefetch_blobs(&digests),
            provider.prefetch_blobs(&reversed),
        );
        first.unwrap();
        second.unwrap();
    })
    .await
    .unwrap();
    for (body, digest) in bodies.iter().zip(&digests) {
        assert_eq!(cache.local.get_blob(digest).await.unwrap(), *body);
    }
    assert_eq!(
        cache.local.get_blob(&Digest::of_bytes(&[])).await.unwrap(),
        Vec::<u8>::new()
    );
    let state = server.0.lock().await;
    assert!(state.batch_requests.iter().any(|batch| batch.len() > 1));
    assert!(state.batch_requests.iter().all(|batch| batch
        .iter()
        .map(|digest| digest.size_bytes)
        .sum::<i64>()
        <= 64 * 1024));
    assert!(state.calls["read"] > 0);
    assert_eq!(
        state.batch_requests.iter().map(Vec::len).sum::<usize>() + state.calls["read"],
        bodies.len() - 1
    );
    drop(state);
    released(&cache).await;
}

#[tokio::test]
async fn cancelled_preparation_and_atomic_admission_do_not_block_individual_readers() {
    for preparing in [true, false] {
        let (_temp, _server, cache, bodies) = fixture(MAX_PENDING, 0).await;
        let mut digests = bodies
            .iter()
            .map(|body| Digest::of_bytes(body))
            .collect::<Vec<_>>();
        digests.sort_unstable_by_key(|digest| *digest.as_bytes());
        let held_flight = if preparing {
            Some(
                cache
                    .download_flights
                    .acquire(*digests.last().unwrap())
                    .await,
            )
        } else {
            None
        };
        let held_slot = if preparing {
            None
        } else {
            Some(
                Arc::clone(&cache.batch_downloads.slots)
                    .acquire_owned()
                    .await
                    .unwrap(),
            )
        };
        let owner = Arc::new(());
        let task = {
            let provider = CacheProvider::Tuist(cache.clone());
            let digests = digests.clone();
            let owner = Arc::clone(&owner);
            tokio::spawn(async move { provider.prefetch_blobs_guarded(&digests, owner).await })
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Ok(flight) = tokio::time::timeout(
                Duration::from_millis(1),
                cache.download_flights.acquire(digests[0]),
            )
            .await
            {
                drop(flight);
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            cache.batch_downloads.slots.available_permits(),
            MAX_PENDING - usize::from(!preparing)
        );
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(Arc::strong_count(&owner), 1);
        drop(held_flight);
        tokio::time::timeout(Duration::from_secs(5), cache.ensure_blob_local(&digests[0]))
            .await
            .unwrap()
            .unwrap();
        for digest in digests {
            drop(cache.download_flights.acquire(digest).await);
        }
        drop(held_slot);
        released(&cache).await;
        assert_eq!(cache.batch_downloads.slots.available_permits(), MAX_PENDING);
    }
}

#[tokio::test]
async fn a_known_collection_is_submitted_together_and_cancellation_releases_it() {
    let (_temp, server, cache, bodies) = fixture(12, 512 * 1024).await;
    server.0.lock().await.batch_stalls = true;
    let owner = Arc::new(());
    let digests = bodies
        .iter()
        .map(|body| Digest::of_bytes(body))
        .collect::<Vec<_>>();
    let task = {
        let provider = CacheProvider::Tuist(cache.clone());
        let owner = Arc::clone(&owner);
        let digests = digests.clone();
        tokio::spawn(async move { provider.prefetch_blobs_guarded(&digests, owner).await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.calls("batch_read").await == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(server.0.lock().await.batch_requests[0].len(), bodies.len());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if Arc::strong_count(&owner) == 1
                && cache.batch_downloads.slots.available_permits() == MAX_PENDING
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        for digest in digests {
            drop(cache.download_flights.acquire(digest).await);
        }
    })
    .await
    .unwrap();
    released(&cache).await;
}
