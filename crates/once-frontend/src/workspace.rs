//! Disk-side loaders for single manifests and recursive workspace scans.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use glob::Pattern;
use walkdir::WalkDir;

use crate::cache_provider::{CacheProviderConfig, InfrastructureConfig};
use crate::error::{Error, Result};
use crate::manifest::{
    load_cache_provider_toml_str, load_infrastructure_toml_str, load_toml_with,
    load_toml_with_configuration, load_workspace_toml_str, BuildConfiguration,
};
use crate::target::Target;
use crate::TOML_BUILD_FILE_NAME;

/// Load a single manifest from disk and return its targets.
pub fn load_file(path: &Path) -> Result<Vec<Target>> {
    let display = path.display().to_string();
    let src = std::fs::read_to_string(path).map_err(|source| Error::Read {
        path: display.clone(),
        source,
    })?;
    let workspace_root = path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    load_toml_with(&display, &src, &workspace_root, "")
}

/// Recursively scan `root` for `once.toml` files and return every
/// script-like target they declare.
pub fn load_workspace(root: &Path) -> Result<Vec<Target>> {
    let scan = load_workspace_scan(root)?;
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            let visible = entry
                .file_name()
                .to_str()
                .is_none_or(|name| !name.starts_with('.'));
            visible
                && (!entry.file_type().is_dir()
                    || entry.depth() != 1
                    || scan.includes_top_level_dir(entry.file_name()))
        });
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for entry in walker {
        let entry = entry.map_err(|source| Error::Walk {
            root: root.display().to_string(),
            source,
        })?;
        if entry.file_type().is_file() && is_manifest_file(entry.file_name().to_str()) {
            let rel_path = entry
                .path()
                .strip_prefix(root)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            if !scan.includes(&rel_path) {
                continue;
            }
            let parent = entry.path().parent().unwrap_or(root);
            let pkg = parent
                .strip_prefix(root)
                .unwrap_or(parent)
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            entries.push((pkg, entry.into_path()));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let mut all = Vec::new();
    for (pkg, path) in entries {
        let src = std::fs::read_to_string(&path).map_err(|source| Error::Read {
            path: path.display().to_string(),
            source,
        })?;
        let filename = path.file_name().unwrap().to_string_lossy();
        let display = if pkg.is_empty() {
            filename.to_string()
        } else {
            format!("{pkg}/{filename}")
        };
        let targets =
            load_toml_with_configuration(&display, &src, root, &pkg, &scan.configuration)?;
        all.extend(targets);
    }
    let target_kind_schemas = crate::graph::target_kind_schemas_for_workspace(root)?;
    let resolver_kinds = target_kind_schemas
        .iter()
        .filter(|schema| schema.has_resolver)
        .map(|schema| schema.kind.clone())
        .collect::<BTreeSet<_>>();
    let native_project_schemas =
        crate::native_project::native_project_schemas_for_workspace_with_target_kinds(
            root,
            &target_kind_schemas,
        )?;
    for (matched, target) in
        crate::native_project::synthesized_workspace_seeds(root, &native_project_schemas, &scan)?
    {
        let marker_path = if matched.package.is_empty() {
            matched.markers[0].clone()
        } else {
            format!("{}/{}", matched.package, matched.markers[0])
        };
        if !all
            .iter()
            .any(|explicit| explicit.package == target.package && explicit.kind == target.kind)
            && !all.iter().any(|explicit| {
                resolver_kinds.contains(&explicit.kind)
                    && target_covers_path(explicit, &marker_path)
            })
        {
            all.push(target);
        }
    }
    Ok(all)
}

fn target_covers_path(target: &Target, path: &str) -> bool {
    let relative = if target.package.is_empty() {
        path
    } else {
        let prefix = format!("{}/", target.package);
        let Some(relative) = path.strip_prefix(&prefix) else {
            return false;
        };
        relative
    };
    let resolver_inputs = resolver_input_patterns(target);
    target
        .srcs
        .iter()
        .chain(resolver_inputs.iter())
        .filter_map(|pattern| Pattern::new(pattern.trim_start_matches("./")).ok())
        .any(|pattern| pattern.matches(relative))
}

fn resolver_input_patterns(target: &Target) -> Vec<String> {
    let Some(crate::AttrValue::List(values)) = target.typed_attrs.get("resolver_inputs") else {
        return Vec::new();
    };
    values
        .iter()
        .filter_map(crate::AttrValue::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

#[derive(Debug, Default)]
pub(crate) struct WorkspaceScan {
    include: Vec<Pattern>,
    include_roots: Option<BTreeSet<String>>,
    exclude: Vec<Pattern>,
    configuration: BuildConfiguration,
}

impl WorkspaceScan {
    fn includes(&self, path: &str) -> bool {
        if self.exclude.iter().any(|pattern| pattern.matches(path)) {
            return false;
        }
        self.include.is_empty() || self.include.iter().any(|pattern| pattern.matches(path))
    }

    pub(crate) fn includes_native_project(&self, manifest_path: &str, marker_path: &str) -> bool {
        if self
            .exclude
            .iter()
            .any(|pattern| pattern.matches(manifest_path) || pattern.matches(marker_path))
        {
            return false;
        }
        self.include.is_empty()
            || self
                .include
                .iter()
                .any(|pattern| pattern.matches(manifest_path) || pattern.matches(marker_path))
    }

    fn includes_top_level_dir(&self, name: &OsStr) -> bool {
        self.include_roots
            .as_ref()
            .is_none_or(|roots| name.to_str().is_none_or(|name| roots.contains(name)))
    }

    pub(crate) fn includes_native_project_dir(&self, path: &str, depth: usize) -> bool {
        if depth == 1 && !self.includes_top_level_dir(OsStr::new(path)) {
            return false;
        }
        let descendant = format!("{path}/__once_native_project_descendant__");
        !self
            .exclude
            .iter()
            .any(|pattern| pattern.matches(&descendant))
    }
}

pub(crate) fn load_workspace_scan(root: &Path) -> Result<WorkspaceScan> {
    let path = root.join(TOML_BUILD_FILE_NAME);
    let src = match std::fs::read_to_string(&path) {
        Ok(src) => src,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(WorkspaceScan {
                configuration: crate::manifest::load_workspace_configuration(root)?,
                ..WorkspaceScan::default()
            });
        }
        Err(source) => {
            return Err(Error::Read {
                path: path.display().to_string(),
                source,
            });
        }
    };
    let raw = load_workspace_toml_str(TOML_BUILD_FILE_NAME, &src)?;
    Ok(WorkspaceScan {
        include_roots: literal_include_roots(&raw.include),
        include: compile_patterns(TOML_BUILD_FILE_NAME, "workspace.include", &raw.include)?,
        exclude: compile_patterns(TOML_BUILD_FILE_NAME, "workspace.exclude", &raw.exclude)?,
        configuration: BuildConfiguration::from_toml(raw.configuration)?,
    })
}

fn literal_include_roots(patterns: &[String]) -> Option<BTreeSet<String>> {
    if patterns.is_empty() {
        return None;
    }
    let mut roots = BTreeSet::new();
    for pattern in patterns {
        let Some((root, _)) = pattern.split_once('/') else {
            if has_glob_syntax(pattern) {
                return None;
            }
            continue;
        };
        if root.is_empty()
            || root == "."
            || root == ".."
            || root.contains('\\')
            || has_glob_syntax(root)
        {
            return None;
        }
        roots.insert(root.to_string());
    }
    Some(roots)
}

fn has_glob_syntax(value: &str) -> bool {
    value.bytes().any(|byte| matches!(byte, b'*' | b'?' | b'['))
}

fn compile_patterns(path: &str, field: &str, values: &[String]) -> Result<Vec<Pattern>> {
    values
        .iter()
        .map(|value| {
            Pattern::new(value).map_err(|source| Error::Eval {
                path: path.to_string(),
                message: format!("invalid `{field}` glob `{value}`: {source}"),
            })
        })
        .collect()
}

/// Load the workspace-level cache provider config from the root
/// `once.toml`.
pub fn load_cache_provider_override(root: &Path) -> Result<Option<CacheProviderConfig>> {
    let path = root.join(TOML_BUILD_FILE_NAME);
    let src = match std::fs::read_to_string(&path) {
        Ok(src) => src,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(source) => {
            return Err(Error::Read {
                path: path.display().to_string(),
                source,
            });
        }
    };
    load_cache_provider_toml_str(TOML_BUILD_FILE_NAME, &src)
}

/// Load the workspace-level infrastructure config from the root
/// `once.toml`.
pub fn load_infrastructure_config(root: &Path) -> Result<InfrastructureConfig> {
    let path = root.join(TOML_BUILD_FILE_NAME);
    let src = match std::fs::read_to_string(&path) {
        Ok(src) => src,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InfrastructureConfig::default());
        }
        Err(source) => {
            return Err(Error::Read {
                path: path.display().to_string(),
                source,
            });
        }
    };
    load_infrastructure_toml_str(TOML_BUILD_FILE_NAME, &src)
}

