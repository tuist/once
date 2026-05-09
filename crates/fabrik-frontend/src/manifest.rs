//! TOML build-file frontend.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::target::Target;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Manifest {
    rust: RustSection,
    cargo: CargoSection,
    apple: AppleSection,
    task: Vec<TaskTarget>,
    target: Vec<GenericTarget>,
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
struct AppleSection {
    ios_app: Vec<AppleIosAppTarget>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskTarget {
    name: String,
    argv: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    cwd: Option<String>,
    #[serde(default)]
    srcs: Vec<String>,
    #[serde(default)]
    src_globs: Vec<String>,
    #[serde(default)]
    outputs: Vec<String>,
    #[serde(default = "default_true")]
    cache: bool,
    timeout_ms: Option<u64>,
    cpu_slots: Option<usize>,
    memory_bytes: Option<u64>,
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
    deps: Vec<String>,
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
    deps: Vec<String>,
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
    deps: Vec<String>,
    crate_dir: Option<String>,
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
    deps: Vec<String>,
    executable_name: Option<String>,
    minimum_os: Option<String>,
    simulator: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GenericTarget {
    kind: String,
    name: String,
    #[serde(default)]
    srcs: Vec<String>,
    #[serde(default)]
    src_globs: Vec<String>,
    #[serde(default)]
    deps: Vec<String>,
    #[serde(default)]
    attrs: BTreeMap<String, String>,
}

pub fn load_toml_str(name: &str, src: &str) -> Result<Vec<Target>> {
    load_toml_with(name, src, Path::new("."), "")
}

pub(crate) fn load_toml_with(
    name: &str,
    src: &str,
    workspace_root: &Path,
    package: &str,
) -> Result<Vec<Target>> {
    let manifest: Manifest = toml::from_str(src).map_err(|e| Error::Parse {
        path: name.to_owned(),
        message: e.to_string(),
    })?;
    let mut targets = Vec::new();

    for t in manifest.rust.library {
        targets.push(rust_target(
            "rust_library",
            t,
            workspace_root,
            package,
            name,
        )?);
    }
    for t in manifest.rust.binary {
        targets.push(rust_target(
            "rust_binary",
            t,
            workspace_root,
            package,
            name,
        )?);
    }
    for t in manifest.rust.test {
        targets.push(rust_target("rust_test", t, workspace_root, package, name)?);
    }
    for t in manifest.rust.proc_macro {
        targets.push(rust_target(
            "rust_proc_macro",
            t,
            workspace_root,
            package,
            name,
        )?);
    }
    for t in manifest.cargo.binary {
        targets.push(cargo_binary_target(t, workspace_root, package, name)?);
    }
    for t in manifest.cargo.build_script {
        targets.push(cargo_build_script_target(t, workspace_root, package, name)?);
    }
    for t in manifest.apple.ios_app {
        targets.push(apple_ios_app_target(t, workspace_root, package, name)?);
    }
    for t in manifest.task {
        targets.push(task_target(t, workspace_root, package, name)?);
    }
    for t in manifest.target {
        targets.push(generic_target(t, workspace_root, package, name)?);
    }

    Ok(targets)
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
    Ok(Target {
        package: package.to_string(),
        kind: kind.to_string(),
        name: t.name,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: t.deps,
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
    Ok(Target {
        package: package.to_string(),
        kind: "cargo_binary".to_string(),
        name: t.name,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: t.deps,
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
    Ok(Target {
        package: package.to_string(),
        kind: "cargo_build_script".to_string(),
        name: t.name,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: t.deps,
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
    Ok(Target {
        package: package.to_string(),
        kind: "apple_ios_app".to_string(),
        name: t.name,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: t.deps,
        attrs,
    })
}

fn generic_target(
    t: GenericTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    Ok(Target {
        package: package.to_string(),
        kind: t.kind,
        name: t.name,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: t.deps,
        attrs: t.attrs,
    })
}

fn task_target(
    t: TaskTarget,
    workspace_root: &Path,
    package: &str,
    display_name: &str,
) -> Result<Target> {
    let mut attrs = BTreeMap::new();
    attrs.insert(
        "argv_json".to_string(),
        serde_json::to_string(&t.argv).expect("task argv is serializable"),
    );
    if !t.env.is_empty() {
        attrs.insert(
            "env_json".to_string(),
            serde_json::to_string(&t.env).expect("task env is serializable"),
        );
    }
    if !t.outputs.is_empty() {
        attrs.insert(
            "outputs_json".to_string(),
            serde_json::to_string(&t.outputs).expect("task outputs are serializable"),
        );
    }
    insert_opt(&mut attrs, "cwd", t.cwd);
    attrs.insert("cache".to_string(), t.cache.to_string());
    if let Some(timeout_ms) = t.timeout_ms {
        attrs.insert("timeout_ms".to_string(), timeout_ms.to_string());
    }
    if let Some(cpu_slots) = t.cpu_slots {
        attrs.insert("cpu_slots".to_string(), cpu_slots.to_string());
    }
    if let Some(memory_bytes) = t.memory_bytes {
        attrs.insert("memory_bytes".to_string(), memory_bytes.to_string());
    }
    Ok(Target {
        package: package.to_string(),
        kind: "task".to_string(),
        name: t.name,
        srcs: resolve_srcs(t.srcs, t.src_globs, workspace_root, package, display_name)?,
        deps: Vec::new(),
        attrs,
    })
}

fn insert_opt(attrs: &mut BTreeMap<String, String>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        attrs.insert(key.to_string(), value);
    }
}

const fn default_true() -> bool {
    true
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
        let targets = load_toml_str("fabrik.toml", src).unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].kind, "rust_library");
        assert_eq!(targets[0].attrs["edition"], "2021");
        assert_eq!(targets[1].kind, "rust_binary");
        assert_eq!(targets[1].deps, vec![":core".to_string()]);
    }

    #[test]
    fn expands_src_globs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("pkg/src")).unwrap();
        std::fs::write(root.join("pkg/src/lib.rs"), "pub fn hi() {}").unwrap();
        std::fs::write(root.join("pkg/src/main.rs"), "fn main() {}").unwrap();
        let targets = load_toml_with(
            "//pkg:fabrik.toml",
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
    fn loads_apple_ios_app() {
        let targets = load_toml_str(
            "fabrik.toml",
            r#"
[[apple.ios_app]]
name = "Demo"
bundle_id = "dev.fabrik.demo"
srcs = ["Sources/App.swift"]
minimum_os = "17.0"
"#,
        )
        .unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, "apple_ios_app");
        assert_eq!(targets[0].attrs["bundle_id"], "dev.fabrik.demo");
        assert_eq!(targets[0].attrs["minimum_os"], "17.0");
    }

    #[test]
    fn loads_task_target() {
        let targets = load_toml_str(
            "fabrik.toml",
            r#"
[[task]]
name = "hello"
argv = ["/bin/sh", "-c", "printf hello"]
srcs = ["input.txt"]
outputs = ["out.txt"]
cache = false
timeout_ms = 1000
cpu_slots = 2

[task.env]
GREETING = "hello"
"#,
        )
        .unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, "task");
        assert_eq!(targets[0].srcs, vec!["input.txt".to_string()]);
        assert_eq!(targets[0].attrs["cache"], "false");
        assert_eq!(targets[0].attrs["timeout_ms"], "1000");
        assert_eq!(targets[0].attrs["cpu_slots"], "2");
        assert!(targets[0].attrs["argv_json"].contains("printf hello"));
        assert!(targets[0].attrs["env_json"].contains("GREETING"));
        assert!(targets[0].attrs["outputs_json"].contains("out.txt"));
    }
}
