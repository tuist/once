//! Durable ownership of hosted sandboxes.
//!
//! Every create is journaled under the workspace's `.struo/executors` runtime
//! state before its request is sent and tagged with a unique operation marker
//! in provider metadata. The provider id is journaled as soon as it is known,
//! and the record is removed only after a confirmed deletion. Records left by
//! a lost create response, a failed deletion, or a process exit are repaired by
//! [`reconcile`] on a later run, which deletes only sandboxes that an
//! authenticated lookup currently reports with the record's marker.
//!
//! This bounds orphaned cost; it is not a hard bound on the provider fleet,
//! which can still outlive a lost journal, a relabelled sandbox, or a provider
//! that ignores deletion. The journal holds a fixed number of records and
//! refuses new creates when full. Records that cannot be resolved, such as
//! those created under a credential or endpoint that is no longer configured,
//! are kept and occupy that capacity until the original access returns or the
//! records are removed by hand.

use std::future::Future;
use std::time::Duration;

use crate::{Error, ExecutionPermit, Result};

mod cleanup;
mod journal;
mod reconcile;
#[cfg(test)]
pub(in crate::remote) mod tests;

pub(in crate::remote) use cleanup::Cleanup;
pub(in crate::remote) use journal::Journal;
pub(in crate::remote) use reconcile::{reconcile, Budget, Report};

/// Provider metadata key that carries the operation nonce.
pub(in crate::remote) const MARKER_KEY: &str = "struo-operation";

/// Provider control-plane operations the lifecycle needs. Every call must be
/// authenticated against the endpoint that [`Provider::scope`] identifies.
pub(in crate::remote) trait Provider: Clone + Send + Sync + 'static {
    const NAME: &'static str;

    /// Identifies the provider control endpoint. Records are only acted on by
    /// a client with the same scope.
    fn scope(&self) -> String;

    /// Fingerprints the credential. Another credential can still repair
    /// records whose marker it finds, because every deletion is verified by
    /// marker, but absence seen under another credential is never trusted.
    fn credential(&self) -> String;

    /// Lists sandboxes carrying `nonce` under [`MARKER_KEY`].
    fn find(&self, nonce: &str) -> impl Future<Output = Result<Lookup>> + Send;

    /// Deletes a sandbox. A missing sandbox counts as deleted.
    fn delete(&self, id: &str) -> impl Future<Output = Result<()>> + Send;
}

pub(in crate::remote) trait HostedSandbox: Send + 'static {
    fn id(&self) -> &str;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::remote) struct Marker {
    pub nonce: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::remote) enum Lookup {
    /// Sandboxes whose returned metadata carries the exact marker.
    Found(Vec<String>),
    /// A complete listing without the marker.
    Absent,
    /// The listing cannot prove absence: it was truncated, or the provider
    /// returned entries that do not carry the marker, so the filter may have
    /// been ignored.
    Inconclusive,
}

/// Classifies a provider listing. Only entries whose returned marker equals
/// `nonce` are ours; anything else is never deleted.
pub(in crate::remote) fn classify<'a>(
    entries: impl IntoIterator<Item = (&'a str, Option<&'a str>, bool)>,
    nonce: &str,
    truncated: bool,
) -> Lookup {
    let mut found = Vec::new();
    let mut foreign = false;
    for (id, marker, live) in entries {
        if marker != Some(nonce) {
            foreign = true;
        } else if live {
            found.push(id.to_string());
        }
    }
    if !found.is_empty() {
        Lookup::Found(found)
    } else if foreign || truncated {
        Lookup::Inconclusive
    } else {
        Lookup::Absent
    }
}

/// Identifies a provider endpoint.
pub(in crate::remote) fn scope(provider: &str, endpoint: &str) -> String {
    fingerprint(&format!("{provider}\n{}", endpoint.trim_end_matches('/')))
}

/// A non-reversible credential fingerprint, so no secret is stored.
pub(in crate::remote) fn credential(provider: &str, credential: &str) -> String {
    fingerprint(&format!("{provider}\n{credential}"))
}

fn fingerprint(material: &str) -> String {
    blake3::Hash::from(blake3::derive_key(
        "struo remote lifecycle scope v1",
        material.as_bytes(),
    ))
    .to_hex()[..32]
        .to_string()
}

/// Creates a sandbox on a task owned independently of the caller.
///
/// The intent is journaled before `create` runs. Once the provider answers,
/// the task journals the id and registers cleanup before handing the sandbox
/// back. If the caller was cancelled meanwhile, the task deletes the sandbox
/// itself; if the caller drops the returned pair, the cleanup's drop deletes
/// it. Creation is never retried. A response that does not prove rejection
/// leaves the intent journaled for [`reconcile`].
pub(in crate::remote) async fn create_owned<P, S, F, Fut>(
    provider: P,
    journal: Option<Journal>,
    settle: Duration,
    lease: Option<ExecutionPermit>,
    create: F,
) -> Result<(S, Cleanup<P>)>
where
    P: Provider,
    S: HostedSandbox,
    F: FnOnce(P, Option<Marker>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<S>> + Send + 'static,
{
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let outcome = create_task(provider, journal, settle, lease, create).await;
        if let Err(Ok((_, cleanup))) = sender.send(outcome) {
            tracing::debug!(
                provider = P::NAME,
                "deleting a sandbox whose creator was cancelled"
            );
            if let Err(error) = cleanup.run().await {
                tracing::warn!(provider = P::NAME, %error, "failed to delete an abandoned sandbox");
            }
        }
    });
    match receiver.await {
        Ok(outcome) => outcome,
        Err(_) => Err(Error::RemoteProviderApi {
            provider: P::NAME.to_string(),
            message: match task.await {
                Err(source) => format!("create task failed: {source}"),
                Ok(()) => "create task ended without a result".to_string(),
            },
        }),
    }
}

