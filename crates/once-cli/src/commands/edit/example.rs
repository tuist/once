use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

mod paths;
mod relocation;

use paths::{
    example_manifest_packages, join_display_path, normalize_relative_path, resolve_example_path,
    unsafe_path_reason,
};
use relocation::relocate_manifest_references;

#[derive(Debug, Serialize)]
pub(super) struct MaterializeExampleResult {
    materialized: bool,
    kind: String,
    slug: String,
    destination: String,
    created_files: Vec<String>,
    unchanged_files: Vec<String>,
    conflicts: Vec<FileConflict>,
    targets: Vec<MaterializedTarget>,
    workspace_validation: Option<Value>,
    suggested_calls: Vec<SuggestedCall>,
}

#[derive(Debug, Serialize)]
struct FileConflict {
    path: String,
    reason: String,
}

#[derive(Debug, Serialize)]
struct MaterializedTarget {
    id: String,
    kind: String,
    capabilities: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SuggestedCall {
    tool: &'static str,
    arguments: Value,
    reason: String,
}

struct PendingFile {
    relative_path: String,
    path: PathBuf,
    contents: String,
}

struct ExamplePreparation {
    destination_label: String,
    pending: Vec<PendingFile>,
    conflicts: Vec<FileConflict>,
    unchanged_files: Vec<String>,
}

pub(crate) fn materialize_example_json(
    workspace: &Path,
    kind: &str,
    slug: &str,
    destination: &str,
) -> Result<Value> {
    Ok(serde_json::to_value(materialize_example_value(
        workspace,
        kind,
        slug,
        destination,
    )?)?)
}

pub(super) fn materialize_example_value(
    workspace: &Path,
    kind: &str,
    slug: &str,
    destination: &str,
) -> Result<MaterializeExampleResult> {
    let ExamplePreparation {
        destination_label,
        pending,
        conflicts,
        mut unchanged_files,
    } = prepare_example(workspace, kind, slug, destination)?;

    if !conflicts.is_empty() {
        return Ok(MaterializeExampleResult {
            materialized: false,
            kind: kind.to_string(),
            slug: slug.to_string(),
            destination: destination_label,
            created_files: Vec::new(),
            unchanged_files,
            conflicts,
            targets: Vec::new(),
            workspace_validation: None,
            suggested_calls: Vec::new(),
        });
    }

    let mut created_files = Vec::with_capacity(pending.len());
    for file in pending {
        let parent = file
            .path
            .parent()
            .context("example file path has no parent directory")?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating example directory `{}`", parent.display()))?;
        std::fs::write(&file.path, file.contents)
            .with_context(|| format!("writing example file `{}`", file.path.display()))?;
        created_files.push(file.relative_path);
    }

    created_files.sort();
    unchanged_files.sort();
    let manifest_packages = example_manifest_packages(
        &destination_label,
        created_files.iter().chain(unchanged_files.iter()),
    );
    let graph = once_frontend::load_graph_workspace(workspace).context(
        "loading the workspace after materializing the example; the files were written successfully",
    )?;
    let mut targets = graph
        .iter()
        .filter(|target| manifest_packages.contains(&target.label.package))
        .map(|target| MaterializedTarget {
            id: target.label.id.clone(),
            kind: target.kind.clone(),
            capabilities: target
                .capabilities
                .iter()
                .map(|capability| capability.name.clone())
                .collect(),
        })
        .collect::<Vec<_>>();
    targets.sort_by(|left, right| left.id.cmp(&right.id));

    let diagnostics = once_frontend::validate_workspace_graph(workspace, &graph)?;
    let workspace_validation = json!({
        "valid": diagnostics.is_empty(),
        "target_count": graph.len(),
        "diagnostics": diagnostics,
    });
    let suggested_calls = suggested_calls(kind, &targets);

    Ok(MaterializeExampleResult {
        materialized: true,
        kind: kind.to_string(),
        slug: slug.to_string(),
        destination: destination_label,
        created_files,
        unchanged_files,
        conflicts: Vec::new(),
        targets,
        workspace_validation: Some(workspace_validation),
        suggested_calls,
    })
}

fn prepare_example(
    workspace: &Path,
    kind: &str,
    slug: &str,
    destination: &str,
) -> Result<ExamplePreparation> {
    let schema = once_frontend::target_kind_schemas_for_workspace(workspace)?
        .into_iter()
        .find(|schema| schema.kind == kind)
        .with_context(|| format!("no target kind schema matches `{kind}`"))?;
    let bundle = once_frontend::load_target_kind_example(&schema, slug)?;
    let destination_dir = super::resolve_package_dir(workspace, destination)?;
    let destination_label = normalize_relative_path(destination);
    let mut files = bundle.files;
    relocate_manifest_references(&mut files, &destination_label)?;

    let mut pending = Vec::with_capacity(files.len());
    let mut conflicts = Vec::new();
    let mut unchanged_files = Vec::new();
    for file in files {
        let relative_path = join_display_path(&destination_label, &file.path);
        let path = resolve_example_path(workspace, &destination_dir, &file.path)?;
        if let Some(reason) = unsafe_path_reason(workspace, &path)? {
            conflicts.push(FileConflict {
                path: relative_path,
                reason,
            });
            continue;
        }
        match std::fs::read(&path) {
            Ok(existing) if existing == file.contents.as_bytes() => {
                unchanged_files.push(relative_path);
            }
            Ok(_) => conflicts.push(FileConflict {
                path: relative_path,
                reason: "an existing file has different contents".to_string(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                pending.push(PendingFile {
                    relative_path,
                    path,
                    contents: file.contents,
                });
            }
            Err(error) => conflicts.push(FileConflict {
                path: relative_path,
                reason: format!("could not read the existing path: {error}"),
            }),
        }
    }

    Ok(ExamplePreparation {
        destination_label,
        pending,
        conflicts,
        unchanged_files,
    })
}

fn suggested_calls(kind: &str, targets: &[MaterializedTarget]) -> Vec<SuggestedCall> {
    let mut suggested_calls = vec![SuggestedCall {
        tool: "once_validate_workspace",
        arguments: json!({}),
        reason: "Confirm the complete workspace after any customization.".to_string(),
    }];
    for target in targets.iter().filter(|target| target.kind == kind) {
        if target.capabilities.iter().any(|name| name == "build") {
            suggested_calls.push(SuggestedCall {
                tool: "once_build_target",
                arguments: json!({ "target": target.id }),
                reason: format!("Build the materialized `{}` target.", target.kind),
            });
        }
        if target.capabilities.iter().any(|name| name == "run") {
            suggested_calls.push(SuggestedCall {
                tool: "once_run_target",
                arguments: json!({ "target": target.id }),
                reason: format!("Run the materialized `{}` target.", target.kind),
            });
        }
        if target.capabilities.iter().any(|name| name == "test") {
            suggested_calls.push(SuggestedCall {
                tool: "once_run_tests",
                arguments: json!({ "target": target.id }),
                reason: format!("Test the materialized `{}` target.", target.kind),
            });
        }
    }
    suggested_calls
}

pub(super) fn render_materialize_example_human(result: &MaterializeExampleResult) -> String {
    if !result.materialized {
        let mut text = format!(
            "example {} was not materialized ({} conflicts)\n",
            result.slug,
            result.conflicts.len()
        );
        for conflict in &result.conflicts {
            writeln!(&mut text, "  {}: {}", conflict.path, conflict.reason).ok();
        }
        return text;
    }
    format!(
        "materialized example {} at {} ({} created, {} unchanged, {} targets)\n",
        result.slug,
        if result.destination.is_empty() {
            "."
        } else {
            &result.destination
        },
        result.created_files.len(),
        result.unchanged_files.len(),
        result.targets.len()
    )
}

#[cfg(test)]
mod tests;
