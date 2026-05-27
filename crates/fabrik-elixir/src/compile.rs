//! Per-target compilation: turn one [`fabrik_frontend::Target`] plus
//! its already-built dep ebins into a single `elixirc`
//! [`fabrik_core::Action`] wrapped in a [`fabrik_core::PlanNode`].
//!
//! Library targets emit a single direct-argv action that compiles every
//! source into a `.ebin` directory. Binary targets wrap the compile in
//! a shell pipeline that also writes a launcher script next to the
//! `.ebin` directory so the resulting file is directly executable.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use fabrik_cas::Digest;
use fabrik_core::{
    workspace_tool, workspace_tool_env, Action, InputDigestBuilder, PlanNode, ResourceRequest,
    WorkspacePath,
};
use fabrik_frontend::Target;

use crate::artifact::{ebin_dir, escript_path, BeamArtifact, ElixirKind};

#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    #[error("target {label} has unsupported kind `{kind}`")]
    UnsupportedKind { label: String, kind: String },
    #[error("target {label} has no srcs; declare at least one .ex source")]
    NoSources { label: String },
    #[error("target {label}: invalid path `{path}`: {source}")]
    InvalidPath {
        label: String,
        path: String,
        #[source]
        source: fabrik_core::WorkspacePathError,
    },
    #[error("target {label} declares dep `{dep}` that is not a known elixir target")]
    UnknownDep { label: String, dep: String },
    #[error("failed to read source `{path}` for target {label}: {source}")]
    ReadSource {
        label: String,
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to resolve toolchain for target {label}: {source}")]
    Toolchain {
        label: String,
        #[source]
        source: fabrik_core::ToolEnvError,
    },
    #[error(
        "elixir_binary target {label} must declare an `entry` attr naming the module with main/1"
    )]
    BinaryMissingEntry { label: String },
}

/// Build the [`PlanNode`] for one target. `dep_artifacts` is keyed by
/// project-root-relative target id and must already contain entries for
/// every transitive dep (callers iterate in topo order).
///
/// Returns the [`PlanNode`] (with `deps` left empty; the plan builder
/// fills it in) and a [`BeamArtifact`] describing the produced ebin so
/// dependents can wire it into their `-pa` list.
pub fn compile_target(
    target: &Target,
    workspace_root: &Path,
    dep_artifacts: &BTreeMap<String, BeamArtifact>,
) -> Result<(PlanNode, BeamArtifact), CompileError> {
    let kind = ElixirKind::parse(&target.kind).ok_or_else(|| CompileError::UnsupportedKind {
        label: target.id(),
        kind: target.kind.clone(),
    })?;

    if target.srcs.is_empty() {
        return Err(CompileError::NoSources { label: target.id() });
    }

    let output_package = target.output_package();
    let ebin = ebin_dir(output_package.as_ref(), &target.name);
    // Resolve the `fabrik` binary the same way we resolve any other
    // workspace tool. The wrapper subcommand (`fabrik elixir-compile`)
    // routes the actual compile through the daemon when one is
    // running and otherwise falls back to direct `elixirc`. Putting
    // `fabrik` in argv[0] makes daemon presence invisible to the cache.
    let fabrik_bin =
        workspace_tool(workspace_root, "fabrik").map_err(|source| CompileError::Toolchain {
            label: target.id(),
            source,
        })?;

    let ws_srcs: Vec<String> = target
        .srcs
        .iter()
        .map(|s| ws_rel(&target.package, s))
        .collect();

    // Direct deps' ebins flow in through `-pa`; we collect them in
    // sorted order so the argv (and therefore the action digest) is
    // deterministic regardless of how the user wrote `deps`.
    let mut dep_ebins: Vec<String> = Vec::new();
    for dep_label in &target.deps {
        let artifact = dep_artifacts
            .get(dep_label)
            .ok_or_else(|| CompileError::UnknownDep {
                label: target.id(),
                dep: dep_label.clone(),
            })?;
        dep_ebins.push(artifact.ebin_dir.clone());
    }
    dep_ebins.sort();
    dep_ebins.dedup();

    let action = match kind {
        ElixirKind::Library => build_compile_action(
            target,
            workspace_root,
            &fabrik_bin,
            &ebin,
            &dep_ebins,
            &ws_srcs,
            dep_artifacts,
        )?,
        ElixirKind::Binary => {
            let entry = target
                .attrs
                .get("entry")
                .cloned()
                .ok_or_else(|| CompileError::BinaryMissingEntry { label: target.id() })?;
            let launcher = escript_path(output_package.as_ref(), &target.name);
            build_binary_action(
                target,
                workspace_root,
                dep_artifacts,
                &BinaryActionSpec {
                    fabrik_bin: &fabrik_bin,
                    ebin: &ebin,
                    dep_ebins: &dep_ebins,
                    ws_srcs: &ws_srcs,
                    entry: &entry,
                    launcher: &launcher,
                },
            )?
        }
    };

    let action_digest = action.digest();
    let node = PlanNode {
        label: target.id(),
        action,
        deps: Vec::new(),
    };
    let artifact = BeamArtifact {
        ebin_dir: ebin,
        action_digest,
        kind,
    };
    Ok((node, artifact))
}

