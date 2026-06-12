//! `once` CLI entry point. Parses arguments via [`cli`], dispatches
//! to the verb modules under [`commands`], and propagates the
//! resulting exit code.

mod cache_provider;
mod cli;
mod commands;
mod dispatch;
mod logging;
mod reference;
mod render;

use std::process::ExitCode;

use clap::Parser;
use tracing::Instrument;

use cli::Cli;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => return handle_parse_error(&e),
    };
    let command = cli.surface_path().join(" ");
    let logging = logging::init(cli.verbose);
    let session_id = logging.session_id();
    let log_path = log_path(&logging);
    let session = tracing::info_span!("once_session", session_id = %session_id);
    tracing::info!(
        session_id = %session_id,
        command = if command.is_empty() {
            "help"
        } else {
            command.as_str()
        },
        log_path,
        "session started"
    );

    match Box::pin(dispatch::dispatch(cli).instrument(session)).await {
        Ok(code) => {
            tracing::info!(session_id = %session_id, exit_code = ?code, "session finished");
            code
        }
        Err(e) => {
            tracing::error!(session_id = %session_id, error = %e, "session failed");
            eprintln!("once: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn handle_parse_error(e: &clap::Error) -> ExitCode {
    let logging = logging::init(0);
    let log_path = log_path(&logging);
    let code = cli::exit_from(e.exit_code());
    tracing::info!(
        session_id = %logging.session_id(),
        log_path,
        exit_code = ?code,
        "argument parsing stopped"
    );
    if let Err(print_error) = e.print() {
        tracing::error!(error = %print_error, "failed to print clap error");
    }
    code
}

fn log_path(logging: &logging::Logging) -> String {
    logging.log_path().map_or_else(
        || "unavailable".to_string(),
        |path| path.display().to_string(),
    )
}
