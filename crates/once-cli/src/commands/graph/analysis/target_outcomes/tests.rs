use std::collections::{BTreeMap, BTreeSet};

use once_cas::{ActionResult, Digest};
use once_core::{EvidenceCacheState, SandboxMode};
use once_frontend::analysis::{AnalysisObservations, Observation};
use once_frontend::Target;
use tempfile::TempDir;

use super::super::source_digest_cache::KnownChanges;
use super::super::BuildOutcome;
use super::TargetOutcomes;
use crate::commands::change_tracker::ChangePosition;

fn target(package: &str, name: &str) -> once_frontend::GraphTarget {
    let target = Target {
        package: package.to_string(),
        kind: "script".to_string(),
        name: name.to_string(),
        deps: Vec::new(),
        dependency_edges: BTreeMap::new(),
        srcs: Vec::new(),
        visibility: Vec::new(),
        attrs: BTreeMap::new(),
        typed_attrs: BTreeMap::new(),
        resolver_input_exclude: Vec::new(),
    };
    once_frontend::graph_from_targets(&[target]).remove(0)
}

fn outcome(outputs: &[&str]) -> BuildOutcome {
    BuildOutcome {
        provider: std::sync::Arc::new(serde_json::json!({"ok": true})),
        action_digest: Digest::of_bytes(b"action"),
        input_digest: None,
        input_fingerprint: None,
        available_inputs: BTreeMap::new(),
        outputs: outputs.iter().map(|o| (*o).to_string()).collect(),
        cache_tag: "hit",
        cache_state: EvidenceCacheState::Hit,
        result: ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::new(),
        },
        cached_results: Vec::new(),
    }
}

fn observations(package: &str, patterns: &[&str]) -> AnalysisObservations {
    expansion_observations("glob", package, patterns)
}

fn expansion_observations(
    expansion: &str,
    package: &str,
    patterns: &[&str],
) -> AnalysisObservations {
    let mut recorded = AnalysisObservations::default();
    recorded.push_for_test(Observation::Paths {
        expansion: expansion.to_string(),
        package: package.to_string(),
        patterns: patterns.iter().map(|p| (*p).to_string()).collect(),
        excludes: Vec::new(),
        matches: Vec::new(),
    });
    recorded
}

fn inputs(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|p| (*p).to_string()).collect()
}

fn changed(sources: &[&str], outputs: &[&str]) -> KnownChanges {
    KnownChanges::Since {
        sources: sources.iter().map(|p| (*p).to_string()).collect(),
        outputs: outputs.iter().map(|p| (*p).to_string()).collect(),
    }
}

fn position() -> ChangePosition {
    ChangePosition {
        instance_id: "tracker".to_string(),
        source_generation: 1,
        output_generation: 1,
    }
}

/// Records survive to the next invocation, which is the whole point.
fn round_trip(workspace: &TempDir, write: impl FnOnce(&TargetOutcomes)) -> TargetOutcomes {
    let outcomes = TargetOutcomes::open(workspace.path(), "build", SandboxMode::Off, "", None);
    write(&outcomes);
    outcomes.save(Some(&position()));
    TargetOutcomes::open(
        workspace.path(),
        "build",
        SandboxMode::Off,
        "",
        Some(&position()),
    )
}

#[test]
fn a_target_nothing_touched_is_reused() {
    let workspace = TempDir::new().unwrap();
    let target = target("apps/tool", "tool");
    let reopened = round_trip(&workspace, |outcomes| {
        outcomes.record(
            &target,
            "key".to_string(),
            &observations("apps/tool", &["src/*.rs"]),
            &inputs(&["apps/tool/src/lib.rs"]),
            &outcome(&[".once/out/tool/tool"]),
        );
    });

    let reused = reopened.reuse(&target, "key", &changed(&["apps/other/src/lib.rs"], &[]));

    assert!(reused.is_some());
}

#[test]
fn editing_a_file_the_target_declared_visits_it_again() {
    let workspace = TempDir::new().unwrap();
    let target = target("apps/tool", "tool");
    let reopened = round_trip(&workspace, |outcomes| {
        outcomes.record(
            &target,
            "key".to_string(),
            &observations("apps/tool", &["src/*.rs"]),
            &inputs(&["apps/tool/src/lib.rs"]),
            &outcome(&[".once/out/tool/tool"]),
        );
    });

    assert!(reopened
        .reuse(&target, "key", &changed(&["apps/tool/src/lib.rs"], &[]))
        .is_none());
}

/// A file that did not exist when the record was written is in no list of
/// declared inputs, and yet an expansion the target ran would have selected it.
#[test]
fn a_file_appearing_under_an_expanded_pattern_visits_it_again() {
    let workspace = TempDir::new().unwrap();
    let target = target("apps/tool", "tool");
    let reopened = round_trip(&workspace, |outcomes| {
        outcomes.record(
            &target,
            "key".to_string(),
            &observations("apps/tool", &["src/*.rs"]),
            &inputs(&["apps/tool/src/lib.rs"]),
            &outcome(&[".once/out/tool/tool"]),
        );
    });

    assert!(reopened
        .reuse(&target, "key", &changed(&["apps/tool/src/added.rs"], &[]))
        .is_none());
}

