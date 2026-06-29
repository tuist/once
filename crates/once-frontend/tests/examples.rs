//! Integration tests that materialize every bundled `TargetKindExample` and
//! load it as a real workspace. This is the rot-prevention invariant
//! the doc-less foundation depends on: if a target kind schema changes in a
//! way that breaks one of the starter examples, this test fails and
//! the example has to be updated alongside the target kind.
//!
//! Scope: parse + diagnostic check (cheap, runs anywhere). End-to-end
//! build verification for examples whose target kind has an `impl` is
//! intentional follow-up work; it needs an Apple toolchain in the test
//! environment and a configured cache provider.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[cfg(unix)]
use once_frontend::analysis::{AnalysisEngine, AnalysisResult};
use once_frontend::{built_in_target_kind_schemas_result, load_target_kind_example};
#[cfg(unix)]
use once_frontend::{AttrValue, GraphTarget};
#[cfg(unix)]
use serde_json::json;
use tempfile::TempDir;

#[test]
fn every_schema_example_materializes() {
    let schemas = built_in_target_kind_schemas_result().expect("built-in target kind schemas load");
    let mut examples = 0;
    for schema in &schemas {
        for example in &schema.examples {
            examples += 1;
            load_target_kind_example(schema, &example.slug).unwrap_or_else(|err| {
                panic!(
                    "example `{}` (target kind `{}`) failed to materialize: {err}",
                    example.slug, schema.kind
                )
            });
        }
    }
    assert!(examples > 0, "no bundled examples found");
}

#[test]
fn every_schema_example_loads_without_diagnostics() {
    let schemas = built_in_target_kind_schemas_result().expect("built-in target kind schemas load");
    for schema in &schemas {
        for example in &schema.examples {
            let bundle = load_target_kind_example(schema, &example.slug).unwrap_or_else(|err| {
                panic!(
                    "example `{}` (target kind `{}`) failed to materialize: {err}",
                    example.slug, schema.kind
                )
            });
            let tmp = TempDir::new().expect("tempdir");
            materialize(tmp.path(), &bundle);
            let graph = once_frontend::load_graph_workspace(tmp.path()).unwrap_or_else(|err| {
                panic!(
                    "example `{}` (target kind `{}`) failed to load: {err}",
                    example.slug, schema.kind
                )
            });
            assert!(
                !graph.is_empty(),
                "example `{}` (target kind `{}`) declared no targets",
                example.slug,
                schema.kind
            );
            for target in &graph {
                assert!(
                    target.diagnostics.is_empty(),
                    "example `{}` target `{}` emitted diagnostics: {:?}",
                    example.slug,
                    target.label.id,
                    target.diagnostics
                );
            }
            let example_targets = graph
                .iter()
                .filter(|target| target.kind == schema.kind)
                .count();
            assert!(
                example_targets > 0,
                "example `{}` declares no target of target kind `{}`",
                example.slug,
                schema.kind
            );
        }
    }
}

#[test]
fn every_schema_example_carries_meta() {
    let schemas = built_in_target_kind_schemas_result().expect("built-in target kind schemas load");
    for schema in &schemas {
        for example in &schema.examples {
            let bundle =
                load_target_kind_example(schema, &example.slug).expect("example materializes");
            assert!(
                !example.name.is_empty(),
                "example `{}` (target kind `{}`) has an empty `name`",
                example.slug,
                schema.kind
            );
            assert!(
                !example.use_when.is_empty(),
                "example `{}` (target kind `{}`) has an empty `use_when`",
                example.slug,
                schema.kind
            );
            assert!(
                !bundle.files.is_empty(),
                "example `{}` (target kind `{}`) has no files",
                example.slug,
                schema.kind
            );
            assert!(
                bundle.files.iter().any(|f| f.path.ends_with("once.toml")),
                "example `{}` (target kind `{}`) ships no once.toml manifest",
                example.slug,
                schema.kind
            );
        }
    }
}

#[test]
fn every_impl_backed_target_kind_has_a_schema_example() {
    let schemas = built_in_target_kind_schemas_result().expect("built-in target kind schemas load");
    for schema in &schemas {
        if once_frontend::analysis::target_kind_has_impl(&schema.kind)
            .expect("target kind impl lookup")
        {
            assert!(
                !schema.examples.is_empty(),
                "impl-backed target kind `{}` has no bundled starter example",
                schema.kind
            );
        }
    }
}

