//! Walks the graph in dependency order and executes the actions each
//! rule impl declares.
//!
//! For every target the driver calls [`once_frontend::analysis::analyze_target`].
//! When the rule has an `impl` callable the analysis returns a list of
//! `DeclaredAction`s plus a provider record; we materialise each
//! declared command as a cacheable `Action`, run it through
//! `once_core::run_with_cache`, and pass the resulting provider down
//! to consumers. When the rule has no `impl` declared in the prelude
//! (the bundle and test rules at present) the driver returns `None`
//! so the caller can fall back to its placeholder path.
//!
//! This module has no Apple-specific logic: it consults the prelude
//! via `rule_has_impl` to know which kinds run through analysis, and
//! the analysis layer is fed everything it needs through generic
//! starlark globals. Dep providers and dep action digests are carried
//! Buck2/Bazel-style so a parent's input digest composes its deps'
//! action digests.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use once_cas::{CacheProvider, Digest};
use once_core::{
    Action, InputDigestBuilder, OutputSymlinkMode, ResourceRequest, RunOpts, WorkspacePath,
};
use once_frontend::analysis::{AnalysisEngine, AnalysisResult, DeclaredAction};
use once_frontend::GraphTarget;
use serde_json::Value as JsonValue;
use tokio::task::JoinSet;

/// Per-target outcome cached during a single command invocation.
///
/// Deliberately not `Clone`: each outcome has exactly one owner at a
/// time — first the producing build task, then `outcomes` in
/// [`BuildSession::build_reachable`], and finally either the
/// `dep_providers` of the last reader (move) or the returning caller
/// (also a move via `outcomes.remove`).
#[derive(Debug)]
pub(super) struct BuildOutcome {
    pub provider: JsonValue,
    pub action_digest: Digest,
    pub outputs: Vec<String>,
    pub cache_tag: &'static str,
}

/// Command-scoped graph build session.
///
/// The session owns the target id map and analysis engine so one graph
/// command does not repeatedly parse rule metadata or linearly scan the
/// graph for every dependency edge.
pub(super) struct BuildSession {
    workspace: PathBuf,
    cache: CacheProvider,
    /// Graph targets are wrapped in `Arc` so spawned tasks can hold a
    /// cheap shared handle (refcount bump) instead of deep-cloning the
    /// whole `GraphTarget` (which itself owns several `Vec`s and a
    /// `BTreeMap`) once for the analysis task and again for action
    /// execution.
    targets: HashMap<String, Arc<GraphTarget>>,
    analyzer: AnalysisEngine,
}

impl BuildSession {
    pub(super) fn new(
        workspace: &Path,
        cache: &CacheProvider,
        graph: &[GraphTarget],
    ) -> Result<Self> {
        Ok(Self::new_with_analyzer(
            workspace,
            cache,
            graph,
            AnalysisEngine::new()?,
        ))
    }

