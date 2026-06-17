//! TOML frontend for workspace configuration.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::cache_provider::{CacheProviderToml, InfrastructureToml};
use crate::error::{Error, Result};
use crate::target::{AttrValue, Target};
use crate::target_ref::{normalize_manifest_target, validate_target_name};

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Manifest {
    workspace: WorkspaceToml,
    infrastructure: InfrastructureToml,
    cache_provider: Option<CacheProviderToml>,
    modules: Option<ModulesToml>,
    rules: Option<ModulesToml>,
    target: Vec<TargetToml>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct WorkspaceToml {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ModulesToml {
    pub(crate) paths: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TargetToml {
    name: String,
    kind: String,
    deps: Option<toml::Value>,
    srcs: Vec<String>,
    attrs: BTreeMap<String, toml::Value>,
}

pub fn load_toml_str(path: &str, src: &str) -> Result<Vec<Target>> {
    load_toml_with(path, src, Path::new("."), "")
}

pub(crate) fn load_toml_with(
    display_name: &str,
    src: &str,
    _workspace_root: &Path,
    package: &str,
) -> Result<Vec<Target>> {
    let manifest: Manifest = toml::from_str(src).map_err(|source| Error::Parse {
        path: display_name.to_string(),
        message: source.to_string(),
    })?;
    if (manifest.modules.is_some() || manifest.rules.is_some()) && !package.is_empty() {
        return Err(Error::Eval {
            path: display_name.to_string(),
            message: "module paths are only loaded from the root once.toml".to_string(),
        });
    }
    manifest
        .target
        .into_iter()
        .map(|target| target.into_target(display_name, package))
        .collect()
}

pub fn load_cache_provider_toml_str(
    path: &str,
    src: &str,
) -> Result<Option<crate::cache_provider::CacheProviderConfig>> {
    let manifest: Manifest = toml::from_str(src).map_err(|source| Error::Parse {
        path: path.to_string(),
        message: source.to_string(),
    })?;
    if let Some(raw) = manifest.infrastructure.cache {
        return raw.into_config(path).map(Some);
    }
    manifest
        .cache_provider
        .map(|raw| raw.into_config(path))
        .transpose()
}

pub(crate) fn load_module_paths_toml_str(path: &str, src: &str) -> Result<Vec<String>> {
    let manifest: Manifest = toml::from_str(src).map_err(|source| Error::Parse {
        path: path.to_string(),
        message: source.to_string(),
    })?;
    match (manifest.modules, manifest.rules) {
        (Some(_), Some(_)) => Err(Error::Eval {
            path: path.to_string(),
            message: "use either [modules] or [rules], not both".to_string(),
        }),
        (Some(modules), None) | (None, Some(modules)) => Ok(modules.paths),
        (None, None) => Ok(Vec::new()),
    }
}

pub(crate) fn load_workspace_toml_str(path: &str, src: &str) -> Result<WorkspaceToml> {
    let manifest: Manifest = toml::from_str(src).map_err(|source| Error::Parse {
        path: path.to_string(),
        message: source.to_string(),
    })?;
    Ok(manifest.workspace)
}

impl TargetToml {
    fn into_target(self, display_name: &str, package: &str) -> Result<Target> {
        if self.name.is_empty() {
            return Err(Error::Eval {
                path: display_name.to_string(),
                message: "target name is required".to_string(),
            });
        }
        if self.kind.is_empty() {
            return Err(Error::Eval {
                path: display_name.to_string(),
                message: format!("target `{}` kind is required", self.name),
            });
        }
        validate_target_name(&self.name).map_err(|source| Error::Eval {
            path: display_name.to_string(),
            message: source.to_string(),
        })?;
        let deps = deps_from_toml(display_name, &self.name, package, self.deps.as_ref())?;
        let typed_attrs = self
            .attrs
            .into_iter()
            .map(|(key, value)| {
                let string_value = toml_value_to_attr_string(&value);
                (key, string_value, toml_value_to_attr_value(value))
            })
            .collect::<Vec<_>>();
        let attrs = typed_attrs
            .iter()
            .map(|(key, value, _)| (key.clone(), value.clone()))
            .collect();
        let typed_attrs = typed_attrs
            .into_iter()
            .map(|(key, _, value)| (key, value))
            .collect();
        Ok(Target {
            package: package.to_string(),
            kind: self.kind,
            name: self.name,
            deps,
            srcs: self.srcs,
            attrs,
            typed_attrs,
        })
    }
}

fn deps_from_toml(
    display_name: &str,
    target_name: &str,
    package: &str,
    value: Option<&toml::Value>,
) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let selected = select_dep_value(display_name, target_name, value)?;
    let toml::Value::Array(deps) = selected else {
        return Err(Error::Eval {
            path: display_name.to_string(),
            message: format!("target `{target_name}` deps must be an array or select table"),
        });
    };
    deps.iter()
        .map(|dep| {
            let Some(dep) = dep.as_str() else {
                return Err(Error::Eval {
                    path: display_name.to_string(),
                    message: format!("target `{target_name}` deps entries must be strings"),
                });
            };
            normalize_manifest_target(package, dep).map_err(|source| Error::Eval {
                path: display_name.to_string(),
                message: source.to_string(),
            })
        })
        .collect()
}

fn select_dep_value<'a>(
    display_name: &str,
    target_name: &str,
    value: &'a toml::Value,
) -> Result<&'a toml::Value> {
    let tokens = host_select_tokens();
    select_dep_value_for_tokens(display_name, target_name, value, &tokens)
}

