//! `once query` - inspect the typed build graph.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::{Format, Output};
use crate::render;

#[derive(Debug, Serialize)]
struct TargetRecord {
    id: String,
    package: String,
    name: String,
    kind: String,
    deps: Vec<String>,
    capabilities: Vec<String>,
}

pub async fn targets(workspace: &Path, output: Output, kind: Option<&str>) -> Result<()> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    let records = graph
        .into_iter()
        .filter(|target| kind.is_none_or(|kind| target.kind == kind))
        .map(|target| TargetRecord {
            id: target.label.id,
            package: target.label.package,
            name: target.label.name,
            kind: target.kind,
            deps: target.deps,
            capabilities: target
                .capabilities
                .into_iter()
                .map(|capability| capability.name)
                .collect(),
        })
        .collect::<Vec<_>>();
    write_body(output, || render_targets_human(&records), &records).await
}

pub async fn capabilities(workspace: &Path, output: Output, target_id: &str) -> Result<()> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    let target = graph
        .into_iter()
        .find(|target| target.label.id == target_id)
        .with_context(|| format!("no target matches `{target_id}`"))?;
    write_body(
        output,
        || {
            let mut out = format!("{} ({})\n", target.label.id, target.kind);
            if target.capabilities.is_empty() {
                out.push_str("capabilities: none\n");
                return out;
            }
            out.push_str("capabilities:\n");
            for capability in &target.capabilities {
                writeln!(out, "  {}", capability.name).expect("writing to string cannot fail");
                writeln!(out, "    outputs: {}", capability.output_groups.join(", "))
                    .expect("writing to string cannot fail");
                if !capability.requires_outputs.is_empty() {
                    writeln!(
                        out,
                        "    requires: {}",
                        capability.requires_outputs.join(", ")
                    )
                    .expect("writing to string cannot fail");
                }
            }
            out
        },
        &target,
    )
    .await
}

pub async fn schema(workspace: &Path, output: Output, kind: &str) -> Result<()> {
    let _ = workspace;
    let schema = once_frontend::built_in_rule_schema(kind)
        .with_context(|| format!("no built-in rule schema matches `{kind}`"))?;
    write_body(
        output,
        || {
            let mut out = format!("{}: {}\n", schema.kind, schema.docs);
            if !schema.attrs.is_empty() {
                out.push_str("attrs:\n");
                for attr in &schema.attrs {
                    let required = if attr.required {
                        "required"
                    } else {
                        "optional"
                    };
                    let configurable = if attr.configurable {
                        ", configurable"
                    } else {
                        ""
                    };
                    writeln!(
                        out,
                        "  {}: {} ({required}{configurable})",
                        attr.name, attr.ty
                    )
                    .expect("writing to string cannot fail");
                }
            }
            if !schema.capabilities.is_empty() {
                out.push_str("capabilities:\n");
                for capability in &schema.capabilities {
                    writeln!(
                        out,
                        "  {}: {}",
                        capability.name,
                        capability.output_groups.join(", ")
                    )
                    .expect("writing to string cannot fail");
                }
            }
            out
        },
        &schema,
    )
    .await
}

fn render_targets_human(records: &[TargetRecord]) -> String {
    if records.is_empty() {
        return "targets: none\n".to_string();
    }
    let mut out = String::from("targets:\n");
    for target in records {
        let capabilities = if target.capabilities.is_empty() {
            "none".to_string()
        } else {
            target.capabilities.join(", ")
        };
        writeln!(out, "  {} ({}) [{}]", target.id, target.kind, capabilities)
            .expect("writing to string cannot fail");
    }
    out
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