fn build_compile_action(
    target: &Target,
    workspace_root: &Path,
    fabrik_bin: &str,
    ebin: &str,
    dep_ebins: &[String],
    ws_srcs: &[String],
    dep_artifacts: &BTreeMap<String, BeamArtifact>,
) -> Result<Action, CompileError> {
    let mut argv: Vec<String> = vec![
        fabrik_bin.to_string(),
        "elixir-compile".into(),
        "--out".into(),
        ebin.to_string(),
    ];
    for dep in dep_ebins {
        argv.push("--pa".into());
        argv.push(dep.clone());
    }
    for src in ws_srcs {
        argv.push(src.clone());
    }

    let input_digest = build_input_digest(target, workspace_root, dep_artifacts)?;
    let env = tool_env(workspace_root).map_err(|source| CompileError::Toolchain {
        label: target.id(),
        source,
    })?;
    let outputs = vec![ws_path(target, ebin)?];
    Ok(Action::RunCommand {
        argv,
        env,
        cwd: None,
        input_digest: Some(input_digest),
        outputs,
        resources: elixir_resources(),
        timeout_ms: Some(300_000),
        remote: None,
    })
}

/// Resource request shape for every elixir compile action. Reserves
/// one CPU slot plus one [`crate::ELIXIR_COMPILE_SLOT`] so the runner
/// bounds elixir-action fan-out by the size of the dedicated pool the
/// CLI publishes - which matches the daemon's own bounded queue.
fn elixir_resources() -> ResourceRequest {
    ResourceRequest::default().with_slot(crate::ELIXIR_COMPILE_SLOT, 1)
}

/// Inputs needed to assemble an `elixir.binary` action. Grouped into
/// one struct so the helper stays under the argument-count threshold
/// and the call site reads as a labelled record.
struct BinaryActionSpec<'a> {
    fabrik_bin: &'a str,
    ebin: &'a str,
    dep_ebins: &'a [String],
    ws_srcs: &'a [String],
    entry: &'a str,
    launcher: &'a str,
}

fn build_binary_action(
    target: &Target,
    workspace_root: &Path,
    dep_artifacts: &BTreeMap<String, BeamArtifact>,
    spec: &BinaryActionSpec<'_>,
) -> Result<Action, CompileError> {
    let BinaryActionSpec {
        fabrik_bin,
        ebin,
        dep_ebins,
        ws_srcs,
        entry,
        launcher,
    } = *spec;

    // Single shell action: compile the modules via the fabrik wrapper
    // (which routes through the daemon when available), then write a
    // launcher script that resolves the workspace root at run time and
    // execs `elixir` with the right code path and entry point.
    let ebin_q = shell_quote(ebin);
    let fabrik_q = shell_quote(fabrik_bin);
    let mut script = String::from("set -eu\n");
    let _ = writeln!(script, "mkdir -p {ebin_q}");
    script.push_str(&fabrik_q);
    script.push_str(" elixir-compile --out ");
    script.push_str(&ebin_q);
    for dep in dep_ebins {
        script.push_str(" --pa ");
        script.push_str(&shell_quote(dep));
    }
    for src in ws_srcs {
        script.push(' ');
        script.push_str(&shell_quote(src));
    }
    script.push('\n');

    // The launcher embeds workspace-relative `-pa` paths and discovers
    // its workspace root at startup. That keeps the cached file
    // byte-identical across machines with different absolute paths.
    let mut launcher_body = String::from(
        "#!/bin/sh\nset -e\nself=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\nws=\"$self\"\nwhile [ \"$ws\" != \"/\" ] && [ ! -d \"$ws/.fabrik\" ]; do\n  ws=\"$(dirname \"$ws\")\"\ndone\nif [ ! -d \"$ws/.fabrik\" ]; then\n  echo \"fabrik elixir launcher: could not locate workspace root\" >&2\n  exit 2\nfi\n",
    );
    launcher_body.push_str("exec elixir");
    for dep in dep_ebins {
        let _ = write!(launcher_body, " -pa \"$ws/{dep}\"");
    }
    let _ = write!(launcher_body, " -pa \"$ws/{ebin}\"");
    let entry_escaped = escape_single_quotes(entry);
    let _ = write!(
        launcher_body,
        " -e '{entry_escaped}.main(System.argv())' --"
    );
    launcher_body.push_str(" \"$@\"\n");

    let launcher_q = shell_quote(launcher);
    let _ = write!(
        script,
        "cat > {launcher_q} <<'__FABRIK_LAUNCHER__'\n{launcher_body}__FABRIK_LAUNCHER__\nchmod +x {launcher_q}\n"
    );

    let argv = vec!["/bin/sh".into(), "-c".into(), script];

    let input_digest = build_input_digest(target, workspace_root, dep_artifacts)?;
    let env = tool_env(workspace_root).map_err(|source| CompileError::Toolchain {
        label: target.id(),
        source,
    })?;
    // Order matters: the first declared output becomes `built.output`,
    // which `fabrik build` reports and `fabrik run` invokes. The
    // launcher is the user-visible artifact; the ebin directory is the
    // implementation detail dependents reach for.
    let outputs = vec![ws_path(target, launcher)?, ws_path(target, ebin)?];
    Ok(Action::RunCommand {
        argv,
        env,
        cwd: None,
        input_digest: Some(input_digest),
        outputs,
        resources: elixir_resources(),
        timeout_ms: Some(300_000),
        remote: None,
    })
}

