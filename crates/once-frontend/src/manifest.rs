//! TOML frontend for script declarations.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::cache_provider::{CacheProviderToml, InfrastructureToml};
use crate::error::{Error, Result};
use crate::script::parse_script_annotations;
use crate::target::Target;
use crate::target_ref::{target_id, validate_target_name};

const MAX_SCRIPT_GLOB_MATCHES: usize = 1_000;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Manifest {
    infrastructure: InfrastructureToml,
    cache_provider: Option<CacheProviderToml>,
    script: Vec<ScriptTarget>,
    target: Vec<RuleTarget>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleTarget {
    name: String,
    rule: String,
    #[serde(default)]
    attrs: toml::Table,
    #[serde(default)]
    script: toml::Table,
    runtime: Option<RuntimeTask>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeTask {
    kind: Option<String>,
    runtime: Option<String>,
    target: Option<String>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    interface: Vec<RuntimeInterface>,
}

#[derive(Debug, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeInterface {
    name: String,
    kind: String,
    #[serde(default)]
    argv: Vec<String>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptTarget {
    name: String,
    path: Option<String>,
    #[serde(default)]
    argv: Vec<String>,
    #[serde(default)]
    env: Vec<String>,
    cwd: Option<String>,
    #[serde(default)]
    input: Vec<String>,
    #[serde(default)]
    output: Vec<String>,
    remote: Option<String>,
    runtime: Option<RuntimeTask>,
}

struct ManifestScriptSpec {
    name: String,
    argv: Vec<String>,
    env: Vec<String>,
    cwd: Option<String>,
    input: Vec<String>,
    output: Vec<String>,
    remote: Option<String>,
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
    let manifest: Manifest = toml::from_str(src).map_err(|source| Error::Parse {
        path: display_name.to_string(),
        message: source.to_string(),
    })?;

    let mut targets = Vec::new();
    for script in manifest.script {
        targets.push(script_target(
            script,
            workspace_root,
            package,
            display_name,
        )?);
    }
    for target in manifest.target {
        targets.push(rule_target(target, workspace_root, package, display_name)?);
    }
    Ok(targets)
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

fn rule_target(
    target: RuleTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    if target.rule != "script" {
        return Err(Error::Eval {
            path: display_name.to_string(),
            message: format!(
                "unknown rule `{}` for target `{}`; Once only supports `script`",
                target.rule, target.name
            ),
        });
    }
    if !target.attrs.is_empty() {
        return Err(Error::Eval {
            path: display_name.to_string(),
            message: format!(
                "script target `{}` must declare rule fields in `[target.script]`, not `[target.attrs]`",
                target.name
            ),
        });
    }
    let mut script_table = target.script;
    script_table.insert("name".to_string(), toml::Value::String(target.name.clone()));
    let mut script: ScriptTarget =
        toml::Value::Table(script_table)
            .try_into()
            .map_err(|source| Error::Eval {
                path: display_name.to_string(),
                message: format!(
                    "invalid script fields for target `{}`: {source}",
                    target.name
                ),
            })?;
    script.runtime = target.runtime;
    script_target(script, workspace_root, package, display_name)
}

fn script_target(
    t: ScriptTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    let ScriptTarget {
        name,
        path,
        argv,
        env,
        cwd,
        input,
        output,
        remote,
        runtime,
    } = t;
    if path.is_some()
        && (!env.is_empty() || cwd.is_some() || !input.is_empty() || !output.is_empty())
    {
        return Err(Error::Eval {
            path: display_name.to_string(),
            message: format!(
                "file-backed script target `{name}` must declare execution metadata in ONCE headers"
            ),
        });
    }

    match (path, argv.is_empty()) {
        (Some(path), true) => file_script_target(
            name,
            &path,
            remote,
            workspace_root,
            package,
            display_name,
            runtime,
        ),
        (None, false) => manifest_script_target(
            ManifestScriptSpec {
                name,
                argv,
                env,
                cwd,
                input,
                output,
                remote,
            },
            runtime,
            workspace_root,
            package,
            display_name,
        ),
        (Some(_), false) => Err(Error::Eval {
            path: display_name.to_string(),
            message: format!("script target `{name}` must set either `path` or `argv`, not both"),
        }),
        (None, true) => Err(Error::Eval {
            path: display_name.to_string(),
            message: format!("script target `{name}` must set one of `path` or `argv`"),
        }),
    }
}

fn file_script_target(
    name: String,
    path: &str,
    remote: Option<String>,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
    runtime: Option<RuntimeTask>,
) -> Result<Target> {
    let checked_name = checked_name(name, display_name)?;
    let script_rel = normalize_package_relative_path("", path, display_name)?;
    let script_path = normalize_script_path(package, &script_rel);
    let script_abs = workspace_root.join(&script_path);
    let annotations = parse_script_annotations(&script_abs, display_name)?;
    let script_parent = script_relative_parent(&script_rel);
    let mut attrs = BTreeMap::new();

    attrs.insert("script_path".to_string(), script_path);
    attrs.insert("script_runtime".to_string(), annotations.runtime.clone());
    if !annotations.runtime_args.is_empty() {
        attrs.insert(
            "script_runtime_args_json".to_string(),
            serde_json::to_string(&annotations.runtime_args)
                .expect("script runtime args are serializable"),
        );
    }
    insert_opt(&mut attrs, "remote_provider", remote.or(annotations.remote));
    insert_json_vec(&mut attrs, "script_env_json", &annotations.env_vars);

    let outputs =
        resolve_script_outputs(&annotations.outputs, package, &script_parent, display_name)?;
    if !outputs.is_empty() {
        attrs.insert(
            "outputs_json".to_string(),
            serde_json::to_string(&outputs).expect("script outputs are serializable"),
        );
    }

    let default_cwd = normalize_script_path(package, &script_parent);
    let cwd = annotations
        .cwd
        .as_deref()
        .map(|raw| {
            normalize_package_relative_path(&script_parent, raw, display_name)
                .map(|path| normalize_script_path(package, &path))
        })
        .transpose()?
        .unwrap_or(default_cwd);
    attrs.insert("cwd".to_string(), cwd);

    let has_runtime = runtime.is_some();
    attrs.insert("cache".to_string(), (!has_runtime).to_string());
    if let Some(runtime) = runtime {
        insert_runtime_attrs(&mut attrs, runtime, package, display_name)?;
    }

    let mut srcs = resolve_script_inputs(
        &annotations.inputs,
        workspace_root,
        package,
        &script_parent,
        display_name,
    )?;
    if !srcs.iter().any(|src| src == &script_rel) {
        srcs.push(script_rel);
        srcs.sort();
        srcs.dedup();
    }

    Ok(Target {
        package: package.to_string(),
        kind: if has_runtime {
            "runtime_script"
        } else {
            "script"
        }
        .to_string(),
        name: checked_name,
        srcs,
        attrs,
    })
}

fn manifest_script_target(
    spec: ManifestScriptSpec,
    runtime: Option<RuntimeTask>,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    let ManifestScriptSpec {
        name,
        argv,
        env,
        cwd,
        input,
        output,
        remote,
    } = spec;
    let mut attrs = BTreeMap::new();
    insert_manifest_script_attrs(&mut attrs, &argv, &env, &output);
    insert_opt(&mut attrs, "cwd", cwd);
    insert_opt(&mut attrs, "remote_provider", remote);
    let has_runtime = runtime.is_some();
    attrs.insert("cache".to_string(), (!has_runtime).to_string());
    if let Some(runtime) = runtime {
        insert_runtime_attrs(&mut attrs, runtime, package, display_name)?;
    }
    Ok(Target {
        package: package.to_string(),
        kind: if has_runtime {
            "runtime_script"
        } else {
            "script"
        }
        .to_string(),
        name: checked_name(name, display_name)?,
        srcs: resolve_script_inputs(&input, workspace_root, package, "", display_name)?,
        attrs,
    })
}

fn insert_runtime_attrs(
    attrs: &mut BTreeMap<String, String>,
    runtime: RuntimeTask,
    package: &str,
    display_name: &str,
) -> Result<()> {
    let kind = runtime
        .kind
        .or(runtime.runtime)
        .ok_or_else(|| Error::Eval {
            path: display_name.to_string(),
            message: "runtime metadata must set `kind`".to_string(),
        })?;
    attrs.insert("runtime".to_string(), kind);
    if !runtime.capabilities.is_empty() {
        attrs.insert(
            "runtime_capabilities_json".to_string(),
            serde_json::to_string(&runtime.capabilities)
                .expect("runtime capabilities are serializable"),
        );
    }
    if !runtime.interface.is_empty() {
        attrs.insert(
            "runtime_interfaces_json".to_string(),
            serde_json::to_string(&runtime.interface).expect("runtime interfaces are serializable"),
        );
    }
    if let Some(target) = runtime.target {
        attrs.insert(
            "runtime_target".to_string(),
            normalize_runtime_target(package, &target),
        );
    }
    Ok(())
}

fn insert_manifest_script_attrs(
    attrs: &mut BTreeMap<String, String>,
    argv: &[String],
    env: &[String],
    outputs: &[String],
) {
    attrs.insert(
        "script_argv_json".to_string(),
        serde_json::to_string(argv).expect("script argv is serializable"),
    );
    insert_json_vec(attrs, "script_env_json", env);
    insert_json_vec(attrs, "outputs_json", outputs);
}

fn insert_json_vec(attrs: &mut BTreeMap<String, String>, name: &str, values: &[String]) {
    if !values.is_empty() {
        attrs.insert(
            name.to_string(),
            serde_json::to_string(values).expect("string vec is serializable"),
        );
    }
}

fn insert_opt(attrs: &mut BTreeMap<String, String>, name: &str, value: Option<String>) {
    if let Some(value) = value {
        attrs.insert(name.to_string(), value);
    }
}

fn checked_name(name: String, display_name: &str) -> Result<String> {
    validate_target_name(&name).map_err(|e| Error::Eval {
        path: display_name.to_string(),
        message: e.to_string(),
    })?;
    Ok(name)
}

fn normalize_script_path(package: &str, path: &str) -> String {
    if path.is_empty() {
        package.to_string()
    } else if package.is_empty() {
        path.to_string()
    } else {
        format!("{package}/{path}")
    }
}

fn normalize_runtime_target(package: &str, raw: &str) -> String {
    if raw.contains('/') {
        raw.to_string()
    } else {
        target_id(package, raw)
    }
}

fn normalize_package_relative_path(base: &str, raw: &str, display_name: &str) -> Result<String> {
    if raw.is_empty() || raw.starts_with('/') || raw.contains('\\') {
        return Err(Error::Eval {
            path: display_name.to_string(),
            message: format!("invalid workspace path `{raw}`"),
        });
    }
    let mut out: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for segment in raw.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop().ok_or_else(|| Error::Eval {
                    path: display_name.to_string(),
                    message: format!("path `{raw}` escapes the package"),
                })?;
            }
            segment => out.push(segment),
        }
    }
    Ok(out.join("/"))
}

