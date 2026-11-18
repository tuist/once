use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::staging::{CapturePermit, StagingPermit, StagingPool};
use super::{Executor, RemoteIoLimits, ResourceDomain};
use crate::resources::ResourcePermit;
use crate::{ResourcePool, ResourceRequest};

const MATERIALIZATION_MEMORY_BYTES: u64 = 16 * 1024 * 1024;

/// Shared admission for a host and a bounded remote-request domain.
///
/// Clones share both budgets. Every hosted adapter and provider alias shares
/// the remote quota; this is a client bound, not a claim about fleet capacity.
#[derive(Debug, Clone)]
pub struct ExecutorResources {
    host: Arc<ResourcePool>,
    remote: Arc<Semaphore>,
    remote_concurrency: usize,
    remote_configured: bool,
    client_memory_bytes: u64,
    io_limits: RemoteIoLimits,
    staging: StagingPool,
}

impl ExecutorResources {
    pub fn new(host: Arc<ResourcePool>) -> Self {
        let remote_concurrency = host.max_parallel_actions().min(Semaphore::MAX_PERMITS);
        Self {
            host,
            remote: Arc::new(Semaphore::new(remote_concurrency)),
            remote_concurrency,
            remote_configured: false,
            client_memory_bytes: 0,
            io_limits: RemoteIoLimits::default(),
            staging: StagingPool::for_limits(&RemoteIoLimits::default()),
        }
    }

    /// Override outstanding remote requests independently of host CPU slots.
    /// Configure before cloning or admitting work. Zero is raised to one.
    #[must_use]
    pub fn with_remote_concurrency(mut self, count: usize) -> Self {
        self.remote_configured = true;
        self.remote_concurrency = count.clamp(1, Semaphore::MAX_PERMITS);
        self.remote = Arc::new(Semaphore::new(self.remote_concurrency));
        self
    }

    /// Host memory reserved throughout a remote request, including transfers
    /// and output capture. Zero uses the host pool's default action estimate.
    #[must_use]
    pub fn with_client_memory_bytes(mut self, memory_bytes: u64) -> Self {
        self.client_memory_bytes = memory_bytes;
        self
    }

    /// Limit hosted protocol responses and per-action archive staging.
    /// Buffer and entry limits are also bounded by the actual host reservation.
    #[must_use]
    pub fn with_remote_io_limits(mut self, limits: RemoteIoLimits) -> Self {
        self.io_limits = limits;
        self.staging = StagingPool::for_limits(&limits);
        self
    }

    pub(crate) fn with_host(mut self, host: Arc<ResourcePool>) -> Self {
        if !self.remote_configured {
            self.remote_concurrency = host.max_parallel_actions().min(Semaphore::MAX_PERMITS);
            self.remote = Arc::new(Semaphore::new(self.remote_concurrency));
        }
        self.host = host;
        self
    }

    pub fn host(&self) -> &Arc<ResourcePool> {
        &self.host
    }

    pub fn remote_concurrency(&self) -> usize {
        self.remote_concurrency
    }

    pub async fn acquire(&self, executor: Executor, request: &ResourceRequest) -> ExecutionPermit {
        let (remote, host) = match executor.resource_domain() {
            ResourceDomain::Host => (None, self.host.acquire(request.clone()).await),
            ResourceDomain::Remote => {
                // Remote first: a queued remote request must not pin host memory.
                let remote = Arc::clone(&self.remote)
                    .acquire_owned()
                    .await
                    .expect("executor request semaphore is never closed");
                let host = self
                    .host
                    .acquire_client(self.client_memory_bytes, request.slots.clone())
                    .await;
                (Some(remote), host)
            }
        };
        let permit = ExecutionPermit {
            staging: self.staging.clone(),
            io_limits: self.io_limits.for_memory(host.request().memory_bytes),
            leases: Arc::new(Leases {
                host,
                _remote: remote,
            }),
            owners: Vec::new(),
            executor: Some(executor),
        };
        tracing::debug!(
            executor = executor.name(),
            domain = ?executor.resource_domain(),
            host_cpu_slots = permit.host_request().cpu_slots,
            host_memory_bytes = permit.host_request().memory_bytes,
            "executor admitted"
        );
        permit
    }

