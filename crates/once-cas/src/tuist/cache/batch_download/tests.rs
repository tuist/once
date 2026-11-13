use super::*;
use crate::tuist::cache::fault_server::{connected_cache, FaultServer};
use crate::{CacheProvider, MIN_TRANSFER_MEMORY_BYTES};
use futures::future::join_all;

mod collections;

async fn fixture(
    count: usize,
    limit: u64,
) -> (tempfile::TempDir, FaultServer, TuistCache, Vec<Vec<u8>>) {
    let temp = tempfile::TempDir::new().unwrap();
    let server = FaultServer::default();
    let url = server.serve().await;
    let mut cache = connected_cache(&temp, "reader", &url).await;
    cache.capabilities_cache = Arc::default();
    cache.capabilities_cache.set(RemoteCapabilities {
        batch_read_limit: Some(limit),
        zstd_batches: true,
        ..RemoteCapabilities::default()
    });
    cache.set_transfer_budget(TransferBudget::new(
        MIN_TRANSFER_MEMORY_BYTES,
        2 * 1024 * 1024,
    ));
    let mut bodies = Vec::new();
    for index in 0..count {
        let body = vec![u8::try_from(index + 1).unwrap(); 9_000 + index * 1_307];
        let remote = sha256_digest(&body).unwrap();
        server
            .0
            .lock()
            .await
            .blobs
            .insert(remote.hash.clone(), body.clone());
        cache
            .known_remote_blobs
            .lock()
            .await
            .insert(Digest::of_bytes(&body), remote);
        bodies.push(body);
    }
    (temp, server, cache, bodies)
}

async fn released(cache: &TuistCache) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while cache.budget.available()
            != (cache.budget.memory_bytes(), cache.budget.staging_bytes())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

fn reply(body: &[u8], code: Code) -> reapi::batch_read_blobs_response::Response {
    reapi::batch_read_blobs_response::Response {
        digest: Some(sha256_digest(body).unwrap()),
        data: body.to_vec(),
        status: Some(bazel_remote_apis::google::rpc::Status {
            code: code as i32,
            ..Default::default()
        }),
        compressor: reapi::compressor::Value::Identity as i32,
    }
}

#[tokio::test]
async fn concurrent_restores_batch_within_server_limits_and_preserve_each_observer() {
    let (temp, server, cache, bodies) = fixture(20, 64 * 1024).await;
    let provider = CacheProvider::Tuist(cache.clone());
    let observations = bodies
        .iter()
        .map(|_| Arc::new(transfer::Recorder::default()))
        .collect::<Vec<_>>();
    join_all(bodies.iter().enumerate().map(|(index, body)| {
        let provider = &provider;
        let destination = temp.path().join(format!("output-{index}"));
        let recorder = Arc::clone(&observations[index]);
        async move {
            let digest = Digest::of_bytes(body);
            transfer::observe(recorder, provider.copy_blob_to_file(&digest, &destination))
                .await
                .unwrap();
            assert_eq!(std::fs::read(destination).unwrap(), *body);
        }
    }))
    .await;
    for (body, observer) in bodies.iter().zip(observations) {
        let transfers = observer.transfers();
        assert_eq!(transfers.len(), 1);
        assert_eq!(transfers[0].digest, Digest::of_bytes(body));
        assert_eq!(transfers[0].size_bytes, body.len() as u64);
    }
    let state = server.0.lock().await;
    assert_eq!(
        state.batch_requests.iter().map(Vec::len).sum::<usize>(),
        bodies.len()
    );
    assert!(state.batch_requests.iter().all(|batch| batch
        .iter()
        .map(|digest| digest.size_bytes)
        .sum::<i64>()
        <= 64 * 1024));
    assert_eq!(state.calls.get("read").copied().unwrap_or_default(), 0);
    drop(state);
    released(&cache).await;
}

