//! TOML frontend for workspace configuration.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cache_provider::{CacheProviderToml, InfrastructureProviderToml, InfrastructureToml};
use crate::error::{Error, Result};
use crate::target::{AttrValue, Target};
use crate::target_ref::{normalize_manifest_target, validate_target_name};
use crate::TOML_BUILD_FILE_NAME;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Manifest {
    workspace: WorkspaceToml,
    infrastructure: InfrastructureToml,
    infrastructures: BTreeMap<String, InfrastructureProviderToml>,
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
    pub configuration: ConfigurationToml,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ConfigurationToml {
    os: Option<String>,
    arch: Option<String>,
    tokens: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BuildConfiguration {
    pub os: String,
    pub arch: String,
    pub tokens: Vec<String>,
}

impl BuildConfiguration {
    fn host() -> Self {
        Self::new(std::env::consts::OS, std::env::consts::ARCH, Vec::new())
    }

    pub(crate) fn from_toml(configuration: ConfigurationToml) -> Result<Self> {
        let os = configuration
            .os
            .unwrap_or_else(|| std::env::consts::OS.to_string());
        let arch = configuration
            .arch
            .unwrap_or_else(|| std::env::consts::ARCH.to_string());
        if os.trim().is_empty() || arch.trim().is_empty() {
            return Err(Error::Eval {
                path: TOML_BUILD_FILE_NAME.to_string(),
                message:
                    "workspace configuration operating system and architecture must be non-empty"
                        .to_string(),
            });
        }
        if configuration
            .tokens
            .iter()
            .any(|token| token.trim().is_empty())
        {
            return Err(Error::Eval {
                path: TOML_BUILD_FILE_NAME.to_string(),
                message: "workspace configuration tokens must be non-empty".to_string(),
            });
        }
        Ok(Self::new(&os, &arch, configuration.tokens))
    }

    fn new(os: &str, arch: &str, extra_tokens: Vec<String>) -> Self {
        let os = normalize_os(os);
        let arch = normalize_arch(arch);
        let mut tokens = select_tokens_for(&os, &arch);
        for token in extra_tokens {
            if !tokens.contains(&token) {
                tokens.push(token);
            }
        }
        if !tokens.iter().any(|token| token == "default") {
            tokens.push("default".to_string());
        }
        Self { os, arch, tokens }
    }
}

impl Default for BuildConfiguration {
    fn default() -> Self {
        Self::host()
    }
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
    dependencies: BTreeMap<String, toml::Value>,
    srcs: Vec<String>,
    visibility: Vec<String>,
    attrs: BTreeMap<String, toml::Value>,
}

pub fn load_toml_str(path: &str, src: &str) -> Result<Vec<Target>> {
    load_toml_with(path, src, Path::new("."), "")
}

pub(crate) fn load_toml_with(
    display_name: &str,
    src: &str,
    workspace_root: &Path,
    package: &str,
) -> Result<Vec<Target>> {
    load_toml_with_configuration(
        display_name,
        src,
        workspace_root,
        package,
        &BuildConfiguration::host(),
    )
}

pub(crate) fn load_toml_with_configuration(
    display_name: &str,
    src: &str,
    _workspace_root: &Path,
    package: &str,
    configuration: &BuildConfiguration,
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
        .map(|target| target.into_target(display_name, package, configuration))
        .collect()
}

pub fn load_cache_provider_toml_str(
    path: &str,
    src: &str,
) -> Result<Option<crate::cache_provider::CacheProviderConfig>> {
    Ok(load_infrastructure_toml_str(path, src)?.cache)
}

pub fn load_infrastructure_toml_str(
    path: &str,
    src: &str,
) -> Result<crate::cache_provider::InfrastructureConfig> {
    let manifest: Manifest = toml::from_str(src).map_err(|source| Error::Parse {
        path: path.to_string(),
        message: source.to_string(),
    })?;
    let cache = if let Some(raw) = manifest.infrastructure.cache {
        Some(raw.into_config(path)?)
    } else {
        manifest
            .cache_provider
            .map(|raw| raw.into_config(path))
            .transpose()?
    };
    let execution = manifest
        .infrastructure
        .execution
        .map(|raw| raw.into_config(path, "infrastructure.execution"))
        .transpose()?;
    let providers = manifest
        .infrastructures
        .into_iter()
        .map(|(name, provider)| {
            provider
                .into_config(path, &name)
                .map(|provider| (name, provider))
        })
        .collect::<Result<_>>()?;
    Ok(crate::cache_provider::InfrastructureConfig {
        cache,
        execution,
        providers,
    })
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

pub fn load_workspace_configuration(root: &Path) -> Result<BuildConfiguration> {
    let path = root.join(TOML_BUILD_FILE_NAME);
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BuildConfiguration::host());
        }
        Err(source) => {
            return Err(Error::Read {
                path: path.display().to_string(),
                source,
            });
        }
    };
    let workspace = load_workspace_toml_str(TOML_BUILD_FILE_NAME, &source)?;
    BuildConfiguration::from_toml(workspace.configuration)
}