    fn new_with_analyzer(
        workspace: &Path,
        cache: &CacheProvider,
        graph: &[GraphTarget],
        analyzer: AnalysisEngine,
    ) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            cache: cache.clone(),
            targets: graph
                .iter()
                .map(|target| (target.label.id.clone(), Arc::new(target.clone())))
                .collect(),
            analyzer,
        }
    }

    /// Build a target and the impl-backed portion of its dependency
    /// closure. Returns `Ok(None)` when the target's own rule has no
    /// impl, allowing callers to fall back to placeholder actions.
    pub(super) async fn build_with_impl(
        &self,
        target: &GraphTarget,
    ) -> Result<Option<BuildOutcome>> {
        if !self.analyzer.rule_has_impl(&target.kind) {
            // Placeholder rules keep their existing shell scripts; we
            // don't walk their deps because the placeholder doesn't
            // consume them yet.
            return Ok(None);
        }

        let reachable = self.reachable_impl_targets(target);
        let outcome = self.build_reachable(&target.label.id, &reachable).await?;
        Ok(Some(outcome))
    }

    pub(super) async fn run_with_impl(
        &self,
        target: &GraphTarget,
        capability: &str,
    ) -> Result<Option<BuildOutcome>> {
        if !self.analyzer.rule_has_impl(&target.kind) {
            return Ok(None);
        }

        let dep_outcomes = self.build_direct_impl_deps(target).await?;
        let mut dep_providers = Vec::with_capacity(dep_outcomes.len());
        let mut dep_action_digests = Vec::with_capacity(dep_outcomes.len());
        for (dep_id, outcome) in dep_outcomes {
            dep_action_digests.push((dep_id, outcome.action_digest));
            dep_providers.push(outcome.provider);
        }

        let analysis = self
            .analyzer
            .analyze_target_capability(target, &self.workspace, &dep_providers, capability)
            .with_context(|| format!("analysing {}", target.label.id))?;
        let outcome = run_declared_actions(
            &self.workspace,
            &self.cache,
            target,
            analysis,
            &dep_action_digests,
        )
        .await
        .with_context(|| format!("executing {capability} for {}", target.label.id))?;
        Ok(Some(outcome))
    }

    fn reachable_impl_targets(&self, target: &GraphTarget) -> HashSet<String> {
        self.reachable_impl_targets_from([target.label.id.clone()])
    }

    fn reachable_impl_deps(&self, target: &GraphTarget) -> HashSet<String> {
        let roots = target.deps.iter().filter_map(|dep_id| {
            let dep = self.targets.get(dep_id)?;
            self.analyzer
                .rule_has_impl(&dep.kind)
                .then(|| dep_id.clone())
        });
        self.reachable_impl_targets_from(roots)
    }

    fn reachable_impl_targets_from(
        &self,
        roots: impl IntoIterator<Item = String>,
    ) -> HashSet<String> {
        let mut reachable = HashSet::new();
        let mut stack = roots.into_iter().collect::<Vec<_>>();
        while let Some(target_id) = stack.pop() {
            // Check membership before insert so the owned `target_id`
            // moves into the set instead of being cloned.
            if reachable.contains(&target_id) {
                continue;
            }
            let target = self.targets.get(&target_id).cloned();
            reachable.insert(target_id);
            let Some(target) = target else {
                continue;
            };
            for dep_id in &target.deps {
                let Some(dep) = self.targets.get(dep_id) else {
                    continue;
                };
                if self.analyzer.rule_has_impl(&dep.kind) {
                    stack.push(dep_id.clone());
                }
            }
        }
        reachable
    }

    async fn build_direct_impl_deps(
        &self,
        target: &GraphTarget,
    ) -> Result<Vec<(String, BuildOutcome)>> {
        let reachable = self.reachable_impl_deps(target);
        if reachable.is_empty() {
            return Ok(Vec::new());
        }

        let direct_deps = target
            .deps
            .iter()
            .filter(|dep_id| reachable.contains(*dep_id))
            .cloned()
            .collect::<Vec<_>>();
        let retained = direct_deps.iter().cloned().collect::<HashSet<_>>();
        let mut outcomes = self
            .build_reachable_retaining(&target.label.id, &reachable, &retained)
            .await?;
        direct_deps
            .into_iter()
            .map(|dep_id| {
                let outcome = outcomes
                    .remove(&dep_id)
                    .with_context(|| format!("missing build outcome for dependency `{dep_id}`"))?;
                Ok((dep_id, outcome))
            })
            .collect()
    }

    async fn build_reachable(
        &self,
        root_id: &str,
        reachable: &HashSet<String>,
    ) -> Result<BuildOutcome> {
        let retained = HashSet::from([root_id.to_string()]);
        let mut outcomes = self
            .build_reachable_retaining(root_id, reachable, &retained)
            .await?;
        outcomes
            .remove(root_id)
            .with_context(|| format!("missing build outcome for `{root_id}`"))
    }

    async fn build_reachable_retaining(
        &self,
        root_id: &str,
        reachable: &HashSet<String>,
        retained: &HashSet<String>,
    ) -> Result<HashMap<String, BuildOutcome>> {
        let mut remaining_deps: HashMap<String, usize> = HashMap::new();
        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
        for target_id in reachable {
            let target = self
                .targets
                .get(target_id)
                .with_context(|| format!("target `{target_id}` vanished from graph"))?;
            let mut count = 0;
            for dep_id in &target.deps {
                if reachable.contains(dep_id) {
                    count += 1;
                    dependents
                        .entry(dep_id.clone())
                        .or_default()
                        .push(target_id.clone());
                }
            }
            remaining_deps.insert(target_id.clone(), count);
        }

        // Reader count tracks how many dependents still need to read
        // each outcome's provider. When it reaches zero we move the
        // outcome out of the map instead of cloning its provider, so
        // the last reader takes sole ownership of the (potentially
        // large) `JsonValue` tree.
        let mut remaining_readers: HashMap<String, usize> = dependents
            .iter()
            .map(|(target_id, deps)| (target_id.clone(), deps.len()))
            .collect();
        for target_id in retained {
            *remaining_readers.entry(target_id.clone()).or_default() += 1;
        }

        let mut ready = remaining_deps
            .iter()
            .filter_map(|(target_id, count)| (*count == 0).then_some(target_id.clone()))
            .collect::<VecDeque<_>>();
        let mut running = JoinSet::new();
        let mut outcomes: HashMap<String, BuildOutcome> = HashMap::new();
        let mut completed = 0;

        self.spawn_ready(
            &mut ready,
            &mut outcomes,
            &mut remaining_readers,
            reachable,
            &mut running,
        )?;
        while completed < reachable.len() {
            if running.is_empty() {
                anyhow::bail!("cycle detected while building graph target `{root_id}`");
            }

            let joined = running
                .join_next()
                .await
                .context("build task set ended unexpectedly")?;
            let (target_id, outcome) = joined.context("joining graph build task")??;
            outcomes.insert(target_id.clone(), outcome);
            completed += 1;

            if let Some(next_targets) = dependents.get(&target_id) {
                for next_id in next_targets {
                    let count = remaining_deps
                        .get_mut(next_id)
                        .with_context(|| format!("missing dependency count for `{next_id}`"))?;
                    *count -= 1;
                    if *count == 0 {
                        ready.push_back(next_id.clone());
                    }
                }
            }
            self.spawn_ready(
                &mut ready,
                &mut outcomes,
                &mut remaining_readers,
                reachable,
                &mut running,
            )?;
        }

        Ok(outcomes)
    }

    fn spawn_ready(
        &self,
        ready: &mut VecDeque<String>,
        outcomes: &mut HashMap<String, BuildOutcome>,
        remaining_readers: &mut HashMap<String, usize>,
        reachable: &HashSet<String>,
        running: &mut JoinSet<Result<(String, BuildOutcome)>>,
    ) -> Result<()> {
        while let Some(target_id) = ready.pop_front() {
            let target = Arc::clone(
                self.targets
                    .get(&target_id)
                    .with_context(|| format!("target `{target_id}` vanished from graph"))?,
            );
            let mut dep_providers = Vec::new();
            let mut dep_action_digests = Vec::new();
            for dep_id in &target.deps {
                if !reachable.contains(dep_id) {
                    continue;
                }
                let remaining = remaining_readers
                    .get_mut(dep_id)
                    .with_context(|| format!("missing reader count for `{dep_id}`"))?;
                *remaining = remaining.saturating_sub(1);
                let take_ownership = *remaining == 0;

                let (provider, action_digest) = if take_ownership {
                    // Last dependent moves the provider out of the
                    // map; the outcome is dropped along with the
                    // entry, so the JsonValue tree never gets cloned.
                    let outcome = outcomes.remove(dep_id).with_context(|| {
                        format!("missing build outcome for dependency `{dep_id}`")
                    })?;
                    (outcome.provider, outcome.action_digest)
                } else {
                    let outcome = outcomes.get(dep_id).with_context(|| {
                        format!("missing build outcome for dependency `{dep_id}`")
                    })?;
                    (outcome.provider.clone(), outcome.action_digest)
                };
                dep_providers.push(provider);
                dep_action_digests.push((dep_id.clone(), action_digest));
            }

            let workspace = self.workspace.clone();
            let cache = self.cache.clone();
            let analyzer = self.analyzer.clone();
            running.spawn(Box::pin(build_one(
                workspace,
                cache,
                analyzer,
                target,
                dep_providers,
                dep_action_digests,
            )));
        }
        Ok(())
    }
}

