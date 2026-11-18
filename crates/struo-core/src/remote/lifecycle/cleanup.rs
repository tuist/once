use super::journal::Entry;
use super::{blocking, delete_with_retries, Provider};
use crate::{Error, ExecutionPermit, Result};

/// Deletes a hosted sandbox and holds its admission lease until the deletion
/// finishes, so a cancelled caller never frees quota for a machine that is
/// still running. A journaled sandbox keeps its record until the provider
/// confirms deletion; a failed deletion leaves it for reconciliation.
pub(in crate::remote) struct Cleanup<P: Provider> {
    provider: P,
    id: Option<String>,
    lease: Option<ExecutionPermit>,
    entry: Option<Entry>,
}

impl<P: Provider> Cleanup<P> {
    pub(in crate::remote) fn journaled(
        provider: P,
        id: String,
        lease: Option<ExecutionPermit>,
        entry: Option<Entry>,
    ) -> Self {
        Self {
            provider,
            id: Some(id),
            lease,
            entry,
        }
    }

    /// Deletion runs on a detached task: cancelling this future leaves the
    /// task, and the lease it owns, running until the provider answers.
    pub(in crate::remote) async fn run(mut self) -> Result<()> {
        let deletion = self.take();
        tokio::spawn(deletion)
            .await
            .map_err(|source| Error::RemoteProviderApi {
                provider: P::NAME.to_string(),
                message: format!("cleanup task failed: {source}"),
            })?
    }

    fn take(&mut self) -> impl std::future::Future<Output = Result<()>> + Send + 'static {
        let id = self.id.take().expect("cleanup sandbox is present");
        delete(
            self.provider.clone(),
            id,
            self.lease.take(),
            self.entry.take(),
        )
    }
}

impl<P: Provider> Drop for Cleanup<P> {
    fn drop(&mut self) {
        if self.id.is_none() {
            return;
        }
        let deletion = self.take();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(error) = deletion.await {
                    tracing::warn!(provider = P::NAME, %error, "failed to delete remote sandbox");
                }
            });
        } else {
            tracing::warn!(
                provider = P::NAME,
                "skipping remote sandbox deletion because no Tokio runtime is available; a journaled sandbox is reconciled on the next run"
            );
        }
    }
}

async fn delete<P: Provider>(
    provider: P,
    id: String,
    lease: Option<ExecutionPermit>,
    entry: Option<Entry>,
) -> Result<()> {
    let result = delete_with_retries(&provider, &id).await;
    if let Some(mut entry) = entry {
        let deleted = result.is_ok();
        let journaled = blocking(move || {
            if deleted {
                entry.complete()
            } else {
                entry.defer(std::time::Duration::ZERO)
            }
        })
        .await;
        if let Err(error) = journaled {
            tracing::warn!(provider = P::NAME, sandbox = %id, %error, "failed to update remote lifecycle record");
        }
    }
    tracing::debug!(
        provider = P::NAME,
        sandbox = %id,
        deleted = result.is_ok(),
        "releasing remote admission lease after sandbox cleanup"
    );
    drop(lease);
    result
}
