use tokio::sync::oneshot;

pub(super) fn should_sample_host(
    readiness: &Result<Result<(), oneshot::error::RecvError>, tokio::time::error::Elapsed>,
) -> bool {
    // A timed-out preflight may still connect after graph work starts.
    !matches!(readiness, Ok(Err(_)))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn connected_service_receives_host_samples() {
        let (sender, receiver) = oneshot::channel();
        sender.send(()).unwrap();
        let readiness = tokio::time::timeout(Duration::ZERO, receiver).await;
        assert!(should_sample_host(&readiness));
    }

    #[tokio::test]
    async fn completed_preflight_without_service_does_not_sample() {
        let (sender, receiver) = oneshot::channel::<()>();
        drop(sender);
        let readiness = tokio::time::timeout(Duration::ZERO, receiver).await;
        assert!(!should_sample_host(&readiness));
    }

    #[tokio::test]
    async fn pending_preflight_keeps_sampling_for_a_late_connection() {
        let (_sender, receiver) = oneshot::channel::<()>();
        let readiness = tokio::time::timeout(Duration::ZERO, receiver).await;
        assert!(readiness.is_err());
        assert!(should_sample_host(&readiness));
    }
}
