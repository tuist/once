//! `fabrik run` - execute the action(s) that produce a target.
//!
//! For a `rust_binary`, that action is the rustc invocation. The verb
//! is the same regardless of target type: target-specific composition
//! lives in the build-file declarations, not in the CLI.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fabrik_cas::{Cas, Digest};
use fabrik_core::{
    workspace_tool, workspace_tool_env, Action, InputDigestBuilder, ResourceRequest, RunOpts,
    Runner, WorkspacePath,
};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::{exit_from, Format, CACHE_DIR};
use crate::commands::util::{cache_tag, find_target};
use crate::render;

#[derive(Serialize)]
struct RunRecord<'a> {
    target: &'a str,
    kind: &'a str,
    action_digest: String,
    cache: &'a str,
    exit_code: i32,
    output: String,
}

struct ActionPlan {
    action: Action,
    output: String,
    output_dir: Option<PathBuf>,
}

pub async fn run(workspace: &Path, cas: &Cas, target_id: &str, format: Format) -> Result<ExitCode> {
    let (targets, idx) = find_target(workspace, target_id)?;
    let target = &targets[idx];

    if target.kind == "apple_ios_app" {
        return run_apple_ios_app(workspace, cas, target_id, &targets, target, format).await;
    }

    let plan = action_for(workspace, target)?;
    if let Some(out_dir) = &plan.output_dir {
        tokio::fs::create_dir_all(out_dir)
            .await
            .with_context(|| format!("creating output directory {}", out_dir.display()))?;
    }

    let outcome = fabrik_core::run(&plan.action, workspace, cas, RunOpts::default())
        .await
        .context("executing action")?;

    render_run_output(cas, &outcome, target_id, target, &plan.output, format).await?;
    Ok(exit_from(outcome.result.exit_code))
}

async fn run_apple_ios_app(
    workspace: &Path,
    cas: &Cas,
    target_id: &str,
    targets: &[fabrik_frontend::Target],
    target: &fabrik_frontend::Target,
    format: Format,
) -> Result<ExitCode> {
    let built =
        fabrik_apple::build_plan(targets, target_id, workspace).context("building app plan")?;
    let runner = Runner::new(cas.clone(), workspace.to_path_buf(), RunOpts::default());
    let _build_outcomes = runner
        .run_plan(&built.plan)
        .await
        .with_context(|| format!("building app target {target_id}"))?;
    let launch = fabrik_apple::launch_ios_app(target, workspace)?;
    let outcome = runner
        .run(&launch.action)
        .await
        .with_context(|| format!("launching app target {target_id}"))?;

    render_run_output(cas, &outcome, target_id, target, &launch.output, format).await?;
    Ok(exit_from(outcome.result.exit_code))
}

async fn render_run_output(
    cas: &Cas,
    outcome: &fabrik_core::Outcome,
    target_id: &str,
    target: &fabrik_frontend::Target,
    output: &str,
    format: Format,
) -> Result<()> {
    let stdout_blob = cas.get_blob(&outcome.result.stdout).await?;
    let stderr_blob = cas.get_blob(&outcome.result.stderr).await?;
    let tag = cache_tag(outcome.cache);
    let record = RunRecord {
        target: target_id,
        kind: &target.kind,
        action_digest: outcome.action.to_string(),
        cache: tag,
        exit_code: outcome.result.exit_code,
        output: output.to_string(),
    };

    match format {
        Format::Human => {
            let mut out = tokio::io::stdout();
            out.write_all(&stdout_blob).await?;
            out.flush().await?;
            let mut err = tokio::io::stderr();
            err.write_all(&stderr_blob).await?;
            let trailer = format!(
                "fabrik: ran {target_id} (cache {tag}, exit={})\n",
                outcome.result.exit_code
            );
            err.write_all(trailer.as_bytes()).await?;
            err.flush().await?;
        }
        Format::Json | Format::Toon => {
            // Subprocess stderr stays on stderr (so e.g. rustc's
            // diagnostics still flow to the terminal); the structured
            // outcome record goes to stdout where agents pick it up.
            let mut err = tokio::io::stderr();
            err.write_all(&stderr_blob).await?;
            err.flush().await?;
            let mut out = tokio::io::stdout();
            out.write_all(render::structured(format, &record)?.as_bytes())
                .await?;
            out.flush().await?;
        }
    }
    Ok(())
}

