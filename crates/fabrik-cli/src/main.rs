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
use tracing_subscriber::{fmt, EnvFilter};

use cli::{Cli, Cmd, Format, CACHE_DIR};

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
    let cas = Cas::open(workspace.join(CACHE_DIR));
    let format: Format = cli.format;

    match cli.command {
        Cmd::Run { target } => {
            let target = resolve_target_arg(&workspace, &target)?;
            commands::run::run(&workspace, &cas, &target, format).await
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
        Cmd::Targets => commands::targets::print_targets(&workspace, format)
            .await
            .map(|()| ExitCode::SUCCESS),
        Cmd::Vendor => commands::vendor::vendor(&workspace, format).await,
    }
}

fn resolve_target_arg(workspace: &std::path::Path, raw: &str) -> Result<String> {
    fabrik_frontend::normalize_cli_target(workspace, raw).context("resolving target argument")
}
