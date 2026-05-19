//! `fabrik targets` - list every declared target in the workspace.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::Format;
use crate::render;

/// JSON view of a [`fabrik_frontend::Target`] that includes the
/// computed `id` field. We avoid embedding `id` directly in the
/// frontend's `Target` struct because the canonical representation is
/// `(package, name)`; the id is a derived display form.
#[derive(Serialize)]
struct TargetView<'a> {
    id: String,
    package: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    external_package: Option<&'a str>,
    kind: &'a str,
    name: &'a str,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    srcs: &'a [String],
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    deps: &'a [String],
    #[serde(skip_serializing_if = "<[fabrik_frontend::ExternalDependency]>::is_empty")]
    external_deps: &'a [fabrik_frontend::ExternalDependency],
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    attrs: &'a BTreeMap<String, String>,
}

#[derive(Serialize)]
struct TargetFields<'a> {
    package: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    external_package: Option<&'a str>,
    kind: &'a str,
    name: &'a str,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    srcs: &'a [String],
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    deps: &'a [String],
    #[serde(skip_serializing_if = "<[fabrik_frontend::ExternalDependency]>::is_empty")]
    external_deps: &'a [fabrik_frontend::ExternalDependency],
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    attrs: &'a BTreeMap<String, String>,
}

#[derive(Serialize)]
struct TargetsToonView<'a> {
    targets: BTreeMap<String, TargetFields<'a>>,
}

pub async fn print_targets(workspace: &Path, format: Format) -> Result<()> {
    let targets = fabrik_frontend::load_workspace(workspace).context("loading workspace")?;
    if format == Format::Toon {
        let targets = TargetsToonView {
            targets: targets
                .iter()
                .map(|t| {
                    (
                        t.id(),
                        TargetFields {
                            package: &t.package,
                            external_package: t.external_package.as_deref(),
                            kind: &t.kind,
                            name: &t.name,
                            srcs: &t.srcs,
                            deps: &t.deps,
                            external_deps: &t.external_deps,
                            attrs: &t.attrs,
                        },
                    )
                })
                .collect(),
        };
        let mut out = tokio::io::stdout();
        out.write_all(render::structured(format, &targets)?.as_bytes())
            .await?;
        out.flush().await?;
        return Ok(());
    }

    let mut out = tokio::io::stdout();
    for t in &targets {
        let line = match format {
            Format::Human => format!("{} {}\n", t.kind, t.id()),
            Format::Json => {
                let view = TargetView {
                    id: t.id(),
                    package: &t.package,
                    external_package: t.external_package.as_deref(),
                    kind: &t.kind,
                    name: &t.name,
                    srcs: &t.srcs,
                    deps: &t.deps,
                    external_deps: &t.external_deps,
                    attrs: &t.attrs,
                };
                format!("{}\n", serde_json::to_string(&view)?)
            }
            Format::Toon => unreachable!("TOON targets are emitted as a single document"),
        };
        out.write_all(line.as_bytes()).await?;
    }
    out.flush().await?;
    Ok(())
}
