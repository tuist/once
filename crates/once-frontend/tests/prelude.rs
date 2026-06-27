use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use once_frontend::analysis::{
    globals_for_prelude, target_kind_has_impl, with_active_store, AnalysisStore,
    DeclaredActionOperation, DeclaredArgFileFormat, DeclaredCopyPathMode, DeclaredPreparePathMode,
};
use once_frontend::{built_in_target_kind_schema, graph_from_targets, Target};
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::list::ListRef;
use tempfile::TempDir;

fn store_for(workspace: &Path, package: &str) -> AnalysisStore {
    AnalysisStore::new(
        workspace.to_path_buf(),
        package.to_string(),
        format!(".once/out/{package}"),
    )
}

fn workspace_arg(workspace: &Path, path: &str) -> String {
    workspace.join(path).to_string_lossy().into_owned()
}

fn apple_prelude_source() -> String {
    format!(
        "{}\n{}",
        include_str!("../prelude/common.star"),
        include_str!("../prelude/apple.star")
    )
}

fn android_prelude_source() -> String {
    format!(
        "{}\n{}",
        include_str!("../prelude/common.star"),
        include_str!("../prelude/android.star")
    )
}

fn all_prelude_source() -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        include_str!("../prelude/common.star"),
        include_str!("../prelude/apple.star"),
        include_str!("../prelude/android.star"),
        include_str!("../prelude/rust.star"),
        include_str!("../prelude/swift.star"),
        include_str!("../prelude/kotlin.star")
    )
}

#[test]
fn target_kind_has_impl_returns_true_for_apple_library() {
    assert!(target_kind_has_impl("apple_library").unwrap());
}

#[test]
fn apple_application_exposes_build_and_run() {
    let target = Target {
        package: "apps/ios".to_string(),
        kind: "apple_application".to_string(),
        name: "App".to_string(),
        deps: vec!["apps/ios/AppKit".to_string()],
        srcs: Vec::new(),
        attrs: BTreeMap::new(),
        typed_attrs: BTreeMap::new(),
    };

    let graph = graph_from_targets(&[target]);
    let app = &graph[0];
    assert_eq!(app.label.id, "apps/ios/App");
    let mut names = app
        .capabilities
        .iter()
        .map(|capability| capability.name.as_str())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, vec!["build", "run"]);
}

#[test]
fn android_binary_exposes_build_and_run() {
    let schema = built_in_target_kind_schema("android_binary").expect("android_binary schema");
    let run = schema
        .capabilities
        .iter()
        .find(|capability| capability.name == "run")
        .expect("android_binary run capability");
    assert_eq!(run.output_groups, vec!["default"]);
    assert_eq!(run.requires_outputs, vec!["apk"]);

    let attr_names = schema
        .attrs
        .iter()
        .map(|attr| attr.name.as_str())
        .collect::<Vec<_>>();
    assert!(attr_names.contains(&"adb"));
    assert!(attr_names.contains(&"adb_serial"));
    assert!(attr_names.contains(&"launch_activity"));
    assert!(attr_names.contains(&"kotlinc"));
    assert!(attr_names.contains(&"kotlin_home"));
    assert!(attr_names.contains(&"kotlin_stdlib"));
    assert!(!attr_names.contains(&"keytool"));
}

#[test]
fn android_target_kind_schemas_expose_all_target_kinds() {
    for kind in ["android_resource", "android_library", "android_binary"] {
        let schema = built_in_target_kind_schema(kind).expect("android target kind schema");
        assert_eq!(schema.kind, kind);
        assert!(
            !schema.examples.is_empty(),
            "{kind} should expose a starter example"
        );
        assert!(
            target_kind_has_impl(kind).unwrap(),
            "{kind} should have an impl"
        );
    }

    let library = built_in_target_kind_schema("android_library").unwrap();
    let attr_names = library
        .attrs
        .iter()
        .map(|attr| attr.name.as_str())
        .collect::<Vec<_>>();
    assert!(attr_names.contains(&"kotlinc_opts"));
    assert!(attr_names.contains(&"kotlinc"));
    assert!(attr_names.contains(&"kotlin_stdlib"));
}

#[test]
fn cross_platform_target_kind_schemas_are_discoverable() {
    let swift =
        built_in_target_kind_schema("swift_android_library").expect("swift_android_library schema");
    assert!(target_kind_has_impl("swift_android_library").unwrap());
    assert!(swift
        .providers
        .iter()
        .any(|p| p == "android_native_library"));
    assert!(swift.providers.iter().any(|p| p == "native_linkable"));
    assert!(swift
        .attrs
        .iter()
        .any(|attr| attr.name == "android_abi" && !attr.required));

    let kotlin = built_in_target_kind_schema("kotlin_apple_framework")
        .expect("kotlin_apple_framework schema");
    assert!(target_kind_has_impl("kotlin_apple_framework").unwrap());
    assert!(kotlin.providers.iter().any(|p| p == "apple_framework"));
    assert!(kotlin.providers.iter().any(|p| p == "native_linkable"));

    let rust = built_in_target_kind_schema("rust_library").expect("rust_library schema");
    assert!(rust.providers.iter().any(|p| p == "apple_linkable"));
    assert!(rust.providers.iter().any(|p| p == "android_native_library"));
    assert!(rust.attrs.iter().any(|attr| attr.name == "android_abi"));
    assert!(rust.attrs.iter().any(|attr| attr.name == "native_linkopts"));
}

#[test]
fn prelude_android_kotlin_toolchain_helpers_resolve_stdlib() {
    let prelude = android_prelude_source();

    let home = eval_prelude_string_function_in(
        &prelude,
        "_android_kotlin_home",
        r#"("/opt/kotlinc/bin/kotlinc")"#,
    )
    .unwrap();
    assert_eq!(home, "/opt/kotlinc");

    let default_stdlib = eval_prelude_string_function_in(
        &prelude,
        "_android_kotlin_stdlib",
        r#"({"kotlin_home": "/opt/kotlinc"}, "/ignored/bin/kotlinc")"#,
    )
    .unwrap();
    assert_eq!(default_stdlib, "/opt/kotlinc/lib/kotlin-stdlib.jar");

    let configured_stdlib = eval_prelude_string_function_in(
        &prelude,
        "_android_kotlin_stdlib",
        r#"({"kotlin_stdlib": "/third_party/kotlin-stdlib.jar"}, "/ignored/bin/kotlinc")"#,
    )
    .unwrap();
    assert_eq!(configured_stdlib, "/third_party/kotlin-stdlib.jar");
}

#[cfg(unix)]
#[test]
fn prelude_android_unsigned_apk_packages_native_libraries() {
    let prelude = android_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "apps/hello",
        "name": "Hello",
        "id": "apps/hello/Hello",
    }},
    "attr": {{}},
    "deps": [],
    "srcs": [],
    "build_dir": ".once/out/apps/hello/Hello",
}}
tools = {{
    "jar": "/jdk/bin/jar",
    "identity": "android-tools",
    "sdk_root": "/sdk",
}}
_android_package_unsigned_apk(
    ctx,
    tools,
    ".once/out/apps/hello/Hello/resources.apk",
    ".once/out/apps/hello/Hello/dex",
    ".once/out/apps/hello/Hello/dex.sha256",
    [{{"abi": "arm64-v8a", "path": ".once/out/shared/libshared.so"}}],
)
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "apps/hello/Hello");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    assert_eq!(store.actions.len(), 5);
    assert_eq!(
        store.actions[0].operation,
        Some(DeclaredActionOperation::CopyPath {
            sources: vec![".once/out/apps/hello/Hello/resources.apk".to_string()],
            destination: ".once/out/apps/hello/Hello/unsigned.apk".to_string(),
            mode: DeclaredCopyPathMode::File,
        })
    );
    assert_eq!(
        store.actions[1].identifier.as_deref(),
        Some("android_unsigned_apk_dex:apps/hello/Hello")
    );
    assert_eq!(
        store.actions[2].operation,
        Some(DeclaredActionOperation::PreparePath {
            path: ".once/out/apps/hello/Hello/native_staging".to_string(),
            mode: DeclaredPreparePathMode::Remove,
        })
    );
    let action = &store.actions[3];
    assert_eq!(
        action.operation,
        Some(DeclaredActionOperation::CopyPath {
            sources: vec![".once/out/shared/libshared.so".to_string()],
            destination: ".once/out/apps/hello/Hello/native_staging/lib/arm64-v8a/libshared.so"
                .to_string(),
            mode: DeclaredCopyPathMode::File,
        })
    );
    assert_eq!(action.inputs, vec![".once/out/shared/libshared.so"]);
    assert!(store.actions[4]
        .argv
        .contains(&".once/out/apps/hello/Hello/native_staging".to_string()));
}

#[test]
fn prelude_android_resource_link_seeds_empty_r_txt() {
    let prelude = android_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "apps/hello",
        "name": "Hello",
        "id": "apps/hello/Hello",
    }},
    "attr": {{
        "application_id": "dev.once.hello",
    }},
    "deps": [],
    "srcs": [],
    "build_dir": ".once/out/apps/hello/Hello",
}}
tools = {{
    "aapt2": "/sdk/build-tools/35.0.0/aapt2",
    "android_jar": "/sdk/platforms/android-35/android.jar",
    "compile_sdk": "35",
    "java": "/jdk/bin/java",
    "javac": "/jdk/bin/javac",
    "identity": "android-tools",
    "sdk_root": "/sdk",
}}
_android_link_resources(
    ctx,
    ctx["attr"],
    tools,
    "apps/hello/AndroidManifest.xml",
    [],
    [],
    False,
    [],
    [],
)
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "apps/hello/Hello");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    assert!(!store.actions.iter().any(|action| {
        action.operation.as_ref().is_some_and(|operation| {
            matches!(
                operation,
                DeclaredActionOperation::WriteFile { path, .. }
                    if path == ".once/out/apps/hello/Hello/R.txt"
            )
        })
    }));
    let compile_tool = store
        .actions
        .iter()
        .find(|action| {
            action
                .identifier
                .as_deref()
                .is_some_and(|id| id == "android_resource_link_tool_compile:apps/hello/Hello")
        })
        .expect("link tool compile action");
    assert_eq!(compile_tool.argv[0], "/jdk/bin/javac");
    let link = store
        .actions
        .iter()
        .find(|action| {
            action
                .identifier
                .as_deref()
                .is_some_and(|id| id == "android_resource_link:apps/hello/Hello")
        })
        .expect("resource link action");
    assert_eq!(
        link.identifier.as_deref(),
        Some("android_resource_link:apps/hello/Hello")
    );
    assert_eq!(link.argv[0], "/jdk/bin/java");
    assert!(link.argv.iter().any(|arg| arg == "OnceAndroidAapt2Link"));
    assert!(link
        .outputs
        .iter()
        .any(|output| output == ".once/out/apps/hello/Hello/R.txt"));
    assert!(link
        .inputs
        .iter()
        .any(|input| input == ".once/out/apps/hello/Hello/aapt2_link_tool/classes"));
}

#[test]
fn prelude_android_java_compile_discovers_generated_sources() {
    let prelude = android_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "apps/hello",
        "name": "Hello",
        "id": "apps/hello/Hello",
    }},
    "attr": {{
        "namespace": "dev.once.hello",
    }},
    "deps": [],
    "srcs": [],
    "build_dir": ".once/out/apps/hello/Hello",
}}
tools = {{
    "android_jar": "/sdk/platforms/android-35/android.jar",
    "javac": "/jdk/bin/javac",
    "java": "/jdk/bin/java",
    "identity": "android-tools",
    "sdk_root": "/sdk",
}}
_android_compile_java(
    ctx,
    ctx["attr"],
    tools,
    ["apps/hello/src/MainActivity.java"],
    ".once/out/apps/hello/Hello/generated/r",
    ".once/out/apps/hello/Hello/generated/r_sources.sha256",
    [],
)
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "apps/hello/Hello");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    let source_list_tool = store
        .actions
        .iter()
        .find(|action| {
            action
                .identifier
                .as_deref()
                .is_some_and(|id| id == "android_java_source_list_tool_compile:apps/hello/Hello")
        })
        .expect("source list tool compile action");
    assert_eq!(source_list_tool.argv[0], "/jdk/bin/javac");
    let source_list = store
        .actions
        .iter()
        .find(|action| {
            action
                .identifier
                .as_deref()
                .is_some_and(|id| id == "android_java_source_list:apps/hello/Hello")
        })
        .expect("source list action");
    assert_eq!(source_list.argv[0], "/jdk/bin/java");
    assert!(source_list
        .argv
        .iter()
        .any(|arg| arg == "OnceAndroidJavaSourceList"));
    assert!(source_list
        .inputs
        .iter()
        .any(|input| input == ".once/out/apps/hello/Hello/generated/r_sources.sha256"));
    let javac = store
        .actions
        .iter()
        .find(|action| {
            action
                .identifier
                .as_deref()
                .is_some_and(|id| id == "android_java_compile:apps/hello/Hello")
        })
        .expect("javac action");
    assert!(javac
        .argv
        .iter()
        .any(|arg| arg == "@.once/out/apps/hello/Hello/java_sources.list"));
}

