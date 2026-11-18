use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use serde::Deserialize;

use super::process::{error, Client};
use crate::resources::ResourcePermit;
use crate::{ExecutionPermit, ResourceLimits, ResourcePool, Result};

#[derive(Deserialize)]
struct Capacity {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "NCPU")]
    cpus: usize,
    #[serde(rename = "MemTotal")]
    memory: u64,
}

impl Capacity {
    fn parse(bytes: &[u8]) -> Result<Self> {
        let capacity: Self = serde_json::from_slice(bytes)
            .map_err(|_| error("invalid Docker daemon capacity response"))?;
        if capacity.id.is_empty()
            || capacity.id.len() > 256
            || !capacity
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":-_.".contains(&byte))
            || capacity.cpus == 0
            || capacity.memory < 3
        {
            return Err(error(
                "Docker daemon identity, CPU and memory capacity must be known",
            ));
        }
        Ok(capacity)
    }

    fn limits(&self) -> ResourceLimits {
        ResourceLimits::new(self.cpus, self.memory / 3 * 2 + self.memory % 3 * 2 / 3)
    }
}

#[derive(Default)]
struct Pools(Mutex<BTreeMap<String, Weak<ResourcePool>>>);

impl Pools {
    fn get(&self, capacity: &Capacity) -> Result<Arc<ResourcePool>> {
        let mut pools = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pools.retain(|_, pool| pool.strong_count() > 0);
        let limits = capacity.limits();
        if let Some(pool) = pools.get(&capacity.id).and_then(Weak::upgrade) {
            if pool.limits() != limits {
                return Err(error(
                    "Docker daemon capacity changed while reservations are active",
                ));
            }
            return Ok(pool);
        }
        let pool = Arc::new(ResourcePool::new(limits));
        pools.insert(capacity.id.clone(), Arc::downgrade(&pool));
        Ok(pool)
    }
}

async fn read(client: &Client) -> Result<Capacity> {
    let bytes = client
        .control(&[
            "info".into(),
            "--format".into(),
            "{\"ID\":{{json .ID}},\"NCPU\":{{json .NCPU}},\"MemTotal\":{{json .MemTotal}}}".into(),
        ])
        .await?;
    Capacity::parse(&bytes)
}

pub(super) async fn identity(client: &Client) -> Result<String> {
    Ok(read(client).await?.id)
}

pub(super) async fn pool(client: &Client) -> Result<Arc<ResourcePool>> {
    static POOLS: OnceLock<Pools> = OnceLock::new();
    let capacity = read(client).await?;
    client.check_identity(&capacity.id)?;
    POOLS.get_or_init(Pools::default).get(&capacity)
}