/// Load the workspace-level cache provider config from the root
/// `once.toml`. Missing files or missing config default to the local
/// on-disk provider.
pub fn load_cache_provider(root: &Path) -> Result<CacheProviderConfig> {
    Ok(load_cache_provider_override(root)?.unwrap_or(CacheProviderConfig::Local))
}

fn is_manifest_file(name: Option<&str>) -> bool {
    matches!(name, Some(TOML_BUILD_FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn root_workspace_scan_filters_manifest_paths() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("once.toml"),
            r#"
[workspace]
include = ["crates/*/once.toml"]
exclude = ["crates/skip/once.toml"]
"#,
        );
        write(
            &tmp.path().join("crates/keep/once.toml"),
            r#"
[[target]]
name = "keep"
kind = "custom_library"
"#,
        );
        write(
            &tmp.path().join("crates/skip/once.toml"),
            r#"
[[target]]
name = "skip"
kind = "custom_library"
"#,
        );
        write(
            &tmp.path().join("fixtures/example/once.toml"),
            r#"
[[target]]
name = "fixture"
kind = "custom_library"
"#,
        );

        let targets = load_workspace(tmp.path()).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id(), "crates/keep/keep");
    }

    #[test]
    fn literal_include_roots_prune_unrelated_top_level_directories() {
        assert_eq!(
            literal_include_roots(&[
                "crates/*/once.toml".to_string(),
                "tools/generator/once.toml".to_string(),
            ]),
            Some(BTreeSet::from(["crates".to_string(), "tools".to_string(),]))
        );
        assert_eq!(
            literal_include_roots(&["once.toml".to_string()]),
            Some(BTreeSet::new())
        );
    }

    #[test]
    fn wildcard_include_roots_preserve_full_traversal() {
        assert_eq!(literal_include_roots(&[]), None);
        assert_eq!(literal_include_roots(&["*/once.toml".to_string()]), None);
        assert_eq!(
            literal_include_roots(&["crates*/once.toml".to_string()]),
            None
        );
    }

    #[test]
    fn native_project_supplies_a_seed_when_no_targets_are_declared() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("mix.exs"),
            "defmodule Demo.MixProject do\n  use Mix.Project\nend\n",
        );

        let targets = load_workspace(tmp.path()).unwrap();

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id(), "mix");
        assert_eq!(targets[0].kind, "mix_workspace");
    }

    #[test]
    fn configuration_only_manifest_still_allows_native_project_discovery() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("once.toml"),
            "[infrastructure.cache]\nprovider = \"local\"\n",
        );
        write(
            &tmp.path().join("mix.exs"),
            "defmodule Demo.MixProject do\n  use Mix.Project\nend\n",
        );

        let targets = load_workspace(tmp.path()).unwrap();

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id(), "mix");
    }

    #[test]
    fn workspace_scan_can_exclude_a_native_project_marker() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("once.toml"),
            r#"[workspace]