#[cfg(unix)]
#[test]
fn prelude_swift_android_library_declares_native_provider() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("shared/swift/Sources");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        package_dir.join("Greeting.swift"),
        "public func greeting() {}\n",
    )
    .unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "swiftc":
        return "/toolchains/swift/bin/swiftc"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None):
    if len(argv) >= 2 and argv[1] == "--version":
        return "Swift version test\n"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{
        "package": "shared/swift",
        "name": "SharedSwift",
        "id": "shared/swift/SharedSwift",
    }},
    "attr": {{
        "android_abi": "arm64-v8a",
        "module_name": "SharedSwift",
        "sdk": "/android/sdk",
        "resource_dir": "/swift/android/resources",
        "tools_directory": "/android/ndk/bin",
    }},
    "deps": [{{
        "transitive_swiftmodule_dirs": [".once/out/shared/swift/Dep"],
        "transitive_android_native_libraries": [{{"abi": "arm64-v8a", "path": ".once/out/shared/swift/libdep.so"}}],
    }}],
    "srcs": ["Sources/**/*.swift"],
    "build_dir": ".once/out/shared/swift/SharedSwift",
}}
provider = _swift_android_library_impl(ctx)
result = repr([
    provider["target"],
    provider["android_abi"],
    provider["android_native_libraries"],
    provider["transitive_android_native_libraries"],
])
"#
    );
    let store = store_for(workspace.path(), "shared/swift");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    assert!(out.contains("aarch64-unknown-linux-android28"), "{out}");
    assert!(out.contains("arm64-v8a"), "{out}");
    assert!(out.contains("libSharedSwift.so"), "{out}");
    assert!(out.contains("libdep.so"), "{out}");
    assert_eq!(store.actions.len(), 1);
    let action = &store.actions[0];
    assert_eq!(
        action.identifier.as_deref(),
        Some("swift_android_compile:shared/swift/SharedSwift")
    );
    assert!(action.argv.iter().any(|arg| arg == "-emit-library"));
    assert!(action.argv.iter().any(|arg| arg == "-target"));
    assert!(action.argv.iter().any(|arg| arg == "-tools-directory"));
    assert!(action
        .inputs
        .iter()
        .any(|input| input == "shared/swift/Sources/Greeting.swift"));
    assert!(action
        .inputs
        .iter()
        .any(|input| input == ".once/out/shared/swift/libdep.so"));
}

#[test]
fn prelude_swift_android_native_libraries_skip_empty_records() {
    let prelude = all_prelude_source();
    let out = eval_prelude_function_in(
        prelude,
        "_swift_android_unique_native_libraries",
        r#"([
            {"abi": "", "path": ""},
            {"abi": "arm64-v8a", "path": ".once/out/libshared.so"},
            {"abi": "arm64-v8a", "path": ".once/out/libshared.so"},
            {"abi": "x86_64", "path": ""},
        ])"#,
    )
    .unwrap();

    assert_eq!(
        out,
        "[{\"abi\": \"arm64-v8a\", \"path\": \".once/out/libshared.so\"}]"
    );
}

#[test]
fn prelude_kotlin_apple_target_inference_covers_ios_simulator() {
    let prelude = all_prelude_source();
    let out = eval_prelude_function_in(
        &prelude,
        "_kotlin_apple_default_target",
        r#"("ios", "simulator", "arm64")"#,
    )
    .unwrap();

    assert_eq!(out, "\"ios_simulator_arm64\"");
}

#[cfg(unix)]
#[test]
fn prelude_kotlin_apple_identity_includes_konan_data_dir() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("shared/kotlin/Sources");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(package_dir.join("Greeting.kt"), "fun greeting() = \"hi\"\n").unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "kotlinc-native":
        return "/kotlin/bin/kotlinc-native"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None):
    if len(argv) >= 2 and argv[1] == "-version":
        return "kotlinc-native test\n"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{
        "package": "shared/kotlin",
        "name": "SharedKotlin",
        "id": "shared/kotlin/SharedKotlin",
    }},
    "attr": {{
        "platform": "ios",
        "sdk_variant": "simulator",
        "arch": "arm64",
        "module_name": "SharedKotlin",
        "konan_data_dir": "/tmp/konan",
    }},
    "deps": [],
    "srcs": ["Sources/**/*.kt"],
    "build_dir": ".once/out/shared/kotlin/SharedKotlin",
}}
provider = _kotlin_apple_framework_impl(ctx)
result = repr(provider["framework_path"])
"#
    );
    let store = store_for(workspace.path(), "shared/kotlin");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert!(out.unwrap().contains("SharedKotlin.framework"));
    let identity = store.actions[1].toolchain_identity.as_deref().unwrap();
    assert!(
        identity.contains("\x00konan_data_dir\x00/tmp/konan"),
        "{identity:?}"
    );
}

#[test]
fn prelude_android_rejects_rust_rlib_native_dep() {
    let prelude = all_prelude_source();
    let err = eval_prelude_function_in(
        prelude,
        "_android_native_libraries",
        r#"([
            {
                "target_kind": "rust_library",
                "label_id": "SharedRust",
                "crate_type": "rlib",
                "rlib": ".once/out/libshared.rlib",
            },
        ], "AndroidApp")"#,
    )
    .unwrap_err();

    assert!(
        err.contains("does not provide an Android shared library"),
        "{err}"
    );
}

#[test]
fn prelude_apple_rejects_rust_rlib_native_dep() {
    let prelude = all_prelude_source();
    let err = eval_prelude_function_in(
        prelude,
        "_validate_apple_native_deps",
        r#"([
            {
                "target_kind": "rust_library",
                "label_id": "SharedRust",
                "crate_type": "rlib",
                "rlib": ".once/out/libshared.rlib",
            },
        ], "AppleApp")"#,
    )
    .unwrap_err();

    assert!(
        err.contains("does not provide an Apple static library"),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn prelude_rust_native_outputs_emit_mobile_provider_fields() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("shared/rust/src");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(package_dir.join("lib.rs"), "pub fn greeting() {}\n").unwrap();
    let fake_ndk = workspace.path().join("android-ndk");
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
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(bin_dir.join("clang"), "").unwrap();
    }
    let fake_linker =
        fake_ndk.join("toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android23-clang");
    let fake_linker_arg = format!("linker={}", fake_linker.to_string_lossy());
    let fake_ndk = fake_ndk.to_string_lossy();
    let fake_linker = fake_linker.to_string_lossy();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "rustc":
        return "/toolchains/rust/bin/rustc"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None):
    if len(argv) >= 3 and argv[1] == "--print" and argv[2] == "sysroot":
        return "/toolchains/rust\n"
    if len(argv) >= 2 and argv[1] == "--version":
        return "rustc test\nhost: x86_64-unknown-linux-gnu\n"
    fail("unexpected host_command: " + str(argv))

android_ctx = {{
    "label": {{
        "package": "shared/rust",
        "name": "SharedRustAndroid",
        "id": "shared/rust/SharedRustAndroid",
    }},
    "attr": {{
        "crate_name": "shared_rust",
        "crate_root": "src/lib.rs",
        "target": "aarch64-linux-android",
        "linker": "{fake_linker}",
        "android_ndk": "{fake_ndk}",
    }},
    "deps": [],
    "srcs": ["src/**/*.rs"],
    "build_dir": ".once/out/shared/rust/SharedRustAndroid",
}}
apple_ctx = {{
    "label": {{
        "package": "shared/rust",
        "name": "SharedRustApple",
        "id": "shared/rust/SharedRustApple",
    }},
    "attr": {{
        "crate_name": "shared_rust",
        "crate_root": "src/lib.rs",
        "target": "aarch64-apple-ios",
        "native_linkopts": ["-lc++"],
    }},
    "deps": [],
    "srcs": ["src/**/*.rs"],
    "build_dir": ".once/out/shared/rust/SharedRustApple",
}}
android = _rust_compile(android_ctx, "cdylib", "src/lib.rs", "libshared_rust.so")
apple = _rust_compile(apple_ctx, "staticlib", "src/lib.rs", "libshared_rust.a")
result = repr([
    android["android_abi"],
    android["android_native_libraries"],
    android["transitive_android_native_libraries"],
    apple["archive"],
    apple["transitive_archives"],
    apple["transitive_linkopts"],
])
"#
    );
    let store = store_for(workspace.path(), "shared/rust");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    assert!(out.contains("arm64-v8a"), "{out}");
    assert!(out.contains("libshared_rust.so"), "{out}");
    assert!(out.contains("libshared_rust.a"), "{out}");
    assert!(out.contains("-lc++"), "{out}");
    assert_eq!(store.actions.len(), 2);
    assert!(store.actions[0].argv.iter().any(|arg| arg == "--target"));
    assert!(store.actions[0]
        .argv
        .iter()
        .any(|arg| arg == &fake_linker_arg));
    assert!(store.actions[1].argv.iter().any(|arg| arg == "--target"));
}

#[cfg(unix)]
#[test]
fn prelude_apple_application_embeds_framework_self_path_output() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("app/Sources");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(package_dir.join("App.swift"), "import Shared\n").unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "xcrun":
        return "/usr/bin/xcrun"
    if name == "codesign":
        return "/usr/bin/codesign"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None):
    if "--find" in argv:
        return "/toolchain/" + argv[len(argv) - 1] + "\n"
    if "--show-sdk-path" in argv:
        return "/sdks/iPhoneSimulator.sdk\n"
    if "--version" in argv:
        return "Swift version test\n"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{
        "package": "app",
        "name": "App",
        "id": "app/App",
    }},
    "attr": {{
        "platform": "ios",
        "bundle_id": "dev.once.App",
        "minimum_os": "17.0",
        "sdk_variant": "simulator",
        "families": ["iphone"],
    }},
    "deps": [{{
        "label_id": "shared/Shared",
        "framework_path": ".once/out/shared/Shared.framework",
        "framework_module_name": "Shared",
        "framework_files": [
            ".once/out/shared/Shared.framework",
            ".once/out/shared/Shared.framework/Shared",
        ],
        "transitive_frameworks": [".once/out/shared/Shared.framework"],
    }}],
    "srcs": ["Sources/**/*.swift"],
    "build_dir": ".once/out/app/App",
    "capability": "build",
}}
provider = _apple_application_impl(ctx)
result = repr(provider["app_path"])
"#
    );
    let store = store_for(workspace.path(), "app");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    assert!(out.contains("App.app"), "{out}");
    let embed = store
        .actions
        .iter()
        .find(|action| {
            action
                .identifier
                .as_deref()
                .is_some_and(|id| id == "apple_application_embed_Shared.framework")
        })
        .expect("embed action");
    assert!(
        embed
            .outputs
            .iter()
            .any(|output| output == ".once/out/app/App.app/Frameworks/Shared.framework"),
        "{:?}",
        embed.outputs
    );
}

#[cfg(unix)]
#[test]
fn prelude_android_kotlin_compile_declares_merged_classes_action() {
    let prelude = android_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "apps/hello",
        "name": "Hello",
        "id": "apps/hello/Hello",
    }},
    "attr": {{
        "kotlinc_opts": ["-Xjsr305=strict"],
    }},
    "deps": [],
    "srcs": [],
    "build_dir": ".once/out/apps/hello/Hello",
}}
tools = {{
    "android_jar": "/sdk/platforms/android-35/android.jar",
    "kotlin_stdlib": "/kotlin/lib/kotlin-stdlib.jar",
    "kotlinc": "/kotlin/bin/kotlinc",
    "identity": "android-tools",
    "sdk_root": "/sdk",
}}
classes_dir, classes_hash = _android_compile_kotlin(
    ctx,
    ctx["attr"],
    tools,
    ["apps/hello/src/MainActivity.kt"],
    ".once/out/apps/hello/Hello/java_classes",
    ".once/out/apps/hello/Hello/classes.sha256",
    ["apps/hello/Greeting.jar", "/kotlin/lib/kotlin-stdlib.jar"],
)
result = repr([classes_dir, classes_hash])
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "apps/hello/Hello");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(
        out.unwrap(),
        "[\".once/out/apps/hello/Hello/classes\", \".once/out/apps/hello/Hello/classes.kotlin.sha256\"]"
    );
    assert_eq!(store.actions.len(), 4);
    assert_eq!(
        store.actions[0].operation,
        Some(DeclaredActionOperation::CopyPath {
            sources: vec![".once/out/apps/hello/Hello/java_classes".to_string()],
            destination: ".once/out/apps/hello/Hello/classes".to_string(),
            mode: DeclaredCopyPathMode::Tree,
        })
    );
    assert_eq!(
        store.actions[1].operation,
        Some(DeclaredActionOperation::WriteFile {
            path: ".once/out/apps/hello/Hello/kotlin_sources.list".to_string(),
            bytes: b"apps/hello/src/MainActivity.kt\n".to_vec(),
        })
    );
    let action = &store.actions[2];
    assert_eq!(
        action.identifier.as_deref(),
        Some("android_kotlin_compile:apps/hello/Hello")
    );
    assert_eq!(
        action.inputs,
        vec![
            "apps/hello/src/MainActivity.kt",
            ".once/out/apps/hello/Hello/classes.sha256",
            ".once/out/apps/hello/Hello/kotlin_sources.list",
            "apps/hello/Greeting.jar",
        ]
    );
    assert_eq!(action.outputs, vec![".once/out/apps/hello/Hello/classes"]);
    assert!(action
        .argv
        .iter()
        .any(|arg| arg.contains("/kotlin/lib/kotlin-stdlib.jar")));
    assert!(action.argv.contains(&"-Xjsr305=strict".to_string()));
    assert_eq!(
        store.actions[3].operation,
        Some(DeclaredActionOperation::WriteTreeDigest {
            root: ".once/out/apps/hello/Hello/classes".to_string(),
            output: ".once/out/apps/hello/Hello/classes.kotlin.sha256".to_string(),
            include_suffixes: vec![],
        })
    );
}