async fn build_one(
    workspace: PathBuf,
    cache: CacheProvider,
    analyzer: AnalysisEngine,
    target: Arc<GraphTarget>,
    dep_providers: Vec<JsonValue>,
    dep_action_digests: Vec<(String, Digest)>,
) -> Result<(String, BuildOutcome)> {
    let target_id = target.label.id.clone();
    // Cheap refcount bumps so the analyzer task and the action runner
    // both reach the same `GraphTarget` without deep-cloning it.
    let analysis_target = Arc::clone(&target);
    let analysis_workspace = workspace.clone();
    let analysis = tokio::task::spawn_blocking(move || {
        analyzer
            .analyze_target(&analysis_target, &analysis_workspace, &dep_providers)
            .with_context(|| format!("analysing {}", analysis_target.label.id))
    })
    .await
    .context("joining graph analysis task")??;

    let outcome =
        run_declared_actions(&workspace, &cache, &target, analysis, &dep_action_digests).await?;
    Ok((target_id, outcome))
}

/// Materialise each declared action through the action cache, then
/// fold the analysis provider directly into the build outcome.
///
/// Takes `analysis` by value so the impl-returned provider record (a
/// potentially large `JsonValue` tree) and each declared action's
/// `env`/`outputs` move into their destinations rather than being
/// cloned.
async fn run_declared_actions(
    workspace: &Path,
    cache: &CacheProvider,
    target: &GraphTarget,
    analysis: AnalysisResult,
    dep_action_digests: &[(String, Digest)],
) -> Result<BuildOutcome> {
    let AnalysisResult {
        actions, provider, ..
    } = analysis;
    let mut action_digests = Vec::new();
    let mut last_cache_tag = "miss";
    let mut all_outputs: Vec<String> = Vec::new();
    for (index, declared) in actions.into_iter().enumerate() {
        // The identifier is only a short label; clone once for the
        // error contexts that survive past the move into the action.
        let identifier_for_error = declared
            .identifier
            .clone()
            .unwrap_or_else(|| "<anonymous>".to_string());
        all_outputs.extend(declared.outputs.iter().cloned());
        let action =
            declared_to_action(workspace, declared, dep_action_digests).with_context(|| {
                format!(
                    "building action {index} for {} ({identifier_for_error})",
                    target.label.id,
                )
            })?;
        let outcome = once_core::run_with_cache(&action, workspace, cache, RunOpts::default())
            .await
            .with_context(|| {
                format!(
                    "executing action {index} for {} ({identifier_for_error})",
                    target.label.id,
                )
            })?;
        let exit_code = outcome.result.exit_code;
        if exit_code != 0 {
            anyhow::bail!(
                "{identifier_for_error} ({index}) failed for {} with exit code {exit_code}",
                target.label.id,
            );
        }
        action_digests.push(outcome.action);
        last_cache_tag = crate::commands::util::cache_tag(outcome.cache);
    }
    let action_digest = compose_target_action_digest(&target.label.id, &action_digests);
    Ok(BuildOutcome {
        provider,
        action_digest,
        outputs: all_outputs,
        cache_tag: last_cache_tag,
    })
}

