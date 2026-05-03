//! `fabrik` CLI entry point. Parses arguments via [`cli`], dispatches
//! to the verb modules under [`commands`], and propagates the
//! resulting exit code.

mod cli;
mod commands;

use std::env;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use fabrik_cas::Cas;
use tracing_subscriber::{fmt, EnvFilter};

use cli::{Cli, Cmd};

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
        Some(d) => d,
        None => env::current_dir().context("resolving workspace root")?,
    };
    let cas = Cas::open(workspace.join(".fabrik"));

    match cli.command {
        Cmd::Run {
            env,
            cwd,
            timeout_ms,
            cache_failures,
            argv,
        } => {
            commands::run::run_command(&workspace, &cas, env, cwd, timeout_ms, cache_failures, argv)
                .await
        }
        Cmd::Cache {
            cmd: cli::CacheCmd::Stats,
        } => commands::cache::print_stats(&cas)
            .await
            .map(|()| ExitCode::SUCCESS),
        Cmd::Targets => commands::targets::print_targets(&workspace)
            .await
            .map(|()| ExitCode::SUCCESS),
        Cmd::Build { label } => commands::build::build(&workspace, &cas, &label).await,
    }
}
