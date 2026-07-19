use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use once_core::WorkspacePath;
use once_frontend::GraphTarget;

pub(super) fn target_input_patterns(
    workspace: &Path,
    graph: &[GraphTarget],
) -> BTreeMap<String, Vec<InputPattern>> {
    let analyzer = once_frontend::analysis::AnalysisEngine::for_workspace(workspace).ok();
    graph
        .iter()
        .map(|target| {
            let mut patterns = target
                .srcs
                .iter()
                .filter_map(|pattern| {
                    InputPattern::package_relative(&target.label.package, pattern)
                })
                .collect::<Vec<_>>();
            if let Some(provider) = metadata_provider(analyzer.as_ref(), workspace, target) {
                patterns.extend(
                    provider_string_list(&provider, "affected_inputs")
                        .into_iter()
                        .filter_map(|pattern| InputPattern::workspace_relative(&pattern)),
                );
            }
            (target.label.id.clone(), patterns)
        })
        .collect()
}

pub(super) fn workspace_graph_input_patterns(workspace: &Path) -> Result<Vec<InputPattern>> {
    let manifest_path = workspace.join(once_frontend::TOML_BUILD_FILE_NAME);
    let source = match std::fs::read_to_string(&manifest_path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("reading workspace manifest"),
    };
    let value: toml::Value = toml::from_str(&source).context("parsing workspace manifest")?;
    let module_patterns = value
        .get("modules")
        .or_else(|| value.get("rules"))
        .and_then(|modules| modules.get("paths"))
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .filter_map(InputPattern::workspace_relative);
    Ok(std::iter::once(InputPattern::Exact(
        once_frontend::TOML_BUILD_FILE_NAME.to_string(),
    ))
    .chain(module_patterns)
    .collect())
}

pub(super) fn is_graph_input(path: &str, configured_patterns: &[InputPattern]) -> bool {
    path == once_frontend::TOML_BUILD_FILE_NAME
        || path.ends_with(&format!("/{}", once_frontend::TOML_BUILD_FILE_NAME))
        || configured_patterns
            .iter()
            .any(|pattern| pattern.matches(path))
}

#[derive(Debug)]
pub(super) enum InputPattern {
    Exact(String),
    Glob(glob::Pattern),
}

impl InputPattern {
    fn package_relative(package: &str, pattern: &str) -> Option<Self> {
        let path = if package.is_empty() {
            pattern.to_string()
        } else {
            format!("{package}/{pattern}")
        };
        Self::workspace_relative(&path)
    }

    fn workspace_relative(pattern: &str) -> Option<Self> {
        if !safe_pattern(pattern) {
            return None;
        }
        if has_glob_metacharacters(pattern) {
            glob::Pattern::new(pattern).ok().map(Self::Glob)
        } else {
            WorkspacePath::try_from(pattern)
                .ok()
                .map(|path| Self::Exact(path.to_string()))
        }
    }

    pub(super) fn matches(&self, path: &str) -> bool {
        match self {
            Self::Exact(expected) => expected == path,
            Self::Glob(pattern) => pattern.matches(path),
        }
    }
}

fn has_glob_metacharacters(pattern: &str) -> bool {
    pattern
        .chars()
        .any(|character| matches!(character, '?' | '*' | '['))
}

fn safe_pattern(pattern: &str) -> bool {
    let path = Path::new(pattern);
    !path.is_absolute()
        && path.components().all(|component| {
            matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
}

fn metadata_provider(
    analyzer: Option<&once_frontend::analysis::AnalysisEngine>,
    workspace: &Path,
    target: &GraphTarget,
) -> Option<serde_json::Value> {
    let analyzer = analyzer?;
    let analysis = analyzer
        .analyze_target_capability(target, workspace, &[], "metadata")
        .ok()?;
    Some(analysis.provider)
}

fn provider_string_list(provider: &serde_json::Value, key: &str) -> Vec<String> {
    provider
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
