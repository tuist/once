use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use walkdir::WalkDir;

use super::{observe_absent_link_target, observe_host_path};

/// Expand `patterns` against `package` and return workspace-relative
/// file paths.
///
/// Each match is canonicalized for containment validation but returned by its
/// logical workspace path. This keeps an internal symlink visible to actions
/// that need to materialize its value while rejecting links that point outside
/// the tree. The check is best-effort against the on-disk state at evaluation
/// time: a write-capable attacker on the workspace could in principle swap a
/// symlink between the directory walk and `canonicalize`. Struo treats the workspace
/// as trusted, so this TOCTOU window is out of scope for the threat model.
/// Windows junctions are not exercised by tests yet; the `canonicalize` call
/// covers them in production but a dedicated Windows test should land
/// alongside Windows CI.
///
/// Glob match options used across `srcs`, `exclude`, and change-tracking
/// pattern checks. `require_literal_separator` gives `*` and `?` shell-like
/// semantics: a single `*` matches one path segment only, and matching across
/// path separators requires `**`. Without this, `*.swift` at a package root
/// silently absorbs every `.swift` file in every descendant directory,
/// including files that belong to another package with its own `struo.toml`.
pub(super) const GLOB_MATCH_OPTIONS: glob::MatchOptions = glob::MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};

/// Shorthand for `pattern.matches_with(path, GLOB_MATCH_OPTIONS)` so call
/// sites stay readable when they inline several checks against the same
/// pattern list.
pub(super) fn pattern_matches(pattern: &glob::Pattern, path: &str) -> bool {
    pattern.matches_with(path, GLOB_MATCH_OPTIONS)
}

#[cfg(test)]
pub(in crate::analysis) fn expand_globs(
    workspace_root: &Path,
    package: &str,
    patterns: &[String],
) -> Result<Vec<String>> {
    expand_globs_with_excludes(workspace_root, package, patterns, &[])
}

pub fn expand_globs_with_excludes(
    workspace_root: &Path,
    package: &str,
    patterns: &[String],
    excludes: &[String],
) -> Result<Vec<String>> {
    let package_dir = if package.is_empty() {
        workspace_root.to_path_buf()
    } else {
        workspace_root.join(package)
    };
    let excludes = excludes
        .iter()
        .map(|pattern| {
            glob::Pattern::new(pattern)
                .with_context(|| format!("invalid exclude glob pattern `{pattern}`"))
        })
        .collect::<Result<Vec<_>>>()?;
    let canonical_workspace = std::fs::canonicalize(workspace_root)
        .with_context(|| format!("canonicalizing workspace `{}`", workspace_root.display()))?;
    let patterns = patterns
        .iter()
        .map(|pattern| {
            let normalized = pattern.strip_prefix("./").unwrap_or(pattern);
            glob::Pattern::new(normalized)
                .with_context(|| format!("invalid glob pattern `{pattern}`"))
        })
        .collect::<Result<Vec<_>>>()?;
    let walk_workspace = patterns.iter().any(|pattern| {
        Path::new(pattern.as_str())
            .components()
            .any(|component| component == Component::ParentDir)
    });
    let walk_root = if walk_workspace {
        workspace_root.to_path_buf()
    } else {
        let narrowed = narrow_walk_root(&package_dir, &patterns);
        narrowed.unwrap_or_else(|| package_dir.clone())
    };
    if !walk_root.exists() {
        return Ok(Vec::new());
    }

    let mut out = collect_glob_matches(
        workspace_root,
        package,
        walk_root.as_path(),
        &patterns,
        &excludes,
        &canonical_workspace,
    )?;
    out.sort();
    out.dedup();
    let symlink_roots = out
        .iter()
        .filter(|path| {
            std::fs::symlink_metadata(workspace_root.join(path))
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
        })
        .cloned()
        .collect::<Vec<_>>();
    out.retain(|candidate| {
        !symlink_roots.iter().any(|root| {
            candidate
                .strip_prefix(root)
                .is_some_and(|suffix| suffix.starts_with('/'))
        })
    });
    Ok(out)
}

