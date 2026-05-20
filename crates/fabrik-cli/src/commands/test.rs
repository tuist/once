//! `fabrik test` - build and run a Rust test target.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use fabrik_cas::Cas;
use fabrik_core::{workspace_tool_env, Action, CacheState, ResourceRequest};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::{exit_from, Format};
use crate::commands::util::{cache_tag, find_target};
use crate::render;

#[derive(Serialize)]
struct TestSummary<'a> {
    target: &'a str,
    build_nodes: usize,
    build_cache_hits: usize,
    build_cache_misses: usize,
    test_cache: &'a str,
    exit_code: i32,
    binary: String,
    test_action_digest: String,
}

pub async fn test(
    workspace: &Path,
    cas: &Cas,
    target_id: &str,
    test_args: Vec<String>,
    format: Format,
) -> Result<ExitCode> {
    let (targets, idx) = find_target(workspace, target_id)?;
    let target = &targets[idx];
    if target.kind != "rust_test" {
        anyhow::bail!(
            "target {target_id} has kind `{}`; expected rust_test. Run `fabrik targets` to list rust_test targets",
            target.kind
        );
    }

    let built =
        fabrik_rust::build_plan(&targets, target_id, workspace).context("building test plan")?;
    let runner = crate::commands::util::runner(cas, workspace);
    let build_outcomes = runner
        .run_plan(&built.plan)
        .await
        .with_context(|| format!("building test target {target_id}"))?;

    let root_outcome = build_outcomes
        .iter()
        .find(|o| o.index == built.root_index)
        .ok_or_else(|| {
            anyhow::anyhow!("test build did not produce root outcome for {target_id}")
        })?;
    let binary = test_binary_path(&built)?;
    let test_action = test_action(workspace, &binary, test_args, root_outcome.outcome.action)?;
    let test_outcome = runner
        .run(&test_action)
        .await
        .with_context(|| format!("running test target {target_id}"))?;

    let build_hits = build_outcomes
        .iter()
        .filter(|o| o.outcome.cache == CacheState::Hit)
        .count();
    let build_misses = build_outcomes.len() - build_hits;
    let test_cache_tag = cache_tag(test_outcome.cache);
    let summary = TestSummary {
        target: target_id,
        build_nodes: build_outcomes.len(),
        build_cache_hits: build_hits,
        build_cache_misses: build_misses,
        test_cache: test_cache_tag,
        exit_code: test_outcome.result.exit_code,
        binary,
        test_action_digest: test_outcome.action.to_string(),
    };

    render_output(cas, &test_outcome, &summary, format).await?;
    Ok(exit_from(test_outcome.result.exit_code))
}

fn test_binary_path(built: &fabrik_core::BuiltPlan) -> Result<String> {
    if built.output.is_empty() {
        Err(anyhow::anyhow!(
            "rust_test {} has no declared output",
            built.root_id
        ))
    } else {
        Ok(built.output.clone())
    }
}

fn test_action(
    workspace: &Path,
    binary: &str,
    test_args: Vec<String>,
    input_digest: fabrik_cas::Digest,
) -> Result<Action> {
    let mut argv = vec![binary.to_string()];
    argv.extend(test_args);
    Ok(Action::RunCommand {
        argv,
        env: test_env(workspace)?,
        cwd: None,
        input_digest: Some(input_digest),
        outputs: vec![],
        resources: ResourceRequest::default(),
        timeout_ms: Some(300_000),
    })
}

async fn render_output(
    cas: &Cas,
    outcome: &fabrik_core::Outcome,
    summary: &TestSummary<'_>,
    format: Format,
) -> Result<()> {
    let stdout = cas.get_blob(&outcome.result.stdout).await?;
    let stderr = cas.get_blob(&outcome.result.stderr).await?;

    match format {
        Format::Human => {
            let mut out = tokio::io::stdout();
            out.write_all(&stdout).await?;
            out.flush().await?;
            let mut err = tokio::io::stderr();
            err.write_all(&stderr).await?;
            let trailer = format!(
                "fabrik: tested {label} (build {nodes} nodes, {hits} hit, {misses} miss; test cache {test_cache}, exit={exit}) -> {binary}\n",
                label = summary.target,
                nodes = summary.build_nodes,
                hits = summary.build_cache_hits,
                misses = summary.build_cache_misses,
                test_cache = summary.test_cache,
                exit = summary.exit_code,
                binary = summary.binary,
            );
            err.write_all(trailer.as_bytes()).await?;
            err.flush().await?;
        }
        Format::Json | Format::Toon => {
            let mut err = tokio::io::stderr();
            err.write_all(&stdout).await?;
            err.write_all(&stderr).await?;
            err.flush().await?;
            let mut out = tokio::io::stdout();
            out.write_all(render::structured(format, summary)?.as_bytes())
                .await?;
            out.flush().await?;
        }
    }
    Ok(())
}

fn test_env(workspace: &Path) -> Result<BTreeMap<String, String>> {
    Ok(workspace_tool_env(
        workspace,
        &[],
        &["RUST_BACKTRACE", "RUST_LOG"],
    )?)
}
