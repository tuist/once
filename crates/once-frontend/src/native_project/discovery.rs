use std::collections::BTreeMap;
use std::path::{Component, Path};

use walkdir::WalkDir;

use crate::error::{Error, Result};

use super::{display_relative, NativeProjectMatch, NativeProjectSchema};

mod memory;

use memory::{RetainedMatches, MAX_RETAINED_MATCH_BYTES};

pub(super) fn detect_native_projects_with_schemas(
    root: &Path,
    schemas: &[NativeProjectSchema],
    boundary: &crate::workspace::WorkspaceScan,
) -> Result<(Vec<NativeProjectMatch>, usize)> {
    detect_native_projects_with_limit(root, schemas, boundary, MAX_RETAINED_MATCH_BYTES)
}

fn detect_native_projects_with_limit(
    root: &Path,
    schemas: &[NativeProjectSchema],
    boundary: &crate::workspace::WorkspaceScan,
    retained_match_limit: usize,
) -> Result<(Vec<NativeProjectMatch>, usize)> {
    let Some(max_depth) = schemas.iter().map(|schema| schema.max_depth).max() else {
        return Ok((Vec::new(), 0));
    };
    let walker = WalkDir::new(root)
        .max_depth(max_depth)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            let Some(name) = entry.file_name().to_str() else {
                return false;
            };
            if name.starts_with('.') {
                return false;
            }
            if !entry.file_type().is_dir() {
                return true;
            }
            let relative = display_relative(root, entry.path());
            boundary.includes_native_project_dir(&relative, entry.depth())
                && schemas.iter().any(|schema| {
                    entry.depth() < schema.max_depth
                        && !schema.exclude.iter().any(|excluded| excluded == name)
                })
        });
    let mut scan = DiscoveryScan::new(root, schemas, boundary, retained_match_limit)?;
    let mut visited_entries = 0;
    for entry in walker {
        let entry = entry.map_err(|source| Error::Walk {
            root: root.display().to_string(),
            source,
        })?;
        visited_entries += 1;
        scan.visit(&entry)?;
    }
    let mut matches = scan.finish();
    matches.sort_unstable_by(|left, right| {
        left.package
            .cmp(&right.package)
            .then(left.native_project.cmp(&right.native_project))
    });
    Ok((matches, visited_entries))
}

struct DiscoveryScan<'a> {
    root: &'a Path,
    schemas: &'a [NativeProjectSchema],
    boundary: &'a crate::workspace::WorkspaceScan,
    stop_markers: BTreeMap<&'a str, Vec<usize>>,
    descend_markers: BTreeMap<&'a str, Vec<usize>>,
    stop_schema_count: usize,
    retained: RetainedMatches,
}

impl<'a> DiscoveryScan<'a> {
    fn new(
        root: &'a Path,
        schemas: &'a [NativeProjectSchema],
        boundary: &'a crate::workspace::WorkspaceScan,
        retained_match_limit: usize,
    ) -> Result<Self> {
        let mut stop_markers = BTreeMap::<&str, Vec<usize>>::new();
        let mut descend_markers = BTreeMap::<&str, Vec<usize>>::new();
        for (index, schema) in schemas.iter().enumerate() {
            let markers = if schema.on_match == "stop" {
                &mut stop_markers
            } else {
                &mut descend_markers
            };
            markers
                .entry(schema.markers[0].as_str())
                .or_default()
                .push(index);
        }
        let stop_schema_count = stop_markers.values().map(Vec::len).sum();
        Ok(Self {
            root,
            schemas,
            boundary,
            stop_markers,
            descend_markers,
            stop_schema_count,
            retained: RetainedMatches::new(root, schemas.len(), retained_match_limit)?,
        })
    }