fn action_for(workspace: &Path, target: &fabrik_frontend::Target) -> Result<ActionPlan> {
    match target.kind.as_str() {
        "rust_binary" => rust_binary_action(workspace, target),
        "cargo_binary" => cargo_binary_action(workspace, target),
        "task" => task_action(workspace, target),
        other => anyhow::bail!("running `{other}` targets is not yet supported"),
    }
}

fn rust_binary_action(workspace: &Path, target: &fabrik_frontend::Target) -> Result<ActionPlan> {
    let main_src = target
        .srcs
        .first()
        .ok_or_else(|| anyhow::anyhow!("rust_binary {} has no srcs", target.id()))?;
    let src_rel = source_path(target, main_src)?;
    let out_rel = if target.package.is_empty() {
        format!("{CACHE_DIR}/out/{}", target.name)
    } else {
        format!("{CACHE_DIR}/out/{}/{}", target.package, target.name)
    };
    let input_digest = input_digest(workspace, target)?;
    let output_dir = workspace.join(CACHE_DIR).join("out").join(&target.package);
    let rustc = workspace_tool(workspace, "rustc")?;

    Ok(ActionPlan {
        action: Action::RunCommand {
            argv: vec![
                rustc,
                "--edition=2021".into(),
                format!("--crate-name={}", target.name),
                "--crate-type=bin".into(),
                "-o".into(),
                out_rel.clone(),
                src_rel.as_str().to_string(),
            ],
            env: tool_env(workspace, &["rustc"])?,
            cwd: None,
            input_digest,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: Some(120_000),
        },
        output: out_rel,
        output_dir: Some(output_dir),
    })
}

fn cargo_binary_action(workspace: &Path, target: &fabrik_frontend::Target) -> Result<ActionPlan> {
    if target.srcs.is_empty() {
        anyhow::bail!("cargo_binary {} has no srcs", target.id());
    }
    let cargo_package = target
        .attrs
        .get("cargo_package")
        .ok_or_else(|| anyhow::anyhow!("cargo_binary {} has no cargo_package", target.id()))?;
    let bin = target.attrs.get("bin").unwrap_or(&target.name);
    let input_digest = input_digest(workspace, target)?;
    let output = format!("target/debug/{bin}{}", std::env::consts::EXE_SUFFIX);
    let cargo = workspace_tool(workspace, "cargo")?;

    Ok(ActionPlan {
        action: Action::RunCommand {
            argv: vec![
                cargo,
                "build".into(),
                "--locked".into(),
                "--package".into(),
                cargo_package.to_string(),
                "--bin".into(),
                bin.to_string(),
            ],
            env: tool_env(workspace, &["cargo", "rustc"])?,
            cwd: None,
            input_digest,
            outputs: vec![],
            resources: ResourceRequest::new(2, 0),
            timeout_ms: Some(300_000),
        },
        output,
        output_dir: None,
    })
}

fn task_action(workspace: &Path, target: &fabrik_frontend::Target) -> Result<ActionPlan> {
    let argv_json = target
        .attrs
        .get("argv_json")
        .ok_or_else(|| anyhow::anyhow!("task {} has no argv", target.id()))?;
    let argv: Vec<String> = serde_json::from_str(argv_json)
        .with_context(|| format!("parsing argv for task {}", target.id()))?;
    if argv.is_empty() {
        anyhow::bail!("task {} has empty argv", target.id());
    }
    let env = match target.attrs.get("env_json") {
        Some(raw) => serde_json::from_str(raw)
            .with_context(|| format!("parsing env for task {}", target.id()))?,
        None => BTreeMap::new(),
    };
    let cwd = match target.attrs.get("cwd") {
        Some(raw) => Some(
            WorkspacePath::try_from(raw.as_str())
                .with_context(|| format!("invalid cwd for task {}", target.id()))?,
        ),
        None => None,
    };
    let outputs = match target.attrs.get("outputs_json") {
        Some(raw) => {
            let values: Vec<String> = serde_json::from_str(raw)
                .with_context(|| format!("parsing outputs for task {}", target.id()))?;
            values
                .iter()
                .map(|value| {
                    WorkspacePath::try_from(value.as_str())
                        .with_context(|| format!("invalid output `{value}` in {}", target.id()))
                })
                .collect::<Result<_>>()?
        }
        None => Vec::new(),
    };
    let cache = target
        .attrs
        .get("cache")
        .map_or(Ok(true), |raw| raw.parse::<bool>())
        .with_context(|| format!("parsing cache setting for task {}", target.id()))?;
    let timeout_ms = parse_attr::<u64>(target, "timeout_ms")?;
    let cpu_slots = parse_attr::<usize>(target, "cpu_slots")?.unwrap_or(1);
    let memory_bytes = parse_attr::<u64>(target, "memory_bytes")?.unwrap_or(0);
    let input_digest = if cache {
        input_digest(workspace, target)?
    } else {
        Some(uncached_task_digest(target))
    };

    Ok(ActionPlan {
        action: Action::RunCommand {
            argv,
            env,
            cwd,
            input_digest,
            outputs,
            resources: ResourceRequest::new(cpu_slots, memory_bytes),
            timeout_ms,
        },
        output: String::new(),
        output_dir: None,
    })
}

