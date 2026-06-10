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

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Context, Result};
use once_cas::{CacheProvider, Digest};
use once_core::{
    Action, InputDigestBuilder, OutputSymlinkMode, ResourceRequest, RunOpts, WorkspacePath,
};
use once_frontend::analysis::{analyze_target, rule_has_impl, AnalysisResult, DeclaredAction};
use once_frontend::GraphTarget;
use serde_json::Value as JsonValue;

/// Per-target outcome cached during a single command invocation.
#[derive(Debug, Clone)]
pub(super) struct BuildOutcome {
    pub provider: JsonValue,
    pub action_digest: Digest,
    pub outputs: Vec<String>,
    pub cache_tag: &'static str,
}

/// Build a target and (recursively) its declared deps. Returns the
/// terminal action's outcome and the impl-returned provider; if the
/// rule has no impl, returns `Ok(None)` and the caller is expected to
/// fall back to its placeholder path.
pub(super) async fn build_with_impl(
    workspace: &Path,
    cache: &CacheProvider,
    graph: &[GraphTarget],
    target: &GraphTarget,
    built: &mut HashMap<String, BuildOutcome>,
) -> Result<Option<BuildOutcome>> {
    if !rule_has_impl(&target.kind)? {
        // Placeholder rules keep their existing shell scripts; we
        // don't walk their deps because the placeholder doesn't
        // consume them yet.
        return Ok(None);
    }
    if let Some(outcome) = built.get(&target.label.id) {
        return Ok(Some(outcome.clone()));
    }

    let mut dep_providers: Vec<JsonValue> = Vec::new();
    let mut dep_action_digests: Vec<(String, Digest)> = Vec::new();
    for dep_id in &target.deps {
        let Some(dep) = graph.iter().find(|candidate| candidate.label.id == *dep_id) else {
            continue;
        };
        if !rule_has_impl(&dep.kind)? {
            continue;
        }
        let dep_outcome = Box::pin(build_with_impl(workspace, cache, graph, dep, built)).await?;
        if let Some(outcome) = dep_outcome {
            dep_providers.push(outcome.provider.clone());
            dep_action_digests.push((dep_id.clone(), outcome.action_digest));
        }
    }

    let analysis = analyze_target(target, workspace, &dep_providers)
        .with_context(|| format!("analysing {}", target.label.id))?;

    let outcome =
        run_declared_actions(workspace, cache, target, &analysis, &dep_action_digests).await?;
    built.insert(target.label.id.clone(), outcome.clone());
    Ok(Some(outcome))
}

async fn run_declared_actions(
    workspace: &Path,
    cache: &CacheProvider,
    target: &GraphTarget,
    analysis: &AnalysisResult,
    dep_action_digests: &[(String, Digest)],
) -> Result<BuildOutcome> {
    let mut terminal_digest = None;
    let mut last_cache_tag = "miss";
    for (index, declared) in analysis.actions.iter().enumerate() {
        let action =
            declared_to_action(workspace, declared, dep_action_digests).with_context(|| {
                format!(
                    "building action {index} for {} ({})",
                    target.label.id,
                    declared.identifier.as_deref().unwrap_or("<anonymous>")
                )
            })?;
        let outcome = once_core::run_with_cache(&action, workspace, cache, RunOpts::default())
            .await
            .with_context(|| {
                format!(
                    "executing action {index} for {} ({})",
                    target.label.id,
                    declared.identifier.as_deref().unwrap_or("<anonymous>")
                )
            })?;
        let exit_code = outcome.result.exit_code;
        if exit_code != 0 {
            anyhow::bail!(
                "{} ({}) failed for {} with exit code {}",
                declared.identifier.as_deref().unwrap_or("<anonymous>"),
                index,
                target.label.id,
                exit_code
            );
        }
        terminal_digest = Some(outcome.action);
        last_cache_tag = crate::commands::util::cache_tag(outcome.cache);
    }
    let action_digest = terminal_digest
        .unwrap_or_else(|| Digest::of_bytes(format!("empty:{}", target.label.id).as_bytes()));
    let outputs: Vec<String> = analysis
        .actions
        .iter()
        .flat_map(|declared| declared.outputs.iter().cloned())
        .collect();
    Ok(BuildOutcome {
        provider: analysis.provider.clone(),
        action_digest,
        outputs,
        cache_tag: last_cache_tag,
    })
}

fn declared_to_action(
    workspace: &Path,
    declared: &DeclaredAction,
    dep_action_digests: &[(String, Digest)],
) -> Result<Action> {
    let input_digest = compose_input_digest(workspace, declared, dep_action_digests)?;
    let outputs: Vec<WorkspacePath> = declared
        .outputs
        .iter()
        .map(|path| {
            WorkspacePath::try_from(path.as_str())
                .with_context(|| format!("invalid declared output path `{path}`"))
        })
        .collect::<Result<_>>()?;
    let script = wrap_in_script(&declared.argv, &declared.outputs);
    Ok(Action::RunCommand {
        argv: vec!["/bin/sh".into(), "-c".into(), script],
        env: declared.env.clone().into_iter().collect::<BTreeMap<_, _>>(),
        cwd: None,
        input_digest: Some(input_digest),
        outputs,
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        timeout_ms: None,
        remote: None,
    })
}

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
    let mut sorted_env: Vec<(&String, &String)> = declared.env.iter().collect();
    sorted_env.sort_by(|a, b| a.0.cmp(b.0));
    for (key, value) in sorted_env {
        builder.push_bytes(key.as_bytes());
        builder.push_bytes(value.as_bytes());
    }
    let mut sorted_inputs = declared.inputs.clone();
    sorted_inputs.sort();
    sorted_inputs.dedup();
    for input in &sorted_inputs {
        builder
            .push_source(workspace, input)
            .with_context(|| format!("hashing declared input `{input}`"))?;
    }
    let mut sorted_deps = dep_action_digests.to_vec();
    sorted_deps.sort_by(|a, b| a.0.cmp(&b.0));
    for (label, digest) in &sorted_deps {
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
    use once_frontend::analysis::DeclaredAction;

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
}
