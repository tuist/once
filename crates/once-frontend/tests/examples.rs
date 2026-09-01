//! Integration tests that materialize every bundled `TargetKindExample` and
//! load it as a real workspace. This is the rot-prevention invariant
//! the doc-less foundation depends on: if a target kind schema changes in a
//! way that breaks one of the starter examples, this test fails and
//! the example has to be updated alongside the target kind.
//!
//! Scope: this test performs the cheap parse and diagnostic checks that can
//! run anywhere. The portable executable starter matrix runs through
//! `mise run examples:verify-portable`. Platform-specific starters remain
//! covered by their dedicated toolchain setup and action tests.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::Command;

#[cfg(unix)]
use once_frontend::analysis::{AnalysisEngine, AnalysisResult};
use once_frontend::{
    built_in_target_kind_schemas_result, load_target_kind_example, validate_workspace,
};
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
            if !example.platforms.is_empty()
                && !example
                    .platforms
                    .iter()
                    .any(|platform| platform == std::env::consts::OS)
            {
                // Platform-specific starters (for example an Xcode project whose
                // resolver reads `project.pbxproj` through `plutil`) only load
                // where their toolchain exists. They still materialize and carry
                // meta everywhere; their resolution is covered on the platforms
                // they name.
                continue;
            }
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
            let diagnostics = validate_workspace(tmp.path()).unwrap_or_else(|err| {
                panic!(
                    "example `{}` (target kind `{}`) failed workspace validation: {err}",
                    example.slug, schema.kind
                )
            });
            assert!(
                diagnostics.is_empty(),
                "example `{}` (target kind `{}`) failed workspace validation: {diagnostics:?}",
                example.slug,
                schema.kind
            );
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
            assert!(
                bundle
                    .files
                    .iter()
                    .all(|file| !file.path.split('/').any(|component| component == ".once")),
                "example `{}` (target kind `{}`) exposes Once runtime state",
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

#[cfg(unix)]
#[test]
fn cargo_native_project_loads_without_a_once_manifest() {
    let schemas = built_in_target_kind_schemas_result().expect("built-in target kind schemas load");
    let schema = schemas
        .iter()
        .find(|schema| schema.kind == "cargo_workspace")
        .expect("cargo_workspace schema");
    let bundle = load_target_kind_example(schema, "cargo-workspace-native-project")
        .expect("Cargo native project example materializes");
    let tmp = TempDir::new().expect("tempdir");
    materialize(tmp.path(), &bundle);
    fs::remove_file(tmp.path().join("once.toml")).expect("remove explicit Once manifest");

    let graph = once_frontend::load_graph_workspace(tmp.path())
        .expect("Cargo native project graph loads without once.toml");
    let kinds_by_id = graph
        .iter()
        .map(|target| (target.label.id.as_str(), target.kind.as_str()))
        .collect::<BTreeMap<_, _>>();

    assert_eq!(kinds_by_id.get("cargo"), Some(&"cargo_workspace"));
    assert_eq!(
        kinds_by_id.get("cargo_once_native_project_example"),
        Some(&"rust_library")
    );
    assert_eq!(
        kinds_by_id.get("cargo_once_native_project_example_unit_tests"),
        Some(&"rust_test")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn swift_package_native_project_loads_without_a_once_manifest() {
    let schemas = built_in_target_kind_schemas_result().expect("built-in target kind schemas load");
    let schema = schemas
        .iter()
        .find(|schema| schema.kind == "swift_package_workspace")
        .expect("swift_package_workspace schema");
    let bundle = load_target_kind_example(schema, "swift-package-workspace-native-project")
        .expect("Swift Package Manager native project example materializes");
    let tmp = TempDir::new().expect("tempdir");
    materialize(tmp.path(), &bundle);
    fs::remove_file(tmp.path().join("once.toml")).expect("remove explicit Once manifest");

    let graph = once_frontend::load_graph_workspace(tmp.path())
        .expect("Swift Package Manager native project graph loads without once.toml");
    assert!(
        !tmp.path().join(".build").exists(),
        "Swift package graph loading must not create Swift Package Manager build state"
    );
    let kinds_by_id = graph
        .iter()
        .map(|target| (target.label.id.as_str(), target.kind.as_str()))
        .collect::<BTreeMap<_, _>>();

    assert_eq!(
        kinds_by_id.get("swift_package"),
        Some(&"swift_package_workspace")
    );
    assert_eq!(
        kinds_by_id.get("SwiftPackage_OnceNativeSwiftPackage_OnceNativeSwiftPackage"),
        Some(&"apple_library")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn swift_package_workspace_keeps_all_targets_of_a_library_product() {
    let tmp = TempDir::new().expect("tempdir");
    fs::create_dir_all(tmp.path().join("Sources/First")).expect("first source directory");
    fs::create_dir_all(tmp.path().join("Sources/Second")).expect("second source directory");
    fs::write(
        tmp.path().join("Package.swift"),
        r#"// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "MultiProduct",
    products: [.library(name: "MultiProduct", targets: ["First", "Second"])],
    targets: [.target(name: "First"), .target(name: "Second")]
)
"#,
    )
    .expect("package manifest");
    fs::write(
        tmp.path().join("Sources/First/First.swift"),
        "public func first() {}\n",
    )
    .expect("first source");
    fs::write(
        tmp.path().join("Sources/Second/Second.swift"),
        "public func second() {}\n",
    )
    .expect("second source");

    let graph = once_frontend::load_graph_workspace(tmp.path())
        .expect("Swift package workspace with a multi-target product loads");
    let workspace = graph
        .iter()
        .find(|target| target.label.id == "swift_package")
        .expect("Swift package workspace target");

    assert_eq!(
        workspace.deps,
        vec![
            "SwiftPackage_MultiProduct_First",
            "SwiftPackage_MultiProduct_Second",
        ]
    );
}

#[cfg(target_os = "macos")]
fn run_git(directory: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(directory)
        .status()
        .expect("run git");
    assert!(status.success(), "git command failed");
}

#[cfg(target_os = "macos")]
fn local_swift_package(root: &Path) -> (String, String) {
    let remote = root.join("remote-support");
    fs::create_dir_all(remote.join("Sources/RemoteSupport")).expect("remote source directory");
    fs::write(
        remote.join("Package.swift"),
        r#"// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "RemoteSupport",
    products: [.library(name: "RemoteSupport", targets: ["RemoteSupport"])],
    targets: [.target(name: "RemoteSupport")]
)
"#,
    )
    .expect("remote package manifest");
    fs::write(
        remote.join("Sources/RemoteSupport/RemoteSupport.swift"),
        "public func remoteValue() -> Int { 42 }\n",
    )
    .expect("remote source");
    for args in [
        &["init", "--quiet"][..],
        &["config", "user.email", "once@example.invalid"][..],
        &["config", "user.name", "Once tests"][..],
        &["add", "."][..],
        &["commit", "--quiet", "-m", "initial"][..],
    ] {
        run_git(&remote, args);
    }
    let revision = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&remote)
            .output()
            .expect("read remote revision")
            .stdout,
    )
    .expect("remote revision is utf-8")
    .trim()
    .to_string();
    (format!("file://{}", remote.display()), revision)
}

#[cfg(target_os = "macos")]
#[test]
fn swift_package_native_project_lowers_remote_packages_directly() {
    let tmp = TempDir::new().expect("tempdir");
    let (remote_url, revision) = local_swift_package(tmp.path());

    fs::create_dir_all(tmp.path().join("Sources/App")).expect("source directory");
    fs::create_dir_all(tmp.path().join("Tests/AppTests")).expect("test source directory");
    fs::write(
        tmp.path().join("Package.swift"),
        format!(
            r#"// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "LazyRemote",
    dependencies: [
        .package(url: "{remote_url}", exact: "1.0.0"),
    ],
    targets: [
        .target(name: "App", dependencies: [
            .product(name: "RemoteSupport", package: "remote-support"),
        ]),
        .testTarget(name: "AppTests", dependencies: ["App"]),
    ]
)
"#
        ),
    )
    .expect("package manifest");
    fs::write(
        tmp.path().join("Package.resolved"),
        format!(
            r#"{{
  "version": 3,
  "pins": [
    {{
      "identity": "remote-support",
      "kind": "remoteSourceControl",
      "location": "{remote_url}",
      "state": {{
        "revision": "{revision}",
        "version": "1.0.0"
      }}
    }}
  ]
}}
"#
        ),
    )
    .expect("package lock");
    fs::write(
        tmp.path().join("Sources/App/App.swift"),
        "import RemoteSupport\n",
    )
    .expect("source file");
    fs::write(
        tmp.path().join("Tests/AppTests/AppTests.swift"),
        "import XCTest\n@testable import App\n",
    )
    .expect("test source file");

    let graph = once_frontend::load_graph_workspace(tmp.path())
        .expect("native graph loads with a locked remote package");
    assert!(
        !tmp.path().join(".build").exists(),
        "graph loading must not create Swift Package Manager build state"
    );
    assert!(
        tmp.path()
            .join(".once/swift-package-packages/remote-support/Package.swift")
            .exists(),
        "the pinned remote source must be materialized for direct compilation"
    );
    assert!(
        !graph
            .iter()
            .any(|target| target.kind == "swift_package_dependencies"),
        "native package lowering must not delegate compilation to Swift Package Manager"
    );
    let remote = graph
        .iter()
        .find(|target| {
            target.kind == "apple_library"
                && target.attrs.get("module_name")
                    == Some(&AttrValue::String("RemoteSupport".to_string()))
        })
        .expect("directly lowered remote library");
    let app = graph
        .iter()
        .find(|target| target.label.id == "SwiftPackage_LazyRemote_App")
        .expect("first-party app target");
    assert!(app.deps.contains(&remote.label.id));
    let tests = graph
        .iter()
        .find(|target| target.label.id == "SwiftPackage_LazyRemote_AppTests")
        .expect("directly lowered test bundle");
    assert_eq!(tests.kind, "apple_test_bundle");
    assert_eq!(
        tests.attrs.get("product_name"),
        Some(&AttrValue::String("AppTests".to_string()))
    );
    assert!(!tests.attrs.contains_key("module_name"));
    assert!(tests.deps.contains(&app.label.id));
}