    /// Cache lookup and output restoration only need the host, not a worker.
    pub async fn acquire_materialization(&self) -> ExecutionPermit {
        ExecutionPermit {
            staging: self.staging.clone(),
            io_limits: self.io_limits.for_memory(MATERIALIZATION_MEMORY_BYTES),
            leases: Arc::new(Leases {
                host: self
                    .host
                    .acquire_client(MATERIALIZATION_MEMORY_BYTES, BTreeMap::default())
                    .await,
                _remote: None,
            }),
            owners: Vec::new(),
            executor: None,
        }
    }
}

/// Shared admission lease. Cleanup keeps a clone until its resources stop.
#[derive(Debug, Clone)]
pub struct ExecutionPermit {
    leases: Arc<Leases>,
    io_limits: RemoteIoLimits,
    staging: StagingPool,
    owners: Vec<Arc<LifetimeOwner>>,
    executor: Option<Executor>,
}

struct LifetimeOwner {
    _owner: Box<dyn Send + Sync>,
}

impl std::fmt::Debug for LifetimeOwner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LifetimeOwner")
    }
}

impl ExecutionPermit {
    /// Retain a caller-owned lock or guard through physical cleanup.
    /// Attach before cloning; earlier clones do not inherit the owner.
    /// Owners are runtime state and never participate in action identity.
    #[must_use]
    pub fn with_lifetime_owner<T: Send + Sync + 'static>(mut self, owner: T) -> Self {
        self.owners.push(Arc::new(LifetimeOwner {
            _owner: Box::new(owner),
        }));
        self
    }

    /// Run a blocking step with an owned lease, even if its observer is dropped.
    pub async fn run_blocking<T, F>(
        &self,
        work: F,
    ) -> std::result::Result<T, tokio::task::JoinError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        crate::execute::spawn_admitted(Some(self), work).await
    }

    pub(crate) async fn acquire_staging(&self, bytes: u64) -> crate::Result<StagingPermit> {
        self.staging.acquire(bytes).await
    }

    pub(crate) fn capture_budget(&self) -> CapturePermit {
        self.staging.capture()
    }

    pub(crate) fn io_limits(&self) -> RemoteIoLimits {
        self.io_limits
    }

    pub(crate) fn executor(&self) -> Option<Executor> {
        self.executor
    }

    pub(crate) fn scoped_host(&self) -> ResourcePermit {
        self.leases.host.clone().with_lifetime_owner(self.clone())
    }

    pub(crate) fn host_request(&self) -> &ResourceRequest {
        self.leases.host.request()
    }
}

#[derive(Debug)]
struct Leases {
    host: ResourcePermit,
    _remote: Option<OwnedSemaphorePermit>,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::ResourceLimits;

    fn resources(cpu: usize, memory: u64) -> ExecutorResources {
        ExecutorResources::new(Arc::new(ResourcePool::new(
            ResourceLimits::new(cpu, memory).with_default_action_memory_bytes(10),
        )))
    }

    async fn blocked<F: std::future::Future>(future: F) {
        assert!(tokio::time::timeout(Duration::from_millis(20), future)
            .await
            .is_err());
    }

    #[test]
    fn replacing_host_preserves_explicit_remote_and_client_settings() {
        let resources = resources(1, 100)
            .with_remote_concurrency(3)
            .with_client_memory_bytes(7)
            .with_host(Arc::new(ResourcePool::new(ResourceLimits::new(8, 100))));
        assert_eq!(resources.remote_concurrency(), 3);
        assert_eq!(resources.client_memory_bytes, 7);
    }

    #[tokio::test]
    async fn restore_reservation_is_smaller_than_the_action_default() {
        let resources = ExecutorResources::new(Arc::new(ResourcePool::new(ResourceLimits::new(
            1,
            256 * 1024 * 1024,
        ))));
        let first = resources.acquire_materialization().await;
        assert_eq!(first.host_request().cpu_slots, 0);
        assert_eq!(
            first.host_request().memory_bytes,
            MATERIALIZATION_MEMORY_BYTES
        );
        let _second =
            tokio::time::timeout(Duration::from_secs(1), resources.acquire_materialization())
                .await
                .unwrap();
    }

