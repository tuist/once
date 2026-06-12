//! `once edit` - mutate workspace manifests.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::cli::{Format, Output};
use crate::commands::query;
use crate::render;

#[derive(Debug, Deserialize)]
struct ApplyInput {
    package: String,
    operations: Vec<once_frontend::EditOperation>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ApplyResult {
    Applied {
        applied: bool,
        path: String,
    },
    Rejected {
        applied: bool,
        diagnostics: Vec<once_frontend::Diagnostic>,
    },
}

pub async fn apply(workspace: &Path, output: Output, file: Option<PathBuf>) -> Result<()> {
    let raw = query::read_json_input(file)?;
    let input: ApplyInput = serde_json::from_str(&raw).context(
        "apply input is not valid JSON matching `{ \"package\": \"...\", \"operations\": [...] }`",
    )?;
    let package_dir = if input.package.is_empty() {
        workspace.to_path_buf()
    } else {
        workspace.join(&input.package)
    };
    let manifest_path = package_dir.join(once_frontend::TOML_BUILD_FILE_NAME);
    let existing = match std::fs::read_to_string(&manifest_path) {
        Ok(src) => src,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(anyhow::anyhow!(
                "reading `{}`: {err}",
                manifest_path.display()
            ));
        }
    };
    let result = match once_frontend::apply_operations(&existing, &input.operations) {
        Ok(new_src) => {
            std::fs::create_dir_all(&package_dir).with_context(|| {
                format!("creating package directory `{}`", package_dir.display())
            })?;
            std::fs::write(&manifest_path, &new_src)
                .with_context(|| format!("writing `{}`", manifest_path.display()))?;
            ApplyResult::Applied {
                applied: true,
                path: manifest_path.to_string_lossy().into_owned(),
            }
        }
        Err(diagnostics) => ApplyResult::Rejected {
            applied: false,
            diagnostics,
        },
    };
    write_body(output, || render_apply_human(&result), &result).await
}

fn render_apply_human(result: &ApplyResult) -> String {
    match result {
        ApplyResult::Applied { path, .. } => format!("applied: {path}\n"),
        ApplyResult::Rejected { diagnostics, .. } => {
            let mut out = String::from("rejected:\n");
            for diagnostic in diagnostics {
                let scope = match (&diagnostic.target, &diagnostic.attribute) {
                    (Some(t), Some(a)) => format!(" [{t}/{a}]"),
                    (Some(t), None) => format!(" [{t}]"),
                    (None, Some(a)) => format!(" [{a}]"),
                    (None, None) => String::new(),
                };
                writeln!(
                    out,
                    "  {}{}: {}",
                    diagnostic.code, scope, diagnostic.message
                )
                .expect("writing to string cannot fail");
                for repair in &diagnostic.repairs {
                    writeln!(out, "    - {repair}").expect("writing to string cannot fail");
                }
            }
            out
        }
    }
}

async fn write_body<T: Serialize>(
    output: Output,
    human: impl FnOnce() -> String,
    value: &T,
) -> Result<()> {
    let body = match output.format {
        Format::Human => human(),
        Format::Json | Format::Toon => render::structured(output.format, value)?,
    };
    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}
