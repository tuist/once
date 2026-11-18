//! Byte budgets for moving blobs between a remote tier and the local store.
//!
//! A [`TransferBudget`] holds two pools: memory that in-flight transfers and
//! cache RPCs may buffer, and scratch disk that downloads may stage before
//! they are verified. Every remote transfer reserves the most its buffers can
//! hold from the relevant pool before it starts and releases it when it
//! finishes, fails, or is cancelled. Providers cloned from one another share
//! the same pools.
//!
//! A reservation is a requirement, not an estimate: the buffer limits a
//! transfer runs with are the ones it reserved, and a transfer whose limits
//! do not fit the whole pool fails with [`ErrorKind::Exhausted`] before it
//! sends anything. Capacities round down to the accounting unit and requests
//! round up, so a budget never admits more than it was configured with.
//!
//! The pools account for the buffers this crate sizes for transfers and RPC
//! responses. They are not a bound on process memory: connection setup,
//! authentication, capability discovery, transport internals, and allocator
//! overhead are outside them, and they are not coordinated with other
//! memory a process uses unless the caller carves them out of its own
//! budget.

use std::fmt;
use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{Error, ErrorKind, Result};

/// Pools are counted in units of this many bytes. Capacities round down and
/// requests round up.
const UNIT: u64 = 4 * 1024;

/// Largest single `ByteStream` message a download decodes. Larger blobs
/// arrive as several messages, each at most this size.
pub(crate) const MAX_STREAM_MESSAGE_BYTES: u64 = 32 * 1024 * 1024;
/// In-process pipe between a download and the file it stages into.
pub(crate) const STREAM_PIPE_BYTES: u64 = 64 * 1024;

/// The smallest memory budget with which every remote operation can run:
/// one full-size stream message plus its pipe, which is what downloading a
/// large blob reserves. Metadata calls, small blob reads, and streamed
/// uploads from files all reserve less. A smaller budget makes the
/// operations that need more fail as exhausted instead of running.
pub const MIN_TRANSFER_MEMORY_BYTES: u64 = MAX_STREAM_MESSAGE_BYTES + STREAM_PIPE_BYTES;

const _: () = assert!(MIN_TRANSFER_MEMORY_BYTES.is_multiple_of(UNIT));

/// Memory remote transfers may buffer when no budget is configured.
pub const DEFAULT_TRANSFER_MEMORY_BYTES: u64 = 256 * 1024 * 1024;
/// Scratch disk downloads may stage when no budget is configured. A single
/// blob larger than the staging pool cannot be downloaded.
pub const DEFAULT_TRANSFER_STAGING_BYTES: u64 = 32 * 1024 * 1024 * 1024;

/// Memory and scratch disk budgets for remote cache transfers.
///
/// Cloning shares the pools. Pass one budget to every provider that should
/// draw from the same allowance.
#[derive(Clone)]
pub struct TransferBudget {
    memory: Pool,
    staging: Pool,
}

impl TransferBudget {
    /// A budget allowing `memory_bytes` of transfer buffers and
    /// `staging_bytes` of downloads staged on disk at once, each rounded down
    /// to a 4 KiB unit. A pool smaller than one unit admits nothing, so every
    /// transfer that needs it fails as exhausted.
    pub fn new(memory_bytes: u64, staging_bytes: u64) -> Self {
        Self {
            memory: Pool::new("memory", memory_bytes),
            staging: Pool::new("staging", staging_bytes),
        }
    }

    /// Bytes of memory the budget admits after rounding down.
    pub fn memory_bytes(&self) -> u64 {
        self.memory.capacity_bytes()
    }

    /// Bytes of scratch disk the budget admits after rounding down.
    pub fn staging_bytes(&self) -> u64 {
        self.staging.capacity_bytes()
    }

    /// Reserve the memory a transfer's buffers can hold. A request larger
    /// than the whole pool fails without waiting: running it anyway would let
    /// its buffers exceed the budget.
    pub(crate) async fn reserve_memory(&self, bytes: u64) -> Result<Reservation> {
        self.memory.reserve(bytes).await
    }

    /// Reserve scratch disk for a download of `bytes`. A request larger than
    /// the pool fails, so a size the remote declares cannot decide how much
    /// disk is written.
    pub(crate) async fn reserve_staging(&self, bytes: u64) -> Result<Reservation> {
        self.staging.reserve(bytes).await
    }

    #[cfg(test)]
    pub(crate) fn available(&self) -> (u64, u64) {
        (
            self.memory.available_bytes(),
            self.staging.available_bytes(),
        )
    }
}

impl Default for TransferBudget {
    fn default() -> Self {
        Self::new(
            DEFAULT_TRANSFER_MEMORY_BYTES,
            DEFAULT_TRANSFER_STAGING_BYTES,
        )
    }
}

impl fmt::Debug for TransferBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransferBudget")
            .field("memory_bytes", &self.memory_bytes())
            .field("staging_bytes", &self.staging_bytes())
            .finish()
    }
}

/// Capacity held by one transfer. Dropping it, including when the transfer
/// is cancelled, returns the capacity.
#[derive(Debug)]
pub(crate) struct Reservation {
    _permit: Option<OwnedSemaphorePermit>,
}

#[derive(Clone)]
struct Pool {
    name: &'static str,
    units: Arc<Semaphore>,
    capacity: u32,
}

