//! `once` CLI entry point. Parses arguments via [`cli`], dispatches
//! to the verb modules under [`commands`], and propagates the
//! resulting exit code.

mod argv_normalize;
mod bus_events;
mod cache_provider;
mod cli;
mod commands;
mod discovery;
mod dispatch;
mod errors;
mod live_run_reporter;
mod logging;
mod provision;
mod reference;
mod render;
mod reporter;
mod sound;
mod startup;
mod termination;

use std::process::ExitCode;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    startup::run().await
}