#[cfg(unix)]
#[test]
fn mix_native_project_lints_in_the_development_environment() {
    let schemas = built_in_target_kind_schemas_result().expect("built-in target kind schemas load");
    let schema = schemas
        .iter()
        .find(|schema| schema.kind == "mix_workspace")
        .expect("mix_workspace schema");
    let bundle = load_target_kind_example(schema, "mix-workspace-native-project")
        .expect("Mix native project example materializes");
    let tmp = TempDir::new().expect("tempdir");
    materialize(tmp.path(), &bundle);

    let graph = once_frontend::load_graph_workspace(tmp.path()).expect("Mix example graph loads");
    let lint = graph
        .iter()
        .find(|target| target.label.id == "mix_lint")
        .expect("mix_lint target");

    assert_eq!(
        lint.attrs.get("mix_env"),
        Some(&AttrValue::String("dev".to_string()))
    );
}

#[test]
fn eslint_starter_includes_a_flat_configuration() {
    let schemas = built_in_target_kind_schemas_result().expect("built-in target kind schemas load");
    let schema = schemas
        .iter()
        .find(|schema| schema.kind == "eslint_lint")
        .expect("eslint_lint schema");
    let bundle =
        load_target_kind_example(schema, "eslint-lint-minimal").expect("example materializes");

    assert!(bundle
        .files
        .iter()
        .any(|file| file.path == "eslint.config.mjs"));
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
    assert!(
        staged_sources
            .iter()
            .any(|source| source.ends_with("libc++_shared.so")),
        "{staged_sources:?}"
    );
    assert!(result
        .actions
        .iter()
        .any(|action| action.identifier.as_deref() == Some("SharedRust:rustc:android")));
    assert!(!result
        .actions
        .iter()
        .any(|action| action.identifier.as_deref() == Some("SharedRust:rustc:apple")));
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
        .analyze_target(
            android_app,
            root,
            &native_mobile_android_dep_providers(root),
        )
        .expect("AndroidApp analysis")
}

