//! Lint result validation, persistence, and rendering.
//!
//! Split out of the graph command dispatch so `mod.rs` stays a table of
//! contents: these functions are only reachable from the `lint` verb and
//! share no state with the other capabilities.

use std::path::Path;

use anyhow::{Context, Result};
use once_core::{LintResults, WorkspacePath};
use once_frontend::GraphTarget;
use tokio::io::AsyncWriteExt;

use crate::cli::{Format, Output};
use crate::render;

pub(super) fn validate_lint_provider(
    target: &GraphTarget,
    provider: &serde_json::Value,
) -> Result<()> {
    lint_provider_output_path(target, provider, "sarif")?;
    lint_provider_output_path(target, provider, "results")?;
    Ok(())
}

pub(super) fn lint_provider_output_path<'a>(
    target: &GraphTarget,
    provider: &'a serde_json::Value,
    output_name: &str,
) -> Result<&'a str> {
    let attribute = format!("lint_info.outputs.{output_name}");
    let pointer = format!("/lint_info/outputs/{output_name}");
    let path = provider
        .pointer(&pointer)
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.is_empty());
    if let Some(path) = path {
        if WorkspacePath::try_from(path).is_ok() {
            return Ok(path);
        }
    }
    let diagnostic = once_frontend::Diagnostic::new(
        "invalid_lint_provider_output",
        format!(
            "lint provider for `{}` must return a non-empty workspace-relative path at `{attribute}`",
            target.label.id
        ),
    )
    .with_target(&target.label.id)
    .with_attribute(&attribute)
    .with_repair(format!(
        "Return the declared {output_name} output path at `{attribute}`"
    ));
    Err(anyhow::Error::new(
        once_frontend::analysis::AnalysisFailure { diagnostic },
    ))
}

pub(super) async fn persist_lint_results(
    workspace: &Path,
    path: &str,
    results: &LintResults,
) -> Result<()> {
    let absolute = workspace.join(path);
    if let Some(parent) = absolute.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let bytes = serde_json::to_vec_pretty(results)?;
    tokio::fs::write(&absolute, bytes)
        .await
        .with_context(|| format!("writing normalized lint results `{}`", absolute.display()))
}

pub(super) async fn write_lint_results(output: Output, results: &LintResults) -> Result<()> {
    let body = match output.format {
        Format::Human => render_lint_results(results),
        Format::Json | Format::Toon => render::structured(output.format, results)?,
    };
    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

pub(super) fn render_lint_results(results: &LintResults) -> String {
    let mut out = format!(
        "once: lint {} complete, {} errors, {} warnings, {} notes\n",
        results.target, results.summary.errors, results.summary.warnings, results.summary.notes
    );
    for finding in &results.findings {
        if let Some(location) = &finding.location {
            if let Some(path) = &location.path {
                out.push_str(path);
                if let Some(line) = location.line {
                    out.push(':');
                    out.push_str(&line.to_string());
                    if let Some(column) = location.column {
                        out.push(':');
                        out.push_str(&column.to_string());
                    }
                }
                out.push_str(": ");
            }
        }
        out.push_str(&format!("{:?}", finding.severity).to_lowercase());
        if let Some(rule_id) = &finding.rule_id {
            out.push('[');
            out.push_str(rule_id);
            out.push(']');
        }
        out.push_str(": ");
        out.push_str(&finding.message);
        out.push('\n');
    }
    out
}