fn source_path(target: &fabrik_frontend::Target, src: &str) -> Result<WorkspacePath> {
    WorkspacePath::from_package_relative(&target.package, src)
        .with_context(|| format!("invalid source path `{src}` in {}", target.id()))
}

fn parse_attr<T>(target: &fabrik_frontend::Target, name: &str) -> Result<Option<T>>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    target
        .attrs
        .get(name)
        .map(|value| {
            value
                .parse::<T>()
                .with_context(|| format!("parsing {name} for {}", target.id()))
        })
        .transpose()
}

fn uncached_task_digest(target: &fabrik_frontend::Target) -> Digest {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut buf = Vec::new();
    buf.extend_from_slice(b"fabrik.task.uncached.v1\0");
    buf.extend_from_slice(target.id().as_bytes());
    buf.push(0);
    buf.extend_from_slice(&nonce.to_le_bytes());
    Digest::of_bytes(&buf)
}

/// Hash the declared sources of a target into a single digest the
/// action depends on. Two targets that disagree on either the set of
/// source paths or on any source's contents get different digests, so
/// the action cache invalidates correctly.
///
/// The encoding is the empty domain prefix followed by sorted
/// `(workspace_path, file_digest)` pairs. The empty domain matches the
/// historical wire format from when this verb predated the granular
/// rust planner; bumping it would invalidate every adopted-mode cache
/// slot in the wild.
fn input_digest(workspace: &Path, target: &fabrik_frontend::Target) -> Result<Option<Digest>> {
    if target.srcs.is_empty() {
        return Ok(None);
    }

    let mut paths: Vec<_> = target
        .srcs
        .iter()
        .map(|src| source_path(target, src))
        .collect::<Result<_>>()?;
    paths.sort_by(|a, b| a.as_str().cmp(b.as_str()));

    let mut builder = InputDigestBuilder::new(b"");
    for path in paths {
        builder
            .push_source(workspace, path.as_str())
            .with_context(|| format!("hashing source input `{path}`"))?;
    }
    Ok(Some(builder.finish()))
}

