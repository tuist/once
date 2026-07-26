use std::fs;
use std::path::Path;

use once_frontend::analysis::{AnalysisEngine, DeclaredActionOperation};
use once_frontend::{
    built_in_target_kind_schema, load_target_kind_example, AttrValue, TargetKindExampleBundle,
};
use tempfile::TempDir;

#[test]
fn cmake_target_kinds_are_discoverable_with_the_shared_example() {
    for kind in ["cmake_project", "cmake_workspace", "cmake_target"] {
        let schema = built_in_target_kind_schema(kind)
            .unwrap_or_else(|| panic!("missing target kind schema `{kind}`"));
        assert!(
            schema
                .examples
                .iter()
                .any(|example| example.slug == "cmake-project-minimal"),
            "`{kind}` should expose the CMake starter"
        );
        assert!(
            schema
                .source_references
                .iter()
                .any(|reference| reference.url.contains("cmake-file-api")),
            "`{kind}` should link the CMake file interface"
        );
    }
}

#[test]
fn cmake_snapshot_expands_to_queryable_logical_targets() {
    let tmp = materialized_cmake_example();
    let graph = once_frontend::load_graph_workspace(tmp.path()).expect("CMake graph loads");

    let workspace = graph
        .iter()
        .find(|target| target.label.id == "CMakeGraph")
        .expect("CMakeGraph target");
    assert_eq!(workspace.kind, "cmake_workspace");
    assert_eq!(workspace.deps, vec!["cmake-greeting"]);

    let imported = graph
        .iter()
        .find(|target| target.label.id == "cmake-greeting")
        .expect("imported greeting target");
    assert_eq!(imported.kind, "cmake_target");
    assert_eq!(
        imported.attrs.get("cmake_name").and_then(AttrValue::as_str),
        Some("greeting")
    );
    assert_eq!(
        imported.attrs.get("cmake_type").and_then(AttrValue::as_str),
        Some("STATIC_LIBRARY")
    );
    assert_eq!(imported.srcs, vec!["src/greeting.c"]);
    assert!(imported.diagnostics.is_empty());
}

#[test]
fn cmake_snapshot_rejects_changed_configuration_inputs() {
    let tmp = materialized_cmake_example();
    let cmake_lists = tmp.path().join("CMakeLists.txt");
    let mut contents = fs::read_to_string(&cmake_lists).expect("read CMakeLists.txt");
    contents.push_str("\nadd_compile_definitions(GREETING_CHANGED=1)\n");
    fs::write(cmake_lists, contents).expect("change CMakeLists.txt");

    let error = once_frontend::load_graph_workspace(tmp.path())
        .expect_err("changed CMakeLists.txt should make the snapshot stale");
    let message = error.to_string();
    assert!(message.contains("is stale"), "{message}");
    assert!(message.contains("CMakeLists.txt"), "{message}");
}

#[cfg(unix)]
#[test]
fn cmake_project_declares_one_coarse_build_and_exact_product_staging() {
    let tmp = materialized_cmake_example();
    let tools = tmp.path().join("tools");
    fs::create_dir_all(&tools).expect("create fake tool directory");
    let cmake = tools.join("cmake");
    let ninja = tools.join("ninja");
    write_executable(&cmake, "#!/bin/sh\nprintf 'cmake version 4.3.2\\n'\n");
    write_executable(&ninja, "#!/bin/sh\nprintf '1.13.2\\n'\n");

    let graph = once_frontend::load_graph_workspace(tmp.path()).expect("CMake graph loads");
    let mut project = graph
        .into_iter()
        .find(|target| target.label.id == "CMakeProject")
        .expect("CMakeProject target");
    project.attrs.insert(
        "cmake".to_string(),
        AttrValue::String(cmake.to_string_lossy().into_owned()),
    );
    project.attrs.insert(
        "build_program".to_string(),
        AttrValue::String(ninja.to_string_lossy().into_owned()),
    );

    let result = AnalysisEngine::for_workspace(tmp.path())
        .expect("analysis engine")
        .analyze_target(&project, tmp.path(), &[])
        .expect("CMakeProject analysis");

    let build = result
        .actions
        .iter()
        .find(|action| action.identifier.as_deref() == Some("CMakeProject:cmake-build"))
        .expect("coarse CMake build action");
    assert!(build.operation.is_none());
    assert!(build.argv.iter().any(|arg| arg == "-P"));
    assert_eq!(build.outputs.len(), 1);
    assert!(build.outputs[0].ends_with("/cmake-build/libgreeting.a"));
    assert_eq!(build.clean_paths.len(), 1);
    assert!(build.clean_paths[0].ends_with("/cmake-build"));

    let staged = result
        .actions
        .iter()
        .find(|action| {
            action.identifier.as_deref() == Some("CMakeProject:cmake-product:libgreeting.a")
        })
        .expect("product staging action");
    match staged.operation.as_ref() {
        Some(DeclaredActionOperation::CopyPath {
            sources,
            destination,
            ..
        }) => {
            assert_eq!(sources, &build.outputs);
            assert_eq!(destination, ".once/out/CMakeProject/products/libgreeting.a");
        }
        operation => panic!("expected file staging action, got {operation:?}"),
    }
    assert_eq!(
        result.provider["default_output"],
        ".once/out/CMakeProject/products/libgreeting.a"
    );
}

fn materialized_cmake_example() -> TempDir {
    let schema = built_in_target_kind_schema("cmake_project").expect("cmake_project schema");
    let bundle = load_target_kind_example(&schema, "cmake-project-minimal")
        .expect("CMake example materializes");
    let tmp = TempDir::new().expect("tempdir");
    materialize(tmp.path(), &bundle);
    tmp
}

fn materialize(root: &Path, example: &TargetKindExampleBundle) {
    for file in &example.files {
        let path = root.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create example directory");
        }
        fs::write(path, &file.contents).expect("write example file");
    }
}

#[cfg(unix)]
fn write_executable(path: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;

    fs::write(path, contents).expect("write executable");
    let mut permissions = fs::metadata(path).expect("read permissions").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("set executable permissions");
}
