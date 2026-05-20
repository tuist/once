use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use fabrik_cas::Cas;
use fabrik_core::Xdg;

use crate::cli::{self, Cli, Cmd, Format};
use crate::commands;

pub(crate) async fn dispatch(cli: Cli) -> Result<ExitCode> {
    if cli.list {
        return commands::surface::print(&cli.surface_path(), cli.format)
            .await
            .map(|()| ExitCode::SUCCESS);
    }

    let Some(command) = cli.command else {
        return Ok(ExitCode::SUCCESS);
    };

    let workspace = resolve_workspace(cli.directory)?;
    let cas = Cas::open(Xdg::from_env().fabrik_cas());
    run_command(&workspace, &cas, cli.format, command).await
}

fn resolve_workspace(directory: Option<PathBuf>) -> Result<PathBuf> {
    let workspace = match directory {
        Some(directory) => {
            fabrik_frontend::absolutize(directory).context("resolving workspace root")?
        }
        None => env::current_dir().context("resolving workspace root")?,
    };

    // A typo'd `-C` filename used to surface as a CAS write failure
    // because the CAS lived under `<workspace>/.fabrik`. With the CAS
    // moved to `$XDG_CACHE_HOME/fabrik/cas`, the bogus path would
    // silently run; explicit validation here keeps the loud error.
    if !workspace.is_dir() {
        anyhow::bail!(
            "fabrik: workspace `{}` is not a directory",
            workspace.display()
        );
    }
    Ok(workspace)
}

async fn run_command(
    workspace: &Path,
    cas: &Cas,
    format: Format,
    command: Cmd,
) -> Result<ExitCode> {
    match command {
        Cmd::Run {
            target,
            runtime_rpc,
            runtime_rpc_socket,
        } => {
            run_target_command(
                workspace,
                cas,
                format,
                target,
                runtime_rpc,
                runtime_rpc_socket,
            )
            .await
        }
        Cmd::Build { target } => build_target_command(workspace, cas, format, target).await,
        Cmd::Test { target, test_args } => {
            test_target_command(workspace, cas, format, target, test_args).await
        }
        Cmd::Exec {
            env,
            cwd,
            timeout_ms,
            cache_failures,
            argv,
        } => {
            commands::exec::exec(
                workspace,
                cas,
                commands::exec::ExecArgs {
                    env,
                    cwd,
                    timeout_ms,
                    cache_failures,
                    argv,
                },
                format,
            )
            .await
        }
        Cmd::Cache {
            cmd: Some(cli::CacheCmd::Stats),
        } => commands::cache::print_stats(cas, format)
            .await
            .map(|()| ExitCode::SUCCESS),
        Cmd::Cache { cmd: None } => anyhow::bail!("cache subcommand required"),
        Cmd::Toolchain {
            cmd: Some(cli::ToolchainCmd::Inspect { platform }),
        } => commands::toolchain::inspect(workspace, format, platform.as_deref())
            .await
            .map(|()| ExitCode::SUCCESS),
        Cmd::Toolchain { cmd: None } => anyhow::bail!("toolchain subcommand required"),
        Cmd::Runtime {
            cmd:
                Some(cli::RuntimeCmd::Rpc {
                    session_dir,
                    socket,
                }),
        } => commands::runtime::rpc(&session_dir, socket.as_deref())
            .await
            .map(|()| ExitCode::SUCCESS),
        Cmd::Runtime { cmd: None } => anyhow::bail!("runtime subcommand required"),
        Cmd::Targets => commands::targets::print_targets(workspace, format)
            .await
            .map(|()| ExitCode::SUCCESS),
        Cmd::Deps {
            cmd: Some(cli::DepsCmd::Sync { name }),
        } => commands::deps::sync(workspace, cas, format, name.as_deref()).await,
        Cmd::Deps { cmd: None } => anyhow::bail!("deps subcommand required"),
        Cmd::Init(args) => commands::init::run(workspace, args, format).await,
        Cmd::Vendor => {
            eprintln!(
                "fabrik: `fabrik vendor` is deprecated and will be removed in v0.8.0; use `fabrik deps sync` instead"
            );
            commands::deps::sync(workspace, cas, format, None).await
        }
        #[cfg(unix)]
        Cmd::ElixirCompile(args) => commands::elixir_compile::run(workspace, &args),
        #[cfg(unix)]
        Cmd::ElixirDaemon { cmd: Some(cmd) } => commands::elixir_daemon::run(workspace, cmd),
        #[cfg(unix)]
        Cmd::ElixirDaemon { cmd: None } => anyhow::bail!("elixir-daemon subcommand required"),
    }
}

async fn run_target_command(
    workspace: &Path,
    cas: &Cas,
    format: Format,
    target: Option<String>,
    runtime_rpc: bool,
    runtime_rpc_socket: Option<PathBuf>,
) -> Result<ExitCode> {
    let target = resolve_required_target(workspace, target)?;
    commands::run::run(
        workspace,
        cas,
        &target,
        commands::run::RunArgs {
            format,
            runtime_rpc,
            runtime_rpc_socket,
        },
    )
    .await
}

async fn build_target_command(
    workspace: &Path,
    cas: &Cas,
    format: Format,
    target: Option<String>,
) -> Result<ExitCode> {
    let target = resolve_required_target(workspace, target)?;
    commands::build::build(workspace, cas, &target, format).await
}

async fn test_target_command(
    workspace: &Path,
    cas: &Cas,
    format: Format,
    target: Option<String>,
    test_args: Vec<String>,
) -> Result<ExitCode> {
    let target = resolve_required_target(workspace, target)?;
    commands::test::test(workspace, cas, &target, test_args, format).await
}

fn resolve_required_target(workspace: &Path, target: Option<String>) -> Result<String> {
    let raw = target.context("missing target")?;
    resolve_target_arg(workspace, &raw)
}

fn resolve_target_arg(workspace: &Path, raw: &str) -> Result<String> {
    fabrik_frontend::normalize_cli_target(workspace, raw).context("resolving target argument")
}
