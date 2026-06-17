//! Rule example bundles. Rule schemas carry lightweight example
//! descriptors during discovery; the file tree is loaded only when a
//! caller asks to materialize a specific starter.

use std::path::{Component, Path};

use include_dir::{include_dir, Dir};
use walkdir::WalkDir;

use crate::error::{Error, Result};
use crate::graph::{
    RuleExample, RuleExampleBundle, RuleExampleFile, RuleExampleRoot, RuleExampleSource, RuleSchema,
};

static PRELUDE_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/prelude");

pub(crate) fn example_source(
    root: RuleExampleRoot,
    base: &str,
    path: &str,
) -> std::result::Result<RuleExampleSource, String> {
    validate_relative_path(path)?;
    let path = join_relative(base, path);
    let source = RuleExampleSource { root, path };
    validate_example_source(&source)?;
    Ok(source)
}

pub fn load_rule_example(schema: &RuleSchema, slug: &str) -> Result<RuleExampleBundle> {
    let example = schema
        .examples
        .iter()
        .find(|example| example.slug == slug)
        .ok_or_else(|| Error::Eval {
            path: schema.kind.clone(),
            message: format!("rule `{}` has no example `{slug}`", schema.kind),
        })?;
    load_example_bundle(example)
}

pub fn load_example_bundle(example: &RuleExample) -> Result<RuleExampleBundle> {
    let files = match &example.source.root {
        RuleExampleRoot::BuiltInPrelude => load_included_files(&example.source.path)?,
        RuleExampleRoot::Workspace { root } => load_workspace_files(root, &example.source.path)?,
    };
    Ok(RuleExampleBundle {
        name: example.name.clone(),
        slug: example.slug.clone(),
        use_when: example.use_when.clone(),
        files,
    })
}

pub(crate) fn validate_example_source(
    source: &RuleExampleSource,
) -> std::result::Result<(), String> {
    match &source.root {
        RuleExampleRoot::BuiltInPrelude => {
            if PRELUDE_DIR.get_dir(&source.path).is_none() {
                return Err(format!(
                    "references missing built-in example directory `{}`",
                    source.path
                ));
            }
            Ok(())
        }
        RuleExampleRoot::Workspace { root } => validate_workspace_source(root, &source.path),
    }
}

fn load_included_files(path: &str) -> Result<Vec<RuleExampleFile>> {
    let dir = PRELUDE_DIR.get_dir(path).ok_or_else(|| Error::Eval {
        path: path.to_string(),
        message: format!("example directory `{path}` is not bundled"),
    })?;
    let mut files = Vec::new();
    collect_included_files(dir, Path::new(path), &mut files);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn collect_included_files(dir: &Dir<'_>, root: &Path, out: &mut Vec<RuleExampleFile>) {
    for file in dir.files() {
        let path = file.path();
        let relative = path.strip_prefix(root).unwrap_or(path);
        out.push(RuleExampleFile {
            path: display_path(relative),
            contents: file.contents_utf8().unwrap_or_default().to_string(),
        });
    }
    for sub in dir.dirs() {
        collect_included_files(sub, root, out);
    }
}

fn load_workspace_files(root: &Path, path: &str) -> Result<Vec<RuleExampleFile>> {
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
        let contents = std::fs::read_to_string(entry.path()).map_err(|source| Error::Read {
            path: entry.path().display().to_string(),
            source,
        })?;
        files.push(RuleExampleFile {
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
                return Err("path must stay inside the rule package".to_string());
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