fn tool_env(workspace: &Path, tools: &[&str]) -> Result<BTreeMap<String, String>> {
    Ok(workspace_tool_env(
        workspace,
        tools,
        &["CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN"],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabrik_frontend::Target;
    use tempfile::TempDir;

    fn target(package: &str, kind: &str, srcs: &[&str]) -> Target {
        Target {
            package: package.into(),
            kind: kind.into(),
            name: "demo".into(),
            srcs: srcs.iter().map(|s| (*s).into()).collect(),
            deps: vec![],
            attrs: BTreeMap::new(),
        }
    }

    #[test]
    fn source_path_joins_package_and_rejects_escapes() {
        let t = target("crates/foo", "rust_binary", &["src/main.rs"]);
        assert_eq!(
            source_path(&t, "src/main.rs").unwrap().as_str(),
            "crates/foo/src/main.rs"
        );
        let root = target("", "rust_binary", &["main.rs"]);
        assert_eq!(source_path(&root, "main.rs").unwrap().as_str(), "main.rs");

        // A `..` in the declared src must not let the target escape its
        // own package. The frontend collects whatever the user wrote;
        // path validation lives here.
        let escape = target("pkg", "rust_binary", &["../escape.rs"]);
        assert!(source_path(&escape, "../escape.rs").is_err());
    }

    #[test]
    fn input_digest_is_none_when_no_srcs_are_declared() {
        let tmp = TempDir::new().unwrap();
        let t = target("", "rust_binary", &[]);
        assert!(input_digest(tmp.path(), &t).unwrap().is_none());
    }

    #[test]
    fn input_digest_changes_with_content_and_is_stable_across_runs() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("pkg")).unwrap();
        std::fs::write(tmp.path().join("pkg/main.rs"), b"fn main() {}").unwrap();
        let t = target("pkg", "rust_binary", &["main.rs"]);

        let first = input_digest(tmp.path(), &t).unwrap().unwrap();
        let second = input_digest(tmp.path(), &t).unwrap().unwrap();
        assert_eq!(first, second, "stable for identical content");

        // Edit the file: digest must change.
        std::fs::write(tmp.path().join("pkg/main.rs"), b"fn main() { /*!*/ }").unwrap();
        let third = input_digest(tmp.path(), &t).unwrap().unwrap();
        assert_ne!(first, third, "content change must invalidate the digest");
    }

    #[test]
    fn input_digest_is_independent_of_declared_src_order() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("pkg")).unwrap();
        std::fs::write(tmp.path().join("pkg/a.rs"), b"a").unwrap();
        std::fs::write(tmp.path().join("pkg/b.rs"), b"b").unwrap();
        let forward = target("pkg", "rust_binary", &["a.rs", "b.rs"]);
        let reversed = target("pkg", "rust_binary", &["b.rs", "a.rs"]);
        assert_eq!(
            input_digest(tmp.path(), &forward).unwrap().unwrap(),
            input_digest(tmp.path(), &reversed).unwrap().unwrap()
        );
    }

    #[test]
    fn rust_binary_action_has_expected_argv_and_output() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("pkg")).unwrap();
        std::fs::write(tmp.path().join("pkg/main.rs"), b"fn main() {}").unwrap();
        let t = target("pkg", "rust_binary", &["main.rs"]);
        let plan = rust_binary_action(tmp.path(), &t).unwrap();

        assert_eq!(plan.output, format!("{CACHE_DIR}/out/pkg/demo"));
        assert_eq!(
            plan.output_dir.as_deref(),
            Some(tmp.path().join(CACHE_DIR).join("out").join("pkg")).as_deref()
        );
        match plan.action {
            Action::RunCommand {
                argv,
                input_digest,
                cwd,
                ..
            } => {
                assert_eq!(argv[0], "rustc");
                assert!(argv.contains(&"--crate-type=bin".to_string()));
                assert!(argv.iter().any(|a| a == "--crate-name=demo"));
                assert_eq!(argv.last().map(String::as_str), Some("pkg/main.rs"));
                assert!(input_digest.is_some(), "rust_binary must hash its inputs");
                assert!(cwd.is_none(), "rust_binary runs at the workspace root");
            }
        }
    }

    #[test]
    fn rust_binary_action_root_package_drops_separator() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("main.rs"), b"fn main() {}").unwrap();
        let t = target("", "rust_binary", &["main.rs"]);
        let plan = rust_binary_action(tmp.path(), &t).unwrap();
        assert_eq!(plan.output, format!("{CACHE_DIR}/out/demo"));
    }

    #[test]
    fn cargo_binary_action_uses_attrs_and_falls_back_to_name_for_bin() {
        let mut t = target("", "cargo_binary", &["Cargo.toml"]);
        t.attrs.insert("cargo_package".into(), "fabrik-cli".into());
        // No explicit `bin`: defaults to `name`.
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), b"[package]\nname=\"x\"\n").unwrap();
        let plan = cargo_binary_action(tmp.path(), &t).unwrap();
        match plan.action {
            Action::RunCommand { argv, .. } => {
                assert_eq!(argv[0], "cargo");
                assert!(argv.contains(&"--locked".to_string()));
                assert!(argv
                    .windows(2)
                    .any(|w| w[0] == "--package" && w[1] == "fabrik-cli"));
                assert!(argv.windows(2).any(|w| w[0] == "--bin" && w[1] == "demo"));
            }
        }
        assert!(plan.output_dir.is_none());
    }

    #[test]
    fn cargo_binary_action_requires_cargo_package_attr() {
        let t = target("", "cargo_binary", &["Cargo.toml"]);
        let tmp = TempDir::new().unwrap();
        let err = cargo_binary_action(tmp.path(), &t)
            .err()
            .expect("missing cargo_package must error")
            .to_string();
        assert!(err.contains("cargo_package"), "got {err}");
    }

    #[test]
    fn action_for_rejects_unsupported_kinds() {
        let tmp = TempDir::new().unwrap();
        let t = target("", "rust_library", &["lib.rs"]);
        let err = action_for(tmp.path(), &t)
            .err()
            .expect("rust_library is unsupported")
            .to_string();
        assert!(err.contains("rust_library"), "got {err}");
    }
}
