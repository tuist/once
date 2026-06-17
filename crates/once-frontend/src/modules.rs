//! Loading and composing Starlark graph modules.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use crate::error::{Error, Result};
use crate::manifest::load_module_paths_toml_str;
use crate::TOML_BUILD_FILE_NAME;
use include_dir::{include_dir, Dir};
use starlark::environment::{GlobalsBuilder, Module};
use starlark::syntax::{AstModule, Dialect};
use starlark::values::dict::DictRef;
use starlark::values::list::ListRef;
use starlark::values::Value;

pub(crate) const BUILT_IN_MODULE_PATH: &str = "once//prelude/all.star";
pub(crate) const COMBINED_MODULE_PATH: &str = "once//modules/all.star";

const PRELUDE_INDEX_PATH: &str = "index.star";
const COMMON_PRELUDE_PATH: &str = "common.star";
static PRELUDE_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/prelude");
static BUILT_IN_MODULE_SOURCE: LazyLock<String> = LazyLock::new(load_built_in_module_source);

pub(crate) fn built_in_module_source() -> &'static str {
    BUILT_IN_MODULE_SOURCE.as_str()
}

pub(crate) fn common_module_source() -> &'static str {
    prelude_source(COMMON_PRELUDE_PATH)
}

pub(crate) fn combined_module_source_for_workspace(root: &Path) -> Result<String> {
    let module_files = load_module_files(root)?;
    Ok(combine_module_sources(
        built_in_module_source(),
        &module_files,
    ))
}

pub(crate) fn combine_module_sources(built_in: &str, module_files: &[ModuleFile]) -> String {
    let mut source = String::new();
    source.push_str(built_in);
    for module_file in module_files {
        source.push_str("\n# once module file: ");
        source.push_str(&module_file.display_path);
        source.push('\n');
        source.push_str(&module_file.source);
        source.push('\n');
    }
    source
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TargetKindExport<'v> {
    pub name: &'v str,
    pub value: Value<'v>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleFile {
    pub(crate) display_path: String,
    pub(crate) source: String,
}

