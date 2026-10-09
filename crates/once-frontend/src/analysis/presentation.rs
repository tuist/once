use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use starlark::values::dict::DictRef;
use starlark::values::Value;

use super::store::{with_store, with_store_mut, DeclaredAction};
use super::values::unpack_string_list;

#[derive(Debug, Default)]
pub(super) struct SourceFileCache {
    workspaces: Mutex<BTreeMap<PathBuf, Arc<WorkspaceSourceFiles>>>,
}

#[derive(Debug)]
struct WorkspaceSourceFiles {
    prefix: Option<String>,
    files: Mutex<BTreeMap<String, Option<String>>>,
}

impl SourceFileCache {
    pub(super) fn resolve(&self, workspace: &Path, paths: Vec<String>) -> Vec<String> {
        let cache = self
            .workspaces
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(workspace.to_path_buf())
            .or_insert_with(|| {
                Arc::new(WorkspaceSourceFiles {
                    prefix: repository_prefix(
                        workspace,
                        std::env::var_os("GIT_WORK_TREE").as_deref(),
                    ),
                    files: Mutex::default(),
                })
            })
            .clone();
        let Some(prefix) = &cache.prefix else {
            return Vec::new();
        };
        let mut seen = BTreeSet::new();
        paths
            .into_iter()
            .filter_map(|path| {
                let path = path.strip_prefix("./").unwrap_or(&path);
                if !portable_path(path) || path.starts_with(".once/") {
                    return None;
                }
                let cached = cache
                    .files
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(path)
                    .cloned();
                let link = cached.unwrap_or_else(|| {
                    let link = workspace
                        .join(path)
                        .is_file()
                        .then(|| format!("{prefix}{path}"));
                    cache
                        .files
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .entry(path.to_string())
                        .or_insert(link)
                        .clone()
                })?;
                seen.insert(link.clone()).then_some(link)
            })
            .collect()
    }
}

fn repository_prefix(workspace: &Path, work_tree: Option<&OsStr>) -> Option<String> {
    let relative = if let Some(work_tree) = work_tree {
        let root = workspace.join(work_tree).canonicalize().ok()?;
        workspace
            .canonicalize()
            .ok()?
            .strip_prefix(root)
            .ok()?
            .to_path_buf()
    } else {
        let root = workspace
            .ancestors()
            .find(|root| root.join(".git").exists())
            .unwrap_or(workspace);
        workspace.strip_prefix(root).ok()?.to_path_buf()
    };
    let prefix = relative.to_string_lossy().replace('\\', "/");
    if prefix.is_empty() {
        Some(prefix)
    } else {
        portable_path(&prefix).then(|| format!("{prefix}/"))
    }
}

pub(super) fn unpack_source_files(value: Option<Value<'_>>) -> Result<Vec<String>> {
    let Some(value) = value.filter(|value| !value.is_none()) else {
        return Ok(Vec::new());
    };
    if let Some(dict) = DictRef::from_value(value) {
        let paths = dict.get_str("_once_workspace_source_files").ok_or_else(|| anyhow::anyhow!("source_files must be a list of repository-relative paths or an _action_source_files descriptor"))?;
        let paths = unpack_string_list(paths, "source_files")?;
        // Presentation-only filesystem probes must not enter execution observations
        // or receipt fingerprints, otherwise Git activity invalidates no-op builds.
        return Ok(with_store(|store| {
            store.map_or_else(Vec::new, |store| {
                store
                    .host_cache
                    .source_files
                    .resolve(&store.workspace_root, paths)
            })
        }));
    }
    let paths = unpack_string_list(value, "source_files")?;
    let mut unique = Vec::new();
    let mut seen = BTreeSet::new();
    for path in paths {
        if !portable_path(&path) {
            anyhow::bail!("source_files must contain repository-relative file paths with forward slashes, got `{path}`");
        }
        if seen.insert(path.clone()) {
            unique.push(path);
        }
    }
    Ok(unique)
}

pub(super) fn unpack_presentation(
    value: Option<Value<'_>>,
) -> Option<once_presentation::ActionPresentation> {
    let value = value.filter(|value| !value.is_none())?;
    value
        .to_json_value()
        .ok()
        .and_then(|value| {
            serde_json::from_value::<once_presentation::ActionPresentation>(value).ok()
        })
        .and_then(once_presentation::ActionPresentation::normalized)
}

pub(super) fn record_action(mut action: DeclaredAction) {
    action.display_name = normalize_display_name(action.display_name);
    with_store_mut(|store| {
        if let Some(store) = store {
            store.actions.push(action);
        }
    });
}

fn normalize_display_name(name: Option<String>) -> Option<String> {
    name.map(|name| {
        name.chars()
            .map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            })
            .collect()
    })
}

fn portable_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':'])
        && !path.chars().any(char::is_control)
        && !path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_single_line_without_control_characters() {
        assert_eq!(normalize_display_name(None), None);
        assert_eq!(
            normalize_display_name(Some("Compile\nmain.c\u{1b}".into())).as_deref(),
            Some("Compile main.c ")
        );
    }

    #[cfg(unix)]
    #[test]
    fn nonportable_repository_prefix_is_omitted_without_failing_analysis() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        let workspace = repo.path().join("app:one");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::write(workspace.join("main.c"), "source").unwrap();
        assert!(SourceFileCache::default()
            .resolve(&workspace, vec!["main.c".into()])
            .is_empty());
    }

    #[test]
    fn explicit_git_work_tree_can_live_outside_the_git_directory() {
        let repo = tempfile::tempdir().unwrap();
        let workspace = repo.path().join("projects/app");
        std::fs::create_dir_all(&workspace).unwrap();
        for work_tree in [repo.path().as_os_str(), OsStr::new("../..")] {
            assert_eq!(
                repository_prefix(&workspace, Some(work_tree)).as_deref(),
                Some("projects/app/")
            );
        }
        let unrelated = tempfile::tempdir().unwrap();
        assert_eq!(
            repository_prefix(&workspace, Some(unrelated.path().as_os_str())),
            None
        );
    }
}