exclude = ["mix.exs"]

[[target]]
name = "lint"
kind = "custom_library"
"#,
        );
        write(
            &tmp.path().join("mix.exs"),
            "defmodule Demo.MixProject do\n  use Mix.Project\nend\n",
        );

        let targets = load_workspace(tmp.path()).unwrap();

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id(), "lint");
    }

    #[test]
    fn unrelated_explicit_targets_do_not_hide_native_projects() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("once.toml"),
            "[[target]]\nname = \"explicit\"\nkind = \"custom_library\"\n",
        );
        write(
            &tmp.path().join("mix.exs"),
            "defmodule Demo.MixProject do\n  use Mix.Project\nend\n",
        );

        let targets = load_workspace(tmp.path()).unwrap();

        let ids = targets
            .into_iter()
            .map(|target| target.id())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["explicit", "mix"]);
    }

    #[test]
    fn explicit_target_in_one_package_does_not_hide_other_native_projects() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("once.toml"),
            r#"[workspace]
include = ["once.toml", "apps/*/once.toml"]

[[target]]
name = "docs"
kind = "custom_library"
"#,
        );
        for package in ["api", "web"] {
            write(
                &tmp.path().join(format!("apps/{package}/mix.exs")),
                "defmodule Demo.MixProject do\n  use Mix.Project\nend\n",
            );
        }

        let targets = load_workspace(tmp.path()).unwrap();
        let ids = targets
            .into_iter()
            .map(|target| target.id())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["docs", "apps/api/mix", "apps/web/mix"]);
    }

    #[test]
    fn non_resolver_target_does_not_claim_nested_native_project_markers() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("once.toml"),
            r#"[[target]]
name = "sources"
kind = "custom_library"
srcs = ["apps/**/mix.exs"]
"#,
        );
        write(
            &tmp.path().join("apps/api/mix.exs"),
            "defmodule Demo.MixProject do\n  use Mix.Project\nend\n",
        );

        let targets = load_workspace(tmp.path()).unwrap();
        let ids = targets
            .into_iter()
            .map(|target| target.id())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["sources", "apps/api/mix"]);
    }

    #[test]
    fn resolver_target_claims_nested_native_project_markers_it_covers() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("once.toml"),
            r#"[[target]]
name = "workspace"
kind = "mix_workspace"
srcs = ["apps/**/mix.exs"]
"#,
        );
        write(
            &tmp.path().join("apps/api/mix.exs"),
            "defmodule Demo.MixProject do\n  use Mix.Project\nend\n",
        );

        let targets = load_workspace(tmp.path()).unwrap();
        let ids = targets
            .into_iter()
            .map(|target| target.id())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["workspace"]);
    }
}