fn select_dep_value_for_tokens<'a>(
    display_name: &str,
    target_name: &str,
    value: &'a toml::Value,
    tokens: &[&str],
) -> Result<&'a toml::Value> {
    let toml::Value::Table(table) = value else {
        return Ok(value);
    };
    if table.len() != 1 || !table.contains_key("select") {
        return Ok(value);
    }
    let Some(toml::Value::Table(branches)) = table.get("select") else {
        return Err(Error::Eval {
            path: display_name.to_string(),
            message: format!("target `{target_name}` deps select must be a table"),
        });
    };
    for token in tokens {
        if let Some(value) = branches.get(*token) {
            return Ok(value);
        }
    }
    branches.get("default").ok_or_else(|| Error::Eval {
        path: display_name.to_string(),
        message: format!(
            "target `{target_name}` deps select has no matching branch and no default"
        ),
    })
}

fn host_select_tokens() -> Vec<&'static str> {
    select_tokens_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn select_tokens_for(os: &str, arch: &str) -> Vec<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") => vec!["macos-arm64", "macos-aarch64", "macos", "arm64", "aarch64"],
        ("macos", "x86_64") => vec!["macos-x86_64", "macos", "x86_64"],
        ("linux", "aarch64") => vec!["linux-arm64", "linux-aarch64", "linux", "arm64", "aarch64"],
        ("linux", "x86_64") => vec!["linux-x86_64", "linux", "x86_64"],
        ("windows", "aarch64") => vec![
            "windows-arm64",
            "windows-aarch64",
            "windows",
            "arm64",
            "aarch64",
        ],
        ("windows", "x86_64") => vec!["windows-x86_64", "windows", "x86_64"],
        ("macos", _) => vec!["macos"],
        ("linux", _) => vec!["linux"],
        ("windows", _) => vec!["windows"],
        _ => Vec::new(),
    }
}

fn toml_value_to_attr_string(value: &toml::Value) -> String {
    match value {
        toml::Value::String(value) => value.clone(),
        other => other.to_string(),
    }
}

