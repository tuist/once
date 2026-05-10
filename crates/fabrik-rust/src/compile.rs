//! Per-target compilation: turn one [`fabrik_frontend::Target`] plus
//! its already-built dependency artifacts into a single rustc
//! [`fabrik_core::Action`] wrapped in a [`fabrik_core::PlanNode`].

use std::collections::BTreeMap;
use std::path::Path;

use fabrik_cas::Digest;
use fabrik_core::{Action, PlanNode, ResourceRequest, WorkspacePath};
use fabrik_frontend::Target;

use crate::artifact::{
    binary_path, default_crate_name, out_dir, proc_macro_path, rlib_path, rmeta_path, DepArtifact,
    RustKind,
};

#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    #[error("target {label} has unsupported kind `{kind}`")]
    UnsupportedKind { label: String, kind: String },
    #[error("target {label} has no srcs; declare at least the crate root")]
    NoSources { label: String },
    #[error("target {label} crate root `{root}` is not in srcs")]
    CrateRootMissing { label: String, root: String },
    #[error("target {label}: invalid path `{path}`: {source}")]
    InvalidPath {
        label: String,
        path: String,
        #[source]
        source: fabrik_core::WorkspacePathError,
    },
    #[error("target {label} declares dep `{dep}` that is not a known rust target")]
    UnknownDep { label: String, dep: String },
    #[error("failed to read source `{path}` for target {label}: {source}")]
    ReadSource {
        label: String,
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// Build the [`PlanNode`] for one target. `dep_artifacts` is keyed by
/// project-root-relative target id, for example
/// `crates/fabrik-cas/fabrik-cas`. Callers should have populated it
/// via topological iteration over the workspace's targets.
///
/// On success, returns the new [`PlanNode`] (whose `deps` field is
/// indices the caller has not yet assigned - see
/// [`crate::plan::build_plan`]) and a [`DepArtifact`] describing this
/// target's outputs so dependents can wire it in.
pub fn compile_target(
    target: &Target,
    workspace_root: &Path,
    dep_artifacts: &BTreeMap<String, DepArtifact>,
) -> Result<(PlanNode, DepArtifact), CompileError> {
    let kind = RustKind::parse(&target.kind)
        .filter(|kind| kind.is_rustc_target())
        .ok_or_else(|| CompileError::UnsupportedKind {
            label: target.id(),
            kind: target.kind.clone(),
        })?;

    if target.srcs.is_empty() {
        return Err(CompileError::NoSources { label: target.id() });
    }

    let crate_name = target
        .attrs
        .get("crate_name")
        .cloned()
        .unwrap_or_else(|| default_crate_name(&target.name));

    let edition = target
        .attrs
        .get("edition")
        .cloned()
        .unwrap_or_else(|| "2021".to_string());

    let crate_root_pkg_rel = pick_crate_root(target, kind)?;
    let crate_root_ws_rel = if target.package.is_empty() {
        crate_root_pkg_rel.clone()
    } else {
        format!("{}/{}", target.package, crate_root_pkg_rel)
    };

    let out_dir = out_dir(&target.package, &target.name);
    let mut argv: Vec<String> = vec![
        "rustc".into(),
        format!("--edition={edition}"),
        format!("--crate-name={crate_name}"),
    ];
    match kind {
        RustKind::Library => {
            argv.push("--crate-type=lib".into());
            argv.push("--emit=metadata,link".into());
        }
        RustKind::Binary => {
            argv.push("--crate-type=bin".into());
        }
        RustKind::Test => {
            argv.push("--test".into());
        }
        RustKind::ProcMacro => {
            argv.push("--crate-type=proc-macro".into());
            argv.push("--emit=metadata,link".into());
        }
        RustKind::BuildScript => unreachable!("build scripts do not compile through rustc target"),
    }
    argv.push("--out-dir".into());
    argv.push(out_dir.clone());
    // Pinned dependency search path: dependents only ever look in
    // their own dep's out_dir, but for transitive lookups by file name
    // (e.g. proc-macros that reference shared deps by `-L`) we add
    // every dep's directory.
    let mut dep_dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut build_script_outputs = Vec::new();
    for dep_label in &target.deps {
        let artifact = dep_artifacts
            .get(dep_label)
            .ok_or_else(|| CompileError::UnknownDep {
                label: target.id(),
                dep: dep_label.clone(),
            })?;
        if artifact.kind == RustKind::BuildScript {
            if let Some(path) = &artifact.build_script_outputs {
                build_script_outputs.push(path.clone());
            }
            continue;
        }
        // Libraries link via rlib; proc-macros link via the platform
        // dylib (rustc loads it dynamically at proc-macro expansion
        // time). Both paths come pre-resolved on the DepArtifact so
        // this loop stays kind-agnostic.
        match artifact.kind {
            RustKind::Library | RustKind::ProcMacro => {}
            // Binaries and tests are not linkable artifacts; depending
            // on them in Rust is unusual but harmless to model as a
            // build-order edge alone (no --extern flag emitted).
            RustKind::Binary | RustKind::Test | RustKind::BuildScript => continue,
        }
        argv.push("--extern".into());
        argv.push(format!("{}={}", artifact.crate_name, artifact.extern_path));
        dep_dirs.insert(artifact.out_dir.clone());
    }
    for dir in &dep_dirs {
        argv.push("-L".into());
        argv.push(format!("dependency={dir}"));
    }
    argv.push(crate_root_ws_rel.clone());
    if !build_script_outputs.is_empty() {
        argv = wrap_rustc_with_build_script_outputs(&argv, &build_script_outputs);
    }

    // Build the input digest. It mixes the source file digests, the
    // dep action digests, and the rust toolchain identity. Two builds
    // with byte-identical sources but different toolchains must miss
    // each other's cache.
    let input_digest = build_input_digest(target, workspace_root, dep_artifacts)?;

    // Declared outputs: what rustc will write that downstream actions
    // (or the user) need to find on disk. The runner stores these in
    // the CAS and restores them on a cache hit.
    let outputs: Vec<WorkspacePath> = output_paths(kind, &out_dir, &crate_name, &target.name)
        .into_iter()
        .map(|p| {
            WorkspacePath::try_from(p.as_str()).map_err(|source| CompileError::InvalidPath {
                label: target.id(),
                path: p.clone(),
                source,
            })
        })
        .collect::<Result<_, _>>()?;

    let action = Action::RunCommand {
        argv,
        env: tool_env(),
        cwd: None,
        input_digest: Some(input_digest),
        outputs,
        resources: ResourceRequest::default(),
        timeout_ms: Some(120_000),
    };
    let action_digest = action.digest();

    let extern_path = match kind {
        RustKind::Library => rlib_path(&out_dir, &crate_name),
        RustKind::ProcMacro => proc_macro_path(&out_dir, &crate_name),
        RustKind::BuildScript => unreachable!("build scripts do not compile through rustc target"),
        // Binaries and tests aren't usable as `--extern` deps; the
        // path is recorded for completeness but never read.
        RustKind::Binary | RustKind::Test => binary_path(&out_dir, &target.name),
    };
    let artifact = DepArtifact {
        crate_name: crate_name.clone(),
        extern_path,
        rmeta_path: rmeta_path(&out_dir, &crate_name),
        out_dir: out_dir.clone(),
        action_digest,
        kind,
        build_script_outputs: None,
    };

    let node = PlanNode {
        label: target.id(),
        action,
        // Dep indices are assigned by the plan builder; leave empty
        // here so this function stays oblivious to plan layout.
        deps: Vec::new(),
    };

    Ok((node, artifact))
}

/// Cargo convention: a library's crate root is `src/lib.rs`, a binary
/// or test's is `src/main.rs`. Users can override by setting a
/// `crate_root` attribute (package-relative). The chosen root must
/// also appear in `srcs` so the input digest covers it.
fn pick_crate_root(target: &Target, kind: RustKind) -> Result<String, CompileError> {
    if let Some(explicit) = target.attrs.get("crate_root") {
        if !target.srcs.iter().any(|s| s == explicit) {
            return Err(CompileError::CrateRootMissing {
                label: target.id(),
                root: explicit.clone(),
            });
        }
        return Ok(explicit.clone());
    }
    let candidates: &[&str] = match kind {
        RustKind::Library | RustKind::ProcMacro => &["src/lib.rs", "lib.rs"],
        RustKind::Binary | RustKind::Test => &["src/main.rs", "main.rs"],
        RustKind::BuildScript => unreachable!("build scripts do not use rustc crate roots"),
    };
    for c in candidates {
        if target.srcs.iter().any(|s| s == c) {
            return Ok((*c).to_string());
        }
    }
    // Fall back to whatever first src looks like a crate root.
    for s in &target.srcs {
        if s.ends_with("/lib.rs") || s.ends_with("/main.rs") || s == "lib.rs" || s == "main.rs" {
            return Ok(s.clone());
        }
    }
    Err(CompileError::CrateRootMissing {
        label: target.id(),
        root: match kind {
            RustKind::Library | RustKind::ProcMacro => "src/lib.rs".into(),
            RustKind::Binary | RustKind::Test => "src/main.rs".into(),
            RustKind::BuildScript => unreachable!("build scripts do not use rustc crate roots"),
        },
    })
}

fn output_paths(kind: RustKind, out_dir: &str, crate_name: &str, target_name: &str) -> Vec<String> {
    match kind {
        RustKind::Library => vec![
            rlib_path(out_dir, crate_name),
            rmeta_path(out_dir, crate_name),
        ],
        RustKind::Binary => vec![binary_path(out_dir, target_name)],
        RustKind::Test => vec![binary_path(out_dir, target_name)],
        RustKind::ProcMacro => vec![
            proc_macro_path(out_dir, crate_name),
            rmeta_path(out_dir, crate_name),
        ],
        RustKind::BuildScript => Vec::new(),
    }
}

fn wrap_rustc_with_build_script_outputs(
    rustc_argv: &[String],
    build_script_outputs: &[String],
) -> Vec<String> {
    let mut script = String::from("set -eu\nset --");
    for arg in rustc_argv {
        script.push(' ');
        script.push_str(&shell_quote(arg));
    }
    script.push('\n');
    for output in build_script_outputs {
        script.push_str("while IFS= read -r line || [ -n \"$line\" ]; do\n");
        script.push_str("  case \"$line\" in\n");
        script.push_str(
            "    cargo::rustc-cfg=*) set -- \"$@\" --cfg \"${line#cargo::rustc-cfg=}\" ;;\n",
        );
        script.push_str(
            "    cargo:rustc-cfg=*) set -- \"$@\" --cfg \"${line#cargo:rustc-cfg=}\" ;;\n",
        );
        script.push_str("    cargo::rustc-env=*) export \"${line#cargo::rustc-env=}\" ;;\n");
        script.push_str("    cargo:rustc-env=*) export \"${line#cargo:rustc-env=}\" ;;\n");
        script.push_str(
            "    cargo::rustc-link-lib=*) set -- \"$@\" -l \"${line#cargo::rustc-link-lib=}\" ;;\n",
        );
        script.push_str(
            "    cargo:rustc-link-lib=*) set -- \"$@\" -l \"${line#cargo:rustc-link-lib=}\" ;;\n",
        );
        script.push_str(
            "    cargo::rustc-link-search=*) set -- \"$@\" -L \"${line#cargo::rustc-link-search=}\" ;;\n",
        );
        script.push_str(
            "    cargo:rustc-link-search=*) set -- \"$@\" -L \"${line#cargo:rustc-link-search=}\" ;;\n",
        );
        script.push_str("  esac\n");
        script.push_str("done < ");
        script.push_str(&shell_quote(output));
        script.push('\n');
    }
    script.push_str("exec \"$@\"\n");
    vec!["/bin/sh".into(), "-c".into(), script]
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Hash sources, dep action digests, and toolchain identifiers into
/// the action's `input_digest`. This is what makes a one-line edit to
/// a leaf crate invalidate exactly the affected nodes and nothing
/// more.
fn build_input_digest(
    target: &Target,
    workspace_root: &Path,
    dep_artifacts: &BTreeMap<String, DepArtifact>,
) -> Result<Digest, CompileError> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"fabrik.rust.input.v1\0");

    // Sources: deterministic order via sorted clone.
    let mut srcs: Vec<&String> = target.srcs.iter().collect();
    srcs.sort();
    for src in srcs {
        let ws_rel = if target.package.is_empty() {
            src.clone()
        } else {
            format!("{}/{}", target.package, src)
        };
        let abs = workspace_root.join(&ws_rel);
        let bytes = std::fs::read(&abs).map_err(|source| CompileError::ReadSource {
            label: target.id(),
            path: ws_rel.clone(),
            source,
        })?;
        let digest = Digest::of_bytes(&bytes);
        buf.extend_from_slice(ws_rel.as_bytes());
        buf.push(0);
        buf.extend_from_slice(digest.as_bytes());
        buf.push(0);
    }

    // Dep action digests, in label order. We only mix in deps that
    // were declared on the target; transitive deps are already folded
    // into our direct deps' digests.
    let mut dep_labels: Vec<&String> = target.deps.iter().collect();
    dep_labels.sort();
    for label in dep_labels {
        if let Some(art) = dep_artifacts.get(label) {
            buf.extend_from_slice(b"dep:");
            buf.extend_from_slice(label.as_bytes());
            buf.push(0);
            buf.extend_from_slice(art.action_digest.as_bytes());
            buf.push(0);
        }
    }

    // Toolchain identity. RUSTUP_TOOLCHAIN pins a specific channel
    // when set (the mise-managed setup does set it). When unset, fall
    // back to "system-rustc" so two machines with the same plain
    // rustc on PATH still share a cache slot. The argv already
    // includes "rustc" verbatim, so a different binary on PATH would
    // surface as a different cache slot for env reasons too (PATH is
    // in the env).
    let toolchain = std::env::var("RUSTUP_TOOLCHAIN").unwrap_or_else(|_| "system-rustc".into());
    buf.extend_from_slice(b"toolchain:");
    buf.extend_from_slice(toolchain.as_bytes());
    buf.push(0);

    Ok(Digest::of_bytes(&buf))
}

