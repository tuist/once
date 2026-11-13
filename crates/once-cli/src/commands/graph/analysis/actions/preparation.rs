use anyhow::{Context, Result};
use once_frontend::analysis::DeclaredAction;

use super::{pending_outputs, DeclaredActionContext};

pub(super) async fn restore_generated_directories(
    repair: &super::Repair<'_>,
    declared: &DeclaredAction,
    available: &std::collections::BTreeMap<String, super::AvailableInput>,
) -> Result<()> {
    let mut outputs = std::collections::BTreeMap::new();
    for input in &declared.inputs {
        if super::resolve_available_input(available, input).is_some() {
            continue;
        }
        for (path, value) in super::generated_input_children(available, input) {
            outputs.insert(path.clone(), value.blob_digest);
        }
    }
    if !outputs.is_empty() {
        // A directory assembled by earlier actions has no single producer.
        // Hash its complete children, not a partially materialized cache hit.
        repair
            .restore(&once_cas::ActionResult {
                exit_code: 0,
                stdout: None,
                stderr: None,
                outputs,
            })
            .await?;
    }
    Ok(())
}

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
            &declared.clean_paths,
            &declared.create_dirs,
            lease,
        )
        .await?;
    }
    output_parents(context.workspace, &declared.outputs, lease).await?;
    write_args(context.workspace, &declared.arg_files, lease).await
}

pub(super) async fn output_parents(
    workspace: &std::path::Path,
    outputs: &[String],
    lease: &once_core::ExecutionPermit,
) -> Result<()> {
    let workspace = workspace.to_path_buf();
    let outputs = outputs
        .iter()
        .map(|path| super::workspace_path(path, "declared output path"))
        .collect::<Result<Vec<_>>>()?;
    lease
        .run_blocking(move || super::ensure_output_parent_dirs(&workspace, &outputs))
        .await??;
    Ok(())
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
