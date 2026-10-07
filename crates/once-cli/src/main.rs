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

fn main() -> ExitCode {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .event_interval(1)
        .build()
        .expect("initialize Once runtime")
        .block_on(startup::run())
}
