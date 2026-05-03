//! `fabrik run` — execute the action(s) that produce a target.
//!
//! For a `rust_binary`, that action is the rustc invocation. The verb
//! is the same regardless of target type: target-specific composition
//! lives in the build-file declarations, not in the CLI.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use fabrik_cas::Cas;
use fabrik_core::{Action, CacheState, RunOpts};
use tokio::io::AsyncWriteExt;

use crate::cli::{exit_from, Format};

pub async fn run(workspace: &Path, cas: &Cas, label: &str, format: Format) -> Result<ExitCode> {
    let targets = fabrik_frontend::load_workspace(workspace).context("loading workspace")?;
    let target = targets
        .iter()
        .find(|t| t.label() == label)
        .ok_or_else(|| anyhow::anyhow!("no target matches `{label}`"))?;

    let action = action_for(target)?;
    // The output directory must exist before rustc writes into it.
    // Pre-create it here rather than wrap the action in a shell so the
    // action's cache key stays focused on the compilation itself.
    let out_dir = workspace.join(".fabrik").join("out").join(&target.package);
    tokio::fs::create_dir_all(&out_dir)
        .await
        .with_context(|| format!("creating output directory {}", out_dir.display()))?;

    let outcome = fabrik_core::run(&action, workspace, cas, RunOpts::default())
        .await
        .context("executing action")?;

    let stderr_blob = cas.get_blob(&outcome.result.stderr).await?;
    let cache_tag = match outcome.cache {
        CacheState::Hit => "hit",
        CacheState::Miss => "miss",
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
        Format::Json => {
            // Subprocess stderr stays on stderr (so e.g. rustc's
            // diagnostics still flow to the terminal); the structured
            // outcome record goes to stdout where agents pick it up.
            let mut err = tokio::io::stderr();
            err.write_all(&stderr_blob).await?;
            err.flush().await?;
            let output_rel = if target.package.is_empty() {
                format!(".fabrik/out/{}", target.name)
            } else {
                format!(".fabrik/out/{}/{}", target.package, target.name)
            };
            let record = serde_json::json!({
                "label": label,
                "kind": target.kind,
                "action_digest": outcome.action.to_string(),
                "cache": cache_tag,
                "exit_code": outcome.result.exit_code,
                "output": output_rel,
            });
            let mut out = tokio::io::stdout();
            out.write_all(format!("{record}\n").as_bytes()).await?;
            out.flush().await?;
        }
    }

    Ok(exit_from(outcome.result.exit_code))
}

fn action_for(target: &fabrik_frontend::Target) -> Result<Action> {
    match target.kind.as_str() {
        "rust_binary" => rust_binary_action(target),
        other => anyhow::bail!("running `{other}` targets is not yet supported"),
    }
}

fn rust_binary_action(target: &fabrik_frontend::Target) -> Result<Action> {
    let main_src = target
        .srcs
        .first()
        .ok_or_else(|| anyhow::anyhow!("rust_binary {} has no srcs", target.label()))?;
    let prefix = if target.package.is_empty() {
        String::new()
    } else {
        format!("{}/", target.package)
    };
    let src_rel = format!("{prefix}{main_src}");
    let out_rel = if target.package.is_empty() {
        format!(".fabrik/out/{}", target.name)
    } else {
        format!(".fabrik/out/{}/{}", target.package, target.name)
    };

    // rustc needs at least PATH (so it can locate its own helpers like
    // the linker). HOME unblocks toolchains installed via rustup, which
    // resolves the active toolchain through `~/.rustup`. These pollute
    // the cache key across machines; hermetic toolchains land later.
    let mut env = BTreeMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".into(), path);
    }
    if let Ok(home) = std::env::var("HOME") {
        env.insert("HOME".into(), home);
    }

    Ok(Action::RunCommand {
        argv: vec![
            "rustc".into(),
            "--edition=2021".into(),
            format!("--crate-name={}", target.name),
            "--crate-type=bin".into(),
            "-o".into(),
            out_rel,
            src_rel,
        ],
        env,
        cwd: None,
        timeout_ms: Some(120_000),
    })
}
