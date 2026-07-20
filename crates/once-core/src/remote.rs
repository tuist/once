use std::collections::BTreeMap;
use std::path::Path;

use once_cas::{ActionResult, CacheProvider};

use crate::{Error, RemoteExecution, ResourceRequest, Result, WorkspacePath};

mod archive;
mod daytona;
mod e2b;
mod microsandbox;
mod output_install;
mod path;
#[cfg(test)]
mod test_server;

pub(crate) struct PreparedCommand<'a> {
    pub argv: &'a [String],
    pub env: &'a BTreeMap<String, String>,
    pub cwd: Option<&'a WorkspacePath>,
    pub inputs: &'a [WorkspacePath],
    pub outputs: &'a [WorkspacePath],
    pub resources: &'a ResourceRequest,
    pub timeout_ms: Option<u64>,
}

pub(crate) async fn execute_command(
    remote: &RemoteExecution,
    command: PreparedCommand<'_>,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
) -> Result<ActionResult> {
    let executor = remote.executor.as_deref().unwrap_or(&remote.provider);
    tracing::debug!(
        provider = %remote.provider,
        executor,
        environment = remote.environment.as_deref().unwrap_or("provider-default"),
        inputs = command.inputs.len(),
        outputs = command.outputs.len(),
        "dispatching remote command"
    );
    match executor {
        "microsandbox" => {
            microsandbox::execute_command(remote, command, workspace_root, cache, stream_to_parent)
                .await
        }
        "daytona" => {
            daytona::execute_command(remote, command, workspace_root, cache, stream_to_parent).await
        }
        "e2b" => {
            e2b::execute_command(remote, command, workspace_root, cache, stream_to_parent).await
        }
        executor => Err(Error::UnsupportedRemoteProvider {
            provider: executor.to_string(),
        }),
    }
}

pub(super) fn join_path(root: &str, rel: &str) -> String {
    if root.ends_with('/') {
        format!("{root}{rel}")
    } else {
        format!("{root}/{rel}")
    }
}
