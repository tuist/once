use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use anyhow::Result;
use once_core::{TestSelectionPolicy, TestSelectionReport, WorkspacePath, TEST_SELECTION_SCHEMA};
use once_frontend::GraphTarget;

use super::inputs::{is_graph_input, target_input_patterns, workspace_graph_input_patterns};
use super::AffectedTestRecord;

pub(super) fn selection_report(
    workspace: &Path,
    graph: &[GraphTarget],
    changed_paths: &[String],
) -> Result<TestSelectionReport> {
    let changed_paths = normalize_changed_paths(changed_paths);
    if changed_paths.is_empty() {
        return Ok(TestSelectionReport {
            schema: TEST_SELECTION_SCHEMA.to_string(),
            policy: TestSelectionPolicy {
                mode: "full".to_string(),
                safety: "exact".to_string(),
                evidence: "complete_test_scope".to_string(),
            },
            changed_paths,
            unmatched_paths: Vec::new(),
            tests: all_tests(graph, "no changed paths supplied; include test target"),
        });
    }

    let graph_inputs = workspace_graph_input_patterns(workspace)?;
    let target_patterns = target_input_patterns(workspace, graph);
    let reverse_dependencies = reverse_dependencies(graph);
    let targets = graph
        .iter()
        .map(|target| (target.label.id.as_str(), target))
        .collect::<BTreeMap<_, _>>();
    let mut reasons = BTreeMap::<String, BTreeSet<String>>::new();
    let mut unmatched_paths = Vec::new();

    for path in &changed_paths {
        if is_graph_input(path, &graph_inputs) {
            add_full_test_scope(
                graph,
                &format!("changed graph definition `{path}`; include test target"),
                &mut reasons,
            );
            continue;
        }

        let owners = target_patterns
            .iter()
            .filter_map(|(target_id, patterns)| {
                patterns
                    .iter()
                    .any(|pattern| pattern.matches(path))
                    .then_some(target_id.as_str())
            })
            .collect::<Vec<_>>();
        if owners.is_empty() {
            // Deliberately conservative: a path no target claims may still affect
            // any test (for example documentation compiled into doc-tests), so we
            // select the whole test scope rather than risk skipping a regression.
            // The path is also recorded in `unmatched_paths` so callers can see why.
            unmatched_paths.push(path.clone());
            add_full_test_scope(
                graph,
                &format!("changed path `{path}` has no declared owner; include test target"),
                &mut reasons,
            );
            continue;
        }

        for owner in owners {
            add_affected_tests(path, owner, &targets, &reverse_dependencies, &mut reasons);
        }
    }

    let tests = graph
        .iter()
        .filter(|target| has_capability(target, "test"))
        .filter_map(|target| {
            reasons
                .remove(&target.label.id)
                .map(|reasons| AffectedTestRecord {
                    id: target.label.id.clone(),
                    kind: target.kind.clone(),
                    reasons: reasons.into_iter().collect(),
                })
        })
        .collect();

    Ok(TestSelectionReport {
        schema: TEST_SELECTION_SCHEMA.to_string(),
        policy: TestSelectionPolicy {
            mode: "affected".to_string(),
            safety: "conservative".to_string(),
            evidence: "declared_graph".to_string(),
        },
        changed_paths,
        unmatched_paths,
        tests,
    })
}

fn normalize_changed_paths(paths: &[String]) -> Vec<String> {
    // Normalize to workspace-relative form when possible, but tolerate inputs that
    // cannot be (absolute paths, `..`, Windows separators) rather than failing the
    // whole command. A path we cannot normalize simply won't match a declared
    // owner and falls through to conservative selection.
    let mut normalized = paths
        .iter()
        .map(|path| {
            WorkspacePath::try_from(path.as_str()).map_or_else(
                |_| path.trim_start_matches("./").to_string(),
                |workspace_path| workspace_path.to_string(),
            )
        })
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
}

fn all_tests(graph: &[GraphTarget], reason: &str) -> Vec<AffectedTestRecord> {
    graph
        .iter()
        .filter(|target| has_capability(target, "test"))
        .map(|target| AffectedTestRecord {
            id: target.label.id.clone(),
            kind: target.kind.clone(),
            reasons: vec![reason.to_string()],
        })
        .collect()
}

fn add_full_test_scope(
    graph: &[GraphTarget],
    reason: &str,
    reasons: &mut BTreeMap<String, BTreeSet<String>>,
) {
    for test in graph.iter().filter(|target| has_capability(target, "test")) {
        reasons
            .entry(test.label.id.clone())
            .or_default()
            .insert(reason.to_string());
    }
}

fn add_affected_tests(
    path: &str,
    owner: &str,
    targets: &BTreeMap<&str, &GraphTarget>,
    reverse_dependencies: &BTreeMap<&str, Vec<&str>>,
    reasons: &mut BTreeMap<String, BTreeSet<String>>,
) {
    let mut queue = VecDeque::from([owner]);
    let mut visited = BTreeSet::new();
    while let Some(target_id) = queue.pop_front() {
        if !visited.insert(target_id) {
            continue;
        }
        if let Some(target) = targets.get(target_id) {
            if has_capability(target, "test") {
                let reason = if target_id == owner {
                    format!("changed test input `{path}`")
                } else {
                    format!("changed dependency `{owner}` input `{path}`")
                };
                reasons
                    .entry(target.label.id.clone())
                    .or_default()
                    .insert(reason);
            }
        }
        if let Some(dependents) = reverse_dependencies.get(target_id) {
            queue.extend(dependents.iter().copied());
        }
    }
}

fn reverse_dependencies(graph: &[GraphTarget]) -> BTreeMap<&str, Vec<&str>> {
    let mut reverse = BTreeMap::<&str, Vec<&str>>::new();
    for target in graph {
        for dependency in &target.deps {
            reverse
                .entry(dependency.as_str())
                .or_default()
                .push(target.label.id.as_str());
        }
    }
    reverse
}

fn has_capability(target: &GraphTarget, name: &str) -> bool {
    target
        .capabilities
        .iter()
        .any(|capability| capability.name == name)
}

#[cfg(test)]
mod tests;
