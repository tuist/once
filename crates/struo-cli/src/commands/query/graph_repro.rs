//! `struo query graph-repro`: a redacted native project export, packaged to be
//! attached to an issue on the Struo repository.

use std::fmt::Write as _;
use std::io::{BufWriter, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::write_body;
use crate::cli::Output;

const ISSUES_URL: &str = "https://github.com/tuist/struo/issues/new";
const ISSUE_REPOSITORY: &str = "tuist/struo";
const ARCHIVE_NAME: &str = "graph-repro.tar.gz";
/// Written first into an export directory, so a later export recognizes the
/// directory as its own even when the earlier one stopped partway.
const MARKER_NAME: &str = ".struo-graph-repro";
const MARKER_CONTENTS: &str = "struo.graph_repro.v1\n";
/// Everything an export writes. Replacing an export removes these and nothing
/// else.
const OWNED_ENTRIES: [&str; 6] = [
    MARKER_NAME,
    "workspace",
    "repro.json",
    "README.md",
    "ISSUE.md",
    ARCHIVE_NAME,
];
/// GitHub rejects issue links much longer than this, so a body that would
/// encode past it is left for the reporter to paste from `ISSUE.md`.
const MAX_URL_BYTES: usize = 8_000;
const MAX_ISSUE_FAILURE_CHARS: usize = 4_000;

pub async fn graph_repro(workspace: &Path, output: Output, directory: Option<&Path>) -> Result<()> {
    let export = graph_repro_value(workspace, directory)?;
    write_body(output, || render_human(&export), &export).await
}

/// Write a redacted native project export with the graph it loads to, or the
/// error loading it stopped at, and the issue to report it with.
pub(crate) fn graph_repro_value(
    workspace: &Path,
    directory: Option<&Path>,
) -> Result<serde_json::Value> {
    let directory = prepare_directory(workspace, directory)?;
    let mirror = directory.join("workspace");
    std::fs::create_dir_all(&mirror).with_context(|| format!("creating `{}`", mirror.display()))?;
    let export = struo_frontend::export_native_projects(workspace, &mirror)
        .context("exporting native projects")?;
    let loaded = crate::native_invocation::query_graph(workspace);
    let redactions = path_redactions(&export.workspace);
    let redact = |text: String| redact_text(&text, &redactions);
    let (graph, load_error) = match loaded {
        Ok((_, graph)) => (Some(graph), None),
        Err(error) => (None, Some(redact(load_error_message(&error)))),
    };
    let diagnostics = graph
        .iter()
        .flatten()
        .flat_map(|target| target.diagnostics.iter())
        .collect::<Vec<_>>();
    let canonical_workspace =
        std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let invocation = canonical_workspace
        .strip_prefix(&export.workspace)
        .unwrap_or(Path::new(""))
        .to_path_buf();
    let replay_directory = if invocation.as_os_str().is_empty() {
        mirror.clone()
    } else {
        mirror.join(&invocation)
    };
    let mut graph_value = serde_json::to_value(&graph)?;
    redact_environment(&mut graph_value);
    let mut record = serde_json::json!({
        "schema": "struo.graph_repro.v1",
        "once_version": env!("CARGO_PKG_VERSION"),
        "host": {"os": std::env::consts::OS, "arch": std::env::consts::ARCH},
        "invocation_directory": invocation.to_string_lossy(),
        "projects": export.projects,
        "copied": export.copied,
        "redacted": export.redacted,
        "links": export.links,
        "workspace_markers": export.workspace_markers,
        "discovery_error": export.discovery_error,
        "graph": graph_value,
        "diagnostics": diagnostics,
        "load_error": load_error,
    });
    redact_value(&mut record, &redactions);
    write_file(
        &directory.join("repro.json"),
        &format!("{}\n", serde_json::to_string_pretty(&record)?),
    )?;
    write_file(&directory.join("README.md"), README)?;

    let failure = load_error
        .clone()
        .or_else(|| export.discovery_error.clone().map(redact))
        .or_else(|| {
            (!diagnostics.is_empty()).then(|| {
                diagnostics
                    .iter()
                    .map(|diagnostic| {
                        redact(format!("{}: {}", diagnostic.code, diagnostic.message))
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
        });
    let issue = Issue::new(&export.projects, failure.as_deref());
    let issue_path = directory.join("ISSUE.md");
    write_file(&issue_path, &format!("# {}\n\n{}", issue.title, issue.body))?;
    let archive = write_archive(&directory)?;
    let issue_url = issue.url()?;

    Ok(serde_json::json!({
        "schema": "struo.graph_repro_export.v1",
        "directory": directory.to_string_lossy(),
        "archive": archive.to_string_lossy(),
        "replay_directory": replay_directory.to_string_lossy(),
        "projects": export.projects,
        "copied": export.copied,
        "redacted_count": export.redacted.len(),
        "links": export.links,
        "target_count": graph.as_ref().map(Vec::len),
        "diagnostic_count": diagnostics.len(),
        "load_error": load_error,
        "issue": {
            "repository": ISSUE_REPOSITORY,
            "title": issue.title,
            "body_path": issue_path.to_string_lossy(),
            "url": issue_url,
        },
        "next_steps": next_steps(&issue.title, &issue_path, &archive, &replay_directory),
    }))
}

/// The structured failure in a loading error when it has one, which reads
/// without the context frames that repeat it.
fn load_error_message(error: &anyhow::Error) -> String {
    struo_frontend::analysis::AnalysisFailure::find(error).map_or_else(
        || format!("{error:#}"),
        |failure| {
            format!(
                "{}: {}",
                failure.diagnostic.code, failure.diagnostic.message
            )
        },
    )
}

/// The export's directory, emptied of an earlier export's files but never of
/// anything else, with the ownership marker written before anything else.
fn prepare_directory(workspace: &Path, directory: Option<&Path>) -> Result<PathBuf> {
    let directory = match directory {
        Some(directory) => std::path::absolute(directory)
            .with_context(|| format!("resolving `{}`", directory.display()))?,
        None => workspace.join(".struo/graph-repro"),
    };
    let occupied = std::fs::read_dir(&directory).is_ok_and(|mut entries| entries.next().is_some());
    if occupied {
        let ours = std::fs::read_to_string(directory.join(MARKER_NAME))
            .is_ok_and(|contents| contents == MARKER_CONTENTS);
        if !ours {
            anyhow::bail!(
                "`{}` already holds files that are not a graph reproduction; pass an empty or new directory with --output",
                directory.display()
            );
        }
        for name in OWNED_ENTRIES {
            let path = directory.join(name);
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            }
            .with_context(|| format!("replacing `{}`", path.display()))?;
        }
    }
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("creating `{}`", directory.display()))?;
    write_file(&directory.join(MARKER_NAME), MARKER_CONTENTS)?;
    Ok(directory)
}

/// Absolute paths to hide, longest first so a workspace under the home
/// directory reads `$WORKSPACE`. A home directory at the filesystem root is
/// left alone, because hiding it would rewrite every path.
fn path_redactions(workspace: &Path) -> Vec<(String, &'static str)> {
    let mut redactions = vec![(workspace.to_string_lossy().into_owned(), "$WORKSPACE")];
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    if let Some(home) = home.filter(|home| Path::new(home).parent().is_some()) {
        redactions.push((home.to_string_lossy().into_owned(), "$HOME"));
    }
    redactions.sort_by_key(|(path, _)| std::cmp::Reverse(path.len()));
    redactions
}

/// `text` with each path replaced wherever it ends at a path boundary, so
/// `/work/app` hides `/work/app/src` but not `/work/application`.
fn redact_text(text: &str, redactions: &[(String, &str)]) -> String {
    redactions
        .iter()
        .fold(text.to_string(), |text, (path, name)| {
            let mut out = String::with_capacity(text.len());
            let mut rest = text.as_str();
            while let Some(index) = rest.find(path.as_str()) {
                let after = &rest[index + path.len()..];
                let boundary = after
                    .chars()
                    .next()
                    .is_none_or(|c| !(c.is_alphanumeric() || "-_.".contains(c)));
                out.push_str(&rest[..index]);
                out.push_str(if boundary { name } else { path });
                rest = after;
            }
            out.push_str(rest);
            out
        })
}

/// Redact every string in a JSON value. Working on values rather than on the
/// serialized text matters on Windows, where JSON escapes path separators.
fn redact_value(value: &mut serde_json::Value, redactions: &[(String, &str)]) {
    match value {
        serde_json::Value::String(text) => *text = redact_text(text, redactions),
        serde_json::Value::Array(items) => {
            for item in items {
                redact_value(item, redactions);
            }
        }
        serde_json::Value::Object(fields) => {
            for field in fields.values_mut() {
                redact_value(field, redactions);
            }
        }
        _ => {}
    }
}

/// Hide the values under every environment-shaped key, because they often
/// hold tokens. The names stay, since which variables are set can matter.
fn redact_environment(value: &mut serde_json::Value) {
    fn hide(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(text) => *text = "<redacted>".to_string(),
            serde_json::Value::Array(items) => items.iter_mut().for_each(hide),
            serde_json::Value::Object(fields) => fields.values_mut().for_each(hide),
            _ => {}
        }
    }
    match value {
        serde_json::Value::Array(items) => items.iter_mut().for_each(redact_environment),
        serde_json::Value::Object(fields) => {
            for (key, field) in fields.iter_mut() {
                let key = key.to_ascii_lowercase();
                if key == "env" || key.ends_with("_env") || key.ends_with("environment") {
                    hide(field);
                } else {
                    redact_environment(field);
                }
            }
        }
        _ => {}
    }
}

fn next_steps(title: &str, issue: &Path, archive: &Path, replay: &Path) -> Vec<String> {
    vec![
        "Nothing has been shared. Show the user the copied files and repro.json, which holds the loaded graph and its diagnostics, and ask them to review both for secrets, such as private registry hosts or tokens. Credentials in URLs and environment values are already hidden.".to_string(),
        format!("Fill in what the native build system builds that Struo gets wrong under `What went wrong` in {}.", issue.display()),
        format!(
            "With the user's approval, open an issue on {ISSUE_REPOSITORY}: run `gh issue create --repo {ISSUE_REPOSITORY} --title {title:?} --body-file {}`, or open the issue URL. Then ask the user to attach {} to the issue in the browser, because GitHub accepts attachments only there.",
            issue.display(),
            archive.display(),
        ),
        format!("Reproduce the graph by running Struo queries in {}.", replay.display()),
    ]
}

/// An issue that reports the export without any of the project's sources.
struct Issue {
    title: String,
    body: String,
}

impl Issue {
    fn new(projects: &[struo_frontend::ExportedNativeProject], failure: Option<&str>) -> Self {
        let kinds = projects
            .iter()
            .map(|project| project.native_project.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(", ");
        let kinds = if kinds.is_empty() {
            "no native project".to_string()
        } else {
            kinds
        };
        let title = if failure.is_some() {
            format!("Native graph fails to load: {kinds}")
        } else {
            format!("Native graph disagrees with the native build system: {kinds}")
        };
        let mut body = String::from(
            "## What went wrong\n\n<!-- What the native build system builds, and what Struo's graph gets wrong. -->\n\n## Environment\n\n",
        );
        let _ = writeln!(body, "- Struo {}", env!("CARGO_PKG_VERSION"));
        let _ = writeln!(
            body,
            "- Host {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        for project in projects {
            let package = if project.package.is_empty() {
                "."
            } else {
                project.package.as_str()
            };
            let _ = writeln!(
                body,
                "- Native project `{}` at `{package}`",
                project.native_project
            );
        }
        if let Some(failure) = failure {
            let failure = truncate(failure, MAX_ISSUE_FAILURE_CHARS);
            let _ = write!(body, "\n## Failure\n\n```text\n{failure}\n```\n");
        }
        let _ = write!(
            body,
            "\n## Reproduction\n\nAttached `{ARCHIVE_NAME}`, written by `struo query graph-repro`. It holds the native manifests with every other file emptied, and `repro.json` with the graph Struo loaded or the error that stopped it.\n"
        );
        Self { title, body }
    }

    fn url(&self) -> Result<String> {
        let link = |body: &str| -> Result<String> {
            let mut url = reqwest::Url::parse(ISSUES_URL)?;
            url.query_pairs_mut()
                .append_pair("title", &self.title)
                .append_pair("body", body);
            Ok(url.into())
        };
        let url = link(&self.body)?;
        if url.len() <= MAX_URL_BYTES {
            return Ok(url);
        }
        link("Paste the contents of `ISSUE.md` from the export here.")
    }
}

fn truncate(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((index, _)) => format!("{}\n[truncated]", &text[..index]),
        None => text.to_string(),
    }
}

/// Pack the export into one file to attach, streaming it so the archive never
/// has to fit in memory.
fn write_archive(directory: &Path) -> Result<PathBuf> {
    let path = directory.join(ARCHIVE_NAME);
    let file =
        std::fs::File::create(&path).with_context(|| format!("creating `{}`", path.display()))?;
    let encoder =
        flate2::write::GzEncoder::new(BufWriter::new(file), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    builder.follow_symlinks(false);
    for name in ["README.md", "ISSUE.md", "repro.json"] {
        // The marker stays out: it only claims the export directory.
        builder.append_path_with_name(directory.join(name), name)?;
    }
    builder.append_dir_all("workspace", directory.join("workspace"))?;
    builder
        .into_inner()?
        .finish()?
        .flush()
        .with_context(|| format!("writing `{}`", path.display()))?;
    Ok(path)
}

fn write_file(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).with_context(|| format!("writing `{}`", path.display()))
}

const README: &str = "# Struo graph reproduction

This directory reproduces how Struo loads the graph of a native project, without
the project's source code.

- `workspace/` mirrors every native project Struo discovered. Native manifests
  and lockfiles keep their contents. Every other file is empty, at the same
  path, so globs and resolvers see the same tree.
- `repro.json` records the Struo version, the host platform, the files copied
  and redacted, and the graph Struo loaded, or the error that stopped it.
  Absolute workspace and home directory paths read `$WORKSPACE` and `$HOME`.
- `ISSUE.md` is the issue to open on https://github.com/tuist/struo, and
  `graph-repro.tar.gz` packs everything here to attach to it.

Review the copied files before sharing this directory: manifests can name
private registries, hosts, or credentials.

To reproduce, run Struo queries, such as `struo query targets --format json`, in
the directory `repro.json` names as `invocation_directory` under `workspace/`.
";

fn render_human(export: &serde_json::Value) -> String {
    let mut text = format!(
        "exported to {}\ncopied {} manifest files, redacted {} files\narchive: {}\n",
        export["directory"].as_str().unwrap_or_default(),
        export["copied"].as_array().map_or(0, Vec::len),
        export["redacted_count"],
        export["archive"].as_str().unwrap_or_default(),
    );
    if let Some(error) = export["load_error"].as_str() {
        let _ = writeln!(text, "graph failed to load: {error}");
    } else {
        let _ = writeln!(
            text,
            "graph: {} targets, {} diagnostics",
            export["target_count"], export["diagnostic_count"]
        );
    }
    for path in export["copied"].as_array().into_iter().flatten() {
        let _ = writeln!(text, "  copied {}", path.as_str().unwrap_or_default());
    }
    let _ = writeln!(
        text,
        "issue: {}",
        export["issue"]["url"].as_str().unwrap_or_default()
    );
    for step in export["next_steps"].as_array().into_iter().flatten() {
        let _ = writeln!(text, "next: {}", step.as_str().unwrap_or_default());
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_that_is_not_an_export_is_never_emptied() {
        let workspace = tempfile::tempdir().unwrap();
        let output = workspace.path().join("out");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(output.join("repro.json"), "{}").unwrap();
        std::fs::write(output.join("notes.txt"), "keep").unwrap();

        assert!(prepare_directory(workspace.path(), Some(&output)).is_err());
        assert!(output.join("notes.txt").is_file());
        assert!(output.join("repro.json").is_file());
    }

    #[test]
    fn an_earlier_export_is_replaced_without_touching_other_files() {
        let workspace = tempfile::tempdir().unwrap();
        let output = prepare_directory(workspace.path(), None).unwrap();
        std::fs::create_dir_all(output.join("workspace/src")).unwrap();
        std::fs::write(output.join("repro.json"), "{}").unwrap();
        std::fs::write(output.join("notes.txt"), "keep").unwrap();

        prepare_directory(workspace.path(), None).unwrap();

        assert!(!output.join("workspace").exists());
        assert!(!output.join("repro.json").exists());
        assert!(output.join("notes.txt").is_file());
        assert!(output.join(MARKER_NAME).is_file());
    }

    #[test]
    fn paths_are_redacted_only_at_boundaries_and_never_for_a_root_home() {
        let redactions = vec![("/work/app".to_string(), "$WORKSPACE")];
        assert_eq!(
            redact_text("/work/app/src /work/application /work/app", &redactions),
            "$WORKSPACE/src /work/application $WORKSPACE"
        );
        let mut value = serde_json::json!({"paths": ["C:\\work\\app\\x"]});
        redact_value(&mut value, &[("C:\\work\\app".to_string(), "$WORKSPACE")]);
        assert_eq!(value, serde_json::json!({"paths": ["$WORKSPACE\\x"]}));
    }

    #[test]
    fn environment_values_are_hidden_and_their_names_kept() {
        let mut graph = serde_json::json!([{"attrs": {
            "cargo_config_env": {"API_TOKEN": "sk-live"},
            "env": ["A=1"],
            "name": "app",
        }}]);
        redact_environment(&mut graph);
        assert_eq!(
            graph,
            serde_json::json!([{"attrs": {
                "cargo_config_env": {"API_TOKEN": "<redacted>"},
                "env": ["<redacted>"],
                "name": "app",
            }}])
        );
    }

    #[test]
    fn a_failure_titles_the_issue_and_fits_in_its_link() {
        let projects = [struo_frontend::ExportedNativeProject {
            native_project: "native".to_string(),
            package: "apps/tool".to_string(),
            manifests: Vec::new(),
            skipped_dirs: Vec::new(),
        }];
        let issue = Issue::new(&projects, Some("resolver failed: a & b"));

        assert_eq!(issue.title, "Native graph fails to load: native");
        assert!(issue
            .body
            .contains("- Native project `native` at `apps/tool`"));
        assert!(issue.body.contains("```text\nresolver failed: a & b\n```"));
        let url = reqwest::Url::parse(&issue.url().unwrap()).unwrap();
        let query = url
            .query_pairs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(query["title"], issue.title);
        assert_eq!(query["body"], issue.body);
    }

    #[test]
    fn a_long_failure_is_truncated_and_left_out_of_the_link() {
        let failure = "é".repeat(MAX_ISSUE_FAILURE_CHARS + 1);
        let issue = Issue::new(&[], Some(&failure));

        assert_eq!(issue.title, "Native graph fails to load: no native project");
        assert!(issue.body.contains(&format!(
            "{}\n[truncated]",
            "é".repeat(MAX_ISSUE_FAILURE_CHARS)
        )));
        let url = reqwest::Url::parse(&issue.url().unwrap()).unwrap();
        let body = url
            .query_pairs()
            .find(|(key, _)| key == "body")
            .unwrap()
            .1
            .into_owned();
        assert_eq!(
            body,
            "Paste the contents of `ISSUE.md` from the export here."
        );
        assert!(issue.url().unwrap().len() <= MAX_URL_BYTES);
    }
}
