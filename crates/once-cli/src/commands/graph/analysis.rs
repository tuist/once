//! Walks the graph in dependency order and executes the actions each
//! rule impl declares.
//!
//! For every target the driver calls [`once_frontend::analysis::analyze_target`].
//! When the rule has an `impl` callable the analysis returns a list of
//! `DeclaredAction`s plus a provider record; we materialise each
//! declared command as a cacheable `Action`, run it through
//! `once_core::run_with_cache`, and pass the resulting provider down
//! to consumers. When the rule has no `impl` declared in the prelude,
//! the driver returns `None` so the caller can fall back to its generic
//! marker action.
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
    /// impl, allowing callers to fall back to generic marker actions.
    pub(super) async fn build_with_impl(
        &self,
        target: &GraphTarget,
    ) -> Result<Option<BuildOutcome>> {
        if !self.analyzer.rule_has_impl(&target.kind) {
            // Capability-only rules keep their generic marker action; we
            // don't walk their deps because that fallback doesn't
            // consume them yet.
            return Ok(None);
        }

        let reachable = self.reachable_impl_targets(target);
        let outcome = self.build_reachable(&target.label.id, &reachable).await?;
        Ok(Some(outcome))
    }

    fn reachable_impl_targets(&self, target: &GraphTarget) -> HashSet<String> {
        let mut reachable = HashSet::new();
        let mut stack = vec![target.label.id.clone()];
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

    async fn build_reachable(
        &self,
        root_id: &str,
        reachable: &HashSet<String>,
    ) -> Result<BuildOutcome> {
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

        outcomes
            .remove(root_id)
            .with_context(|| format!("missing build outcome for `{root_id}`"))
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
    let mut terminal_digest = None;
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
            let stderr = match outcome.result.stderr {
                Some(digest) => {
                    String::from_utf8_lossy(&cache.get_blob(&digest).await?).to_string()
                }
                None => String::new(),
            };
            let detail = if stderr.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", stderr.trim())
            };
            anyhow::bail!(
                "{identifier_for_error} ({index}) failed for {} with exit code {exit_code}{detail}",
                target.label.id,
            );
        }
        terminal_digest = Some(outcome.action);
        last_cache_tag = crate::commands::util::cache_tag(outcome.cache);
    }
    let action_digest = terminal_digest
        .unwrap_or_else(|| Digest::of_bytes(format!("empty:{}", target.label.id).as_bytes()));
    Ok(BuildOutcome {
        provider,
        action_digest,
        outputs: all_outputs,
        cache_tag: last_cache_tag,
    })
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

    fn test_target(name: &str, deps: &[&str], script: &str) -> GraphTarget {
        GraphTarget {
            label: TargetLabel {
                package: String::new(),
                name: name.to_string(),
                id: name.to_string(),
            },
            kind: "test_rule".to_string(),
            deps: deps.iter().map(|dep| (*dep).to_string()).collect(),
            srcs: Vec::new(),
            attrs: BTreeMap::from([("script".to_string(), AttrValue::String(script.to_string()))]),
            capabilities: vec![Capability {
                name: "build".to_string(),
                output_groups: Vec::new(),
                requires_outputs: Vec::new(),
            }],
            providers: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn independent_dependencies_run_in_parallel() {
        static TEST_PRELUDE: &str = r#"
def rule(kind, impl = None):
    return {"kind": kind, "impl": impl}

def _impl(ctx):
    out = declare_output(ctx["label"]["name"] + ".txt")
    run_action(
        argv = ["/bin/sh", "-c", ctx["attr"]["script"], "sh", out],
        outputs = [out],
        identifier = ctx["label"]["name"],
    )
    return {"out": out}

RULES = [rule("test_rule", impl = _impl)]
"#;
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
        let graph = vec![
            test_target("Root", &["LeafA", "LeafB"], "printf root > \"$1\""),
            test_target("LeafA", &[], "sleep 0.7; printf a > \"$1\""),
            test_target("LeafB", &[], "sleep 0.7; printf b > \"$1\""),
        ];
        let analyzer = AnalysisEngine::from_source(TEST_PRELUDE).unwrap();
        let session = BuildSession::new_with_analyzer(workspace.path(), &cache, &graph, analyzer);

        let started = std::time::Instant::now();
        let outcome = session.build_with_impl(&graph[0]).await.unwrap().unwrap();
        let elapsed = started.elapsed();

        assert!(
            elapsed < std::time::Duration::from_millis(1_200),
            "expected sibling deps to run concurrently, elapsed {elapsed:?}"
        );
        assert_eq!(outcome.outputs, vec![".once/out/Root/Root.txt".to_string()]);
    }
}
