//! `fabrik` CLI entry point. Parses arguments via [`cli`], dispatches
//! to the verb modules under [`commands`], and propagates the
//! resulting exit code.

mod cli;
mod commands;
mod render;

use std::env;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use fabrik_cas::Cas;
use fabrik_core::Xdg;
use tracing_subscriber::{fmt, EnvFilter};

use cli::{Cli, Cmd, Format};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    match dispatch(cli).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("fabrik: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn init_tracing(verbose: u8) {
    // RUST_LOG always wins; otherwise -v sets the floor.
    let default = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

async fn dispatch(cli: Cli) -> Result<ExitCode> {
    let workspace = match cli.directory {
        Some(d) => fabrik_frontend::absolutize(d).context("resolving workspace root")?,
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
    // CAS lives outside the workspace under `$XDG_CACHE_HOME/fabrik/cas`
    // so identical actions hit across projects on the same host.
    // Workspace-local `.fabrik/out` still holds build outputs that
    // users consume from their checkout.
    let cas = Cas::open(Xdg::from_env().fabrik_cas());
    let format: Format = cli.format;

    match cli.command {
        Cmd::Run {
            target,
            runtime_rpc,
            runtime_rpc_socket,
        } => {
            let target = resolve_target_arg(&workspace, &target)?;
            commands::run::run(
                &workspace,
                &cas,
                &target,
                commands::run::RunArgs {
                    format,
                    runtime_rpc,
                    runtime_rpc_socket,
                },
            )
            .await
        }
        Cmd::Build { target } => {
            let target = resolve_target_arg(&workspace, &target)?;
            commands::build::build(&workspace, &cas, &target, format).await
        }
        Cmd::Test { target, test_args } => {
            let target = resolve_target_arg(&workspace, &target)?;
            commands::test::test(&workspace, &cas, &target, test_args, format).await
        }
        Cmd::Exec {
            env,
            cwd,
            timeout_ms,
            cache_failures,
            argv,
        } => {
            commands::exec::exec(
                &workspace,
                &cas,
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
            cmd: cli::CacheCmd::Stats,
        } => commands::cache::print_stats(&cas, format)
            .await
            .map(|()| ExitCode::SUCCESS),
        Cmd::Toolchain {
            cmd: cli::ToolchainCmd::Inspect { platform },
        } => commands::toolchain::inspect(&workspace, format, platform.as_deref())
            .await
            .map(|()| ExitCode::SUCCESS),
        Cmd::Runtime {
            cmd:
                cli::RuntimeCmd::Rpc {
                    session_dir,
                    socket,
                },
        } => commands::runtime::rpc(&session_dir, socket.as_deref())
            .await
            .map(|()| ExitCode::SUCCESS),
        Cmd::Targets => commands::targets::print_targets(&workspace, format)
            .await
            .map(|()| ExitCode::SUCCESS),
        Cmd::Deps {
            cmd: cli::DepsCmd::Sync { name },
        } => commands::deps::sync(&workspace, &cas, format, name.as_deref()).await,
        Cmd::Vendor => {
            eprintln!(
                "fabrik: `fabrik vendor` is deprecated and will be removed after the next release; use `fabrik deps sync` instead"
            );
            commands::deps::sync(&workspace, &cas, format, None).await
        }
        #[cfg(unix)]
        Cmd::ElixirCompile(args) => commands::elixir_compile::run(&workspace, &args),
        #[cfg(unix)]
        Cmd::ElixirDaemon { cmd } => commands::elixir_daemon::run(&workspace, cmd),
    }
}

fn resolve_target_arg(workspace: &std::path::Path, raw: &str) -> Result<String> {
    fabrik_frontend::normalize_cli_target(workspace, raw).context("resolving target argument")
}
