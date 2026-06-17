use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use once_cas::CacheProvider;
use once_core::Xdg;

use crate::cli::{self, Cli, Cmd, Output};
use crate::commands;

pub(crate) async fn dispatch(cli: Cli) -> Result<ExitCode> {
    let output = Output::new(cli.format, cli.quiet);
    if cli.list {
        return commands::surface::print(&cli.surface_path(), output)
            .await
            .map(|()| ExitCode::SUCCESS);
    }

    let Some(command) = cli.command else {
        return Ok(ExitCode::SUCCESS);
    };

    let workspace = resolve_workspace(cli.directory)?;
    let xdg = Xdg::from_env();
    Box::pin(run_command(&workspace, &xdg, output, command)).await
}

fn resolve_workspace(directory: Option<PathBuf>) -> Result<PathBuf> {
    let workspace = match directory {
        Some(directory) => {
            once_frontend::absolutize(directory).context("resolving workspace root")?
        }
        None => env::current_dir().context("resolving workspace root")?,
    };

    // A typo'd `-C` filename used to surface as a CAS write failure
    // because the CAS lived under `<workspace>/.once`. With the CAS
    // moved to `$XDG_CACHE_HOME/once/cas`, the bogus path would
    // silently run; explicit validation here keeps the loud error.
    if !workspace.is_dir() {
        anyhow::bail!(
            "once: workspace `{}` is not a directory",
            workspace.display()
        );
    }
    Ok(workspace)
}

