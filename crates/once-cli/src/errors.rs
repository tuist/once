use std::fmt::Write as _;
use std::io::Write;

use crate::{cli, render};

pub(crate) fn write_dispatch_error(format: cli::Format, verbose: u8, error: &anyhow::Error) {
    if format == cli::Format::Human {
        let body = format_human_error(verbose, error);
        if let Err(write_error) = std::io::stderr().write_all(body.as_bytes()) {
            tracing::error!(target: "once", error = %write_error, "failed to write human error");
        }
        return;
    }
    // Errors always go to stderr, whatever the format, so stdout carries only a
    // command's structured result and stays safe to pipe into a JSON consumer.
    let body = structured_dispatch_error(format, error);
    if let Err(write_error) = std::io::stderr().write_all(body.as_bytes()) {
        tracing::error!(target: "once", error = %write_error, "failed to write structured error");
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