fn script_relative_parent(script: &str) -> String {
    script
        .rsplit_once('/')
        .map_or_else(String::new, |(parent, _)| parent.to_string())
}

fn resolve_script_inputs(
    inputs: &[String],
    workspace_root: &Path,
    package: &str,
    base: &str,
    display_name: &str,
) -> Result<Vec<String>> {
    resolve_paths(inputs, workspace_root, package, base, display_name)
}

fn resolve_script_outputs(
    outputs: &[String],
    package: &str,
    base: &str,
    display_name: &str,
) -> Result<Vec<String>> {
    outputs
        .iter()
        .map(|raw| {
            normalize_package_relative_path(base, raw, display_name)
                .map(|path| normalize_script_path(package, &path))
        })
        .collect()
}

fn resolve_paths(
    paths: &[String],
    workspace_root: &Path,
    package: &str,
    base: &str,
    display_name: &str,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for raw in paths {
        let path = normalize_package_relative_path(base, raw, display_name)?;
        if raw.contains('*') || raw.contains('?') || raw.contains('[') {
            let pattern = normalize_script_path(package, &path);
            let abs_pattern = workspace_root.join(&pattern);
            let abs_pattern = abs_pattern.to_string_lossy().to_string();
            let mut matches = glob::glob(&abs_pattern)
                .map_err(|source| Error::Eval {
                    path: display_name.to_string(),
                    message: format!("invalid glob pattern `{raw}`: {source}"),
                })?
                .take(MAX_SCRIPT_GLOB_MATCHES + 1)
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|source| Error::Eval {
                    path: display_name.to_string(),
                    message: format!("failed to expand glob pattern `{raw}`: {source}"),
                })?;
            if matches.len() > MAX_SCRIPT_GLOB_MATCHES {
                return Err(Error::Eval {
                    path: display_name.to_string(),
                    message: format!(
                        "glob pattern `{raw}` matched more than {MAX_SCRIPT_GLOB_MATCHES} files"
                    ),
                });
            }
            matches.sort();
            for matched in matches {
                let rel = matched
                    .strip_prefix(workspace_root)
                    .map_err(|_| Error::Eval {
                        path: display_name.to_string(),
                        message: format!("glob pattern `{raw}` matched outside the workspace"),
                    })?
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/");
                out.push(rel);
            }
        } else {
            out.push(path);
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_manifest_script_target() {
        let src = r#"
[[script]]
name = "hello"
argv = ["sh", "-c", "printf hello"]
input = ["input.txt"]
output = ["out.txt"]
"#;
        let targets = load_toml_str("once.toml", src).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, "script");
        assert_eq!(targets[0].name, "hello");
        assert_eq!(targets[0].srcs, vec!["input.txt"]);
        assert_eq!(
            targets[0].attrs["script_argv_json"],
            "[\"sh\",\"-c\",\"printf hello\"]"
        );
    }

    #[test]
    fn rejects_language_rules() {
        let src = r#"
[[target]]
name = "app"
rule = "rust.binary"
"#;
        let err = load_toml_str("once.toml", src).unwrap_err().to_string();
        assert!(err.contains("Once only supports `script`"));
    }
}
