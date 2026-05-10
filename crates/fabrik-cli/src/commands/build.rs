//! `fabrik build` - compile a target via the granular per-crate
//! action graph.
//!
//! Resolves the workspace's build files, expands the requested target's
//! transitive deps into a [`fabrik_core::Plan`], and runs the plan through
//! the shared cache-aware [`fabrik_core::Runner`]. Granular targets expand
//! into one or more actions; a one-line edit in a leaf target invalidates
//! only its node and the nodes that transitively depend on it. The wire-up
//! to remote execution is the same plan, executed by a different runner.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use fabrik_cas::Cas;
use fabrik_core::{CacheState, Plan, RunOpts, Runner};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::Format;
use crate::render;

#[derive(Serialize)]
struct BuildSummary<'a> {
    target: &'a str,
    nodes: usize,
    cache_hits: usize,
    cache_misses: usize,
    output: String,
}

#[derive(Serialize)]
struct NodeRecord<'a> {
    label: &'a str,
    kind: &'a str,
    cache: &'a str,
    action_digest: String,
}

pub async fn build(workspace: &Path, cas: &Cas, target: &str, format: Format) -> Result<ExitCode> {
    let targets = fabrik_frontend::load_workspace(workspace).context("loading workspace")?;
    let built = build_plan(&targets, target, workspace).context("building plan")?;
    let runner = Runner::new(cas.clone(), workspace.to_path_buf(), RunOpts::default());

    let outcomes = runner
        .run_plan(&built.plan)
        .await
        .with_context(|| format!("executing plan for {target}"))?;

    let cache_hits = outcomes
        .iter()
        .filter(|o| o.outcome.cache == CacheState::Hit)
        .count();
    let cache_misses = outcomes.len() - cache_hits;

    let output_path = built.output.clone();

    match format {
        Format::Human => {
            let mut err = tokio::io::stderr();
            for o in &outcomes {
                let info = &built.nodes[o.index];
                let tag = match o.outcome.cache {
                    CacheState::Hit => "hit ",
                    CacheState::Miss => "miss",
                };
                let line = format!(
                    "fabrik: [{tag}] {kind:<16} {label}\n",
                    kind = info.kind,
                    label = o.label,
                );
                err.write_all(line.as_bytes()).await?;
            }
            let trailer = format!(
                "fabrik: built {target} ({n} nodes, {hits} hit, {miss} miss) -> {out}\n",
                n = outcomes.len(),
                hits = cache_hits,
                miss = cache_misses,
                out = output_path,
            );
            err.write_all(trailer.as_bytes()).await?;
            err.flush().await?;
        }
        Format::Json | Format::Toon => {
            let mut err = tokio::io::stderr();
            for o in &outcomes {
                let info = &built.nodes[o.index];
                let cache_tag = match o.outcome.cache {
                    CacheState::Hit => "hit",
                    CacheState::Miss => "miss",
                };
                let record = NodeRecord {
                    label: &o.label,
                    kind: &info.kind,
                    cache: cache_tag,
                    action_digest: o.outcome.action.to_string(),
                };
                let line = serde_json::to_string(&record)? + "\n";
                err.write_all(line.as_bytes()).await?;
            }
            err.flush().await?;
            let summary = BuildSummary {
                target,
                nodes: outcomes.len(),
                cache_hits,
                cache_misses,
                output: output_path,
            };
            let mut out = tokio::io::stdout();
            out.write_all(render::structured(format, &summary)?.as_bytes())
                .await?;
            out.flush().await?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

struct BuiltCliPlan {
    plan: Plan,
    nodes: Vec<NodeInfo>,
    output: String,
}

struct NodeInfo {
    kind: String,
}

fn build_plan(
    targets: &[fabrik_frontend::Target],
    target_id: &str,
    workspace: &Path,
) -> Result<BuiltCliPlan> {
    let target = targets
        .iter()
        .find(|t| t.id() == target_id)
        .ok_or_else(|| anyhow::anyhow!("no target matches `{target_id}`"))?;
    if fabrik_apple::supports_kind(&target.kind) {
        let built = fabrik_apple::build_plan(targets, target_id, workspace)?;
        Ok(BuiltCliPlan {
            plan: built.plan,
            nodes: built
                .nodes
                .into_iter()
                .map(|n| NodeInfo { kind: n.kind })
                .collect(),
            output: built.output,
        })
    } else {
        let built = fabrik_rust::build_plan(targets, target_id, workspace)?;
        let fabrik_core::Action::RunCommand { outputs, .. } =
            &built.plan.nodes[built.root_index].action;
        let output = outputs
            .first()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default();
        Ok(BuiltCliPlan {
            plan: built.plan,
            nodes: built
                .nodes
                .into_iter()
                .map(|n| NodeInfo {
                    kind: n.kind.as_str().to_string(),
                })
                .collect(),
            output,
        })
    }
}
