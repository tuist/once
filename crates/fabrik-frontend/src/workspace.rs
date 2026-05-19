//! Disk-side loaders: a single file ([`load_file`]) and a recursive
//! workspace walk ([`load_workspace`]).

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::dependency::{
    external_target_id, parse_generated_external_format, DependencyEntry,
    EXTERNAL_PACKAGE_CACHE_ROOT, GENERATED_EXTERNAL_FORMAT_VERSION,
};
use crate::error::{Error, Result};
use crate::manifest::{load_dependency_entries_toml_with, load_toml_with};
use crate::target::Target;
use crate::TOML_BUILD_FILE_NAME;

/// Load a single build file from disk and return its targets.
/// The file's parent directory is treated as the workspace root and the
/// package is empty; intended for cases where the caller has already
/// located one file. Use [`load_workspace`] to walk a tree.
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

/// Recursively scan `root` for build files and return every
/// target they declare, with `Target::package` set to the workspace
/// path of each enclosing directory.
///
/// Hidden directories (`.git`, the `.fabrik` cache tree, etc.) are
/// skipped. Targets are returned in package, then source, order.
pub fn load_workspace(root: &Path) -> Result<Vec<Target>> {
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            // Skip hidden directories (anything starting with '.') so we
            // don't recurse into the cache or VCS metadata. The root
            // itself is exempted because callers may legitimately point
            // at a path whose final component starts with a dot.
            if entry.depth() == 0 {
                return true;
            }
            entry
                .file_name()
                .to_str()
                .is_none_or(|name| !name.starts_with('.'))
        });
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for entry in walker {
        let entry = entry.map_err(|source| Error::Walk {
            root: root.display().to_string(),
            source,
        })?;
        if entry.file_type().is_file() && is_build_file(entry.file_name().to_str()) {
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
        let targets = load_toml_with(&display, &src, root, &pkg)?;
        all.extend(targets);
    }
    all.extend(load_external_packages(root)?);
    Ok(all)
}

fn load_external_packages(root: &Path) -> Result<Vec<Target>> {
    let external_root = root.join(EXTERNAL_PACKAGE_CACHE_ROOT);
    if !external_root.is_dir() {
        return Ok(Vec::new());
    }

    let mut entries: Vec<(String, String, PathBuf)> = Vec::new();
    for entry in WalkDir::new(&external_root).follow_links(false) {
        let entry = entry.map_err(|source| Error::Walk {
            root: external_root.display().to_string(),
            source,
        })?;
        if !entry.file_type().is_file() || !is_build_file(entry.file_name().to_str()) {
            continue;
        }
        let parent = entry.path().parent().unwrap_or(&external_root);
        let external_package = parent
            .strip_prefix(&external_root)
            .unwrap_or(parent)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        if external_package.is_empty() {
            continue;
        }
        let source_package = parent
            .strip_prefix(root)
            .unwrap_or(parent)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        entries.push((external_package, source_package, entry.into_path()));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)));

    let mut all = Vec::new();
    for (external_package, source_package, path) in entries {
        let src = std::fs::read_to_string(&path).map_err(|source| Error::Read {
            path: path.display().to_string(),
            source,
        })?;
        // A stamped file from a different generator is refused so a
        // stale `.fabrik/external` tree fails loudly instead of feeding
        // a silently wrong graph into the build. A missing stamp is
        // tolerated: hand-authored and pre-versioning packages stay
        // loadable.
        if let Some(found) = parse_generated_external_format(&src) {
            if found != GENERATED_EXTERNAL_FORMAT_VERSION {
                return Err(Error::IncompatibleGeneratedFormat {
                    path: path.display().to_string(),
                    found,
                    expected: GENERATED_EXTERNAL_FORMAT_VERSION,
                });
            }
        }
        let display = format!("{source_package}/{TOML_BUILD_FILE_NAME}");
        let mut targets = load_toml_with(&display, &src, root, &source_package)?;
        for target in &mut targets {
            target.external_package = Some(external_package.clone());
            remap_external_target_deps(target, &source_package, &external_package);
        }
        all.extend(targets);
    }
    Ok(all)
}

fn remap_external_target_deps(target: &mut Target, source_package: &str, external_package: &str) {
    let source_prefix = format!("{source_package}/");
    let external_root_prefix = format!("{EXTERNAL_PACKAGE_CACHE_ROOT}/");
    for dep in &mut target.deps {
        if let Some(name) = dep.strip_prefix(&source_prefix) {
            *dep = external_target_id(external_package, name);
        } else if let Some(external_name) = dep.strip_prefix(&external_root_prefix) {
            if let Some((external_package, name)) = external_name.rsplit_once('/') {
                *dep = external_target_id(external_package, name);
            }
        }
    }
}

/// Load workspace-level dependency entries from the root `fabrik.toml`.
pub fn load_dependency_entries(root: &Path) -> Result<Vec<DependencyEntry>> {
    let path = root.join(TOML_BUILD_FILE_NAME);
    let src = std::fs::read_to_string(&path).map_err(|source| Error::Read {
        path: path.display().to_string(),
        source,
    })?;
    load_dependency_entries_toml_with(TOML_BUILD_FILE_NAME, &src, "")
}