#[tokio::test]
async fn batched_reads_verify_both_hashes_and_reject_malformed_envelopes() {
    for failure in ["sha256", "native", "digest", "duplicate", "inflation"] {
        let (_temp, server, cache, bodies) = fixture(1, 0).await;
        let body = &bodies[0];
        let mut digest = Digest::of_bytes(body);
        let mut response = reply(body, Code::Ok);
        match failure {
            "sha256" => {
                response.data[0] ^= 1;
            }
            "native" => {
                digest = Digest::of_bytes(b"wrong native content");
                cache
                    .known_remote_blobs
                    .lock()
                    .await
                    .insert(digest, sha256_digest(body).unwrap());
            }
            "digest" => {
                response.digest = Some(sha256_digest(b"unsolicited").unwrap());
            }
            "duplicate" => {
                server.0.lock().await.batch_duplicate_response = true;
            }
            "inflation" => {
                response.data = zstd::encode_all(&vec![0; body.len() * 2][..], 1).unwrap();
                response.compressor = reapi::compressor::Value::Zstd as i32;
            }
            _ => unreachable!(),
        }
        server
            .0
            .lock()
            .await
            .batch_overrides
            .insert(sha256_digest(body).unwrap().hash, response);
        let error = cache.ensure_blob_local(&digest).await.unwrap_err();
        let expected = if matches!(failure, "digest" | "duplicate") {
            ErrorKind::Protocol
        } else {
            ErrorKind::Corrupt
        };
        assert_eq!(error.kind(), expected, "{failure}: {error}");
        assert!(!cache.local.has_blob(&digest).await.unwrap());
        assert_eq!(
            server.calls("read").await,
            0,
            "do not mask corrupt batches with a stream fallback"
        );
        released(&cache).await;
    }
}

#[tokio::test]
async fn per_blob_failures_do_not_discard_successes_or_retransfer_them() {
    for code in [
        Code::NotFound,
        Code::PermissionDenied,
        Code::Unavailable,
        Code::Ok,
    ] {
        let (_temp, server, cache, bodies) = fixture(2, 0).await;
        let mut response = reply(&bodies[1], code);
        if code == Code::Ok {
            response.data = zstd::encode_all(bodies[1].as_slice(), 1).unwrap();
            response.compressor = reapi::compressor::Value::Zstd as i32;
        }
        server
            .0
            .lock()
            .await
            .batch_overrides
            .insert(sha256_digest(&bodies[1]).unwrap().hash, response);
        let results = join_all(
            bodies
                .iter()
                .map(|body| async { cache.ensure_blob_local(&Digest::of_bytes(body)).await }),
        )
        .await;
        assert!(results[0].is_ok());
        assert_eq!(
            cache
                .local
                .get_blob(&Digest::of_bytes(&bodies[0]))
                .await
                .unwrap(),
            bodies[0]
        );
        match code {
            Code::NotFound => assert!(matches!(results[1], Err(Error::BlobNotFound(_)))),
            Code::PermissionDenied => {
                assert_eq!(results[1].as_ref().unwrap_err().kind(), ErrorKind::Refused);
            }
            Code::Unavailable | Code::Ok => {
                assert!(results[1].is_ok(), "{:?}", results[1]);
                assert_eq!(
                    cache
                        .local
                        .get_blob(&Digest::of_bytes(&bodies[1]))
                        .await
                        .unwrap(),
                    bodies[1]
                );
            }
            _ => unreachable!(),
        }
        assert_eq!(
            server.calls("read").await,
            usize::from(code == Code::Unavailable)
        );
        assert_eq!(
            server
                .0
                .lock()
                .await
                .batch_requests
                .iter()
                .map(Vec::len)
                .sum::<usize>(),
            2
        );
        released(&cache).await;
    }
}