#[cfg(unix)]
#[test]
fn prelude_android_debug_signing_declares_local_keystore_action() {
    let prelude = android_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("apps/hello");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(package_dir.join("debug.keystore"), b"debug-keystore-bytes").unwrap();

    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "apps/hello",
        "name": "Hello",
        "id": "apps/hello/Hello",
    }},
    "attr": {{
        "debug_keystore": "debug.keystore",
    }},
    "deps": [],
    "srcs": [],
    "build_dir": ".once/out/apps/hello/Hello",
}}
tools = {{
    "apksigner": "/sdk/build-tools/35.0.0/apksigner",
    "identity": "android-tools",
    "sdk_root": "/sdk",
}}
apk, keystore = _android_sign_or_copy(
    ctx,
    ctx["attr"],
    tools,
    ".once/out/apps/hello/Hello/aligned.apk",
)
result = repr([apk, keystore])
"#
    );
    let store = store_for(workspace.path(), "apps/hello/Hello");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(
        out.unwrap(),
        "[\".once/out/apps/hello/Hello/Hello.apk\", \".once/out/apps/hello/Hello/debug.keystore\"]"
    );
    assert_eq!(store.actions.len(), 2);
    assert_eq!(
        store.actions[0].operation,
        Some(DeclaredActionOperation::CopyPath {
            sources: vec!["apps/hello/debug.keystore".to_string()],
            destination: ".once/out/apps/hello/Hello/debug.keystore".to_string(),
            mode: DeclaredCopyPathMode::File,
        })
    );
    let action = &store.actions[1];
    assert_eq!(
        action.identifier.as_deref(),
        Some("android_sign:apps/hello/Hello")
    );
    assert_eq!(
        action.inputs,
        vec![
            "apps/hello/debug.keystore",
            ".once/out/apps/hello/Hello/aligned.apk",
            ".once/out/apps/hello/Hello/debug.keystore",
        ]
    );
    assert_eq!(action.outputs, vec![".once/out/apps/hello/Hello/Hello.apk"]);
    assert_eq!(action.argv[0], "/sdk/build-tools/35.0.0/apksigner");
    assert!(action.argv.contains(&"sign".to_string()));
    assert!(action
        .argv
        .contains(&".once/out/apps/hello/Hello/debug.keystore".to_string()));
    let identity = action.toolchain_identity.as_deref().unwrap();
    assert!(
        identity.contains(
            "\x00debug_sign\x00keystore_sha256\x00764ea889b83367ee6a573d3c0f09847e303701bee50a5a9cc068c9c5736fe37f"
        ),
        "{identity:?}"
    );
    assert!(!identity.contains("pass:android"), "{identity:?}");
}

#[test]
fn apple_library_schema_exposes_multi_arch_attributes() {
    let schema = built_in_target_kind_schema("apple_library").expect("apple_library schema");
    let attr_names = schema
        .attrs
        .iter()
        .map(|attr| attr.name.as_str())
        .collect::<Vec<_>>();

    assert!(
        attr_names.contains(&"archs"),
        "apple_library should expose an archs attribute, got {attr_names:?}"
    );
    assert!(
        attr_names.contains(&"mac_catalyst"),
        "apple_library should expose a mac_catalyst attribute, got {attr_names:?}"
    );
}

#[test]
fn apple_library_swift_compile_is_split_into_module_and_archive_actions() {
    let source = include_str!("../prelude/apple.star");

    assert!(source.contains("identifier = \"swift_module_compile_"));
    assert!(source.contains("outputs = [swiftmodule, swiftdoc, swift_objc_header]"));
    assert!(source.contains("identifier = \"swift_archive_compile_"));
    assert!(source.contains("outputs = [swift_archive]"));
}

#[test]
fn target_kind_has_impl_returns_true_for_swift_macro() {
    assert!(target_kind_has_impl("swift_macro").unwrap());
}

#[test]
fn target_kind_has_impl_returns_true_for_all_apple_bundle_kinds() {
    // Every bundled Apple target kind now has a Starlark impl that
    // declares actions; the CLI's generic fallback action is
    // bypassed for these kinds in favour of the Starlark-driven
    // analysis.
    assert!(target_kind_has_impl("apple_framework").unwrap());
    assert!(target_kind_has_impl("apple_application").unwrap());
    assert!(target_kind_has_impl("apple_test_bundle").unwrap());
}

fn eval_prelude_function(
    function_name: &str,
    call_source: &str,
) -> std::result::Result<String, String> {
    let prelude = apple_prelude_source();
    eval_prelude_function_in(prelude, function_name, call_source)
}

fn eval_prelude_function_in(
    prelude: impl AsRef<str>,
    function_name: &str,
    call_source: &str,
) -> std::result::Result<String, String> {
    let prelude = prelude.as_ref();
    let source = format!("{prelude}\nresult = repr({function_name}{call_source})\n");
    eval_prelude_source_to_repr(source)
}

fn eval_prelude_source_to_repr(source: String) -> std::result::Result<String, String> {
    // Build a Starlark module that splices the prelude's source
    // inline and invokes the requested helper. Returning the
    // result as a string via `repr()` keeps the test independent
    // of starlark Value plumbing details.
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse("test.star", source, &Dialect::Standard)
            .map_err(|error| format!("parse: {error:?}"))?;
        let globals = globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        // The prelude calls host_arch() in some helpers, but the
        // resolver path itself doesn't. The host primitives
        // already return inert values outside of an active
        // analysis store, so this evaluates cleanly.
        eval.eval_module(ast, &globals)
            .map_err(|error| format!("eval: {error:?}"))?;
        let result = module
            .get("result")
            .ok_or_else(|| "missing result".to_string())?;
        Ok(result
            .unpack_str()
            .ok_or_else(|| "result was not a string".to_string())?
            .to_string())
    })
}

fn eval_prelude_string_function(
    function_name: &str,
    call_source: &str,
) -> std::result::Result<String, String> {
    let prelude = apple_prelude_source();
    eval_prelude_string_function_in(prelude, function_name, call_source)
}

fn eval_prelude_string_function_in(
    prelude: impl AsRef<str>,
    function_name: &str,
    call_source: &str,
) -> std::result::Result<String, String> {
    let prelude = prelude.as_ref();
    let source = format!("{prelude}\nresult = {function_name}{call_source}\n");
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse("test.star", source, &Dialect::Standard)
            .map_err(|error| format!("parse: {error:?}"))?;
        let globals = globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|error| format!("eval: {error:?}"))?;
        let result = module
            .get("result")
            .ok_or_else(|| "missing result".to_string())?;
        Ok(result
            .unpack_str()
            .ok_or_else(|| "result was not a string".to_string())?
            .to_string())
    })
}

fn starlark_string_literal(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}

#[cfg(unix)]
fn write_executable(path: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, contents).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn prelude_resolve_select_picks_matching_branch() {
    let out = eval_prelude_function(
        "_resolve_select",
        r#"({"select": {"ios": ["FOO"], "macos": ["BAR"]}}, ["ios"], "tgt", "defines")"#,
    )
    .unwrap();
    assert_eq!(out, "[\"FOO\"]");
}

#[test]
fn prelude_resolve_select_falls_back_to_default() {
    let out = eval_prelude_function(
        "_resolve_select",
        r#"({"select": {"macos": "M", "default": "fallback"}}, ["ios"], "tgt", "x")"#,
    )
    .unwrap();
    assert_eq!(out, "\"fallback\"");
}

#[test]
fn prelude_resolve_select_prefers_longest_composite_key() {
    let out = eval_prelude_function(
            "_resolve_select",
            r#"({"select": {"ios": "ios-any", "ios:simulator": "ios-sim"}}, ["ios", "simulator"], "tgt", "x")"#,
        )
        .unwrap();
    assert_eq!(out, "\"ios-sim\"");
}

#[test]
fn prelude_resolve_select_fails_without_default() {
    let err = eval_prelude_function(
        "_resolve_select",
        r#"({"select": {"macos": "M"}}, ["ios"], "tgt", "x")"#,
    )
    .unwrap_err();
    assert!(err.contains("no branch matching"), "{err}");
}

#[test]
fn prelude_cargo_metadata_targets_preserve_rust_target() {
    let prelude = all_prelude_source();
    let out = eval_prelude_function_in(
            &prelude,
            "_cargo_metadata_targets",
            r#"({
                "attrs": {
                    "target": "x86_64-apple-darwin",
                    "vendor_dir": "third_party/rust/vendor",
                },
            }, {
                "packages": [{
                    "id": "registry+https://github.com/rust-lang/crates.io-index#cpufeatures@0.2.17",
                    "name": "cpufeatures",
                    "version": "0.2.17",
                    "source": "registry+https://github.com/rust-lang/crates.io-index",
                    "manifest_path": "/workspace/vendor/cpufeatures-0.2.17/Cargo.toml",
                    "targets": [{
                        "name": "cpufeatures",
                        "kind": ["lib"],
                        "crate_types": ["lib"],
                        "src_path": "/workspace/vendor/cpufeatures-0.2.17/src/lib.rs",
                        "edition": "2018",
                    }],
                }],
                "resolve": {
                    "nodes": [{
                        "id": "registry+https://github.com/rust-lang/crates.io-index#cpufeatures@0.2.17",
                        "features": [],
                        "deps": [],
                    }],
                },
            })"#,
        )
        .unwrap();

    assert!(out.contains("\"target\": \"x86_64-apple-darwin\""), "{out}");
    assert!(
            out.contains("\"srcs\": [\"third_party/rust/vendor/cpufeatures-0.2.17/Cargo.toml\", \"third_party/rust/vendor/cpufeatures-0.2.17/build.rs\", \"third_party/rust/vendor/cpufeatures-0.2.17/src/**/*.rs\"]"),
            "{out}"
        );
}

#[test]
fn prelude_cargo_metadata_targets_normalize_windows_build_script_paths() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
targets = _cargo_metadata_targets({{
    "attrs": {{
        "target": "x86_64-pc-windows-msvc",
        "vendor_dir": "third_party/rust/vendor",
    }},
}}, {{
    "packages": [{{
        "id": "registry+https://github.com/rust-lang/crates.io-index#anyhow@1.0.102",
        "name": "anyhow",
        "version": "1.0.102",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "manifest_path": "C:\\Users\\runneradmin\\.cargo\\registry\\src\\index\\anyhow-1.0.102\\Cargo.toml",
        "targets": [
            {{
                "name": "anyhow",
                "kind": ["lib"],
                "crate_types": ["lib"],
                "src_path": "C:\\Users\\runneradmin\\.cargo\\registry\\src\\index\\anyhow-1.0.102\\src\\lib.rs",
                "edition": "2021",
            }},
            {{
                "name": "build-script-build",
                "kind": ["custom-build"],
                "crate_types": ["bin"],
                "src_path": "C:\\Users\\runneradmin\\.cargo\\registry\\src\\index\\anyhow-1.0.102\\build.rs",
                "edition": "2021",
            }},
        ],
    }}],
    "resolve": {{
        "nodes": [{{
            "id": "registry+https://github.com/rust-lang/crates.io-index#anyhow@1.0.102",
            "features": [],
            "deps": [],
        }}],
    }},
}})
by_name = {{target["name"]: target for target in targets}}
result = repr([
    by_name["anyhow-1.0.102"]["attrs"]["crate_root"],
    by_name["anyhow-1.0.102"]["attrs"]["build_script"],
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(
        out,
        "[\"third_party/rust/vendor/anyhow-1.0.102/src/lib.rs\", \"third_party/rust/vendor/anyhow-1.0.102/build.rs\"]"
    );
}

#[test]
fn prelude_cargo_metadata_windows_features_keep_response_file_cfgs_literal() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "windows"

def host_env(name):
    return ""

def host_which(name):
    fail("unexpected host_which call: " + name)

def host_command(argv, env = None):
    fail("unexpected host_command call")

def _rustc_toolchain(target):
    return ("C:/Rust/bin/rustc.exe", "rustc-test", "x86_64-pc-windows-msvc")

targets = _cargo_metadata_targets({{
    "attrs": {{
        "target": "x86_64-pc-windows-msvc",
        "vendor_dir": "third_party/rust/vendor",
    }},
}}, {{
    "packages": [{{
        "id": "registry+https://github.com/rust-lang/crates.io-index#anyhow@1.0.102",
        "name": "anyhow",
        "version": "1.0.102",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "manifest_path": "C:\\Users\\runneradmin\\.cargo\\registry\\src\\index\\anyhow-1.0.102\\Cargo.toml",
        "targets": [{{
            "name": "anyhow",
            "kind": ["lib"],
            "crate_types": ["lib"],
            "src_path": "C:\\Users\\runneradmin\\.cargo\\registry\\src\\index\\anyhow-1.0.102\\src\\lib.rs",
            "edition": "2021",
        }}],
    }}],
    "resolve": {{
        "nodes": [{{
            "id": "registry+https://github.com/rust-lang/crates.io-index#anyhow@1.0.102",
            "features": ["default"],
            "deps": [],
        }}],
    }},
}})
target = {{target["name"]: target for target in targets}}["anyhow-1.0.102"]
ctx = {{
    "label": {{
        "package": "cargo_dependencies_x86_64_pc_windows_msvc",
        "name": target["name"],
        "id": "cargo_dependencies_x86_64_pc_windows_msvc/" + target["name"],
    }},
    "attr": target["attrs"],
    "deps": [],
    "srcs": target["srcs"],
}}
_rust_compile(ctx, "rlib", "src/lib.rs", "libanyhow.rlib")
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(
        workspace.path(),
        "cargo_dependencies_x86_64_pc_windows_msvc/anyhow-1.0.102",
    );

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    let rustc = store
        .actions
        .iter()
        .find(|action| {
            action.identifier.as_deref()
                == Some("cargo_dependencies_x86_64_pc_windows_msvc/anyhow-1.0.102:rustc")
        })
        .expect("rustc action");
    assert_eq!(rustc.arg_files.len(), 1);
    let arg_file = &rustc.arg_files[0];
    assert_eq!(arg_file.format, DeclaredArgFileFormat::RustcResponse);
    assert!(arg_file
        .args
        .iter()
        .any(|arg| arg == "--cfg=feature=\"default\""));
    assert!(!arg_file
        .args
        .iter()
        .any(|arg| arg == "--cfg=feature=default"));
    assert!(!arg_file
        .args
        .iter()
        .any(|arg| arg == "--cfg=feature=\\\"default\\\""));
    assert!(!arg_file
        .args
        .iter()
        .any(|arg| arg == "--cfg=feature=r#default#"));
}

