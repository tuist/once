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
    infrastructure: InfrastructureToml,
    cache_provider: Option<CacheProviderToml>,
    target: Vec<TargetToml>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TargetToml {
    name: String,
    kind: String,
    deps: Vec<String>,
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
        let deps = self
            .deps
            .iter()
            .map(|dep| {
                normalize_manifest_target(package, dep).map_err(|source| Error::Eval {
                    path: display_name.to_string(),
                    message: source.to_string(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
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
        let targets = load_toml_str("once.toml", "").unwrap();
        assert!(targets.is_empty());
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
}