fn compose_target_action_digest(target_id: &str, action_digests: &[Digest]) -> Digest {
    match action_digests {
        [] => Digest::of_bytes(format!("empty:{target_id}").as_bytes()),
        [digest] => *digest,
        _ => {
            let mut builder = InputDigestBuilder::new(b"once.target.actions.v1\0");
            builder.push_bytes(target_id.as_bytes());
            for (index, digest) in action_digests.iter().enumerate() {
                let key = format!("action:{index}");
                builder.push_keyed(key.as_bytes(), digest);
            }
            builder.finish()
        }
    }
}

/// Convert a single declared action into a cacheable [`Action`].
///
/// Takes `declared` by value so `env` moves into the resulting
/// `Action::RunCommand` instead of being cloned. The borrow phase
/// (input digest, output paths, script body) runs first; the
/// destructure at the end relinquishes ownership of just `env`.
fn declared_to_action(
    workspace: &Path,
    declared: DeclaredAction,
    dep_action_digests: &[(String, Digest)],
) -> Result<Action> {
    let input_digest = compose_input_digest(workspace, &declared, dep_action_digests)?;
    let outputs: Vec<WorkspacePath> = declared
        .outputs
        .iter()
        .map(|path| {
            WorkspacePath::try_from(path.as_str())
                .with_context(|| format!("invalid declared output path `{path}`"))
        })
        .collect::<Result<_>>()?;
    let script = wrap_in_script(&declared.argv, &declared.outputs);
    let DeclaredAction { env, .. } = declared;
    Ok(Action::RunCommand {
        argv: vec!["/bin/sh".into(), "-c".into(), script],
        env,
        cwd: None,
        input_digest: Some(input_digest),
        outputs,
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        timeout_ms: None,
        remote: None,
    })
}

