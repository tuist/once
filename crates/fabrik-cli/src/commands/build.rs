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
use fabrik_cas::CacheProvider;
use fabrik_core::CacheState;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::{Format, Output};
use crate::commands::util::cache_tag;
use crate::planner::plan_for_target;
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

pub async fn build(
    workspace: &Path,
    cache: &CacheProvider,
    target: &str,
    output: Output,
) -> Result<ExitCode> {
    let targets = fabrik_frontend::load_workspace(workspace).context("loading workspace")?;
    let built = plan_for_target(&targets, target, workspace).context("building plan")?;
    let runner = crate::commands::util::runner(cache, workspace);

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

    match output.format {
        Format::Human => {
            if output.show_human_trailers() {
                let mut err = tokio::io::stderr();
                for o in &outcomes {
                    let info = &built.nodes[o.index];
                    // Right-pad single-letter "hit" so columns line up.
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
        }
        Format::Json | Format::Toon => {
            let mut err = tokio::io::stderr();
            for o in &outcomes {
                let info = &built.nodes[o.index];
                let record = NodeRecord {
                    label: &o.label,
                    kind: &info.kind,
                    cache: cache_tag(o.outcome.cache),
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
            out.write_all(render::structured(output.format, &summary)?.as_bytes())
                .await?;
            out.flush().await?;
        }
    }

    Ok(ExitCode::SUCCESS)
}
