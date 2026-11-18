use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Semaphore;

use super::*;

fn outage() -> Error {
    Error::remote(
        "tuist",
        "connect endpoint",
        ErrorKind::Unavailable,
        "refused",
    )
}

fn cell() -> Arc<Recovering<u32>> {
    Arc::new(Recovering::default())
}

/// An `init` that counts its calls and returns `outcome` once `gate` grants a
/// permit, so a test controls when the backend answers.
fn gated(
    calls: &Arc<AtomicUsize>,
    gate: &Arc<Semaphore>,
    outcome: Result<u32>,
) -> impl FnOnce() -> std::pin::Pin<Box<dyn Future<Output = Result<u32>> + Send>> {
    let calls = Arc::clone(calls);
    let gate = Arc::clone(gate);
    move || {
        calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            gate.acquire().await.unwrap().forget();
            outcome
        })
    }
}

fn must_not_run() -> std::pin::Pin<Box<dyn Future<Output = Result<u32>> + Send>> {
    panic!("a waiter must not start a second probe")
}

async fn until(calls: &AtomicUsize, expected: usize) {
    while calls.load(Ordering::SeqCst) < expected {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn transient_failures_recover_after_a_cooldown() {
    let cell = cell();
    let calls = Arc::new(AtomicUsize::new(0));
    let open = Arc::new(Semaphore::new(Semaphore::MAX_PERMITS));

    let first = cell
        .get_or_try_init(gated(&calls, &open, Err(outage())))
        .await
        .unwrap_err();
    assert!(first.is_unavailable());
    let replayed = cell
        .get_or_try_init(gated(&calls, &open, Ok(1)))
        .await
        .unwrap_err();
    assert!(replayed.is_unavailable(), "{replayed}");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "no probe during cooldown");

    tokio::time::advance(FIRST_COOLDOWN).await;
    assert_eq!(
        *cell
            .get_or_try_init(gated(&calls, &open, Ok(7)))
            .await
            .unwrap(),
        7
    );
    assert_eq!(*cell.get_or_try_init(must_not_run).await.unwrap(), 7);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn repeated_failures_back_off_up_to_a_ceiling() {
    let cell = cell();
    for failures in 1..=8 {
        assert!(cell
            .get_or_try_init(|| async { Err(outage()) })
            .await
            .is_err());
        tokio::time::advance(cooldown(failures).saturating_sub(Duration::from_millis(1))).await;
        assert!(cell.get_or_try_init(must_not_run).await.is_err());
        tokio::time::advance(Duration::from_millis(1)).await;
    }
    assert_eq!(cooldown(8), MAX_COOLDOWN);
}

#[tokio::test(start_paused = true)]
async fn refusals_and_misconfiguration_stay_permanent() {
    for error in [
        Error::remote("tuist", "load auth token", ErrorKind::Refused, "denied"),
        Error::InvalidConfig {
            provider: "tuist",
            message: "run struo auth login".into(),
        },
    ] {
        let kind = error.kind();
        let cell = cell();
        assert!(cell
            .get_or_try_init(|| async move { Err(error) })
            .await
            .is_err());
        tokio::time::advance(MAX_COOLDOWN * 4).await;
        let replayed = cell.get_or_try_init(must_not_run).await.unwrap_err();
        assert_eq!(replayed.kind(), kind);
    }
}

#[tokio::test(start_paused = true)]
async fn concurrent_callers_share_one_probe() {
    let cell = cell();
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(Semaphore::new(0));
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let cell = Arc::clone(&cell);
        let init = gated(&calls, &gate, Err(outage()));
        tasks.push(tokio::spawn(async move {
            cell.get_or_try_init(init).await.copied()
        }));
    }
    until(&calls, 1).await;
    gate.add_permits(16);
    for task in tasks {
        assert!(task.await.unwrap().unwrap_err().is_unavailable());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn cancelling_the_caller_that_started_a_half_open_probe_does_not_cancel_it() {
    let cell = cell();
    let calls = Arc::new(AtomicUsize::new(0));
    let open = Arc::new(Semaphore::new(Semaphore::MAX_PERMITS));
    assert!(cell
        .get_or_try_init(gated(&calls, &open, Err(outage())))
        .await
        .is_err());
    tokio::time::advance(FIRST_COOLDOWN).await;

    // The half-open probe starts and its response is pending.
    let gate = Arc::new(Semaphore::new(0));
    let initiator = {
        let cell = Arc::clone(&cell);
        let init = gated(&calls, &gate, Ok(42));
        tokio::spawn(async move { cell.get_or_try_init(init).await.copied() })
    };
    until(&calls, 2).await;
    let waiters: Vec<_> = (0..8)
        .map(|_| {
            let cell = Arc::clone(&cell);
            tokio::spawn(async move { cell.get_or_try_init(must_not_run).await.copied() })
        })
        .collect();
    tokio::task::yield_now().await;
    initiator.abort();
    assert!(initiator.await.unwrap_err().is_cancelled());

    gate.add_permits(1);
    for waiter in waiters {
        assert_eq!(waiter.await.unwrap().unwrap(), 42);
    }
    assert_eq!(*cell.get_or_try_init(must_not_run).await.unwrap(), 42);
    assert_eq!(calls.load(Ordering::SeqCst), 2, "one failure and one probe");
}

#[tokio::test(start_paused = true)]
async fn a_probe_that_never_answers_fails_at_its_deadline_and_releases_the_slot() {
    let cell = cell();
    let waiter = {
        let cell = Arc::clone(&cell);
        tokio::spawn(async move { cell.get_or_try_init(std::future::pending).await.copied() })
    };
    let error = waiter.await.unwrap().unwrap_err();
    assert!(error.is_unavailable(), "{error}");
    assert!(error.to_string().contains("did not finish"), "{error}");
    tokio::time::advance(FIRST_COOLDOWN).await;
    assert_eq!(*cell.get_or_try_init(|| async { Ok(3) }).await.unwrap(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_panicking_probe_releases_the_slot_for_a_later_probe() {
    let cell = cell();
    let error = cell
        .get_or_try_init(|| async { panic!("probe bug") })
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Other);
    tokio::time::advance(FIRST_COOLDOWN).await;
    assert_eq!(*cell.get_or_try_init(|| async { Ok(5) }).await.unwrap(), 5);
}