/// Compute the deepest directory under `package_dir` that could contain
/// files matching any of the compiled `patterns`. Returns `None` when a
/// pattern starts with a wildcard or has no literal directory prefix,
/// meaning the walk cannot be narrowed below `package_dir`.
fn narrow_walk_root(package_dir: &Path, patterns: &[glob::Pattern]) -> Option<PathBuf> {
    let mut common: Option<PathBuf> = None;
    for pattern in patterns {
        let raw = pattern.as_str();
        let wildcard_pos = raw.find(['*', '?', '[']);
        let literal = match wildcard_pos {
            Some(pos) => &raw[..pos],
            None => raw,
        };
        let dir_str = if literal.is_empty() {
            return None;
        } else if let Some(stripped) = literal.strip_suffix('/') {
            stripped
        } else {
            match literal.rfind('/') {
                Some(pos) => &literal[..pos],
                None => return None,
            }
        };
        if dir_str.is_empty() {
            return None;
        }
        // An absolute literal prefix would make `join` discard `package_dir`
        // and could collapse the common ancestor to the filesystem root,
        // walking far more than the package. Fall back to the package walk.
        if Path::new(dir_str).is_absolute() {
            return None;
        }
        let candidate = package_dir.join(dir_str);
        common = Some(match common {
            None => candidate,
            Some(existing) => common_ancestor(&existing, &candidate),
        });
        if common.as_ref().is_some_and(|c| c == package_dir) {
            return None;
        }
    }
    common.filter(|path| path != package_dir)
}

fn common_ancestor(a: &Path, b: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for (comp_a, comp_b) in a.components().zip(b.components()) {
        if comp_a == comp_b {
            result.push(comp_a.as_os_str());
        } else {
            break;
        }
    }
    result
}

fn collect_glob_matches(
    workspace_root: &Path,
    package: &str,
    walk_root: &Path,
    patterns: &[glob::Pattern],
    excludes: &[glob::Pattern],
    canonical_workspace: &Path,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let walker = WalkDir::new(walk_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            let Ok(logical) = entry.path().strip_prefix(workspace_root) else {
                return true;
            };
            if logical
                .components()
                .any(|component| component.as_os_str() == ".struo")
            {
                return false;
            }
            if !entry.file_type().is_dir() {
                return true;
            }
            let Ok(ws_rel) = normalize_logical_workspace_path(logical) else {
                return true;
            };
            let package_relative = workspace_path_relative_to_package(package, &ws_rel);
            let descendant = format!("{package_relative}/__struo_glob_descendant__");
            !excludes.iter().any(|p| pattern_matches(p, &descendant))
        });
    for entry in walker {
        let entry = entry.with_context(|| {
            format!(
                "walking glob root `{}` without following directory symlinks",
                walk_root.display()
            )
        })?;
        if entry.depth() == 0 {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("reading metadata for `{}`", path.display()))?;
        if !metadata.is_file() && !metadata.file_type().is_symlink() {
            continue;
        }
        let logical = path.strip_prefix(workspace_root).with_context(|| {
            format!(
                "glob result `{}` is outside workspace `{}`",
                path.display(),
                workspace_root.display()
            )
        })?;
        let ws_rel = normalize_logical_workspace_path(logical)?;
        let package_relative = workspace_path_relative_to_package(package, &ws_rel);
        let symlink_tree = metadata.file_type().is_symlink() && path.is_dir();
        let descendant = format!("{package_relative}/__struo_glob_descendant__");
        let matches = patterns.iter().any(|p| {
            pattern_matches(p, &package_relative) || symlink_tree && pattern_matches(p, &descendant)
        });
        if !matches {
            continue;
        }
        let is_excluded = excludes
            .iter()
            .any(|p| pattern_matches(p, &package_relative));
        let excluded_symlink_tree =
            symlink_tree && excludes.iter().any(|p| pattern_matches(p, &descendant));
        if is_excluded || excluded_symlink_tree {
            continue;
        }
        // Canonicalizing proves the match cannot reach outside the workspace
        // through a symlink. It fails on a path that resolves to nothing:
        // a file that vanished between the walk and this check, or a link with
        // no target. Drop the match rather than failing the whole graph load,
        // because real trees carry stray links left behind by test suites and
        // interrupted tooling, and one of those should not make a workspace
        // unloadable.
        //
        // Dropping alone is not enough when the link names something outside
        // the workspace. Nothing watches outside the workspace, so if that
        // target were created later no change would be seen, a stored receipt
        // would stay valid, and the source would silently never enter the
        // build. Record the absent target as an observed host path so its
        // appearance invalidates what was built without it.
        //
        // A symlink whose canonical target already exists but lives outside
        // the workspace is treated the same way: skip it and record the
        // observation, so a hybrid project (a Cargo workspace that also runs
        // Bazel, for instance, leaving `bazel-*` convenience symlinks that
        // point into the Bazel output tree) loads cleanly instead of failing
        // the whole graph.
        let canonical = match std::fs::canonicalize(path) {
            Ok(canonical) => canonical,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                observe_absent_link_target(path);
                continue;
            }
            Err(error) => {
                return Err(anyhow::Error::from(error)
                    .context(format!("canonicalizing `{}`", path.display())))
            }
        };
        if canonical.strip_prefix(canonical_workspace).is_err() {
            if metadata.file_type().is_symlink() {
                observe_absent_link_target(path);
                continue;
            }
            // A real file whose canonical location is outside the workspace
            // was reached by descending through a symlinked ancestor. Swift
            // Package Manager's registry integration produces exactly this
            // shape: `.build/registry/downloads/<scope>/<name>/<version>/`
            // holds a `Sources` symlink into `~/.cache/swifterpm/…-registry/`,
            // and the source files inside are then real regulars. The
            // workspace-relative path is what the build tools reference, so
            // keep it in the results, and observe the canonical target so a
            // change to the cache still invalidates the build. The observe
            // is best-effort: outside an active analysis it returns Err, and
            // that is fine because there is no store to record into.
            let _ = observe_host_path(&canonical);
        }
        if !ws_rel.is_empty() {
            out.push(ws_rel);
        }
    }
    Ok(out)
}

