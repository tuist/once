use std::future::Future;
use std::time::Duration;

use super::client::{lifetime, Client, Sandbox};
use crate::remote::lifecycle::{
    self, Budget, Cleanup, HostedSandbox, Journal, Lookup, Provider, Report,
};
use crate::{ExecutionPermit, Result};

/// Deletes a hosted sandbox and holds its admission lease until the deletion
/// finishes, so a cancelled caller never frees quota for a machine that is
/// still running.
pub(super) type SandboxCleanup = Cleanup<Client>;

/// Extra time after the requested lifetime before a complete listing without
/// the marker is accepted as proof that a create never landed.
const SETTLE_GRACE: Duration = Duration::from_mins(10);

impl Provider for Client {
    const NAME: &'static str = "e2b";

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
    /// returned. See [`lifecycle::create_owned`].
    pub(super) async fn create_owned(
        &self,
        journal: Option<Journal>,
        template: Option<String>,
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
                    .create_marked(template.as_deref(), timeout_ms, marker.as_ref())
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

    const CREATED: &str =
        r#"{"sandboxID":"e2b-test","envdAccessToken":"token","envdVersion":"1.0.0"}"#;

    #[tokio::test]
    async fn journals_the_marker_and_removes_it_after_deletion() {
        let workspace = tempfile::tempdir().unwrap();
        let journal = Journal::new(workspace.path());
        let (url, server) =
            test_server::spawn(vec![Response::json(CREATED), Response::empty(204)]).await;
        let client = client(&url);

        let (sandbox, cleanup) = client
            .create_owned(Some(journal), None, None, None)
            .await
            .unwrap();
        let journaled = records(workspace.path());
        cleanup.run().await.unwrap();
        let requests = test_server::assert_ready(server).await.unwrap();

        assert_eq!(sandbox.id, "e2b-test");
        assert_eq!(journaled.len(), 1);
        assert_eq!(journaled[0]["phase"], "created");
        assert_eq!(journaled[0]["sandbox_ids"][0], "e2b-test");
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["metadata"]["struo-operation"], journaled[0]["nonce"]);
        assert_eq!(requests[1].path, "/sandboxes/e2b-test");
        assert!(records(workspace.path()).is_empty());
    }

    #[tokio::test]
    async fn an_uncertain_create_keeps_its_intent() {
        let workspace = tempfile::tempdir().unwrap();
        let (url, server) = test_server::spawn(vec![Response::empty(502)]).await;
        let error = client(&url)
            .create_owned(Some(Journal::new(workspace.path())), None, None, None)
            .await
            .err()
            .unwrap();
        test_server::assert_ready(server).await.unwrap();

        assert!(error.to_string().contains("502"));
        assert_eq!(records(workspace.path())[0]["phase"], "intent");
    }

    #[tokio::test]
    async fn a_lost_create_response_is_found_by_its_marker_and_deleted() {
        let workspace = tempfile::tempdir().unwrap();
        let journal = Journal::new(workspace.path());
        let nonce = intent(&journal);
        let listed =
            format!(r#"[{{"sandboxID":"e2b-lost","metadata":{{"struo-operation":"{nonce}"}}}}]"#);
        let (url, server) = capture(vec![(200, listed), (404, String::new())]).await;
        let client = client(&url);
        rescope(
            workspace.path(),
            "e2b",
            &client.scope(),
            &client.credential(),
        );

        let report = client.reconcile(&journal).await;
        let requests = test_server::assert_ready(server).await.unwrap();

        for request in &requests {
            assert_eq!(request.headers["x-api-key"], "test-key");
        }
        assert_eq!(report.deleted, 1);
        assert_eq!(
            requests[0].path,
            format!(
                "/v2/sandboxes?metadata=struo-operation%3D{nonce}&state=running%2Cpaused&limit=100"
            )
        );
        assert_eq!(
            (requests[1].method.as_str(), requests[1].path.as_str()),
            ("DELETE", "/sandboxes/e2b-lost")
        );
        assert!(records(workspace.path()).is_empty());
    }

    #[tokio::test]
    async fn a_journaled_id_is_never_deleted_without_its_marker() {
        let workspace = tempfile::tempdir().unwrap();
        let journal = Journal::new(workspace.path());
        intent(&journal);
        let listed = r#"[{"sandboxID":"e2b-reused","metadata":{"struo-operation":"0123456789abcdef0123456789abcdef"}},{"sandboxID":"unlabelled"}]"#;
        let (url, server) = capture(vec![(200, listed.to_string())]).await;
        let client = client(&url);
        rescope(
            workspace.path(),
            "e2b",
            &client.scope(),
            &client.credential(),
        );
        mark_created(workspace.path(), "e2b-reused");

        let report = client.reconcile(&journal).await;
        let requests = test_server::assert_ready(server).await.unwrap();

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].headers["x-api-key"], "test-key");
        assert_eq!(report.deleted, 0);
        assert_eq!(report.retained, 1);
        assert_eq!(records(workspace.path())[0]["sandbox_ids"][0], "e2b-reused");
    }

    #[tokio::test]
    async fn a_refused_create_keeps_its_intent() {
        for status in [400, 403] {
            let workspace = tempfile::tempdir().unwrap();
            let (url, server) = test_server::spawn(vec![Response::empty(status)]).await;
            let error = client(&url)
                .create_owned(Some(Journal::new(workspace.path())), None, None, None)
                .await
                .err()
                .unwrap();
            test_server::assert_ready(server).await.unwrap();

            assert!(error.to_string().contains(&status.to_string()));
            assert_eq!(records(workspace.path())[0]["phase"], "intent");
        }
    }
}