#[test]
#[allow(clippy::too_many_lines)]
fn prelude_cargo_metadata_windows_omits_unrelated_prior_dependency_search_paths() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "windows"

def host_env(name):
    return ""

def host_which(name):
    fail("unexpected host_which call: " + name)

def host_command(argv, env = None):
    fail("unexpected host_command call")

def _rustc_toolchain(target):
    return ("C:/Rust/bin/rustc.exe", "rustc-test", "x86_64-pc-windows-msvc")

ctx = {{
    "label": {{
        "package": "cargo_dependencies_x86_64_pc_windows_msvc",
        "name": "cargo_dependencies_x86_64_pc_windows_msvc",
        "id": "cargo_dependencies_x86_64_pc_windows_msvc",
    }},
    "attr": {{}},
    "deps": [],
    "srcs": [],
}}
specs = [
    {{
        "name": "alpha-1.0.0",
        "kind": "rust_crate",
        "deps": [],
        "srcs": [],
        "attrs": {{
            "package_name": "alpha",
            "crate_name": "alpha",
            "version": "1.0.0",
            "crate_root": "third_party/rust/vendor/alpha-1.0.0/src/lib.rs",
            "edition": "2021",
        }},
    }},
    {{
        "name": "beta-1.0.0",
        "kind": "rust_crate",
        "deps": [],
        "srcs": [],
        "attrs": {{
            "package_name": "beta",
            "crate_name": "beta",
            "version": "1.0.0",
            "crate_root": "third_party/rust/vendor/beta-1.0.0/src/lib.rs",
            "edition": "2021",
        }},
    }},
]
providers, _ = _cargo_compile_resolved_specs(ctx, specs)
result = repr([provider["label_id"] for provider in providers])
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(
        workspace.path(),
        "cargo_dependencies_x86_64_pc_windows_msvc",
    );

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(
        out.unwrap(),
        "[\"cargo_dependencies_x86_64_pc_windows_msvc/alpha-1.0.0\", \"cargo_dependencies_x86_64_pc_windows_msvc/beta-1.0.0\"]"
    );
    let beta_rustc = store
        .actions
        .iter()
        .find(|action| {
            action.identifier.as_deref()
                == Some("cargo_dependencies_x86_64_pc_windows_msvc/beta-1.0.0:rustc")
        })
        .expect("beta rustc action");
    let arg_file = beta_rustc.arg_files.first().expect("beta response file");
    // beta does not depend on alpha, so an unrelated prior provider must never
    // leak into beta's externs or search path. Folding every prior provider
    // into each crate grew the Windows search set with the whole dependency
    // closure and exhausted the runner's disk.
    assert!(
        !arg_file
            .args
            .iter()
            .any(|arg| arg.contains("cargo_dependencies_x86_64_pc_windows_msvc/alpha-1.0.0")),
        "unrelated prior provider alpha leaked into {:?}",
        arg_file.args
    );
    assert!(!arg_file.args.iter().any(|arg| arg.starts_with("alpha=")));
    assert!(
        !store.actions.iter().any(|action| {
            action
                .outputs
                .iter()
                .any(|output| output.contains("/search/prior-deps"))
        }),
        "no per-crate prior-deps staging directory should be created",
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn prelude_cargo_metadata_windows_proc_macro_deps_from_metadata_are_direct_externs() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "windows"

def host_env(name):
    return ""

def host_which(name):
    fail("unexpected host_which call: " + name)

def host_command(argv, env = None):
    fail("unexpected host_command call")

def _rustc_toolchain(target):
    return ("C:/Rust/bin/rustc.exe", "rustc-test", "x86_64-pc-windows-msvc")

ctx = {{
    "attrs": {{
        "target": "x86_64-pc-windows-msvc",
        "vendor_dir": "third_party/rust/vendor",
    }},
}}

def package(name, version, target_name, kind = "lib"):
    crate_types = ["proc-macro"] if kind == "proc-macro" else ["lib"]
    return {{
        "id": "registry+https://github.com/rust-lang/crates.io-index#" + name + "@" + version,
        "name": name,
        "version": version,
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "manifest_path": "/workspace/vendor/" + name + "-" + version + "/Cargo.toml",
        "targets": [{{
            "name": target_name,
            "kind": [kind],
            "crate_types": crate_types,
            "src_path": "/workspace/vendor/" + name + "-" + version + "/src/lib.rs",
            "edition": "2018",
        }}],
    }}

def dep(name, package, version):
    return {{
        "name": name,
        "pkg": "registry+https://github.com/rust-lang/crates.io-index#" + package + "@" + version,
        "dep_kinds": [{{"kind": None, "target": None}}],
    }}

packages = [
    package("futures-channel", "0.3.32", "futures_channel"),
    package("futures-core", "0.3.32", "futures_core"),
    package("futures-io", "0.3.32", "futures_io"),
    package("futures-macro", "0.3.32", "futures_macro", "proc-macro"),
    package("futures-sink", "0.3.32", "futures_sink"),
    package("futures-task", "0.3.32", "futures_task"),
    package("memchr", "2.8.0", "memchr"),
    package("pin-project-lite", "0.2.17", "pin_project_lite"),
    package("slab", "0.4.12", "slab"),
    package("futures-util", "0.3.32", "futures_util"),
]
metadata = {{
    "packages": packages,
    "resolve": {{
        "nodes": [
            {{"id": package["id"], "features": [], "deps": []}}
            for package in packages
            if package["name"] != "futures-util"
        ] + [{{
            "id": "registry+https://github.com/rust-lang/crates.io-index#futures-util@0.3.32",
            "features": [
                "alloc",
                "async-await",
                "async-await-macro",
                "channel",
                "default",
                "futures-channel",
                "futures-io",
                "futures-macro",
                "futures-sink",
                "io",
                "memchr",
                "sink",
                "slab",
                "std",
            ],
            "deps": [
                dep("futures_channel", "futures-channel", "0.3.32"),
                dep("futures_core", "futures-core", "0.3.32"),
                dep("futures_io", "futures-io", "0.3.32"),
                dep("futures_macro", "futures-macro", "0.3.32"),
                dep("futures_sink", "futures-sink", "0.3.32"),
                dep("futures_task", "futures-task", "0.3.32"),
                dep("memchr", "memchr", "2.8.0"),
                dep("pin_project_lite", "pin-project-lite", "0.2.17"),
                dep("slab", "slab", "0.4.12"),
            ],
        }}],
    }},
}}
specs = _cargo_metadata_targets(ctx, metadata)
deps, _ = _cargo_compile_resolved_specs({{
    "label": {{
        "package": "cargo_dependencies_x86_64_pc_windows_msvc",
        "name": "cargo_dependencies_x86_64_pc_windows_msvc",
        "id": "cargo_dependencies_x86_64_pc_windows_msvc",
    }},
    "attr": {{}},
    "deps": [],
    "srcs": [],
}}, specs)
result = repr([provider["label_id"] for provider in deps])
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(
        workspace.path(),
        "cargo_dependencies_x86_64_pc_windows_msvc",
    );

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert!(out.unwrap().contains("futures-util-0.3.32"));
    let rustc = store
        .actions
        .iter()
        .find(|action| {
            action.identifier.as_deref()
                == Some("cargo_dependencies_x86_64_pc_windows_msvc/futures-util-0.3.32:rustc")
        })
        .expect("futures-util rustc action");
    let arg_file = rustc.arg_files.first().expect("futures-util response file");
    let macro_dir = workspace_arg(
        workspace.path(),
        ".once/out/cargo_dependencies_x86_64_pc_windows_msvc/futures-macro-0.3.32/proc-macro-search",
    );
    let macro_artifact = format!(
        "{macro_dir}/futures_macro-CARGO_DEPENDENCIES_X86_64_PC_WINDOWS_MSVC_FUTURES_MACRO_0_3_32.dll"
    );
    let macro_extern = format!("futures_macro={macro_artifact}");

    // Proc-macros are passed as ordinary externs in the response file, the
    // same way as rlibs and as on other platforms.
    assert!(
        arg_file
            .args
            .windows(2)
            .any(|args| args[0] == "--extern" && args[1] == macro_extern),
        "{macro_extern} extern missing from {:?}",
        arg_file.args
    );
    assert!(
        !rustc
            .argv
            .windows(2)
            .any(|args| args[0] == "--extern" && args[1] == macro_extern),
        "{macro_extern} should not be passed inline: {:?}",
        rustc.argv
    );
}

#[test]
fn prelude_cargo_metadata_targets_split_proc_macro_host_deps() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
targets = _cargo_metadata_targets({{
    "attrs": {{
        "target": "x86_64-apple-darwin",
        "vendor_dir": "third_party/rust/vendor",
    }},
}}, {{
    "packages": [
        {{
            "id": "registry+https://github.com/rust-lang/crates.io-index#quote@1.0.45",
            "name": "quote",
            "version": "1.0.45",
            "source": "registry+https://github.com/rust-lang/crates.io-index",
            "manifest_path": "/workspace/vendor/quote-1.0.45/Cargo.toml",
            "targets": [{{
                "name": "quote",
                "kind": ["lib"],
                "crate_types": ["lib"],
                "src_path": "/workspace/vendor/quote-1.0.45/src/lib.rs",
                "edition": "2018",
            }}],
        }},
        {{
            "id": "registry+https://github.com/rust-lang/crates.io-index#linktime-proc-macro@0.2.0",
            "name": "linktime-proc-macro",
            "version": "0.2.0",
            "source": "registry+https://github.com/rust-lang/crates.io-index",
            "manifest_path": "/workspace/vendor/linktime-proc-macro-0.2.0/Cargo.toml",
            "targets": [{{
                "name": "linktime_proc_macro",
                "kind": ["proc-macro"],
                "crate_types": ["proc-macro"],
                "src_path": "/workspace/vendor/linktime-proc-macro-0.2.0/src/lib.rs",
                "edition": "2021",
            }}],
        }},
    ],
    "resolve": {{
        "nodes": [
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#quote@1.0.45",
                "features": [],
                "deps": [],
            }},
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#linktime-proc-macro@0.2.0",
                "features": [],
                "deps": [{{
                    "name": "quote",
                    "pkg": "registry+https://github.com/rust-lang/crates.io-index#quote@1.0.45",
                    "dep_kinds": [{{"kind": None}}],
                }}],
            }},
        ],
    }},
}})
by_name = {{target["name"]: target for target in targets}}
result = repr([
    by_name["quote-1.0.45"]["attrs"].get("target"),
    by_name["quote-1.0.45-host"]["attrs"].get("target"),
    by_name["linktime-proc-macro-0.2.0"]["attrs"].get("target"),
    by_name["linktime-proc-macro-0.2.0"]["deps"],
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(
        out,
        "[\"x86_64-apple-darwin\", None, None, [\"./quote-1.0.45-host\"]]"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn prelude_cargo_metadata_targets_use_host_metadata_for_host_variants() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "attrs": {{
        "target": "x86_64-apple-darwin",
        "vendor_dir": "third_party/rust/vendor",
    }},
}}
packages = [
    {{
        "id": "registry+https://github.com/rust-lang/crates.io-index#builder@1.0.0",
        "name": "builder",
        "version": "1.0.0",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "manifest_path": "/workspace/vendor/builder-1.0.0/Cargo.toml",
        "targets": [
            {{
                "name": "builder",
                "kind": ["lib"],
                "crate_types": ["lib"],
                "src_path": "/workspace/vendor/builder-1.0.0/src/lib.rs",
                "edition": "2021",
            }},
            {{
                "name": "build-script-build",
                "kind": ["custom-build"],
                "crate_types": ["bin"],
                "src_path": "/workspace/vendor/builder-1.0.0/build.rs",
                "edition": "2021",
            }},
        ],
    }},
    {{
        "id": "registry+https://github.com/rust-lang/crates.io-index#cpufeatures@0.2.17",
        "name": "cpufeatures",
        "version": "0.2.17",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "manifest_path": "/workspace/vendor/cpufeatures-0.2.17/Cargo.toml",
        "targets": [{{
            "name": "cpufeatures",
            "kind": ["lib"],
            "crate_types": ["lib"],
            "src_path": "/workspace/vendor/cpufeatures-0.2.17/src/lib.rs",
            "edition": "2018",
        }}],
    }},
    {{
        "id": "registry+https://github.com/rust-lang/crates.io-index#libc@0.2.186",
        "name": "libc",
        "version": "0.2.186",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "manifest_path": "/workspace/vendor/libc-0.2.186/Cargo.toml",
        "targets": [{{
            "name": "libc",
            "kind": ["lib"],
            "crate_types": ["lib"],
            "src_path": "/workspace/vendor/libc-0.2.186/src/lib.rs",
            "edition": "2021",
        }}],
    }},
]
target_metadata = {{
    "packages": packages,
    "resolve": {{
        "nodes": [
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#builder@1.0.0",
                "features": [],
                "deps": [{{
                    "name": "cpufeatures",
                    "pkg": "registry+https://github.com/rust-lang/crates.io-index#cpufeatures@0.2.17",
                    "dep_kinds": [{{"kind": "build"}}],
                }}],
            }},
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#cpufeatures@0.2.17",
                "features": [],
                "deps": [],
            }},
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#libc@0.2.186",
                "features": [],
                "deps": [],
            }},
        ],
    }},
}}
host_metadata = {{
    "packages": packages,
    "resolve": {{
        "nodes": [
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#builder@1.0.0",
                "features": [],
                "deps": [{{
                    "name": "cpufeatures",
                    "pkg": "registry+https://github.com/rust-lang/crates.io-index#cpufeatures@0.2.17",
                    "dep_kinds": [{{"kind": "build"}}],
                }}],
            }},
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#cpufeatures@0.2.17",
                "features": [],
                "deps": [{{
                    "name": "libc",
                    "pkg": "registry+https://github.com/rust-lang/crates.io-index#libc@0.2.186",
                    "dep_kinds": [{{"kind": None}}],
                }}],
            }},
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#libc@0.2.186",
                "features": [],
                "deps": [],
            }},
        ],
    }},
}}
targets = _cargo_metadata_targets(ctx, target_metadata, host_metadata)
by_name = {{target["name"]: target for target in targets}}
result = repr([
    by_name["cpufeatures-0.2.17"]["deps"],
    by_name["cpufeatures-0.2.17-host"]["deps"],
    by_name["cpufeatures-0.2.17-host"]["attrs"].get("target"),
    by_name["libc-0.2.186-host"]["attrs"].get("target"),
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(out, "[[], [\"./libc-0.2.186-host\"], None, None]");
}

#[test]
fn prelude_cargo_metadata_targets_use_host_metadata_for_proc_macro_features() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "attrs": {{
        "target": "x86_64-apple-darwin",
        "vendor_dir": "third_party/rust/vendor",
    }},
}}
packages = [
    {{
        "id": "registry+https://github.com/rust-lang/crates.io-index#document-features@0.2.12",
        "name": "document-features",
        "version": "0.2.12",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "manifest_path": "/workspace/vendor/document-features-0.2.12/Cargo.toml",
        "targets": [{{
            "name": "document_features",
            "kind": ["proc-macro"],
            "crate_types": ["proc-macro"],
            "src_path": "/workspace/vendor/document-features-0.2.12/lib.rs",
            "edition": "2018",
        }}],
    }},
    {{
        "id": "registry+https://github.com/rust-lang/crates.io-index#litrs@1.0.0",
        "name": "litrs",
        "version": "1.0.0",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "manifest_path": "/workspace/vendor/litrs-1.0.0/Cargo.toml",
        "targets": [{{
            "name": "litrs",
            "kind": ["lib"],
            "crate_types": ["lib"],
            "src_path": "/workspace/vendor/litrs-1.0.0/src/lib.rs",
            "edition": "2021",
        }}],
    }},
]
target_metadata = {{
    "packages": packages,
    "resolve": {{
        "nodes": [
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#document-features@0.2.12",
                "features": [],
                "deps": [],
            }},
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#litrs@1.0.0",
                "features": [],
                "deps": [],
            }},
        ],
    }},
}}
host_metadata = {{
    "packages": packages,
    "resolve": {{
        "nodes": [
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#document-features@0.2.12",
                "features": ["default"],
                "deps": [{{
                    "name": "litrs",
                    "pkg": "registry+https://github.com/rust-lang/crates.io-index#litrs@1.0.0",
                    "dep_kinds": [{{"kind": None}}],
                }}],
            }},
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#litrs@1.0.0",
                "features": [],
                "deps": [],
            }},
        ],
    }},
}}
targets = _cargo_metadata_targets(ctx, target_metadata, host_metadata)
by_name = {{target["name"]: target for target in targets}}
result = repr([
    by_name["document-features-0.2.12"]["attrs"]["features"],
    by_name["document-features-0.2.12"]["deps"],
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(out, "[[\"default\"], [\"./litrs-1.0.0-host\"]]");
}

#[cfg(unix)]
#[test]
fn prelude_rust_build_script_metadata_deps_are_not_duplicated() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{
        "build_script": "build.rs",
        "crate_root": "src/lib.rs",
    }},
    "deps": [{{
        "label_id": "third_party/rust/native",
        "crate_name": "native",
        "rlib": ".once/out/native/libnative.rlib",
        "links": "native",
        "build_script_stdout": ".once/out/native/build-script.stdout",
    }}],
    "srcs": ["src/**/*.rs"],
}}
_rust_compile(ctx, "rlib", "src/lib.rs", "libapp.rlib")
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "crates/app/app");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    let script = store
        .actions
        .iter()
        .find(|action| action.identifier.as_deref() == Some("crates/app/app:build-script"))
        .and_then(|action| action.argv.get(2))
        .unwrap();
    assert_eq!(script.matches("done <").count(), 1, "{script}");
}

