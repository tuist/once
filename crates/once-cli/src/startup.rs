use std::ffi::OsStr;
use std::process::ExitCode;

use tracing::Instrument;
use usage::Error;

use crate::cli::{self, Cli};
use crate::{dispatch, errors, logging, sound};

pub(crate) async fn run() -> ExitCode {
    let process_args = std::env::args_os().collect::<Vec<_>>();
    let argv = process_args
        .iter()
        .map(std::ffi::OsString::as_os_str)
        .collect::<Vec<_>>();
    let cli = match Cli::try_parse_from(&argv) {
        Ok(cli) => cli,
        Err(error) => return handle_parse_error(&argv, &error),
    };
    if let Some(path) = cli.incomplete_command_help_path() {
        return handle_incomplete_command(path);
    }
    let command = cli.surface_path().join(" ");
    let format = cli.format;
    let verbose = cli.verbose;
    crate::terminal::install_panic_hook();
    sound::init(cli.sound);
    let logging = logging::init(cli.verbose);
    let session_id = logging.session_id();
    let log_path = log_path(&logging);
    let session = tracing::info_span!(target: "once", "once_session", session_id = %session_id);
    tracing::info!(
        target: "once",
        session_id = %session_id,
        command = if command.is_empty() {
            "help"
        } else {
            command.as_str()
        },
        log_path,
        "session started"
    );

    let status = if !cli.list
        && matches!(
            cli.command.as_ref(),
            Some(
                cli::Cmd::Build { .. }
                    | cli::Cmd::Test { .. }
                    | cli::Cmd::Lint { .. }
                    | cli::Cmd::Run { .. }
            )
        ) {
        Some(crate::terminal::StatusGuard::start(
            crate::terminal::Policy::new(
                cli::Output::new(cli.format, cli.quiet)
                    .with_terminal_controls(cli.terminal_controls),
            ),
            &format!("once {command}"),
        ))
    } else {
        None
    };
    let outcome = Box::pin(dispatch::dispatch(cli).instrument(session)).await;
    // The cache hangs off a process-wide map that nothing drops, so the run
    // has to hand back what it learned before it ends.
    once_frontend::flush_host_tree_digest_caches();
    let code = match outcome {
        Ok(code) => {
            tracing::info!(target: "once", session_id = %session_id, exit_code = ?code, "session finished");
            code
        }
        Err(e) => {
            tracing::error!(target: "once", session_id = %session_id, error = %e, "session failed");
            sound::emit(sound::Event::Failed);
            errors::write_dispatch_error(format, verbose, &e);
            ExitCode::from(2)
        }
    };
    if let Some(status) = status {
        status.finish(code == ExitCode::SUCCESS);
    }
    // Give the last queued note time to reach the speakers before the process
    // ends. No-op when --sound is off.
    sound::wait_for_tail();
    code
}

fn handle_parse_error(argv: &[&OsStr], error: &Error<'static, '_>) -> ExitCode {
    let logging = logging::init(0);
    let log_path = log_path(&logging);
    let code = match error {
        Error::Help { cmd, long } => {
            if let Some(body) = Cli::render_help(cmd, *long) {
                print!("{body}");
            }
            ExitCode::SUCCESS
        }
        Error::HelpAll { cmd } => {
            if let Some(body) = usage::help::render_all(Cli::spec(), cmd) {
                print!("{body}");
            }
            ExitCode::SUCCESS
        }
        Error::Version { .. } => {
            println!("once {}", cli::CLI_VERSION);
            ExitCode::SUCCESS
        }
        Error::MissingArgsHelp { cmd } => {
            if let Some(body) = Cli::render_help(cmd, false) {
                eprint!("{body}");
            }
            ExitCode::from(2)
        }
        _ => {
            eprint!("{}", Cli::render_failure(argv, error));
            ExitCode::from(2)
        }
    };
    tracing::info!(
        target: "once",
        session_id = %logging.session_id(),
        log_path,
        exit_code = ?code,
        "argument parsing stopped"
    );
    code
}

fn handle_incomplete_command(path: &[&str]) -> ExitCode {
    let command = path.iter().try_fold(Cli::spec().root, |command, segment| {
        command
            .subcommands
            .iter()
            .copied()
            .find(|subcommand| subcommand.cmd.name == *segment)
    });
    let logging = logging::init(0);
    let log_path = log_path(&logging);
    let code = ExitCode::from(2);
    if let Some(command) = command {
        if let Some(body) = Cli::render_help(command.cmd, false) {
            eprint!("{body}");
        }
    }
    tracing::info!(
        target: "once",
        session_id = %logging.session_id(),
        log_path,
        exit_code = ?code,
        "argument parsing stopped"
    );
    code
}

fn log_path(logging: &logging::Logging) -> String {
    logging.log_path().map_or_else(
        || "unavailable".to_string(),
        |path| path.display().to_string(),
    )
}
