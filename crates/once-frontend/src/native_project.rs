use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use crate::error::{Error, Result};
use crate::graph::GraphTarget;
use crate::target::{AttrValue, Target};
use serde::Serialize;
use starlark::environment::Module;
use starlark::values::dict::DictRef;
use starlark::values::list::ListRef;

mod discovery;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NativeProjectSchema {
    pub name: String,
    pub docs: String,
    pub markers: Vec<String>,
    pub target_name: String,
    pub target_kind: String,
    pub inputs: Vec<String>,
    pub exclude: Vec<String>,
    /// Directory names to skip when gathering the project's resolver inputs.
    ///
    /// Separate from `exclude`, which says where not to look for a project at
    /// all. A vendored dependency's directory is excluded from discovery, so it
    /// is not mistaken for a project of its own, while its manifest is still an
    /// input to the project that vendored it. What belongs here is a directory
    /// that cannot hold an input under any reading: a build output tree, a
    /// version control directory.
    pub input_exclude: Vec<String>,
    pub on_match: String,
    pub max_depth: usize,
    pub requires_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct NativeProjectMatch {
    pub native_project: String,
    pub package: String,
    pub markers: Vec<String>,
    pub seed_target: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NativeProjectPreview {
    pub native_project: NativeProjectSchema,
    pub matched: NativeProjectMatch,
    pub seed: Target,
    pub targets: Vec<GraphTarget>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NativeProjectCatalog {
    pub native_projects: Vec<NativeProjectSchema>,
    pub matches: Vec<NativeProjectMatch>,
}

pub fn native_project_schemas_for_workspace(root: &Path) -> Result<Vec<NativeProjectSchema>> {
    let target_kinds = crate::graph::target_kind_schemas_for_workspace(root)?;
    native_project_schemas_for_workspace_with_target_kinds(root, &target_kinds)
}

pub(crate) fn native_project_schemas_for_workspace_with_target_kinds(
    root: &Path,
    target_kinds: &[crate::graph::TargetKindSchema],
) -> Result<Vec<NativeProjectSchema>> {
    let source = crate::modules::combined_module_source_for_workspace(root)?;
    let native_projects = crate::graph::prelude_exports(&source)
        .native_projects
        .clone()
        .map_err(|message| Error::Eval {
            path: crate::modules::COMBINED_MODULE_PATH.to_string(),
            message,
        })?;
    let target_kinds = target_kinds
        .iter()
        .map(|schema| schema.kind.as_str())
        .collect::<BTreeSet<_>>();
    for native_project in &native_projects {
        if !target_kinds.contains(native_project.target_kind.as_str()) {
            return Err(native_project_error(
                &native_project.name,
                format!(
                    "native project `{}` references unknown target kind `{}`",
                    native_project.name, native_project.target_kind
                ),
            ));
        }
    }
    Ok(native_projects)
}

pub fn discover_native_projects(root: &Path) -> Result<NativeProjectCatalog> {
    let target_kinds = crate::graph::target_kind_schemas_for_workspace(root)?;
    let native_projects =
        native_project_schemas_for_workspace_with_target_kinds(root, &target_kinds)?;
    let boundary = crate::workspace::load_workspace_scan(root)?;
    let (matches, _) =
        discovery::detect_native_projects_with_schemas(root, &native_projects, &boundary)?;
    Ok(NativeProjectCatalog {
        native_projects,
        matches,
    })
}

pub fn detect_native_projects(root: &Path) -> Result<Vec<NativeProjectMatch>> {
    Ok(discover_native_projects(root)?.matches)
}

pub(crate) fn synthesized_workspace_seeds(
    root: &Path,
    schemas: &[NativeProjectSchema],
    boundary: &crate::workspace::WorkspaceScan,
) -> Result<Vec<(NativeProjectMatch, Target)>> {
    let schema_by_name = schemas
        .iter()
        .map(|schema| (schema.name.as_str(), schema))
        .collect::<BTreeMap<_, _>>();
    let mut targets = Vec::new();
    let mut ids = BTreeMap::<String, String>::new();
    let (matches, _) = discovery::detect_native_projects_with_schemas(root, schemas, boundary)?;
    for matched in matches {
        let schema = schema_by_name
            .get(matched.native_project.as_str())
            .ok_or_else(|| Error::Eval {
                path: crate::modules::COMBINED_MODULE_PATH.to_string(),
                message: format!(
                    "native project `{}` disappeared during discovery",
                    matched.native_project
                ),
            })?;
        let target = seed_target(schema, &matched.package, &matched.markers);
        if let Some(previous_native_project) =
            ids.insert(target.id(), matched.native_project.clone())
        {
            return Err(Error::Eval {
                path: matched.native_project.clone(),
                message: format!(
                    "native projects `{previous_native_project}` and `{}` both emitted seed target `{}` in package `{}`",
                    matched.native_project,
                    target.id(),
                    if matched.package.is_empty() { "." } else { &matched.package },
                ),
            });
        }
        targets.push((matched, target));
    }
    Ok(targets)
}

pub fn native_project_seed_target(root: &Path, name: &str, package: &str) -> Result<Target> {
    let catalog = discover_native_projects(root)?;
    let schema = catalog
        .native_projects
        .into_iter()
        .find(|schema| schema.name == name)
        .ok_or_else(|| Error::Eval {
            path: crate::modules::COMBINED_MODULE_PATH.to_string(),
            message: format!("unknown native project `{name}`"),
        })?;
    let matched = catalog
        .matches
        .into_iter()
        .find(|matched| matched.native_project == name && matched.package == package)
        .ok_or_else(|| Error::Eval {
            path: name.to_string(),
            message: format!(
                "native project `{name}` does not match package `{}`",
                if package.is_empty() { "." } else { package }
            ),
        })?;
    Ok(seed_target(&schema, package, &matched.markers))
}

pub fn preview_native_project(
    root: &Path,
    name: &str,
    package: &str,
) -> Result<NativeProjectPreview> {
    let target_kinds = crate::graph::target_kind_schemas_for_workspace(root)?;
    let native_projects =
        native_project_schemas_for_workspace_with_target_kinds(root, &target_kinds)?;
    let boundary = crate::workspace::load_workspace_scan(root)?;
    let (matches, _) =
        discovery::detect_native_projects_with_schemas(root, &native_projects, &boundary)?;
    let schema = native_projects
        .into_iter()
        .find(|schema| schema.name == name)
        .ok_or_else(|| Error::Eval {
            path: crate::modules::COMBINED_MODULE_PATH.to_string(),
            message: format!("unknown native project `{name}`"),
        })?;
    let selected_match = matches
        .into_iter()
        .find(|matched| matched.native_project == name && matched.package == package)
        .ok_or_else(|| Error::Eval {
            path: name.to_string(),
            message: format!(
                "native project `{name}` does not match package `{}`",
                if package.is_empty() { "." } else { package }
            ),
        })?;
    let seed = seed_target(&schema, package, &selected_match.markers);
    let targets = crate::graph::load_graph_workspace_with_targets_and_schemas(
        root,
        vec![seed.clone()],
        &target_kinds,
    )?;
    Ok(NativeProjectPreview {
        native_project: schema,
        matched: selected_match,
        seed,
        targets,
    })
}

/// Build the ephemeral seed for one match. `markers` are the marker paths as
/// they were resolved against the package, so a wildcard marker contributes the
/// concrete file it matched rather than the declared pattern.
fn seed_target(schema: &NativeProjectSchema, package: &str, markers: &[String]) -> Target {
    let resolver_inputs = markers
        .iter()
        .chain(schema.inputs.iter())
        .cloned()
        .map(AttrValue::String)
        .collect();
    Target {
        package: package.to_string(),
        kind: schema.target_kind.clone(),
        name: schema.target_name.clone(),
        deps: Vec::new(),
        dependency_edges: BTreeMap::new(),
        srcs: markers.to_vec(),
        visibility: Vec::new(),
        attrs: BTreeMap::new(),
        typed_attrs: BTreeMap::from([(
            "resolver_inputs".to_string(),
            AttrValue::List(resolver_inputs),
        )]),
        resolver_input_exclude: schema.input_exclude.clone(),
    }
}

/// Read the native project declarations out of an already-evaluated module.
///
/// Split from evaluation so one evaluation of a prelude source answers both
/// this and the target kind schemas, which is what an invocation asks for.
/// Errors come back as text because the caller knows which path to report them
/// under: the same source reaches here under more than one name.
pub(crate) fn native_project_schemas_from_module(
    module: &Module<'_>,
) -> std::result::Result<Vec<NativeProjectSchema>, String> {
    let exports = crate::modules::exported_native_project_values(module);
    let mut names = BTreeSet::new();
    exports
        .into_iter()
        .map(|export| {
            let dict = DictRef::from_value(export.value).ok_or_else(|| Error::Eval {
                path: crate::modules::COMBINED_MODULE_PATH.to_string(),
                message: format!("native project export `{}` should be a dict", export.name),
            })?;
            let name = optional_string(&dict, "name")?.unwrap_or_else(|| export.name.to_string());
            if name.is_empty() {
                return Err(native_project_error(
                    &name,
                    format!("native project export `{}` has an empty name", export.name),
                ));
            }
            if !names.insert(name.clone()) {
                return Err(native_project_error(
                    &name,
                    format!("native project `{name}` is declared more than once"),
                ));
            }
            native_project_schema(name, &dict)
        })
        .collect::<Result<Vec<_>>>()
        .map_err(|error| error.to_string())
}

/// Validate one native project export and turn it into its schema.
///
/// The caller has already resolved the export's name, since that is
/// what deduplicates declarations across the prelude.
fn native_project_schema(name: String, dict: &DictRef<'_>) -> Result<NativeProjectSchema> {
    let markers = string_list(dict, "markers")?;
    if markers.is_empty() {
        return Err(native_project_error(
            &name,
            format!("native project `{name}` must declare markers"),
        ));
    }
    let target_kind = required_string(dict, "target_kind")?;
    if target_kind.is_empty() {
        return Err(native_project_error(
            &name,
            format!("native project `{name}` has an empty target kind"),
        ));
    }
    let target_name = optional_string(dict, "target_name")?.unwrap_or_else(|| name.clone());
    crate::target_ref::validate_target_name(&target_name)
        .map_err(|source| native_project_error(&name, source.to_string()))?;
    let inputs = string_list(dict, "inputs")?;
    let exclude = string_list(dict, "exclude")?;
    let input_exclude = string_list(dict, "input_exclude")?;
    let on_match = required_string(dict, "on_match")?;
    if on_match != "stop" && on_match != "descend" {
        return Err(native_project_error(
            &name,
            format!("native project `{name}` on_match must be `stop` or `descend`"),
        ));
    }
    let max_depth = positive_usize(required_i32(dict, "max_depth")?, &name, "max_depth")?;
    let requires_tools = string_list(dict, "requires_tools")?;
    for marker in &markers {
        validate_marker(&name, marker)?;
    }
    // Descend matching indexes the primary marker by file name as the
    // walk visits files, so a primary marker that needs directory
    // expansion or a parent segment can only be matched by the
    // stop-mode directory pass.
    if (marker_is_pattern(&markers[0]) || markers[0].contains('/')) && on_match != "stop" {
        return Err(native_project_error(
                        &name,
                        format!(
                            "native project `{name}` marker `{}` spans a directory, so it requires on_match = \"stop\"",
                            markers[0]
                        ),
                    ));
    }
    for excluded in exclude.iter().chain(input_exclude.iter()) {
        validate_relative_literal(&name, "excluded directory", excluded)?;
    }
    for input in &inputs {
        validate_source_pattern(&name, input)?;
    }
    Ok(NativeProjectSchema {
        name,
        docs: required_string(dict, "docs")?,
        markers,
        target_name,
        target_kind,
        inputs,
        exclude,
        input_exclude,
        on_match,
        max_depth,
        requires_tools,
    })
}

fn validate_relative_literal(native_project: &str, field: &str, value: &str) -> Result<()> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(native_project_error(
            native_project,
            format!("native project {field} `{value}` must be a normalized relative path"),
        ));
    }
    Ok(())
}

/// Validate one marker, which is a normalized relative path whose segments may
/// each be a literal name or a leading-wildcard pattern such as
/// `*.xcodeproj`. A wildcard segment matches the directory entries that end
/// with its literal suffix, so a project bundle whose name varies by repository
/// is still discoverable.
fn validate_marker(native_project: &str, marker: &str) -> Result<()> {
    validate_relative_literal(native_project, "marker", marker)?;
    for segment in marker.split('/') {
        let Some(suffix) = segment.strip_prefix('*') else {
            if segment.contains('*') {
                return Err(native_project_error(
                    native_project,
                    format!(
                        "native project marker `{marker}` may only use `*` at the start of a path segment"
                    ),
                ));
            }
            continue;
        };
        if suffix.is_empty() || suffix.contains('*') {
            return Err(native_project_error(
                native_project,
                format!(
                    "native project marker `{marker}` wildcard segment must be `*` followed by one literal suffix"
                ),
            ));
        }
    }
    Ok(())
}

/// Whether a marker needs directory expansion before it can be matched.
pub(crate) fn marker_is_pattern(marker: &str) -> bool {
    marker.contains('*')
}

fn validate_source_pattern(native_project: &str, value: &str) -> Result<()> {
    if value.is_empty() || Path::new(value).is_absolute() {
        return Err(native_project_error(
            native_project,
            format!("native project input `{value}` must be a non-empty relative glob"),
        ));
    }
    if Path::new(value).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(native_project_error(
            native_project,
            format!("native project input `{value}` must stay inside its package"),
        ));
    }
    Ok(())
}

