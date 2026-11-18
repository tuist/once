//! Redacted copies of native projects, for reproducing graph loading problems
//! without the project's source code.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use walkdir::WalkDir;

use crate::error::{Error, Result};

use super::{native_invocation, native_project_schemas_for_workspace_with_target_kinds};

const MAX_EXPORTED_FILES: usize = 500_000;
/// Copied text larger than this is copied as is, without removing URL
/// credentials, so one file never has to fit in memory twice.
const MAX_SCRUBBED_FILE_BYTES: u64 = 16 * 1024 * 1024;
const ALWAYS_SKIPPED_DIRS: [&str; 2] = [".git", ".once"];
const MATCH_OPTIONS: glob::MatchOptions = glob::MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};

/// The workspace an invocation loads, its native projects, and the workspace
/// markers that bound it, each with whether it is a directory.
struct Discovered {
    workspace: PathBuf,
    projects: Vec<ExportedNativeProject>,
    markers: Vec<(String, bool)>,
}

/// One native project root included in an export.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ExportedNativeProject {
    pub native_project: String,
    pub package: String,
    pub manifests: Vec<String>,
    /// Directory names the export walk skips, because they cannot hold a
    /// resolver input.
    #[serde(skip)]
    pub skipped_dirs: Vec<String>,
}

/// What a redacted native project export wrote.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct NativeProjectExport {
    /// Workspace the export mirrors.
    #[serde(skip)]
    pub workspace: PathBuf,
    pub projects: Vec<ExportedNativeProject>,
    /// Workspace-relative files copied with their contents.
    pub copied: Vec<String>,
    /// Workspace-relative files written empty in place of their contents.
    pub redacted: Vec<String>,
    /// Workspace-relative symbolic links, left out of the export.
    pub links: Vec<String>,
    /// Workspace markers recreated empty at the export root, so an invocation
    /// in the export finds the same workspace boundary.
    pub workspace_markers: Vec<String>,
    /// Why native projects could not be discovered. The export then holds
    /// only the workspace manifests and modules that explain the failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovery_error: Option<String>,
}

/// Mirror every native project under `directory` into `output`.
///
/// Files matching a project's declared `manifests`, every `once.toml`, and
/// configured project-local modules are copied, with credentials removed from
/// URLs in them. Every other file is written empty at the same path, so globs,
/// directory listings, and existence checks see the same tree while no
/// implementation source leaves the workspace. Directories a project declares
/// in `input_exclude` are skipped, as resolver input gathering skips them.
///
/// A failure to discover native projects is recorded rather than returned,
/// because it is often the very failure the export is meant to report.
pub fn export_native_projects(directory: &Path, output: &Path) -> Result<NativeProjectExport> {
    let directory = std::fs::canonicalize(directory).map_err(|source| Error::Read {
        path: directory.display().to_string(),
        source,
    })?;
    let (
        Discovered {
            workspace,
            projects,
            markers,
        },
        discovery_error,
    ) = match exported_projects(&directory) {
        Ok(discovered) => (discovered, None),
        Err(error) => (
            Discovered {
                workspace: directory.clone(),
                projects: Vec::new(),
                markers: Vec::new(),
            },
            Some(error.to_string()),
        ),
    };
    let skipped_output = std::fs::canonicalize(output).ok();
    let mut files = BTreeMap::<String, bool>::new();
    let mut links = BTreeSet::new();
    for project in &projects {
        walk_project(
            &workspace,
            project,
            skipped_output.as_deref(),
            &mut files,
            &mut links,
        )?;
    }
    // Manifests and project-local modules describe the build, not the
    // implementation, and the export cannot load the same graph without them.
    // Either can be what fails to load, so a broken one still leaves the rest.
    for path in workspace_manifests(&workspace) {
        if skipped_output
            .as_deref()
            .is_none_or(|output| !workspace.join(&path).starts_with(output))
        {
            files.insert(path, true);
        }
    }
    if let Ok(modules) = crate::modules::load_module_files(&workspace) {
        for module in modules {
            files.insert(module.display_path, true);
        }
    }
    let mut copied = Vec::new();
    let mut redacted = Vec::new();
    for (path, copy) in files {
        let destination = output.join(&path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|source| write_error(parent, source))?;
        }
        if copy {
            copy_scrubbed(&workspace.join(&path), &destination)?;
            copied.push(path);
        } else {
            std::fs::write(&destination, b"")
                .map_err(|source| write_error(&destination, source))?;
            redacted.push(path);
        }
    }
    for (marker, is_dir) in &markers {
        let destination = output.join(marker);
        let created = if *is_dir {
            std::fs::create_dir_all(&destination)
        } else {
            std::fs::write(&destination, b"")
        };
        created.map_err(|source| write_error(&destination, source))?;
    }
    Ok(NativeProjectExport {
        workspace,
        projects,
        copied,
        redacted,
        links: links.into_iter().collect(),
        workspace_markers: markers.into_iter().map(|(marker, _)| marker).collect(),
        discovery_error,
    })
}

