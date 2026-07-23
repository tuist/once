use super::*;
use tempfile::TempDir;

#[test]
fn materializes_a_starter_and_returns_targets_and_next_calls() {
    let temporary = TempDir::new().unwrap();

    let result =
        materialize_example_value(temporary.path(), "rust_library", "rust-library-minimal", "")
            .unwrap();

    assert!(result.materialized);
    assert!(!result.created_files.is_empty());
    assert!(temporary.path().join("crates/hello/once.toml").is_file());
    assert!(result
        .targets
        .iter()
        .any(|target| target.id == "crates/hello/hello"));
    assert!(result
        .suggested_calls
        .iter()
        .any(|call| call.tool == "once_build_target"));
    assert_eq!(result.workspace_validation.as_ref().unwrap()["valid"], true);
}

#[test]
fn an_identical_retry_is_idempotent() {
    let temporary = TempDir::new().unwrap();
    materialize_example_value(temporary.path(), "rust_library", "rust-library-minimal", "")
        .unwrap();

    let retried =
        materialize_example_value(temporary.path(), "rust_library", "rust-library-minimal", "")
            .unwrap();

    assert!(retried.materialized);
    assert!(retried.created_files.is_empty());
    assert!(!retried.unchanged_files.is_empty());
}

#[test]
fn a_conflict_rejects_every_write() {
    let temporary = TempDir::new().unwrap();
    std::fs::create_dir_all(temporary.path().join("crates/hello")).unwrap();
    std::fs::write(temporary.path().join("crates/hello/once.toml"), "different").unwrap();

    let result =
        materialize_example_value(temporary.path(), "rust_library", "rust-library-minimal", "")
            .unwrap();

    assert!(!result.materialized);
    assert!(!result.conflicts.is_empty());
    assert!(!temporary.path().join("crates/hello/src/lib.rs").exists());
}

#[test]
fn a_nested_starter_relocates_internal_target_references() {
    let temporary = TempDir::new().unwrap();

    let result = materialize_example_value(
        temporary.path(),
        "go_binary",
        "go-comprehensive",
        "samples/go",
    )
    .unwrap();

    assert!(result.materialized);
    assert_eq!(result.workspace_validation.as_ref().unwrap()["valid"], true);
    assert!(result
        .targets
        .iter()
        .any(|target| target.id == "samples/go/Hello"));
    let manifest = std::fs::read_to_string(temporary.path().join("samples/go/once.toml")).unwrap();
    assert!(manifest.contains("samples/go/Greeting"));
}

#[test]
fn suggested_execution_calls_exclude_helper_target_kinds() {
    let temporary = TempDir::new().unwrap();

    let result = materialize_example_value(
        temporary.path(),
        "rust_binary",
        "rust-binary-with-crate",
        "",
    )
    .unwrap();

    let execution_targets = result
        .suggested_calls
        .iter()
        .filter(|call| call.tool == "once_build_target" || call.tool == "once_run_target")
        .map(|call| call.arguments["target"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(execution_targets, ["apps/hello/hello", "apps/hello/hello"]);
}
