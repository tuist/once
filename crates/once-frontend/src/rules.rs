//! Loading and composing Starlark rule sources.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use crate::error::{Error, Result};
use crate::manifest::load_rule_paths_toml_str;
use crate::TOML_BUILD_FILE_NAME;
use include_dir::{include_dir, Dir};
use starlark::environment::{GlobalsBuilder, Module};
use starlark::syntax::{AstModule, Dialect};
use starlark::values::dict::DictRef;
use starlark::values::list::ListRef;
use starlark::values::Value;

pub(crate) const BUILT_IN_RULE_PATH: &str = "once//prelude/all.star";
pub(crate) const COMBINED_RULE_PATH: &str = "once//rules/all.star";

const PRELUDE_INDEX_PATH: &str = "index.star";
const COMMON_PRELUDE_PATH: &str = "common.star";
static PRELUDE_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/prelude");
static BUILT_IN_RULE_SOURCE: LazyLock<String> = LazyLock::new(load_built_in_rule_source);

pub(crate) fn built_in_rule_source() -> &'static str {
    BUILT_IN_RULE_SOURCE.as_str()
}

pub(crate) fn common_rule_source() -> &'static str {
    prelude_source(COMMON_PRELUDE_PATH)
}

pub(crate) fn combined_rule_source_for_workspace(root: &Path) -> Result<String> {
    let rule_files = load_rule_files(root)?;
    Ok(combine_rule_sources(built_in_rule_source(), &rule_files))
}

pub(crate) fn combine_rule_sources(built_in: &str, rule_files: &[RuleFile]) -> String {
    let mut source = String::new();
    source.push_str(built_in);
    for rule_file in rule_files {
        source.push_str("\n# once rule file: ");
        source.push_str(&rule_file.display_path);
        source.push('\n');
        source.push_str(&rule_file.source);
        source.push('\n');
    }
    source
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RuleExport<'v> {
    pub name: &'v str,
    pub value: Value<'v>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuleFile {
    pub(crate) display_path: String,
    pub(crate) source: String,
}

pub(crate) fn load_rule_files(root: &Path) -> Result<Vec<RuleFile>> {
    let patterns = load_rule_path_patterns(root)?;
    let canonical_root = std::fs::canonicalize(root).map_err(|source| Error::Read {
        path: root.display().to_string(),
        source,
    })?;
    let mut files = BTreeMap::new();
    for pattern in patterns {
        validate_rule_path_pattern(&pattern)?;
        let glob_pattern = root.join(&pattern);
        let glob_pattern = glob_pattern.to_string_lossy().into_owned();
        let mut matched = false;
        for entry in glob::glob(&glob_pattern).map_err(|source| Error::Eval {
            path: TOML_BUILD_FILE_NAME.to_string(),
            message: format!("invalid rule path pattern `{pattern}`: {source}"),
        })? {
            let path = entry.map_err(|source| Error::Eval {
                path: TOML_BUILD_FILE_NAME.to_string(),
                message: format!("failed to resolve rule path pattern `{pattern}`: {source}"),
            })?;
            let Some((display, canonical_path)) = resolve_rule_file(root, &canonical_root, &path)?
            else {
                continue;
            };
            matched = true;
            files.entry(display).or_insert(canonical_path);
        }
        if !matched {
            return Err(Error::Eval {
                path: TOML_BUILD_FILE_NAME.to_string(),
                message: format!("rule path pattern `{pattern}` did not match any files"),
            });
        }
    }

    let rule_files = files
        .into_iter()
        .map(|(display_path, path)| {
            let source = std::fs::read_to_string(&path).map_err(|source| Error::Read {
                path: display_path.clone(),
                source,
            })?;
            Ok(RuleFile {
                display_path,
                source,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    for rule_file in &rule_files {
        validate_rule_file_source(rule_file)?;
    }
    Ok(rule_files)
}

fn resolve_rule_file(
    root: &Path,
    canonical_root: &Path,
    path: &Path,
) -> Result<Option<(String, PathBuf)>> {
    if !path.is_file() {
        return Ok(None);
    }
    let display_path = display_rule_path(root, path);
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
            message: format!("rule file `{display_path}` resolves outside the project root"),
        });
    }
    Ok(Some((display_path, canonical_path)))
}

fn validate_rule_file_source(rule_file: &RuleFile) -> Result<()> {
    AstModule::parse(
        &rule_file.display_path,
        rule_file.source.clone(),
        &Dialect::Standard,
    )
    .map(|_| ())
    .map_err(|source| Error::Parse {
        path: rule_file.display_path.clone(),
        message: format!("{source:?}"),
    })
}

fn load_rule_path_patterns(root: &Path) -> Result<Vec<String>> {
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
    load_rule_paths_toml_str(TOML_BUILD_FILE_NAME, &src)
}

fn validate_rule_path_pattern(pattern: &str) -> Result<()> {
    if pattern.trim().is_empty() {
        return Err(Error::Eval {
            path: TOML_BUILD_FILE_NAME.to_string(),
            message: "`rules.paths` entries must be non-empty".to_string(),
        });
    }
    let path = Path::new(pattern);
    if path.is_absolute() {
        return Err(Error::Eval {
            path: TOML_BUILD_FILE_NAME.to_string(),
            message: format!("rule path `{pattern}` must be relative to the project root"),
        });
    }
    for component in path.components() {
        match component {
            Component::CurDir => {
                return Err(Error::Eval {
                    path: TOML_BUILD_FILE_NAME.to_string(),
                    message: format!("rule path `{pattern}` must not contain `.` components"),
                });
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::Eval {
                    path: TOML_BUILD_FILE_NAME.to_string(),
                    message: format!("rule path `{pattern}` must stay inside the project root"),
                });
            }
            Component::Normal(name) if name == ".once" => {
                return Err(Error::Eval {
                    path: TOML_BUILD_FILE_NAME.to_string(),
                    message: "rule paths under `.once` are reserved for Once state".to_string(),
                });
            }
            Component::Normal(_) => {}
        }
    }
    Ok(())
}