#[test]
fn a_changed_output_visits_it_again() {
    let workspace = TempDir::new().unwrap();
    let target = target("apps/tool", "tool");
    let reopened = round_trip(&workspace, |outcomes| {
        outcomes.record(
            &target,
            "key".to_string(),
            &observations("apps/tool", &["src/*.rs"]),
            &inputs(&[]),
            &outcome(&[".once/out/tool"]),
        );
    });

    assert!(reopened
        .reuse(&target, "key", &changed(&[], &[".once/out/tool/tool"]))
        .is_none());
}

/// The name folds in the target definition and its dependencies' outcomes, so a
/// dependency that rebuilt gives everything above it a different name.
#[test]
fn a_different_name_is_a_different_build() {
    let workspace = TempDir::new().unwrap();
    let target = target("apps/tool", "tool");
    let reopened = round_trip(&workspace, |outcomes| {
        outcomes.record(
            &target,
            "key".to_string(),
            &observations("apps/tool", &["src/*.rs"]),
            &inputs(&[]),
            &outcome(&[".once/out/tool/tool"]),
        );
    });

    assert!(reopened
        .reuse(&target, "a-dependency-rebuilt", &changed(&[], &[]))
        .is_none());
}

/// An outcome that declined to be cached has to run every time, so it is never
/// recorded.
#[test]
fn an_uncacheable_outcome_is_never_recorded() {
    let workspace = TempDir::new().unwrap();
    let target = target("apps/tool", "tool");
    let mut bypass = outcome(&[".once/out/tool/tool"]);
    bypass.cache_state = EvidenceCacheState::Bypass;
    let reopened = round_trip(&workspace, |outcomes| {
        outcomes.record(
            &target,
            "key".to_string(),
            &AnalysisObservations::default(),
            &inputs(&[]),
            &bypass,
        );
    });

    assert!(reopened.reuse(&target, "key", &changed(&[], &[])).is_none());
}

/// With no watcher there is nothing to say what moved, so nothing is reused.
/// Records describe one moment. When the caller's account of the window is
/// relative to a different one, nothing can be said about what moved since,
/// so the records are not used.
#[test]
fn records_from_another_moment_are_not_used() {
    let workspace = TempDir::new().unwrap();
    let target = target("apps/tool", "tool");
    let outcomes = TargetOutcomes::open(workspace.path(), "build", SandboxMode::Off, "", None);
    outcomes.record(
        &target,
        "key".to_string(),
        &AnalysisObservations::default(),
        &inputs(&[]),
        &outcome(&[".once/out/tool/tool"]),
    );
    outcomes.save(Some(&position()));

    let elsewhere = ChangePosition {
        source_generation: 99,
        ..position()
    };
    let reopened = TargetOutcomes::open(
        workspace.path(),
        "build",
        SandboxMode::Off,
        "",
        Some(&elsewhere),
    );

    assert!(reopened.reuse(&target, "key", &changed(&[], &[])).is_none());
}

#[test]
fn an_unknown_window_reuses_nothing() {
    let workspace = TempDir::new().unwrap();
    let target = target("apps/tool", "tool");
    let reopened = round_trip(&workspace, |outcomes| {
        outcomes.record(
            &target,
            "key".to_string(),
            &AnalysisObservations::default(),
            &inputs(&[]),
            &outcome(&[".once/out/tool/tool"]),
        );
    });

    assert!(reopened
        .reuse(&target, "key", &KnownChanges::Unknown)
        .is_none());
}

/// A walk owns everything under its directory, so a file dropped in there is a
/// target that has to be visited again. Matching the directory as a glob
/// pattern instead would quietly reuse the outcome.
#[test]
fn a_file_appearing_under_a_walked_directory_visits_it_again() {
    let workspace = TempDir::new().unwrap();
    let target = target("apps/Hello", "Hello");
    let reopened = round_trip(&workspace, |outcomes| {
        outcomes.record(
            &target,
            "key".to_string(),
            &expansion_observations("walk_workspace_files", "", &["apps/Hello/Sources"]),
            &inputs(&[]),
            &outcome(&[".once/out/Hello/Hello"]),
        );
    });

    assert!(reopened
        .reuse(
            &target,
            "key",
            &changed(&["apps/Hello/Sources/Extra.h"], &[])
        )
        .is_none());
}

/// Reusing a record means this invocation did no work, so it reports a hit even
/// when the build that produced the record had to compile.
#[test]
fn a_reused_outcome_reports_a_hit_however_it_was_produced() {
    let workspace = TempDir::new().unwrap();
    let target = target("apps/tool", "tool");
    let mut compiled = outcome(&[".once/out/tool/tool"]);
    compiled.cache_state = EvidenceCacheState::Miss;
    compiled.cache_tag = "miss";
    let reopened = round_trip(&workspace, |outcomes| {
        outcomes.record(
            &target,
            "key".to_string(),
            &AnalysisObservations::default(),
            &inputs(&[]),
            &compiled,
        );
    });

    let reused = reopened
        .reuse(&target, "key", &changed(&[], &[]))
        .expect("nothing moved, so the record still describes this build");

    assert_eq!(reused.cache_state, EvidenceCacheState::Hit);
    assert_eq!(reused.cache_tag, "hit");
}
