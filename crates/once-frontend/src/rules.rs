//! Loading and composing Starlark rule sources.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use crate::error::{Error, Result};
use crate::manifest::load_rule_paths_toml_str;
use crate::TOML_BUILD_FILE_NAME;

pub(crate) const BUILT_IN_RULE_PATH: &str = "once//prelude/apple.star";
pub(crate) const COMBINED_RULE_PATH: &str = "once//rules/all.star";

pub(crate) fn built_in_rule_source() -> &'static str {
    include_str!("../prelude/apple.star")
}

pub(crate) fn combined_rule_source_for_workspace(root: &Path) -> Result<String> {
    let rule_files = load_rule_files(root)?;
    Ok(combine_rule_sources(built_in_rule_source(), &rule_files))
}

pub(crate) fn combine_rule_sources(built_in: &str, rule_files: &[RuleFile]) -> String {
    if rule_files.is_empty() {
        return built_in.to_string();
    }

    let mut source = String::new();
    source.push_str(built_in);
    source.push_str("\n_ONCE_BUILT_IN_RULES = RULES\n");
    source.push_str(
        r#"
def _once_capture_rules(path, rules):
    if rules == None:
        fail("rule file `" + path + "` must assign RULES")
    return rules
"#,
    );
    let mut custom_names = Vec::with_capacity(rule_files.len());
    for (index, rule_file) in rule_files.iter().enumerate() {
        let name = format!("_ONCE_CUSTOM_RULES_{index}");
        custom_names.push(name.clone());
        source.push_str("\n# once rule file: ");
        source.push_str(&rule_file.display_path);
        source.push_str("\nRULES = None\n");
        source.push_str(&rule_file.source);
        source.push('\n');
        source.push_str(&name);
        source.push_str(" = _once_capture_rules(");
        source.push_str(&starlark_string_literal(&rule_file.display_path));
        source.push_str(", RULES)\n");
    }
    source.push_str("RULES = _ONCE_BUILT_IN_RULES");
    for name in custom_names {
        source.push_str(" + ");
        source.push_str(&name);
    }
    source.push('\n');
    source
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuleFile {
    display_path: String,
    source: String,
}

fn load_rule_files(root: &Path) -> Result<Vec<RuleFile>> {
    let patterns = load_rule_path_patterns(root)?;
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
            if !path.is_file() {
                continue;
            }
            matched = true;
            let display = display_rule_path(root, &path);
            files.entry(display).or_insert(path);
        }
        if !matched {
            return Err(Error::Eval {
                path: TOML_BUILD_FILE_NAME.to_string(),
                message: format!("rule path pattern `{pattern}` did not match any files"),
            });
        }
    }

    files
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
        .collect()
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
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(Error::Eval {
            path: TOML_BUILD_FILE_NAME.to_string(),
            message: format!("rule path `{pattern}` must stay inside the project root"),
        });
    }
    if path
        .components()
        .next()
        .is_some_and(|component| component.as_os_str() == ".once")
    {
        return Err(Error::Eval {
            path: TOML_BUILD_FILE_NAME.to_string(),
            message: "rule paths under `.once` are reserved for Once state".to_string(),
        });
    }
    Ok(())
}

fn display_rule_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn starlark_string_literal(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_rule_files_extend_built_in_rules() {
        let custom = RuleFile {
            display_path: "rules/demo.star".to_string(),
            source: r#"
RULES = [
    rule(
        kind = "demo_rule",
        docs = "Demo",
        attrs = [],
        deps = [],
        providers = [],
        capabilities = [],
    ),
]
"#
            .to_string(),
        };

        let source = combine_rule_sources("RULES = [\"built_in\"]\n", &[custom]);

        assert!(source.contains("_ONCE_BUILT_IN_RULES"));
        assert!(source.contains("_ONCE_CUSTOM_RULES_0"));
        assert!(source.contains("RULES = _ONCE_BUILT_IN_RULES + _ONCE_CUSTOM_RULES_0"));
    }

    #[test]
    fn rejects_dot_once_rule_paths() {
        let err = validate_rule_path_pattern(".once/rules/*.star").unwrap_err();
        assert!(err.to_string().contains("reserved"));
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
        std::fs::write(tmp.path().join("rules/demo.star"), "RULES = []\n").unwrap();

        let files = load_rule_files(tmp.path()).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].display_path, "rules/demo.star");
    }
}