#[cfg(unix)]
#[test]
fn prelude_rust_build_script_env_encodes_rustflags() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
rustc, _identity, host_triple = _rustc_toolchain("")
ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{
        "rustc_flags": ["-C", "opt-level=3"],
    }},
    "deps": [],
    "srcs": [],
}}
env = _rust_build_script_env(
    ctx,
    rustc,
    host_triple,
    host_triple,
    ".once/out/app/build",
    "crates/app/build.rs",
)
result = repr(env.get("CARGO_ENCODED_RUSTFLAGS"))
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "crates/app/app");

    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"-C\\x1fopt-level=3\"");
}

#[test]
fn prelude_rustc_wrapper_passes_initial_argv_positionally() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "macos"

def host_which(name):
    if name == "sh":
        return "/usr/bin/sh"
    fail("unexpected host_which call: " + name)

wrapped = _rustc_with_build_script_args(
    {{"attr": {{}}}},
    ["rustc", "arg with spaces", "O'Reilly"],
    ".once/out/pkg/build script.stdout",
)
result = repr([wrapped[0], wrapped[1]])
"#
    );

    let out = eval_prelude_source_to_repr(source).unwrap();
    let values: Vec<Vec<String>> = serde_json::from_str(&out).unwrap();
    let argv = &values[0];

    assert_eq!(argv[0], "/usr/bin/sh");
    assert_eq!(argv[1], "-c");
    assert_eq!(argv[3], "once-rustc");
    assert_eq!(&argv[4..], ["rustc", "arg with spaces", "O'Reilly"]);
    assert!(values[1].is_empty());
    let script = &argv[2];
    assert_eq!(script.lines().nth(1), Some("while IFS= read -r line; do"));
    assert!(script.contains("done < '.once/out/pkg/build script.stdout'"));
    assert!(script.contains("exec \"$@\""));
    assert!(!script.contains("O'Reilly"), "{script}");
}

#[test]
fn prelude_windows_rustc_wrapper_generates_powershell_trampoline() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "windows"

def host_which(name):
    if name == "powershell":
        return "C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
    fail("unexpected host_which call: " + name)

ctx = {{
    "label": {{
        "id": "crates/app/app",
    }},
    "attr": {{}},
}}
wrapped = _rustc_with_build_script_args(
    ctx,
    ["rustc", "@.once/out/pkg/rustc-features.rsp", "arg with spaces"],
    ".once/out/pkg/build script.stdout",
)
result = repr([wrapped[0], wrapped[1]])
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "crates/app/app");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let values: Vec<Vec<String>> = serde_json::from_str(&out.unwrap()).unwrap();
    let argv = &values[0];
    let inputs = &values[1];

    assert_eq!(
        argv[0],
        "C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
    );
    assert_eq!(
        &argv[1..6],
        [
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File"
        ]
    );
    assert_eq!(
        argv[6],
        ".once/out/crates/app/app/rustc-build-script-wrapper.ps1"
    );
    assert_eq!(
        &argv[7..],
        [
            "rustc",
            "@.once/out/pkg/rustc-features.rsp",
            "arg with spaces"
        ]
    );
    assert_eq!(
        inputs,
        &[".once/out/crates/app/app/rustc-build-script-wrapper.ps1".to_string()]
    );
    assert_eq!(store.actions.len(), 1);
    let Some(DeclaredActionOperation::WriteFile { path, bytes }) = &store.actions[0].operation
    else {
        panic!("wrapper should be written before rustc action");
    };
    assert_eq!(
        path,
        ".once/out/crates/app/app/rustc-build-script-wrapper.ps1"
    );
    let script = String::from_utf8(bytes.clone()).unwrap();
    assert!(script.contains("[System.IO.File]::ReadLines('.once/out/pkg/build script.stdout')"));
    assert!(script.contains("[void]$dynamicRustcArgs.Add('--cfg')"));
    assert!(script.contains("[void]$dynamicRustcArgs.Add('--check-cfg')"));
    assert!(script.contains("New-Object System.Text.UTF8Encoding -ArgumentList $false"));
    assert!(script.contains(
        "[System.IO.File]::WriteAllLines($responseFile, $dynamicRustcArgs.ToArray(), $encoding)"
    ));
    assert!(script.contains("[void]$rustcArgs.Add(\"@$responseFile\")"));
    assert!(script.contains("& $program @rest"));
}

#[test]
fn prelude_rust_windows_feature_cfgs_use_response_file() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "windows"

def host_env(name):
    return ""

def host_which(name):
    fail("unexpected host_which call: " + name)

def host_command(argv, env = None):
    fail("unexpected host_command call")

def _rustc_toolchain(target):
    return ("C:/Rust/bin/rustc.exe", "rustc-test", "x86_64-pc-windows-msvc")

features = ["default", "std"] + ["feature_" + str(i) for i in range(400)]
ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{
        "target": "x86_64-pc-windows-msvc",
        "crate_root": "src/lib.rs",
        "features": features,
    }},
    "deps": [],
    "srcs": ["src/**/*.rs"],
}}
_rust_compile(ctx, "rlib", "src/lib.rs", "libapp.rlib")
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "crates/app/app");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    let rustc = store
        .actions
        .iter()
        .find(|action| action.identifier.as_deref() == Some("crates/app/app:rustc"))
        .expect("app rustc action");
    assert_eq!(rustc.identifier.as_deref(), Some("crates/app/app:rustc"));
    assert!(rustc
        .argv
        .iter()
        .any(|arg| arg == "@.once/tmp/analysis/crates/app/app/rustc.rsp"));
    assert!(!rustc
        .inputs
        .iter()
        .any(|input| input == ".once/tmp/analysis/crates/app/app/rustc.rsp"));
    // Only the toolchain and the response-file reference remain on the command
    // line; everything else is written to the response file.
    assert_eq!(rustc.argv.len(), 2);
    assert_eq!(rustc.arg_files.len(), 1);
    let arg_file = &rustc.arg_files[0];
    assert_eq!(arg_file.path, ".once/tmp/analysis/crates/app/app/rustc.rsp");
    assert_eq!(arg_file.format, DeclaredArgFileFormat::RustcResponse);
    assert!(arg_file.args.len() > 400);
    // The full rustc invocation, not just feature cfgs, is routed through the
    // response file so the command line cannot exceed the Windows limit.
    assert!(arg_file.args.iter().any(|arg| arg == "--crate-name"));
    assert!(arg_file
        .args
        .iter()
        .any(|arg| arg == "--cfg=feature=\"default\""));
    assert!(arg_file
        .args
        .iter()
        .any(|arg| arg == "--cfg=feature=\"std\""));
    assert!(arg_file
        .args
        .iter()
        .any(|arg| arg == "--cfg=feature=\"feature_399\""));
    assert!(!arg_file
        .args
        .iter()
        .any(|arg| arg == "--cfg=feature=default"));
    assert!(!arg_file
        .args
        .iter()
        .any(|arg| arg == "--cfg=feature=\\\"default\\\""));
    assert!(!arg_file
        .args
        .iter()
        .any(|arg| arg == "--cfg=feature=r#default#"));
    assert!(
        !rustc.argv.iter().any(|arg| arg.contains("feature=\"")),
        "{:?}",
        rustc.argv
    );
}

#[test]
fn prelude_rust_non_windows_feature_cfgs_stay_inline() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "linux"

def host_env(name):
    return ""

def host_which(name):
    fail("unexpected host_which call: " + name)

def host_command(argv, env = None):
    fail("unexpected host_command call")

def _rustc_toolchain(target):
    return ("rustc", "rustc-test", "x86_64-unknown-linux-gnu")

ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{
        "target": "wasm32-unknown-unknown",
        "crate_root": "src/lib.rs",
        "features": ["default", "std"],
    }},
    "deps": [],
    "srcs": ["src/**/*.rs"],
}}
_rust_compile(ctx, "rlib", "src/lib.rs", "libapp.rlib")
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "crates/app/app");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    let rustc = store
        .actions
        .iter()
        .find(|action| action.identifier.as_deref() == Some("crates/app/app:rustc"))
        .expect("app rustc action");
    assert_eq!(rustc.identifier.as_deref(), Some("crates/app/app:rustc"));
    assert!(rustc
        .argv
        .iter()
        .any(|arg| arg == "--cfg=feature=\"default\""));
    assert!(rustc.argv.iter().any(|arg| arg == "--cfg=feature=\"std\""));
    assert!(
        !rustc.argv.iter().any(|arg| arg.starts_with('@')),
        "{:?}",
        rustc.argv
    );
    assert!(rustc
        .operation
        .as_ref()
        .is_none_or(|operation| !matches!(operation, DeclaredActionOperation::WriteFile { .. })));
}

#[test]
fn prelude_rust_windows_routes_invocation_through_response_file_without_features() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "windows"

def host_env(name):
    return ""

def host_which(name):
    fail("unexpected host_which call: " + name)

def host_command(argv, env = None):
    fail("unexpected host_command call")