fn toml_value_to_attr_value(value: toml::Value) -> AttrValue {
    match value {
        toml::Value::String(value) => AttrValue::String(value),
        toml::Value::Integer(value) => AttrValue::Integer(value),
        toml::Value::Float(value) => AttrValue::Float(value),
        toml::Value::Boolean(value) => AttrValue::Bool(value),
        toml::Value::Array(values) => {
            AttrValue::List(values.into_iter().map(toml_value_to_attr_value).collect())
        }
        toml::Value::Table(values) => AttrValue::Map(
            values
                .into_iter()
                .map(|(key, value)| (key, toml_value_to_attr_value(value)))
                .collect(),
        ),
        toml::Value::Datetime(value) => AttrValue::String(value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_config_does_not_declare_targets() {
        let src = r#"
[workspace]
include = ["crates/*/once.toml"]
exclude = ["fixtures/**"]
"#;
        let targets = load_toml_str("once.toml", src).unwrap();
        assert!(targets.is_empty());
    }

    #[test]
    fn loads_root_module_paths() {
        let paths = load_module_paths_toml_str(
            "once.toml",
            r#"
[modules]
paths = ["modules/*.star"]
"#,
        )
        .unwrap();

        assert_eq!(paths, vec!["modules/*.star"]);
    }

    #[test]
    fn loads_legacy_root_rule_paths() {
        let paths = load_module_paths_toml_str(
            "once.toml",
            r#"
[rules]
paths = ["rules/*.star"]
"#,
        )
        .unwrap();

        assert_eq!(paths, vec!["rules/*.star"]);
    }

    #[test]
    fn rejects_root_module_and_rule_paths_together() {
        let err = load_module_paths_toml_str(
            "once.toml",
            r#"
[modules]
paths = ["modules/*.star"]

[rules]
paths = ["rules/*.star"]
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("not both"));
    }

    #[test]
    fn rejects_package_module_paths() {
        let err = load_toml_with(
            "apps/once.toml",
            r#"
[modules]
paths = ["modules/*.star"]
"#,
            Path::new("."),
            "apps",
        )
        .unwrap_err();

        assert!(err.to_string().contains("root once.toml"));
    }

    #[test]
    fn rejects_package_legacy_rule_paths() {
        let err = load_toml_with(
            "apps/once.toml",
            r#"
[rules]
paths = ["rules/*.star"]
"#,
            Path::new("."),
            "apps",
        )
        .unwrap_err();

        assert!(err.to_string().contains("root once.toml"));
    }

    #[test]
    fn loads_workspace_scan_config() {
        let src = r#"
[workspace]
include = ["crates/*/once.toml"]
exclude = ["fixtures/**"]
"#;
        let workspace = load_workspace_toml_str("once.toml", src).unwrap();
        assert_eq!(workspace.include, vec!["crates/*/once.toml"]);
        assert_eq!(workspace.exclude, vec!["fixtures/**"]);
    }

    #[test]
    fn rejects_script_declarations() {
        let src = r#"
[[script]]
name = "hello"
argv = ["sh", "-c", "printf hello"]
"#;
        let err = load_toml_str("once.toml", src).unwrap_err().to_string();
        assert!(err.contains("unknown field `script`"));
    }

    #[test]
    fn loads_apple_target_declarations() {
        let src = r#"
[[target]]
name = "App"
kind = "apple_application"
deps = ["apps/ios/AppKit"]
srcs = ["Sources/**/*.swift"]

[target.attrs]
platform = "ios"
bundle_id = "dev.once.App"
minimum_os = "17.0"
resources = ["Resources/**"]
"#;
        let targets =
            load_toml_with("apps/ios/once.toml", src, Path::new("."), "apps/ios").unwrap();

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id(), "apps/ios/App");
        assert_eq!(targets[0].deps, vec!["apps/ios/AppKit"]);
        assert_eq!(
            targets[0].attrs.get("bundle_id").map(String::as_str),
            Some("dev.once.App")
        );
        assert_eq!(
            targets[0].attrs.get("resources").map(String::as_str),
            Some("[\"Resources/**\"]")
        );
        assert_eq!(
            targets[0].typed_attrs.get("resources"),
            Some(&AttrValue::List(vec![AttrValue::String(
                "Resources/**".to_string()
            )]))
        );
    }

    #[test]
    fn dot_deps_are_package_relative() {
        let src = r#"
[[target]]
name = "App"
kind = "apple_application"
deps = ["./AppKit"]
"#;
        let targets =
            load_toml_with("apps/ios/once.toml", src, Path::new("."), "apps/ios").unwrap();
        assert_eq!(targets[0].deps, vec!["apps/ios/AppKit"]);
    }

    #[test]
    fn deps_accept_host_select_tables() {
        let src = r#"
[[target]]
name = "core"
kind = "custom_library"
srcs = ["src/**/*.custom"]

[target.deps.select]
macos = ["./mac_only"]
linux = ["./linux_only"]
default = ["./portable"]

[target.attrs]
module_name = "core"
"#;
        let targets =
            load_toml_with("crates/core/once.toml", src, Path::new("."), "crates/core").unwrap();
        let expected = if std::env::consts::OS == "macos" {
            "crates/core/mac_only"
        } else if std::env::consts::OS == "linux" {
            "crates/core/linux_only"
        } else {
            "crates/core/portable"
        };
        assert_eq!(targets[0].deps, vec![expected]);
    }

    #[test]
    fn deps_select_can_fall_back_for_other_hosts() {
        let value: toml::Value = toml::from_str(
            r#"
[select]
"macos-arm64" = ["./native"]
default = ["./portable"]
"#,
        )
        .unwrap();

        let selected = select_dep_value_for_tokens(
            "crates/core/once.toml",
            "core",
            &value,
            &select_tokens_for("macos", "x86_64"),
        )
        .unwrap();

        assert_eq!(selected.as_array().unwrap()[0].as_str(), Some("./portable"));
    }

    #[test]
    fn host_select_tokens_include_specific_and_general_tokens() {
        assert_eq!(
            select_tokens_for("macos", "x86_64"),
            vec!["macos-x86_64", "macos", "x86_64"]
        );
        assert_eq!(
            select_tokens_for("windows", "aarch64"),
            vec![
                "windows-arm64",
                "windows-aarch64",
                "windows",
                "arm64",
                "aarch64"
            ]
        );
    }
}