    fn visit(&mut self, entry: &walkdir::DirEntry) -> Result<()> {
        if entry.file_type().is_file() {
            let Some(name) = entry.file_name().to_str() else {
                return Ok(());
            };
            let Some(schema_indices) = self.descend_markers.get(name) else {
                return Ok(());
            };
            let package = entry.path().parent().unwrap_or(self.root);
            let package_name = display_relative(self.root, package);
            let primary_markers = [name.to_string()];
            return retain_marker_matches(
                self.root,
                self.schemas,
                schema_indices,
                self.boundary,
                &mut self.retained,
                package,
                &package_name,
                &primary_markers,
                entry.depth(),
            );
        }
        if !entry.file_type().is_dir() {
            return Ok(());
        }
        let package = entry.path();
        let marker_depth = entry.depth().saturating_add(1);
        let mut package_name = None;
        if self
            .retained
            .all_stop_schemas_rooted(self.stop_schema_count)
        {
            return Ok(());
        }
        for (primary_marker, schema_indices) in &self.stop_markers {
            if !schema_indices.iter().any(|index| {
                marker_depth <= self.schemas[*index].max_depth
                    && !package_has_excluded_component(
                        self.root,
                        package,
                        &self.schemas[*index].exclude,
                    )
                    && !self.retained.stop_covers(*index, self.root, package)
            }) {
                continue;
            }
            let primary_markers = resolve_marker(package, primary_marker);
            if primary_markers.is_empty() {
                continue;
            }
            let package_name =
                package_name.get_or_insert_with(|| display_relative(self.root, package));
            retain_marker_matches(
                self.root,
                self.schemas,
                schema_indices,
                self.boundary,
                &mut self.retained,
                package,
                package_name,
                &primary_markers,
                marker_depth,
            )?;
        }
        Ok(())
    }

    fn finish(self) -> Vec<NativeProjectMatch> {
        self.retained.finish()
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the shared marker check keeps stop and descend traversal behavior identical"
)]
fn retain_marker_matches(
    root: &Path,
    schemas: &[NativeProjectSchema],
    schema_indices: &[usize],
    boundary: &crate::workspace::WorkspaceScan,
    retained: &mut RetainedMatches,
    package: &Path,
    package_name: &str,
    primary_markers: &[String],
    marker_depth: usize,
) -> Result<()> {
    let manifest_path = package_child(package_name, crate::TOML_BUILD_FILE_NAME);
    let Some(first_marker) = primary_markers.first() else {
        return Ok(());
    };
    let marker_path = package_child(package_name, first_marker);
    for index in schema_indices {
        let schema = &schemas[*index];
        if marker_depth > schema.max_depth
            || package_has_excluded_component(root, package, &schema.exclude)
            || !boundary.includes_native_project(&manifest_path, &marker_path)
        {
            continue;
        }
        // Every secondary marker must also be present. Resolving them here keeps
        // a wildcard marker's concrete path in the match, so the seed's sources
        // name the files that were actually found.
        let mut markers = primary_markers.to_vec();
        let mut satisfied = true;
        for marker in schema.markers.iter().skip(1) {
            let resolved = resolve_marker(package, marker);
            if resolved.is_empty() {
                satisfied = false;
                break;
            }
            markers.extend(resolved);
        }
        if !satisfied {
            continue;
        }
        markers.dedup();
        retained.insert(root, *index, schema, package_name, markers)?;
    }
    Ok(())
}

fn package_child(package: &str, child: &str) -> String {
    if package.is_empty() {
        child.to_string()
    } else {
        format!("{package}/{child}")
    }
}

/// Expand one marker against a package directory and return every package-
/// relative path it matches, sorted for a stable graph. A literal marker
/// resolves to itself when it is a file. A marker with wildcard segments, such
/// as `*.xcodeproj/project.pbxproj`, resolves to each existing file whose
/// bundle name ends with the segment's literal suffix.
fn resolve_marker(package: &Path, marker: &str) -> Vec<String> {
    let mut resolved = vec![(package.to_path_buf(), String::new())];
    for segment in marker.split('/') {
        let mut next = Vec::new();
        for (directory, relative) in resolved {
            match segment.strip_prefix('*') {
                None => next.push((directory.join(segment), join_relative(&relative, segment))),
                Some(suffix) => {
                    let Ok(entries) = std::fs::read_dir(&directory) else {
                        continue;
                    };
                    let mut names = entries
                        .flatten()
                        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
                        .filter(|name| !name.starts_with('.') && name.ends_with(suffix))
                        .collect::<Vec<_>>();
                    names.sort_unstable();
                    for name in names {
                        next.push((directory.join(&name), join_relative(&relative, &name)));
                    }
                }
            }
        }
        resolved = next;
        if resolved.is_empty() {
            return Vec::new();
        }
    }
    let mut matched = resolved
        .into_iter()
        .filter(|(path, _)| is_primary_marker(path))
        .map(|(_, relative)| relative)
        .collect::<Vec<_>>();
    matched.sort_unstable();
    matched.dedup();
    matched
}

fn join_relative(prefix: &str, segment: &str) -> String {
    if prefix.is_empty() {
        segment.to_string()
    } else {
        format!("{prefix}/{segment}")
    }
}