/// Compose the action's input digest from references, sorting borrows
/// rather than copying owned data:
///
/// * Inputs sort as `Vec<&str>` so the digest never owns parallel
///   copies of the source path strings.
/// * Dep digests sort by index into the caller's slice instead of
///   `to_vec()`-ing the slice up front.
/// * `declared.env` is a `BTreeMap`, which already iterates in key
///   order, so we walk it directly without a separate sort buffer.
fn compose_input_digest(
    workspace: &Path,
    declared: &DeclaredAction,
    dep_action_digests: &[(String, Digest)],
) -> Result<Digest> {
    let mut builder = InputDigestBuilder::new(b"once.declared_action.input.v1\0");
    if let Some(identity) = &declared.toolchain_identity {
        builder.push_bytes(identity.as_bytes());
    }
    if let Some(identifier) = &declared.identifier {
        builder.push_bytes(identifier.as_bytes());
    }
    for arg in &declared.argv {
        builder.push_bytes(arg.as_bytes());
    }
    for (key, value) in &declared.env {
        builder.push_bytes(key.as_bytes());
        builder.push_bytes(value.as_bytes());
    }
    let mut sorted_inputs: Vec<&str> = declared.inputs.iter().map(String::as_str).collect();
    sorted_inputs.sort_unstable();
    sorted_inputs.dedup();
    for input in &sorted_inputs {
        builder
            .push_source(workspace, input)
            .with_context(|| format!("hashing declared input `{input}`"))?;
    }
    let mut dep_order: Vec<usize> = (0..dep_action_digests.len()).collect();
    dep_order.sort_unstable_by(|&a, &b| dep_action_digests[a].0.cmp(&dep_action_digests[b].0));
    for index in dep_order {
        let (label, digest) = &dep_action_digests[index];
        let key = format!("dep:{label}");
        builder.push_keyed(key.as_bytes(), digest);
    }
    Ok(builder.finish())
}

