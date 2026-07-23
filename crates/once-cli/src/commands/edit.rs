//! `once edit` - mutate workspace manifests.

use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::cli::{Format, Output};
use crate::commands::query;
use crate::render;

mod example;

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
        changed: bool,
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
    let package_dir = resolve_package_dir(workspace, &input.package)?;
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
    let schemas = once_frontend::target_kind_schemas_for_workspace(workspace)
        .context("loading target kind schemas")?;
    let result = match once_frontend::apply_operations_with_schemas(
        &existing,
        &input.operations,
        &schemas,
    ) {
        Ok(new_src) => {
            let changed = new_src != existing;
            if changed {
                std::fs::create_dir_all(&package_dir).with_context(|| {
                    format!("creating package directory `{}`", package_dir.display())
                })?;
                std::fs::write(&manifest_path, &new_src)
                    .with_context(|| format!("writing `{}`", manifest_path.display()))?;
            }
            ApplyResult::Applied {
                applied: true,
                changed,
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

pub async fn materialize_example(
    workspace: &Path,
    output: Output,
    kind: &str,
    slug: &str,
    destination: &str,
) -> Result<()> {
    let result = example::materialize_example_value(workspace, kind, slug, destination)?;
    write_body(
        output,
        || example::render_materialize_example_human(&result),
        &result,
    )
    .await
}

pub(crate) use example::materialize_example_json;

pub(crate) fn resolve_package_dir(workspace: &Path, package: &str) -> Result<PathBuf> {
    if package.is_empty() {
        return Ok(workspace.to_path_buf());
    }
    let package_path = Path::new(package);
    if package_path.is_absolute()
        || package_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        anyhow::bail!("package must be a relative path inside the workspace");
    }
    Ok(workspace.join(package_path))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_dir_rejects_absolute_paths() {
        let err = resolve_package_dir(Path::new("/workspace"), "/tmp/outside").unwrap_err();
        assert!(err
            .to_string()
            .contains("relative path inside the workspace"));
    }

    #[test]
    fn package_dir_rejects_parent_traversal() {
        let err = resolve_package_dir(Path::new("/workspace"), "../outside").unwrap_err();
        assert!(err
            .to_string()
            .contains("relative path inside the workspace"));
    }

    #[test]
    fn package_dir_accepts_relative_package() {
        let path = resolve_package_dir(Path::new("/workspace"), "apps/Hello").unwrap();
        assert_eq!(path, Path::new("/workspace/apps/Hello"));
    }
}