/// Record every file of one project: `true` to copy it, `false` to write it
/// empty. Symbolic links are recorded separately and left out.
fn walk_project(
    workspace: &Path,
    project: &ExportedNativeProject,
    skipped_output: Option<&Path>,
    files: &mut BTreeMap<String, bool>,
    links: &mut BTreeSet<String>,
) -> Result<()> {
    let patterns = project
        .manifests
        .iter()
        .map(|pattern| {
            glob::Pattern::new(pattern).map_err(|source| Error::Eval {
                path: project.native_project.clone(),
                message: format!("invalid native project manifest glob `{pattern}`: {source}"),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let package_root = workspace.join(&project.package);
    let walker = WalkDir::new(&package_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 || !entry.file_type().is_dir() {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            !ALWAYS_SKIPPED_DIRS.contains(&name.as_ref())
                && !project
                    .skipped_dirs
                    .iter()
                    .any(|excluded| excluded == name.as_ref())
                && skipped_output != Some(entry.path())
        });
    for entry in walker {
        let entry = entry.map_err(|source| Error::Eval {
            path: package_root.display().to_string(),
            message: format!("walking native project: {source}"),
        })?;
        if entry.file_type().is_dir() {
            continue;
        }
        let relative = slash_path(
            entry
                .path()
                .strip_prefix(&package_root)
                .unwrap_or(entry.path()),
        );
        let workspace_relative =
            slash_path(entry.path().strip_prefix(workspace).unwrap_or(entry.path()));
        if entry.file_type().is_symlink() {
            links.insert(workspace_relative);
            continue;
        }
        let copy = patterns
            .iter()
            .any(|pattern| pattern.matches_with(&relative, MATCH_OPTIONS));
        *files.entry(workspace_relative).or_insert(false) |= copy;
        if files.len() > MAX_EXPORTED_FILES {
            return Err(Error::Eval {
                path: project.native_project.clone(),
                message: format!(
                    "native project export exceeds {MAX_EXPORTED_FILES} files; declare the directories that cannot hold a resolver input with `input_exclude`"
                ),
            });
        }
    }
    Ok(())
}

/// Every `once.toml` in the workspace, workspace-relative. The root manifest
/// is included even when the workspace scan cannot load, because a broken
/// root manifest is worth reporting.
fn workspace_manifests(workspace: &Path) -> Vec<String> {
    let mut paths = crate::workspace::load_workspace_scan(workspace)
        .and_then(|scan| crate::workspace::manifest_entries(workspace, &scan))
        .map(|entries| {
            entries
                .into_iter()
                .map(|(_, path)| slash_path(path.strip_prefix(workspace).unwrap_or(&path)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if workspace.join(crate::TOML_BUILD_FILE_NAME).is_file() {
        paths.push(crate::TOML_BUILD_FILE_NAME.to_string());
    }
    paths
}

/// Discover what an invocation in `directory` loads.
fn exported_projects(directory: &Path) -> Result<Discovered> {
    let kinds = crate::target_kind_schemas_for_workspace(directory)?;
    let schemas = native_project_schemas_for_workspace_with_target_kinds(directory, &kinds)?;
    if let Some(invocation) = native_invocation(directory)? {
        let mut markers = BTreeMap::new();
        let projects = invocation
            .targets
            .iter()
            .map(|target| {
                let matching = schemas
                    .iter()
                    .filter(|schema| {
                        schema.target_kind == target.kind && schema.target_name == target.name
                    })
                    .collect::<Vec<_>>();
                for marker in matching.iter().flat_map(|schema| &schema.workspace_markers) {
                    let path = invocation.workspace.join(marker);
                    if path.exists() {
                        markers.insert(marker.clone(), path.is_dir());
                    }
                }
                ExportedNativeProject {
                    native_project: matching
                        .iter()
                        .map(|schema| schema.name.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                    package: target.package.clone(),
                    manifests: union(matching.iter().map(|schema| &schema.manifests)),
                    skipped_dirs: target.resolver_input_exclude.clone(),
                }
            })
            .collect();
        return Ok(Discovered {
            workspace: invocation.workspace,
            projects,
            markers: markers.into_iter().collect(),
        });
    }
    let boundary = crate::workspace::load_workspace_scan(directory)?;
    let (matches, _) =
        super::discovery::detect_native_projects_with_schemas(directory, &schemas, &boundary)?;
    let projects = matches
        .into_iter()
        .filter_map(|matched| {
            let schema = schemas
                .iter()
                .find(|schema| schema.name == matched.native_project)?;
            Some(ExportedNativeProject {
                native_project: schema.name.clone(),
                package: matched.package,
                manifests: schema.manifests.clone(),
                skipped_dirs: schema.input_exclude.clone(),
            })
        })
        .collect();
    Ok(Discovered {
        workspace: directory.to_path_buf(),
        projects,
        markers: Vec::new(),
    })
}

/// Copy one file, removing credentials from any URL in it when it is text.
fn copy_scrubbed(source: &Path, destination: &Path) -> Result<()> {
    let small = std::fs::metadata(source)
        .map_err(|error| read_error(source, error))?
        .len()
        <= MAX_SCRUBBED_FILE_BYTES;
    let text = if small {
        std::fs::read(source)
            .map_err(|error| read_error(source, error))
            .map(|bytes| String::from_utf8(bytes).ok())?
    } else {
        None
    };
    match text {
        Some(text) => std::fs::write(destination, without_url_credentials(&text)),
        None => std::fs::copy(source, destination).map(|_| ()),
    }
    .map_err(|error| write_error(destination, error))
}

/// `text` with the user information of every `scheme://user:secret@host` URL
/// replaced, so a registry or mirror URL keeps its host but not its secret.
#[must_use]
pub fn without_url_credentials(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find("://") {
        let (before, after) = rest.split_at(index + 3);
        out.push_str(before);
        let authority_end = after
            .find(|c: char| {
                matches!(c, '/' | '?' | '#') || c.is_whitespace() || "\"'`<>".contains(c)
            })
            .unwrap_or(after.len());
        match after[..authority_end].rfind('@') {
            Some(at) => {
                out.push_str("<redacted>");
                rest = &after[at..];
            }
            None => rest = after,
        }
    }
    out.push_str(rest);
    out
}

fn union<'a>(lists: impl Iterator<Item = &'a Vec<String>>) -> Vec<String> {
    lists
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn slash_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn read_error(path: &Path, source: std::io::Error) -> Error {
    Error::Read {
        path: path.display().to_string(),
        source,
    }
}

fn write_error(path: &Path, source: std::io::Error) -> Error {
    read_error(path, source)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, contents: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn export(root: &Path) -> (NativeProjectExport, tempfile::TempDir) {
        let output = tempfile::tempdir().unwrap();
        (export_native_projects(root, output.path()).unwrap(), output)
    }

    #[test]
    fn manifests_keep_their_contents_and_sources_become_empty_placeholders() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        write(
            root,
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\", \"vendor/*\"]\n",
        );
        write(root, "crates/app/Cargo.toml", "[package]\nname = \"app\"\n");
        write(root, "crates/app/src/main.rs", "fn secret() {}\n");
        write(
            root,
            "vendor/shim/Cargo.toml",
            "[package]\nname = \"shim\"\n",
        );
        write(root, "target/debug/app", "binary");
        write(root, ".git/HEAD", "ref: refs/heads/main\n");

        let (export, output) = export(root);

        assert!(export.copied.contains(&"crates/app/Cargo.toml".to_string()));
        // `vendor` is excluded from discovery but holds a resolver input.
        assert!(export
            .copied
            .contains(&"vendor/shim/Cargo.toml".to_string()));
        assert_eq!(export.redacted, ["crates/app/src/main.rs"]);
        assert_eq!(
            std::fs::read(output.path().join("crates/app/src/main.rs")).unwrap(),
            b""
        );
        assert!(!output.path().join("target").exists());
        assert!(!output.path().join(".git").exists());
    }

    #[test]
    fn a_root_manifest_is_exported_with_a_nested_native_project() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        write(root, "once.toml", "[modules]\npaths = [\"defs.star\"]\n");
        write(root, "defs.star", "x = 1\n");
        write(root, "tools/Cargo.toml", "[package]\nname = \"tools\"\n");

        let (export, output) = export(root);

        assert!(export.copied.contains(&"once.toml".to_string()));
        assert!(export.copied.contains(&"defs.star".to_string()));
        assert!(output.path().join("once.toml").is_file());
    }

    #[test]
    fn a_broken_module_is_recorded_and_the_manifest_still_exported() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        write(root, "once.toml", "[modules]\npaths = [\"missing.star\"]\n");

        let (export, _output) = export(root);

        assert!(export.discovery_error.unwrap().contains("missing.star"));
        assert_eq!(export.copied, ["once.toml"]);
    }

    #[test]
    fn url_credentials_are_removed_and_hosts_kept() {
        assert_eq!(
            without_url_credentials(
                "index = \"https://user:pa@ss@registry.example/x\"\nsite = https://example.com/a@b"
            ),
            "index = \"https://<redacted>@registry.example/x\"\nsite = https://example.com/a@b"
        );
    }
}
