use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::dict::DictRef;
use starlark::values::list::ListRef;
use walkdir::WalkDir;

use crate::error::{Error, Result};
use crate::graph::GraphTarget;
use crate::target::{AttrValue, Target};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NativeProjectSchema {
    pub name: String,
    pub docs: String,
    pub markers: Vec<String>,
    pub target_name: String,
    pub target_kind: String,
    pub inputs: Vec<String>,
    pub exclude: Vec<String>,
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

pub fn native_project_schemas_for_workspace(root: &Path) -> Result<Vec<NativeProjectSchema>> {
    let source = crate::modules::combined_module_source_for_workspace(root)?;
    let native_projects =
        parse_native_project_schemas(crate::modules::COMBINED_MODULE_PATH, &source)?;
    let target_kinds = crate::graph::target_kind_schemas_for_workspace(root)?
        .into_iter()
        .map(|schema| schema.kind)
        .collect::<BTreeSet<_>>();
    for native_project in &native_projects {
        if !target_kinds.contains(&native_project.target_kind) {
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

pub fn detect_native_projects(root: &Path) -> Result<Vec<NativeProjectMatch>> {
    let schemas = native_project_schemas_for_workspace(root)?;
    let mut matches = Vec::new();
    for schema in schemas {
        let mut candidates = native_project_candidates(root, &schema)?;
        candidates.sort();
        for package_path in candidates {
            let package = display_relative(root, &package_path);
            let seed_target = crate::target_ref::target_id(&package, &schema.target_name);
            matches.push(NativeProjectMatch {
                native_project: schema.name.clone(),
                package,
                markers: schema.markers.clone(),
                seed_target,
            });
        }
    }
    matches.sort_by(|left, right| {
        left.package
            .cmp(&right.package)
            .then(left.native_project.cmp(&right.native_project))
    });
    Ok(matches)
}

pub(crate) fn synthesized_workspace_seeds(
    root: &Path,
) -> Result<Vec<(NativeProjectMatch, Target)>> {
    let schemas = native_project_schemas_for_workspace(root)?;
    let schemas = schemas
        .into_iter()
        .map(|schema| (schema.name.clone(), schema))
        .collect::<BTreeMap<_, _>>();
    let mut targets = Vec::new();
    let mut ids = BTreeMap::<String, String>::new();
    for matched in detect_native_projects(root)? {
        let schema = schemas
            .get(&matched.native_project)
            .ok_or_else(|| Error::Eval {
                path: crate::modules::COMBINED_MODULE_PATH.to_string(),
                message: format!(
                    "native project `{}` disappeared during discovery",
                    matched.native_project
                ),
            })?;
        let target = seed_target(schema, &matched.package);
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
    let schema = native_project_schemas_for_workspace(root)?
        .into_iter()
        .find(|schema| schema.name == name)
        .ok_or_else(|| Error::Eval {
            path: crate::modules::COMBINED_MODULE_PATH.to_string(),
            message: format!("unknown native project `{name}`"),
        })?;
    let matched = detect_native_projects(root)?
        .into_iter()
        .any(|matched| matched.native_project == name && matched.package == package);
    if !matched {
        return Err(Error::Eval {
            path: name.to_string(),
            message: format!(
                "native project `{name}` does not match package `{}`",
                if package.is_empty() { "." } else { package }
            ),
        });
    }
    Ok(seed_target(&schema, package))
}

pub fn preview_native_project(
    root: &Path,
    name: &str,
    package: &str,
) -> Result<NativeProjectPreview> {
    let schema = native_project_schemas_for_workspace(root)?
        .into_iter()
        .find(|schema| schema.name == name)
        .ok_or_else(|| Error::Eval {
            path: crate::modules::COMBINED_MODULE_PATH.to_string(),
            message: format!("unknown native project `{name}`"),
        })?;
    let matched = detect_native_projects(root)?
        .into_iter()
        .find(|matched| matched.native_project == name && matched.package == package)
        .ok_or_else(|| Error::Eval {
            path: name.to_string(),
            message: format!(
                "native project `{name}` does not match package `{}`",
                if package.is_empty() { "." } else { package }
            ),
        })?;
    let seed = seed_target(&schema, package);
    let schemas = crate::graph::target_kind_schemas_for_workspace(root)?;
    let targets = crate::graph::load_graph_workspace_with_targets_and_schemas(
        root,
        vec![seed.clone()],
        &schemas,
    )?;
    Ok(NativeProjectPreview {
        native_project: schema,
        matched,
        seed,
        targets,
    })
}

fn seed_target(schema: &NativeProjectSchema, package: &str) -> Target {
    let resolver_inputs = schema
        .markers
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
        srcs: schema.markers.clone(),
        visibility: Vec::new(),
        attrs: BTreeMap::new(),
        typed_attrs: BTreeMap::from([(
            "resolver_inputs".to_string(),
            AttrValue::List(resolver_inputs),
        )]),
    }
}

fn parse_native_project_schemas(path: &str, source: &str) -> Result<Vec<NativeProjectSchema>> {
    Module::with_temp_heap(|module| {
        let ast =
            AstModule::parse(path, source.to_string(), &Dialect::Standard).map_err(|error| {
                Error::Parse {
                    path: path.to_string(),
                    message: format!("{error:?}"),
                }
            })?;
        let globals = crate::analysis::globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|error| Error::Eval {
                path: path.to_string(),
                message: format!("{error:?}"),
            })?;
        let exports = crate::modules::exported_native_project_values(&module);
        let mut names = BTreeSet::new();
        exports
            .into_iter()
            .map(|export| {
                let dict = DictRef::from_value(export.value).ok_or_else(|| Error::Eval {
                    path: path.to_string(),
                    message: format!("native project export `{}` should be a dict", export.name),
                })?;
                let name =
                    optional_string(&dict, "name")?.unwrap_or_else(|| export.name.to_string());
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
                let markers = string_list(&dict, "markers")?;
                if markers.is_empty() {
                    return Err(native_project_error(
                        &name,
                        format!("native project `{name}` must declare markers"),
                    ));
                }
                let target_kind = required_string(&dict, "target_kind")?;
                if target_kind.is_empty() {
                    return Err(native_project_error(
                        &name,
                        format!("native project `{name}` has an empty target kind"),
                    ));
                }
                let target_name =
                    optional_string(&dict, "target_name")?.unwrap_or_else(|| name.clone());
                crate::target_ref::validate_target_name(&target_name)
                    .map_err(|source| native_project_error(&name, source.to_string()))?;
                let inputs = string_list(&dict, "inputs")?;
                let exclude = string_list(&dict, "exclude")?;
                let on_match = required_string(&dict, "on_match")?;
                if on_match != "stop" && on_match != "descend" {
                    return Err(native_project_error(
                        &name,
                        format!("native project `{name}` on_match must be `stop` or `descend`"),
                    ));
                }
                let max_depth =
                    positive_usize(required_i32(&dict, "max_depth")?, &name, "max_depth")?;
                let requires_tools = string_list(&dict, "requires_tools")?;
                for marker in &markers {
                    validate_relative_literal(&name, "marker", marker)?;
                }
                for excluded in &exclude {
                    validate_relative_literal(&name, "excluded directory", excluded)?;
                }
                for input in &inputs {
                    validate_source_pattern(&name, input)?;
                }
                Ok(NativeProjectSchema {
                    name,
                    docs: required_string(&dict, "docs")?,
                    markers,
                    target_name,
                    target_kind,
                    inputs,
                    exclude,
                    on_match,
                    max_depth,
                    requires_tools,
                })
            })
            .collect()
    })
}

fn native_project_candidates(root: &Path, schema: &NativeProjectSchema) -> Result<Vec<PathBuf>> {
    let walker = WalkDir::new(root)
        .max_depth(schema.max_depth)
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
            !entry.file_type().is_dir() || !schema.exclude.iter().any(|excluded| excluded == name)
        });
    let primary = &schema.markers[0];
    let mut candidates = Vec::new();
    for entry in walker {
        let entry = entry.map_err(|source| Error::Walk {
            root: root.display().to_string(),
            source,
        })?;
        if !entry.file_type().is_file() || entry.file_name() != primary.as_str() {
            continue;
        }
        let package = entry.path().parent().unwrap_or(root);
        if schema
            .markers
            .iter()
            .all(|marker| package.join(marker).is_file())
        {
            candidates.push(package.to_path_buf());
        }
    }
    if schema.on_match == "stop" {
        candidates.sort_by(|left, right| {
            left.components()
                .count()
                .cmp(&right.components().count())
                .then(left.cmp(right))
        });
        let mut roots = Vec::<PathBuf>::new();
        for candidate in candidates {
            if roots.iter().any(|root| candidate.starts_with(root)) {
                continue;
            }
            roots.push(candidate);
        }
        return Ok(roots);
    }
    Ok(candidates)
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

    #[test]
    fn parses_and_detects_a_native_project() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("native.project"), "project").unwrap();
        let source = format!(
            "{}\ndemo = native_project(target_kind = \"demo_workspace\", docs = \"Demo\", markers = [\"native.project\"])\n",
            crate::modules::common_module_source()
        );
        let schemas = parse_native_project_schemas("demo.star", &source).unwrap();
        assert_eq!(schemas[0].name, "demo");
        assert_eq!(schemas[0].target_kind, "demo_workspace");

        let candidates = native_project_candidates(temporary.path(), &schemas[0]).unwrap();
        assert_eq!(candidates, vec![temporary.path().to_path_buf()]);
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
}
