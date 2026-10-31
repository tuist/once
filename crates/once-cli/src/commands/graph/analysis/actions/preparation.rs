use anyhow::{Context, Result};
use once_frontend::analysis::DeclaredAction;

use super::{pending_outputs, DeclaredActionContext};

pub(super) async fn restore(
    context: &DeclaredActionContext<'_>,
    declared: &DeclaredAction,
) -> Result<pending_outputs::Restoration> {
    pending_outputs::restore_admitted(
        &context.repair(),
        context.prior_cached_results,
        context.available_inputs,
        declared,
    )
    .await
    .with_context(|| {
        format!(
            "materializing prerequisites for action {} for {} ({})",
            context.index, context.target_id, context.identifier
        )
    })
}

pub(super) async fn setup(
    context: &DeclaredActionContext<'_>,
    action: &once_core::Action,
    declared: &DeclaredAction,
    lease: &once_core::ExecutionPermit,
) -> Result<()> {
    if declared.operation.is_none()
        && once_core::Executor::resolve(action)? == once_core::Executor::Local
    {
        super::prepare_command_setup_admitted(
            context.workspace,
            context.cache,
            &declared.clean_paths,
            &declared.create_dirs,
            lease,
        )
        .await?;
    }
    write_args(context.workspace, &declared.arg_files, lease).await
}

pub(super) async fn write_args(
    workspace: &std::path::Path,
    arg_files: &[once_frontend::analysis::DeclaredArgFile],
    lease: &once_core::ExecutionPermit,
) -> Result<()> {
    let workspace = workspace.to_path_buf();
    let arg_files = arg_files.to_vec();
    lease
        .run_blocking(move || super::materialize_declared_arg_files(&workspace, &arg_files))
        .await??;
    Ok(())
}