def _rustc_toolchain(target):
    return ("C:/Rust/bin/rustc.exe", "rustc-test", "x86_64-pc-windows-msvc")

ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{
        "target": "x86_64-pc-windows-msvc",
        "crate_root": "src/lib.rs",
    }},
    "deps": [{{
        "label_id": "crates/dep/dep",
        "crate_name": "dep",
        "rlib": ".once/out/crates/dep/dep/libdep.rlib",
    }}],
    "srcs": ["src/**/*.rs"],
}}
_rust_compile(ctx, "rlib", "src/lib.rs", "libapp.rlib")
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "crates/app/app");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    let rustc = store
        .actions
        .iter()
        .find(|action| action.identifier.as_deref() == Some("crates/app/app:rustc"))
        .expect("app rustc action");
    assert_eq!(rustc.identifier.as_deref(), Some("crates/app/app:rustc"));
    // On Windows the invocation is always routed through a response file, even
    // when the crate has no features, because the command line still carries
    // the crate metadata, source, and dependency flags.
    assert!(
        rustc
            .argv
            .iter()
            .any(|arg| arg == "@.once/tmp/analysis/crates/app/app/rustc.rsp"),
        "{:?}",
        rustc.argv
    );
    assert_eq!(rustc.arg_files.len(), 1);
    let arg_file = &rustc.arg_files[0];
    assert_eq!(arg_file.path, ".once/tmp/analysis/crates/app/app/rustc.rsp");
    assert!(arg_file.args.iter().any(|arg| arg == "--crate-name"));
    let extern_arg = format!(
        "dep={}",
        workspace_arg(workspace.path(), ".once/out/crates/dep/dep/libdep.rlib")
    );
    let extern_position = arg_file
        .args
        .windows(2)
        .position(|args| args[0] == "--extern" && args[1] == extern_arg)
        .expect("dependency extern flag");
    let crate_root = workspace_arg(workspace.path(), "crates/app/src/lib.rs");
    let root_position = arg_file
        .args
        .iter()
        .position(|arg| arg == &crate_root)
        .expect("crate root");
    assert!(
        extern_position < root_position,
        "dependency flags should precede the crate root: {:?}",
        arg_file.args
    );
    assert_eq!(
        arg_file.args.last().map(String::as_str),
        Some(crate_root.as_str())
    );
    assert!(
        !arg_file
            .args
            .iter()
            .any(|arg| arg.starts_with("--cfg=feature=")),
        "{:?}",
        arg_file.args
    );
}

const RELEASE_DEPENDENCY_RESPONSE_FILE_SOURCE: &str = r#"
def host_os():
    return "windows"

def host_env(name):
    return ""

def host_which(name):
    fail("unexpected host_which call: " + name)

def host_command(argv, env = None):
    fail("unexpected host_command call")

def _rustc_toolchain(target):
    return ("C:/Rust/bin/rustc.exe", "rustc-test", "x86_64-pc-windows-msvc")

ctx = {
    "label": {
        "package": "crates/once-core",
        "name": "once_core_x86_64_pc_windows_msvc",
        "id": "crates/once-core/once_core_x86_64_pc_windows_msvc",
    },
    "attr": {
        "crate_name": "once_core",
        "crate_root": "src/lib.rs",
        "target": "x86_64-pc-windows-msvc",
        "cargo_package": "once-core",
    },
    "deps": [
        {
            "label_id": "crates/once-cas/once_cas_x86_64_pc_windows_msvc",
            "crate_name": "once_cas",
            "rlib": ".once/out/crates/once-cas/once_cas_x86_64_pc_windows_msvc/libonce_cas-CRATES_ONCE_CAS_ONCE_CAS_X86_64_PC_WINDOWS_MSVC.rlib",
            "transitive_rlibs": [
                ".once/out/cargo_dependencies_x86_64_pc_windows_msvc/serde-1.0.228/libserde-CARGO_DEPENDENCIES_X86_64_PC_WINDOWS_MSVC_SERDE_1_0_228.rlib",
            ],
        },
        {
            "dependency_set": True,
            "deps": [],
            "workspace_deps": {
                "once-core": [
                    {
                        "label_id": "cargo_dependencies_x86_64_pc_windows_msvc/tokio-1.52.3",
                        "crate_name": "tokio",
                        "rlib": ".once/out/cargo_dependencies_x86_64_pc_windows_msvc/tokio-1.52.3/libtokio-CARGO_DEPENDENCIES_X86_64_PC_WINDOWS_MSVC_TOKIO_1_52_3.rlib",
                    },
                    {
                        "label_id": "cargo_dependencies_x86_64_pc_windows_msvc/serde-1.0.228",
                        "crate_name": "serde",
                        "rlib": ".once/out/cargo_dependencies_x86_64_pc_windows_msvc/serde-1.0.228/libserde-CARGO_DEPENDENCIES_X86_64_PC_WINDOWS_MSVC_SERDE_1_0_228.rlib",
                    },
                    {
                        "label_id": "cargo_dependencies_x86_64_pc_windows_msvc/tracing-0.1.43",
                        "crate_name": "tracing",
                        "rlib": ".once/out/cargo_dependencies_x86_64_pc_windows_msvc/tracing-0.1.43/libtracing-CARGO_DEPENDENCIES_X86_64_PC_WINDOWS_MSVC_TRACING_0_1_43.rlib",
                    },
                ],
            },
        },
    ],
    "srcs": ["src/**/*.rs"],
}
_rust_compile(ctx, "rlib", "src/lib.rs", "libonce_core.rlib")
result = repr("ok")
"#;

#[test]
fn prelude_rust_windows_response_file_keeps_release_dependency_args() {
    let source = format!(
        "{}\n{}",
        all_prelude_source(),
        RELEASE_DEPENDENCY_RESPONSE_FILE_SOURCE
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(
        workspace.path(),
        "crates/once-core/once_core_x86_64_pc_windows_msvc",
    );

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    assert_release_dependency_response_file(&store, workspace.path());
}

#[test]
fn prelude_rust_windows_response_file_paths_use_forward_slashes() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "D:\\a\\once\\once"

result = repr([
    _rust_response_path_arg(".once/out/libfoo.rlib"),
    _workspace_extern_arg("foo=.once/out/libfoo.rlib"),
    _workspace_search_path_arg("dependency=.once/out/foo"),
    _rust_response_path_arg("D:\\a\\once\\once\\crates\\foo\\src\\lib.rs"),
    _rust_response_path_arg("--cfg=feature=\"default\""),
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(
        out,
        "[\"D:/a/once/once/.once/out/libfoo.rlib\", \"foo=D:/a/once/once/.once/out/libfoo.rlib\", \"dependency=D:/a/once/once/.once/out/foo\", \"D:/a/once/once/crates/foo/src/lib.rs\", \"--cfg=feature=\\\"default\\\"\"]"
    );
}

fn assert_release_dependency_response_file(store: &AnalysisStore, workspace: &Path) {
    let rustc = store
        .actions
        .iter()
        .find(|action| {
            action.identifier.as_deref()
                == Some("crates/once-core/once_core_x86_64_pc_windows_msvc:rustc")
        })
        .expect("once-core rustc action");
    assert_eq!(
        rustc.identifier.as_deref(),
        Some("crates/once-core/once_core_x86_64_pc_windows_msvc:rustc")
    );
    assert_eq!(rustc.argv.len(), 2);
    assert_eq!(rustc.arg_files.len(), 1);
    let arg_file = &rustc.arg_files[0];
    assert_eq!(
        arg_file.path,
        ".once/tmp/analysis/crates/once-core/once_core_x86_64_pc_windows_msvc/rustc.rsp"
    );
    for extern_arg in [
        format!(
            "once_cas={}",
            workspace_arg(
                workspace,
                ".once/out/crates/once-cas/once_cas_x86_64_pc_windows_msvc/libonce_cas-CRATES_ONCE_CAS_ONCE_CAS_X86_64_PC_WINDOWS_MSVC.rlib"
            )
        ),
        format!(
            "tokio={}",
            workspace_arg(
                workspace,
                ".once/out/cargo_dependencies_x86_64_pc_windows_msvc/tokio-1.52.3/libtokio-CARGO_DEPENDENCIES_X86_64_PC_WINDOWS_MSVC_TOKIO_1_52_3.rlib"
            )
        ),
        format!(
            "serde={}",
            workspace_arg(
                workspace,
                ".once/out/cargo_dependencies_x86_64_pc_windows_msvc/serde-1.0.228/libserde-CARGO_DEPENDENCIES_X86_64_PC_WINDOWS_MSVC_SERDE_1_0_228.rlib"
            )
        ),
        format!(
            "tracing={}",
            workspace_arg(
                workspace,
                ".once/out/cargo_dependencies_x86_64_pc_windows_msvc/tracing-0.1.43/libtracing-CARGO_DEPENDENCIES_X86_64_PC_WINDOWS_MSVC_TRACING_0_1_43.rlib"
            )
        ),
    ] {
        assert!(
            arg_file
                .args
                .windows(2)
                .any(|args| args[0] == "--extern" && args[1] == extern_arg.as_str()),
            "{extern_arg} missing from {:?}",
            arg_file.args
        );
    }
    let crate_root = workspace_arg(workspace, "crates/once-core/src/lib.rs");
    let root_position = arg_file
        .args
        .iter()
        .position(|arg| arg == &crate_root)
        .expect("crate root");
    for extern_position in arg_file
        .args
        .iter()
        .enumerate()
        .filter_map(|(index, arg)| (arg == "--extern").then_some(index))
    {
        assert!(
            extern_position + 1 < root_position,
            "dependency flags should precede the crate root: {:?}",
            arg_file.args
        );
    }
    assert_release_dependency_search_path(&arg_file.args, workspace);
    assert_eq!(
        arg_file.args.last().map(String::as_str),
        Some(crate_root.as_str())
    );
}

fn assert_release_dependency_search_path(args: &[String], workspace: &Path) {
    let staged_dependency = format!(
        "dependency={}",
        workspace_arg(
            workspace,
            ".once/out/crates/once-core/once_core_x86_64_pc_windows_msvc/deps-rlib-search"
        )
    );
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "-L" && pair[1] == staged_dependency),
        "{staged_dependency} missing from {args:?}"
    );
    assert!(
        !args.iter().any(|arg| arg.contains("/search/deps")),
        "rlib-only deps should not create a proc-macro staging directory: {args:?}"
    );
}

#[cfg(unix)]
#[test]
fn prelude_rust_proc_macro_compile_uses_host_target() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
_rustc, _identity, host_triple = _rustc_toolchain("")
def other_target(host_triple):
    if host_triple == "aarch64-unknown-linux-gnu":
        return "x86_64-unknown-linux-gnu"
    return "aarch64-unknown-linux-gnu"
ctx = {{
    "label": {{
        "package": "macros/stringify",
        "name": "stringify",
        "id": "macros/stringify",
    }},
    "attr": {{
        "target": other_target(host_triple),
        "crate_root": "src/lib.rs",
    }},
    "deps": [],
    "srcs": ["src/**/*.rs"],
}}
_rust_compile(ctx, "proc-macro", "src/lib.rs", "libstringify.so")
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "macros/stringify");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    let action = &store.actions[0];
    assert!(
        !action.argv.iter().any(|arg| arg == "--target"),
        "{:?}",
        action.argv
    );
    assert!(
        action
            .argv
            .windows(2)
            .any(|args| args[0] == "-C" && args[1] == "prefer-dynamic"),
        "{:?}",
        action.argv
    );
    assert!(
        action
            .argv
            .windows(2)
            .any(|args| args[0] == "--out-dir" && args[1] == ".once/out/macros/stringify"),
        "{:?}",
        action.argv
    );
    assert!(
        action
            .argv
            .windows(2)
            .any(|args| args[0] == "-C" && args[1] == "extra-filename=-MACROS_STRINGIFY"),
        "{:?}",
        action.argv
    );
    let dylib_ext = if cfg!(target_os = "macos") {
        ".dylib"
    } else {
        ".so"
    };
    assert_eq!(
        action.outputs,
        vec![format!(
            ".once/out/macros/stringify/libstringify-MACROS_STRINGIFY{dylib_ext}"
        )]
    );
}

#[cfg(unix)]
#[test]
fn prelude_rust_build_script_env_uses_absolute_c_tool_paths() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
rustc, _identity, host_triple = _rustc_toolchain("")
ctx = {{
    "label": {{
        "package": "third_party/rust/vendor/pkg-1.0.0",
        "name": "pkg",
        "id": "third_party/rust/vendor/pkg-1.0.0",
    }},
    "attr": {{}},
    "srcs": [],
}}
tool_env = _rust_c_tool_env(host_triple, host_triple)
build_env = _rust_build_script_env(
    ctx,
    rustc,
    host_triple,
    host_triple,
    ".once/out/pkg/build",
    "third_party/rust/vendor/pkg-1.0.0/build.rs",
)
result = repr([
    tool_env.get("CC") or "",
    tool_env.get("AR") or "",
    tool_env.get("RANLIB") or "",
    tool_env.get("PKG_CONFIG") or "",
    tool_env.get("PATH") or "",
    build_env.get("CC") or "",
    build_env.get("AR") or "",
    build_env.get("RANLIB") or "",
    build_env.get("PKG_CONFIG") or "",
    build_env.get("PATH") or "",
])
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "");

    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let values: Vec<String> = serde_json::from_str(&out.unwrap()).unwrap();

    assert!(std::path::Path::new(&values[0]).is_absolute());
    assert!(std::path::Path::new(&values[1]).is_absolute());
    if !values[2].is_empty() {
        assert!(std::path::Path::new(&values[2]).is_absolute());
    }
    if !values[3].is_empty() {
        assert!(std::path::Path::new(&values[3]).is_absolute());
    }
    assert_eq!(values[0], values[5]);
    assert_eq!(values[1], values[6]);
    assert_eq!(values[2], values[7]);
    assert_eq!(values[3], values[8]);
    assert_eq!(values[4], values[9]);
    for entry in values[4].split(':') {
        assert!(std::path::Path::new(entry).is_absolute());
    }
    for tool in [&values[0], &values[1], &values[2], &values[3]] {
        if tool.is_empty() {
            continue;
        }
        let tool_dir = std::path::Path::new(tool)
            .parent()
            .unwrap()
            .to_string_lossy();
        assert!(values[4].split(':').any(|entry| entry == tool_dir));
    }
    assert!(values[4].split(':').any(|entry| entry == "/bin"));
}

#[cfg(unix)]
#[test]
fn prelude_rust_build_script_compile_action_gets_sanitized_c_tool_path() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{
        "build_script": "build.rs",
        "crate_root": "src/lib.rs",
    }},
    "deps": [],
    "srcs": ["src/**/*.rs"],
}}
_rust_compile(ctx, "rlib", "src/lib.rs", "libapp.rlib")
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "crates/app/app");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    let action = store
        .actions
        .iter()
        .find(|action| action.identifier.as_deref() == Some("crates/app/app:build-script-rustc"))
        .expect("build script rustc action");
    let path = action.env.get("PATH").expect("host linker PATH");
    assert!(path.split(':').any(|entry| entry == "/bin"), "{path}");
    for entry in path.split(':') {
        assert!(std::path::Path::new(entry).is_absolute(), "{path}");
    }
}