async fn create_task<P, S, F, Fut>(
    provider: P,
    journal: Option<Journal>,
    settle: Duration,
    lease: Option<ExecutionPermit>,
    create: F,
) -> Result<(S, Cleanup<P>)>
where
    P: Provider,
    S: HostedSandbox,
    F: FnOnce(P, Option<Marker>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<S>> + Send + 'static,
{
    let entry = match journal {
        Some(journal) => {
            let scope = provider.scope();
            let credential = provider.credential();
            Some(blocking(move || journal.begin(P::NAME, &scope, &credential, settle)).await?)
        }
        None => None,
    };
    let marker = entry.as_ref().map(|entry| Marker {
        nonce: entry.nonce().to_string(),
    });
    match create(provider.clone(), marker).await {
        Ok(sandbox) => {
            let id = sandbox.id().to_string();
            tracing::debug!(provider = P::NAME, sandbox = %id, "journaling created sandbox");
            let (entry, journaled) = match entry {
                Some(mut owned) => {
                    let ids = vec![id.clone()];
                    match blocking(move || {
                        let result = owned.mark_created(&ids);
                        Ok((owned, result))
                    })
                    .await
                    {
                        Ok((owned, result)) => (Some(owned), result),
                        Err(error) => (None, Err(error)),
                    }
                }
                None => (None, Ok(())),
            };
            let cleanup = Cleanup::journaled(provider, id, lease, entry);
            match journaled {
                Ok(()) => Ok((sandbox, cleanup)),
                Err(error) => {
                    if let Err(cleanup_error) = cleanup.run().await {
                        tracing::warn!(provider = P::NAME, %cleanup_error, "failed to delete an unjournaled sandbox");
                    }
                    Err(error)
                }
            }
        }
        Err(error) => {
            if let Some(entry) = entry {
                if rejected(&error) {
                    if let Err(journal_error) = blocking(move || entry.complete()).await {
                        tracing::warn!(provider = P::NAME, %journal_error, "failed to remove a rejected create intent");
                    }
                } else {
                    tracing::warn!(
                        provider = P::NAME,
                        nonce = entry.nonce(),
                        %error,
                        "create outcome is unknown; keeping its intent for reconciliation"
                    );
                }
            }
            drop(lease);
            Err(error)
        }
    }
}

/// Only a failure raised locally before the request is sent proves that no
/// sandbox exists: request metadata that exceeds its byte budget is refused
/// while encoding. No provider response is proof. A 400 or 403 can follow
/// validation, bootstrap, or a post-allocation failure, and neither provider
/// documents error statuses as side-effect free, so every status, like
/// timeouts and transport failures, keeps the intent for reconciliation.
///
/// The tradeoff is availability: a record from a refused create stays until a
/// lookup under the original credential settles it. Records from a credential
/// that is later revoked or replaced can stay indefinitely, because an empty
/// listing under another credential is never trusted, and they count toward
/// the journal's capacity, which fails new creates closed once full.
fn rejected(error: &Error) -> bool {
    matches!(
        error,
        Error::ExecutorResourceLimit {
            resource: "request metadata bytes",
            ..
        }
    )
}

/// Retries a deletion a bounded number of times on transient failures.
/// Deletion is idempotent: a missing sandbox is reported as success.
pub(in crate::remote) async fn delete_with_retries<P: Provider>(
    provider: &P,
    id: &str,
) -> Result<()> {
    const BACKOFF: [Duration; 2] = [Duration::from_millis(200), Duration::from_secs(1)];
    let mut attempt = 0;
    loop {
        match provider.delete(id).await {
            Ok(()) => return Ok(()),
            Err(error) if attempt < BACKOFF.len() && transient(&error) => {
                tracing::debug!(provider = P::NAME, sandbox = id, attempt, %error, "retrying sandbox deletion");
                tokio::time::sleep(BACKOFF[attempt]).await;
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

fn transient(error: &Error) -> bool {
    match error {
        Error::RemoteProviderHttp { .. } | Error::ExecutorStalled { .. } | Error::Timeout(_) => {
            true
        }
        Error::RemoteProviderStatus { status, .. } => *status >= 500 || matches!(status, 408 | 429),
        _ => false,
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|source| Error::RemoteProviderApi {
            provider: "remote".to_string(),
            message: format!("lifecycle journal task failed: {source}"),
        })?
}
