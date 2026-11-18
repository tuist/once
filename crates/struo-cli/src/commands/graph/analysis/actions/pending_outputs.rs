use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;
use struo_cas::ActionResult;
#[cfg(test)]
use struo_cas::CacheProvider;
use struo_frontend::analysis::DeclaredAction;

use super::{
    enclosing_available_output, AvailableInput, DeclaredActionsState, PendingResults, Repair,
};

#[cfg(test)]
#[path = "pending_reuse_tests.rs"]
mod reuse_tests;
#[cfg(test)]
#[path = "pending_outputs_tests.rs"]
mod tests;

#[derive(Default)]
pub(super) struct Restoration {
    pub(super) paths: BTreeSet<String>,
    ready: BTreeSet<String>,
}

fn overlaps(left: &str, right: &str) -> bool {
    Path::new(left).starts_with(right) || Path::new(right).starts_with(left)
}

#[cfg(test)]
pub(super) async fn restore(
    workspace: &Path,
    cache: &CacheProvider,
    pending: PendingResults<'_>,
    available: &BTreeMap<String, AvailableInput>,
    declared: &DeclaredAction,
    digests: Option<&super::SourceDigestCache>,
) -> Result<Restoration> {
    restore_with(
        workspace,
        pending,
        available,
        declared,
        |result| async move {
            match digests {
                Some(digests) => digests
                    .materialize_outputs(&result, workspace, cache, None)
                    .await
                    .map_err(anyhow::Error::from),
                None => struo_core::materialize_outputs(&result, workspace, cache)
                    .await
                    .map_err(anyhow::Error::from),
            }
        },
    )
    .await
}

pub(super) async fn restore_admitted(
    repair: &Repair<'_>,
    pending: PendingResults<'_>,
    available: &BTreeMap<String, AvailableInput>,
    declared: &DeclaredAction,
) -> Result<Restoration> {
    restore_with(
        repair.workspace,
        pending,
        available,
        declared,
        |result| async move { repair.restore(&result).await },
    )
    .await
}

async fn restore_with<F, W>(
    workspace: &Path,
    pending: PendingResults<'_>,
    available: &BTreeMap<String, AvailableInput>,
    declared: &DeclaredAction,
    restore: F,
) -> Result<Restoration>
where
    F: Fn(ActionResult) -> W,
    W: std::future::Future<Output = Result<()>>,
{
    let required = declared
        .inputs
        .iter()
        .filter_map(|input| enclosing_available_output(available, input))
        .map(|(path, _)| path.clone())
        .collect::<BTreeSet<_>>();
    let prior = pending
        .all()
        .flat_map(|result| result.outputs.keys().cloned())
        .collect::<BTreeSet<_>>();
    let mutations = super::touched_paths(declared)
        .into_iter()
        .filter_map(|path| match path {
            super::TouchedPath::Replaced(path) => Some(path),
            super::TouchedPath::Created(path) => {
                super::super::materialization::creation_conflicts(workspace, &path, &prior)
                    .then_some(path)
            }
        })
        .collect::<BTreeSet<_>>();
    let unselected = prior
        .iter()
        .filter(|path| !required.iter().any(|input| overlaps(path, input)))
        .cloned()
        .collect::<BTreeSet<_>>();
    let all = declared.depends_on_prior_actions
        || !super::super::materialization::disjoint_paths(workspace, &prior, &mutations)
        || !super::super::materialization::disjoint_paths(workspace, &unselected, &required);
    let mut restored = BTreeSet::new();
    for result in pending.all() {
        let outputs = result
            .outputs
            .iter()
            .filter(|(path, _)| all || required.iter().any(|input| overlaps(path, input)))
            .map(|(path, digest)| (path.clone(), *digest))
            .collect::<BTreeMap<_, _>>();
        if outputs.is_empty() {
            continue;
        }
        let result = ActionResult {
            exit_code: result.exit_code,
            stdout: result.stdout,
            stderr: result.stderr,
            outputs,
        };
        restore(result.clone()).await?;
        restored.extend(result.outputs.into_keys());
    }
    let reusable = if all {
        BTreeSet::new()
    } else {
        reusable_inputs(workspace, pending, available, &prior, &required)
    };
    let outputs = required
        .iter()
        .filter(|path| !reusable.contains(*path))
        .cloned()
        .filter_map(|path| {
            let input = &available[&path];
            (!input.materialized).then_some((path, input.blob_digest))
        })
        .collect::<BTreeMap<_, _>>();
    if !outputs.is_empty() {
        let result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs,
        };
        restore(result.clone()).await?;
        restored.retain(|path| {
            !result
                .outputs
                .keys()
                .any(|parent| path != parent && Path::new(path).starts_with(parent))
        });
        restored.extend(result.outputs.into_keys());
    }
    tracing::trace!(
        restored_outputs = restored.len(),
        prior_outputs = prior.len(),
        ordered = declared.depends_on_prior_actions,
        "materialized declared action prerequisites"
    );
    let ready = restored.union(&required).cloned().collect();
    Ok(Restoration {
        paths: restored,
        ready,
    })
}

fn reusable_inputs(
    workspace: &Path,
    pending: PendingResults<'_>,
    available: &BTreeMap<String, AvailableInput>,
    prior: &BTreeSet<String>,
    required: &BTreeSet<String>,
) -> BTreeSet<String> {
    let matching = required.iter().filter(|path| {
        pending
            .all()
            .filter_map(|result| result.outputs.get(*path))
            .next_back()
            == Some(&available[*path].blob_digest)
    });
    super::super::materialization::independent_paths(workspace, prior, matching.map(String::as_str))
        .into_iter()
        .map(str::to_owned)
        .collect()
}

pub(super) fn mark_restored(state: &mut DeclaredActionsState, restored: &Restoration) {
    for path in &restored.ready {
        if let Some(input) = state.available_inputs.get_mut(path) {
            input.materialized = true;
        }
    }
    for result in state
        .cached_results
        .iter_mut()
        .chain(state.pending_intermediate.iter_mut())
    {
        result
            .outputs
            .retain(|path, _| !restored.paths.contains(path));
    }
    state
        .cached_results
        .retain(|result| !result.outputs.is_empty());
    state
        .pending_intermediate
        .retain(|result| !result.outputs.is_empty());
}