#[cfg(unix)]
#[test]
fn prelude_rust_host_compile_actions_get_sanitized_c_tool_path() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
rustc, _identity, host_triple = _rustc_toolchain("")
ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{
        "target": host_triple,
        "crate_root": "src/main.rs",
    }},
    "deps": [],
    "srcs": ["src/**/*.rs"],
}}
_rust_compile(ctx, "bin", "src/main.rs", "app")
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "crates/app/app");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    let path = store.actions[0].env.get("PATH").expect("host linker PATH");
    assert!(path.split(':').any(|entry| entry == "/bin"), "{path}");
    for entry in path.split(':') {
        assert!(std::path::Path::new(entry).is_absolute(), "{path}");
    }
}

#[cfg(unix)]
#[test]
fn prelude_rust_compile_action_env_merges_c_tool_env_with_existing_path() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
rustc, _identity, host_triple = _rustc_toolchain("")
ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{
        "target": host_triple,
        "crate_root": "src/lib.rs",
        "env": {{
            "PATH": "/custom/bin",
            "CC": "/custom/cc",
        }},
    }},
    "deps": [],
    "srcs": ["src/**/*.rs"],
}}
_rust_compile(ctx, "rlib", "src/lib.rs", "libapp.rlib")
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "crates/app/app");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    let action = &store.actions[0];
    let path = action.env.get("PATH").expect("merged linker PATH");
    let entries = path.split(':').collect::<Vec<_>>();
    assert_eq!(entries[0], "/custom/bin");
    assert!(entries.contains(&"/bin"), "{path}");
    assert_eq!(action.env.get("CC").map(String::as_str), Some("/custom/cc"));
    assert!(action
        .env
        .get("AR")
        .is_some_and(|ar| std::path::Path::new(ar).is_absolute()));
}

#[test]
fn prelude_rust_compile_action_env_uses_target_for_c_tool_env() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "linux"

def host_env(name):
    return ""

def host_command(argv, env = None):
    fail("unexpected host_command call")

def host_which(name):
    fail("unexpected host_which call: " + name)

def _rustc_toolchain(target):
    return ("rustc", "rustc-test", "x86_64-unknown-linux-gnu")

def _rust_c_tool_env(target, host_triple):
    if target != "thumbv7em-none-eabihf":
        fail("unexpected c tool target: " + target)
    if host_triple != "x86_64-unknown-linux-gnu":
        fail("unexpected host triple: " + host_triple)
    return {{
        "CC": "/opt/thumb/bin/thumbv7em-none-eabihf-cc",
        "AR": "/opt/thumb/bin/thumbv7em-none-eabihf-ar",
        "PATH": "/opt/thumb/bin:/opt/thumb/libexec",
    }}

ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{
        "target": "thumbv7em-none-eabihf",
        "crate_root": "src/lib.rs",
    }},
    "deps": [],
    "srcs": ["src/**/*.rs"],
}}
_rust_compile(ctx, "rlib", "src/lib.rs", "libapp.rlib")
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "crates/app/app");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    let action = &store.actions[0];
    assert_eq!(
        action.env.get("CC").map(String::as_str),
        Some("/opt/thumb/bin/thumbv7em-none-eabihf-cc")
    );
    assert_eq!(
        action.env.get("AR").map(String::as_str),
        Some("/opt/thumb/bin/thumbv7em-none-eabihf-ar")
    );
    let path = action.env.get("PATH").expect("target c tool PATH");
    assert!(
        path.split(':').any(|entry| entry == "/opt/thumb/bin"),
        "{path}"
    );
    assert!(
        path.split(':').any(|entry| entry == "/opt/thumb/libexec"),
        "{path}"
    );
}

#[cfg(unix)]
#[test]
fn prelude_rust_compile_env_does_not_forward_unix_host_path() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{}},
    "srcs": [],
}}
env = _rust_compile_env(ctx)
result = repr([
    env.get("PATH"),
    env.get("LIB"),
    env.get("INCLUDE"),
])
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "");

    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "[None, None, None]");
}

#[test]
fn prelude_rust_compile_env_forwards_windows_tool_env_without_overrides() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "windows"

_host_values = {{
    "PATH": "C:/msvc/bin;C:/windows/system32",
    "Path": "C:/ignored",
    "INCLUDE": "C:/include",
    "LIB": "C:/lib",
    "SystemRoot": "C:/Windows",
    "TEMP": "C:/Temp",
    "VCINSTALLDIR": "C:/VS/VC",
}}

def host_env(name):
    return _host_values.get(name, "")

def host_which(name):
    fail("unexpected host_which call: " + name)

def host_command(argv, env = None):
    fail("unexpected host_command call")

ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{
        "env": {{
            "CUSTOM": "configured",
            "LIB": "configured-lib",
        }},
        "rustc_env": {{
            "INCLUDE": "configured-include",
        }},
    }},
    "srcs": [],
}}
env = _rust_compile_env(ctx)
result = repr([
    env.get("PATH"),
    env.get("INCLUDE"),
    env.get("LIB"),
    env.get("SystemRoot"),
    env.get("TEMP"),
    env.get("VCINSTALLDIR"),
    env.get("CUSTOM"),
    env.get("PATHEXT"),
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(
        out,
        "[\"C:/msvc/bin;C:/windows/system32\", \"configured-include\", \"configured-lib\", \"C:/Windows\", \"C:/Temp\", \"C:/VS/VC\", \"configured\", None]"
    );
}

#[cfg(unix)]
#[test]
fn prelude_rust_build_script_env_does_not_use_host_c_tool_for_cross_target() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
_rustc, _identity, host_triple = _rustc_toolchain("")
def other_target(host_triple):
    if host_triple == "aarch64-unknown-linux-gnu":
        return "x86_64-unknown-linux-gnu"
    return "aarch64-unknown-linux-gnu"
target = other_target(host_triple)
env = _rust_c_tool_env(target, host_triple)
result = repr([
    env.get("CC"),
    env.get("AR"),
    env.get("PATH"),
    env.get("CC_" + target.replace("-", "_")),
    env.get("CC_" + target),
])
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "");

    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "[None, None, None, None, None]");
}

#[test]
fn prelude_ios_simulator_selection_filters_to_iphone_and_ipad() {
    let out =
        eval_prelude_string_function("_ios_simulator_selection_script", r#"("/usr/bin/xcrun")"#)
            .unwrap();

    assert!(out.contains("ONCE_APPLE_SIMULATOR_UDID"), "{out}");
    assert!(out.contains("simctl list devices booted"), "{out}");
    assert!(out.contains("simctl list devices available"), "{out}");
    assert!(out.contains("/iPhone/ s/^.*"), "{out}");
    assert!(out.contains("/iPad/ s/^.*"), "{out}");
    assert!(out.contains("(Booted)[[:space:]]*$"), "{out}");
    assert!(out.contains("(Shutdown)[[:space:]]*$"), "{out}");
    assert!(!out.contains("sed -n 's/.*"), "{out}");
}

#[cfg(unix)]
#[test]
fn prelude_ios_simulator_selection_script_picks_booted_ios_device() {
    let tmp = TempDir::new().unwrap();
    let xcrun = tmp.path().join("xcrun");
    write_executable(
        &xcrun,
        r#"#!/bin/sh
if [ "${1:-}" = "simctl" ] && [ "${2:-}" = "list" ] && [ "${3:-}" = "devices" ] && [ "${4:-}" = "booted" ]; then
  printf '%s\n' '    Apple TV (AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA) (Booted)'
  printf '%s\n' '    iPhone Preview (BBBBBBBB-BBBB-BBBB-BBBB-BBBBBBBBBBBB) (Extra) (Booted)'
  printf '%s\n' '    iPhone 15 Pro (11111111-1111-1111-1111-111111111111) (Booted)'
  exit 0
fi
if [ "${1:-}" = "simctl" ] && [ "${2:-}" = "list" ] && [ "${3:-}" = "devices" ] && [ "${4:-}" = "available" ]; then
  printf '%s\n' '    iPad Pro (22222222-2222-2222-2222-222222222222) (Shutdown)'
  exit 0
fi
exit 1
"#,
    );
    let call = format!(
        "({})",
        starlark_string_literal(&xcrun.display().to_string())
    );
    let selection_script =
        eval_prelude_string_function("_ios_simulator_selection_script", &call).unwrap();
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("{selection_script}\nprintf '%s' \"$simulator_id\""))
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "11111111-1111-1111-1111-111111111111"
    );
}

#[cfg(unix)]
#[test]
fn prelude_ios_simulator_selection_script_errors_without_ios_device() {
    let tmp = TempDir::new().unwrap();
    let xcrun = tmp.path().join("xcrun");
    write_executable(
        &xcrun,
        r#"#!/bin/sh
if [ "${1:-}" = "simctl" ] && [ "${2:-}" = "list" ] && [ "${3:-}" = "devices" ]; then
  printf '%s\n' '    Apple TV (AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA) (Booted)'
  exit 0
fi
exit 1
"#,
    );
    let call = format!(
        "({})",
        starlark_string_literal(&xcrun.display().to_string())
    );
    let selection_script =
        eval_prelude_string_function("_ios_simulator_selection_script", &call).unwrap();
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("{selection_script}\nprintf '%s' \"$simulator_id\""))
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("no booted or available iOS simulator found"));
}

#[test]
fn prelude_swift_testing_macros_plugin_uses_swift_toolchain_path() {
    let swiftc = "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc";
    let call = format!("({})", starlark_string_literal(swiftc));

    let out = eval_prelude_string_function("_swift_testing_macros_plugin", &call).unwrap();

    assert_eq!(
        out,
        "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/host/plugins/testing/libTestingMacros.dylib"
    );
}

#[test]
fn prelude_swift_testing_macros_plugin_rejects_unexpected_swiftc_path() {
    let call = format!("({})", starlark_string_literal("/tmp/swiftc"));

    let err = eval_prelude_string_function("_swift_testing_macros_plugin", &call).unwrap_err();

    assert!(
        err.contains("unable to derive Swift toolchain path"),
        "{err}"
    );
}

#[test]
fn prelude_ios_simulator_selection_helper_feeds_run_and_test_scripts() {
    let source = include_str!("../prelude/apple.star");

    // The helper is defined once and called from exactly two sites:
    // the application run script (with `xcrun`) and the test runner
    // (with `runner_xcrun`). Match each call site by its bound
    // argument so the assertion doesn't break if the helper is
    // mentioned in a comment or docstring and so the definition
    // doesn't need to be subtracted out.
    assert_eq!(
        source
            .matches("def _ios_simulator_selection_script(")
            .count(),
        1,
        "expected exactly one definition of _ios_simulator_selection_script"
    );
    // Match the helper concatenated with the surrounding `+ """` to
    // exclude the `def` line and to anchor each call site to its
    // actual usage (script-building expression). The two call sites
    // pass `xcrun` and `runner_xcrun` respectively.
    assert_eq!(
        source
            .matches("_ios_simulator_selection_script(xcrun) + \"\"\"")
            .count(),
        1,
        "expected the application run script to call _ios_simulator_selection_script(xcrun)"
    );
    assert_eq!(
        source
            .matches("_ios_simulator_selection_script(runner_xcrun) + \"\"\"")
            .count(),
        1,
        "expected the test runner to call _ios_simulator_selection_script(runner_xcrun)"
    );
}

/// The prelude `_serialize_hmap` helper must lay out the
/// header-map byte sequence correctly: 4-byte magic, version 1,
/// reserved 0, the rest of the header, a power-of-two bucket
/// array, and a string table that starts with a 0 byte. We assert
/// each invariant from a Starlark-driven run so the format
/// implementation stays a Starlark concern.
#[test]
fn prelude_serialize_hmap_lays_out_canonical_header_and_entries() {
    let prelude = apple_prelude_source();
    let source = format!(
        "{prelude}\n\
             entries = {{\"Foo.h\": \"AppCore/Foo.h\", \"Bar.h\": \"AppCore/Bar.h\"}}\n\
             bytes = _serialize_hmap(entries)\n"
    );
    let mut bytes: Option<Vec<u8>> = None;
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse("test.star", source, &Dialect::Standard)?;
        let globals = globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)?;
        let value = module.get("bytes").expect("bytes binding");
        let list = ListRef::from_value(value).expect("bytes is a list");
        let collected: Vec<u8> = list
            .iter()
            .map(|item| u8::try_from(item.unpack_i32().expect("int byte")).expect("0..=255"))
            .collect();
        bytes = Some(collected);
        starlark::Result::Ok(())
    })
    .expect("prelude eval");
    let bytes = bytes.unwrap();

    // magic + version + reserved
    assert_eq!(&bytes[0..4], &0x6861_6D70_u32.to_le_bytes());
    assert_eq!(&bytes[4..6], &1u16.to_le_bytes());
    assert_eq!(&bytes[6..8], &0u16.to_le_bytes());

    // num_entries == 2; num_buckets is a power of two; strings
    // offset lands right after the bucket array.
    let strings_off = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let num_entries = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let num_buckets = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    assert_eq!(num_entries, 2);
    assert!(num_buckets.is_power_of_two() && num_buckets >= 2);
    assert_eq!(strings_off, 24 + (num_buckets as usize) * 12);
    assert_eq!(bytes[strings_off], 0);
}

#[test]
fn prelude_apple_config_tokens_rejects_select_on_platform() {
    let err = eval_prelude_function(
        "_apple_config_tokens",
        r#"({"platform": {"select": {"default": "ios"}}}, "tgt")"#,
    )
    .unwrap_err();
    assert!(
        err.contains("attribute `platform` cannot use select()"),
        "{err}"
    );
}