fn display_rule_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

pub(crate) fn exported_rule_values<'v>(module: &Module<'v>) -> Vec<RuleExport<'v>> {
    let mut rules = module
        .names()
        .filter_map(|name| {
            let name = name.as_str();
            if name.starts_with('_') {
                return None;
            }
            let value = module.get(name)?;
            is_rule_value(value).then_some(RuleExport { name, value })
        })
        .collect::<Vec<_>>();
    rules.sort_unstable_by(|a, b| a.name.cmp(b.name));
    rules
}

pub(crate) fn rule_kind(
    value: Value<'_>,
    export_name: &str,
) -> std::result::Result<String, String> {
    let dict = DictRef::from_value(value)
        .ok_or_else(|| format!("rule export `{export_name}` should be a dict"))?;
    let Some(kind) = dict.get_str("kind") else {
        return Ok(export_name.to_string());
    };
    if kind.is_none() {
        return Ok(export_name.to_string());
    }
    kind.unpack_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("rule export `{export_name}` kind should be a string or None"))
}

fn is_rule_value(value: Value<'_>) -> bool {
    let Some(dict) = DictRef::from_value(value) else {
        return false;
    };
    dict.get_str("_once_rule")
        .and_then(Value::unpack_bool)
        .unwrap_or(false)
}

fn load_built_in_rule_source() -> String {
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
    fn custom_rule_files_are_appended_to_built_in_rules() {
        let custom = RuleFile {
            display_path: "rules/demo.star".to_string(),
            source: r#"
demo_rule = rule(docs = "Demo")
"#
            .to_string(),
        };

        let source = combine_rule_sources("built_in_rule = rule(docs = \"Built in\")\n", &[custom]);

        assert!(source.contains("built_in_rule = rule"));
        assert!(source.contains("# once rule file: rules/demo.star"));
        assert!(source.contains("demo_rule = rule"));
        assert!(!source.contains("_ONCE_BUILT_IN_RULES"));
    }

    #[test]
    fn rejects_dot_once_rule_paths() {
        let err = validate_rule_path_pattern(".once/rules/*.star").unwrap_err();
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn rejects_nested_dot_once_rule_paths() {
        let err = validate_rule_path_pattern("build/.once/*.star").unwrap_err();
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn rejects_current_dir_rule_paths() {
        let err = validate_rule_path_pattern("./rules/*.star").unwrap_err();
        assert!(err.to_string().contains("must not contain `.`"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_rule_file_symlinks_outside_workspace() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("escape.star"), "demo_rule = None\n").unwrap();
        std::fs::write(
            workspace.path().join(TOML_BUILD_FILE_NAME),
            "[rules]\npaths = [\"rules/*.star\"]\n",
        )
        .unwrap();
        std::fs::create_dir(workspace.path().join("rules")).unwrap();
        symlink(
            outside.path().join("escape.star"),
            workspace.path().join("rules/escape.star"),
        )
        .unwrap();

        let err = load_rule_files(workspace.path()).unwrap_err();

        assert!(err.to_string().contains("outside the project root"));
    }

    #[test]
    fn rule_file_parse_errors_use_rule_file_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(TOML_BUILD_FILE_NAME),
            "[rules]\npaths = [\"rules/*.star\"]\n",
        )
        .unwrap();
        std::fs::create_dir(tmp.path().join("rules")).unwrap();
        std::fs::write(tmp.path().join("rules/bad.star"), "demo_rule = rule(\n").unwrap();

        let err = combined_rule_source_for_workspace(tmp.path()).unwrap_err();

        assert!(err.to_string().contains("rules/bad.star"));
    }

    #[test]
    fn loads_rule_files_from_root_rules_table() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(TOML_BUILD_FILE_NAME),
            "[rules]\npaths = [\"rules/*.star\"]\n",
        )
        .unwrap();
        std::fs::create_dir(tmp.path().join("rules")).unwrap();
        std::fs::write(tmp.path().join("rules/demo.star"), "demo_rule = None\n").unwrap();

        let files = load_rule_files(tmp.path()).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].display_path, "rules/demo.star");
    }
}
