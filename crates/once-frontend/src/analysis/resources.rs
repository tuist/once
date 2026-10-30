use std::future::Future;
use std::sync::{Arc, OnceLock};

use once_core::{ResourceLimits, ResourcePool};

tokio::task_local! {
    static RESOURCES: Arc<ResourcePool>;
}

/// The command's shared pool, or the process-wide default for standalone callers.
pub fn analysis_resource_pool() -> Arc<ResourcePool> {
    static DEFAULT: OnceLock<Arc<ResourcePool>> = OnceLock::new();
    RESOURCES.try_with(Arc::clone).unwrap_or_else(|_| {
        Arc::clone(DEFAULT.get_or_init(|| Arc::new(ResourcePool::new(ResourceLimits::default()))))
    })
}

/// Share admission with the command's execution pool. Engines retain this pool
/// when moved to blocking workers; separately spawned callers must scope it again.
pub async fn with_analysis_resources<F: Future>(
    resources: Arc<ResourcePool>,
    future: F,
) -> F::Output {
    RESOURCES.scope(resources, future).await
}

/// Install the same pool on a synchronous graph-loading or server worker.
pub fn with_analysis_resources_blocking<T>(
    resources: Arc<ResourcePool>,
    operation: impl FnOnce() -> T,
) -> T {
    RESOURCES.sync_scope(resources, operation)
}
