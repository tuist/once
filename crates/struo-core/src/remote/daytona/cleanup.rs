use std::future::Future;
use std::time::Duration;

use super::client::{lifetime, Client, Sandbox};
use crate::remote::lifecycle::{
    self, Budget, Cleanup, HostedSandbox, Journal, Lookup, Provider, Report,
};
use crate::{ExecutionPermit, ResourceRequest, Result};

/// Deletes a hosted sandbox and holds its admission lease until the deletion
/// finishes, so a cancelled caller never frees quota for a machine that is
/// still running.
pub(super) type SandboxCleanup = Cleanup<Client>;

/// Extra time after the expected lifetime before a complete listing without
/// the marker is accepted as proof that a create never landed.
const SETTLE_GRACE: Duration = Duration::from_mins(10);

impl Provider for Client {
    const NAME: &'static str = "daytona";

    fn scope(&self) -> String {
        Client::scope(self)
    }

    fn credential(&self) -> String {
        Client::credential(self)
    }

    fn find(&self, nonce: &str) -> impl Future<Output = Result<Lookup>> + Send {
        self.find_marked(nonce)
    }

    fn delete(&self, id: &str) -> impl Future<Output = Result<()>> + Send {
        Client::delete(self, id)
    }
}

impl HostedSandbox for Sandbox {
    fn id(&self) -> &str {
        &self.id
    }
}

impl Client {
    /// Creates a journaled sandbox whose cleanup is registered before it is
    /// returned. See [`lifecycle::create_owned`]. Callers still wait for the
    /// sandbox to start after this returns.
    pub(super) async fn create_owned(
        &self,
        journal: Option<Journal>,
        image: Option<String>,
        resources: ResourceRequest,
        timeout_ms: Option<u64>,
        lease: Option<ExecutionPermit>,
    ) -> Result<(Sandbox, SandboxCleanup)> {
        let settle = lifetime(timeout_ms).saturating_add(SETTLE_GRACE);
        lifecycle::create_owned(
            self.clone(),
            journal,
            settle,
            lease,
            move |client, marker| async move {
                client
                    .create_marked(image.as_deref(), &resources, timeout_ms, marker.as_ref())
                    .await
            },
        )
        .await
    }

    /// Repairs sandboxes left behind by earlier runs in this workspace.
    pub(super) async fn reconcile(&self, journal: &Journal) -> Report {
        lifecycle::reconcile(journal, self, Budget::default()).await
    }
}

#[cfg(test)]
mod tests {
    use super::super::client::Config;
    use super::*;
    use crate::remote::lifecycle::tests::{capture, intent, mark_created, records, rescope};
    use crate::remote::test_server::{self, Response};

    fn client(url: &str) -> Client {
        Client::new(Config::for_test(url.to_string(), url.to_string())).unwrap()
    }

    #[tokio::test]
    async fn journals_the_label_and_removes_it_after_deletion() {
        let workspace = tempfile::tempdir().unwrap();
        let journal = Journal::new(workspace.path());
        let (url, server) = test_server::spawn(vec![
            Response::json(r#"{"id":"daytona-test","state":"creating"}"#),
            Response::empty(204),
        ])
        .await;

        let (sandbox, cleanup) = client(&url)
            .create_owned(Some(journal), None, ResourceRequest::default(), None, None)
            .await
            .unwrap();
        let journaled = records(workspace.path());
        cleanup.run().await.unwrap();
        let requests = test_server::assert_ready(server).await.unwrap();

        assert_eq!(sandbox.id, "daytona-test");
        assert_eq!(journaled[0]["sandbox_ids"][0], "daytona-test");
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["labels"]["struo-operation"], journaled[0]["nonce"]);
        assert!(records(workspace.path()).is_empty());
    }

    #[tokio::test]
    async fn an_uncertain_create_keeps_its_intent() {
        let workspace = tempfile::tempdir().unwrap();
        let (url, server) = test_server::spawn(vec![Response::empty(500)]).await;
        client(&url)
            .create_owned(
                Some(Journal::new(workspace.path())),
                None,
                ResourceRequest::default(),
                None,
                None,
            )
            .await
            .err()
            .unwrap();
        test_server::assert_ready(server).await.unwrap();

        assert_eq!(records(workspace.path())[0]["phase"], "intent");
    }

    #[tokio::test]
    async fn a_lost_create_response_is_found_by_its_label_and_deleted() {
        let workspace = tempfile::tempdir().unwrap();
        let journal = Journal::new(workspace.path());
        let nonce = intent(&journal);
        let listed = format!(
            r#"[{{"id":"gone","state":"destroyed","labels":{{"struo-operation":"{nonce}"}}}},{{"id":"daytona-lost","state":"started","labels":{{"struo-operation":"{nonce}"}}}}]"#
        );
        let (url, server) = capture(vec![(200, listed), (200, String::new())]).await;
        let client = client(&url);
        rescope(
            workspace.path(),
            "daytona",
            &client.scope(),
            &client.credential(),
        );

        let report = client.reconcile(&journal).await;
        let requests = test_server::assert_ready(server).await.unwrap();

        for request in &requests {
            assert_eq!(request.headers["authorization"], "Bearer test-key");
        }
        assert_eq!(report.deleted, 1);
        assert_eq!(
            requests[0].path,
            format!("/sandbox?labels=%7B%22struo-operation%22%3A%22{nonce}%22%7D")
        );
        assert_eq!(
            (requests[1].method.as_str(), requests[1].path.as_str()),
            ("DELETE", "/sandbox/daytona-lost")
        );
        assert!(records(workspace.path()).is_empty());
    }

    #[tokio::test]
    async fn a_journaled_id_is_never_deleted_without_its_label() {
        let workspace = tempfile::tempdir().unwrap();
        let journal = Journal::new(workspace.path());
        intent(&journal);
        let listed = r#"[{"id":"daytona-reused","state":"started"}]"#;
        let (url, server) = capture(vec![(200, listed.to_string())]).await;
        let client = client(&url);
        rescope(
            workspace.path(),
            "daytona",
            &client.scope(),
            &client.credential(),
        );
        mark_created(workspace.path(), "daytona-reused");

        let report = client.reconcile(&journal).await;
        let requests = test_server::assert_ready(server).await.unwrap();

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].headers["authorization"], "Bearer test-key");
        assert_eq!(report.retained, 1);
        assert_eq!(
            records(workspace.path())[0]["sandbox_ids"][0],
            "daytona-reused"
        );
    }
}
