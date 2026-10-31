use std::collections::BTreeMap;
use std::path::Path;

use once_cas::{ActionResult, CacheProvider};

use crate::{ExecutionPermit, Executor, RemoteExecution, ResourceRequest, Result, WorkspacePath};

mod archive;
mod daytona;
mod docker;
mod e2b;
mod http;
mod lifecycle;
mod microsandbox;
mod output_install;
mod path;
mod phase;
#[cfg(test)]
mod test_server;

pub(crate) struct PreparedCommand<'a> {
    pub lease: Option<ExecutionPermit>,
    pub argv: &'a [String],
    pub env: &'a BTreeMap<String, String>,
    pub cwd: Option<&'a WorkspacePath>,
    pub inputs: &'a [WorkspacePath],
    pub outputs: &'a [WorkspacePath],
    pub resources: &'a ResourceRequest,
    pub timeout_ms: Option<u64>,
    pub success_exit_codes: &'a [i32],
}

#[derive(Debug, Clone, Default)]
pub(super) struct PhasePermit {
    _action: Option<ExecutionPermit>,
    _staging: Option<crate::executor::staging::StagingPermit>,
}

impl PreparedCommand<'_> {
    pub fn accepts_exit_code(&self, exit_code: i32) -> bool {
        self.success_exit_codes.contains(&exit_code)
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct CommandFilesystemSetup<'a> {
    pub clean_paths: &'a [WorkspacePath],
    pub create_dirs: &'a [WorkspacePath],
    pub post_setup_inputs: &'a [WorkspacePath],
}

pub(super) async fn phase_permit(
    lease: Option<&ExecutionPermit>,
    bytes: u64,
) -> Result<PhasePermit> {
    let staging = match lease {
        Some(lease) => Some(lease.acquire_staging(bytes).await?),
        None => None,
    };
    Ok(PhasePermit {
        _action: lease.cloned(),
        _staging: staging,
    })
}

pub(crate) async fn execute_command(
    executor: Executor,
    remote: &RemoteExecution,
    command: PreparedCommand<'_>,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    setup: CommandFilesystemSetup<'_>,
) -> Result<ActionResult> {
    if executor != Executor::Docker
        && (!setup.clean_paths.is_empty()
            || !setup.create_dirs.is_empty()
            || !setup.post_setup_inputs.is_empty())
    {
        return Err(executor.unsupported("explicit command filesystem setup"));
    }
    tracing::debug!(
        provider = %remote.provider,
        executor = executor.name(),
        environment_identity = "unverified-provider-reference",
        inputs = command.inputs.len(),
        outputs = command.outputs.len(),
        "dispatching remote command"
    );
    match executor {
        Executor::Docker => {
            Box::pin(docker::execute_command(
                remote,
                command,
                workspace_root,
                cache,
                stream_to_parent,
                setup,
            ))
            .await
        }
        Executor::Microsandbox => {
            microsandbox::execute_command(remote, command, workspace_root, cache, stream_to_parent)
                .await
        }
        Executor::Daytona => {
            Box::pin(daytona::execute_command(
                remote,
                command,
                workspace_root,
                cache,
                stream_to_parent,
            ))
            .await
        }
        Executor::E2b => {
            Box::pin(e2b::execute_command(
                remote,
                command,
                workspace_root,
                cache,
                stream_to_parent,
            ))
            .await
        }
        Executor::Local => unreachable!("local execution does not use a remote transport"),
    }
}

pub(super) fn join_path(root: &str, rel: &str) -> String {
    if root.ends_with('/') {
        format!("{root}{rel}")
    } else {
        format!("{root}/{rel}")
    }
}
