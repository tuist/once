mod task;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fabrik_cas::Digest;
use fabrik_core::{
    workspace_tool, workspace_tool_env, Action, InputDigestBuilder, ResourceRequest, WorkspacePath,
};

use crate::cli::CACHE_DIR;

pub(super) struct ActionPlan {
    pub(super) action: Action,
    pub(super) output: String,
    pub(super) output_dir: Option<PathBuf>,
}

pub(super) fn action_for(workspace: &Path, target: &fabrik_frontend::Target) -> Result<ActionPlan> {
    match target.kind.as_str() {
        "rust_binary" => rust_binary_action(workspace, target),
        "cargo_binary" => cargo_binary_action(workspace, target),
        "task" | "runtime_task" | "runner_task" => task::task_action(workspace, target),
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
            input_digest: input_digest(workspace, target)?,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: Some(120_000),
        },
        output: out_rel,
        output_dir: Some(workspace.join(CACHE_DIR).join("out").join(&target.package)),
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
            input_digest: input_digest(workspace, target)?,
            outputs: vec![],
            resources: ResourceRequest::new(2, 0),
            timeout_ms: Some(300_000),
        },
        output: format!("target/debug/{bin}{}", std::env::consts::EXE_SUFFIX),
        output_dir: None,
    })
}

fn source_path(target: &fabrik_frontend::Target, src: &str) -> Result<WorkspacePath> {
    WorkspacePath::from_package_relative(&target.package, src)
        .with_context(|| format!("invalid source path `{src}` in {}", target.id()))
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