fn workspace_path_relative_to_package(package: &str, workspace_path: &str) -> String {
    let package_parts = package
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let path_parts = workspace_path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let common = package_parts
        .iter()
        .zip(path_parts.iter())
        .take_while(|(left, right)| left == right)
        .count();
    std::iter::repeat_n("..", package_parts.len() - common)
        .chain(path_parts[common..].iter().copied())
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize_logical_workspace_path(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(
                value
                    .to_str()
                    .ok_or_else(|| anyhow!("glob result contains non-UTF-8 text"))?,
            ),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(anyhow!(
                        "glob result `{}` escapes the workspace",
                        path.display()
                    ));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(anyhow!(
                    "glob result `{}` is not workspace-relative",
                    path.display()
                ));
            }
        }
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
pub(in crate::analysis) fn walk_package_files(
    workspace_root: &Path,
    package: &str,
    root: &str,
    excluded_paths: &[String],
    excluded_names: &[String],
) -> Result<Vec<String>> {
    walk_package_entries(
        workspace_root,
        package,
        root,
        excluded_paths,
        excluded_names,
        false,
        false,
    )
}

/// Walk a package directory and return its files and symbolic links, plus,
/// when asked, every directory that has no remaining entries. An archive or
/// build context that copies a tree needs those directories to exist, and a
/// list of files alone cannot say so.
pub(super) fn walk_package_entries(
    workspace_root: &Path,
    package: &str,
    root: &str,
    excluded_paths: &[String],
    excluded_names: &[String],
    include_empty_directories: bool,
    symlink_targets: bool,
) -> Result<Vec<String>> {
    let package_dir = if package.is_empty() {
        workspace_root.to_path_buf()
    } else {
        workspace_root.join(package)
    };
    let root = normalize_walk_path(root, "root", true)?;
    let exclusions = excluded_paths
        .iter()
        .map(|path| normalize_walk_path(path, "excluded path", false))
        .collect::<Result<Vec<_>>>()?;
    let excluded_names = excluded_names
        .iter()
        .map(|name| normalize_walk_name(name))
        .collect::<Result<Vec<_>>>()?;
    let canonical_workspace = std::fs::canonicalize(workspace_root)
        .with_context(|| format!("canonicalizing workspace `{}`", workspace_root.display()))?;
    let requested_root = package_dir.join(root);
    let canonical_root = std::fs::canonicalize(&requested_root)
        .with_context(|| format!("canonicalizing walk root `{}`", requested_root.display()))?;
    canonical_root
        .strip_prefix(&canonical_workspace)
        .with_context(|| {
            format!(
                "walk root `{}` is outside the workspace `{}`",
                canonical_root.display(),
                canonical_workspace.display()
            )
        })?;
    if !canonical_root.is_dir() {
        return Err(anyhow!(
            "walk root `{}` is not a directory",
            requested_root.display()
        ));
    }

    let walker = WalkDir::new(&canonical_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            let relative = entry
                .path()
                .strip_prefix(&canonical_root)
                .unwrap_or(entry.path());
            relative.as_os_str().is_empty()
                || (!excluded_names
                    .iter()
                    .any(|excluded| entry.file_name() == excluded)
                    && !exclusions
                        .iter()
                        .any(|excluded| relative == excluded || relative.starts_with(excluded)))
        });
    let mut out = Vec::new();
    let mut empty_directories = EmptyDirectories::default();
    for entry in walker {
        let entry =
            entry.with_context(|| format!("walking directory `{}`", requested_root.display()))?;
        let file_type = entry.file_type();
        if include_empty_directories {
            empty_directories.observe(&entry, &canonical_workspace);
        }
        if !file_type.is_file() && !file_type.is_symlink() {
            continue;
        }
        let workspace_relative = entry
            .path()
            .strip_prefix(&canonical_workspace)
            .with_context(|| {
                format!(
                    "walk result `{}` is outside the workspace `{}`",
                    entry.path().display(),
                    canonical_workspace.display()
                )
            })?
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if symlink_targets {
            if file_type.is_symlink() && !workspace_relative.is_empty() {
                out.push(symlink_record(&workspace_relative, entry.path())?);
            }
            continue;
        }
        if !workspace_relative.is_empty() {
            out.push(workspace_relative);
        }
    }
    out.extend(empty_directories.finish());
    out.sort();
    out.dedup();
    Ok(out)
}

