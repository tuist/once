//! Target kind example bundles. Target kind schemas carry lightweight
//! example descriptors during discovery; the file tree is loaded only
//! when a caller asks to materialize a specific starter.

use std::path::{Component, Path};

use include_dir::{include_dir, Dir};
use walkdir::WalkDir;

use crate::error::{Error, Result};
use crate::graph::{
    TargetKindExample, TargetKindExampleBundle, TargetKindExampleFile, TargetKindExampleRoot,
    TargetKindExampleSource, TargetKindSchema,
};

static PRELUDE_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/prelude");

pub(crate) fn example_source(
    root: TargetKindExampleRoot,
    base: &str,
    path: &str,
) -> std::result::Result<TargetKindExampleSource, String> {
    validate_relative_path(path)?;
    let path = join_relative(base, path);
    let source = TargetKindExampleSource { root, path };
    validate_example_source(&source)?;
    Ok(source)
}

pub fn load_target_kind_example(
    schema: &TargetKindSchema,
    slug: &str,
) -> Result<TargetKindExampleBundle> {
    let example = schema
        .examples
        .iter()
        .find(|example| example.slug == slug)
        .ok_or_else(|| Error::Eval {
            path: schema.kind.clone(),
            message: format!("target kind `{}` has no example `{slug}`", schema.kind),
        })?;
    load_example_bundle(example)
}

pub fn load_example_bundle(example: &TargetKindExample) -> Result<TargetKindExampleBundle> {
    let files = match &example.source.root {
        TargetKindExampleRoot::BuiltInPrelude => load_included_files(&example.source.path)?,
        TargetKindExampleRoot::Workspace { root } => {
            load_workspace_files(root, &example.source.path)?
        }
    };
    Ok(TargetKindExampleBundle {
        name: example.name.clone(),
        slug: example.slug.clone(),
        use_when: example.use_when.clone(),
        files,
    })
}

pub(crate) fn validate_example_source(
    source: &TargetKindExampleSource,
) -> std::result::Result<(), String> {
    match &source.root {
        TargetKindExampleRoot::BuiltInPrelude => {
            if PRELUDE_DIR.get_dir(&source.path).is_none() {
                return Err(format!(
                    "references missing built-in example directory `{}`",
                    source.path
                ));
            }
            Ok(())
        }
        TargetKindExampleRoot::Workspace { root } => validate_workspace_source(root, &source.path),
    }
}

fn load_included_files(path: &str) -> Result<Vec<TargetKindExampleFile>> {
    let dir = PRELUDE_DIR.get_dir(path).ok_or_else(|| Error::Eval {
        path: path.to_string(),
        message: format!("example directory `{path}` is not bundled"),
    })?;
    let mut files = Vec::new();
    collect_included_files(dir, Path::new(path), &mut files);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn collect_included_files(dir: &Dir<'_>, root: &Path, out: &mut Vec<TargetKindExampleFile>) {
    for file in dir.files() {
        let path = file.path();
        let relative = path.strip_prefix(root).unwrap_or(path);
        if path_has_runtime_state(relative) {
            continue;
        }
        out.push(TargetKindExampleFile {
            path: display_path(relative),
            contents: file.contents_utf8().unwrap_or_default().to_string(),
        });
    }
    for sub in dir.dirs() {
        collect_included_files(sub, root, out);
    }
}

fn load_workspace_files(root: &Path, path: &str) -> Result<Vec<TargetKindExampleFile>> {
    let source_root = root.join(path);
    let mut files = Vec::new();
    for entry in WalkDir::new(&source_root) {
        let entry = entry.map_err(|source| Error::Walk {
            root: source_root.display().to_string(),
            source,
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(&source_root)
            .unwrap_or(entry.path());
        if path_has_runtime_state(relative) {
            continue;
        }
        let contents = std::fs::read_to_string(entry.path()).map_err(|source| Error::Read {
            path: entry.path().display().to_string(),
            source,
        })?;
        files.push(TargetKindExampleFile {
            path: display_path(relative),
            contents,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn validate_workspace_source(root: &Path, path: &str) -> std::result::Result<(), String> {
    let canonical_root = std::fs::canonicalize(root).map_err(|err| {
        format!(
            "could not resolve workspace root `{}`: {err}",
            root.display()
        )
    })?;
    let example_root = root.join(path);
    let canonical_example = std::fs::canonicalize(&example_root).map_err(|err| {
        format!(
            "references missing example directory `{}`: {err}",
            example_root.display()
        )
    })?;
    if !canonical_example.starts_with(&canonical_root) {
        return Err(format!(
            "example directory `{}` resolves outside the workspace",
            example_root.display()
        ));
    }
    if !canonical_example.is_dir() {
        return Err(format!(
            "example path `{}` must be a directory",
            example_root.display()
        ));
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> std::result::Result<(), String> {
    if path.trim().is_empty() {
        return Err("path must be non-empty".to_string());
    }
    let path = Path::new(path);
    if path.is_absolute() {
        return Err("path must be relative".to_string());
    }
    for component in path.components() {
        match component {
            Component::Normal(name) if name == ".once" => {
                return Err("path must not reference `.once`".to_string());
            }
            Component::Normal(_) => {}
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err("path must stay inside the module package".to_string());
            }
        }
    }
    Ok(())
}

fn join_relative(base: &str, path: &str) -> String {
    if base.is_empty() {
        return path.to_string();
    }
    format!("{base}/{path}")
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn path_has_runtime_state(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::Normal(name) if name == ".once"))
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn workspace_example_omits_once_runtime_state() {
        let tmp = TempDir::new().unwrap();
        let example = tmp.path().join("examples/hello");
        std::fs::create_dir_all(example.join("src")).unwrap();
        std::fs::create_dir_all(example.join(".once/out/App")).unwrap();
        std::fs::write(example.join("once.toml"), "[[target]]\n").unwrap();
        std::fs::write(example.join("src/Main.kt"), "fun main() {}\n").unwrap();
        std::fs::write(example.join(".once/once.sqlite"), "runtime").unwrap();
        std::fs::write(example.join(".once/out/App/App.jar"), "artifact").unwrap();

        let files = load_workspace_files(tmp.path(), "examples/hello").unwrap();
        let paths = files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();

        assert_eq!(paths, vec!["once.toml", "src/Main.kt"]);
    }
}