fn is_build_file(name: Option<&str>) -> bool {
    matches!(name, Some(TOML_BUILD_FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn workspace_walk_finds_packages_and_attaches_ids() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        std::fs::create_dir_all(root.join("crates/a")).unwrap();
        std::fs::create_dir_all(root.join("crates/b")).unwrap();
        std::fs::create_dir_all(root.join(".fabrik/should-be-skipped")).unwrap();

        std::fs::write(
            root.join("fabrik.toml"),
            r#"
[[rust.binary]]
name = "top"
srcs = ["main.rs"]
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("crates/a/fabrik.toml"),
            r#"
[[rust.library]]
name = "a"
srcs = ["lib.rs"]
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("crates/b/fabrik.toml"),
            r#"
[[rust.binary]]
name = "b"
srcs = ["main.rs"]
"#,
        )
        .unwrap();
        std::fs::write(
            root.join(".fabrik/should-be-skipped/fabrik.toml"),
            r#"
[[rust.binary]]
name = "hidden"
srcs = ["x.rs"]
"#,
        )
        .unwrap();

        let ids: Vec<_> = load_workspace(root)
            .unwrap()
            .into_iter()
            .map(|t| t.id())
            .collect();
        assert_eq!(
            ids,
            vec!["top", "crates/a/a", "crates/b/b"],
            "expected three ids in package-sorted order"
        );
    }

    #[test]
    fn workspace_walk_loads_toml_build_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        std::fs::create_dir_all(root.join("crates/a")).unwrap();
        std::fs::write(
            root.join("crates/a/fabrik.toml"),
            r#"
[[rust.library]]
name = "a"
srcs = ["lib.rs"]
"#,
        )
        .unwrap();

        let ids: Vec<_> = load_workspace(root)
            .unwrap()
            .into_iter()
            .map(|t| t.id())
            .collect();
        assert_eq!(ids, vec!["crates/a/a"]);
    }

    #[test]
    fn workspace_walk_loads_generated_external_packages_outside_local_namespace() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        std::fs::create_dir_all(root.join(".fabrik/external/cargo")).unwrap();
        std::fs::write(
            root.join(".fabrik/external/cargo/fabrik.toml"),
            r#"
[[rust.library]]
name = "dep"
srcs = ["dep/src/lib.rs"]
deps = ["helper"]

[[rust.library]]
name = "helper"
srcs = ["helper/src/lib.rs"]
"#,
        )
        .unwrap();

        let targets = load_workspace(root).unwrap();
        let ids: Vec<_> = targets.iter().map(Target::id).collect();
        assert_eq!(ids, vec!["external:cargo/dep", "external:cargo/helper"]);
        assert_eq!(targets[0].package, ".fabrik/external/cargo");
        assert_eq!(targets[0].deps, vec!["external:cargo/helper"]);
    }

    #[test]
    fn a_correctly_stamped_external_package_loads() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".fabrik/external/cargo")).unwrap();
        std::fs::write(
            root.join(".fabrik/external/cargo/fabrik.toml"),
            format!(
                "{}\n[[rust.library]]\nname = \"dep\"\nsrcs = [\"dep/src/lib.rs\"]\n",
                crate::generated_external_format_header()
            ),
        )
        .unwrap();

        let ids: Vec<_> = load_workspace(root)
            .unwrap()
            .into_iter()
            .map(|t| t.id())
            .collect();
        assert_eq!(ids, vec!["external:cargo/dep"]);
    }

    #[test]
    fn an_incompatibly_stamped_external_package_is_refused() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".fabrik/external/cargo")).unwrap();
        let stale = GENERATED_EXTERNAL_FORMAT_VERSION + 1;
        std::fs::write(
            root.join(".fabrik/external/cargo/fabrik.toml"),
            format!(
                "# fabrik:generated-external-format={stale}\n[[rust.library]]\nname = \"dep\"\n"
            ),
        )
        .unwrap();

        let err = load_workspace(root).unwrap_err();
        assert!(
            matches!(err, Error::IncompatibleGeneratedFormat { found, expected, .. }
                if found == stale && expected == GENERATED_EXTERNAL_FORMAT_VERSION),
            "unexpected error: {err}"
        );
        assert!(err.to_string().contains("re-run `fabrik deps sync`"));
    }

    #[test]
    fn glob_returns_package_relative_paths() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        std::fs::create_dir_all(root.join("pkg/src")).unwrap();
        std::fs::write(root.join("pkg/src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("pkg/src/lib.rs"), "pub fn hi() {}\n").unwrap();
        std::fs::write(root.join("pkg/README.md"), "ignored\n").unwrap();
        std::fs::write(
            root.join("pkg/fabrik.toml"),
            r#"
[[rust.binary]]
name = "pkg"
src_globs = ["src/*.rs"]
"#,
        )
        .unwrap();

        let targets = load_workspace(root).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].package, "pkg");
        assert_eq!(
            targets[0].srcs,
            vec!["src/lib.rs".to_string(), "src/main.rs".to_string()],
            "expected sorted package-relative srcs"
        );
    }

    #[test]
    fn glob_with_no_matches_yields_empty_srcs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("pkg")).unwrap();
        std::fs::write(
            root.join("pkg/fabrik.toml"),
            r#"
[[rust.binary]]
name = "empty"
src_globs = ["src/*.rs"]
"#,
        )
        .unwrap();
        let targets = load_workspace(root).unwrap();
        assert_eq!(targets[0].srcs, Vec::<String>::new());
    }
}