fn optional_string(dict: &DictRef<'_>, field: &str) -> Result<Option<String>> {
    let value = dict
        .get_str(field)
        .ok_or_else(|| native_project_error("", format!("native project is missing `{field}`")))?;
    if value.is_none() {
        return Ok(None);
    }
    value
        .unpack_str()
        .map(|value| Some(value.to_string()))
        .ok_or_else(|| {
            native_project_error(
                "",
                format!("native project `{field}` must be a string or None"),
            )
        })
}

fn required_string(dict: &DictRef<'_>, field: &str) -> Result<String> {
    optional_string(dict, field)?.ok_or_else(|| {
        native_project_error("", format!("native project `{field}` must be a string"))
    })
}

fn string_list(dict: &DictRef<'_>, field: &str) -> Result<Vec<String>> {
    let value = dict
        .get_str(field)
        .ok_or_else(|| native_project_error("", format!("native project is missing `{field}`")))?;
    let list = ListRef::from_value(value).ok_or_else(|| {
        native_project_error(
            "",
            format!("native project `{field}` must be a list of strings"),
        )
    })?;
    list.iter()
        .map(|value| {
            value.unpack_str().map(ToOwned::to_owned).ok_or_else(|| {
                native_project_error(
                    "",
                    format!("native project `{field}` entries must be strings"),
                )
            })
        })
        .collect()
}