async fn run_command(
    workspace: &Path,
    xdg: &Xdg,
    output: Output,
    command: Cmd,
) -> Result<ExitCode> {
    match command {
        Cmd::Auth { cmd } => run_auth_command(workspace, xdg, output, cmd).await,
        Cmd::Build { target } => {
            let target = resolve_required_target(workspace, target)?;
            let cache = crate::cache_provider::resolve(workspace, xdg)?;
            commands::graph::build(workspace, &cache, output, &target).await
        }
        Cmd::Run {
            target,
            runtime_rpc,
            runtime_rpc_socket,
            remote,
            compute,
        } => {
            Box::pin(dispatch_run(
                workspace,
                xdg,
                output,
                target,
                runtime_rpc,
                runtime_rpc_socket,
                remote.then_some(compute),
            ))
            .await
        }
        Cmd::Exec {
            script,
            env,
            cwd,
            timeout_ms,
            cache_failures,
            remote,
            compute,
            argv,
        } => {
            dispatch_exec(
                workspace,
                xdg,
                output,
                commands::exec::ExecArgs {
                    script,
                    env,
                    cwd,
                    timeout_ms,
                    cache_failures,
                    remote: remote.then_some(compute),
                    argv,
                },
            )
            .await
        }
        Cmd::Cache { cmd } => run_cache_command(workspace, xdg, output, cmd).await,
        Cmd::Test { target } => {
            let target = resolve_required_target(workspace, target)?;
            let cache = crate::cache_provider::resolve(workspace, xdg)?;
            Box::pin(commands::graph::test(workspace, &cache, output, &target)).await
        }
        Cmd::Toolchain { cmd } => run_toolchain_command(workspace, output, cmd).await,
        Cmd::Query { cmd } => run_query_command(workspace, output, cmd).await,
        Cmd::Edit { cmd } => run_edit_command(workspace, output, cmd).await,
        Cmd::Runtime { cmd } => run_runtime_command(workspace, output, cmd).await,
        Cmd::Mcp {
            workspace: workspace_override,
            allow_run,
        } => {
            let mcp_workspace = workspace_override.unwrap_or_else(|| workspace.to_path_buf());
            commands::mcp::serve(mcp_workspace, allow_run)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Cmd::Reference { out } => crate::reference::generate(&out),
    }
}

async fn run_toolchain_command(
    workspace: &Path,
    output: Output,
    command: Option<cli::ToolchainCmd>,
) -> Result<ExitCode> {
    match command {
        Some(cli::ToolchainCmd::Inspect { platform }) => {
            commands::toolchain::inspect(workspace, output, platform.as_deref())
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        None => anyhow::bail!("toolchain subcommand required"),
    }
}

async fn run_query_command(
    workspace: &Path,
    output: Output,
    command: Option<cli::QueryCmd>,
) -> Result<ExitCode> {
    match command {
        Some(cli::QueryCmd::Targets { kind }) => {
            commands::query::targets(workspace, output, kind.as_deref())
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::Capabilities { target }) => {
            commands::query::capabilities(workspace, output, &target)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::Schema { kind }) => commands::query::schema(workspace, output, &kind)
            .await
            .map(|()| ExitCode::SUCCESS),
        Some(cli::QueryCmd::Example { kind, slug }) => {
            commands::query::example(workspace, output, &kind, &slug)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::TargetKinds) => commands::query::target_kinds(workspace, output)
            .await
            .map(|()| ExitCode::SUCCESS),
        Some(cli::QueryCmd::Target { target }) => {
            commands::query::target(workspace, output, &target)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::Tests) => commands::query::tests(workspace, output)
            .await
            .map(|()| ExitCode::SUCCESS),
        Some(cli::QueryCmd::AffectedTests { changed_paths }) => {
            commands::query::affected_tests(workspace, output, &changed_paths)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::TestResults { target }) => {
            commands::query::test_results(workspace, output, &target)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::ValidateTarget { file }) => {
            commands::query::validate_target(workspace, output, file)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        None => anyhow::bail!("query subcommand required"),
    }
}

async fn run_edit_command(
    workspace: &Path,
    output: Output,
    command: Option<cli::EditCmd>,
) -> Result<ExitCode> {
    match command {
        Some(cli::EditCmd::Apply { file }) => commands::edit::apply(workspace, output, file)
            .await
            .map(|()| ExitCode::SUCCESS),
        None => anyhow::bail!("edit subcommand required"),
    }
}

async fn run_runtime_command(
    workspace: &Path,
    output: Output,
    command: Option<cli::RuntimeCmd>,
) -> Result<ExitCode> {
    match command {
        Some(cli::RuntimeCmd::Start { target }) => {
            let target = resolve_required_target(workspace, target)?;
            commands::runtime::start(workspace, output, &target).await
        }
        Some(cli::RuntimeCmd::Status { session }) => {
            commands::runtime::status(workspace, output, &session).await
        }
        Some(cli::RuntimeCmd::Logs {
            session,
            source,
            cursor,
            limit,
        }) => {
            commands::runtime::logs(
                workspace,
                output,
                &session,
                source.as_deref(),
                cursor.as_deref(),
                limit,
            )
            .await
        }
        Some(cli::RuntimeCmd::Stop { session }) => {
            commands::runtime::stop(workspace, output, &session).await
        }
        Some(cli::RuntimeCmd::Rpc {
            session_dir,
            socket,
        }) => commands::runtime::rpc(&session_dir, socket.as_deref())
            .await
            .map(|()| ExitCode::SUCCESS),
        Some(cli::RuntimeCmd::Supervise {
            session_dir,
            target,
        }) => commands::runtime::supervise(workspace, &session_dir, &target),
        None => anyhow::bail!("runtime subcommand required"),
    }
}

async fn run_auth_command(
    workspace: &Path,
    xdg: &Xdg,
    output: Output,
    command: Option<cli::AuthCmd>,
) -> Result<ExitCode> {
    match command {
        Some(cli::AuthCmd::Login {
            provider,
            no_browser,
        }) => commands::auth::login(workspace, xdg, &provider, !no_browser, output)
            .await
            .map(|()| ExitCode::SUCCESS),
        Some(cli::AuthCmd::Logout { provider }) => {
            commands::auth::logout(workspace, xdg, &provider, output)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        None => anyhow::bail!("auth subcommand required"),
    }
}

async fn run_cache_command(
    workspace: &Path,
    xdg: &Xdg,
    output: Output,
    command: Option<cli::CacheCmd>,
) -> Result<ExitCode> {
    match command {
        Some(cli::CacheCmd::Stats) => {
            let cache = crate::cache_provider::resolve(workspace, xdg)?;
            commands::cache::print_stats(&cache, output)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::CacheCmd::Blob { cmd }) => {
            let cache = crate::cache_provider::resolve(workspace, xdg)?;
            match cmd {
                Some(cli::CacheBlobCmd::Put { path }) => {
                    commands::cache::put_blob(&cache, path.as_deref(), output)
                        .await
                        .map(|()| ExitCode::SUCCESS)
                }
                Some(cli::CacheBlobCmd::Get {
                    digest,
                    output: output_path,
                }) => commands::cache::get_blob(&cache, digest, output_path.as_deref(), output)
                    .await
                    .map(|()| ExitCode::SUCCESS),
                Some(cli::CacheBlobCmd::Exists { digest }) => {
                    commands::cache::exists_blob(&cache, digest, output).await
                }
                None => anyhow::bail!("cache blob subcommand required"),
            }
        }
        Some(cli::CacheCmd::Action { cmd }) => {
            let cache = crate::cache_provider::resolve(workspace, xdg)?;
            match cmd {
                Some(cli::CacheActionCmd::Get {
                    action,
                    inputs,
                    if_success,
                }) => commands::cache::get_action(&cache, action, inputs, if_success, output).await,
                Some(cli::CacheActionCmd::Put {
                    action,
                    inputs,
                    exit_code,
                    stdout,
                    stderr,
                    outputs,
                }) => commands::cache::put_action(
                    &cache, action, inputs, exit_code, stdout, stderr, outputs, output,
                )
                .await
                .map(|()| ExitCode::SUCCESS),
                Some(cli::CacheActionCmd::Forget { action }) => {
                    commands::cache::forget_action(&cache, action, output)
                        .await
                        .map(|()| ExitCode::SUCCESS)
                }
                None => anyhow::bail!("cache action subcommand required"),
            }
        }
        None => anyhow::bail!("cache subcommand required"),
    }
}

async fn dispatch_run(
    workspace: &Path,
    xdg: &Xdg,
    output: Output,
    target: Option<String>,
    runtime_rpc: bool,
    runtime_rpc_socket: Option<PathBuf>,
    remote: Option<String>,
) -> Result<ExitCode> {
    let resolved_target = resolve_required_target(workspace, target.clone())?;
    if commands::graph::supports(workspace, &resolved_target, "run")? {
        if runtime_rpc || runtime_rpc_socket.is_some() {
            anyhow::bail!("--runtime-rpc is only supported for executable script targets");
        }
        if remote.is_some() {
            anyhow::bail!("--remote is only supported for executable script targets");
        }
        let cache = crate::cache_provider::resolve(workspace, xdg)?;
        // Keep dispatch_run's future below clippy's large_futures limit.
        return Box::pin(commands::graph::run(
            workspace,
            &cache,
            output,
            &resolved_target,
        ))
        .await;
    }
    let cache = crate::cache_provider::resolve(workspace, xdg)?;
    run_target_command(
        workspace,
        &cache,
        output,
        Some(resolved_target),
        runtime_rpc,
        runtime_rpc_socket,
        remote,
    )
    .await
}

async fn dispatch_exec(
    workspace: &Path,
    xdg: &Xdg,
    output: Output,
    args: commands::exec::ExecArgs,
) -> Result<ExitCode> {
    let cache = crate::cache_provider::resolve(workspace, xdg)?;
    commands::exec::exec(workspace, &cache, args, output).await
}

async fn run_target_command(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target: Option<String>,
    runtime_rpc: bool,
    runtime_rpc_socket: Option<PathBuf>,
    remote: Option<String>,
) -> Result<ExitCode> {
    let target = resolve_required_target(workspace, target)?;
    commands::run::run(
        workspace,
        cache,
        &target,
        commands::run::RunArgs {
            output,
            runtime_rpc,
            runtime_rpc_socket,
            remote,
        },
    )
    .await
}

fn resolve_required_target(workspace: &Path, target: Option<String>) -> Result<String> {
    let raw = target.context("missing target")?;
    resolve_target_arg(workspace, &raw)
}

fn resolve_target_arg(workspace: &Path, raw: &str) -> Result<String> {
    once_frontend::normalize_cli_target(workspace, raw).context("resolving target argument")
}