pub(super) async fn admit(
    client: &Client,
    lease: ExecutionPermit,
) -> Result<(ExecutionPermit, ResourcePermit)> {
    let workers = pool(client).await?;
    let mut request = lease.host_request().clone();
    request.slots.clear();
    let worker = workers.acquire(request).await;
    Ok((lease.with_lifetime_owner(worker.clone()), worker))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::ResourceRequest;

    #[test]
    fn unknown_or_invalid_capacity_fails_closed() {
        for bytes in [
            "{}",
            "{\"ID\":\"\",\"NCPU\":8,\"MemTotal\":8319410176}",
            "{\"ID\":\"daemon\",\"NCPU\":0,\"MemTotal\":8319410176}",
            "{\"ID\":\"daemon\",\"NCPU\":8,\"MemTotal\":0}",
            "{\"ID\":\"daemon\",\"NCPU\":-1,\"MemTotal\":8319410176}",
            "{\"ID\":\"daemon\",\"NCPU\":8,\"MemTotal\":null}",
        ] {
            assert!(Capacity::parse(bytes.as_bytes()).is_err(), "{bytes}");
        }
    }

    #[tokio::test]
    async fn daemon_aliases_share_admission_and_oversized_requests_are_normalized() {
        let pools = Pools::default();
        let capacity = Capacity {
            id: "same-daemon-through-different-contexts".into(),
            cpus: 8,
            memory: 8_319_410_176,
        };
        let first_alias = pools.get(&capacity).unwrap();
        let second_alias = pools.get(&capacity).unwrap();
        assert!(Arc::ptr_eq(&first_alias, &second_alias));
        assert_eq!(first_alias.limits().memory_bytes, 5_546_273_450);
        let request = ResourceRequest::new(1, 4 * 1024 * 1024 * 1024);
        let first = first_alias.acquire(request.clone()).await;
        let mut second = Box::pin(second_alias.acquire(request.clone()));
        let mut third = Box::pin(first_alias.acquire(request));
        assert!(tokio::time::timeout(Duration::from_millis(20), &mut second)
            .await
            .is_err());
        assert!(tokio::time::timeout(Duration::from_millis(20), &mut third)
            .await
            .is_err());
        drop(first);
        let second = tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(20), &mut third)
            .await
            .is_err());
        drop(second);
        drop(
            tokio::time::timeout(Duration::from_secs(1), third)
                .await
                .unwrap(),
        );
        let oversized = first_alias
            .acquire(ResourceRequest::new(100, u64::MAX))
            .await;
        assert_eq!(oversized.request().cpu_slots, 8);
        assert_eq!(oversized.request().memory_bytes, 5_546_273_450);
        let changed = Capacity {
            memory: capacity.memory / 2,
            ..capacity
        };
        assert!(pools.get(&changed).is_err());
    }

    #[tokio::test]
    async fn daemon_cpu_slots_are_admitted_independently_of_host_cpus() {
        let pools = Pools::default();
        let workers = pools
            .get(&Capacity {
                id: "cpu-test".into(),
                cpus: 2,
                memory: 3000,
            })
            .unwrap();
        let first = workers.acquire(ResourceRequest::new(2, 1)).await;
        assert!(tokio::time::timeout(
            Duration::from_millis(20),
            workers.acquire(ResourceRequest::new(1, 1))
        )
        .await
        .is_err());
        drop(first);
        let _next = workers.acquire(ResourceRequest::new(1, 1)).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn client_aliases_resolve_the_same_daemon_pool() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("docker");
        let alias = root.path().join("docker-alias");
        std::fs::write(&binary, b"#!/bin/sh\nprintf '%s\\n' '{\"ID\":\"client-aliases-test\",\"NCPU\":8,\"MemTotal\":8319410176}'\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        symlink(&binary, &alias).unwrap();
        let client = Client::new(binary.into(), crate::RemoteIoLimits::default())
            .identify()
            .await
            .unwrap();
        let alias = Client::new(alias.into(), client.limits)
            .identify()
            .await
            .unwrap();
        assert_eq!(
            crate::remote::lifecycle::Provider::scope(&client),
            crate::remote::lifecycle::Provider::scope(&alias)
        );
        let (first, second) = tokio::try_join!(pool(&client), pool(&alias)).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let held = first
            .acquire(ResourceRequest::new(1, 4 * 1024 * 1024 * 1024))
            .await;
        assert!(tokio::time::timeout(
            Duration::from_millis(20),
            second.acquire(ResourceRequest::new(1, 4 * 1024 * 1024 * 1024))
        )
        .await
        .is_err());
        drop(held);
        let host = crate::ExecutorResources::new(Arc::new(ResourcePool::new(ResourceLimits::new(
            2,
            1024 * 1024 * 1024,
        ))));
        let lease = host
            .acquire(
                crate::Executor::Docker,
                &ResourceRequest::new(128, u64::MAX).with_slot("exclusive", 1),
            )
            .await;
        let (lease, worker) = admit(&alias, lease).await.unwrap();
        assert_eq!(worker.request().memory_bytes, 1024 * 1024 * 1024);
        assert_eq!(worker.request().cpu_slots, 2);
        assert!(worker.request().slots.is_empty());
        assert_eq!(lease.host_request().slots.get("exclusive"), Some(&1));
        let cleanup = lease.clone();
        drop(lease);
        drop(worker);
        assert!(tokio::time::timeout(
            Duration::from_millis(20),
            first.acquire(ResourceRequest::new(8, u64::MAX))
        )
        .await
        .is_err());
        drop(cleanup);
        let _next = first.acquire(ResourceRequest::new(8, u64::MAX)).await;
    }
}