fn required_i32(dict: &DictRef<'_>, field: &str) -> Result<i32> {
    dict.get_str(field)
        .and_then(starlark::values::Value::unpack_i32)
        .ok_or_else(|| {
            native_project_error("", format!("native project `{field}` must be an integer"))
        })
}

fn positive_usize(value: i32, name: &str, field: &str) -> Result<usize> {
    usize::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            native_project_error(
                name,
                format!("native project `{name}` {field} must be positive"),
            )
        })
}

fn native_project_error(path: &str, message: impl Into<String>) -> Error {
    Error::Eval {
        path: path.to_string(),
        message: message.into(),
    }
}

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Evaluate a module source and read its native project declarations.
    ///
    /// Production code reaches these through the shared prelude evaluation,
    /// which is keyed by the source text, so there is no path to name here.
    fn native_project_schemas_from_source(
        source: &str,
    ) -> std::result::Result<Vec<NativeProjectSchema>, String> {
        crate::graph::prelude_exports(source)
            .native_projects
            .clone()
    }

    #[test]
    fn parses_and_detects_a_native_project() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("native.project"), "project").unwrap();
        let source = format!(
            "{}\ndemo = native_project(target_kind = \"demo_workspace\", docs = \"Demo\", markers = [\"native.project\"])\n",
            crate::modules::common_module_source()
        );
        let schemas = native_project_schemas_from_source(&source).unwrap();
        assert_eq!(schemas[0].name, "demo");
        assert_eq!(schemas[0].target_kind, "demo_workspace");

        let boundary = crate::workspace::load_workspace_scan(temporary.path()).unwrap();
        let (matches, _) =
            discovery::detect_native_projects_with_schemas(temporary.path(), &schemas, &boundary)
                .unwrap();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].package.is_empty());
    }

    #[test]
    fn wildcard_markers_resolve_to_the_files_they_matched() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temporary.path().join("Browser.xcodeproj")).unwrap();
        std::fs::write(
            temporary.path().join("Browser.xcodeproj/project.pbxproj"),
            "// !$*UTF8*$!\n",
        )
        .unwrap();
        let source = format!(
            "{}\ndemo = native_project(target_kind = \"demo_workspace\", docs = \"Demo\", markers = [\"*.xcodeproj/project.pbxproj\"], on_match = \"stop\")\n",
            crate::modules::common_module_source()
        );
        let schemas = native_project_schemas_from_source(&source).unwrap();

        let boundary = crate::workspace::load_workspace_scan(temporary.path()).unwrap();
        let (matches, _) =
            discovery::detect_native_projects_with_schemas(temporary.path(), &schemas, &boundary)
                .unwrap();

        assert_eq!(matches.len(), 1);
        assert!(matches[0].package.is_empty());
        // The match and its seed carry the concrete path, not the pattern, so
        // the resolver reads the project that was actually found.
        assert_eq!(
            matches[0].markers,
            vec!["Browser.xcodeproj/project.pbxproj".to_string()]
        );
        let seed = seed_target(&schemas[0], &matches[0].package, &matches[0].markers);
        assert_eq!(seed.srcs, vec!["Browser.xcodeproj/project.pbxproj"]);
    }

    #[test]
    fn wildcard_markers_do_not_match_a_bundle_without_its_manifest() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temporary.path().join("Browser.xcodeproj")).unwrap();
        let source = format!(
            "{}\ndemo = native_project(target_kind = \"demo_workspace\", docs = \"Demo\", markers = [\"*.xcodeproj/project.pbxproj\"], on_match = \"stop\")\n",
            crate::modules::common_module_source()
        );
        let schemas = native_project_schemas_from_source(&source).unwrap();

        let boundary = crate::workspace::load_workspace_scan(temporary.path()).unwrap();
        let (matches, _) =
            discovery::detect_native_projects_with_schemas(temporary.path(), &schemas, &boundary)
                .unwrap();

        assert!(matches.is_empty());
    }

    #[test]
    fn a_directory_spanning_marker_requires_stop_matching() {
        let source = format!(
            "{}\ndemo = native_project(target_kind = \"demo_workspace\", docs = \"Demo\", markers = [\"*.xcodeproj/project.pbxproj\"])\n",
            crate::modules::common_module_source()
        );
        let error = native_project_schemas_from_source(&source).unwrap_err();
        assert!(
            error.contains("on_match = \"stop\""),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_marker_wildcard_is_only_allowed_at_the_start_of_a_segment() {
        let source = format!(
            "{}\ndemo = native_project(target_kind = \"demo_workspace\", docs = \"Demo\", markers = [\"project.*proj\"], on_match = \"stop\")\n",
            crate::modules::common_module_source()
        );
        let error = native_project_schemas_from_source(&source).unwrap_err();
        assert!(
            error.contains("only use `*` at the start"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn built_in_native_projects_detect_xcode_projects_at_the_workspace_root() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temporary.path().join("Browser.xcodeproj")).unwrap();
        std::fs::write(
            temporary.path().join("Browser.xcodeproj/project.pbxproj"),
            "// !$*UTF8*$!\n",
        )
        .unwrap();
        std::fs::create_dir_all(temporary.path().join("ios/Nested.xcodeproj")).unwrap();
        std::fs::write(
            temporary
                .path()
                .join("ios/Nested.xcodeproj/project.pbxproj"),
            "// !$*UTF8*$!\n",
        )
        .unwrap();

        let matches = detect_native_projects(temporary.path()).unwrap();

        let xcode = matches
            .iter()
            .filter(|matched| matched.native_project == "xcode")
            .collect::<Vec<_>>();
        assert_eq!(xcode.len(), 1);
        assert!(xcode[0].package.is_empty());
        assert_eq!(
            xcode[0].markers,
            vec!["Browser.xcodeproj/project.pbxproj".to_string()]
        );
    }

    #[test]
    fn built_in_native_projects_detect_native_markers_without_execution() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("mix.exs"), "raise \"not executed\"\n").unwrap();
        std::fs::create_dir_all(temporary.path().join("rust")).unwrap();
        std::fs::write(
            temporary.path().join("rust/Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temporary.path().join("rust/member")).unwrap();
        std::fs::write(
            temporary.path().join("rust/member/Cargo.toml"),
            "[package]\nname = \"member\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let matches = detect_native_projects(temporary.path()).unwrap();

        assert!(matches
            .iter()
            .any(|matched| matched.native_project == "mix" && matched.package.is_empty()));
        assert!(matches
            .iter()
            .any(|matched| matched.native_project == "cargo" && matched.package == "rust"));
        assert!(!matches
            .iter()
            .any(|matched| matched.native_project == "cargo" && matched.package == "rust/member"));
    }

    #[test]
    fn built_in_native_projects_ignore_generated_dependency_trees() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("mix.exs"), "raise \"not executed\"\n").unwrap();
        std::fs::create_dir_all(temporary.path().join("deps/native")).unwrap();
        std::fs::write(
            temporary.path().join("deps/native/Cargo.toml"),
            "[package]\nname = \"native\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temporary.path().join("target/tool")).unwrap();
        std::fs::write(
            temporary.path().join("target/tool/mix.exs"),
            "raise \"not executed\"\n",
        )
        .unwrap();

        let matches = detect_native_projects(temporary.path()).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].native_project, "mix");
        assert!(matches[0].package.is_empty());
    }
}