pub(crate) fn load_module_files(root: &Path) -> Result<Vec<ModuleFile>> {
    let patterns = load_module_path_patterns(root)?;
    let canonical_root = std::fs::canonicalize(root).map_err(|source| Error::Read {
        path: root.display().to_string(),
        source,
    })?;
    let mut files = BTreeMap::new();
    for pattern in patterns {
        validate_module_path_pattern(&pattern)?;
        let glob_pattern = root.join(&pattern);
        let glob_pattern = glob_pattern.to_string_lossy().into_owned();
        let mut matched = false;
        for entry in glob::glob(&glob_pattern).map_err(|source| Error::Eval {
            path: TOML_BUILD_FILE_NAME.to_string(),
            message: format!("invalid module path pattern `{pattern}`: {source}"),
        })? {
            let path = entry.map_err(|source| Error::Eval {
                path: TOML_BUILD_FILE_NAME.to_string(),
                message: format!("failed to resolve module path pattern `{pattern}`: {source}"),
            })?;
            let Some((display, canonical_path)) =
                resolve_module_file(root, &canonical_root, &path)?
            else {
                continue;
            };
            matched = true;
            files.entry(display).or_insert(canonical_path);
        }
        if !matched {
            return Err(Error::Eval {
                path: TOML_BUILD_FILE_NAME.to_string(),
                message: format!("module path pattern `{pattern}` did not match any files"),
            });
        }
    }

    let module_files = files
        .into_iter()
        .map(|(display_path, path)| {
            let source = std::fs::read_to_string(&path).map_err(|source| Error::Read {
                path: display_path.clone(),
                source,
            })?;
            Ok(ModuleFile {
                display_path,
                source,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    for module_file in &module_files {
        validate_module_file_source(module_file)?;
    }
    Ok(module_files)
}

fn resolve_module_file(
    root: &Path,
    canonical_root: &Path,
    path: &Path,
) -> Result<Option<(String, PathBuf)>> {
    if !path.is_file() {
        return Ok(None);
    }
    let display_path = display_module_path(root, path);
    let canonical_path = std::fs::canonicalize(path).map_err(|source| Error::Read {
        path: display_path.clone(),
        source,
    })?;
    if !canonical_path.is_file() {
        return Ok(None);
    }
    if !canonical_path.starts_with(canonical_root) {
        return Err(Error::Eval {
            path: TOML_BUILD_FILE_NAME.to_string(),
            message: format!("module file `{display_path}` resolves outside the project root"),
        });
    }
    Ok(Some((display_path, canonical_path)))
}

fn validate_module_file_source(module_file: &ModuleFile) -> Result<()> {
    AstModule::parse(
        &module_file.display_path,
        module_file.source.clone(),
        &Dialect::Standard,
    )
    .map(|_| ())
    .map_err(|source| Error::Parse {
        path: module_file.display_path.clone(),
        message: format!("{source:?}"),
    })
}

fn load_module_path_patterns(root: &Path) -> Result<Vec<String>> {
    let path = root.join(TOML_BUILD_FILE_NAME);
    let src = match std::fs::read_to_string(&path) {
        Ok(src) => src,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(source) => {
            return Err(Error::Read {
                path: path.display().to_string(),
                source,
            });
        }
    };
    load_module_paths_toml_str(TOML_BUILD_FILE_NAME, &src)
}

fn validate_module_path_pattern(pattern: &str) -> Result<()> {
    if pattern.trim().is_empty() {
        return Err(Error::Eval {
            path: TOML_BUILD_FILE_NAME.to_string(),
            message: "`modules.paths` entries must be non-empty".to_string(),
        });
    }
    let path = Path::new(pattern);
    if path.is_absolute() {
        return Err(Error::Eval {
            path: TOML_BUILD_FILE_NAME.to_string(),
            message: format!("module path `{pattern}` must be relative to the project root"),
        });
    }
    for component in path.components() {
        match component {
            Component::CurDir => {
                return Err(Error::Eval {
                    path: TOML_BUILD_FILE_NAME.to_string(),
                    message: format!("module path `{pattern}` must not contain `.` components"),
                });
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::Eval {
                    path: TOML_BUILD_FILE_NAME.to_string(),
                    message: format!("module path `{pattern}` must stay inside the project root"),
                });
            }
            Component::Normal(name) if name == ".once" => {
                return Err(Error::Eval {
                    path: TOML_BUILD_FILE_NAME.to_string(),
                    message: "module paths under `.once` are reserved for Once state".to_string(),
                });
            }
            Component::Normal(_) => {}
        }
    }
    Ok(())
}

fn display_module_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

pub(crate) fn exported_target_kind_values<'v>(module: &Module<'v>) -> Vec<TargetKindExport<'v>> {
    let mut target_kinds = module
        .names()
        .filter_map(|name| {
            let name = name.as_str();
            if name.starts_with('_') {
                return None;
            }
            let value = module.get(name)?;
            is_target_kind_value(value).then_some(TargetKindExport { name, value })
        })
        .collect::<Vec<_>>();
    target_kinds.sort_unstable_by(|a, b| a.name.cmp(b.name));
    target_kinds
}

pub(crate) fn target_kind(
    value: Value<'_>,
    export_name: &str,
) -> std::result::Result<String, String> {
    let dict = DictRef::from_value(value)
        .ok_or_else(|| format!("target kind export `{export_name}` should be a dict"))?;
    let Some(kind) = dict.get_str("kind") else {
        return Ok(export_name.to_string());
    };
    if kind.is_none() {
        return Ok(export_name.to_string());
    }
    kind.unpack_str().map(ToOwned::to_owned).ok_or_else(|| {
        format!("target kind export `{export_name}` kind should be a string or None")
    })
}

fn is_target_kind_value(value: Value<'_>) -> bool {
    let Some(dict) = DictRef::from_value(value) else {
        return false;
    };
    dict.get_str("_once_target_kind")
        .and_then(Value::unpack_bool)
        .unwrap_or(false)
}

fn load_built_in_module_source() -> String {
    let sources = prelude_sources_from_index();
    let mut source = String::new();
    for path in sources {
        source.push_str(prelude_source(&path));
        source.push('\n');
    }
    source
}

fn prelude_source(path: &str) -> &'static str {
    let file = PRELUDE_DIR
        .get_file(path)
        .unwrap_or_else(|| panic!("built-in prelude source `{path}` is missing"));
    file.contents_utf8()
        .unwrap_or_else(|| panic!("built-in prelude source `{path}` is not UTF-8"))
}

