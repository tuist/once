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

use std::io::Write;
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
    let format = cli.format;
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
            write_dispatch_error(format, &e);
            ExitCode::from(2)
        }
    }
}

fn write_dispatch_error(format: cli::Format, error: &anyhow::Error) {
    if format == cli::Format::Human {
        eprintln!("once: {error:#}");
        return;
    }
    // Errors always go to stderr, whatever the format, so stdout carries only a
    // command's structured result and stays safe to pipe into a JSON consumer.
    let body = structured_dispatch_error(format, error);
    if let Err(write_error) = std::io::stderr().write_all(body.as_bytes()) {
        tracing::error!(error = %write_error, "failed to write structured error");
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