#[test]
fn native_mobile_shared_code_example_wires_cross_platform_apps() {
    let schemas = built_in_target_kind_schemas_result().expect("built-in target kind schemas load");
    for kind in [
        "swift_android_library",
        "kotlin_apple_framework",
        "rust_mobile_library",
        "android_binary",
        "apple_application",
    ] {
        let schema = schemas
            .iter()
            .find(|schema| schema.kind == kind)
            .unwrap_or_else(|| panic!("missing `{kind}` schema"));
        assert!(
            schema
                .examples
                .iter()
                .any(|example| example.slug == "native-mobile-shared-code-e2e"),
            "`{kind}` should expose the composed shared-code example"
        );
    }

    let swift_schema = schemas
        .iter()
        .find(|schema| schema.kind == "swift_android_library")
        .expect("swift_android_library schema");
    let bundle = load_target_kind_example(swift_schema, "native-mobile-shared-code-e2e")
        .expect("native mobile example materializes");
    let tmp = TempDir::new().expect("tempdir");
    materialize(tmp.path(), &bundle);
    let graph = once_frontend::load_graph_workspace(tmp.path()).expect("example graph loads");
    let by_id = graph
        .iter()
        .map(|target| (target.label.id.as_str(), target))
        .collect::<BTreeMap<_, _>>();

    let android_app = by_id.get("AndroidApp").expect("AndroidApp target");
    assert_eq!(android_app.kind, "android_binary");
    assert_eq!(
        android_app.deps,
        vec!["SharedSwiftAndroid".to_string(), "SharedRust".to_string()]
    );

    let apple_app = by_id.get("AppleApp").expect("AppleApp target");
    assert_eq!(apple_app.kind, "apple_application");
    assert_eq!(
        apple_app.deps,
        vec!["SharedKotlinApple".to_string(), "SharedRust".to_string()]
    );

    assert_eq!(
        by_id
            .keys()
            .filter(|id| id.starts_with("SharedRust"))
            .count(),
        1
    );
    assert!(by_id
        .get("SharedSwiftAndroid")
        .expect("SharedSwiftAndroid target")
        .providers
        .contains(&"android_native_library".to_string()));
    assert!(by_id
        .get("SharedKotlinApple")
        .expect("SharedKotlinApple target")
        .providers
        .contains(&"apple_framework".to_string()));
    let shared_rust = by_id.get("SharedRust").expect("SharedRust target");
    assert_eq!(shared_rust.kind, "rust_mobile_library");
    assert!(shared_rust
        .providers
        .contains(&"android_native_library".to_string()));
    assert!(shared_rust
        .providers
        .contains(&"apple_linkable".to_string()));
}

#[cfg(unix)]
#[test]
fn native_mobile_shared_code_example_declares_android_native_packaging_actions() {
    let tmp = TempDir::new().expect("tempdir");
    let android_app = native_mobile_android_app(tmp.path());
    let result = analyze_native_mobile_android_app(tmp.path(), &android_app);
    let staged_sources = staged_android_native_sources(&result);

    assert!(
        staged_sources
            .iter()
            .any(|source| source.ends_with("libSharedSwift.so")),
        "{staged_sources:?}"
    );
    assert!(
        staged_sources
            .iter()
            .any(|source| source.ends_with("libshared_rust.so")),
        "{staged_sources:?}"
    );
    assert!(declares_android_native_apk_action(&result));
}

#[cfg(unix)]
fn native_mobile_android_app(root: &Path) -> GraphTarget {
    let graph = materialized_native_mobile_graph(root);
    let mut android_app = graph
        .into_iter()
        .find(|target| target.label.id == "AndroidApp")
        .expect("AndroidApp target");
    configure_fake_android_tools(root, &mut android_app);
    android_app
}

#[cfg(unix)]
fn materialized_native_mobile_graph(root: &Path) -> Vec<GraphTarget> {
    let schemas = built_in_target_kind_schemas_result().expect("built-in target kind schemas load");
    let swift_schema = schemas
        .iter()
        .find(|schema| schema.kind == "swift_android_library")
        .expect("swift_android_library schema");
    let bundle = load_target_kind_example(swift_schema, "native-mobile-shared-code-e2e")
        .expect("native mobile example materializes");

    materialize(root, &bundle);
    once_frontend::load_graph_workspace(root).expect("example graph loads")
}

