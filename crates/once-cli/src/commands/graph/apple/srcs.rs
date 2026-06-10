//! Source resolution for Apple targets.
//!
//! `srcs` patterns on a graph target are glob patterns rooted in the
//! package directory (matching how Buck2 and Bazel resolve `glob()`
//! results). The expanded paths feed both the swiftc argv and the
//! action input digest, so the resolver returns a sorted, deduplicated
//! list of workspace-relative paths.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use once_frontend::GraphTarget;

/// Maximum sources a single Apple target can expand to. Mirrors the
/// limit `once exec` uses for script glob inputs so a wildly broad
/// pattern surfaces as a clear error instead of a multi-minute hang.
const MAX_SRC_MATCHES: usize = 4096;

/// Expand `srcs` patterns and return workspace-relative `.swift` files.
///
/// Non-Swift sources are filtered out today; Objective-C, C, and C++
/// support will land alongside their own clang driver wiring. Returns
/// an empty list when no patterns match Swift files, leaving the
/// caller to decide whether that is fatal.
pub(crate) fn resolve_swift_sources(workspace: &Path, target: &GraphTarget) -> Result<Vec<String>> {
    resolve_sources_with_extension(workspace, target, "swift")
}

fn resolve_sources_with_extension(
    workspace: &Path,
    target: &GraphTarget,
    extension: &str,
) -> Result<Vec<String>> {
    let package_dir = if target.label.package.is_empty() {
        workspace.to_path_buf()
    } else {
        workspace.join(&target.label.package)
    };
    let mut out: Vec<String> = Vec::new();
    for pattern in &target.srcs {
        let abs_pattern = package_dir.join(pattern);
        let pattern_str = abs_pattern
            .to_str()
            .ok_or_else(|| anyhow!("non-utf8 srcs pattern `{pattern}`"))?;
        for entry in
            glob::glob(pattern_str).with_context(|| format!("invalid srcs pattern `{pattern}`"))?
        {
            let path = entry.with_context(|| format!("srcs glob failed for `{pattern}`"))?;
            if !path.is_file() {
                continue;
            }
            if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_none_or(|ext| !ext.eq_ignore_ascii_case(extension))
            {
                continue;
            }
            let ws_rel = workspace_relative(workspace, &path)?;
            out.push(ws_rel);
            if out.len() > MAX_SRC_MATCHES {
                anyhow::bail!(
                    "srcs for {} matched more than {MAX_SRC_MATCHES} files; narrow the pattern",
                    target.label.id
                );
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn workspace_relative(workspace: &Path, path: &Path) -> Result<String> {
    let canonical_workspace = std::fs::canonicalize(workspace)
        .with_context(|| format!("canonicalizing workspace `{}`", workspace.display()))?;
    let canonical_path = std::fs::canonicalize(path)
        .with_context(|| format!("canonicalizing source `{}`", path.display()))?;
    let stripped = canonical_path
        .strip_prefix(&canonical_workspace)
        .with_context(|| {
            format!(
                "source `{}` is outside the workspace `{}`",
                canonical_path.display(),
                canonical_workspace.display()
            )
        })?;
    let normalized = stripped
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    if normalized.is_empty() {
        Err(anyhow!(
            "source `{}` resolved to an empty workspace path",
            path.display()
        ))
    } else {
        Ok(normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_frontend::{Capability, TargetLabel};
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::TempDir;

    fn library_target(package: &str, name: &str, srcs: Vec<String>) -> GraphTarget {
        GraphTarget {
            label: TargetLabel {
                package: package.to_string(),
                name: name.to_string(),
                id: if package.is_empty() {
                    name.to_string()
                } else {
                    format!("{package}/{name}")
                },
            },
            kind: "apple_library".to_string(),
            deps: Vec::new(),
            srcs,
            attrs: BTreeMap::new(),
            capabilities: vec![Capability {
                name: "build".to_string(),
                output_groups: Vec::new(),
                requires_outputs: Vec::new(),
            }],
            providers: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn resolves_swift_sources_from_package_relative_glob() {
        let tmp = TempDir::new().unwrap();
        let pkg = tmp.path().join("apps/ios/AppCore/Sources");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("first.swift"), "").unwrap();
        fs::write(pkg.join("second.swift"), "").unwrap();
        fs::write(pkg.join("readme.md"), "").unwrap();

        let target = library_target(
            "apps/ios/AppCore",
            "AppCore",
            vec!["Sources/*.swift".to_string()],
        );

        let resolved = resolve_swift_sources(tmp.path(), &target).unwrap();
        assert_eq!(
            resolved,
            vec![
                "apps/ios/AppCore/Sources/first.swift".to_string(),
                "apps/ios/AppCore/Sources/second.swift".to_string(),
            ]
        );
    }

    #[test]
    fn filters_out_non_swift_files_when_pattern_is_broad() {
        let tmp = TempDir::new().unwrap();
        let pkg = tmp.path().join("apps/ios/AppCore");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("a.swift"), "").unwrap();
        fs::write(pkg.join("b.m"), "").unwrap();
        let target = library_target("apps/ios/AppCore", "AppCore", vec!["*".to_string()]);

        let resolved = resolve_swift_sources(tmp.path(), &target).unwrap();
        assert_eq!(resolved, vec!["apps/ios/AppCore/a.swift".to_string()]);
    }

    #[test]
    fn dedups_when_multiple_patterns_overlap() {
        let tmp = TempDir::new().unwrap();
        let pkg = tmp.path().join("apps/ios/AppCore/Sources");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("first.swift"), "").unwrap();
        let target = library_target(
            "apps/ios/AppCore",
            "AppCore",
            vec![
                "Sources/*.swift".to_string(),
                "Sources/first.swift".to_string(),
            ],
        );

        let resolved = resolve_swift_sources(tmp.path(), &target).unwrap();
        assert_eq!(resolved.len(), 1);
    }

    #[test]
    fn returns_empty_when_no_matches_so_caller_can_diagnose() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("apps/ios/AppCore/Sources")).unwrap();
        let target = library_target(
            "apps/ios/AppCore",
            "AppCore",
            vec!["Sources/*.swift".to_string()],
        );
        let resolved = resolve_swift_sources(tmp.path(), &target).unwrap();
        assert!(resolved.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_pattern_that_escapes_workspace_via_canonicalize() {
        let workspace = TempDir::new().unwrap();
        let external = TempDir::new().unwrap();
        let pkg = workspace.path().join("apps/ios/AppCore");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(external.path().join("stolen.swift"), "").unwrap();
        // A symlink inside the package that points outside the workspace
        // would resolve outside on canonicalize; resolve_swift_sources
        // must reject it rather than silently include the file.
        std::os::unix::fs::symlink(
            external.path().join("stolen.swift"),
            pkg.join("escape.swift"),
        )
        .unwrap();
        let target = library_target(
            "apps/ios/AppCore",
            "AppCore",
            vec!["escape.swift".to_string()],
        );

        let err = resolve_swift_sources(workspace.path(), &target)
            .unwrap_err()
            .to_string();
        assert!(err.contains("outside the workspace"), "{err}");
    }
}