fn ws_rel(package: &str, rel: &str) -> String {
    if package.is_empty() {
        rel.to_string()
    } else {
        format!("{package}/{rel}")
    }
}

fn ws_path(target: &Target, p: &str) -> Result<WorkspacePath, CompileError> {
    WorkspacePath::try_from(p).map_err(|source| CompileError::InvalidPath {
        label: target.id(),
        path: p.to_string(),
        source,
    })
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn escape_single_quotes(value: &str) -> String {
    value.replace('\'', "'\\''")
}

/// Hash sources, dep action digests, and toolchain identifiers into the
/// action's `input_digest`. A one-line edit to a dep's source invalidates
/// this target's cache slot because the dep's `action_digest` flows in
/// through the keyed-dep entries below.
fn build_input_digest(
    target: &Target,
    workspace_root: &Path,
    dep_artifacts: &BTreeMap<String, BeamArtifact>,
) -> Result<Digest, CompileError> {
    let mut builder = InputDigestBuilder::new(b"fabrik.elixir.input.v1\0");

    let mut srcs: Vec<&String> = target.srcs.iter().collect();
    srcs.sort();
    for src in srcs {
        let ws_rel = ws_rel(&target.package, src);
        builder
            .push_source(workspace_root, &ws_rel)
            .map_err(|source| CompileError::ReadSource {
                label: target.id(),
                path: ws_rel.clone(),
                source,
            })?;
    }

    let mut dep_labels: Vec<&String> = target.deps.iter().collect();
    dep_labels.sort();
    for label in dep_labels {
        if let Some(art) = dep_artifacts.get(label) {
            let mut keyed = b"dep:".to_vec();
            keyed.extend_from_slice(label.as_bytes());
            builder.push_keyed(&keyed, &art.action_digest);
        }
    }

    if let Some(entry) = target.attrs.get("entry") {
        let mut tag = b"entry:".to_vec();
        tag.extend_from_slice(entry.as_bytes());
        builder.push_bytes(&tag);
    }

    // Pin the elixir toolchain into the cache key. Workspaces without
    // a pinned toolchain fall back to a literal so the slot still
    // differs from any "real" toolchain identifier.
    let toolchain = fabrik_core::workspace_tool_var(workspace_root, "ELIXIR_VERSION")
        .map_err(|source| CompileError::Toolchain {
            label: target.id(),
            source,
        })?
        .unwrap_or_else(|| "system-elixir".into());
    let mut tag = b"toolchain:".to_vec();
    tag.extend_from_slice(toolchain.as_bytes());
    builder.push_bytes(&tag);

    Ok(builder.finish())
}

fn tool_env(workspace_root: &Path) -> Result<BTreeMap<String, String>, fabrik_core::ToolEnvError> {
    workspace_tool_env(
        workspace_root,
        // `fabrik` is the wrapper that the action invokes; `elixirc`
        // and `elixir` are what the wrapper exec()s on the fallback
        // path. PATH must contain all three for both modes to work.
        &["fabrik", "elixirc", "elixir"],
        &[
            "ELIXIR_VERSION",
            "ERL_LIBS",
            "MIX_ENV",
            "LANG",
            "LC_ALL",
            // Lets the wrapper find a daemon socket parked at a
            // non-default path. Declared here so it's part of the
            // action's env contract.
            "FABRIK_ELIXIR_DAEMON_SOCKET",
        ],
    )
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

    fn lib(pkg: &str, name: &str, srcs: &[&str], deps: &[&str]) -> Target {
        Target {
            package: pkg.into(),
            external_package: None,
            kind: "elixir_library".into(),
            name: name.into(),
            srcs: srcs.iter().map(|s| (*s).to_string()).collect(),
            deps: deps.iter().map(|s| (*s).to_string()).collect(),
            external_deps: Vec::new(),
            attrs: BTreeMap::new(),
        }
    }

    fn bin(pkg: &str, name: &str, srcs: &[&str], deps: &[&str], entry: &str) -> Target {
        let mut attrs = BTreeMap::new();
        attrs.insert("entry".into(), entry.into());
        Target {
            package: pkg.into(),
            external_package: None,
            kind: "elixir_binary".into(),
            name: name.into(),
            srcs: srcs.iter().map(|s| (*s).to_string()).collect(),
            deps: deps.iter().map(|s| (*s).to_string()).collect(),
            external_deps: Vec::new(),
            attrs,
        }
    }

    #[test]
    fn library_action_declares_the_elixir_compile_slot() {
        // The runner gates elixir actions by a dedicated named-slot
        // pool so the scheduler never admits more compiles than the
        // daemon (or fallback elixirc parallelism) can absorb. The
        // action's ResourceRequest is the publishing side of that
        // contract; if this assertion drifts, the runner will silently
        // permit unbounded fan-out again.
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "apps/greeter/lib/greeter.ex",
            "defmodule Greeter do\nend\n",
        );
        let target = lib("apps/greeter", "greeter", &["lib/greeter.ex"], &[]);
        let (node, _) = compile_target(&target, tmp.path(), &BTreeMap::new()).unwrap();
        let Action::RunCommand { resources, .. } = &node.action;
        assert_eq!(resources.slots.get(crate::ELIXIR_COMPILE_SLOT), Some(&1));
    }

    #[test]
    fn library_action_invokes_fabrik_elixir_compile_wrapper() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "apps/greeter/lib/greeter.ex",
            "defmodule Greeter do\nend\n",
        );
        let target = lib("apps/greeter", "greeter", &["lib/greeter.ex"], &[]);
        let (node, artifact) = compile_target(&target, tmp.path(), &BTreeMap::new()).unwrap();
        let Action::RunCommand { argv, outputs, .. } = &node.action;
        // Actions go through the `fabrik elixir-compile` wrapper so the
        // daemon (when running) is transparent to the cache.
        assert_eq!(argv[0], "fabrik");
        assert_eq!(argv[1], "elixir-compile");
        let dash_out = argv.iter().position(|a| a == "--out").unwrap();
        assert_eq!(argv[dash_out + 1], ".fabrik/out/apps/greeter/greeter.ebin");
        assert!(argv.iter().any(|a| a == "apps/greeter/lib/greeter.ex"));
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].as_str(), ".fabrik/out/apps/greeter/greeter.ebin");
        assert_eq!(artifact.kind, ElixirKind::Library);
        assert_eq!(artifact.ebin_dir, ".fabrik/out/apps/greeter/greeter.ebin");
    }

    #[test]
    fn library_with_dep_passes_pa_flag() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "apps/util/lib/util.ex",
            "defmodule Util do\nend\n",
        );
        write(tmp.path(), "apps/app/lib/app.ex", "defmodule App do\nend\n");
        let util = lib("apps/util", "util", &["lib/util.ex"], &[]);
        let (_, util_art) = compile_target(&util, tmp.path(), &BTreeMap::new()).unwrap();
        let mut deps = BTreeMap::new();
        deps.insert("apps/util/util".to_string(), util_art);
        let app = lib("apps/app", "app", &["lib/app.ex"], &["apps/util/util"]);
        let (node, _) = compile_target(&app, tmp.path(), &deps).unwrap();
        let Action::RunCommand { argv, .. } = &node.action;
        let pa = argv.iter().position(|a| a == "--pa").unwrap();
        assert_eq!(argv[pa + 1], ".fabrik/out/apps/util/util.ebin");
    }

    #[test]
    fn binary_writes_launcher_and_compiles_modules() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "cli/lib/cli.ex",
            "defmodule Cli do\n  def main(_), do: IO.puts(\"hi\")\nend\n",
        );
        let target = bin("cli", "cli", &["lib/cli.ex"], &[], "Cli");
        let (node, _) = compile_target(&target, tmp.path(), &BTreeMap::new()).unwrap();
        let Action::RunCommand { argv, outputs, .. } = &node.action;
        assert_eq!(argv[0], "/bin/sh");
        assert_eq!(argv[1], "-c");
        // The shell pipeline must compile via the wrapper and write a
        // launcher that references the entry module and workspace-root
        // marker.
        assert!(argv[2].contains("elixir-compile"));
        assert!(argv[2].contains("Cli.main(System.argv())"));
        assert!(argv[2].contains(".fabrik"));
        assert!(argv[2].contains("cli.ebin"));
        // Launcher path first so `fabrik build`/`fabrik run` treats it
        // as the target's output.
        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].as_str(), ".fabrik/out/cli/cli");
        assert_eq!(outputs[1].as_str(), ".fabrik/out/cli/cli.ebin");
    }

    #[test]
    fn binary_without_entry_attr_is_an_error() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "cli/lib/cli.ex", "");
        let mut target = bin("cli", "cli", &["lib/cli.ex"], &[], "Cli");
        target.attrs.remove("entry");
        let err = compile_target(&target, tmp.path(), &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, CompileError::BinaryMissingEntry { .. }));
    }

    #[test]
    fn unsupported_kind_is_a_clean_error() {
        let target = Target {
            package: String::new(),
            external_package: None,
            kind: "ruby_library".into(),
            name: "x".into(),
            srcs: vec!["x.rb".into()],
            deps: vec![],
            external_deps: Vec::new(),
            attrs: BTreeMap::new(),
        };
        let err = compile_target(&target, Path::new("/tmp"), &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, CompileError::UnsupportedKind { .. }));
    }

    #[test]
    fn no_sources_is_a_clean_error() {
        let target = lib("apps/foo", "foo", &[], &[]);
        let err = compile_target(&target, Path::new("/tmp"), &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, CompileError::NoSources { .. }));
    }

    #[test]
    fn dep_changes_propagate_into_action_digest() {
        // A one-line edit to a dep's source must change the dependent
        // target's action digest; this is what makes per-target caching
        // actually invalidate downstream consumers.
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "apps/util/lib/util.ex",
            "defmodule Util do\nend\n",
        );
        write(tmp.path(), "apps/app/lib/app.ex", "defmodule App do\nend\n");
        let util = lib("apps/util", "util", &["lib/util.ex"], &[]);
        let app = lib("apps/app", "app", &["lib/app.ex"], &["apps/util/util"]);

        let (_, util_v1) = compile_target(&util, tmp.path(), &BTreeMap::new()).unwrap();
        let mut deps_v1 = BTreeMap::new();
        deps_v1.insert("apps/util/util".to_string(), util_v1);
        let (app_node_v1, _) = compile_target(&app, tmp.path(), &deps_v1).unwrap();

        // Mutate the dep's source.
        write(
            tmp.path(),
            "apps/util/lib/util.ex",
            "defmodule Util do\n  def x, do: 1\nend\n",
        );
        let (_, util_v2) = compile_target(&util, tmp.path(), &BTreeMap::new()).unwrap();
        assert_ne!(
            util_v2.action_digest,
            deps_v1["apps/util/util"].action_digest
        );
        let mut deps_v2 = BTreeMap::new();
        deps_v2.insert("apps/util/util".to_string(), util_v2);
        let (app_node_v2, _) = compile_target(&app, tmp.path(), &deps_v2).unwrap();

        // The app target's action digest must change because its
        // dep-ebin reference flows through `-pa` and the dep's action
        // digest is part of the dep's ebin path-bearing record.
        assert_ne!(app_node_v1.action.digest(), app_node_v2.action.digest());
    }

    #[test]
    fn binary_action_digest_changes_with_entry_attr() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "cli/lib/cli.ex", "defmodule Cli do\nend\n");
        let a = bin("cli", "cli", &["lib/cli.ex"], &[], "Cli");
        let b = bin("cli", "cli", &["lib/cli.ex"], &[], "Other");
        let (node_a, _) = compile_target(&a, tmp.path(), &BTreeMap::new()).unwrap();
        let (node_b, _) = compile_target(&b, tmp.path(), &BTreeMap::new()).unwrap();
        assert_ne!(node_a.action.digest(), node_b.action.digest());
    }
}