#[cfg(unix)]
fn configure_fake_android_tools(root: &Path, android_app: &mut GraphTarget) {
    let tools = root.join("tools");
    fs::create_dir_all(&tools).unwrap();
    for tool in [
        "aapt2",
        "apksigner",
        "d8",
        "java",
        "javac",
        "jar",
        "kotlinc",
        "zipalign",
    ] {
        write_executable(
            &tools.join(tool),
            "#!/bin/sh\ncase \"$1\" in version|--version|-version) echo \"$0 test\" ;; *) echo \"$0 test\" ;; esac\n",
        );
    }
    let sdk = root.join("android-sdk");
    let attr_paths = [
        ("android_sdk", sdk.to_string_lossy().into_owned()),
        ("compile_sdk", "35".to_string()),
        ("build_tools_version", "35.0.0".to_string()),
        ("signing", "none".to_string()),
        ("aapt2", tools.join("aapt2").to_string_lossy().into_owned()),
        (
            "apksigner",
            tools.join("apksigner").to_string_lossy().into_owned(),
        ),
        ("d8", tools.join("d8").to_string_lossy().into_owned()),
        ("java", tools.join("java").to_string_lossy().into_owned()),
        ("javac", tools.join("javac").to_string_lossy().into_owned()),
        ("jar", tools.join("jar").to_string_lossy().into_owned()),
        (
            "kotlinc",
            tools.join("kotlinc").to_string_lossy().into_owned(),
        ),
        (
            "kotlin_stdlib",
            tools
                .join("kotlin-stdlib.jar")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "zipalign",
            tools.join("zipalign").to_string_lossy().into_owned(),
        ),
    ];
    for (key, value) in attr_paths {
        android_app
            .attrs
            .insert(key.to_string(), AttrValue::String(value));
    }
}

#[cfg(unix)]
fn analyze_native_mobile_android_app(root: &Path, android_app: &GraphTarget) -> AnalysisResult {
    let engine = AnalysisEngine::for_workspace(root).expect("analysis engine");
    engine
        .analyze_target(android_app, root, &native_mobile_android_dep_providers())
        .expect("AndroidApp analysis")
}

#[cfg(unix)]
fn native_mobile_android_dep_providers() -> [serde_json::Value; 2] {
    [
        json!({
            "label_id": "SharedSwiftAndroid",
            "target_kind": "swift_android_library",
            "android_native_libraries": [
                {"abi": "arm64-v8a", "path": ".once/out/SharedSwiftAndroid/libSharedSwift.so"}
            ],
            "transitive_android_native_libraries": [
                {"abi": "arm64-v8a", "path": ".once/out/SharedSwiftAndroid/libSharedSwift.so"}
            ],
        }),
        json!({
            "label_id": "SharedRust",
            "target_kind": "rust_mobile_library",
            "android_native_libraries": [
                {"abi": "arm64-v8a", "path": ".once/out/SharedRust/android/libshared_rust.so"}
            ],
            "transitive_android_native_libraries": [
                {"abi": "arm64-v8a", "path": ".once/out/SharedRust/android/libshared_rust.so"}
            ],
        }),
    ]
}

#[cfg(unix)]
fn staged_android_native_sources(result: &AnalysisResult) -> Vec<String> {
    result
        .actions
        .iter()
        .filter_map(|action| match &action.operation {
            Some(once_frontend::analysis::DeclaredActionOperation::CopyPath {
                sources,
                destination,
                ..
            }) if destination.contains("native_staging/lib/arm64-v8a") => sources.first(),
            _ => None,
        })
        .cloned()
        .collect()
}

#[cfg(unix)]
fn declares_android_native_apk_action(result: &AnalysisResult) -> bool {
    result.actions.iter().any(|action| {
        action
            .identifier
            .as_deref()
            .is_some_and(|id| id == "android_unsigned_apk_native:AndroidApp")
    })
}

fn materialize(root: &Path, example: &once_frontend::TargetKindExampleBundle) {
    for file in &example.files {
        let path = root.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|err| {
                panic!(
                    "creating {} for example `{}`: {err}",
                    parent.display(),
                    example.slug
                )
            });
        }
        fs::write(&path, &file.contents).unwrap_or_else(|err| {
            panic!(
                "writing {} for example `{}`: {err}",
                path.display(),
                example.slug
            )
        });
    }
}

#[cfg(unix)]
fn write_executable(path: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;

    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}
