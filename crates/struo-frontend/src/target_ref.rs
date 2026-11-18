//! Target id normalization.
//!
//! Build files declare dependency references in the local context of
//! the directory they live in. CLI arguments are project-root relative
//! unless they start with `./` or `../`, in which case they resolve
//! from the caller's current directory.

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TargetIdError {
    #[error("target reference is empty")]
    Empty,
    #[error("target name `{0}` must be a single path segment")]
    InvalidName(String),
    #[error("target reference `{raw}` uses Bazel label syntax; use `{suggestion}`")]
    BazelSyntax { raw: String, suggestion: String },
    #[error("target reference `{raw}` must not contain `:`; use `{suggestion}`")]
    Colon { raw: String, suggestion: String },
    #[error("target reference `{raw}` must be relative to the project root; use `{suggestion}`")]
    Absolute { raw: String, suggestion: String },
    #[error("target reference `{0}` must not escape the project root")]
    EscapesRoot(String),
    #[error("target reference `{raw}` contains an empty path segment; use `{suggestion}`")]
    EmptySegment { raw: String, suggestion: String },
    #[error("current directory `{cwd}` is outside project root `{root}`")]
    CurrentDirOutsideProject { cwd: String, root: String },
}

/// Derive a corrected target reference from a malformed one. Follows the
/// AGENTS.md "no invented path grammar" rule: strip Bazel-style prefixes
/// and separators, collapse empty segments, drop redundant `.` segments,
/// and drop `..` segments (which would only push the suggestion back
/// through the `EscapesRoot` failure path). Falls back to a placeholder
/// when nothing survives so the message never shows empty backticks.
fn cleanup_suggestion(raw: &str) -> String {
    let cleaned = raw
        .trim_matches('/')
        .split('/')
        .flat_map(|segment| segment.split([':', '\\']))
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .collect::<Vec<_>>()
        .join("/");
    if cleaned.is_empty() {
        "target-name".to_string()
    } else {
        cleaned
    }
}

pub fn target_id(package: &str, name: &str) -> String {
    if package.is_empty() {
        name.to_string()
    } else {
        format!("{package}/{name}")
    }
}

pub fn validate_target_name(name: &str) -> Result<(), TargetIdError> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', ':']) {
        return Err(TargetIdError::InvalidName(name.to_string()));
    }
    Ok(())
}

pub fn normalize_cli_target(workspace_root: &Path, raw: &str) -> Result<String, TargetIdError> {
    let current_dir = std::env::current_dir().map_err(|_| TargetIdError::Empty)?;
    normalize_cli_target_from(workspace_root, &current_dir, raw)
}

pub fn normalize_cli_target_from(
    workspace_root: &Path,
    current_dir: &Path,
    raw: &str,
) -> Result<String, TargetIdError> {
    validate_raw(raw)?;
    if raw.starts_with("./") || raw.starts_with("../") {
        let package = current_package(workspace_root, current_dir)?;
        normalize_from(&package, raw)
    } else {
        normalize_from(&[], raw)
    }
}

pub fn normalize_manifest_target(package: &str, raw: &str) -> Result<String, TargetIdError> {
    validate_raw(raw)?;
    if raw.starts_with("./") || raw.starts_with("../") {
        let base = package
            .split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        normalize_from(&base, raw)
    } else {
        normalize_from(&[], raw)
    }
}

fn validate_raw(raw: &str) -> Result<(), TargetIdError> {
    if raw.is_empty() {
        return Err(TargetIdError::Empty);
    }
    if let Some(suggestion) = bazel_suggestion(raw) {
        return Err(TargetIdError::BazelSyntax {
            raw: raw.to_string(),
            suggestion,
        });
    }
    if raw.contains(':') {
        return Err(TargetIdError::Colon {
            raw: raw.to_string(),
            suggestion: cleanup_suggestion(raw),
        });
    }
    if raw.starts_with('/') {
        return Err(TargetIdError::Absolute {
            raw: raw.to_string(),
            suggestion: cleanup_suggestion(raw),
        });
    }
    Ok(())
}

fn bazel_suggestion(raw: &str) -> Option<String> {
    if let Some(name) = raw.strip_prefix(':') {
        return Some(name.to_string());
    }
    let rest = raw.strip_prefix("//")?;
    let (package, name) = rest.split_once(':')?;
    if package.is_empty() {
        Some(name.to_string())
    } else {
        Some(format!("{package}/{name}"))
    }
}

fn normalize_from(base: &[String], raw: &str) -> Result<String, TargetIdError> {
    let mut out = base.to_vec();
    for segment in raw.split('/') {
        match segment {
            "" => {
                return Err(TargetIdError::EmptySegment {
                    raw: raw.to_string(),
                    suggestion: cleanup_suggestion(raw),
                });
            }
            "." => {}
            ".." => {
                out.pop()
                    .ok_or_else(|| TargetIdError::EscapesRoot(raw.to_string()))?;
            }
            segment => {
                validate_segment(raw, segment)?;
                out.push(segment.to_string());
            }
        }
    }
    if out.is_empty() {
        return Err(TargetIdError::Empty);
    }
    Ok(out.join("/"))
}