/// Directories seen during a walk, and which of them have an entry inside.
#[derive(Default)]
struct EmptyDirectories {
    directories: std::collections::BTreeSet<String>,
    occupied: std::collections::BTreeSet<String>,
}

impl EmptyDirectories {
    fn observe(&mut self, entry: &walkdir::DirEntry, workspace: &Path) {
        if entry.depth() == 0 {
            return;
        }
        let relative = |path: &Path| {
            path.strip_prefix(workspace)
                .ok()
                .map(|path| path.to_string_lossy().into_owned())
        };
        if let Some(parent) = entry.path().parent().and_then(relative) {
            self.occupied.insert(parent);
        }
        if entry.file_type().is_dir() {
            if let Some(directory) = relative(entry.path()) {
                self.directories.insert(directory);
            }
        }
    }

    fn finish(self) -> Vec<String> {
        self.directories
            .into_iter()
            .filter(|directory| !self.occupied.contains(directory))
            .map(|directory| directory.replace(std::path::MAIN_SEPARATOR, "/"))
            .collect()
    }
}

fn symlink_record(workspace_relative: &str, path: &Path) -> Result<String> {
    let target =
        std::fs::read_link(path).with_context(|| format!("reading link `{}`", path.display()))?;
    Ok(format!(
        "{workspace_relative}\0{}",
        target.to_string_lossy()
    ))
}

fn normalize_walk_path(path: &str, field: &str, allow_empty: bool) -> Result<std::path::PathBuf> {
    let mut normalized = std::path::PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(value) => normalized.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                return Err(anyhow!(
                    "walk_files {field} must stay inside its package, got `{path}`"
                ));
            }
        }
    }
    if !allow_empty && normalized.as_os_str().is_empty() {
        return Err(anyhow!("walk_files {field} must not be empty"));
    }
    Ok(normalized)
}

fn normalize_walk_name(name: &str) -> Result<std::ffi::OsString> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || matches!(name, "." | "..") {
        return Err(anyhow!(
            "walk_files excluded name must be one file name, got `{name}`"
        ));
    }
    Ok(std::ffi::OsString::from(name))
}

#[cfg(test)]
mod narrow_walk_root_tests {
    use super::narrow_walk_root;
    use std::path::{Path, PathBuf};

    fn compile(patterns: &[&str]) -> Vec<glob::Pattern> {
        patterns
            .iter()
            .map(|pattern| glob::Pattern::new(pattern).expect("valid glob"))
            .collect()
    }

    #[test]
    fn narrows_to_literal_prefix() {
        let package = Path::new("/ws");
        let patterns = compile(&["codex-rs/tui/**/*.rs"]);
        assert_eq!(
            narrow_walk_root(package, &patterns),
            Some(PathBuf::from("/ws/codex-rs/tui"))
        );
    }

    #[test]
    fn common_ancestor_across_patterns() {
        let package = Path::new("/ws");
        let patterns = compile(&["codex-rs/tui/**/*.rs", "codex-rs/core/**/*.rs"]);
        assert_eq!(
            narrow_walk_root(package, &patterns),
            Some(PathBuf::from("/ws/codex-rs"))
        );
    }

    #[test]
    fn leading_wildcard_cannot_narrow() {
        let package = Path::new("/ws");
        assert_eq!(narrow_walk_root(package, &compile(&["**/*.rs"])), None);
        assert_eq!(narrow_walk_root(package, &compile(&["*.rs"])), None);
    }

    #[test]
    fn absolute_pattern_does_not_escape_package() {
        // An absolute literal prefix must not make the walk root jump outside
        // the package (previously `join` discarded the package and the common
        // ancestor collapsed toward the filesystem root).
        let package = Path::new("/ws/pkg");
        let patterns = compile(&["/abs/src/*.rs", "src/**/*.rs"]);
        assert_eq!(narrow_walk_root(package, &patterns), None);
        assert_eq!(
            narrow_walk_root(package, &compile(&["/abs/src/*.rs"])),
            None
        );
    }
}