/// Wrap a declared argv as `/bin/sh -c "mkdir -p <output dirs> && <argv>"`.
///
/// Real toolchains expect their output directories to exist; doing
/// the `mkdir -p` inside the script (rather than as a separate
/// pre-action) keeps the whole compile expressed as one cacheable
/// action.
fn wrap_in_script(argv: &[String], outputs: &[String]) -> String {
    let mut script = String::from("set -eu\n");
    let mut seen_dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for output in outputs {
        if let Some(parent) = Path::new(output).parent().and_then(|p| p.to_str()) {
            if !parent.is_empty() && seen_dirs.insert(parent.to_string()) {
                script.push_str("mkdir -p ");
                script.push_str(&shell_quote(parent));
                script.push('\n');
            }
        }
    }
    let mut first = true;
    for arg in argv {
        if !first {
            script.push(' ');
        }
        first = false;
        script.push_str(&shell_quote(arg));
    }
    script.push('\n');
    script
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use once_frontend::analysis::DeclaredAction;
    use once_frontend::{AttrValue, Capability, TargetLabel};

    static GRAPH_TEST_PRELUDE: &str = r#"
def rule(kind, impl = None):
    return {"kind": kind, "impl": impl}

def _impl(ctx):
    out = declare_output(ctx["label"]["name"] + "-" + ctx["capability"] + ".txt")
    srcs = glob(ctx["srcs"])
    if "script" in ctx["attr"]:
        run_action(
            argv = ["/bin/sh", "-c", ctx["attr"]["script"], "sh", out],
            inputs = srcs,
            outputs = [out],
            identifier = ctx["label"]["name"] + "-" + ctx["capability"],
        )
        return {"target": ctx["label"]["name"], "out": out}

    if ctx["capability"] == "test":
        run_action(
            argv = ["/bin/sh", "-c", "printf test > \"$1\"", "sh", out],
            outputs = [out],
            identifier = ctx["label"]["name"] + "-test",
        )
    elif len(srcs) > 0:
        run_action(
            argv = ["/bin/sh", "-c", "cat \"$1\" > \"$2\"", "sh", srcs[0], out],
            inputs = srcs,
            outputs = [out],
            identifier = ctx["label"]["name"] + "-build",
        )
    else:
        run_action(
            argv = ["/bin/sh", "-c", "printf " + ctx["label"]["name"] + " > \"$1\"", "sh", out],
            outputs = [out],
            identifier = ctx["label"]["name"] + "-build",
        )
    return {"target": ctx["label"]["name"], "out": out}

RULES = [
    rule("test_rule", impl = _impl),
    rule("metadata_rule"),
]
"#;

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("A'B"), "'A'\"'\"'B'");
    }

    #[test]
    fn wrap_script_prepends_mkdir_for_each_output_parent() {
        let outputs = vec![
            ".once/out/x/A.a".to_string(),
            ".once/out/x/A.swiftmodule".to_string(),
            ".once/out/x/sub/B.swiftdoc".to_string(),
        ];
        let script = wrap_in_script(&["swiftc".to_string(), "-o".to_string()], &outputs);
        assert!(script.contains("mkdir -p '.once/out/x'"));
        assert!(script.contains("mkdir -p '.once/out/x/sub'"));
        // The parent for x/A.a and x/A.swiftmodule is the same and
        // appears only once after dedup.
        assert_eq!(script.matches("mkdir -p '.once/out/x'\n").count(), 1);
    }

    #[test]
    fn input_digest_changes_with_toolchain_identity() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("a.swift"), b"print(1)").unwrap();
        let declared = DeclaredAction {
            argv: vec!["swiftc".to_string()],
            inputs: vec!["a.swift".to_string()],
            outputs: vec![".once/out/A.a".to_string()],
            env: BTreeMap::new(),
            toolchain_identity: Some("id-1".to_string()),
            identifier: None,
        };
        let one = compose_input_digest(workspace.path(), &declared, &[]).unwrap();
        let declared2 = DeclaredAction {
            toolchain_identity: Some("id-2".to_string()),
            ..declared.clone()
        };
        let two = compose_input_digest(workspace.path(), &declared2, &[]).unwrap();
        assert_ne!(one, two);
    }

    #[test]
    fn input_digest_stable_under_dep_reordering() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("a.swift"), b"print(1)").unwrap();
        let declared = DeclaredAction {
            argv: vec!["swiftc".to_string()],
            inputs: vec!["a.swift".to_string()],
            outputs: vec![".once/out/A.a".to_string()],
            env: BTreeMap::new(),
            toolchain_identity: None,
            identifier: None,
        };
        let a = compose_input_digest(
            workspace.path(),
            &declared,
            &[
                ("dep1".to_string(), Digest::of_bytes(b"d1")),
                ("dep2".to_string(), Digest::of_bytes(b"d2")),
            ],
        )
        .unwrap();
        let b = compose_input_digest(
            workspace.path(),
            &declared,
            &[
                ("dep2".to_string(), Digest::of_bytes(b"d2")),
                ("dep1".to_string(), Digest::of_bytes(b"d1")),
            ],
        )
        .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn target_action_digest_preserves_single_action_digest() {
        let action = Digest::of_bytes(b"action");

        assert_eq!(compose_target_action_digest("Root", &[action]), action);
    }

    #[test]
    fn target_action_digest_for_empty_actions_is_target_specific() {
        let root = compose_target_action_digest("Root", &[]);
        let same_root = compose_target_action_digest("Root", &[]);
        let other = compose_target_action_digest("Other", &[]);

        assert_eq!(root, same_root);
        assert_ne!(root, other);
    }

    #[test]
    fn target_action_digest_includes_all_declared_actions_in_order() {
        let first = Digest::of_bytes(b"first");
        let second = Digest::of_bytes(b"second");
        let changed_second = Digest::of_bytes(b"changed-second");

        let original = compose_target_action_digest("Root", &[first, second]);
        let changed = compose_target_action_digest("Root", &[first, changed_second]);
        let reordered = compose_target_action_digest("Root", &[second, first]);

        assert_ne!(original, changed);
        assert_ne!(original, reordered);
    }

    fn test_target(name: &str, deps: &[&str], script: impl Into<String>) -> GraphTarget {
        target_with_capabilities(
            name,
            deps,
            &[],
            &["build"],
            [("script".to_string(), AttrValue::String(script.into()))],
        )
    }

    fn target_of_kind(
        kind: &str,
        name: &str,
        deps: &[&str],
        srcs: &[&str],
        capabilities: &[&str],
        attrs: impl IntoIterator<Item = (String, AttrValue)>,
    ) -> GraphTarget {
        let mut target = target_with_capabilities(name, deps, srcs, capabilities, attrs);
        target.kind = kind.to_string();
        target
    }

    fn target_with_capabilities(
        name: &str,
        deps: &[&str],
        srcs: &[&str],
        capabilities: &[&str],
        attrs: impl IntoIterator<Item = (String, AttrValue)>,
    ) -> GraphTarget {
        GraphTarget {
            label: TargetLabel {
                package: String::new(),
                name: name.to_string(),
                id: name.to_string(),
            },
            kind: "test_rule".to_string(),
            deps: deps.iter().map(|dep| (*dep).to_string()).collect(),
            srcs: srcs.iter().map(|src| (*src).to_string()).collect(),
            attrs: attrs.into_iter().collect(),
            capabilities: capabilities
                .iter()
                .map(|capability| Capability {
                    name: (*capability).to_string(),
                    output_groups: Vec::new(),
                    requires_outputs: Vec::new(),
                })
                .collect(),
            providers: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn reachable_impl_deps_walks_only_impl_backed_direct_deps() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
        let graph = vec![
            target_with_capabilities(
                "Root",
                &["DirectImpl", "DirectMetadata"],
                &[],
                &["test"],
                [],
            ),
            target_with_capabilities("DirectImpl", &["TransitiveImpl"], &[], &["build"], []),
            target_with_capabilities("TransitiveImpl", &[], &[], &["build"], []),
            target_of_kind(
                "metadata_rule",
                "DirectMetadata",
                &["HiddenImpl"],
                &[],
                &["build"],
                [],
            ),
            target_with_capabilities("HiddenImpl", &[], &[], &["build"], []),
        ];
        let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();
        let session = BuildSession::new_with_analyzer(workspace.path(), &cache, &graph, analyzer);

        let reachable = session.reachable_impl_deps(&graph[0]);

        assert!(reachable.contains("DirectImpl"));
        assert!(reachable.contains("TransitiveImpl"));
        assert!(!reachable.contains("DirectMetadata"));
        assert!(!reachable.contains("HiddenImpl"));
    }

    #[tokio::test]
    async fn run_with_impl_returns_none_for_rules_without_impl() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
        let graph = vec![
            target_of_kind("metadata_rule", "Root", &["Dep"], &[], &["test"], []),
            target_with_capabilities("Dep", &[], &[], &["build"], []),
        ];
        let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();
        let session = BuildSession::new_with_analyzer(workspace.path(), &cache, &graph, analyzer);

        let outcome = session.run_with_impl(&graph[0], "test").await.unwrap();

        assert!(outcome.is_none());
        assert!(!workspace.path().join(".once/out/Dep").exists());
    }

    #[cfg(unix)]
    fn parallel_leaf_script(marker: &str, peer: &str, output: &str) -> String {
        format!(
            r#"mkdir -p sync
: > sync/{marker}
i=0
while [ ! -f sync/{peer} ]; do
  i=$((i + 1))
  [ "$i" -le 50 ] || exit 42
  sleep 0.1
done
printf {output} > "$1"
"#
        )
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn independent_dependencies_run_in_parallel() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
        let graph = vec![
            test_target("Root", &["LeafA", "LeafB"], "printf root > \"$1\""),
            test_target("LeafA", &[], parallel_leaf_script("LeafA", "LeafB", "a")),
            test_target("LeafB", &[], parallel_leaf_script("LeafB", "LeafA", "b")),
        ];
        let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();
        let session = BuildSession::new_with_analyzer(workspace.path(), &cache, &graph, analyzer);

        let outcome = session.build_with_impl(&graph[0]).await.unwrap().unwrap();

        assert_eq!(
            outcome.outputs,
            vec![".once/out/Root/Root-build.txt".to_string()]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn build_direct_impl_deps_returns_only_direct_deps_in_declared_order() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
        let graph = vec![
            target_with_capabilities("Root", &["Second", "Metadata", "First"], &[], &["test"], []),
            target_with_capabilities("Second", &["Shared"], &[], &["build"], []),
            target_of_kind("metadata_rule", "Metadata", &[], &[], &["build"], []),
            target_with_capabilities("First", &["Shared"], &[], &["build"], []),
            target_with_capabilities("Shared", &[], &[], &["build"], []),
        ];
        let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();
        let session = BuildSession::new_with_analyzer(workspace.path(), &cache, &graph, analyzer);

        let outcomes = session.build_direct_impl_deps(&graph[0]).await.unwrap();
        let outcome_ids = outcomes
            .iter()
            .map(|(target_id, _)| target_id.as_str())
            .collect::<Vec<_>>();
        let provider_targets = outcomes
            .iter()
            .map(|(_, outcome)| outcome.provider["target"].as_str().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(outcome_ids, vec!["Second", "First"]);
        assert_eq!(provider_targets, vec!["Second", "First"]);
        assert_eq!(
            outcomes[0].1.outputs,
            vec![".once/out/Second/Second-build.txt".to_string()]
        );
        assert!(workspace
            .path()
            .join(".once/out/Shared/Shared-build.txt")
            .is_file());
        assert!(!workspace.path().join(".once/out/Metadata").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn capability_runs_are_salted_by_dependency_action_digests() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("dep.txt"), b"one").unwrap();
        let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
        let graph = vec![
            target_with_capabilities("Dep", &[], &["dep.txt"], &["build"], []),
            target_with_capabilities("Root", &["Dep"], &[], &["test"], []),
        ];
        let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();

        let session = BuildSession::new_with_analyzer(workspace.path(), &cache, &graph, analyzer);
        let first = session
            .run_with_impl(&graph[1], "test")
            .await
            .unwrap()
            .unwrap();

        std::fs::write(workspace.path().join("dep.txt"), b"two").unwrap();
        let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();
        let session = BuildSession::new_with_analyzer(workspace.path(), &cache, &graph, analyzer);
        let second = session
            .run_with_impl(&graph[1], "test")
            .await
            .unwrap()
            .unwrap();

        assert_ne!(first.action_digest, second.action_digest);
    }
}