impl Pool {
    fn new(name: &'static str, bytes: u64) -> Self {
        let capacity = u32::try_from(bytes / UNIT).unwrap_or(u32::MAX);
        Self {
            name,
            units: Arc::new(Semaphore::new(capacity as usize)),
            capacity,
        }
    }

    fn capacity_bytes(&self) -> u64 {
        u64::from(self.capacity) * UNIT
    }

    #[cfg(test)]
    fn available_bytes(&self) -> u64 {
        self.units.available_permits() as u64 * UNIT
    }

    /// The semaphore is fair and grants a request all at once, so a large
    /// request cannot be starved by smaller ones and a caller never holds
    /// part of what it asked for while waiting for the rest.
    async fn reserve(&self, bytes: u64) -> Result<Reservation> {
        let wanted = bytes.div_ceil(UNIT);
        if wanted == 0 {
            return Ok(Reservation { _permit: None });
        }
        if wanted > u64::from(self.capacity) {
            return Err(Error::remote(
                "transfer budget",
                self.operation(),
                ErrorKind::Exhausted,
                format!(
                    "{bytes} bytes of {} exceed the {}-byte budget",
                    self.name,
                    self.capacity_bytes()
                ),
            ));
        }
        let wanted = u32::try_from(wanted).expect("bounded by a u32 capacity");
        let permit = Arc::clone(&self.units)
            .acquire_many_owned(wanted)
            .await
            .expect("transfer budget pools are never closed");
        tracing::trace!(pool = self.name, bytes, "reserved transfer budget");
        Ok(Reservation {
            _permit: Some(permit),
        })
    }

    fn operation(&self) -> &'static str {
        match self.name {
            "memory" => "reserve transfer memory",
            _ => "reserve staging space",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn reservations_return_their_capacity_when_dropped_or_cancelled() {
        let budget = TransferBudget::new(64 * UNIT, 64 * UNIT);
        let held = budget.reserve_memory(60 * UNIT).await.unwrap();
        let staged = budget.reserve_staging(10 * UNIT).await.unwrap();
        assert_eq!(budget.available(), (4 * UNIT, 54 * UNIT));

        let waiting = {
            let budget = budget.clone();
            tokio::spawn(async move { budget.reserve_memory(10 * UNIT).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        waiting.abort();
        let _ = waiting.await;
        drop((held, staged));
        assert_eq!(budget.available(), (64 * UNIT, 64 * UNIT));
    }

    #[tokio::test]
    async fn requests_larger_than_the_pool_fail_without_waiting() {
        let budget = TransferBudget::new(8 * UNIT, 16 * UNIT);
        for error in [
            budget.reserve_memory(8 * UNIT + 1).await.unwrap_err(),
            budget.reserve_staging(17 * UNIT).await.unwrap_err(),
        ] {
            assert_eq!(error.kind(), ErrorKind::Exhausted, "{error}");
            assert!(!error.is_not_found());
        }
        assert_eq!(budget.available(), (8 * UNIT, 16 * UNIT));
    }

    #[tokio::test]
    async fn capacities_round_down_and_requests_round_up() {
        let budget = TransferBudget::new(2 * UNIT - 1, 2 * UNIT - 1);
        assert_eq!(budget.memory_bytes(), UNIT);
        assert_eq!(budget.staging_bytes(), UNIT);
        drop(budget.reserve_memory(UNIT).await.unwrap());
        drop(budget.reserve_memory(1).await.unwrap());
        assert_eq!(
            budget.reserve_memory(UNIT + 1).await.unwrap_err().kind(),
            ErrorKind::Exhausted
        );
        assert_eq!(
            budget
                .reserve_staging(2 * UNIT - 1)
                .await
                .unwrap_err()
                .kind(),
            ErrorKind::Exhausted
        );
    }

    #[tokio::test]
    async fn an_empty_pool_admits_nothing() {
        for bytes in [0, 1, UNIT - 1] {
            let budget = TransferBudget::new(bytes, bytes);
            assert_eq!((budget.memory_bytes(), budget.staging_bytes()), (0, 0));
            assert_eq!(
                budget.reserve_memory(1).await.unwrap_err().kind(),
                ErrorKind::Exhausted
            );
            assert_eq!(
                budget.reserve_staging(1).await.unwrap_err().kind(),
                ErrorKind::Exhausted
            );
            assert!(budget.reserve_memory(0).await.is_ok());
        }
    }

    #[tokio::test]
    async fn a_large_waiter_is_not_starved_by_later_small_requests() {
        let budget = TransferBudget::new(4 * UNIT, UNIT);
        let first = budget.reserve_memory(2 * UNIT).await.unwrap();
        let large = {
            let budget = budget.clone();
            tokio::spawn(async move { drop(budget.reserve_memory(4 * UNIT).await.unwrap()) })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        let later = {
            let budget = budget.clone();
            tokio::spawn(async move { drop(budget.reserve_memory(UNIT).await.unwrap()) })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !later.is_finished(),
            "fair queue: the earlier large request goes first"
        );
        drop(first);
        large.await.unwrap();
        later.await.unwrap();
    }

    #[tokio::test]
    async fn clones_share_one_pool() {
        let budget = TransferBudget::new(8 * UNIT, 8 * UNIT);
        let _held = budget.clone().reserve_staging(8 * UNIT).await.unwrap();
        assert_eq!(budget.available().1, 0);
        assert_eq!(budget.memory_bytes(), 8 * UNIT);
    }
}