/// Minimal env for rustc invocations. Anything not in this list is
/// unobservable from the action and therefore not part of the cache
/// key - keeping it small reduces accidental cache invalidation when
/// a developer has unrelated env set.
fn tool_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for key in [
        "PATH",
        "HOME",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
        "RUSTC",
        "RUSTC_WRAPPER",
    ] {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.into(), value);
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabrik_frontend::Target;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn write(workspace: &Path, rel: &str, body: &str) {
        let p = workspace.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn lib_target(pkg: &str, name: &str, srcs: &[&str], deps: &[&str]) -> Target {
        Target {
            package: pkg.into(),
            kind: "rust_library".into(),
            name: name.into(),
            srcs: srcs.iter().map(|s| (*s).to_string()).collect(),
            deps: deps.iter().map(|s| (*s).to_string()).collect(),
            attrs: BTreeMap::new(),
        }
    }

    fn bin_target(pkg: &str, name: &str, srcs: &[&str], deps: &[&str]) -> Target {
        Target {
            package: pkg.into(),
            kind: "rust_binary".into(),
            name: name.into(),
            srcs: srcs.iter().map(|s| (*s).to_string()).collect(),
            deps: deps.iter().map(|s| (*s).to_string()).collect(),
            attrs: BTreeMap::new(),
        }
    }

    #[test]
    fn library_emits_lib_and_metadata_outputs() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "crates/foo/src/lib.rs", "pub fn x() {}");
        let target = lib_target("crates/foo", "foo", &["src/lib.rs"], &[]);
        let (node, artifact) = compile_target(&target, tmp.path(), &BTreeMap::new()).unwrap();
        let Action::RunCommand { argv, outputs, .. } = &node.action;
        assert_eq!(argv[0], "rustc");
        assert!(argv.iter().any(|a| a == "--crate-type=lib"));
        assert!(argv.iter().any(|a| a == "--emit=metadata,link"));
        assert!(argv.iter().any(|a| a == "--crate-name=foo"));
        assert!(argv.iter().any(|a| a == "crates/foo/src/lib.rs"));
        assert_eq!(outputs.len(), 2);
        assert!(outputs.iter().any(|p| p.as_str().ends_with("libfoo.rlib")));
        assert!(outputs.iter().any(|p| p.as_str().ends_with("libfoo.rmeta")));
        assert_eq!(artifact.crate_name, "foo");
        assert_eq!(artifact.kind, RustKind::Library);
    }

    #[test]
    fn binary_with_lib_dep_passes_extern_flag() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "crates/lib/src/lib.rs", "pub fn x() {}");
        write(tmp.path(), "crates/bin/src/main.rs", "fn main() {}");
        let lib = lib_target("crates/lib", "my-lib", &["src/lib.rs"], &[]);
        let (_lib_node, lib_art) = compile_target(&lib, tmp.path(), &BTreeMap::new()).unwrap();
        let mut deps = BTreeMap::new();
        deps.insert("crates/lib/my-lib".to_string(), lib_art);
        let bin = bin_target(
            "crates/bin",
            "the-bin",
            &["src/main.rs"],
            &["crates/lib/my-lib"],
        );
        let (node, artifact) = compile_target(&bin, tmp.path(), &deps).unwrap();
        let Action::RunCommand { argv, outputs, .. } = &node.action;
        assert!(argv.iter().any(|a| a == "--crate-type=bin"));
        assert!(argv.iter().any(|a| a == "--extern"));
        // crate_name normalization: my-lib -> my_lib for `--extern`.
        let extern_idx = argv.iter().position(|a| a == "--extern").unwrap();
        assert!(argv[extern_idx + 1].starts_with("my_lib="));
        assert!(argv[extern_idx + 1].ends_with("libmy_lib.rlib"));
        assert_eq!(outputs.len(), 1);
        assert!(outputs[0]
            .as_str()
            .ends_with(&format!("the-bin{}", std::env::consts::EXE_SUFFIX)));
        assert_eq!(artifact.kind, RustKind::Binary);
    }

    #[test]
    fn unsupported_kind_is_a_clean_error() {
        let target = Target {
            package: String::new(),
            kind: "java_library".into(),
            name: "x".into(),
            srcs: vec!["X.java".into()],
            deps: vec![],
            attrs: BTreeMap::new(),
        };
        let err = compile_target(&target, Path::new("/tmp"), &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, CompileError::UnsupportedKind { .. }));
    }

    #[test]
    fn missing_crate_root_in_srcs_is_an_error() {
        let target = lib_target("pkg", "x", &["src/other.rs"], &[]);
        let err = compile_target(&target, Path::new("/tmp"), &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, CompileError::CrateRootMissing { .. }));
    }

    #[test]
    fn explicit_crate_root_attr_overrides_default() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "pkg/custom.rs", "pub fn x() {}");
        let mut target = lib_target("pkg", "x", &["custom.rs"], &[]);
        target.attrs.insert("crate_root".into(), "custom.rs".into());
        let (node, _) = compile_target(&target, tmp.path(), &BTreeMap::new()).unwrap();
        let Action::RunCommand { argv, .. } = &node.action;
        assert!(argv.iter().any(|a| a == "pkg/custom.rs"));
    }

    #[test]
    fn proc_macro_dep_uses_dylib_extern_path() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "crates/pm/src/lib.rs", "");
        write(tmp.path(), "crates/user/src/lib.rs", "");
        let pm = Target {
            package: "crates/pm".into(),
            kind: "rust_proc_macro".into(),
            name: "macros".into(),
            srcs: vec!["src/lib.rs".into()],
            deps: vec![],
            attrs: BTreeMap::new(),
        };
        let (_, pm_art) = compile_target(&pm, tmp.path(), &BTreeMap::new()).unwrap();
        assert_eq!(pm_art.kind, RustKind::ProcMacro);
        assert!(
            pm_art
                .extern_path
                .ends_with(&format!("libmacros.{}", std::env::consts::DLL_EXTENSION)),
            "extern_path was `{}`",
            pm_art.extern_path
        );
        let mut deps = BTreeMap::new();
        deps.insert("crates/pm/macros".to_string(), pm_art);
        let user = lib_target(
            "crates/user",
            "user",
            &["src/lib.rs"],
            &["crates/pm/macros"],
        );
        let (node, _) = compile_target(&user, tmp.path(), &deps).unwrap();
        let Action::RunCommand { argv, .. } = &node.action;
        let extern_idx = argv.iter().position(|a| a == "--extern").unwrap();
        assert!(argv[extern_idx + 1].starts_with("macros="));
        assert!(argv[extern_idx + 1]
            .ends_with(&format!("libmacros.{}", std::env::consts::DLL_EXTENSION)));
    }

    #[test]
    fn build_script_dep_wraps_rustc_invocation() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "pkg/src/lib.rs", "pub fn x() {}");
        let mut deps = BTreeMap::new();
        deps.insert(
            "pkg/build".to_string(),
            DepArtifact {
                crate_name: "build_build_script".into(),
                extern_path: String::new(),
                rmeta_path: String::new(),
                out_dir: String::new(),
                action_digest: Digest::of_bytes(b"build-script"),
                kind: RustKind::BuildScript,
                build_script_outputs: Some(".fabrik/out/pkg/build_build_script.out".into()),
            },
        );
        let lib = lib_target("pkg", "pkg", &["src/lib.rs"], &["pkg/build"]);
        let (node, artifact) = compile_target(&lib, tmp.path(), &deps).unwrap();
        let Action::RunCommand { argv, .. } = &node.action;
        assert_eq!(argv[0], "/bin/sh");
        assert_eq!(argv[1], "-c");
        assert!(argv[2].contains(".fabrik/out/pkg/build_build_script.out"));
        assert!(argv[2].contains("cargo::rustc-cfg="));
        assert!(!argv.iter().any(|arg| arg.contains("--extern")));
        assert_eq!(artifact.kind, RustKind::Library);
    }

    #[test]
    fn input_digest_changes_when_a_dep_changes() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "crates/lib/src/lib.rs", "pub fn x() {}");
        write(tmp.path(), "crates/bin/src/main.rs", "fn main() {}");
        let lib = lib_target("crates/lib", "lib", &["src/lib.rs"], &[]);
        let (_, lib_art_v1) = compile_target(&lib, tmp.path(), &BTreeMap::new()).unwrap();
        // Mutate the lib's source so its action digest changes.
        write(tmp.path(), "crates/lib/src/lib.rs", "pub fn y() {}");
        let (_, lib_art_v2) = compile_target(&lib, tmp.path(), &BTreeMap::new()).unwrap();
        assert_ne!(lib_art_v1.action_digest, lib_art_v2.action_digest);

        // Build the binary against each version of the lib; its
        // action digest must change to reflect the dep change.
        let mut deps_v1 = BTreeMap::new();
        deps_v1.insert("crates/lib/lib".to_string(), lib_art_v1);
        let mut deps_v2 = BTreeMap::new();
        deps_v2.insert("crates/lib/lib".to_string(), lib_art_v2);

        let bin = bin_target("crates/bin", "bin", &["src/main.rs"], &["crates/lib/lib"]);
        let (n1, _) = compile_target(&bin, tmp.path(), &deps_v1).unwrap();
        let (n2, _) = compile_target(&bin, tmp.path(), &deps_v2).unwrap();
        assert_ne!(n1.action.digest(), n2.action.digest());
    }
}