impl TargetToml {
    fn into_target(
        self,
        display_name: &str,
        package: &str,
        configuration: &BuildConfiguration,
    ) -> Result<Target> {
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
        let deps = deps_from_toml(
            display_name,
            &self.name,
            package,
            self.deps.as_ref(),
            configuration,
        )?;
        let dependency_edges = dependency_edges_from_toml(
            display_name,
            &self.name,
            package,
            &self.dependencies,
            configuration,
        )?;
        let mut attrs = BTreeMap::new();
        let mut typed_attrs = BTreeMap::new();
        for (key, value) in self.attrs {
            let string_value = toml_value_to_attr_string(&value);
            let attr_value = toml_value_to_attr_value(value);
            attrs.insert(key.clone(), string_value);
            typed_attrs.insert(key, attr_value);
        }
        Ok(Target {
            package: package.to_string(),
            kind: self.kind,
            name: self.name,
            deps,
            dependency_edges,
            srcs: self.srcs,
            visibility: self.visibility,
            attrs,
            typed_attrs,
        })
    }
}

fn dependency_edges_from_toml(
    display_name: &str,
    target_name: &str,
    package: &str,
    values: &BTreeMap<String, toml::Value>,
    configuration: &BuildConfiguration,
) -> Result<BTreeMap<String, Vec<String>>> {
    let mut edges = BTreeMap::new();
    for (name, value) in values {
        if name == "deps" {
            return Err(Error::Eval {
                path: display_name.to_string(),
                message: format!(
                    "target `{target_name}` dependency role `deps` must use the top-level `deps` field"
                ),
            });
        }
        let dependencies = deps_from_toml(
            display_name,
            &format!("{target_name}` dependency role `{name}"),
            package,
            Some(value),
            configuration,
        )?;
        edges.insert(name.clone(), dependencies);
    }
    Ok(edges)
}

fn deps_from_toml(
    display_name: &str,
    target_name: &str,
    package: &str,
    value: Option<&toml::Value>,
    configuration: &BuildConfiguration,
) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let selected =
        select_dep_value_for_tokens(display_name, target_name, value, &configuration.tokens)?;
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

