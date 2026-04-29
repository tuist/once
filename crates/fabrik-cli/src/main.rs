//! `fabrik` CLI entry point.

use std::collections::BTreeMap;
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use fabrik_cas::Cas;
use fabrik_core::{Action, CacheState};

#[derive(Parser)]
#[command(name = "fabrik", version, about = "Polyglot, agent-native build system")]
struct Cli {
    /// Workspace root. Defaults to the current directory; the cache lives
    /// under `<workspace>/.fabrik/`.
    #[arg(long, global = true)]
    workspace: Option<PathBuf>,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a command through the action cache. The cache key is the full
    /// argv plus any `-e KEY=VALUE` env entries. Subsequent invocations
    /// with the same key reuse the captured stdout/stderr/exit code.
    Run {
        /// Pass an environment variable to the command. Repeatable.
        #[arg(short = 'e', value_parser = parse_env)]
        env: Vec<(String, String)>,

        /// Command and arguments. Use `--` to separate from fabrik flags.
        #[arg(trailing_var_arg = true, required = true)]
        argv: Vec<String>,
    },

    /// Cache management.
    Cache {
        #[command(subcommand)]
        cmd: CacheCmd,
    },
}

#[derive(Subcommand)]
enum CacheCmd {
    /// Print blob and action counts plus on-disk size.
    Stats,
}

fn parse_env(raw: &str) -> std::result::Result<(String, String), String> {
    let (k, v) = raw
        .split_once('=')
        .ok_or_else(|| format!("expected KEY=VALUE, got `{raw}`"))?;
    Ok((k.to_string(), v.to_string()))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match dispatch(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("fabrik: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn dispatch(cli: Cli) -> Result<ExitCode> {
    let workspace = cli
        .workspace
        .map(Ok)
        .unwrap_or_else(env::current_dir)
        .context("resolving workspace root")?;
    let cas_root = workspace.join(".fabrik");
    let cas = Cas::open(&cas_root).with_context(|| format!("opening cas at {cas_root:?}"))?;

    match cli.command {
        Cmd::Run { env, argv } => run_command(&cas, env, argv),
        Cmd::Cache {
            cmd: CacheCmd::Stats,
        } => print_stats(&cas).map(|()| ExitCode::SUCCESS),
    }
}

fn run_command(cas: &Cas, env: Vec<(String, String)>, argv: Vec<String>) -> Result<ExitCode> {
    let action = Action::RunCommand {
        argv,
        env: env.into_iter().collect::<BTreeMap<_, _>>(),
        cwd: None,
    };
    let outcome = fabrik_core::run(&action, cas).context("executing action")?;

    let stdout = cas.get_blob(&outcome.result.stdout)?;
    let stderr = cas.get_blob(&outcome.result.stderr)?;
    io::stdout().write_all(&stdout)?;
    io::stderr().write_all(&stderr)?;

    let tag = match outcome.cache {
        CacheState::Hit => "hit",
        CacheState::Miss => "miss",
    };
    eprintln!(
        "fabrik: cache {tag} action={} exit={}",
        outcome.action, outcome.result.exit_code
    );

    Ok(exit_from(outcome.result.exit_code))
}

fn print_stats(cas: &Cas) -> Result<()> {
    let s = cas.stats()?;
    println!("blobs:   {} ({} bytes)", s.blob_count, s.blob_bytes);
    println!("actions: {} ({} bytes)", s.action_count, s.action_bytes);
    Ok(())
}

fn exit_from(code: i32) -> ExitCode {
    // ExitCode only carries u8; clamp the way most shells do.
    let clamped = u8::try_from(code & 0xff).unwrap_or(1);
    ExitCode::from(clamped)
}