fn is_primary_marker(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn package_has_excluded_component(root: &Path, package: &Path, excluded: &[String]) -> bool {
    package
        .strip_prefix(root)
        .unwrap_or(package)
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .any(|name| excluded.iter().any(|excluded| excluded == name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(name: &str, marker: &str, exclude: &[&str]) -> NativeProjectSchema {
        NativeProjectSchema {
            name: name.to_string(),
            docs: String::new(),
            markers: vec![marker.to_string()],
            target_name: name.to_string(),
            target_kind: format!("{name}_workspace"),
            inputs: Vec::new(),
            exclude: exclude.iter().map(|value| (*value).to_string()).collect(),
            input_exclude: Vec::new(),
            on_match: "descend".to_string(),
            max_depth: 16,
            requires_tools: Vec::new(),
        }
    }

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn prunes_workspace_boundaries_before_descending() {
        let temporary = tempfile::tempdir().unwrap();
        write(
            &temporary.path().join("once.toml"),
            r#"[workspace]
include = ["once.toml", "apps/*/once.toml"]
exclude = ["apps/excluded/**"]
"#,
        );
        write(&temporary.path().join("apps/keep/native.project"), "native");
        write(
            &temporary.path().join("apps/keep/secondary.project"),
            "secondary",
        );
        for index in 0..200 {
            write(
                &temporary
                    .path()
                    .join(format!("unrelated/branch-{index}/native.project")),
                "unrelated",
            );
            write(
                &temporary
                    .path()
                    .join(format!("apps/excluded/branch-{index}/secondary.project")),
                "excluded",
            );
        }
        let schemas = vec![
            schema("native", "native.project", &[]),
            schema("secondary", "secondary.project", &[]),
        ];
        let boundary = crate::workspace::load_workspace_scan(temporary.path()).unwrap();

        let (matches, visited_entries) =
            detect_native_projects_with_schemas(temporary.path(), &schemas, &boundary).unwrap();

        assert_eq!(
            matches
                .iter()
                .map(|matched| (matched.native_project.as_str(), matched.package.as_str()))
                .collect::<Vec<_>>(),
            vec![("native", "apps/keep"), ("secondary", "apps/keep")]
        );
        assert!(
            visited_entries <= 8,
            "bounded discovery visited {visited_entries} entries"
        );
    }

    #[test]
    fn preserves_schema_specific_exclusions() {
        let temporary = tempfile::tempdir().unwrap();
        write(&temporary.path().join("vendor/native.project"), "native");
        write(
            &temporary.path().join("vendor/secondary.project"),
            "secondary",
        );
        let schemas = vec![
            schema("native", "native.project", &["vendor"]),
            schema("secondary", "secondary.project", &[]),
        ];
        let boundary = crate::workspace::load_workspace_scan(temporary.path()).unwrap();

        let (matches, _) =
            detect_native_projects_with_schemas(temporary.path(), &schemas, &boundary).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].native_project, "secondary");
        assert_eq!(matches[0].package, "vendor");
    }

    #[test]
    fn stop_retains_only_the_shallowest_match_during_discovery() {
        let temporary = tempfile::tempdir().unwrap();
        write(&temporary.path().join("workspace/native.project"), "native");
        for index in 0..200 {
            write(
                &temporary
                    .path()
                    .join(format!("workspace/package-{index}/native.project")),
                "nested",
            );
        }
        let mut native = schema("native", "native.project", &[]);
        native.on_match = "stop".to_string();
        native.max_depth = 4;
        let schemas = vec![native];
        let boundary = crate::workspace::load_workspace_scan(temporary.path()).unwrap();
        let root_match_bytes = 4 * 1024;
        assert!(std::mem::size_of::<NativeProjectMatch>() * 200 > root_match_bytes);

        let (matches, _) = detect_native_projects_with_limit(
            temporary.path(),
            &schemas,
            &boundary,
            root_match_bytes,
        )
        .unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].package, "workspace");
    }

    #[test]
    fn descend_fails_before_retained_matches_exceed_the_limit() {
        let temporary = tempfile::tempdir().unwrap();
        for index in 0..20 {
            write(
                &temporary
                    .path()
                    .join(format!("projects/project-{index}/native.project")),
                "native",
            );
        }
        let schemas = vec![schema("native", "native.project", &[])];
        let boundary = crate::workspace::load_workspace_scan(temporary.path()).unwrap();
        let one_match_limit = 256;

        let error = detect_native_projects_with_limit(
            temporary.path(),
            &schemas,
            &boundary,
            one_match_limit,
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("native project discovery exceeded its"));
        assert!(error.to_string().contains("retained-result limit"));
    }
}