    #[test]
    fn default_remote_quota_preserves_bounded_local_parallelism() {
        let resources = resources(8, 20);
        assert_eq!(resources.remote_concurrency(), 2);
        assert_eq!(
            resources
                .clone()
                .with_remote_concurrency(0)
                .remote_concurrency(),
            1
        );
    }

    #[tokio::test]
    async fn cleanup_clones_retain_all_leases() {
        let resources = resources(1, 100).with_remote_concurrency(1);
        let foreground = resources
            .acquire(Executor::E2b, &ResourceRequest::default())
            .await;
        let cleanup = foreground.clone();
        drop(foreground);
        blocked(resources.acquire(Executor::Daytona, &ResourceRequest::default())).await;
        drop(cleanup);
        let _next = resources
            .acquire(Executor::Daytona, &ResourceRequest::default())
            .await;
    }

    #[tokio::test]
    async fn local_and_virtual_machine_share_capacity() {
        let resources = resources(1, 100);
        let permit = resources
            .acquire(Executor::Local, &ResourceRequest::default())
            .await;
        blocked(resources.acquire(Executor::Microsandbox, &ResourceRequest::default())).await;
        drop(permit);
        let _permit = resources
            .acquire(Executor::Microsandbox, &ResourceRequest::default())
            .await;
    }

    #[tokio::test]
    async fn remote_guests_do_not_reserve_host_compute() {
        let resources = resources(1, 100);
        let remote = resources
            .acquire(Executor::E2b, &ResourceRequest::new(128, 1_000_000))
            .await;
        let _local = tokio::time::timeout(
            Duration::from_secs(1),
            resources.acquire(Executor::Local, &ResourceRequest::new(1, 80)),
        )
        .await
        .unwrap();
        drop(remote);
    }

    #[tokio::test]
    async fn hosted_adapters_and_clones_share_a_quota() {
        let resources = resources(8, 100).with_remote_concurrency(1);
        let permit = resources
            .acquire(Executor::E2b, &ResourceRequest::default())
            .await;
        blocked(
            resources
                .clone()
                .acquire(Executor::Daytona, &ResourceRequest::default()),
        )
        .await;
        drop(permit);
        let _permit = resources
            .acquire(Executor::Daytona, &ResourceRequest::default())
            .await;
    }

    #[tokio::test]
    async fn client_memory_is_still_bounded() {
        let resources = resources(1, 100)
            .with_remote_concurrency(8)
            .with_client_memory_bytes(60);
        let permit = resources
            .acquire(Executor::E2b, &ResourceRequest::default())
            .await;
        blocked(resources.acquire(Executor::Daytona, &ResourceRequest::default())).await;
        drop(permit);
        let _permit = resources
            .acquire(Executor::Daytona, &ResourceRequest::default())
            .await;
    }

    #[tokio::test]
    async fn host_named_slots_keep_their_contract_for_remote_actions() {
        let resources = resources(8, 100);
        let request = ResourceRequest::default().with_slot("device", 1);
        let permit = resources.acquire(Executor::E2b, &request).await;
        blocked(resources.acquire(Executor::Local, &request)).await;
        drop(permit);
        let _permit = resources.acquire(Executor::Local, &request).await;
    }

    #[tokio::test]
    async fn cache_materialization_does_not_need_remote_capacity() {
        let resources = resources(1, 32 * 1024 * 1024).with_remote_concurrency(1);
        let _remote = resources
            .acquire(Executor::E2b, &ResourceRequest::default())
            .await;
        let _restore =
            tokio::time::timeout(Duration::from_secs(1), resources.acquire_materialization())
                .await
                .unwrap();
    }

    #[tokio::test]
    async fn cancellation_while_waiting_for_host_releases_remote_quota() {
        let resources = resources(1, 100).with_remote_concurrency(1);
        let host = resources.host.acquire(ResourceRequest::new(1, 100)).await;
        let other = resources.clone();
        let waiting = tokio::spawn(async move {
            other
                .acquire(Executor::E2b, &ResourceRequest::default())
                .await
        });
        tokio::task::yield_now().await;
        waiting.abort();
        assert!(waiting.await.unwrap_err().is_cancelled());
        drop(host);
        let _permit = tokio::time::timeout(
            Duration::from_secs(1),
            resources.acquire(Executor::E2b, &ResourceRequest::default()),
        )
        .await
        .unwrap();
    }
}
