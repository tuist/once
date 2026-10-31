use std::path::Path;

use once_cas::{ActionResult, CacheProvider};

use super::archive::{
    create_input_archive_limited, install_output_archive_limited, output_archive_file,
};
use super::lifecycle::{reconcile, Budget, Journal};
use super::{join_path, CommandFilesystemSetup, PreparedCommand};
use crate::{ExecutionPermit, Executor, RemoteExecution, RemoteIoLimits, Result};

mod capacity;
mod lifecycle;
mod process;
mod setup;
#[cfg(test)]
mod tests;

const ROOT: &str = "/workspace";

pub(super) async fn execute_command(
    remote: &RemoteExecution,
    command: PreparedCommand<'_>,
    workspace: &Path,
    cache: &CacheProvider,
    parent: bool,
    setup: CommandFilesystemSetup<'_>,
) -> Result<ActionResult> {
    Executor::from_binding(remote)?;
    let lease = command.lease.as_ref().ok_or_else(|| {
        Executor::Docker.unsupported("Docker execution without an admission lease")
    })?;
    let client = process::Client::new(
        std::env::var_os("ONCE_DOCKER_BIN").unwrap_or_else(|| "docker".into()),
        lease.io_limits(),
    );
    Box::pin(execute_with_client(
        remote, command, workspace, cache, parent, client, setup,
    ))
    .await
}

async fn execute_with_client(
    remote: &RemoteExecution,
    mut command: PreparedCommand<'_>,
    workspace: &Path,
    cache: &CacheProvider,
    parent: bool,
    client: process::Client,
    setup: CommandFilesystemSetup<'_>,
) -> Result<ActionResult> {
    let lease = command.lease.take().ok_or_else(|| {
        Executor::Docker.unsupported("Docker execution without an admission lease")
    })?;
    let mut deadline = super::phase::Deadline::new(command.timeout_ms);
    let client = deadline.within(client.connect()).await?;
    let image = remote
        .environment
        .as_deref()
        .ok_or_else(|| process::error("an explicit Docker image is required"))?;
    deadline.within(client.verify_image(image)).await?;
    let journal = Journal::new(workspace);
    deadline
        .within(async {
            reconcile(&journal, &client, Budget::default()).await;
            Ok(())
        })
        .await?;
    let (lease, worker) = deadline.within(capacity::admit(&client, lease)).await?;
    command.lease = Some(lease);
    let lease = command.lease.as_ref().expect("Docker lease is attached");
    let (archive, input_phase) = deadline
        .within(create_input_archive_limited(
            workspace,
            command.inputs,
            "docker",
            client.limits,
            Some(lease),
        ))
        .await?;
    let capture = lease.capture_budget();
    deadline.within(capture.reserve_upfront()).await?;
    let (container, cleanup) = deadline
        .within(lifecycle::create(
            client.clone(),
            journal,
            image.into(),
            lease.clone(),
            worker,
            command.timeout_ms,
        ))
        .await?;
    let result = deadline
        .within(async {
            client
                .control(&["start".into(), container.0.clone()])
                .await?;
            setup::stage_inputs(&client, &container.0, archive.path()).await?;
            drop(archive);
            drop(input_phase);
            setup::apply(&client, &container.0, &command, setup).await?;
            setup::restore_inputs(&client, &container.0, workspace, setup, lease).await?;
            let args = setup::execution_args(&container.0, &command)?;
            let result = client.execute(&args, cache, parent, Some(capture)).await?;
            if command.accepts_exit_code(result.exit_code) && !command.outputs.is_empty() {
                client.verify_output_tar(&container.0).await?;
                let output = output_archive_file(workspace)?;
                let phase = output_permit(lease, client.limits).await?;
                let mut args = vec![
                    "exec".into(),
                    container.0.clone(),
                    "tar".into(),
                    "--hard-dereference".into(),
                    "-cf".into(),
                    "-".into(),
                    "-C".into(),
                    ROOT.into(),
                ];
                args.extend(
                    command
                        .outputs
                        .iter()
                        .map(|path| format!("./{}", path.as_str())),
                );
                client.transfer(&args, None, Some(output.path())).await?;
                install_output_archive_limited(
                    output.path(),
                    workspace,
                    command.outputs,
                    "docker",
                    client.limits,
                    phase,
                )
                .await?;
            }
            Ok(result)
        })
        .await;
    let deleted = cleanup.run().await;
    match (result, deleted) {
        (Ok(result), Ok(())) => Ok(result),
        (Err(error), cleanup) => {
            if let Err(cleanup) = cleanup {
                tracing::warn!(%cleanup, "Docker cleanup failed after execution error");
            }
            Err(error)
        }
        (Ok(_), Err(error)) => Err(error),
    }
}

async fn output_permit(
    lease: &ExecutionPermit,
    limits: RemoteIoLimits,
) -> Result<super::PhasePermit> {
    // Output extraction holds both the downloaded tar and its expanded files.
    super::phase_permit(Some(lease), limits.max_archive_bytes.saturating_mul(2)).await
}
