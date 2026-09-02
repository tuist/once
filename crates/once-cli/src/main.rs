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

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io::Write;
use std::process::ExitCode;

use tracing::Instrument;
use usage::Error;

use cli::Cli;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
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

    let outcome = Box::pin(dispatch::dispatch(cli).instrument(session)).await;
    // The cache hangs off a process-wide map that nothing drops, so the run
    // has to hand back what it learned before it ends.
    once_frontend::flush_host_tree_digest_caches();
    match outcome {
        Ok(code) => {
            tracing::info!(session_id = %session_id, exit_code = ?code, "session finished");
            code
        }
        Err(e) => {
            tracing::error!(session_id = %session_id, error = %e, "session failed");
            write_dispatch_error(format, verbose, &e);
            ExitCode::from(2)
        }
    }
}

fn write_dispatch_error(format: cli::Format, verbose: u8, error: &anyhow::Error) {
    if format == cli::Format::Human {
        let body = format_human_error(verbose, error);
        if let Err(write_error) = std::io::stderr().write_all(body.as_bytes()) {
            tracing::error!(error = %write_error, "failed to write human error");
        }
        return;
    }
    // Errors always go to stderr, whatever the format, so stdout carries only a
    // command's structured result and stays safe to pipe into a JSON consumer.
    let body = structured_dispatch_error(format, error);
    if let Err(write_error) = std::io::stderr().write_all(body.as_bytes()) {
        tracing::error!(error = %write_error, "failed to write structured error");
    }
}

// Collapse the anyhow `Caused by:` chain into a compact frame: the root
// cause first (that is what the user needs to read), then one `while`
// line per intermediate context frame, outermost last. Every context
// message is preserved because dropping middle frames throws away the
// specific-most piece of information the user needs (a "resolving
// script path `foo.sh`" middle frame is more actionable than the "root
// cause: No such file or directory" alone). `-v` swaps in the classic
// `Caused by:` layout for the rare deep chain the compact frame makes
// harder to skim.
fn format_human_error(verbose: u8, error: &anyhow::Error) -> String {
    if verbose >= 1 {
        // Debug on anyhow::Error prints the multi-line `Caused by:` chain,
        // which is easier to scan than the single-line alternate form when
        // the chain has more than a couple of links.
        return format!("once: {error:?}\n");
    }
    let chain: Vec<_> = error.chain().collect();
    let root = chain
        .last()
        .expect("anyhow error chains always have at least one element");
    let mut out = format!("once: {root}\n");
    // Skip the terminal cause (already on the primary line) and walk
    // context frames from innermost to outermost so the most specific
    // operation reads closest to the root cause.
    for frame in chain.iter().rev().skip(1) {
        // Writing directly into the String avoids an intermediate allocation
        // (clippy::format_push_string); the write is infallible on a String.
        let _ = writeln!(out, "  while {frame}");
    }
    out
}

fn structured_dispatch_error(format: cli::Format, error: &anyhow::Error) -> String {
    let analysis_diagnostic = error.chain().find_map(|cause| {
        cause
            .downcast_ref::<once_frontend::analysis::AnalysisFailure>()
            .map(|failure| &failure.diagnostic)
    });
    let code =
        analysis_diagnostic.map_or("operation_failed", |diagnostic| diagnostic.code.as_str());
    let envelope = serde_json::json!({
        "schema": "once.error.v1",
        "error": {
            "code": code,
            "message": format!("{error:#}"),
            "diagnostics": analysis_diagnostic.into_iter().collect::<Vec<_>>(),
        }
    });
    render::structured(format, &envelope).unwrap_or_else(|render_error| {
        format!(
            "{{\"schema\":\"once.error.v1\",\"error\":{{\"code\":\"render_failed\",\"message\":{}}}}}\n",
            serde_json::Value::String(render_error.to_string())
        )
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_frame_shows_only_the_root_cause_when_there_is_no_context() {
        let error = anyhow::anyhow!("target `foo` not found");
        let out = format_human_error(0, &error);
        assert_eq!(out, "once: target `foo` not found\n");
    }

    #[test]
    fn human_frame_collapses_a_context_chain_to_root_plus_operation() {
        let error: anyhow::Error = std::io::Error::from(std::io::ErrorKind::NotFound).into();
        let error = error
            .context("reading script file")
            .context("parsing once headers for `foo.sh`");
        let out = format_human_error(0, &error);
        // Frames walk innermost-to-outermost after the root cause so the
        // most specific operation reads closest to the primary line.
        assert_eq!(
            out,
            "once: entity not found\n  \
             while reading script file\n  \
             while parsing once headers for `foo.sh`\n"
        );
    }

    #[test]
    fn human_frame_preserves_every_context_frame_in_the_chain() {
        let error = anyhow::anyhow!("No such file or directory")
            .context("resolving script path `foo.sh`")
            .context("executing action");
        let out = format_human_error(0, &error);
        assert_eq!(
            out,
            "once: No such file or directory\n  \
             while resolving script path `foo.sh`\n  \
             while executing action\n"
        );
    }

    #[test]
    fn human_frame_expands_to_the_full_chain_under_verbose() {
        let error = anyhow::anyhow!("root").context("middle").context("outer");
        let out = format_human_error(1, &error);
        assert!(out.contains("Caused by:"), "expected full chain in:\n{out}");
        assert!(out.contains("outer"));
        assert!(out.contains("middle"));
        assert!(out.contains("root"));
    }

    #[test]
    fn human_frame_prints_a_single_while_line_for_a_one_context_chain() {
        let error = anyhow::anyhow!("root").context("outer");
        let out = format_human_error(0, &error);
        assert_eq!(out, "once: root\n  while outer\n");
    }

    #[test]
    fn structured_dispatch_errors_have_a_stable_envelope() {
        let error = anyhow::anyhow!("unknown test unit");
        let rendered = structured_dispatch_error(cli::Format::Json, &error);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value["schema"], "once.error.v1");
        assert_eq!(value["error"]["code"], "operation_failed");
        assert_eq!(value["error"]["message"], "unknown test unit");
        assert_eq!(value["error"]["diagnostics"], serde_json::json!([]));
    }

    #[test]
    fn structured_dispatch_errors_preserve_analysis_diagnostics() {
        let diagnostic = once_frontend::Diagnostic::new(
            "target_kind_analysis_failed",
            "target kind implementation failed",
        )
        .with_target("App")
        .with_repair("Correct the target");
        let error = anyhow::Error::new(once_frontend::analysis::AnalysisFailure { diagnostic });

        let rendered = structured_dispatch_error(cli::Format::Json, &error);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value["error"]["code"], "target_kind_analysis_failed");
        assert_eq!(
            value["error"]["diagnostics"][0]["target"],
            serde_json::json!("App")
        );
    }
}
