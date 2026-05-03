//! `fabrik run` — execute the action(s) that produce a target.
//!
//! For a `rust_binary`, that action is the rustc invocation. The verb
//! is the same regardless of target type: target-specific composition
//! lives in the build-file declarations, not in the CLI.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use fabrik_cas::{Cas, Digest};
use fabrik_core::{Action, CacheState, RunOpts, WorkspacePath};
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

    let plan = action_for(workspace, target)?;
    if let Some(out_dir) = &plan.output_dir {
        tokio::fs::create_dir_all(out_dir)
            .await
            .with_context(|| format!("creating output directory {}", out_dir.display()))?;
    }

    let outcome = fabrik_core::run(&plan.action, workspace, cas, RunOpts::default())
        .await
        .context("executing action")?;

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
        output: plan.output,
    };

    match format {
        Format::Human => {
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

    Ok(exit_from(outcome.result.exit_code))
}

fn action_for(workspace: &Path, target: &fabrik_frontend::Target) -> Result<ActionPlan> {
    match target.kind.as_str() {
        "rust_binary" => rust_binary_action(workspace, target),
        "cargo_binary" => cargo_binary_action(workspace, target),
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
            timeout_ms: Some(300_000),
        },
        output,
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
    env
}