fn validate_segment(raw: &str, segment: &str) -> Result<(), TargetIdError> {
    if segment.contains(['\\', ':']) {
        return Err(TargetIdError::Colon {
            raw: raw.to_string(),
            suggestion: cleanup_suggestion(raw),
        });
    }
    Ok(())
}

fn current_package(
    workspace_root: &Path,
    current_dir: &Path,
) -> Result<Vec<String>, TargetIdError> {
    let relative = current_dir.strip_prefix(workspace_root).map_err(|_| {
        TargetIdError::CurrentDirOutsideProject {
            cwd: display(current_dir),
            root: display(workspace_root),
        }
    })?;
    Ok(path_components(relative))
}

fn path_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| {
            let value = component.as_os_str().to_string_lossy();
            if value.is_empty() || value == "." {
                None
            } else {
                Some(value.replace(std::path::MAIN_SEPARATOR, "/"))
            }
        })
        .flat_map(|component| {
            component
                .split('/')
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub fn absolutize(path: PathBuf) -> std::io::Result<PathBuf> {
    if path.is_absolute() {
        std::fs::canonicalize(path)
    } else {
        std::fs::canonicalize(std::env::current_dir()?.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn target_id_joins_package_and_name() {
        assert_eq!(target_id("", "tool"), "tool");
        assert_eq!(
            target_id("examples/macos-cli", "hello"),
            "examples/macos-cli/hello"
        );
    }

    #[test]
    fn cli_deps_are_root_relative_by_default() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let current = root.join("examples/macos-cli");
        std::fs::create_dir_all(&current).unwrap();
        assert_eq!(
            normalize_cli_target_from(root, &current, "examples/macos-cli/hello").unwrap(),
            "examples/macos-cli/hello"
        );
        assert_eq!(
            normalize_cli_target_from(root, &current, "./hello").unwrap(),
            "examples/macos-cli/hello"
        );
        assert_eq!(
            normalize_cli_target_from(root, &current, "../shared/Logging").unwrap(),
            "examples/shared/Logging"
        );
    }

    #[test]
    fn cli_current_dir_must_be_inside_project_for_dot_refs() {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        assert!(matches!(
            normalize_cli_target_from(root.path(), outside.path(), "./hello"),
            Err(TargetIdError::CurrentDirOutsideProject { .. })
        ));
    }

    #[test]
    fn manifest_deps_allow_root_and_package_relative_refs() {
        assert_eq!(
            normalize_manifest_target("apps/ios", "packages/auth/Auth").unwrap(),
            "packages/auth/Auth"
        );
        assert_eq!(
            normalize_manifest_target("apps/ios", "./AppKit").unwrap(),
            "apps/ios/AppKit"
        );
        assert_eq!(
            normalize_manifest_target("apps/ios", "../shared/Logging").unwrap(),
            "apps/shared/Logging"
        );
    }

    #[test]
    fn absolute_reference_error_suggests_the_relative_form() {
        let err = normalize_manifest_target("", "/apps/ios/AppKit").unwrap_err();
        assert_eq!(
            err,
            TargetIdError::Absolute {
                raw: "/apps/ios/AppKit".to_string(),
                suggestion: "apps/ios/AppKit".to_string(),
            }
        );
        assert!(err.to_string().contains("use `apps/ios/AppKit`"));
    }

    #[test]
    fn colon_reference_error_suggests_the_slash_form() {
        let err = normalize_manifest_target("", "apps/ios:AppKit").unwrap_err();
        assert_eq!(
            err,
            TargetIdError::Colon {
                raw: "apps/ios:AppKit".to_string(),
                suggestion: "apps/ios/AppKit".to_string(),
            }
        );
        assert!(err.to_string().contains("use `apps/ios/AppKit`"));
    }

    #[test]
    fn empty_segment_error_suggests_the_collapsed_form() {
        let err = normalize_manifest_target("", "apps//ios/AppKit").unwrap_err();
        assert_eq!(
            err,
            TargetIdError::EmptySegment {
                raw: "apps//ios/AppKit".to_string(),
                suggestion: "apps/ios/AppKit".to_string(),
            }
        );
    }

    #[test]
    fn cleanup_suggestion_falls_back_when_nothing_survives() {
        assert_eq!(cleanup_suggestion("/"), "target-name");
        assert_eq!(cleanup_suggestion(":::"), "target-name");
        assert_eq!(cleanup_suggestion("///"), "target-name");
    }

    #[test]
    fn cleanup_suggestion_strips_backslash_separators() {
        assert_eq!(cleanup_suggestion("foo\\bar"), "foo/bar");
        assert_eq!(cleanup_suggestion("apps\\ios\\App"), "apps/ios/App");
    }

    #[test]
    fn cleanup_suggestion_drops_parent_traversal_segments() {
        // `../foo` would still escape the workspace root, so the suggestion
        // must not carry `..` through.
        assert_eq!(cleanup_suggestion("/../foo"), "foo");
        assert_eq!(cleanup_suggestion("../../apps/ios"), "apps/ios");
    }
}
