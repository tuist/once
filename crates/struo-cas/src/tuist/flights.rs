use std::collections::HashMap;
use std::sync::{Arc, Weak};

use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::Digest;

#[derive(Debug, Default)]
pub(super) struct Flights {
    active: Mutex<HashMap<Digest, Weak<Mutex<()>>>>,
}

impl Flights {
    pub(super) async fn acquire(&self, digest: Digest) -> OwnedMutexGuard<()> {
        let lock = {
            let mut active = self.active.lock().await;
            active.retain(|_, lock| lock.strong_count() > 0);
            if let Some(lock) = active.get(&digest).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(Mutex::new(()));
                active.insert(digest, Arc::downgrade(&lock));
                lock
            }
        };
        lock.lock_owned().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn same_digest_waits_and_other_digests_remain_independent() {
        let flights = Flights::default();
        let digest = Digest::of_bytes(b"same");
        let first = flights.acquire(digest).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(10), flights.acquire(digest))
                .await
                .is_err()
        );
        let other = tokio::time::timeout(
            Duration::from_secs(1),
            flights.acquire(Digest::of_bytes(b"other")),
        )
        .await
        .unwrap();
        drop(first);
        let second = tokio::time::timeout(Duration::from_secs(1), flights.acquire(digest))
            .await
            .unwrap();
        drop((second, other));
    }

    #[tokio::test]
    async fn completed_and_cancelled_flights_do_not_accumulate() {
        let flights = Flights::default();
        for index in 0..1_000_u32 {
            drop(
                flights
                    .acquire(Digest::of_bytes(&index.to_le_bytes()))
                    .await,
            );
            assert_eq!(flights.active.lock().await.len(), 1);
        }
        let digest = Digest::of_bytes(b"cancelled");
        let held = flights.acquire(digest).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(10), flights.acquire(digest))
                .await
                .is_err()
        );
        drop(held);
        drop(flights.acquire(digest).await);
        assert_eq!(flights.active.lock().await.len(), 1);
    }
}
