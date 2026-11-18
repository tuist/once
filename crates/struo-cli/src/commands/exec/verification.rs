use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;
use struo_core::{RunOpts, Xdg};
use struo_frontend::parse_script_annotations;

use super::{
    collect_script_graph, parse_script_exec_argv, plan_exec_action, resolve_script_abs,
    script_invocation, ExecArgs, ScriptGraphOptions, SCRIPT_EXEC_USAGE,
};
use crate::cli::Output;

fn normalize_script_command(workspace: &Path, args: &mut ExecArgs) -> Result<()> {
    if let Some(first) = args.argv.first() {
        if let Ok(path) = resolve_script_abs(workspace, first) {
            if struo_frontend::script_has_struo_directives(&path).unwrap_or(false) {
                let annotations = parse_script_annotations(&path, first)?;
                let mut command = vec![annotations.runtime];
                command.extend(annotations.runtime_args);
                command.append(&mut args.argv);
                args.argv = command;
                args.script = true;
            }
        }
    }
    Ok(())
}

pub(super) async fn exec(
    workspace: &Path,
    xdg: &Xdg,
    mut args: ExecArgs,
    output: Output,
) -> Result<ExitCode> {
    if args.remote.is_some() {
        anyhow::bail!(
            "reproducibility verification currently requires local script execution; omit --remote"
        );
    }
    normalize_script_command(workspace, &mut args)?;
    let mut root = workspace.to_path_buf();
    let mut subject = "command".to_string();
    let mut outputs = Vec::new();
    if let Ok((_, _, script_arg, _)) = parse_script_exec_argv(workspace, &args.argv) {
        let script_path = resolve_script_abs(workspace, script_arg)?;
        if args.script || struo_frontend::script_has_struo_directives(&script_path)? {
            let mut invocation = script_invocation(
                workspace,
                args.env.iter().cloned().collect(),
                args.cwd.clone(),
                args.timeout_ms,
                None,
                args.sandbox,
                Some(xdg),
                &args.argv,
            )
            .await?;
            invocation.network = args.network;
            root = invocation.workspace.clone();
            subject = format!("script {}", invocation.script_path);
            args.argv = std::iter::once(invocation.runtime.clone())
                .chain(invocation.runtime_args.clone())
                .chain(std::iter::once(invocation.script_path.to_string()))
                .chain(invocation.script_args.clone())
                .collect();
            let graph_options = ScriptGraphOptions {
                explicit_env: args.env.iter().cloned().collect(),
                timeout_ms_override: args.timeout_ms,
                remote_override: None,
                sandbox: args.sandbox,
                network: args.network,
                resource_limits: args.resource_limits.clone(),
                xdg: Some(xdg),
            };
            let graph = collect_script_graph(invocation, &graph_options).await?;
            for invocation in graph.values() {
                if invocation.remote.is_some() {
                    anyhow::bail!("script `{}` selects remote execution; private reproducibility trials currently require local scripts", invocation.script_path);
                }
                outputs.extend(
                    invocation
                        .outputs
                        .iter()
                        .filter(|output| {
                            output.as_str() == "."
                                || !invocation.inputs.iter().any(|input| {
                                    Path::new(input.as_str()).starts_with(output.as_str())
                                        || Path::new(output.as_str()).starts_with(input.as_str())
                                })
                        })
                        .cloned(),
                );
            }
        }
    } else if args.script {
        anyhow::bail!(SCRIPT_EXEC_USAGE);
    }
    args.verify_reproducible = false;
    let limits = args.resource_limits.clone();
    let report = crate::commands::verification::run(
        &root,
        subject,
        outputs,
        limits,
        move |workspace, cache| {
            let args = args.clone();
            async move {
                let limits = args.resource_limits.clone();
                let (workspace, action) = plan_exec_action(
                    &workspace,
                    &Xdg::from_env(),
                    &cache,
                    RunOpts::default(),
                    args,
                )
                .await?;
                let result = struo_core::Runner::with_cache(cache, workspace, RunOpts::default())
                    .with_resource_limits(limits)
                    .run_fresh(&action, false)
                    .await;
                Ok(struo_core::ReproducibilityTrial {
                    action_digest: Some(action.digest()),
                    result: result.as_ref().ok().cloned(),
                    error: result.err().map(|error| error.to_string()),
                    log_paths: Vec::new(),
                })
            }
        },
    )
    .await?;
    crate::commands::verification::write(&report, output).await
}
