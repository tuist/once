//! Remote transfers draw from a shared byte budget: disk is reserved before a
//! download streams, capacity returns on every exit path, and a budget too
//! small for a request refuses it instead of deadlocking.

use super::fault_server::{connected_cache, FaultServer};
use super::fault_tests::publish_large;
use super::*;

const LARGE: u64 = 2 * download::IN_MEMORY_BLOB_LIMIT as u64;
const LARGE_STAGING: u64 = stream_limits::staging_bytes(LARGE);

async fn served_large() -> (tempfile::TempDir, FaultServer, String, Digest, Vec<u8>) {
    let temp = tempfile::TempDir::new().unwrap();
    let server = FaultServer::default();
    let url = server.serve().await;
    let (digest, bytes) = publish_large(&temp, &url).await;
    (temp, server, url, digest, bytes)
}

async fn reader(
    temp: &tempfile::TempDir,
    name: &str,
    url: &str,
    budget: &TransferBudget,
) -> TuistCache {
    let mut cache = connected_cache(temp, name, url).await;
    cache.set_transfer_budget(budget.clone());
    cache
}

fn scratch_is_empty(cache: &TuistCache) -> bool {
    let scratch = cache.local.scratch_dir();
    !scratch.exists() || std::fs::read_dir(scratch).unwrap().count() == 0
}

fn full(budget: &TransferBudget) -> (u64, u64) {
    (budget.memory_bytes(), budget.staging_bytes())
}

#[tokio::test]
async fn staging_is_reserved_before_a_download_opens_its_stream() {
    let (temp, server, url, digest, _) = served_large().await;
    // Leave out a full unit so the staging pool is one unit short.
    let budget = TransferBudget::new(64 * 1024 * 1024, LARGE - 4096);
    let cache = reader(&temp, "small-staging", &url, &budget).await;

    let error = cache.ensure_blob_local(&digest).await.unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Exhausted, "{error}");
    assert_eq!(server.calls("read").await, 0, "no bytes were requested");
    assert!(!cache.local.has_blob(&digest).await.unwrap());
    assert_eq!(budget.available(), full(&budget));
}

#[tokio::test]
async fn short_and_long_streams_fail_their_size_check_and_return_their_budget() {
    for (replacement, suffix) in [
        (
            Some(vec![1_u8; usize::try_from(LARGE / 2).unwrap()]),
            Vec::new(),
        ),
        (None, vec![0_u8; 4096]),
    ] {
        let (temp, server, url, digest, _) = served_large().await;
        {
            let mut state = server.0.lock().await;
            state.read_replacement = replacement;
            state.read_suffix = suffix;
        }
        let budget = TransferBudget::new(64 * 1024 * 1024, LARGE_STAGING);
        let cache = reader(&temp, "mismatch", &url, &budget).await;

        let error = cache.ensure_blob_local(&digest).await.unwrap_err();

        assert!(error.is_corrupt(), "{error}");
        assert!(!cache.local.has_blob(&digest).await.unwrap());
        assert!(scratch_is_empty(&cache));
        assert_eq!(budget.available(), full(&budget));
    }
}

#[tokio::test]
async fn cancelled_concurrent_downloads_release_their_budget_and_scratch() {
    let (temp, server, url, digest, bytes) = served_large().await;
    server.0.lock().await.read_stalls = true;
    // Staging fits one download at a time, so the others queue for it.
    let budget = TransferBudget::new(64 * 1024 * 1024, LARGE_STAGING);
    let mut caches = Vec::new();
    for index in 0..4 {
        caches.push(reader(&temp, &format!("cancel-{index}"), &url, &budget).await);
    }
    let tasks: Vec<_> = caches
        .iter()
        .map(|cache| {
            let cache = cache.clone();
            tokio::spawn(async move { cache.ensure_blob_local(&digest).await })
        })
        .collect();
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.calls("read").await < 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the admitted download opens its stream");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        server.calls("read").await,
        1,
        "staging admits one at a time"
    );
    assert_eq!(budget.available().1, 0);

    for task in &tasks {
        task.abort();
    }
    for task in tasks {
        assert!(task.await.unwrap_err().is_cancelled());
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while budget.available() != full(&budget) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("physical ingestion releases its budget after cancellation");
    assert!(caches.iter().all(scratch_is_empty));

    server.0.lock().await.read_stalls = false;
    caches[0].ensure_blob_local(&digest).await.unwrap();
    assert_eq!(caches[0].local.get_blob(&digest).await.unwrap(), bytes);
    assert_eq!(budget.available(), full(&budget));
}

#[tokio::test]
async fn an_empty_memory_budget_fails_before_any_rpc() {
    let (temp, server, url, digest, _) = served_large().await;
    let before = (
        server.calls("get_action").await,
        server.calls("find_missing").await,
    );
    let budget = TransferBudget::new(1, LARGE * 8);
    let cache = reader(&temp, "empty-memory", &url, &budget).await;

    let error = cache.has_blob(&digest).await.unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Exhausted, "{error}");
    assert_eq!(
        (
            server.calls("get_action").await,
            server.calls("find_missing").await
        ),
        before,
        "no RPC was sent"
    );
}

