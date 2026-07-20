use super::client::Client;
use crate::Result;

pub(super) struct SandboxCleanup {
    client: Client,
    id: Option<String>,
}

impl SandboxCleanup {
    pub(super) fn new(client: Client, id: String) -> Self {
        Self {
            client,
            id: Some(id),
        }
    }

    pub(super) async fn run(mut self) -> Result<()> {
        let id = self.id.take().expect("cleanup sandbox is present");
        self.client.delete(&id).await
    }
}

impl Drop for SandboxCleanup {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        let client = self.client.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(error) = client.delete(&id).await {
                    tracing::warn!(provider = "daytona", sandbox = %id, %error, "failed to delete remote sandbox");
                }
            });
        }
    }
}