fn select_dep_value_for_tokens<'a>(
    display_name: &str,
    target_name: &str,
    value: &'a toml::Value,
    tokens: &[String],
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
        if let Some(value) = branches.get(token) {
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

fn select_tokens_for(os: &str, arch: &str) -> Vec<String> {
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
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn normalize_os(value: &str) -> String {
    match value {
        "darwin" | "mac" | "macosx" => "macos".to_string(),
        other => other.to_string(),
    }
}

fn normalize_arch(value: &str) -> String {
    match value {
        "arm64" => "aarch64".to_string(),
        "amd64" => "x86_64".to_string(),
        other => other.to_string(),
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
    fn loads_named_microsandbox_execution_provider() {
        let config = load_infrastructure_toml_str(
            "once.toml",
            r#"
[infrastructure.execution]
provider = "remote_tests"

[infrastructures.remote_tests]
kind = "microsandbox"
image = "node:22.18.0-alpine"
"#,
        )
        .unwrap();

        assert_eq!(
            config.execution,
            Some(crate::cache_provider::NamedCacheProviderConfig {
                name: "remote_tests".to_string(),
                account: None,
                project: None,
            })
        );
        assert_eq!(
            config.providers.get("remote_tests"),
            Some(
                &crate::cache_provider::InfrastructureProviderConfig::Microsandbox(
                    crate::cache_provider::MicrosandboxExecutionProviderConfig {
                        image: "node:22.18.0-alpine".to_string(),
                    }
                )
            )
        );
    }

    #[test]
    fn rejects_empty_microsandbox_image() {
        let error = load_infrastructure_toml_str(
            "once.toml",
            r#"
[infrastructures.remote_tests]
kind = "microsandbox"
image = "   "
"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("image must not be empty"));
    }

    #[test]
    fn loads_hosted_execution_providers() {
        let config = load_infrastructure_toml_str(
            "once.toml",
            r#"
[infrastructures.e2b_tests]
kind = "e2b"
template = "vitest-v1"

[infrastructures.daytona_tests]
kind = "daytona"
image = "node:22-bookworm"
"#,
        )
        .unwrap();

        assert_eq!(
            config.providers.get("e2b_tests"),
            Some(&crate::cache_provider::InfrastructureProviderConfig::E2b(
                crate::cache_provider::E2bExecutionProviderConfig {
                    template: "vitest-v1".to_string(),
                }
            ))
        );
        assert_eq!(
            config.providers.get("daytona_tests"),
            Some(
                &crate::cache_provider::InfrastructureProviderConfig::Daytona(
                    crate::cache_provider::DaytonaExecutionProviderConfig {
                        image: "node:22-bookworm".to_string(),
                    }
                )
            )
        );
    }

    #[test]
    fn rejects_empty_hosted_provider_environments() {
        for manifest in [
            r#"
[infrastructures.remote_tests]
kind = "e2b"
template = ""
"#,
            r#"
[infrastructures.remote_tests]
kind = "daytona"
image = "   "
"#,
        ] {
            let error = load_infrastructure_toml_str("once.toml", manifest).unwrap_err();
            assert!(error.to_string().contains("must not be empty"));
        }
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
    fn named_dependency_roles_are_package_relative() {
        let src = r#"
[[target]]
name = "App"
kind = "custom_application"

[target.dependencies]
plugins = ["./CompilerPlugin"]
runtime = ["shared/Runtime"]
"#;
        let targets =
            load_toml_with("apps/ios/once.toml", src, Path::new("."), "apps/ios").unwrap();
        assert_eq!(
            targets[0].dependency_edges.get("plugins"),
            Some(&vec!["apps/ios/CompilerPlugin".to_string()])
        );
        assert_eq!(
            targets[0].dependency_edges.get("runtime"),
            Some(&vec!["shared/Runtime".to_string()])
        );
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
    fn explicit_target_configuration_selects_dependencies() {
        let configuration = BuildConfiguration::new("linux", "arm64", vec!["release".to_string()]);
        let source = r#"
[[target]]
name = "App"
kind = "plain"
deps = { select = { release = ["./Optimized"], default = ["./Portable"] } }
"#;

        let targets =
            load_toml_with_configuration("once.toml", source, Path::new("."), "", &configuration)
                .unwrap();

        assert_eq!(targets[0].deps, vec!["Optimized"]);
        assert_eq!(configuration.os, "linux");
        assert_eq!(configuration.arch, "aarch64");
        assert!(configuration.tokens.contains(&"linux-arm64".to_string()));
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
