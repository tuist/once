use super::*;
use std::collections::BTreeMap;
use struo_cas::Digest;

fn result(path: &str) -> ActionResult {
    ActionResult {
        exit_code: 0,
        stdout: None,
        stderr: None,
        outputs: BTreeMap::from([(path.into(), Digest::of_bytes(path.as_bytes()))]),
    }
}

#[test]
fn independent_paths_are_bounded_and_shared_paths_are_barriers() {
    let workspace = tempfile::tempdir().unwrap();
    let independent = if cfg!(windows)
        || !workspace.path().to_string_lossy().is_ascii()
        || !workspace
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .is_ascii()
    {
        1
    } else {
        2
    };
    assert_eq!(
        batch_len(
            &[result("a"), result("b"), result("c")],
            workspace.path(),
            2
        ),
        independent
    );
    for (first, second) in [
        ("a", "a"),
        ("a", "a/child"),
        ("a/child", "a"),
        ("a", "A"),
        ("a", "./a"),
        ("", "a"),
    ] {
        assert_eq!(
            batch_len(&[result(first), result(second)], workspace.path(), 4),
            1
        );
    }
    assert_eq!(
        batch_len(&[result("a"), result("a-other")], workspace.path(), 4),
        independent
    );
}

#[test]
fn unicode_names_serialize_when_alias_rules_are_not_proven() {
    let workspace = tempfile::tempdir().unwrap();
    assert_eq!(
        batch_len(
            &[result("résumé"), result("re\u{301}sume\u{301}")],
            workspace.path(),
            4
        ),
        1
    );
}

#[cfg(unix)]
#[test]
fn dangling_symlink_parents_are_uncertain_even_before_target_creation() {
    let workspace = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("tree", workspace.path().join("alias")).unwrap();
    assert!(!disjoint(
        workspace.path(),
        &BTreeSet::from(["tree".into()]),
        &BTreeSet::from(["alias/child".into()])
    ));
    assert_eq!(
        batch_len(
            &[result("tree"), result("alias/child")],
            workspace.path(),
            4
        ),
        1
    );
}

#[cfg(unix)]
#[test]
fn existing_symlink_parents_are_barriers_even_for_missing_outputs() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("tree")).unwrap();
    std::os::unix::fs::symlink("tree", workspace.path().join("alias")).unwrap();
    assert_eq!(
        batch_len(
            &[result("tree/child"), result("alias/child")],
            workspace.path(),
            4
        ),
        1
    );
    assert_eq!(
        batch_len(
            &[result("tree"), result("alias/child")],
            workspace.path(),
            4
        ),
        1
    );
    assert_eq!(
        batch_len(
            &[result("alias"), result("alias/child")],
            workspace.path(),
            4
        ),
        1
    );
    assert!(creation_conflicts(
        workspace.path(),
        "alias/child",
        &BTreeSet::from(["tree".into()])
    ));
}

#[cfg(not(windows))]
#[test]
fn independent_member_checks_resolve_each_output_once() {
    let workspace = tempfile::tempdir().unwrap();
    let paths = (0..50)
        .map(|index| format!("out/file-{index}"))
        .collect::<BTreeSet<_>>();
    RESOLUTIONS.set(0);
    let independent =
        independent_members(workspace.path(), &paths, paths.iter().map(String::as_str));
    assert_eq!(independent.len(), paths.len());
    assert_eq!(RESOLUTIONS.get(), paths.len());
    RESOLUTIONS.set(0);
    assert!(independent_members(workspace.path(), &paths, std::iter::empty()).is_empty());
    assert_eq!(RESOLUTIONS.get(), 0);
    assert!(independent_members(
        workspace.path(),
        &BTreeSet::from(["tree".into(), "tree/child".into()]),
        std::iter::once("tree")
    )
    .is_empty());
}

#[test]
fn a_sole_matching_output_needs_no_alias_proof() {
    let workspace = tempfile::tempdir().unwrap();
    let paths = BTreeSet::from(["résumé".into()]);
    RESOLUTIONS.set(0);
    assert_eq!(
        independent_members(workspace.path(), &paths, std::iter::once("résumé")),
        BTreeSet::from(["résumé"])
    );
    assert_eq!(RESOLUTIONS.get(), 0);
}

#[cfg(not(windows))]
#[test]
fn refills_resolve_the_candidate_struo_and_recheck_every_active_result() {
    let workspace = tempfile::tempdir().unwrap();
    let active = [result("b"), result("c")];
    RESOLUTIONS.set(0);
    assert!(disjoint_from_results(
        workspace.path(),
        &result("a"),
        active.iter()
    ));
    assert_eq!(RESOLUTIONS.get(), 3);
}

#[cfg(not(windows))]
#[test]
fn directory_creation_does_not_conflict_with_descendants_or_unrelated_outputs() {
    let workspace = tempfile::tempdir().unwrap();
    assert!(!creation_conflicts(
        workspace.path(),
        "out",
        &BTreeSet::from(["out/a".into(), "x".into()])
    ));
    assert!(creation_conflicts(
        workspace.path(),
        "out/child",
        &BTreeSet::from(["out".into()])
    ));
    assert!(creation_conflicts(
        workspace.path(),
        "out",
        &BTreeSet::from(["out".into()])
    ));
}
