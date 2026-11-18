/// A worker lane taken only once its action is admitted to run.
///
/// Lanes give actions their stable `worker_id`, and there are only as many as
/// actions may run at once. An action still waiting for capacity, for its
/// lock, or for a cache probe would otherwise sit on a lane another admitted
/// action could use.
pub(in crate::commands::graph::analysis) struct WorkerLease<'a> {
    workers: &'a crate::bus_events::WorkerSlots,
    guard: tokio::sync::OnceCell<crate::bus_events::WorkerGuard>,
    on_claim: Box<dyn Fn(&str) + Send + Sync + 'a>,
}

impl<'a> WorkerLease<'a> {
    pub(in crate::commands::graph::analysis) fn new(
        workers: &'a crate::bus_events::WorkerSlots,
        on_claim: impl Fn(&str) + Send + Sync + 'a,
    ) -> Self {
        Self {
            workers,
            guard: tokio::sync::OnceCell::new(),
            on_claim: Box::new(on_claim),
        }
    }

    /// The lane this action runs on, taking one the first time it is asked.
    pub(in crate::commands::graph::analysis) async fn claim(&self) -> &str {
        self.guard
            .get_or_init(|| async {
                let guard = self.workers.acquire().await;
                (self.on_claim)(guard.worker_id());
                guard
            })
            .await
            .worker_id()
    }
}
