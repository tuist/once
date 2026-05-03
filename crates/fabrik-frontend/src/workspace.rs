//! Disk-side loaders: a single file ([`load_file`]) and a recursive
//! workspace walk ([`load_workspace`]).

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::error::{Error, Result};
use crate::eval::eval_with;
use crate::target::Target;
use crate::BUILD_FILE_NAME;

/// Load a single `fabrik.star` file from disk and return its targets.
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
    eval_with(&display, &src, workspace_root, String::new())
}

/// Recursively scan `root` for `fabrik.star` files and return every
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
        if entry.file_type().is_file() && entry.file_name() == BUILD_FILE_NAME {
            let parent = entry.path().parent().unwrap_or(root);
            let pkg = parent
                .strip_prefix(root)
                .unwrap_or(parent)
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            entries.push((pkg, entry.into_path()));
        }
    }
    // Sort by package string so the root package ("") comes first and
    // siblings appear alphabetically.
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut all = Vec::new();
    for (pkg, path) in entries {
        let src = std::fs::read_to_string(&path).map_err(|source| Error::Read {
            path: path.display().to_string(),
            source,
        })?;
        let display = if pkg.is_empty() {
            format!("//:{BUILD_FILE_NAME}")
        } else {
            format!("//{pkg}:{BUILD_FILE_NAME}")
        };
        let targets = eval_with(&display, &src, root.to_path_buf(), pkg)?;
        all.extend(targets);
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn workspace_walk_finds_packages_and_attaches_labels() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        std::fs::create_dir_all(root.join("crates/a")).unwrap();
        std::fs::create_dir_all(root.join("crates/b")).unwrap();
        std::fs::create_dir_all(root.join(".fabrik/should-be-skipped")).unwrap();

        std::fs::write(
            root.join("fabrik.star"),
            "rust_binary(name = \"top\", srcs = [\"main.rs\"])\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/a/fabrik.star"),
            "rust_library(name = \"a\", srcs = [\"lib.rs\"])\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/b/fabrik.star"),
            "rust_binary(name = \"b\", srcs = [\"main.rs\"])\n",
        )
        .unwrap();
        // A fabrik.star inside a hidden dir must NOT be picked up.
        std::fs::write(
            root.join(".fabrik/should-be-skipped/fabrik.star"),
            "rust_binary(name = \"hidden\", srcs = [\"x.rs\"])\n",
        )
        .unwrap();

        let labels: Vec<_> = load_workspace(root)
            .unwrap()
            .into_iter()
            .map(|t| t.label())
            .collect();
        assert_eq!(
            labels,
            vec!["//:top", "//crates/a:a", "//crates/b:b"],
            "expected three labels in package-sorted order"
        );
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
            root.join("pkg/fabrik.star"),
            "rust_binary(name = \"pkg\", srcs = glob([\"src/*.rs\"]))\n",
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
            root.join("pkg/fabrik.star"),
            "rust_binary(name = \"empty\", srcs = glob([\"src/*.rs\"]))\n",
        )
        .unwrap();
        let targets = load_workspace(root).unwrap();
        assert_eq!(targets[0].srcs, Vec::<String>::new());
    }
}