#[cfg(unix)]
fn native_mobile_android_dep_providers(root: &Path) -> [serde_json::Value; 2] {
    let fake_ndk = root.join("android-ndk");
    for tag in [
        "darwin-arm64",
        "darwin-x86_64",
        "linux-arm64",
        "linux-x86_64",
    ] {
        let bin_dir = fake_ndk
            .join("toolchains/llvm/prebuilt")
            .join(tag)
            .join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("clang"), "").unwrap();
    }
    let fake_ndk = fake_ndk.to_string_lossy().into_owned();
    [
        json!({
            "label_id": "SharedSwiftAndroid",
            "target_kind": "swift_android_library",
            "android_native_libraries": [
                {"abi": "arm64-v8a", "path": ".once/out/SharedSwiftAndroid/libSharedSwift.so"},
                {"abi": "arm64-v8a", "path": ".once/out/SharedSwiftAndroid/libc++_shared.so"}
            ],
            "transitive_android_native_libraries": [
                {"abi": "arm64-v8a", "path": ".once/out/SharedSwiftAndroid/libSharedSwift.so"},
                {"abi": "arm64-v8a", "path": ".once/out/SharedSwiftAndroid/libc++_shared.so"}
            ],
        }),
        json!({
            "label": {"package": "", "name": "SharedRust", "id": "SharedRust"},
            "label_id": "SharedRust",
            "target_kind": "rust_mobile_library",
            "attrs": {
                "crate_name": "shared_rust",
                "crate_root": "shared/rust/src/lib.rs",
                "apple_target": "aarch64-apple-ios-sim",
                "android_target": "aarch64-linux-android",
                "android_abi": "arm64-v8a",
                "android_ndk": fake_ndk,
            },
            "srcs": ["shared/rust/src/**/*.rs"],
            "crate_name": "shared_rust",
            "root": "shared/rust/src/lib.rs",
            "apple_target": "aarch64-apple-ios-sim",
            "android_target": "aarch64-linux-android",
            "resolved_sources": ["shared/rust/src/lib.rs"],
            "source_inputs": ["shared/rust/src/lib.rs"],
            "build_script_inputs": [],
            "transitive_sources": ["shared/rust/src/lib.rs"],
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
        fs::write(&path, file.decoded_contents().unwrap()).unwrap_or_else(|err| {
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