fn prelude_sources_from_index() -> Vec<String> {
    let index = PRELUDE_DIR
        .get_file(PRELUDE_INDEX_PATH)
        .unwrap_or_else(|| panic!("built-in prelude index `{PRELUDE_INDEX_PATH}` is missing"));
    let source = index
        .contents_utf8()
        .unwrap_or_else(|| panic!("built-in prelude index `{PRELUDE_INDEX_PATH}` is not UTF-8"));
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse(PRELUDE_INDEX_PATH, source.to_string(), &Dialect::Standard)
            .unwrap_or_else(|error| panic!("built-in prelude index parse failed: {error:?}"));
        let globals = GlobalsBuilder::standard().build();
        let mut eval = starlark::eval::Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .unwrap_or_else(|error| panic!("built-in prelude index eval failed: {error:?}"));
        let value = module
            .get("PRELUDE_SOURCES")
            .unwrap_or_else(|| panic!("built-in prelude index is missing PRELUDE_SOURCES"));
        let list = ListRef::from_value(value)
            .unwrap_or_else(|| panic!("built-in prelude PRELUDE_SOURCES is not a list"));
        list.iter()
            .map(|value| {
                let path = Value::unpack_str(value).unwrap_or_else(|| {
                    panic!("built-in prelude PRELUDE_SOURCES entries must be strings")
                });
                validate_built_in_prelude_source_path(path);
                path.to_string()
            })
            .collect()
    })
}

fn validate_built_in_prelude_source_path(path: &str) {
    let path_value = Path::new(path);
    assert!(
        !path.trim().is_empty() && !path_value.is_absolute(),
        "built-in prelude source path `{path}` must be relative"
    );
    for component in path_value.components() {
        match component {
            Component::Normal(_) => {}
            _ => panic!("built-in prelude source path `{path}` must stay inside the prelude"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_module_files_are_appended_to_built_in_modules() {
        let custom = ModuleFile {
            display_path: "modules/demo.star".to_string(),
            source: r#"
demo_kind = target_kind(docs = "Demo")
"#
            .to_string(),
        };

        let source = combine_module_sources(
            "built_in_kind = target_kind(docs = \"Built in\")\n",
            &[custom],
        );

        assert!(source.contains("built_in_kind = target_kind"));
        assert!(source.contains("# once module file: modules/demo.star"));
        assert!(source.contains("demo_kind = target_kind"));
        assert!(!source.contains("_ONCE_BUILT_IN_TARGET_KINDS"));
    }

    #[test]
    fn rejects_dot_once_module_paths() {
        let err = validate_module_path_pattern(".once/modules/*.star").unwrap_err();
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn rejects_nested_dot_once_module_paths() {
        let err = validate_module_path_pattern("build/.once/*.star").unwrap_err();
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn rejects_current_dir_module_paths() {
        let err = validate_module_path_pattern("./modules/*.star").unwrap_err();
        assert!(err.to_string().contains("must not contain `.`"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_module_file_symlinks_outside_workspace() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("escape.star"), "demo_kind = None\n").unwrap();
        std::fs::write(
            workspace.path().join(TOML_BUILD_FILE_NAME),
            "[modules]\npaths = [\"modules/*.star\"]\n",
        )
        .unwrap();
        std::fs::create_dir(workspace.path().join("modules")).unwrap();
        symlink(
            outside.path().join("escape.star"),
            workspace.path().join("modules/escape.star"),
        )
        .unwrap();

        let err = load_module_files(workspace.path()).unwrap_err();

        assert!(err.to_string().contains("outside the project root"));
    }

    #[test]
    fn module_file_parse_errors_use_module_file_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(TOML_BUILD_FILE_NAME),
            "[modules]\npaths = [\"modules/*.star\"]\n",
        )
        .unwrap();
        std::fs::create_dir(tmp.path().join("modules")).unwrap();
        std::fs::write(
            tmp.path().join("modules/bad.star"),
            "demo_kind = target_kind(\n",
        )
        .unwrap();

        let err = combined_module_source_for_workspace(tmp.path()).unwrap_err();

        assert!(err.to_string().contains("modules/bad.star"));
    }

    #[test]
    fn loads_module_files_from_root_modules_table() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(TOML_BUILD_FILE_NAME),
            "[modules]\npaths = [\"modules/*.star\"]\n",
        )
        .unwrap();
        std::fs::create_dir(tmp.path().join("modules")).unwrap();
        std::fs::write(tmp.path().join("modules/demo.star"), "demo_kind = None\n").unwrap();

        let files = load_module_files(tmp.path()).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].display_path, "modules/demo.star");
    }
}
