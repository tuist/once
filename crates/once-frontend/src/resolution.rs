//! Graph expansion through target-kind-defined resolvers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

use crate::analysis::AnalysisEngine;
use crate::error::{Error, Result};
use crate::graph::{graph_target_from_schema, GraphTarget, TargetKindSchema};
use crate::target::{AttrValue, Target};
use crate::target_ref::{normalize_manifest_target, target_id, validate_target_name};

const MAX_EXPANDED_TARGETS: usize = 100_000;
const MAX_RESOLVER_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RESOLVER_FILES_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) struct ExpandedWorkspaceTargets {
    pub(crate) targets: Vec<Target>,
    pub(crate) diagnostics: BTreeMap<String, Vec<crate::Diagnostic>>,
}

#[derive(Debug)]
enum ResolverOutput {
    Targets(Vec<ResolvedTargetSpec>),
    Expanded(ResolvedTargetSet),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolvedTargetSet {
    #[serde(default)]
    targets: Vec<ResolvedTargetSpec>,
    roots: Option<Vec<String>>,
    #[serde(default)]
    attrs: BTreeMap<String, AttrValue>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolvedTargetSpec {
    name: String,
    kind: String,
    #[serde(default)]
    deps: Vec<String>,
    #[serde(default)]
    dependencies: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    srcs: Vec<String>,
    #[serde(default)]
    visibility: Vec<String>,
    #[serde(default)]
    attrs: BTreeMap<String, AttrValue>,
}

pub(crate) fn expand_workspace_targets(
    root: &Path,
    targets: Vec<Target>,
    schemas: &[TargetKindSchema],
) -> Result<ExpandedWorkspaceTargets> {
    if targets.len() > MAX_EXPANDED_TARGETS {
        return Err(resolution_error(
            "workspace",
            format!(
                "workspace declares {} targets, exceeding the expansion limit of {MAX_EXPANDED_TARGETS}",
                targets.len()
            ),
        ));
    }
    if !targets
        .iter()
        .any(|target| target_kind_has_resolver(schemas, &target.kind))
    {
        return Ok(ExpandedWorkspaceTargets {
            targets,
            diagnostics: BTreeMap::new(),
        });
    }
    AnalysisEngine::resolve_workspace_targets(root, |resolve| {
        Ok(expand_resolver_targets(root, targets, schemas, resolve))
    })
    .map_err(|source| Error::Eval {
        path: crate::modules::COMBINED_MODULE_PATH.to_string(),
        message: source.to_string(),
    })?
}

fn expand_resolver_targets<F>(
    root: &Path,
    mut targets: Vec<Target>,
    schemas: &[TargetKindSchema],
    resolve: &mut F,
) -> Result<ExpandedWorkspaceTargets>
where
    F: FnMut(&GraphTarget, &BTreeMap<String, String>) -> anyhow::Result<Option<serde_json::Value>>
        + ?Sized,
{
    let mut ids = targets.iter().map(Target::id).collect::<BTreeSet<_>>();
    let mut diagnostics = BTreeMap::<String, Vec<crate::Diagnostic>>::new();
    let mut index = 0;
    while index < targets.len() {
        let target = targets[index].clone();
        if target_kind_has_resolver(schemas, &target.kind) {
            let graph_target = graph_target_from_schema(&target, schemas);
            let owner_diagnostics = validate_resolver_owner(&graph_target, schemas)?;
            if !owner_diagnostics.is_empty() {
                diagnostics
                    .entry(graph_target.label.id)
                    .or_default()
                    .extend(owner_diagnostics);
                index += 1;
                continue;
            }
            let files = resolver_files(root, &target)?;
            let raw = resolve(&graph_target, &files)
                .map_err(|source| resolution_error(&target.id(), source.to_string()))?
                .ok_or_else(|| resolution_error(&target.id(), "resolver returned no value"))?;
            let output = parse_resolver_output(&target.id(), raw)?;
            let (specs, roots, attrs) = match output {
                ResolverOutput::Targets(specs) => (specs, None, BTreeMap::new()),
                ResolverOutput::Expanded(expanded) => {
                    (expanded.targets, expanded.roots, expanded.attrs)
                }
            };
            if specs.len() > MAX_EXPANDED_TARGETS - targets.len() {
                return Err(resolution_error(
                    &target.id(),
                    format!(
                        "resolver expansion exceeds the workspace limit of {MAX_EXPANDED_TARGETS} targets"
                    ),
                ));
            }
            let emitted_names = specs
                .iter()
                .map(|spec| spec.name.clone())
                .collect::<BTreeSet<_>>();
            let emitted_ids = specs
                .into_iter()
                .map(|spec| resolved_target(&target, spec))
                .collect::<Result<Vec<_>>>()?;
            for emitted in &emitted_ids {
                if !ids.insert(emitted.id()) {
                    return Err(resolution_error(
                        &target.id(),
                        format!("resolver emitted duplicate target `{}`", emitted.id()),
                    ));
                }
            }
            let roots = roots.unwrap_or_else(|| emitted_names.iter().cloned().collect());
            for root_ref in roots {
                let dependency = resolve_root(&target.package, &root_ref, &emitted_names)
                    .map_err(|message| resolution_error(&target.id(), message))?;
                if !targets[index].deps.contains(&dependency) {
                    targets[index].deps.push(dependency);
                }
            }
            for name in attrs.keys() {
                if targets[index].typed_attrs.contains_key(name) {
                    return Err(resolution_error(
                        &target.id(),
                        format!(
                            "resolver attribute `{name}` conflicts with a value declared by the owner target"
                        ),
                    ));
                }
            }
            targets[index].typed_attrs.extend(attrs);
            targets.extend(emitted_ids);
        }
        index += 1;
    }
    Ok(ExpandedWorkspaceTargets {
        targets,
        diagnostics,
    })
}

fn target_kind_has_resolver(schemas: &[TargetKindSchema], kind: &str) -> bool {
    schemas
        .iter()
        .find(|schema| schema.kind == kind)
        .is_some_and(|schema| schema.has_resolver)
}

fn validate_resolver_owner(
    target: &GraphTarget,
    schemas: &[TargetKindSchema],
) -> Result<Vec<crate::Diagnostic>> {
    let attrs = serde_json::to_value(&target.attrs).map_err(|source| {
        resolution_error(
            &target.label.id,
            format!("serializing resolver owner attributes: {source}"),
        )
    })?;
    let serde_json::Value::Object(attrs) = attrs else {
        unreachable!("target attributes always serialize as an object");
    };
    let spec = crate::manifest_editor::TargetSpec {
        name: target.label.name.clone(),
        kind: target.kind.clone(),
        deps: target.deps.clone(),
        dependencies: target.dependency_edges.clone(),
        srcs: target.srcs.clone(),
        visibility: target.visibility.clone(),
        attrs,
    };
    let mut diagnostics = crate::validate_target(&spec, schemas);
    for diagnostic in &mut diagnostics {
        diagnostic.target = Some(target.label.id.clone());
    }
    Ok(diagnostics)
}

fn parse_resolver_output(target: &str, raw: serde_json::Value) -> Result<ResolverOutput> {
    if raw.is_array() {
        serde_json::from_value::<Vec<ResolvedTargetSpec>>(raw)
            .map(ResolverOutput::Targets)
            .map_err(|source| {
                resolution_error(
                    target,
                    format!("resolver returned an invalid target list: {source}"),
                )
            })
    } else if raw.is_object() {
        serde_json::from_value::<ResolvedTargetSet>(raw)
            .map(ResolverOutput::Expanded)
            .map_err(|source| {
                resolution_error(
                    target,
                    format!("resolver returned an invalid target set: {source}"),
                )
            })
    } else {
        Err(resolution_error(
            target,
            "resolver must return a target list or a target-set object",
        ))
    }
}

fn resolved_target(owner: &Target, spec: ResolvedTargetSpec) -> Result<Target> {
    validate_target_name(&spec.name)
        .map_err(|source| resolution_error(&owner.id(), source.to_string()))?;
    if spec.kind.is_empty() {
        return Err(resolution_error(
            &owner.id(),
            format!("resolver target `{}` has an empty kind", spec.name),
        ));
    }
    let deps = normalize_dependencies(&owner.package, &owner.id(), "deps", spec.deps)?;
    let mut dependency_edges = BTreeMap::new();
    for (role, refs) in spec.dependencies {
        if role == "deps" {
            return Err(resolution_error(
                &owner.id(),
                format!(
                    "resolver target `{}` must use `deps` instead of a named `deps` role",
                    spec.name
                ),
            ));
        }
        dependency_edges.insert(
            role.clone(),
            normalize_dependencies(&owner.package, &owner.id(), &role, refs)?,
        );
    }
    Ok(Target {
        package: owner.package.clone(),
        kind: spec.kind,
        name: spec.name,
        deps,
        dependency_edges,
        srcs: spec.srcs,
        visibility: spec.visibility,
        attrs: BTreeMap::new(),
        typed_attrs: spec.attrs,
    })
}

fn normalize_dependencies(
    package: &str,
    owner: &str,
    role: &str,
    refs: Vec<String>,
) -> Result<Vec<String>> {
    refs.into_iter()
        .map(|raw| {
            normalize_manifest_target(package, &raw).map_err(|source| {
                resolution_error(
                    owner,
                    format!("resolver dependency role `{role}`: {source}"),
                )
            })
        })
        .collect()
}

fn resolve_root(
    package: &str,
    root_ref: &str,
    emitted_names: &BTreeSet<String>,
) -> std::result::Result<String, String> {
    if emitted_names.contains(root_ref) {
        return Ok(target_id(package, root_ref));
    }
    normalize_manifest_target(package, root_ref).map_err(|source| source.to_string())
}

fn resolver_files(root: &Path, target: &Target) -> Result<BTreeMap<String, String>> {
    let package_root = root.join(&target.package);
    let canonical_package_root =
        std::fs::canonicalize(&package_root).map_err(|source| Error::Read {
            path: package_root.display().to_string(),
            source,
        })?;
    let mut files = BTreeMap::new();
    let mut total_bytes = 0_u64;
    let source_patterns = resolver_source_patterns(target)?;
    for source_pattern in &source_patterns {
        let pattern = package_root
            .join(source_pattern)
            .to_string_lossy()
            .into_owned();
        let entries = glob::glob(&pattern).map_err(|source| {
            resolution_error(
                &target.id(),
                format!("invalid resolver source glob `{source_pattern}`: {source}"),
            )
        })?;
        for entry in entries {
            let path = entry.map_err(|source| {
                resolution_error(
                    &target.id(),
                    format!("resolving source glob `{source_pattern}`: {source}"),
                )
            })?;
            if !path.is_file() {
                continue;
            }
            let canonical = std::fs::canonicalize(&path).map_err(|source| Error::Read {
                path: path.display().to_string(),
                source,
            })?;
            if !canonical.starts_with(&canonical_package_root) {
                return Err(resolution_error(
                    &target.id(),
                    format!(
                        "resolver source `{}` resolves outside its owner package",
                        display_path(&path)
                    ),
                ));
            }
            let relative = path.strip_prefix(&package_root).map_err(|_| {
                resolution_error(
                    &target.id(),
                    format!(
                        "resolver source `{}` has no package-relative path",
                        display_path(&path)
                    ),
                )
            })?;
            let key = display_path(relative);
            if files.contains_key(&key) {
                continue;
            }
            let size = std::fs::metadata(&canonical)
                .map_err(|source| Error::Read {
                    path: path.display().to_string(),
                    source,
                })?
                .len();
            if size > MAX_RESOLVER_FILE_BYTES {
                return Err(resolution_error(
                    &target.id(),
                    format!(
                        "resolver source `{}` is {size} bytes, exceeding the per-file limit of {MAX_RESOLVER_FILE_BYTES} bytes",
                        display_path(&path)
                    ),
                ));
            }
            total_bytes = total_bytes
                .checked_add(size)
                .ok_or_else(|| resolution_error(&target.id(), "resolver source size overflowed"))?;
            if total_bytes > MAX_RESOLVER_FILES_BYTES {
                return Err(resolution_error(
                    &target.id(),
                    format!(
                        "resolver sources exceed the total limit of {MAX_RESOLVER_FILES_BYTES} bytes while reading `{}`",
                        display_path(&path)
                    ),
                ));
            }
            let bytes = std::fs::read(&canonical).map_err(|source| Error::Read {
                path: path.display().to_string(),
                source,
            })?;
            let contents = String::from_utf8(bytes).map_err(|_| {
                resolution_error(
                    &target.id(),
                    format!(
                        "resolver source `{}` is not valid UTF-8 text",
                        display_path(&path)
                    ),
                )
            })?;
            files.insert(key, contents);
        }
    }
    Ok(files)
}

fn resolver_source_patterns(target: &Target) -> Result<Vec<String>> {
    let Some(value) = target.typed_attrs.get("resolver_inputs") else {
        return Ok(target.srcs.clone());
    };
    let AttrValue::List(values) = value else {
        return Err(resolution_error(
            &target.id(),
            "resolver_inputs must be a list of package-relative source patterns",
        ));
    };
    if values.is_empty() {
        return Ok(target.srcs.clone());
    }
    values
        .iter()
        .map(|value| {
            value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                resolution_error(
                    &target.id(),
                    "resolver_inputs must contain only package-relative source patterns",
                )
            })
        })
        .collect()
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn resolution_error(target: &str, message: impl Into<String>) -> Error {
    Error::Eval {
        path: target.to_string(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: std::path::PathBuf, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn expands_resolver_targets_and_merges_owner_metadata() {
        let temp = tempfile::tempdir().unwrap();
        write(
            temp.path().join("once.toml"),
            r#"[modules]
paths = ["deps.star"]

[[target]]
name = "packages"
kind = "resolved_set"
srcs = ["deps.lock"]
"#,
        );
        write(temp.path().join("deps.lock"), "locked-v1\n");
        write(
            temp.path().join("deps.star"),
            r#"def _resolve(ctx):
    return {
        "targets": [{
            "name": "leaf",
            "kind": "resolved_leaf",
            "attrs": {"identity": ctx["files"]["deps.lock"].strip()},
        }],
        "roots": ["leaf"],
        "attrs": {"resolution": "locked"},
    }

resolved_set = target_kind(
    attrs = [attr("resolution", "string")],
    deps = [dep("deps", ["resolved_leaf"])],
    providers = ["resolved_set"],
    resolver = _resolve,
)

resolved_leaf = target_kind(
    attrs = [attr("identity", "string", required = True)],
    providers = ["resolved_leaf"],
)
"#,
        );

        let graph = crate::graph::load_graph_workspace(temp.path()).unwrap();
        assert_eq!(graph.len(), 2);
        let owner = graph
            .iter()
            .find(|target| target.label.id == "packages")
            .unwrap();
        assert_eq!(owner.deps, vec!["leaf"]);
        assert_eq!(
            owner.attrs.get("resolution"),
            Some(&AttrValue::String("locked".to_string()))
        );
        let leaf = graph
            .iter()
            .find(|target| target.label.id == "leaf")
            .unwrap();
        assert_eq!(
            leaf.attrs.get("identity"),
            Some(&AttrValue::String("locked-v1".to_string()))
        );
    }

    #[test]
    fn resolver_attributes_cannot_overwrite_owner_values() {
        let temp = tempfile::tempdir().unwrap();
        write(
            temp.path().join("once.toml"),
            r#"[modules]
paths = ["deps.star"]

[[target]]
name = "packages"
kind = "resolved_set"

[target.attrs]
resolution = "declared"
"#,
        );
        write(
            temp.path().join("deps.star"),
            r#"def _resolve(ctx):
    return {"attrs": {"resolution": "resolver"}}

resolved_set = target_kind(
    attrs = [attr("resolution", "string")],
    resolver = _resolve,
)
"#,
        );

        let error = crate::graph::load_graph_workspace(temp.path()).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("resolver attribute `resolution` conflicts"),
            "{error}"
        );
    }

    #[test]
    fn invalid_resolver_owner_attributes_return_structured_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        write(
            temp.path().join("once.toml"),
            r#"[modules]
paths = ["deps.star"]

[[target]]
name = "packages"
kind = "resolved_set"

[target.attrs]
manifest = { select = { default = "deps.lock" } }
vendour_dir = "deps"
"#,
        );
        write(
            temp.path().join("deps.star"),
            r#"def _resolve(ctx):
    fail("resolver should not run")

resolved_set = target_kind(
    attrs = [attr("manifest", "string", configurable = False)],
    resolver = _resolve,
)
"#,
        );

        let graph = crate::graph::load_graph_workspace(temp.path()).unwrap();
        let diagnostics = &graph
            .iter()
            .find(|target| target.label.id == "packages")
            .expect("resolver owner remains in the graph")
            .diagnostics;

        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "select_on_non_configurable_attr"
                && diagnostic.target.as_deref() == Some("packages")
                && diagnostic.attribute.as_deref() == Some("manifest")
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "unknown_attr"
                && diagnostic.attribute.as_deref() == Some("vendour_dir")
        }));

        let workspace_diagnostics = crate::validate_workspace(temp.path()).unwrap();
        assert!(workspace_diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "select_on_non_configurable_attr"
                && diagnostic.target.as_deref() == Some("packages")
        }));
        assert!(workspace_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "unknown_attr"));
    }

    #[test]
    fn resolver_failures_take_precedence_over_side_effect_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        write(
            temp.path().join("once.toml"),
            r#"[modules]
paths = ["deps.star"]

[[target]]
name = "packages"
kind = "resolved_set"
"#,
        );
        write(
            temp.path().join("deps.star"),
            r#"def _resolve(ctx):
    declare_output("forbidden")
    fail("specific resolver failure")

resolved_set = target_kind(resolver = _resolve)
"#,
        );

        let error = crate::graph::load_graph_workspace(temp.path())
            .unwrap_err()
            .to_string();

        assert!(error.contains("specific resolver failure"), "{error}");
        assert!(!error.contains("declared actions or outputs"), "{error}");
    }

    #[test]
    fn successful_resolvers_cannot_declare_outputs() {
        let temp = tempfile::tempdir().unwrap();
        write(
            temp.path().join("once.toml"),
            r#"[modules]
paths = ["deps.star"]

[[target]]
name = "packages"
kind = "resolved_set"
"#,
        );
        write(
            temp.path().join("deps.star"),
            r#"def _resolve(ctx):
    declare_output("forbidden")
    return []

resolved_set = target_kind(resolver = _resolve)
"#,
        );

        let error = crate::graph::load_graph_workspace(temp.path())
            .unwrap_err()
            .to_string();

        assert!(error.contains("declared actions or outputs"), "{error}");
    }

    #[test]
    fn duplicate_manifest_targets_reach_workspace_validation() {
        let temp = tempfile::tempdir().unwrap();
        write(
            temp.path().join("once.toml"),
            r#"[[target]]
name = "duplicate"
kind = "rust_library"

[[target]]
name = "duplicate"
kind = "rust_library"
"#,
        );

        let diagnostics = crate::validate_workspace(temp.path()).unwrap();

        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "duplicate_target"),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn resolver_output_errors_name_the_invalid_field() {
        let error = parse_resolver_output(
            "packages",
            serde_json::json!({
                "targets": [{
                    "name": "leaf",
                    "kind": "resolved_leaf",
                    "unexpected": true,
                }],
            }),
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown field `unexpected`"));
    }

    #[test]
    fn resolver_files_reject_non_text_and_oversized_inputs() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("binary.lock"), [0xff, 0xfe]).unwrap();
        let oversized = std::fs::File::create(temp.path().join("large.lock")).unwrap();
        oversized.set_len(MAX_RESOLVER_FILE_BYTES + 1).unwrap();
        let target = |source: &str| Target {
            package: String::new(),
            kind: "resolved_set".to_string(),
            name: "packages".to_string(),
            deps: Vec::new(),
            dependency_edges: BTreeMap::new(),
            srcs: vec![source.to_string()],
            visibility: Vec::new(),
            attrs: BTreeMap::new(),
            typed_attrs: BTreeMap::new(),
        };

        let binary_error = resolver_files(temp.path(), &target("binary.lock")).unwrap_err();
        assert!(binary_error.to_string().contains("not valid UTF-8 text"));
        let large_error = resolver_files(temp.path(), &target("large.lock")).unwrap_err();
        assert!(large_error.to_string().contains("per-file limit"));
    }

    #[test]
    fn resolver_inputs_narrow_the_text_context_without_narrowing_build_sources() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path().join("deps.lock"), "locked-v1\n");
        std::fs::write(temp.path().join("artifact.bin"), [0xff, 0xfe]).unwrap();
        let mut typed_attrs = BTreeMap::new();
        typed_attrs.insert(
            "resolver_inputs".to_string(),
            AttrValue::List(vec![AttrValue::String("deps.lock".to_string())]),
        );
        let target = Target {
            package: String::new(),
            kind: "resolved_set".to_string(),
            name: "packages".to_string(),
            deps: Vec::new(),
            dependency_edges: BTreeMap::new(),
            srcs: vec!["deps.lock".to_string(), "artifact.bin".to_string()],
            visibility: Vec::new(),
            attrs: BTreeMap::new(),
            typed_attrs,
        };

        let files = resolver_files(temp.path(), &target).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            files.get("deps.lock").map(String::as_str),
            Some("locked-v1\n")
        );
    }

    #[test]
    fn empty_resolver_inputs_fall_back_to_build_sources() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path().join("deps.lock"), "locked-v1\n");
        let mut typed_attrs = BTreeMap::new();
        typed_attrs.insert("resolver_inputs".to_string(), AttrValue::List(Vec::new()));
        let target = Target {
            package: String::new(),
            kind: "resolved_set".to_string(),
            name: "packages".to_string(),
            deps: Vec::new(),
            dependency_edges: BTreeMap::new(),
            srcs: vec!["deps.lock".to_string()],
            visibility: Vec::new(),
            attrs: BTreeMap::new(),
            typed_attrs,
        };

        let files = resolver_files(temp.path(), &target).unwrap();

        assert_eq!(files.get("deps.lock"), Some(&"locked-v1\n".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn resolver_inputs_keep_the_declared_name_for_package_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        write(temp.path().join("actual.lock"), "locked-v1\n");
        symlink("actual.lock", temp.path().join("declared.lock")).unwrap();
        let target = Target {
            package: String::new(),
            kind: "resolved_set".to_string(),
            name: "packages".to_string(),
            deps: Vec::new(),
            dependency_edges: BTreeMap::new(),
            srcs: vec!["declared.lock".to_string()],
            visibility: Vec::new(),
            attrs: BTreeMap::new(),
            typed_attrs: BTreeMap::new(),
        };

        let files = resolver_files(temp.path(), &target).unwrap();

        assert_eq!(
            files.get("declared.lock").map(String::as_str),
            Some("locked-v1\n")
        );
        assert!(!files.contains_key("actual.lock"));
    }

    #[test]
    fn resolver_inputs_cannot_escape_the_owner_package() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path().join("outside.lock"), "locked-v1\n");
        write(temp.path().join("package/marker"), "owner\n");
        let mut typed_attrs = BTreeMap::new();
        typed_attrs.insert(
            "resolver_inputs".to_string(),
            AttrValue::List(vec![AttrValue::String("../outside.lock".to_string())]),
        );
        let target = Target {
            package: "package".to_string(),
            kind: "resolved_set".to_string(),
            name: "packages".to_string(),
            deps: Vec::new(),
            dependency_edges: BTreeMap::new(),
            srcs: Vec::new(),
            visibility: Vec::new(),
            attrs: BTreeMap::new(),
            typed_attrs,
        };

        let error = resolver_files(temp.path(), &target).unwrap_err();
        assert!(error.to_string().contains("outside its owner package"));
    }
}
