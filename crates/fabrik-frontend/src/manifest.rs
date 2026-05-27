//! TOML build-file frontend.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{
    de::{DeserializeOwned, Error as DeError},
    Deserialize, Deserializer,
};

use crate::cache_provider::{CacheProviderToml, InfrastructureToml};
use crate::dependency::{into_entries, DependencyEntry, DependencyEntryToml};
use crate::error::{Error, Result};
use crate::script::parse_script_annotations;
use crate::target::{ExternalDependency, Target};
use crate::target_ref::{normalize_build_dep, validate_target_name};

const MAX_SCRIPT_GLOB_MATCHES: usize = 1_000;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Manifest {
    infrastructure: InfrastructureToml,
    cache_provider: Option<CacheProviderToml>,
    rust: RustSection,
    cargo: CargoSection,
    go: GoSection,
    apple: AppleSection,
    elixir: ElixirSection,
    deps: Vec<DependencyEntryToml>,
    target: Vec<RuleTarget>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RustSection {
    library: Vec<RustTarget>,
    binary: Vec<RustTarget>,
    test: Vec<RustTarget>,
    proc_macro: Vec<RustTarget>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CargoSection {
    binary: Vec<CargoBinaryTarget>,
    build_script: Vec<CargoBuildScriptTarget>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct GoSection {
    binary: Vec<GoTarget>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AppleSection {
    ios_app: Vec<AppleIosAppTarget>,
    simulator_app: Vec<AppleSimulatorAppTarget>,
    swift_library: Vec<AppleSwiftTarget>,
    static_framework: Vec<AppleFrameworkTarget>,
    dynamic_framework: Vec<AppleFrameworkTarget>,
    macos_command_line_application: Vec<AppleSwiftTarget>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ElixirSection {
    library: Vec<ElixirTarget>,
    binary: Vec<ElixirBinaryTarget>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ElixirTarget {
    name: String,
    #[serde(default)]
    srcs: Vec<String>,
    #[serde(default)]
    src_globs: Vec<String>,
    #[serde(default)]
    deps: Vec<ManifestDep>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ElixirBinaryTarget {
    name: String,
    entry: String,
    #[serde(default)]
    srcs: Vec<String>,
    #[serde(default)]
    src_globs: Vec<String>,
    #[serde(default)]
    deps: Vec<ManifestDep>,
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RustTarget {
    name: String,
    #[serde(default)]
    srcs: Vec<String>,
    #[serde(default)]
    src_globs: Vec<String>,
    #[serde(default)]
    deps: Vec<ManifestDep>,
    edition: Option<String>,
    crate_name: Option<String>,
    crate_root: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CargoBinaryTarget {
    name: String,
    cargo_package: String,
    #[serde(default)]
    srcs: Vec<String>,
    #[serde(default)]
    src_globs: Vec<String>,
    #[serde(default)]
    deps: Vec<ManifestDep>,
    bin: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CargoBuildScriptTarget {
    name: String,
    #[serde(default)]
    srcs: Vec<String>,
    #[serde(default)]
    src_globs: Vec<String>,
    #[serde(default)]
    deps: Vec<ManifestDep>,
    crate_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoTarget {
    name: String,
    #[serde(default)]
    srcs: Vec<String>,
    #[serde(default)]
    src_globs: Vec<String>,
    #[serde(default)]
    deps: Vec<ManifestDep>,
    package: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppleIosAppTarget {
    name: String,
    bundle_id: String,
    #[serde(default)]
    srcs: Vec<String>,
    #[serde(default)]
    src_globs: Vec<String>,
    #[serde(default)]
    deps: Vec<ManifestDep>,
    executable_name: Option<String>,
    minimum_os: Option<String>,
    simulator: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppleSimulatorAppTarget {
    name: String,
    platform: String,
    bundle_id: String,
    #[serde(default)]
    srcs: Vec<String>,
    #[serde(default)]
    src_globs: Vec<String>,
    #[serde(default)]
    deps: Vec<ManifestDep>,
    executable_name: Option<String>,
    minimum_os: Option<String>,
    simulator: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppleSwiftTarget {
    name: String,
    #[serde(default)]
    srcs: Vec<String>,
    #[serde(default)]
    src_globs: Vec<String>,
    #[serde(default)]
    deps: Vec<ManifestDep>,
    module_name: Option<String>,
    minimum_os: Option<String>,
    #[serde(default)]
    swiftc_flags: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppleFrameworkTarget {
    name: String,
    #[serde(default)]
    srcs: Vec<String>,
    #[serde(default)]
    src_globs: Vec<String>,
    #[serde(default)]
    deps: Vec<ManifestDep>,
    module_name: Option<String>,
    minimum_os: Option<String>,
    bundle_id: Option<String>,
    #[serde(default)]
    swiftc_flags: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleTarget {
    name: String,
    rule: Option<String>,
    kind: Option<String>,
    #[serde(default)]
    attrs: toml::Table,
    #[serde(default)]
    script: toml::Table,
    runtime: Option<RuntimeTask>,
    #[serde(default)]
    srcs: Vec<String>,
    #[serde(default)]
    src_globs: Vec<String>,
    #[serde(default)]
    deps: Vec<ManifestDep>,
}

#[derive(Debug)]
enum ManifestDep {
    Target(String),
    External(toml::Table),
}

impl<'de> Deserialize<'de> for ManifestDep {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match toml::Value::deserialize(deserializer)? {
            toml::Value::String(dep) => Ok(Self::Target(dep)),
            toml::Value::Table(dep) => Ok(Self::External(dep)),
            other => Err(D::Error::custom(format!(
                "dependency entry must be a string local target or an inline table external dependency, got {}",
                toml_value_kind(&other)
            ))),
        }
    }
}

struct NormalizedDeps {
    local: Vec<String>,
    external: Vec<ExternalDependency>,
}

pub fn load_toml_str(name: &str, src: &str) -> Result<Vec<Target>> {
    load_toml_with(name, src, Path::new("."), "")
}

pub fn load_dependency_entries_toml_str(name: &str, src: &str) -> Result<Vec<DependencyEntry>> {
    load_dependency_entries_toml_with(name, src, "")
}

pub(crate) fn load_toml_with(
    name: &str,
    src: &str,
    workspace_root: &Path,
    package: &str,
) -> Result<Vec<Target>> {
    let manifest = parse_manifest(name, src)?;
    let mut targets = Vec::new();

    push_rust_targets(&mut targets, manifest.rust, workspace_root, package, name)?;
    push_cargo_targets(&mut targets, manifest.cargo, workspace_root, package, name)?;
    push_go_targets(&mut targets, manifest.go, workspace_root, package, name)?;
    push_apple_targets(&mut targets, manifest.apple, workspace_root, package, name)?;
    push_elixir_targets(&mut targets, manifest.elixir, workspace_root, package, name)?;
    for t in manifest.target {
        targets.push(rule_target(t, workspace_root, package, name)?);
    }

    Ok(targets)
}

pub(crate) fn load_dependency_entries_toml_with(
    name: &str,
    src: &str,
    package: &str,
) -> Result<Vec<DependencyEntry>> {
    let manifest = parse_manifest(name, src)?;
    Ok(into_entries(manifest.deps, package))
}

fn parse_manifest(name: &str, src: &str) -> Result<Manifest> {
    let manifest_value: toml::Value = toml::from_str(src).map_err(|e| Error::Parse {
        path: name.to_owned(),
        message: e.to_string(),
    })?;
    if manifest_value
        .as_table()
        .is_some_and(|table| table.contains_key("task"))
    {
        return Err(Error::Parse {
            path: name.to_owned(),
            message: "`[[task]]` has been removed; rewrite it as `[[target]]` with `rule = \"script\"` and `[target.script]`, and rename `srcs` / `src_globs` to `input` plus `outputs` to `output`".to_string(),
        });
    }
    manifest_value.try_into().map_err(|e| Error::Parse {
        path: name.to_owned(),
        message: e.to_string(),
    })
}

fn push_rust_targets(
    targets: &mut Vec<Target>,
    rust: RustSection,
    workspace_root: &Path,
    package: &str,
    name: &str,
) -> Result<()> {
    for t in rust.library {
        targets.push(rust_target(
            "rust_library",
            t,
            workspace_root,
            package,
            name,
        )?);
    }
    for t in rust.binary {
        targets.push(rust_target(
            "rust_binary",
            t,
            workspace_root,
            package,
            name,
        )?);
    }
    for t in rust.test {
        targets.push(rust_target("rust_test", t, workspace_root, package, name)?);
    }
    for t in rust.proc_macro {
        targets.push(rust_target(
            "rust_proc_macro",
            t,
            workspace_root,
            package,
            name,
        )?);
    }
    Ok(())
}

fn push_cargo_targets(
    targets: &mut Vec<Target>,
    cargo: CargoSection,
    workspace_root: &Path,
    package: &str,
    name: &str,
) -> Result<()> {
    for t in cargo.binary {
        targets.push(cargo_binary_target(t, workspace_root, package, name)?);
    }
    for t in cargo.build_script {
        targets.push(cargo_build_script_target(t, workspace_root, package, name)?);
    }
    Ok(())
}

fn push_go_targets(
    targets: &mut Vec<Target>,
    go: GoSection,
    workspace_root: &Path,
    package: &str,
    name: &str,
) -> Result<()> {
    for t in go.binary {
        targets.push(go_binary_target(t, workspace_root, package, name)?);
    }
    Ok(())
}

fn push_apple_targets(
    targets: &mut Vec<Target>,
    apple: AppleSection,
    workspace_root: &Path,
    package: &str,
    name: &str,
) -> Result<()> {
    for t in apple.ios_app {
        targets.push(apple_ios_app_target(t, workspace_root, package, name)?);
    }
    for t in apple.simulator_app {
        targets.push(apple_simulator_app_target(
            t,
            workspace_root,
            package,
            name,
        )?);
    }
    for t in apple.swift_library {
        targets.push(apple_swift_target(
            "swift_library",
            t,
            workspace_root,
            package,
            name,
        )?);
    }
    for t in apple.static_framework {
        targets.push(apple_framework_target(
            "apple_static_framework",
            t,
            workspace_root,
            package,
            name,
        )?);
    }
    for t in apple.dynamic_framework {
        targets.push(apple_framework_target(
            "apple_dynamic_framework",
            t,
            workspace_root,
            package,
            name,
        )?);
    }
    for t in apple.macos_command_line_application {
        targets.push(apple_swift_target(
            "macos_command_line_application",
            t,
            workspace_root,
            package,
            name,
        )?);
    }
    Ok(())
}

fn push_elixir_targets(
    targets: &mut Vec<Target>,
    elixir: ElixirSection,
    workspace_root: &Path,
    package: &str,
    name: &str,
) -> Result<()> {
    for t in elixir.library {
        targets.push(elixir_target(
            "elixir_library",
            t,
            workspace_root,
            package,
            name,
        )?);
    }
    for t in elixir.binary {
        targets.push(elixir_binary_target(t, workspace_root, package, name)?);
    }
    Ok(())
}

fn elixir_target(
    kind: &str,
    t: ElixirTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    let deps = normalize_deps(t.deps, package, display_name)?;
    Ok(Target {
        package: package.to_string(),
        external_package: None,
        kind: kind.to_string(),
        name: checked_name(t.name, display_name)?,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: deps.local,
        external_deps: deps.external,
        attrs: BTreeMap::new(),
    })
}

fn elixir_binary_target(
    t: ElixirBinaryTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    let mut attrs = BTreeMap::new();
    attrs.insert("entry".to_string(), t.entry);
    let deps = normalize_deps(t.deps, package, display_name)?;
    Ok(Target {
        package: package.to_string(),
        external_package: None,
        kind: "elixir_binary".to_string(),
        name: checked_name(t.name, display_name)?,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: deps.local,
        external_deps: deps.external,
        attrs,
    })
}

fn rust_target(
    kind: &str,
    t: RustTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    let mut attrs = BTreeMap::new();
    insert_opt(&mut attrs, "edition", t.edition);
    insert_opt(&mut attrs, "crate_name", t.crate_name);
    insert_opt(&mut attrs, "crate_root", t.crate_root);
    let deps = normalize_deps(t.deps, package, display_name)?;
    Ok(Target {
        package: package.to_string(),
        external_package: None,
        kind: kind.to_string(),
        name: checked_name(t.name, display_name)?,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: deps.local,
        external_deps: deps.external,
        attrs,
    })
}

fn cargo_binary_target(
    t: CargoBinaryTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    let mut attrs = BTreeMap::new();
    attrs.insert("cargo_package".to_string(), t.cargo_package);
    insert_opt(&mut attrs, "bin", t.bin);
    let deps = normalize_deps(t.deps, package, display_name)?;
    Ok(Target {
        package: package.to_string(),
        external_package: None,
        kind: "cargo_binary".to_string(),
        name: checked_name(t.name, display_name)?,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: deps.local,
        external_deps: deps.external,
        attrs,
    })
}

fn cargo_build_script_target(
    t: CargoBuildScriptTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    let mut attrs = BTreeMap::new();
    insert_opt(&mut attrs, "crate_dir", t.crate_dir);
    let deps = normalize_deps(t.deps, package, display_name)?;
    Ok(Target {
        package: package.to_string(),
        external_package: None,
        kind: "cargo_build_script".to_string(),
        name: checked_name(t.name, display_name)?,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: deps.local,
        external_deps: deps.external,
        attrs,
    })
}

fn go_binary_target(
    t: GoTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    let mut attrs = BTreeMap::new();
    insert_opt(&mut attrs, "package", t.package);
    let deps = normalize_deps(t.deps, package, display_name)?;
    Ok(Target {
        package: package.to_string(),
        external_package: None,
        kind: "go_binary".to_string(),
        name: checked_name(t.name, display_name)?,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: deps.local,
        external_deps: deps.external,
        attrs,
    })
}

fn apple_ios_app_target(
    t: AppleIosAppTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    let mut attrs = BTreeMap::new();
    attrs.insert("bundle_id".to_string(), t.bundle_id);
    insert_opt(&mut attrs, "executable_name", t.executable_name);
    insert_opt(&mut attrs, "minimum_os", t.minimum_os);
    insert_opt(&mut attrs, "simulator", t.simulator);
    attrs.insert("platform".to_string(), "ios".to_string());
    let deps = normalize_deps(t.deps, package, display_name)?;
    Ok(Target {
        package: package.to_string(),
        external_package: None,
        kind: "apple_ios_app".to_string(),
        name: checked_name(t.name, display_name)?,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: deps.local,
        external_deps: deps.external,
        attrs,
    })
}

fn apple_simulator_app_target(
    t: AppleSimulatorAppTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    if t.platform != "ios" {
        return Err(Error::Eval {
            path: display_name.to_string(),
            message: format!(
                "apple.simulator_app platform `{}` is not supported yet; use `ios`",
                t.platform
            ),
        });
    }
    let mut attrs = BTreeMap::new();
    attrs.insert("platform".to_string(), t.platform);
    attrs.insert("bundle_id".to_string(), t.bundle_id);
    insert_opt(&mut attrs, "executable_name", t.executable_name);
    insert_opt(&mut attrs, "minimum_os", t.minimum_os);
    insert_opt(&mut attrs, "simulator", t.simulator);
    let deps = normalize_deps(t.deps, package, display_name)?;
    Ok(Target {
        package: package.to_string(),
        external_package: None,
        kind: "apple_simulator_app".to_string(),
        name: checked_name(t.name, display_name)?,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: deps.local,
        external_deps: deps.external,
        attrs,
    })
}

fn apple_swift_target(
    kind: &str,
    t: AppleSwiftTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    let mut attrs = BTreeMap::new();
    insert_opt(&mut attrs, "module_name", t.module_name);
    insert_opt(&mut attrs, "minimum_os", t.minimum_os);
    insert_json_vec(&mut attrs, "swiftc_flags_json", &t.swiftc_flags);
    let deps = normalize_deps(t.deps, package, display_name)?;
    Ok(Target {
        package: package.to_string(),
        external_package: None,
        kind: kind.to_string(),
        name: checked_name(t.name, display_name)?,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: deps.local,
        external_deps: deps.external,
        attrs,
    })
}

fn apple_framework_target(
    kind: &str,
    t: AppleFrameworkTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    let mut attrs = BTreeMap::new();
    insert_opt(&mut attrs, "module_name", t.module_name);
    insert_opt(&mut attrs, "minimum_os", t.minimum_os);
    insert_opt(&mut attrs, "bundle_id", t.bundle_id);
    insert_json_vec(&mut attrs, "swiftc_flags_json", &t.swiftc_flags);
    let deps = normalize_deps(t.deps, package, display_name)?;
    Ok(Target {
        package: package.to_string(),
        external_package: None,
        kind: kind.to_string(),
        name: checked_name(t.name, display_name)?,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: deps.local,
        external_deps: deps.external,
        attrs,
    })
}

fn rule_target(
    t: RuleTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    let rule = t.rule.clone();
    let kind = t.kind.clone();
    match (rule.as_deref(), kind.as_deref()) {
        (Some(rule), None) => builtin_rule_target(rule, t, workspace_root, package, display_name),
        (None, Some(_)) => legacy_generic_target(t, workspace_root, package, display_name),
        (None, None) => Err(Error::Eval {
            path: display_name.to_string(),
            message: format!("target `{}` must set `rule`", t.name),
        }),
        (Some(_), Some(_)) => Err(Error::Eval {
            path: display_name.to_string(),
            message: format!("target `{}` must not set both `rule` and `kind`", t.name),
        }),
    }
}

// Flat dispatch table over every known `rule = "..."` value. Growing it
// linearly is intentional: each plugin adds its own arm here so adding
// a new rule is one diff site, not five. The line cap doesn't carry
// real signal at this shape.
#[allow(clippy::too_many_lines)]
fn builtin_rule_target(
    rule: &str,
    t: RuleTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    let name = t.name;
    let runtime = t.runtime;
    let attrs = t.attrs;
    let script = t.script;
    if rule != "script" && !script.is_empty() {
        return Err(Error::Eval {
            path: display_name.to_string(),
            message: format!(
                "target `{name}` uses `[target.script]`, which is only valid for `rule = \"script\"`"
            ),
        });
    }
    match rule {
        "script" => {
            let script_attrs = script_rule_table(&name, !attrs.is_empty(), script, display_name)?;
            let script =
                decode_rule_table::<ScriptTarget>(&name, script_attrs, "script", display_name)?;
            script_target(script, workspace_root, package, display_name, runtime)
        }
        "rust.library" => rust_target(
            "rust_library",
            decode_rule_attrs(&name, attrs, display_name)?,
            workspace_root,
            package,
            display_name,
        ),
        "rust.binary" => rust_target(
            "rust_binary",
            decode_rule_attrs(&name, attrs, display_name)?,
            workspace_root,
            package,
            display_name,
        ),
        "rust.test" => rust_target(
            "rust_test",
            decode_rule_attrs(&name, attrs, display_name)?,
            workspace_root,
            package,
            display_name,
        ),
        "rust.proc_macro" => rust_target(
            "rust_proc_macro",
            decode_rule_attrs(&name, attrs, display_name)?,
            workspace_root,
            package,
            display_name,
        ),
        "cargo.binary" => cargo_binary_target(
            decode_rule_attrs(&name, attrs, display_name)?,
            workspace_root,
            package,
            display_name,
        ),
        "cargo.build_script" => cargo_build_script_target(
            decode_rule_attrs(&name, attrs, display_name)?,
            workspace_root,
            package,
            display_name,
        ),
        "go.binary" => go_binary_target(
            decode_rule_attrs(&name, attrs, display_name)?,
            workspace_root,
            package,
            display_name,
        ),
        "apple.simulator_app" => apple_simulator_app_target(
            decode_rule_attrs(&name, attrs, display_name)?,
            workspace_root,
            package,
            display_name,
        ),
        "apple.ios_app" => apple_ios_app_target(
            decode_rule_attrs(&name, attrs, display_name)?,
            workspace_root,
            package,
            display_name,
        ),
        "apple.swift_library" => apple_swift_target(
            "swift_library",
            decode_rule_attrs(&name, attrs, display_name)?,
            workspace_root,
            package,
            display_name,
        ),
        "apple.static_framework" => apple_framework_target(
            "apple_static_framework",
            decode_rule_attrs(&name, attrs, display_name)?,
            workspace_root,
            package,
            display_name,
        ),
        "apple.dynamic_framework" => apple_framework_target(
            "apple_dynamic_framework",
            decode_rule_attrs(&name, attrs, display_name)?,
            workspace_root,
            package,
            display_name,
        ),
        "apple.macos_command_line_application" => apple_swift_target(
            "macos_command_line_application",
            decode_rule_attrs(&name, attrs, display_name)?,
            workspace_root,
            package,
            display_name,
        ),
        "elixir.library" => elixir_target(
            "elixir_library",
            decode_rule_attrs(&name, attrs, display_name)?,
            workspace_root,
            package,
            display_name,
        ),
        "elixir.binary" => elixir_binary_target(
            decode_rule_attrs(&name, attrs, display_name)?,
            workspace_root,
            package,
            display_name,
        ),
        other => Err(Error::Eval {
            path: display_name.to_string(),
            message: format!("unknown rule `{other}` for target `{name}`"),
        }),
    }
}

fn legacy_generic_target(
    t: RuleTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    if !t.script.is_empty() {
        return Err(Error::Eval {
            path: display_name.to_string(),
            message: format!(
                "target `{}` uses `[target.script]`, which is only valid for `rule = \"script\"`",
                t.name
            ),
        });
    }
    let deps = normalize_deps(t.deps, package, display_name)?;
    Ok(Target {
        package: package.to_string(),
        external_package: None,
        kind: t.kind.expect("legacy target kind was checked"),
        name: checked_name(t.name, display_name)?,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: deps.local,
        external_deps: deps.external,
        attrs: string_attrs(t.attrs, display_name)?,
    })
}

fn decode_rule_attrs<T>(name: &str, attrs: toml::Table, display_name: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    decode_rule_table(name, attrs, "attrs", display_name)
}

fn decode_rule_table<T>(
    name: &str,
    mut attrs: toml::Table,
    surface: &str,
    display_name: &str,
) -> Result<T>
where
    T: DeserializeOwned,
{
    attrs.insert("name".to_string(), toml::Value::String(name.to_string()));
    toml::Value::Table(attrs)
        .try_into()
        .map_err(|e| Error::Eval {
            path: display_name.to_string(),
            message: format!("invalid {surface} for target `{name}`: {e}"),
        })
}

fn script_rule_table(
    name: &str,
    has_attrs: bool,
    script: toml::Table,
    display_name: &str,
) -> Result<toml::Table> {
    match (has_attrs, script.is_empty()) {
        (true, _) => Err(Error::Eval {
            path: display_name.to_string(),
            message: format!(
                "script target `{name}` must declare rule fields in `[target.script]`, not `[target.attrs]`"
            ),
        }),
        (false, false) => Ok(script),
        (false, true) => Ok(toml::Table::new()),
    }
}

fn string_attrs(attrs: toml::Table, display_name: &str) -> Result<BTreeMap<String, String>> {
    attrs
        .into_iter()
        .map(|(key, value)| match value {
            toml::Value::String(value) => Ok((key, value)),
            other => Err(Error::Eval {
                path: display_name.to_string(),
                message: format!("legacy target attr `{key}` must be a string, got {other}"),
            }),
        })
        .collect()
}

fn script_target(
    t: ScriptTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
    runtime: Option<RuntimeTask>,
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
    } = t;
    if path.is_some()
        && (!env.is_empty() || cwd.is_some() || !input.is_empty() || !output.is_empty())
    {
        return Err(Error::Eval {
            path: display_name.to_string(),
            message: format!(
                "file-backed script target `{name}` must declare execution metadata in FABRIK headers, not in `[target.script]`"
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
        external_package: None,
        kind: if has_runtime {
            "runtime_script"
        } else {
            "script"
        }
        .to_string(),
        name: checked_name,
        srcs,
        deps: Vec::new(),
        external_deps: Vec::new(),
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
        external_package: None,
        kind: if has_runtime {
            "runtime_script"
        } else {
            "script"
        }
        .to_string(),
        name: checked_name(name, display_name)?,
        srcs: resolve_script_inputs(&input, workspace_root, package, "", display_name)?,
        deps: Vec::new(),
        external_deps: Vec::new(),
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
        let normalized = normalize_build_dep(package, &target).map_err(|e| Error::Eval {
            path: display_name.to_string(),
            message: e.to_string(),
        })?;
        attrs.insert("runtime_target".to_string(), normalized);
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

fn normalize_script_path(package: &str, package_rel: &str) -> String {
    join_package_segments(package, package_rel)
}

fn script_relative_parent(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|parent| {
            parent
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/")
        })
        .filter(|parent| !parent.is_empty() && parent != ".")
        .unwrap_or_default()
}

fn normalize_package_relative_path(base: &str, path: &str, display_name: &str) -> Result<String> {
    let joined = if path.is_empty() || path == "." {
        base.to_string()
    } else if base.is_empty() {
        path.to_string()
    } else {
        format!("{base}/{path}")
    };
    normalize_relative_path(&joined, "package", display_name)
}

fn join_package_segments(package: &str, path: &str) -> String {
    match (package.is_empty(), path.is_empty()) {
        (true, true) => String::new(),
        (true, false) => path.to_string(),
        (false, true) => package.to_string(),
        (false, false) => format!("{package}/{path}"),
    }
}

fn normalize_relative_path(raw: &str, scope: &str, display_name: &str) -> Result<String> {
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(Error::Eval {
            path: display_name.to_string(),
            message: format!("path `{raw}` must be relative to the {scope}"),
        });
    }
    let mut parts = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(Error::Eval {
                        path: display_name.to_string(),
                        message: format!("path `{raw}` must not escape the {scope}"),
                    });
                }
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(Error::Eval {
                    path: display_name.to_string(),
                    message: format!("path `{raw}` must not escape the {scope}"),
                });
            }
        }
    }
    Ok(parts.join("/"))
}

fn resolve_script_inputs(
    inputs: &[String],
    workspace_root: &Path,
    package: &str,
    script_parent: &str,
    display_name: &str,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let package_root = if package.is_empty() {
        workspace_root.to_path_buf()
    } else {
        workspace_root.join(package)
    };
    let script_dir = if script_parent.is_empty() {
        package_root.clone()
    } else {
        package_root.join(script_parent)
    };
    for input in inputs {
        if has_glob(input) {
            let mut expanded =
                expand_script_globs(&package_root, &script_dir, input, display_name)?;
            out.append(&mut expanded);
            continue;
        }

        let package_rel = normalize_package_relative_path(script_parent, input, display_name)?;
        let abs = if package_rel.is_empty() {
            package_root.clone()
        } else {
            package_root.join(&package_rel)
        };
        if abs.is_dir() {
            for entry in walkdir::WalkDir::new(&abs)
                .into_iter()
                .filter_map(std::result::Result::ok)
            {
                let entry_path = entry.path();
                if !entry_path.is_file() {
                    continue;
                }
                let rel = entry_path
                    .strip_prefix(&package_root)
                    .map_err(|_| Error::Eval {
                        path: display_name.to_string(),
                        message: format!("input `{input}` resolved outside the package"),
                    })?
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/");
                out.push(rel);
            }
        } else {
            out.push(package_rel);
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn resolve_script_outputs(
    outputs: &[String],
    package: &str,
    script_parent: &str,
    display_name: &str,
) -> Result<Vec<String>> {
    outputs
        .iter()
        .map(|output| {
            normalize_package_relative_path(script_parent, output, display_name)
                .map(|path| normalize_script_path(package, &path))
        })
        .collect()
}

fn expand_script_globs(
    package_root: &Path,
    script_dir: &Path,
    pattern: &str,
    display_name: &str,
) -> Result<Vec<String>> {
    let abs_pattern = script_dir.join(pattern);
    let pattern_str = abs_pattern.to_str().ok_or_else(|| Error::Eval {
        path: display_name.to_string(),
        message: format!("non-utf8 glob pattern: {}", abs_pattern.display()),
    })?;
    let mut out = Vec::new();
    for entry in glob::glob(pattern_str).map_err(|e| Error::Eval {
        path: display_name.to_string(),
        message: format!("invalid glob pattern `{pattern}`: {e}"),
    })? {
        let path = entry.map_err(|e| Error::Eval {
            path: display_name.to_string(),
            message: format!("glob walk failed for `{pattern}`: {e}"),
        })?;
        if !path.is_file() {
            continue;
        }
        let rel = path
            .strip_prefix(package_root)
            .map_err(|_| Error::Eval {
                path: display_name.to_string(),
                message: format!("glob match outside package: {}", path.display()),
            })?
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        out.push(normalize_relative_path(&rel, "package", display_name)?);
        if out.len() > MAX_SCRIPT_GLOB_MATCHES {
            return Err(Error::Eval {
                path: display_name.to_string(),
                message: format!(
                    "glob `{pattern}` matched more than {MAX_SCRIPT_GLOB_MATCHES} files; narrow the pattern before using it in a script target"
                ),
            });
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn has_glob(value: &str) -> bool {
    value.contains('*') || value.contains('?') || value.contains('[')
}

fn insert_opt(attrs: &mut BTreeMap<String, String>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        attrs.insert(key.to_string(), value);
    }
}

fn insert_json_vec(attrs: &mut BTreeMap<String, String>, key: &str, value: &[String]) {
    if !value.is_empty() {
        attrs.insert(
            key.to_string(),
            serde_json::to_string(&value).expect("string vec is serializable"),
        );
    }
}

fn checked_name(name: String, display_name: &str) -> Result<String> {
    validate_target_name(&name).map_err(|e| Error::Eval {
        path: display_name.to_string(),
        message: e.to_string(),
    })?;
    Ok(name)
}

fn normalize_deps(
    deps: Vec<ManifestDep>,
    package: &str,
    display_name: &str,
) -> Result<NormalizedDeps> {
    let mut local = Vec::new();
    let mut external = Vec::new();
    for dep in deps {
        match dep {
            ManifestDep::Target(dep) => {
                local.push(normalize_build_dep(package, &dep).map_err(|e| Error::Eval {
                    path: display_name.to_string(),
                    message: e.to_string(),
                })?);
            }
            ManifestDep::External(dep) => {
                let mut entries = dep.into_iter();
                let Some((graph, spec)) = entries.next() else {
                    return Err(Error::Eval {
                        path: display_name.to_string(),
                        message: "external dependency entry must name one dependency graph"
                            .to_string(),
                    });
                };
                if entries.next().is_some() {
                    return Err(Error::Eval {
                        path: display_name.to_string(),
                        message: format!(
                            "external dependency entry for `{graph}` must name only one dependency graph"
                        ),
                    });
                }
                if !is_dependency_graph_name(&graph) {
                    return Err(Error::Eval {
                        path: display_name.to_string(),
                        message: format!(
                            "external dependency graph name `{graph}` must be a single path segment"
                        ),
                    });
                }
                let spec = serde_json::to_value(spec).map_err(|source| Error::Eval {
                    path: display_name.to_string(),
                    message: format!(
                        "external dependency entry for `{graph}` could not be converted to JSON: {source}"
                    ),
                })?;
                validate_external_spec(&graph, &spec, display_name)?;
                external.push(ExternalDependency { graph, spec });
            }
        }
    }
    Ok(NormalizedDeps { local, external })
}

/// Reject external dependency specs whose shape no plugin can ever
/// accept, at load time, with the offending file in the error. The
/// per-ecosystem shape (a bare crate name vs. a `{ product, package }`
/// table) is still resolved by each plugin, which knows what its
/// `graph` means; this only weeds out values (numbers, booleans,
/// arrays, null, empty string/table) that are unusable everywhere, so
/// the failure surfaces next to the build file instead of deep in a
/// planner.
fn validate_external_spec(graph: &str, spec: &serde_json::Value, display_name: &str) -> Result<()> {
    let usable = match spec {
        serde_json::Value::String(name) => !name.trim().is_empty(),
        serde_json::Value::Object(table) => !table.is_empty(),
        _ => false,
    };
    if usable {
        return Ok(());
    }
    Err(Error::Eval {
        path: display_name.to_string(),
        message: format!(
            "external dependency `{graph}` must be a non-empty name string \
             (e.g. {{ {graph} = \"name\" }}) or a non-empty table \
             (e.g. {{ {graph} = {{ product = \"Name\", package = \"pkg\" }} }}); got {}",
            external_spec_kind(spec)
        ),
    })
}

fn external_spec_kind(spec: &serde_json::Value) -> &'static str {
    match spec {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "an empty string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an empty table",
    }
}

fn toml_value_kind(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "string",
        toml::Value::Integer(_) => "integer",
        toml::Value::Float(_) => "float",
        toml::Value::Boolean(_) => "boolean",
        toml::Value::Datetime(_) => "datetime",
        toml::Value::Array(_) => "array",
        toml::Value::Table(_) => "table",
    }
}

fn is_dependency_graph_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':'])
}

fn resolve_srcs(
    mut srcs: Vec<String>,
    src_globs: Vec<String>,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Vec<String>> {
    let mut expanded = expand_globs(src_globs, workspace_root, package, display_name)?;
    srcs.append(&mut expanded);
    srcs.sort();
    srcs.dedup();
    Ok(srcs)
}

fn expand_globs(
    patterns: Vec<String>,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Vec<String>> {
    let pkg_dir = workspace_root.join(package);
    let mut out = Vec::new();
    for pattern in patterns {
        let abs_pattern = pkg_dir.join(&pattern);
        let pattern_str = abs_pattern.to_str().ok_or_else(|| Error::Eval {
            path: display_name.to_string(),
            message: format!("non-utf8 glob pattern: {}", abs_pattern.display()),
        })?;
        for entry in glob::glob(pattern_str).map_err(|e| Error::Eval {
            path: display_name.to_string(),
            message: format!("invalid glob pattern `{pattern}`: {e}"),
        })? {
            let path = entry.map_err(|e| Error::Eval {
                path: display_name.to_string(),
                message: format!("glob walk failed for `{pattern}`: {e}"),
            })?;
            if !path.is_file() {
                continue;
            }
            let rel = path
                .strip_prefix(&pkg_dir)
                .map_err(|_| Error::Eval {
                    path: display_name.to_string(),
                    message: format!("glob match outside package: {}", path.display()),
                })?
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            out.push(rel);
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn loads_rust_targets_from_toml() {
        let src = r#"
[[rust.library]]
name = "core"
srcs = ["src/lib.rs"]
edition = "2021"
crate_name = "core"

[[rust.binary]]
name = "cli"
srcs = ["src/main.rs"]
deps = [":core"]
"#;
        let err = load_toml_str("fabrik.toml", src).unwrap_err();
        assert!(err
            .to_string()
            .contains("target reference `:core` uses Bazel label syntax"));

        let src = src.replace("deps = [\":core\"]", "deps = [\"core\"]");
        let targets = load_toml_str("fabrik.toml", &src).unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].kind, "rust_library");
        assert_eq!(targets[0].attrs["edition"], "2021");
        assert_eq!(targets[1].kind, "rust_binary");
        assert_eq!(targets[1].deps, vec!["core".to_string()]);
    }

    #[test]
    fn loads_rule_targets_from_toml() {
        let targets = load_toml_str(
            "fabrik.toml",
            r#"
[[target]]
name = "core"
rule = "rust.library"

[target.attrs]
srcs = ["src/lib.rs"]
edition = "2021"

[[target]]
name = "cli"
rule = "rust.binary"

[target.attrs]
srcs = ["src/main.rs"]
deps = ["core"]
edition = "2021"
"#,
        )
        .unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].kind, "rust_library");
        assert_eq!(targets[0].srcs, vec!["src/lib.rs".to_string()]);
        assert_eq!(targets[1].kind, "rust_binary");
        assert_eq!(targets[1].deps, vec!["core".to_string()]);
    }

    #[test]
    fn loads_external_dependency_edges_from_toml() {
        let targets = load_toml_str(
            "fabrik.toml",
            r#"
[[rust.binary]]
name = "cli"
srcs = ["src/main.rs"]
deps = [
  "core",
  { cargo = "serde" },
  { swiftpm = { product = "ArgumentParser", package = "swift-argument-parser" } },
]
"#,
        )
        .unwrap();

        assert_eq!(targets[0].deps, vec!["core".to_string()]);
        assert_eq!(targets[0].external_deps.len(), 2);
        assert_eq!(targets[0].external_deps[0].graph, "cargo");
        assert_eq!(
            targets[0].external_deps[0].spec,
            serde_json::Value::String("serde".to_string())
        );
        assert_eq!(targets[0].external_deps[1].graph, "swiftpm");
        assert_eq!(
            targets[0].external_deps[1].spec["product"],
            serde_json::Value::String("ArgumentParser".to_string())
        );
        assert_eq!(
            targets[0].external_deps[1].spec["package"],
            serde_json::Value::String("swift-argument-parser".to_string())
        );
    }

    #[test]
    fn loads_go_binary_target() {
        let targets = load_toml_str(
            "fabrik.toml",
            r#"
[[go.binary]]
name = "server"
package = "./cmd/server"
srcs = ["go.mod", "cmd/server/main.go"]
deps = [{ go = "github.com/acme/lib" }]
"#,
        )
        .unwrap();

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, "go_binary");
        assert_eq!(targets[0].attrs["package"], "./cmd/server");
        assert_eq!(
            targets[0].srcs,
            vec!["cmd/server/main.go".to_string(), "go.mod".to_string()]
        );
        assert_eq!(targets[0].external_deps[0].graph, "go");
    }

    #[test]
    fn rejects_external_dependency_edges_with_multiple_graphs() {
        let err = load_toml_str(
            "fabrik.toml",
            r#"
[[rust.binary]]
name = "cli"
srcs = ["src/main.rs"]
deps = [{ cargo = "serde", pnpm = "react" }]
"#,
        )
        .unwrap_err();
        assert!(err
            .to_string()
            .contains("must name only one dependency graph"));
    }

    #[test]
    fn rejects_unusable_external_dep_specs_at_load_time() {
        // Each of these is valid TOML but no plugin can ever consume
        // it; the failure must name the graph and the build file
        // rather than surfacing deep in a planner.
        for (spec, want_kind) in [
            ("{ cargo = 1 }", "a number"),
            ("{ cargo = true }", "a boolean"),
            ("{ cargo = [] }", "an array"),
            ("{ cargo = \"\" }", "an empty string"),
            ("{ cargo = {} }", "an empty table"),
        ] {
            let src = format!(
                "[[rust.binary]]\nname = \"cli\"\nsrcs = [\"src/main.rs\"]\ndeps = [{spec}]\n"
            );
            let err = load_toml_str("pkg/fabrik.toml", &src)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("external dependency `cargo`") && err.contains(want_kind),
                "spec {spec} produced unexpected error: {err}"
            );
            assert!(err.contains("pkg/fabrik.toml"), "error lacks file: {err}");
        }
    }

    #[test]
    fn still_accepts_string_and_table_external_dep_specs() {
        let targets = load_toml_str(
            "fabrik.toml",
            r#"
[[rust.binary]]
name = "cli"
srcs = ["src/main.rs"]
deps = [
  { cargo = "serde" },
  { swiftpm = { product = "ArgumentParser", package = "swift-argument-parser" } },
]
"#,
        )
        .unwrap();
        assert_eq!(targets[0].external_deps.len(), 2);
    }

    #[test]
    fn rejects_dependency_entries_with_clear_type_error() {
        let err = load_toml_str(
            "fabrik.toml",
            r#"
[[rust.binary]]
name = "cli"
srcs = ["src/main.rs"]
deps = [1]
"#,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("dependency entry must be a string local target"));
        assert!(message.contains("got integer"));
    }

    #[test]
    fn loads_dependency_entries_from_toml() {
        let entries = load_dependency_entries_toml_str(
            "fabrik.toml",
            r#"
[[deps]]
name = "rust_deps"
ecosystem = "rust"
manifest = "Cargo.toml"
lockfile = "Cargo.lock"
output = ".fabrik/deps/rust_deps/fabrik.rust.lock.json"
"#,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "rust_deps");
        assert_eq!(
            entries[0].ecosystem,
            crate::dependency::DependencyEcosystem::Rust
        );
        assert_eq!(entries[0].lockfile.as_deref(), Some("Cargo.lock"));
    }

    #[test]
    fn loads_script_rule_with_runtime() {
        let targets = load_toml_str(
            "fabrik.toml",
            r#"
[[target]]
name = "dev"
rule = "script"

[target.script]
argv = ["npm", "run", "dev"]

[target.runtime]
kind = "web_server"
capabilities = ["logs", "http"]

[[target.runtime.interface]]
name = "logs"
kind = "stream"
argv = ["tail", "-f", ".fabrik/runtime/dev/stdout.log"]
"#,
        )
        .unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, "runtime_script");
        assert_eq!(targets[0].attrs["cache"], "false");
        assert_eq!(targets[0].attrs["runtime"], "web_server");
        assert!(targets[0].attrs["runtime_capabilities_json"].contains("http"));
    }

    #[test]
    fn rejects_legacy_task_with_migration_message() {
        let err = load_toml_str(
            "fabrik.toml",
            r#"
[[task]]
name = "lint"
argv = ["pnpm", "eslint", "src/"]
src_globs = ["src/**/*.ts"]
outputs = [".fabrik/out/eslint.json"]
"#,
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("`[[task]]` has been removed"));
        assert!(err.contains("`[[target]]` with `rule = \"script\"`"));
        assert!(err.contains("`srcs` / `src_globs` to `input`"));
        assert!(err.contains("`outputs` to `output`"));
    }

    #[test]
    fn loads_manifest_backed_script_rule() {
        let targets = load_toml_str(
            "fabrik.toml",
            r#"
[[target]]
name = "bundle"
rule = "script"

[target.script]
argv = ["/bin/sh", "-c", "printf hello"]
input = ["input.txt"]
output = ["dist/out.txt"]
env = ["GREETING"]
cwd = "pkg"
"#,
        )
        .unwrap();

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, "script");
        assert_eq!(targets[0].srcs, vec!["input.txt".to_string()]);
        assert_eq!(
            targets[0].attrs["script_argv_json"],
            r#"["/bin/sh","-c","printf hello"]"#
        );
        assert_eq!(targets[0].attrs["script_env_json"], "[\"GREETING\"]");
        assert_eq!(targets[0].attrs["cwd"], "pkg");
        assert_eq!(targets[0].attrs["outputs_json"], "[\"dist/out.txt\"]");
    }

    #[test]
    fn loads_script_rule_with_relative_annotations() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("pkg/scripts")).unwrap();
        std::fs::create_dir_all(tmp.path().join("pkg/src")).unwrap();
        std::fs::write(
            tmp.path().join("pkg/scripts/build.sh"),
            r#"#!/usr/bin/env bash
# FABRIK input "../src/*.ts"
# FABRIK input "../package.json"
# FABRIK output "../dist/"
# FABRIK env "NODE_ENV"
# FABRIK cwd ".."
echo hi
"#,
        )
        .unwrap();
        std::fs::write(tmp.path().join("pkg/src/main.ts"), "console.log('hi')\n").unwrap();
        std::fs::write(tmp.path().join("pkg/package.json"), "{}\n").unwrap();

        let targets = load_toml_with(
            "pkg/fabrik.toml",
            r#"
[[target]]
name = "build"
rule = "script"

[target.script]
path = "scripts/build.sh"
"#,
            tmp.path(),
            "pkg",
        )
        .unwrap();

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, "script");
        assert_eq!(
            targets[0].srcs,
            vec![
                "package.json".to_string(),
                "scripts/build.sh".to_string(),
                "src/main.ts".to_string(),
            ]
        );
        assert_eq!(targets[0].attrs["script_path"], "pkg/scripts/build.sh");
        assert_eq!(targets[0].attrs["script_runtime"], "bash");
        assert_eq!(targets[0].attrs["cwd"], "pkg");
        assert_eq!(
            targets[0].attrs["script_env_json"],
            "[\"NODE_ENV\"]".to_string()
        );
        assert_eq!(
            targets[0].attrs["outputs_json"],
            "[\"pkg/dist\"]".to_string()
        );
    }

    #[test]
    fn rejects_script_input_globs_that_match_too_many_files() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("pkg/scripts")).unwrap();
        std::fs::create_dir_all(tmp.path().join("pkg/src")).unwrap();
        std::fs::write(
            tmp.path().join("pkg/scripts/build.sh"),
            r#"#!/usr/bin/env bash
# FABRIK input "../src/*.ts"
echo hi
"#,
        )
        .unwrap();
        for index in 0..=MAX_SCRIPT_GLOB_MATCHES {
            std::fs::write(
                tmp.path().join("pkg/src").join(format!("file-{index}.ts")),
                "console.log('hi')\n",
            )
            .unwrap();
        }

        let err = load_toml_with(
            "pkg/fabrik.toml",
            r#"
[[target]]
name = "build"
rule = "script"

[target.script]
path = "scripts/build.sh"
"#,
            tmp.path(),
            "pkg",
        )
        .unwrap_err();

        assert!(err.to_string().contains(&format!(
            "matched more than {MAX_SCRIPT_GLOB_MATCHES} files"
        )));
    }

    #[test]
    fn rejects_target_attrs_for_script_rule() {
        let err = load_toml_str(
            "fabrik.toml",
            r#"
[[target]]
name = "bundle"
rule = "script"

[target.attrs]
argv = ["/bin/sh", "-c", "printf hello"]
"#,
        )
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("must declare rule fields in `[target.script]`, not `[target.attrs]`"));
    }

    #[test]
    fn loads_apple_simulator_app_rule() {
        let targets = load_toml_str(
            "fabrik.toml",
            r#"
[[target]]
name = "Demo"
rule = "apple.simulator_app"

[target.attrs]
platform = "ios"
bundle_id = "dev.fabrik.demo"
srcs = ["Sources/App.swift"]
"#,
        )
        .unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, "apple_simulator_app");
        assert_eq!(targets[0].attrs["platform"], "ios");
    }

    #[test]
    fn expands_src_globs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("pkg/src")).unwrap();
        std::fs::write(root.join("pkg/src/lib.rs"), "pub fn hi() {}").unwrap();
        std::fs::write(root.join("pkg/src/main.rs"), "fn main() {}").unwrap();
        let targets = load_toml_with(
            "pkg/fabrik.toml",
            r#"
[[rust.binary]]
name = "pkg"
src_globs = ["src/*.rs"]
"#,
            root,
            "pkg",
        )
        .unwrap();
        assert_eq!(
            targets[0].srcs,
            vec!["src/lib.rs".to_string(), "src/main.rs".to_string()]
        );
    }

    #[test]
    fn loads_apple_simulator_app() {
        let targets = load_toml_str(
            "fabrik.toml",
            r#"
[[apple.simulator_app]]
name = "Demo"
platform = "ios"
bundle_id = "dev.fabrik.demo"
srcs = ["Sources/App.swift"]
minimum_os = "17.0"
"#,
        )
        .unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, "apple_simulator_app");
        assert_eq!(targets[0].attrs["platform"], "ios");
        assert_eq!(targets[0].attrs["bundle_id"], "dev.fabrik.demo");
        assert_eq!(targets[0].attrs["minimum_os"], "17.0");
    }

    #[test]
    fn loads_apple_swift_targets() {
        let targets = load_toml_str(
            "fabrik.toml",
            r#"
[[apple.swift_library]]
name = "Greeter"
srcs = ["Sources/Greeter.swift"]
module_name = "Greeter"
minimum_os = "15.0"
swiftc_flags = ["-D", "MOCKING"]

[[apple.macos_command_line_application]]
name = "hello"
srcs = ["Sources/main.swift"]
deps = ["Greeter"]
"#,
        )
        .unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].kind, "swift_library");
        assert_eq!(targets[0].attrs["module_name"], "Greeter");
        assert!(targets[0].attrs["swiftc_flags_json"].contains("MOCKING"));
        assert_eq!(targets[1].kind, "macos_command_line_application");
        assert_eq!(targets[1].deps, vec!["Greeter".to_string()]);
    }
}
