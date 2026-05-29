//! `fabrik run` - execute the action(s) that produce a target.

mod action;
mod apple_runtime;
mod output;
mod runtime_descriptor;
mod session;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use fabrik_cas::CacheProvider;
use fabrik_core::{Action, CacheState, RemoteExecution, RunOpts};

use self::action::action_for;
use self::apple_runtime::is_apple_simulator_app;
use self::runtime_descriptor::runtime_descriptor;
use crate::cli::{exit_from, Output};
use crate::commands::util::{cache_tag, find_target};

pub struct RunArgs {
    pub output: Output,
    pub runtime_rpc: bool,
    pub runtime_rpc_socket: Option<PathBuf>,
    pub remote: Option<String>,
}

pub async fn run(
    workspace: &Path,
    cache: &CacheProvider,
    target_id: &str,
    args: RunArgs,
) -> Result<ExitCode> {
    let (targets, idx) = find_target(workspace, target_id)?;
    let target = &targets[idx];

    if is_apple_simulator_app(target) {
        return run_apple_ios_app(workspace, cache, target_id, &targets, target, args).await;
    }

    let mut plan = action_for(workspace, target)?;
    if let Some(provider) = args.remote.as_deref() {
        set_remote(&mut plan.action, provider);
    }
    if let Some(out_dir) = &plan.output_dir {
        tokio::fs::create_dir_all(out_dir)
            .await
            .with_context(|| format!("creating output directory {}", out_dir.display()))?;
    }

    let streams_live =
        action_remote(&plan.action).is_some() && args.output.format == crate::cli::Format::Human;
    let outcome = if streams_live {
        fabrik_core::run_with_cache_streaming(&plan.action, workspace, cache, RunOpts::default())
            .await
            .context("executing action")?
    } else {
        fabrik_core::run_with_cache(&plan.action, workspace, cache, RunOpts::default())
            .await
            .context("executing action")?
    };

    finish_run(
        workspace,
        cache,
        &outcome,
        target_id,
        target,
        &plan.output,
        args,
        streams_live && outcome.cache == CacheState::Miss,
    )
    .await?;
    Ok(exit_from(outcome.result.exit_code))
}

async fn run_apple_ios_app(
    workspace: &Path,
    cache: &CacheProvider,
    target_id: &str,
    targets: &[fabrik_frontend::Target],
    target: &fabrik_frontend::Target,
    args: RunArgs,
) -> Result<ExitCode> {
    if args.remote.is_some() {
        anyhow::bail!("--remote is only supported for script-like targets today");
    }
    let built =
        fabrik_apple::build_plan(targets, target_id, workspace).context("building app plan")?;
    let runner = crate::commands::util::runner(cache, workspace);
    let _build_outcomes = runner
        .run_plan(&built.plan)
        .await
        .with_context(|| format!("building app target {target_id}"))?;
    let launch = fabrik_apple::launch_ios_app(target, workspace)?;
    let outcome = runner
        .run(&launch.action)
        .await
        .with_context(|| format!("launching app target {target_id}"))?;

    finish_run(
        workspace,
        cache,
        &outcome,
        target_id,
        target,
        &launch.output,
        args,
        false,
    )
    .await?;
    Ok(exit_from(outcome.result.exit_code))
}

#[allow(clippy::too_many_arguments)]
async fn finish_run(
    workspace: &Path,
    cache: &CacheProvider,
    outcome: &fabrik_core::Outcome,
    target_id: &str,
    target: &fabrik_frontend::Target,
    output_path: &str,
    args: RunArgs,
    streams_live: bool,
) -> Result<()> {
    let RunArgs {
        output,
        runtime_rpc,
        runtime_rpc_socket,
        remote: _,
    } = args;
    let stdout_blob = match outcome.result.stdout {
        Some(digest) => cache.get_blob(&digest).await?,
        None => Vec::new(),
    };
    let stderr_blob = match outcome.result.stderr {
        Some(digest) => cache.get_blob(&digest).await?,
        None => Vec::new(),
    };
    let tag = cache_tag(outcome.cache);
    let mut runtime = runtime_descriptor(target_id, target)?;
    let session = match (&mut runtime, runtime_rpc) {
        (Some(runtime), true) => Some(
            session::prepare(
                workspace,
                target_id,
                runtime,
                runtime_rpc_socket,
                &stdout_blob,
                &stderr_blob,
            )
            .await?,
        ),
        (None, true) => anyhow::bail!("--runtime-rpc requires a target with runtime metadata"),
        (_, false) => None,
    };
    let record = output::RunRecord::new(
        target_id,
        &target.kind,
        outcome,
        tag,
        output_path.to_string(),
        runtime,
    );
    output::render(output, &stdout_blob, &stderr_blob, &record, streams_live).await?;

    if let Some(session) = session {
        crate::commands::runtime::rpc(&session.dir, Some(&session.socket)).await?;
    }

    Ok(())
}

fn set_remote(action: &mut Action, provider: &str) {
    match action {
        Action::RunCommand { remote, .. } => {
            *remote = Some(RemoteExecution {
                provider: provider.to_string(),
            });
        }
    }
}

fn action_remote(action: &Action) -> Option<&RemoteExecution> {
    match action {
        Action::RunCommand { remote, .. } => remote.as_ref(),
    }
}
