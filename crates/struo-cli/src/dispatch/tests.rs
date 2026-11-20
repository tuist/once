use super::*;

#[cfg(unix)]
#[test]
fn mcp_workspace_override_is_canonicalized() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    let alias = temporary.path().join("alias");
    std::fs::create_dir(&workspace).unwrap();
    std::os::unix::fs::symlink(&workspace, &alias).unwrap();

    assert_eq!(
        resolve_mcp_workspace(temporary.path(), Some(alias)).unwrap(),
        std::fs::canonicalize(workspace).unwrap()
    );
}

#[test]
fn default_build_target_falls_back_to_the_lone_native_project_resolver() {
    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join("App.xcodeproj");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(project.join("project.pbxproj"), "not a project").unwrap();

    assert_eq!(
        resolve_build_targets(temporary.path(), None, false).unwrap(),
        vec!["xcode".to_string()]
    );
}

#[test]
fn explicit_target_bypasses_default_selection_for_build_and_lint() {
    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join("App.xcodeproj");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(project.join("project.pbxproj"), "not a project").unwrap();

    assert_eq!(
        resolve_build_targets(temporary.path(), Some("xcode".to_string()), false).unwrap(),
        vec!["xcode".to_string()]
    );
    assert_eq!(
        resolve_lint_targets(temporary.path(), Some("xcode".to_string()), false).unwrap(),
        vec!["xcode".to_string()]
    );
}

#[test]
fn targetless_lint_errors_when_no_lint_target_is_declared() {
    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join("App.xcodeproj");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(project.join("project.pbxproj"), "not a project").unwrap();

    // The bare xcode fixture exposes build at the kind level but not lint,
    // so targetless lint has nothing to fall back to and errors either
    // with the workspace-owned message or by surfacing the graph load
    // failure. What matters is that it fails cleanly instead of panicking.
    resolve_lint_targets(temporary.path(), None, false)
        .expect_err("bare xcode fixture has no lint-capable target");
}