#[tokio::test]
async fn a_budget_below_a_streams_buffer_fails_instead_of_overrunning_it() {
    let (temp, server, url, digest, _) = served_large().await;
    // Room for a metadata response but not for one stream message of this
    // blob plus its framing and pipe buffer.
    let budget = TransferBudget::new(METADATA_RESPONSE_LIMIT as u64, LARGE * 8);
    let cache = reader(&temp, "small-memory", &url, &budget).await;

    assert!(cache.has_blob(&digest).await.unwrap());
    let error = cache.get_blob(&digest).await.unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Exhausted, "{error}");
    assert_eq!(server.calls("read").await, 0, "no stream was opened");
    assert!(scratch_is_empty(&cache));
    assert_eq!(budget.available(), full(&budget));
}

/// Publishes a blob that streams as several full messages and whose stream
/// message cap is the transport-wide maximum.
async fn served_beyond_one_message() -> (tempfile::TempDir, FaultServer, String, Digest, Vec<u8>) {
    let temp = tempfile::TempDir::new().unwrap();
    let server = FaultServer::default();
    let url = server.serve().await;
    let size = usize::try_from(crate::budget::MAX_STREAM_MESSAGE_BYTES).unwrap() + 1024 * 1024;
    let bytes: Vec<u8> = (0..size)
        .map(|index| u8::try_from(index % 253).unwrap())
        .collect();
    let writer = connected_cache(&temp, "huge-writer", &url).await;
    let digest = writer.local.put_blob(&bytes).await.unwrap();
    writer.put_blob_remote(&digest, &bytes).await.unwrap();
    (temp, server, url, digest, bytes)
}

#[tokio::test]
async fn the_minimum_budget_downloads_a_blob_larger_than_one_stream_message() {
    let (temp, server, url, digest, bytes) = served_beyond_one_message().await;
    let budget = TransferBudget::new(
        crate::MIN_TRANSFER_MEMORY_BYTES,
        stream_limits::staging_bytes(bytes.len() as u64),
    );
    assert_eq!(budget.memory_bytes(), crate::MIN_TRANSFER_MEMORY_BYTES);
    let cache = reader(&temp, "minimum", &url, &budget).await;

    assert!(cache.has_blob(&digest).await.unwrap());
    assert!(cache
        .get_action_result(&Digest::of_bytes(b"absent"))
        .await
        .unwrap()
        .is_none());
    assert_eq!(cache.get_blob(&digest).await.unwrap(), bytes);

    assert_eq!(server.calls("read").await, 1);
    assert_eq!(budget.available(), full(&budget));
}

#[tokio::test]
async fn one_unit_below_the_minimum_refuses_that_download_before_reading() {
    let (temp, server, url, digest, bytes) = served_beyond_one_message().await;
    // Not a whole unit short: the capacity rounds down to one unit less.
    let budget = TransferBudget::new(
        crate::MIN_TRANSFER_MEMORY_BYTES - 1,
        stream_limits::staging_bytes(bytes.len() as u64),
    );
    assert_eq!(
        budget.memory_bytes(),
        crate::MIN_TRANSFER_MEMORY_BYTES - 4096
    );
    let cache = reader(&temp, "below-minimum", &url, &budget).await;

    assert!(cache.has_blob(&digest).await.unwrap());
    let error = cache.get_blob(&digest).await.unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Exhausted, "{error}");
    assert!(!error.is_not_found());
    assert_eq!(server.calls("read").await, 0, "no stream was opened");
    assert!(scratch_is_empty(&cache));
    assert_eq!(budget.available(), full(&budget));
}

#[tokio::test]
async fn concurrent_work_queues_on_a_budget_that_fits_one_transfer() {
    let (temp, server, url, digest, bytes) = served_large().await;
    let one_stream = crate::MIN_TRANSFER_MEMORY_BYTES;
    let budget = TransferBudget::new(one_stream, LARGE_STAGING);
    let readers = futures::future::join_all((0..6).map(|index| {
        let (temp, url, budget) = (&temp, &url, &budget);
        async move { reader(temp, &format!("queued-{index}"), url, budget).await }
    }))
    .await;
    let work = futures::future::join_all(readers.iter().map(|cache| async move {
        assert!(cache.has_blob(&digest).await.unwrap());
        cache.get_blob(&digest).await.unwrap()
    }));

    let fetched = tokio::time::timeout(Duration::from_secs(20), work)
        .await
        .expect("transfers queue for the budget instead of deadlocking");

    assert!(fetched.iter().all(|blob| *blob == bytes));
    assert_eq!(server.calls("read").await, 6);
    assert_eq!(budget.available(), full(&budget));
}

#[tokio::test]
async fn cloned_providers_share_the_budget_they_were_given() {
    let temp = tempfile::TempDir::new().unwrap();
    let budget = TransferBudget::new(1024 * 1024, 1024 * 1024);
    let provider = crate::CacheProvider::Tuist(
        connected_cache(&temp, "shared", &FaultServer::default().serve().await).await,
    )
    .with_transfer_budget(budget.clone());
    let clone = provider.clone();

    let _held = clone
        .transfer_budget()
        .unwrap()
        .reserve_staging(1024 * 1024)
        .await
        .unwrap();

    assert_eq!(provider.transfer_budget().unwrap().available().1, 0);
    assert_eq!(budget.available().1, 0);
    assert!(crate::CacheProvider::open_local(temp.path())
        .with_transfer_budget(budget)
        .transfer_budget()
        .is_none());
}
