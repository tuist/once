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
use fabrik_core::{Action, CacheState, ResourceRequest, RunOpts, Runner, WorkspacePath};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::{exit_from, Format};
use crate::render;

#[derive(Serialize)]
struct RunRecord<'a> {
    label: &'a str,
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

pub async fn run(workspace: &Path, cas: &Cas, label: &str, format: Format) -> Result<ExitCode> {
    let targets = fabrik_frontend::load_workspace(workspace).context("loading workspace")?;
    let target = targets
        .iter()
        .find(|t| t.label() == label)
        .ok_or_else(|| anyhow::anyhow!("no target matches `{label}`"))?;

    if target.kind == "apple_ios_app" {
        return run_apple_ios_app(workspace, cas, label, &targets, target, format).await;
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

    render_run_output(cas, &outcome, label, target, &plan.output, format).await?;
    Ok(exit_from(outcome.result.exit_code))
}

async fn run_apple_ios_app(
    workspace: &Path,
    cas: &Cas,
    label: &str,
    targets: &[fabrik_frontend::Target],
    target: &fabrik_frontend::Target,
    format: Format,
) -> Result<ExitCode> {
    let built = fabrik_apple::build_plan(targets, label, workspace).context("building app plan")?;
    let runner = Runner::new(cas.clone(), workspace.to_path_buf(), RunOpts::default());
    let _build_outcomes = runner
        .run_plan(&built.plan)
        .await
        .with_context(|| format!("building app target {label}"))?;
    let launch = fabrik_apple::launch_ios_app(target, workspace)?;
    let outcome = runner
        .run(&launch.action)
        .await
        .with_context(|| format!("launching app target {label}"))?;

    render_run_output(cas, &outcome, label, target, &launch.output, format).await?;
    Ok(exit_from(outcome.result.exit_code))
}

async fn render_run_output(
    cas: &Cas,
    outcome: &fabrik_core::Outcome,
    label: &str,
    target: &fabrik_frontend::Target,
    output: &str,
    format: Format,
) -> Result<()> {
    let stdout_blob = cas.get_blob(&outcome.result.stdout).await?;
    let stderr_blob = cas.get_blob(&outcome.result.stderr).await?;
    let cache_tag = match outcome.cache {
        CacheState::Hit => "hit",
        CacheState::Miss => "miss",
    };
    let record = RunRecord {
        label,
        kind: &target.kind,
        action_digest: outcome.action.to_string(),
        cache: cache_tag,
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
                "fabrik: ran {label} (cache {cache_tag}, exit={})\n",
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
        .ok_or_else(|| anyhow::anyhow!("rust_binary {} has no srcs", target.label()))?;
    let src_rel = source_path(target, main_src)?;
    let out_rel = if target.package.is_empty() {
        format!(".fabrik/out/{}", target.name)
    } else {
        format!(".fabrik/out/{}/{}", target.package, target.name)
    };
    let input_digest = input_digest(workspace, target)?;
    let output_dir = workspace.join(".fabrik").join("out").join(&target.package);

    Ok(ActionPlan {
        action: Action::RunCommand {
            argv: vec![
                "rustc".into(),
                "--edition=2021".into(),
                format!("--crate-name={}", target.name),
                "--crate-type=bin".into(),
                "-o".into(),
                out_rel.clone(),
                src_rel.as_str().to_string(),
            ],
            env: tool_env(),
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
        anyhow::bail!("cargo_binary {} has no srcs", target.label());
    }
    let cargo_package = target
        .attrs
        .get("cargo_package")
        .ok_or_else(|| anyhow::anyhow!("cargo_binary {} has no cargo_package", target.label()))?;
    let bin = target.attrs.get("bin").unwrap_or(&target.name);
    let input_digest = input_digest(workspace, target)?;
    let output = format!("target/debug/{bin}{}", std::env::consts::EXE_SUFFIX);

    Ok(ActionPlan {
        action: Action::RunCommand {
            argv: vec![
                "cargo".into(),
                "build".into(),
                "--locked".into(),
                "--package".into(),
                cargo_package.to_string(),
                "--bin".into(),
                bin.to_string(),
            ],
            env: tool_env(),
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
        .ok_or_else(|| anyhow::anyhow!("task {} has no argv", target.label()))?;
    let argv: Vec<String> = serde_json::from_str(argv_json)
        .with_context(|| format!("parsing argv for task {}", target.label()))?;
    if argv.is_empty() {
        anyhow::bail!("task {} has empty argv", target.label());
    }
    let env = match target.attrs.get("env_json") {
        Some(raw) => serde_json::from_str(raw)
            .with_context(|| format!("parsing env for task {}", target.label()))?,
        None => BTreeMap::new(),
    };
    let cwd = match target.attrs.get("cwd") {
        Some(raw) => Some(
            WorkspacePath::try_from(raw.as_str())
                .with_context(|| format!("invalid cwd for task {}", target.label()))?,
        ),
        None => None,
    };
    let outputs = match target.attrs.get("outputs_json") {
        Some(raw) => {
            let values: Vec<String> = serde_json::from_str(raw)
                .with_context(|| format!("parsing outputs for task {}", target.label()))?;
            values
                .iter()
                .map(|value| {
                    WorkspacePath::try_from(value.as_str())
                        .with_context(|| format!("invalid output `{value}` in {}", target.label()))
                })
                .collect::<Result<_>>()?
        }
        None => Vec::new(),
    };
    let cache = target
        .attrs
        .get("cache")
        .map_or(Ok(true), |raw| raw.parse::<bool>())
        .with_context(|| format!("parsing cache setting for task {}", target.label()))?;
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
    let rel = if target.package.is_empty() {
        src.to_string()
    } else {
        format!("{}/{src}", target.package)
    };
    WorkspacePath::try_from(rel.as_str())
        .with_context(|| format!("invalid source path `{src}` in {}", target.label()))
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
                .with_context(|| format!("parsing {name} for {}", target.label()))
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
    buf.extend_from_slice(target.label().as_bytes());
    buf.push(0);
    buf.extend_from_slice(&nonce.to_le_bytes());
    Digest::of_bytes(&buf)
}

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

    let mut buf = Vec::new();
    for path in paths {
        let bytes = std::fs::read(path.resolve(workspace))
            .with_context(|| format!("reading source input `{path}`"))?;
        let digest = Digest::of_bytes(&bytes);
        buf.extend_from_slice(path.as_str().as_bytes());
        buf.push(0);
        buf.extend_from_slice(digest.as_bytes());
        buf.push(0);
    }
    Ok(Some(Digest::of_bytes(&buf)))
}

fn tool_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for key in ["PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME"] {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.into(), value);
        }
    }
    for (key, value) in std::env::vars() {
        if key.starts_with("MISE_") {
            env.insert(key, value);
        }
    }
    env
}