#[tokio::test]
async fn cancelling_batched_readers_releases_capacity_owners_and_download_flights() {
    let (_temp, server, cache, bodies) = fixture(4, 0).await;
    server.0.lock().await.batch_stalls = true;
    let owner = Arc::new(());
    let tasks = bodies
        .iter()
        .map(|body| {
            let cache = cache.clone();
            let owner = Arc::clone(&owner);
            let digest = Digest::of_bytes(body);
            tokio::spawn(async move { cache.ensure_blob_local_guarded(&digest, owner).await })
        })
        .collect::<Vec<_>>();
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.calls("batch_read").await == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for task in &tasks {
        task.abort();
    }
    for task in tasks {
        assert!(task.await.unwrap_err().is_cancelled());
    }
    released(&cache).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while Arc::strong_count(&owner) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    server.0.lock().await.batch_stalls = false;
    let digest = Digest::of_bytes(&bodies[0]);
    tokio::time::timeout(Duration::from_secs(5), cache.ensure_blob_local(&digest))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cache.local.get_blob(&digest).await.unwrap(), bodies[0]);
    released(&cache).await;
}

#[tokio::test]
async fn cancelling_one_reader_releases_its_owner_and_flight_while_the_batch_remains_live() {
    let (_temp, server, cache, bodies) = fixture(2, 0).await;
    server.0.lock().await.batch_stalls = true;
    let owners = [Arc::new(()), Arc::new(())];
    let digests = bodies
        .iter()
        .map(|body| Digest::of_bytes(body))
        .collect::<Vec<_>>();
    let flights = [
        cache.download_flights.acquire(digests[0]).await,
        cache.download_flights.acquire(digests[1]).await,
    ];
    let (cancel, registration) = futures::future::AbortHandle::new_pair();
    let task = {
        let cache = cache.clone();
        let owners = owners.clone();
        let digests = digests.clone();
        tokio::spawn(async move {
            let [a, b] = flights;
            let [owner_a, owner_b] = owners;
            tokio::join!(
                futures::future::Abortable::new(
                    cache.enqueue_download(
                        digests[0],
                        sha256_digest(&bodies[0]).unwrap(),
                        a,
                        owner_a,
                        64 * 1024
                    ),
                    registration
                ),
                cache.enqueue_download(
                    digests[1],
                    sha256_digest(&bodies[1]).unwrap(),
                    b,
                    owner_b,
                    64 * 1024
                ),
            )
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.calls("batch_read").await == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(server.0.lock().await.batch_requests[0].len(), 2);
    cancel.abort();
    tokio::time::timeout(Duration::from_secs(5), async {
        while Arc::strong_count(&owners[0]) != 1 {
            tokio::task::yield_now().await;
        }
        let _flight = cache.download_flights.acquire(digests[0]).await;
    })
    .await
    .unwrap();
    assert_eq!(Arc::strong_count(&owners[1]), 2);
    assert!(!task.is_finished());
    assert_eq!(
        cache.batch_downloads.slots.available_permits(),
        MAX_PENDING - 1
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    released(&cache).await;
}

#[tokio::test]
async fn cancelling_a_streaming_fallback_releases_its_owner_and_transfer_capacity() {
    let (_temp, server, cache, bodies) = fixture(1, 0).await;
    server
        .fail("batch_read", vec![Status::unimplemented("use streaming")])
        .await;
    server.0.lock().await.read_stalls = true;
    let owner = Arc::new(());
    let task = {
        let cache = cache.clone();
        let owner = Arc::clone(&owner);
        let digest = Digest::of_bytes(&bodies[0]);
        tokio::spawn(async move { cache.ensure_blob_local_guarded(&digest, owner).await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.calls("read").await == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    released(&cache).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while Arc::strong_count(&owner) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    server.0.lock().await.read_stalls = false;
    let digest = Digest::of_bytes(&bodies[0]);
    tokio::time::timeout(Duration::from_secs(5), cache.ensure_blob_local(&digest))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cache.local.get_blob(&digest).await.unwrap(), bodies[0]);
    released(&cache).await;
}

#[tokio::test]
async fn cancelling_a_queued_fallback_reader_does_not_wait_for_active_transfers() {
    let count = MAX_PARALLEL_TRANSFERS + 1;
    let (_temp, server, cache, bodies) = fixture(count, 0).await;
    for body in &bodies {
        server.0.lock().await.batch_overrides.insert(
            sha256_digest(body).unwrap().hash,
            reply(body, Code::Unavailable),
        );
    }
    server.0.lock().await.read_stalls = true;
    let owners = (0..count).map(|_| Arc::new(())).collect::<Vec<_>>();
    let digests = bodies
        .iter()
        .map(|body| Digest::of_bytes(body))
        .collect::<Vec<_>>();
    let flights = join_all(
        digests
            .iter()
            .map(|digest| cache.download_flights.acquire(*digest)),
    )
    .await;
    let (cancel, registrations): (Vec<_>, Vec<_>) = (0..count)
        .map(|_| futures::future::AbortHandle::new_pair())
        .unzip();
    let task = {
        let cache = cache.clone();
        let owners = owners.clone();
        tokio::spawn(async move {
            join_all(
                bodies
                    .into_iter()
                    .zip(flights)
                    .zip(owners)
                    .zip(registrations)
                    .map(|(((body, flight), owner), registration)| {
                        futures::future::Abortable::new(
                            cache.enqueue_download(
                                Digest::of_bytes(&body),
                                sha256_digest(&body).unwrap(),
                                flight,
                                owner,
                                BATCH_BLOB_LIMIT as u64,
                            ),
                            registration,
                        )
                    }),
            )
            .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.calls("read").await == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(server.0.lock().await.batch_requests[0].len(), count);
    cancel[count - 1].abort();
    tokio::time::timeout(Duration::from_secs(5), async {
        while Arc::strong_count(&owners[count - 1]) != 1 {
            tokio::task::yield_now().await;
        }
        let _flight = cache.download_flights.acquire(digests[count - 1]).await;
    })
    .await
    .unwrap();
    assert!(owners[..count - 1]
        .iter()
        .all(|owner| Arc::strong_count(owner) == 2));
    assert_eq!(
        cache.batch_downloads.slots.available_permits(),
        MAX_PENDING - count + 1
    );
    assert!(!task.is_finished());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    released(&cache).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while owners.iter().any(|owner| Arc::strong_count(owner) != 1) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(cache.batch_downloads.slots.available_permits(), MAX_PENDING);
}

#[tokio::test]
async fn batch_rpc_failures_keep_their_meaning_and_unsupported_servers_stream() {
    for code in [
        Code::Unavailable,
        Code::PermissionDenied,
        Code::NotFound,
        Code::Unimplemented,
    ] {
        let (_temp, server, cache, bodies) = fixture(2, 0).await;
        server
            .fail("batch_read", vec![Status::new(code, "injected"); 5])
            .await;
        let result = cache.ensure_blob_local(&Digest::of_bytes(&bodies[0])).await;
        if code == Code::Unimplemented {
            result.unwrap();
            cache
                .ensure_blob_local(&Digest::of_bytes(&bodies[1]))
                .await
                .unwrap();
            assert_eq!(server.calls("batch_read").await, 1);
            assert_eq!(server.calls("read").await, 2);
            assert_eq!(
                cache
                    .local
                    .get_blob(&Digest::of_bytes(&bodies[0]))
                    .await
                    .unwrap(),
                bodies[0]
            );
            assert_eq!(
                cache
                    .local
                    .get_blob(&Digest::of_bytes(&bodies[1]))
                    .await
                    .unwrap(),
                bodies[1]
            );
        } else {
            let expected = match code {
                Code::Unavailable => ErrorKind::Unavailable,
                Code::PermissionDenied => ErrorKind::Refused,
                _ => ErrorKind::Protocol,
            };
            assert_eq!(result.unwrap_err().kind(), expected);
            assert!(!cache
                .local
                .has_blob(&Digest::of_bytes(&bodies[0]))
                .await
                .unwrap());
            assert_eq!(server.calls("read").await, 0);
        }
        released(&cache).await;
    }
}