/// Direct-mode swiftc resolution must derive both the compiler and
/// the active SDK from the configured developer dir without
/// shelling out to xcrun. The returned argv is what every Swift
/// action prepends to its flags, so it has to invoke swiftc by
/// absolute path and pass `-sdk <path>` explicitly.
#[test]
fn prelude_resolve_swiftc_direct_mode_skips_xcrun() {
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
def host_which(name):
    fail("host_which must not be called in direct mode (asked for " + name + ")")

def host_command(argv, env = None):
    if "--version" in argv:
        return "Swift version 6.0\n"
    fail("unexpected host_command: " + str(argv))

swiftc = _resolve_swiftc("ios", "simulator", "/opt/Xcode/Developer")
result = repr([
    swiftc["argv"],
    swiftc["sdk_name"],
    swiftc["sdk_path"],
    swiftc["swiftc_path"],
    swiftc["env"],
    "identity:" in ("identity:" if swiftc["identity"].startswith("once.apple.swiftc.v1") else ""),
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();
    assert!(
        out.contains("/opt/Xcode/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc"),
        "{out}"
    );
    assert!(out.contains("/opt/Xcode/Developer/Platforms/iPhoneSimulator.platform/Developer/SDKs/iPhoneSimulator.sdk"), "{out}");
    assert!(out.contains("\"iphonesimulator\""), "{out}");
    assert!(
        out.contains("\"DEVELOPER_DIR\": \"/opt/Xcode/Developer\""),
        "{out}"
    );
    assert!(out.contains("True"), "identity prefix should match: {out}");
}

/// Direct-mode clang resolution must produce both clang and
/// clang++ under `Toolchains/XcodeDefault.xctoolchain/usr/bin/`
/// without xcrun, and the SDK path must follow the standard
/// Platforms layout so the per-source action passes a correct
/// `-isysroot`.
#[test]
fn prelude_resolve_clang_direct_mode_finds_both_drivers() {
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
def host_which(name):
    fail("host_which must not be called in direct mode (asked for " + name + ")")

def host_command(argv, env = None):
    if "--version" in argv:
        return "Apple clang version test\n"
    fail("unexpected host_command: " + str(argv))

clang = _resolve_clang("ios", "device", "/opt/Xcode/Developer")
result = repr([
    clang["clang_path"],
    clang["clangxx_path"],
    clang["sdk_path"],
    clang["sdk_name"],
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();
    assert!(
        out.contains("/opt/Xcode/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang\""),
        "{out}"
    );
    assert!(
        out.contains("/opt/Xcode/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang++"),
        "{out}"
    );
    assert!(
        out.contains(
            "/opt/Xcode/Developer/Platforms/iPhoneOS.platform/Developer/SDKs/iPhoneOS.sdk"
        ),
        "{out}"
    );
    assert!(out.contains("\"iphoneos\""), "{out}");
}

/// codesign is a system tool, not part of the developer dir. Direct
/// mode resolves it through xcrun instead of the shell search path,
/// so signing actions do not pick up a replacement.
#[test]
fn prelude_resolve_codesign_direct_mode_uses_xcrun_find() {
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "xcrun":
        return "/usr/bin/xcrun"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None):
    if argv == ["/usr/bin/xcrun", "--find", "codesign"] and env == {{"DEVELOPER_DIR": "/opt/Xcode/Developer"}}:
        return "/usr/bin/codesign\n"
    fail("unexpected host_command: " + str(argv) + " env=" + str(env))

codesign = _resolve_codesign("/opt/Xcode/Developer")
result = repr([codesign["codesign_path"], codesign["env"]])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();
    assert!(out.contains("/usr/bin/codesign"), "{out}");
    assert!(
        out.contains("\"DEVELOPER_DIR\": \"/opt/Xcode/Developer\""),
        "{out}"
    );
}

/// The xcrun fallback path is what every macOS user hits today
/// (no `xcode_developer_dir` configured). The resolver should
/// still produce a direct tool invocation — the action argv must
/// not contain xcrun even when discovery went through it. This
/// keeps cache keys identical whether or not the user pins a
/// developer dir.
#[test]
fn prelude_resolve_swiftc_fallback_returns_direct_invocation() {
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "xcrun":
        return "/usr/bin/xcrun"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None):
    if "--find" in argv and argv[len(argv) - 1] == "swiftc":
        return "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc\n"
    if "--show-sdk-path" in argv:
        return "/Applications/Xcode.app/Contents/Developer/Platforms/iPhoneSimulator.platform/Developer/SDKs/iPhoneSimulator.sdk\n"
    if "--version" in argv:
        return "Swift version 6.0\n"
    fail("unexpected host_command: " + str(argv))

swiftc = _resolve_swiftc("ios", "simulator", "")
result = repr([
    swiftc["argv"],
    swiftc["swiftc_path"],
    swiftc["sdk_path"],
    swiftc["env"],
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();
    assert!(
        !out.contains("/usr/bin/xcrun"),
        "fallback argv must not include xcrun: {out}"
    );
    assert!(
        out.contains("XcodeDefault.xctoolchain/usr/bin/swiftc"),
        "{out}"
    );
    assert!(out.contains("iPhoneSimulator.sdk"), "{out}");
    // No developer dir was configured, so the action env stays empty.
    assert!(out.contains("{}"), "{out}");
}

/// The SDK and platform path maps that direct mode relies on must
/// have an entry for every SDK name `_apple_sdk_name` can return.
/// If a new Apple platform is added to the SDK selector but its
/// layout entries are forgotten, direct-mode builds against that
/// SDK would fail at runtime with a `fail(...)` instead of being
/// caught by this test.
#[test]
fn prelude_developer_sdk_and_platform_maps_cover_supported_sdks() {
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
def _collect_sdks():
    platforms = [
        ("macos", "device"),
        ("macosx", "device"),
        ("ios", "device"),
        ("ios", "simulator"),
        ("tvos", "device"),
        ("tvos", "simulator"),
        ("watchos", "device"),
        ("watchos", "simulator"),
        ("visionos", "device"),
        ("visionos", "simulator"),
        ("xros", "device"),
        ("xros", "simulator"),
    ]
    sdks = []
    for entry in platforms:
        platform = entry[0]
        sdk_variant = entry[1]
        sdk = _apple_sdk_name(platform, sdk_variant)
        # Both maps must cover the SDK. _developer_sdk_path /
        # _developer_platform_path fail explicitly when an entry is
        # missing, so a successful resolution proves coverage.
        _developer_sdk_path("/dev", sdk)
        _developer_platform_path("/dev", sdk)
        sdks.append(sdk)
    return sdks

result = repr(_collect_sdks())
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();
    // Spot-check that the iteration actually produced an entry per
    // platform, so a future refactor that empties the list fails
    // loudly instead of passing vacuously.
    for sdk in [
        "macosx",
        "iphoneos",
        "iphonesimulator",
        "appletvos",
        "appletvsimulator",
        "watchos",
        "watchsimulator",
        "xros",
        "xrsimulator",
    ] {
        assert!(out.contains(sdk), "expected SDK {sdk} in {out}");
    }
}

/// Direct-mode libtool resolution must come from the standard
/// `Toolchains/XcodeDefault.xctoolchain/usr/bin/` layout and the
/// returned argv must invoke libtool directly so the per-arch
/// archive action keeps cache keys aligned with the rest of the
/// build.
#[test]
fn prelude_resolve_libtool_direct_mode_uses_toolchain_layout() {
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
def host_which(name):
    fail("host_which must not be called in direct mode (asked for " + name + ")")

def host_command(argv, env = None):
    fail("host_command must not be called in direct mode")

libtool = _resolve_libtool("ios", "simulator", "/opt/Xcode/Developer")
result = repr([
    libtool["argv"],
    libtool["libtool_path"],
    libtool["env"],
    libtool["identity"].startswith("once.apple.libtool.v1"),
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();
    assert!(
        out.contains("/opt/Xcode/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/libtool"),
        "{out}"
    );
    assert!(
        out.contains("\"DEVELOPER_DIR\": \"/opt/Xcode/Developer\""),
        "{out}"
    );
    assert!(out.contains("True"), "identity prefix should match: {out}");
}

/// Libtool's xcrun fallback path (no `xcode_developer_dir`
/// configured) must still produce a direct invocation: the argv
/// stored in the action must contain libtool's absolute path, not
/// `xcrun`, so cache keys match what the direct-mode path emits.
#[test]
fn prelude_resolve_libtool_fallback_returns_direct_invocation() {
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "xcrun":
        return "/usr/bin/xcrun"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None):
    if "--find" in argv and argv[len(argv) - 1] == "libtool":
        return "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/libtool\n"
    fail("unexpected host_command: " + str(argv))

libtool = _resolve_libtool("ios", "simulator", "")
result = repr([
    libtool["argv"],
    libtool["libtool_path"],
    libtool["env"],
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();
    assert!(
        !out.contains("/usr/bin/xcrun"),
        "fallback argv must not include xcrun: {out}"
    );
    assert!(
        out.contains("XcodeDefault.xctoolchain/usr/bin/libtool"),
        "{out}"
    );
    assert!(
        out.contains("{}"),
        "no developer dir means an empty action env: {out}"
    );
}

/// Direct-mode lipo resolution mirrors libtool: it resolves the
/// universal-binary tool from the standard toolchain layout and the
/// returned argv invokes lipo by absolute path, never via xcrun.
#[test]
fn prelude_resolve_lipo_direct_mode_uses_toolchain_layout() {
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
def host_which(name):
    fail("host_which must not be called in direct mode (asked for " + name + ")")

def host_command(argv, env = None):
    fail("host_command must not be called in direct mode")

lipo = _resolve_lipo("ios", "simulator", "/opt/Xcode/Developer")
result = repr([
    lipo["argv"],
    lipo["lipo_path"],
    lipo["env"],
    lipo["identity"].startswith("once.apple.lipo.v1"),
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();
    assert!(
        out.contains("/opt/Xcode/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/lipo"),
        "{out}"
    );
    assert!(
        out.contains("\"DEVELOPER_DIR\": \"/opt/Xcode/Developer\""),
        "{out}"
    );
    assert!(out.contains("True"), "identity prefix should match: {out}");
}

/// Lipo's xcrun fallback must produce a direct invocation: the
/// action argv carries the resolved tool path so multi-arch fat
/// binary builds cache the same way regardless of whether the
/// caller pinned a developer dir.
#[test]
fn prelude_resolve_lipo_fallback_returns_direct_invocation() {
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "xcrun":
        return "/usr/bin/xcrun"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None):
    if "--find" in argv and argv[len(argv) - 1] == "lipo":
        return "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/lipo\n"
    fail("unexpected host_command: " + str(argv))

lipo = _resolve_lipo("ios", "simulator", "")
result = repr([
    lipo["argv"],
    lipo["lipo_path"],
    lipo["env"],
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();
    assert!(
        !out.contains("/usr/bin/xcrun"),
        "fallback argv must not include xcrun: {out}"
    );
    assert!(
        out.contains("XcodeDefault.xctoolchain/usr/bin/lipo"),
        "{out}"
    );
    assert!(
        out.contains("{}"),
        "no developer dir means an empty action env: {out}"
    );
}

/// End-to-end direct-mode sanity check: building an `apple_library`
/// against a configured developer dir must produce actions whose
/// argv is rooted at the toolchain path. No action should contain
/// `xcrun` as an argv element, and no `host_which` lookup should
/// fire while the impl runs.
#[test]
fn prelude_apple_library_direct_mode_emits_xcrun_free_actions() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("ios/Lib/Sources");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(package_dir.join("Lib.swift"), "public func hello() {}\n").unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    fail("host_which must not be called in direct mode (asked for " + name + ")")

def host_command(argv, env = None):
    if "--version" in argv:
        return "Swift version 6.0\n"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{
        "package": "ios/Lib",
        "name": "Lib",
        "id": "ios/Lib/Lib",
    }},
    "attr": {{
        "platform": "ios",
        "sdk_variant": "simulator",
        "xcode_developer_dir": "/opt/Xcode/Developer",
    }},
    "deps": [],
    "srcs": ["Sources/**/*.swift"],
    "build_dir": ".once/out/ios/Lib/Lib",
    "capability": "build",
}}
provider = _apple_library_impl(ctx)
result = repr(provider["archive"])
"#
    );
    let store = store_for(workspace.path(), "ios/Lib");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    out.unwrap();
    assert!(!store.actions.is_empty(), "expected swiftc actions");
    for action in &store.actions {
        for arg in &action.argv {
            assert!(
                !arg.contains("xcrun"),
                "direct-mode argv should not mention xcrun: {:?}",
                action.argv
            );
        }
        assert_eq!(
            action.argv[0],
            "/opt/Xcode/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc",
            "first argv element should be the resolved swiftc"
        );
        // The action env carries DEVELOPER_DIR through to the tool so
        // it can find ancillary resources next to swiftc.
        assert_eq!(
            action.env.get("DEVELOPER_DIR").map(String::as_str),
            Some("/opt/Xcode/Developer"),
        );
    }
}

/// `_resolve_attrs` must reject `select()` on attributes the target kind
/// schema marks non-configurable (e.g. `module_name`). Without
/// this guard, a select on `module_name` would silently resolve
/// against the configuration and the build would proceed with a
/// rewritten module name, defeating the schema's intent.
#[test]
fn prelude_resolve_attrs_rejects_select_on_non_configurable_attr() {
    let err = eval_prelude_function(
            "_resolve_attrs",
            r#"({"platform": "ios", "module_name": {"select": {"ios": "X", "default": "Y"}}}, "tgt", ["module_name"])"#,
        )
        .unwrap_err();
    assert!(
        err.contains("attribute `module_name` is not configurable but uses select()"),
        "{err}"
    );
}
