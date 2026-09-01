use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use once_frontend::analysis::{
    globals_for_prelude, target_kind_has_impl, with_active_store, AnalysisStore, DeclaredAction,
    DeclaredActionOperation, DeclaredArchiveEntryKind, DeclaredArchiveFormat,
    DeclaredArgFileFormat, DeclaredCopyPathMode, DeclaredPreparePathMode,
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

fn action_by_identifier<'a>(store: &'a AnalysisStore, identifier: &str) -> &'a DeclaredAction {
    store
        .actions
        .iter()
        .find(|action| action.identifier.as_deref() == Some(identifier))
        .unwrap_or_else(|| panic!("missing action `{identifier}`"))
}

fn action_has_input_suffix(action: &DeclaredAction, suffix: &str) -> bool {
    action.inputs.iter().any(|input| input.ends_with(suffix))
}

fn assert_target_kind_attrs(kind: &str, expected: &[&str]) {
    let schema = built_in_target_kind_schema(kind)
        .unwrap_or_else(|| panic!("missing target kind schema `{kind}`"));
    let names = schema
        .attrs
        .iter()
        .map(|attr| attr.name.as_str())
        .collect::<Vec<_>>();
    for name in expected {
        assert!(
            names.contains(name),
            "target kind `{kind}` is missing parity attribute `{name}`"
        );
    }
}

#[cfg(unix)]
fn android_ndk_prebuilt_tag() -> &'static str {
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "darwin-arm64"
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
        "darwin-x86_64"
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
        "linux-arm64"
    } else {
        "linux-x86_64"
    }
}

#[cfg(unix)]
fn fake_android_ndk_for_mobile_test(workspace: &Path) -> std::path::PathBuf {
    let ndk = workspace.join("android-ndk");
    for tag in [
        "darwin-arm64",
        "darwin-x86_64",
        "linux-arm64",
        "linux-x86_64",
    ] {
        let bin_dir = ndk.join("toolchains/llvm/prebuilt").join(tag).join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(bin_dir.join("clang"), "").unwrap();
    }
    std::fs::write(
        ndk.join("toolchains/llvm/prebuilt")
            .join(android_ndk_prebuilt_tag())
            .join("bin/aarch64-linux-android24-clang"),
        "",
    )
    .unwrap();
    ndk
}

fn apple_prelude_source() -> String {
    format!(
        "{}\n{}",
        include_str!("../prelude/common.star"),
        include_str!("../prelude/apple.star")
    )
}

fn archive_prelude_source() -> String {
    format!(
        "{}\n{}",
        include_str!("../prelude/common.star"),
        include_str!("../prelude/archive.star")
    )
}

fn android_prelude_source() -> String {
    format!(
        "{}\n{}",
        include_str!("../prelude/common.star"),
        include_str!("../prelude/android.star")
    )
}

fn react_native_prelude_source() -> String {
    format!(
        "{}\n{}\n{}",
        include_str!("../prelude/common.star"),
        include_str!("../prelude/android.star"),
        include_str!("../prelude/react_native.star")
    )
}

fn go_prelude_source() -> String {
    format!(
        "{}\n{}",
        include_str!("../prelude/common.star"),
        include_str!("../prelude/go.star")
    )
}

fn xcode_prelude_source() -> String {
    format!(
        "{}\n{}\n{}",
        include_str!("../prelude/common.star"),
        include_str!("../prelude/apple.star"),
        include_str!("../prelude/xcode.star")
    )
}

#[test]
fn prelude_common_deduplicates_complete_argument_groups() {
    let source = format!(
        r#"{}
result = repr(_unique_args(
    [
        "--profile",
        "--forward", "--needs-value", "--forward", "first",
        "--forward", "--flag",
        "--forward", "--needs-value", "--forward", "second",
        "--forward", "--flag",
        "--pair", "one",
        "--pair", "two",
    ],
    option_arity = {{"--pair": 1}},
    forwarder = "--forward",
    forwarded_option_arity = {{"--needs-value": 1}},
))
"#,
        include_str!("../prelude/common.star")
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["--profile", "--forward", "--needs-value", "--forward", "first", "--forward", "--flag", "--forward", "--needs-value", "--forward", "second", "--pair", "one", "--pair", "two"]"#
    );
}

#[test]
fn prelude_apple_merges_link_options_as_complete_argument_groups() {
    let source = include_str!("../prelude/apple.star");
    assert!(
        source
            .matches("_apple_unique_linkopts(linkopts + dep_linkopts)")
            .count()
            >= 2,
        "framework and application links must merge complete argument groups"
    );
    assert!(!source.contains("if opt not in linkopts"));
    assert!(!source.contains("swift_argv.extend([\"-Xlinker\", opt])"));
}

fn oci_prelude_source() -> String {
    format!(
        "{}\n{}",
        include_str!("../prelude/common.star"),
        include_str!("../prelude/oci.star")
    )
}

fn dockerfile_prelude_source() -> String {
    format!(
        "{}\n{}",
        include_str!("../prelude/common.star"),
        include_str!("../prelude/dockerfile.star")
    )
}

fn all_prelude_source() -> String {
    [
        include_str!("../prelude/common.star"),
        include_str!("../prelude/lint.star"),
        include_str!("../prelude/apple.star"),
        include_str!("../prelude/android.star"),
        include_str!("../prelude/go.star"),
        include_str!("../prelude/rust.star"),
        include_str!("../prelude/xcode.star"),
        include_str!("../prelude/c.star"),
        include_str!("../prelude/cmake.star"),
        include_str!("../prelude/zig.star"),
        include_str!("../prelude/oci.star"),
        include_str!("../prelude/dockerfile.star"),
        include_str!("../prelude/swift.star"),
        include_str!("../prelude/kotlin.star"),
        include_str!("../prelude/elixir.star"),
        include_str!("../prelude/python.star"),
        include_str!("../prelude/ruby.star"),
        include_str!("../prelude/javascript.star"),
        include_str!("../prelude/react_native.star"),
    ]
    .join("\n")
}

#[test]
fn ruff_lint_declares_a_cacheable_report_for_finding_exit_codes() {
    let workspace = TempDir::new().unwrap();
    let package = workspace.path().join("quality");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("main.py"), "import os\n").unwrap();
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def _lint_executable(ctx, name, default, workspace_candidates = []):
    return "/tools/" + default

def host_command(argv, env = None, merge_stderr = None):
    return "ruff 1.0.0"

ctx = {{
    "label": {{"package": "quality", "name": "lint", "id": "quality/lint"}},
    "attr": {{}},
    "configuration": {{"tokens": []}},
    "deps": [],
    "srcs": ["*.py"],
    "build_dir": ".once/out/quality/lint",
    "scratch_dir": ".once/tmp/analysis/quality/lint",
    "capability": "lint",
}}
result = repr(_ruff_lint_impl(ctx))
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "quality".to_string(),
        ".once/out/quality/lint".to_string(),
    );
    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let provider = result.unwrap();
    assert!(provider.contains("\"schema\": \"once.lint_info.v1\""));
    let action = action_by_identifier(&store, "quality/lint:ruff");
    assert_eq!(action.success_exit_codes, vec![0, 1]);
    assert!(action
        .outputs
        .iter()
        .any(|path| path.ends_with("/lint/report.sarif")));
    assert!(action.inputs.iter().any(|path| path == "quality/main.py"));
}

#[test]
fn credo_lint_passes_every_declared_source_to_the_analyzer() {
    let workspace = TempDir::new().unwrap();
    let package = workspace.path().join("quality/elixir/lib");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("greeter.ex"), "defmodule Greeter do\nend\n").unwrap();
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def _lint_executable(ctx, name, default, workspace_candidates = []):
    return "/tools/" + default

def host_command(argv, env = None, merge_stderr = None):
    return "Credo 1.7.0"

ctx = {{
    "label": {{"package": "quality/elixir", "name": "lint", "id": "quality/elixir/lint"}},
    "attr": {{}},
    "configuration": {{"tokens": []}},
    "deps": [],
    "srcs": ["lib/**/*.ex"],
    "build_dir": ".once/out/quality/elixir/lint",
    "scratch_dir": ".once/tmp/analysis/quality/elixir/lint",
    "capability": "lint",
}}
result = repr(_credo_impl(ctx))
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "quality/elixir".to_string(),
        ".once/out/quality/elixir/lint".to_string(),
    );
    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    result.unwrap();
    let action = action_by_identifier(&store, "quality/elixir/lint:credo");
    assert!(action
        .argv
        .iter()
        .any(|arg| arg.ends_with("/quality/elixir/lib/greeter.ex")));
    assert!(action_has_input_suffix(
        action,
        "quality/elixir/lib/greeter.ex"
    ));
}

#[test]
fn rubocop_lint_declares_ruby_for_its_report_adapter() {
    let schema = built_in_target_kind_schema("rubocop_lint").unwrap();

    assert!(schema
        .tools
        .iter()
        .any(|tool| tool.name == "ruby" && tool.executables == ["ruby"]));
}

#[test]
fn built_in_target_kinds_do_not_hardcode_a_posix_shell_path() {
    let source = all_prelude_source();
    assert!(!source.contains("\"/bin/sh\""));
    assert!(!source.contains("'/bin/sh'"));
}

#[test]
fn react_native_target_kinds_are_discoverable_with_implementations() {
    let kinds = [
        "react_native_dependencies",
        "react_native_module",
        "react_native_bundle",
        "react_native_codegen",
        "react_native_autolinking",
        "react_native_apple_application",
        "react_native_android_application",
        "react_native_metro",
    ];
    for kind in kinds {
        assert!(
            built_in_target_kind_schema(kind).is_some(),
            "missing target kind schema `{kind}`"
        );
        assert!(
            target_kind_has_impl(kind).unwrap(),
            "target kind `{kind}` should have an implementation"
        );
    }
}

#[test]
fn react_native_schema_exposes_locked_and_live_development_inputs() {
    assert_target_kind_attrs(
        "react_native_dependencies",
        &[
            "package_json",
            "package_manager",
            "package_manager_executable",
            "lockfile",
            "npmrc",
            "package_manager_files",
            "modules_snapshot",
            "allow_network",
        ],
    );
    assert_target_kind_attrs(
        "react_native_bundle",
        &["platform", "entry", "metro_config", "hermes"],
    );
    assert_target_kind_attrs(
        "react_native_metro",
        &["port", "host", "metro_config", "reset_cache"],
    );
    assert_target_kind_attrs(
        "react_native_android_application",
        &["apk_path", "adb_serial", "exclude_srcs"],
    );
    assert_target_kind_attrs("react_native_apple_application", &["sdk", "exclude_srcs"]);
}

#[test]
fn react_native_resolver_accepts_version_ranges_and_nested_hermes() {
    let source = format!(
        r#"{}
ctx = {{
    "label": {{"id": "Dependencies"}},
    "attrs": {{
        "package_json": "package.json",
        "lockfile": "package-lock.json",
    }},
    "files": {{
        "package.json": "{{\"dependencies\":{{\"react-native\":\"^0.86.0\"}}}}",
        "package-lock.json": "{{\"packages\":{{\"node_modules/react-native\":{{\"version\":\"0.86.0\"}},\"node_modules/react-native/node_modules/hermes-compiler\":{{\"version\":\"250829098.0.14\"}}}}}}",
    }},
}}
result = repr(_react_native_dependencies_resolver(ctx))
"#,
        react_native_prelude_source()
    );

    let result = eval_prelude_source_to_repr(source).unwrap();

    assert!(result.contains("\"_react_native_version\": \"0.86.0\""));
    assert!(result.contains("\"_hermes_version\": \"250829098.0.14\""));
    assert!(result.contains("\"_package_manager\": \"npm\""));
    assert!(result.contains("\"_lockfile\": \"package-lock.json\""));
}

#[test]
fn react_native_resolver_supports_pnpm_yarn_and_bun_lockfiles() {
    let cases = [
        (
            "pnpm",
            "pnpm-lock.yaml",
            "lockfileVersion: '9.0'\npackages:\n  hermes-compiler@250829098.0.14:\n    resolution: {}\n  react-native@0.86.0:\n    resolution: {}\n  react-native-safe-area-context@5.8.0:\n    resolution: {}\n",
        ),
        (
            "yarn",
            "yarn.lock",
            "__metadata:\n  version: 8\n\"hermes-compiler@npm:250829098.0.14\":\n  version: 250829098.0.14\n\"react-native@npm:0.86.0\":\n  version: 0.86.0\n\"react-native-safe-area-context@npm:^5.5.2\":\n  version: 5.8.0\n",
        ),
        (
            "bun",
            "bun.lock",
            "{\n  \"lockfileVersion\": 1,\n  \"packages\": {\n    \"hermes-compiler\": [\"hermes-compiler@250829098.0.14\", \"\", {}],\n    \"react-native\": [\"react-native@0.86.0\", \"\", {}],\n    \"react-native-safe-area-context\": [\"react-native-safe-area-context@5.8.0\", \"\", {}],\n  },\n}\n",
        ),
    ];
    for (manager, lockfile, contents) in cases {
        let source = format!(
            r#"{}
ctx = {{
    "label": {{"id": "Dependencies"}},
    "attrs": {{
        "package_json": "package.json",
        "lockfile": "{lockfile}",
        "modules_snapshot": "react-native-modules.json",
    }},
    "files": {{
        "package.json": "{{\"dependencies\":{{\"react-native\":\"^0.86.0\"}}}}",
        "{lockfile}": {contents:?},
        "react-native-modules.json": "{{\"schema\":\"once.react_native.modules.v1\",\"react_native_version\":\"0.86.0\",\"modules\":[{{\"name\":\"react-native-safe-area-context\"}}]}}",
    }},
}}
result = repr(_react_native_dependencies_resolver(ctx))
"#,
            react_native_prelude_source()
        );

        let result = eval_prelude_source_to_repr(source).unwrap();

        assert!(
            result.contains(&format!("\"_package_manager\": \"{manager}\"")),
            "{result}"
        );
        assert!(
            result.contains("\"_react_native_version\": \"0.86.0\""),
            "{result}"
        );
        assert!(
            result.contains("\"_hermes_version\": \"250829098.0.14\""),
            "{result}"
        );
        assert!(
            result.contains("react-native-module-react-native-safe-area-context_x64_5.8.0"),
            "{result}"
        );
    }
}

#[test]
fn react_native_resolver_detects_yarn_classic_from_package_manager() {
    let source = format!(
        r#"{}
ctx = {{
    "label": {{"id": "Dependencies"}},
    "attrs": {{
        "package_json": "package.json",
    }},
    "files": {{
        "package.json": "{{\"packageManager\":\"yarn@1.22.22\",\"dependencies\":{{\"react-native\":\"0.86.0\"}}}}",
        "yarn.lock": "\"hermes-compiler@250829098.0.14\":\n  version \"250829098.0.14\"\n\"react-native@0.86.0\":\n  version \"0.86.0\"\n",
    }},
}}
result = repr(_react_native_dependencies_resolver(ctx))
"#,
        react_native_prelude_source()
    );

    let result = eval_prelude_source_to_repr(source).unwrap();

    assert!(
        result.contains("\"_package_manager\": \"yarn\""),
        "{result}"
    );
    assert!(result.contains("\"_lockfile\": \"yarn.lock\""), "{result}");
    assert!(
        result.contains("\"_react_native_version\": \"0.86.0\""),
        "{result}"
    );
    assert!(
        result.contains("\"_hermes_version\": \"250829098.0.14\""),
        "{result}"
    );
}

#[test]
fn react_native_resolver_rejects_credentials_in_package_manager_configuration() {
    let source = format!(
        r#"{}
ctx = {{
    "label": {{"id": "Dependencies"}},
    "attrs": {{
        "package_json": "package.json",
        "lockfile": "package-lock.json",
        "package_manager_files": [".npmrc"],
    }},
    "files": {{
        "package.json": "{{\"dependencies\":{{\"react-native\":\"0.86.0\"}}}}",
        "package-lock.json": "{{\"packages\":{{\"node_modules/react-native\":{{\"version\":\"0.86.0\"}},\"node_modules/hermes-compiler\":{{\"version\":\"250829098.0.14\"}}}}}}",
        ".npmrc": "//registry.example.com/:_authToken=secret",
    }},
}}
result = repr(_react_native_dependencies_resolver(ctx))
"#,
        react_native_prelude_source()
    );

    let error = eval_prelude_source_to_repr(source).unwrap_err();

    assert!(
        error.contains("must not contain authentication tokens"),
        "{error}"
    );
}

#[test]
fn react_native_dependency_install_does_not_stage_the_module_snapshot() {
    let workspace = TempDir::new().unwrap();
    let package = workspace.path().join("app");
    std::fs::create_dir_all(&package).unwrap();
    for path in [
        "package.json",
        "package-lock.json",
        "react-native-modules.json",
    ] {
        std::fs::write(package.join(path), "{}\n").unwrap();
    }
    let prelude = react_native_prelude_source();
    let source = format!(
        r#"{prelude}
def _react_native_tools(ctx, include_package_manager = False):
    return {{
        "node": "/tools/node",
        "node_version": "v24.0.0",
        "package_manager": "/tools/npm",
        "package_manager_name": "npm",
        "package_manager_version": "11.0.0",
        "identity": "node-v24-npm-v11",
    }}

ctx = {{
    "label": {{"package": "app", "name": "Dependencies", "id": "app/Dependencies"}},
    "attr": {{
        "_react_native_resolved": True,
        "_package_manager": "npm",
        "_lockfile": "package-lock.json",
        "_react_native_version": "0.86.0",
        "_hermes_version": "250829098.0.14",
        "modules_snapshot": "react-native-modules.json",
        "allow_network": True,
    }},
    "configuration": {{"tokens": []}},
    "deps": [],
    "srcs": ["package.json", "package-lock.json", "react-native-modules.json"],
    "build_dir": ".once/out/app/Dependencies",
    "scratch_dir": ".once/tmp/analysis/app/Dependencies",
    "capability": "build",
}}
result = repr(_react_native_dependencies_impl(ctx))
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "app".to_string(),
        ".once/out/app/Dependencies".to_string(),
    );

    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert!(result
        .unwrap()
        .contains("\"react_native_dependency_set\": True"));
    assert!(!store.actions.iter().any(|action| {
        action
            .identifier
            .as_deref()
            .is_some_and(|identifier| identifier.contains("react-native-modules.json"))
    }));
    let install = action_by_identifier(&store, "app/Dependencies:package-install");
    assert!(!install
        .inputs
        .iter()
        .any(|input| input.ends_with("react-native-modules.json")));
}

#[test]
fn react_native_package_managers_use_frozen_offline_install_plans() {
    let cases = [
        ("npm", "11.0.0", &["\"ci\"", "\"--offline\""][..]),
        (
            "pnpm",
            "11.0.0",
            &[
                "\"--frozen-lockfile\"",
                "\"--public-hoist-pattern\"",
                "\"@react-native/*\"",
                "\"--store-dir\"",
                "\"--offline\"",
            ],
        ),
        (
            "yarn",
            "1.22.22",
            &["\"--frozen-lockfile\"", "\"--offline\""],
        ),
        (
            "yarn",
            "4.17.1",
            &[
                "\"--immutable\"",
                "\"YARN_NODE_LINKER\": \"node-modules\"",
                "\"YARN_ENABLE_NETWORK\": \"0\"",
            ],
        ),
        (
            "bun",
            "1.3.11",
            &[
                "\"--frozen-lockfile\"",
                "\"--linker\"",
                "\"hoisted\"",
                "\"--prefer-offline\"",
            ],
        ),
    ];
    for (manager, version, expected) in cases {
        let source = format!(
            r#"{}
tools = {{
    "package_manager": "/tools/{manager}",
    "package_manager_name": "{manager}",
    "package_manager_version": "{version}",
}}
result = repr(_react_native_install_plan(tools, False, ".once/cache"))
"#,
            react_native_prelude_source()
        );

        let result = eval_prelude_source_to_repr(source).unwrap();

        for value in expected {
            assert!(result.contains(value), "{manager}: {result}");
        }
    }
}

#[test]
fn react_native_bundle_declares_metro_hermes_and_portable_staging_actions() {
    let workspace = TempDir::new().unwrap();
    let package = workspace.path().join("app");
    std::fs::create_dir_all(&package).unwrap();
    for (path, contents) in [
        ("index.js", "export default {};\n"),
        ("metro.config.js", "module.exports = {};\n"),
        ("package.json", "{}\n"),
        ("package-lock.json", "{}\n"),
    ] {
        std::fs::write(package.join(path), contents).unwrap();
    }
    let prelude = react_native_prelude_source();
    let source = format!(
        r#"{prelude}
def _react_native_tools(ctx, include_package_manager = False):
    return {{
        "node": "/tools/node",
        "node_version": "v24.0.0",
        "identity": "node-v24",
    }}

def host_os():
    return "macos"

def host_arch():
    return "arm64"

ctx = {{
    "label": {{"package": "app", "name": "Bundle", "id": "app/Bundle"}},
    "attr": {{
        "platform": "ios",
        "entry": "index.js",
        "metro_config": "metro.config.js",
        "hermes": True,
    }},
    "configuration": {{"tokens": []}},
    "deps": [{{
        "react_native_dependency_set": True,
        "javascript_root": ".once/out/Dependencies/javascript",
        "node_modules": ".once/out/Dependencies/javascript/node_modules",
        "package_json": "app/package.json",
        "lockfile": "app/package-lock.json",
        "react_native_version": "0.86.0",
        "hermes_version": "250829098.0.14",
    }}],
    "srcs": ["index.js"],
    "build_dir": ".once/out/app/Bundle",
    "scratch_dir": ".once/tmp/analysis/app/Bundle",
    "capability": "build",
}}
result = repr(_react_native_bundle_impl(ctx))
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "app".to_string(),
        ".once/out/app/Bundle".to_string(),
    );

    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let result = result.unwrap();
    assert!(result.contains("\"react_native_bundle\": True"));
    assert!(result.contains("\"sources\": [\"app/index.js\", \"app/metro.config.js\"]"));
    let stage_link = action_by_identifier(&store, "app/Bundle:stage-node-modules");
    assert!(matches!(
        stage_link.operation,
        Some(DeclaredActionOperation::LinkPath { .. })
    ));
    let metro = action_by_identifier(&store, "app/Bundle:metro-bundle");
    assert!(metro.inputs.iter().any(|input| input.ends_with("project")));
    assert!(store.actions.iter().any(|action| {
        matches!(
            &action.operation,
            Some(DeclaredActionOperation::CopyPath { sources, .. })
                if sources == &vec!["app/metro.config.js".to_string()]
        )
    }));
    let hermes = action_by_identifier(&store, "app/Bundle:hermes-compile");
    assert_eq!(hermes.argv[0], "/tools/node");
    assert!(hermes.argv[1].contains("run-hermes.js"));
    assert!(hermes
        .inputs
        .iter()
        .any(|input| input.ends_with("run-hermes.js")));
    action_by_identifier(&store, "app/Bundle:compose-source-maps");
}

#[test]
fn react_native_metro_uses_live_sources_and_an_immutable_dependency_snapshot() {
    let workspace = TempDir::new().unwrap();
    let package = workspace.path().join("app");
    std::fs::create_dir_all(&package).unwrap();
    for (path, contents) in [
        ("index.js", "export default {};\n"),
        (
            "metro.config.js",
            "module.exports = require('./metro.shared');\n",
        ),
        ("metro.shared", "module.exports = {};\n"),
        ("package.json", "{}\n"),
        ("package-lock.json", "{}\n"),
    ] {
        std::fs::write(package.join(path), contents).unwrap();
    }
    let prelude = react_native_prelude_source();
    let source = format!(
        r#"{prelude}
def _react_native_tools(ctx, include_package_manager = False):
    return {{
        "node": "/tools/node",
        "node_version": "v24.0.0",
        "identity": "node-v24",
    }}

ctx = {{
    "label": {{"package": "app", "name": "Metro", "id": "app/Metro"}},
    "attr": {{
        "metro_config": "metro.config.js",
        "port": 8081,
        "host": "127.0.0.1",
    }},
    "configuration": {{"tokens": []}},
    "deps": [{{
        "react_native_dependency_set": True,
        "javascript_root": ".once/out/Dependencies/javascript",
        "node_modules": ".once/out/Dependencies/javascript/node_modules",
        "package_json": "app/package.json",
        "lockfile": "app/package-lock.json",
        "react_native_version": "0.86.0",
    }}],
    "srcs": ["index.js", "metro.config.js", "metro.shared"],
    "build_dir": ".once/out/app/Metro",
    "scratch_dir": ".once/tmp/analysis/app/Metro",
    "capability": "run",
}}
result = repr(_react_native_metro_impl(ctx))
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "app".to_string(),
        ".once/out/app/Metro".to_string(),
    );

    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert!(result.unwrap().contains("\"react_native_metro\": True"));
    let metro = action_by_identifier(&store, "app/Metro:metro");
    assert!(!metro.cacheable);
    assert!(
        metro
            .argv
            .windows(2)
            .any(|args| args == ["--projectRoot", "{{once.execution_root}}/app"]),
        "{:?}",
        metro.argv
    );
    assert_eq!(
        metro.env.get("ONCE_REACT_NATIVE_METRO_CONFIG"),
        Some(&"{{once.execution_root}}/app/metro.config.js".to_string())
    );
    let snapshot = action_by_identifier(&store, "app/Metro:snapshot-dependencies");
    assert!(matches!(
        snapshot.operation,
        Some(DeclaredActionOperation::CopyPath {
            mode: DeclaredCopyPathMode::Tree,
            ..
        })
    ));
}

#[test]
fn react_native_android_build_uses_custom_application_output_and_exposes_logs() {
    let workspace = TempDir::new().unwrap();
    let package = workspace.path().join("app");
    std::fs::create_dir_all(package.join("android/gradle/wrapper")).unwrap();
    for (path, contents) in [
        ("package.json", "{}\n"),
        ("package-lock.json", "{}\n"),
        ("index.js", "export default {};\n"),
        ("App.tsx", "export default function App() {}\n"),
        ("android/settings.gradle", "rootProject.name = 'App'\n"),
        ("android/gradle/wrapper/gradle-wrapper.jar", "jar"),
    ] {
        std::fs::write(package.join(path), contents).unwrap();
    }
    let prelude = react_native_prelude_source();
    let source = format!(
        r#"{prelude}
def _react_native_tools(ctx, include_package_manager = False):
    return {{
        "node": "/tools/node",
        "node_version": "v24.0.0",
        "identity": "node-v24",
    }}

def _resolve_host_executable(name):
    return "/tools/" + name

def host_command(argv, env = None, merge_stderr = None):
    return "tool version"

def host_env(name):
    if name == "HOME":
        return "/home/test"
    return ""

ctx = {{
    "label": {{"package": "app", "name": "Android", "id": "app/Android"}},
    "attr": {{
        "application_id": "com.example",
        "native_root": "android",
        "module": "app",
        "configuration": "demoRelease",
        "apk_path": "android/app/build/outputs/apk/demo/release/app-demo-release.apk",
        "android_sdk": "/sdk",
        "android_ndk": "/ndk",
    }},
    "configuration": {{"tokens": []}},
    "deps": [{{
        "react_native_dependency_set": True,
        "javascript_root": ".once/out/Dependencies/javascript",
        "node_modules": ".once/out/Dependencies/javascript/node_modules",
        "package_json": "app/package.json",
        "lockfile": "app/package-lock.json",
        "react_native_version": "0.86.0",
        "hermes_version": "250829098.0.14",
    }}, {{
        "react_native_bundle": True,
        "platform": "android",
        "sources": ["app/index.js", "app/App.tsx"],
    }}],
    "srcs": ["android/**/*"],
    "build_dir": ".once/out/app/Android",
    "scratch_dir": ".once/tmp/analysis/app/Android",
    "capability": "build",
}}
result = repr(_react_native_android_application_impl(ctx))
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "app".to_string(),
        ".once/out/app/Android".to_string(),
    );

    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let result = result.unwrap();
    assert!(
        result.contains("\"logs\": [\".once/out/app/Android/gradle.log\"]"),
        "{result}"
    );
    let gradle = action_by_identifier(&store, "app/Android:gradle");
    assert!(gradle
        .argv
        .iter()
        .any(|arg| arg == "app:assembleDemoRelease"));
    assert!(gradle.outputs.iter().any(|output| output
        .ends_with("android/app/build/outputs/apk/demo/release/app-demo-release.apk")));
    assert_eq!(
        gradle.stdout.as_deref(),
        Some(".once/out/app/Android/gradle.log")
    );
    assert_eq!(gradle.stderr, gradle.stdout);
    action_by_identifier(&store, "app/Android:stage:index.js");
    action_by_identifier(&store, "app/Android:stage:App.tsx");
}

#[test]
fn react_native_android_run_waits_for_the_selected_device_and_orders_effects() {
    let workspace = TempDir::new().unwrap();
    let prelude = react_native_prelude_source();
    let source = format!(
        r#"{prelude}
def _resolve_host_executable(name):
    return "/tools/" + name

def host_command(argv, env = None, merge_stderr = None):
    return "Android Debug Bridge version"

ctx = {{
    "label": {{"package": "app", "name": "Android", "id": "app/Android"}},
    "attr": {{
        "application_id": "com.example",
        "configuration": "debug",
        "adb_serial": "emulator-5554",
        "launch_activity": ".MainActivity",
    }},
    "configuration": {{"tokens": []}},
    "deps": [{{"react_native_dependency_set": True}}],
    "srcs": [],
    "build_dir": ".once/out/app/Android",
    "scratch_dir": ".once/tmp/analysis/app/Android",
    "capability": "run",
}}
result = repr(_react_native_android_application_impl(ctx))
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "app".to_string(),
        ".once/out/app/Android".to_string(),
    );

    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert!(result
        .unwrap()
        .contains("\"application_id\": \"com.example\""));
    let wait = action_by_identifier(&store, "app/Android:android-wait");
    assert_eq!(
        wait.argv,
        ["/tools/adb", "-s", "emulator-5554", "wait-for-device"]
    );
    let install = action_by_identifier(&store, "app/Android:android-install");
    assert!(install
        .inputs
        .iter()
        .any(|input| input.ends_with("run/device-ready")));
    let reverse = action_by_identifier(&store, "app/Android:android-metro-reverse");
    assert!(reverse
        .inputs
        .iter()
        .any(|input| input.ends_with("run/installed")));
    let launch = action_by_identifier(&store, "app/Android:android-launch");
    assert!(launch
        .argv
        .iter()
        .any(|arg| arg == "com.example/.MainActivity"));
    assert!(launch
        .inputs
        .iter()
        .any(|input| input.ends_with("run/metro-reversed")));
    for action in [wait, install, reverse, launch] {
        assert!(!action.cacheable);
    }
}

#[test]
fn react_native_apple_build_distinguishes_simulator_and_device_destinations() {
    let workspace = TempDir::new().unwrap();
    let package = workspace.path().join("app");
    std::fs::create_dir_all(package.join("ios")).unwrap();
    for (path, contents) in [
        ("package.json", "{}\n"),
        ("package-lock.json", "{}\n"),
        ("index.js", "export default {};\n"),
        ("App.tsx", "export default function App() {}\n"),
        ("Gemfile", "source 'https://rubygems.org'\n"),
        ("Gemfile.lock", "BUNDLED WITH\n   4.0.0\n"),
        ("ios/Podfile", "platform :ios, '15.1'\n"),
    ] {
        std::fs::write(package.join(path), contents).unwrap();
    }
    let prelude = react_native_prelude_source();
    let source = format!(
        r#"{prelude}
def _react_native_tools(ctx, include_package_manager = False):
    return {{
        "node": "/tools/node",
        "node_version": "v24.0.0",
        "identity": "node-v24",
    }}

def _resolve_host_executable(name):
    return "/tools/" + name

def host_command(argv, env = None, merge_stderr = None):
    return "tool version"

def host_which_optional(name):
    return None

ctx = {{
    "label": {{"package": "app", "name": "Apple", "id": "app/Apple"}},
    "attr": {{
        "bundle_id": "com.example",
        "product_name": "Example",
        "native_root": "ios",
        "workspace": "Example.xcworkspace",
        "scheme": "Example",
        "configuration": "Release",
        "sdk": "iphoneos",
        "allow_network": True,
    }},
    "configuration": {{"tokens": []}},
    "deps": [{{
        "react_native_dependency_set": True,
        "javascript_root": ".once/out/Dependencies/javascript",
        "node_modules": ".once/out/Dependencies/javascript/node_modules",
        "package_json": "app/package.json",
        "lockfile": "app/package-lock.json",
        "react_native_version": "0.86.0",
        "hermes_version": "250829098.0.14",
    }}, {{
        "react_native_bundle": True,
        "platform": "ios",
        "sources": ["app/index.js", "app/App.tsx"],
    }}],
    "srcs": ["Gemfile", "Gemfile.lock", "ios/**/*"],
    "build_dir": ".once/out/app/Apple",
    "scratch_dir": ".once/tmp/analysis/app/Apple",
    "capability": "build",
}}
result = repr(_react_native_apple_application_impl(ctx))
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "app".to_string(),
        ".once/out/app/Apple".to_string(),
    );

    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let result = result.unwrap();
    assert!(
        result.contains("\"logs\": [\".once/out/app/Apple/xcodebuild.log\"]"),
        "{result}"
    );
    let xcodebuild = action_by_identifier(&store, "app/Apple:xcodebuild");
    assert!(xcodebuild
        .argv
        .windows(2)
        .any(|args| args == ["-destination", "generic/platform=iOS"]));
    assert!(!xcodebuild
        .argv
        .iter()
        .any(|arg| arg == "CODE_SIGNING_ALLOWED=NO"));
    assert!(xcodebuild
        .outputs
        .iter()
        .any(|output| output.ends_with("Release-iphoneos/Example.app")));
    assert_eq!(
        xcodebuild.stdout.as_deref(),
        Some(".once/out/app/Apple/xcodebuild.log")
    );
    assert_eq!(xcodebuild.stderr, xcodebuild.stdout);
    action_by_identifier(&store, "app/Apple:stage:index.js");
    action_by_identifier(&store, "app/Apple:stage:App.tsx");
}

#[test]
fn go_target_kind_schemas_cover_bazel_buck_and_locked_modules() {
    for kind in [
        "go_dependencies",
        "go_module",
        "go_source",
        "go_library",
        "go_binary",
        "go_test",
    ] {
        assert!(target_kind_has_impl(kind).unwrap(), "missing `{kind}` impl");
        let schema = built_in_target_kind_schema(kind)
            .unwrap_or_else(|| panic!("missing target kind schema `{kind}`"));
        assert!(
            schema
                .examples
                .iter()
                .any(|example| example.slug == "go-comprehensive"),
            "{kind} should expose the comprehensive Go starter"
        );
        assert!(
            schema
                .source_references
                .iter()
                .all(|reference| reference.content_digest.is_some()),
            "{kind} should bind complete upstream sources"
        );
    }

    assert_target_kind_attrs(
        "go_binary",
        &[
            "build_mode",
            "cgo",
            "goarch",
            "goos",
            "link_mode",
            "linkmode",
            "package_name",
            "pgoprofile",
            "race",
            "x_defs",
        ],
    );
    assert_target_kind_attrs(
        "go_test",
        &[
            "cover_packages",
            "coverage_mode",
            "fail_fast",
            "rundir",
            "test_env",
        ],
    );
}

#[test]
fn oci_target_kinds_expose_layers_images_and_runnable_examples() {
    for kind in ["oci_layer", "oci_image"] {
        assert!(target_kind_has_impl(kind).unwrap(), "missing `{kind}` impl");
        let schema = built_in_target_kind_schema(kind)
            .unwrap_or_else(|| panic!("missing target kind schema `{kind}`"));
        assert!(
            schema
                .examples
                .iter()
                .any(|example| example.slug == "oci-image-minimal"),
            "{kind} should expose the minimal container image starter"
        );
        assert!(
            schema
                .source_references
                .iter()
                .all(|reference| reference.content_digest.is_some()),
            "{kind} should bind complete upstream sources"
        );
    }

    let layer = built_in_target_kind_schema("oci_layer").unwrap();
    assert!(layer
        .providers
        .iter()
        .any(|provider| provider == "oci_layer"));
    assert_target_kind_attrs(
        "oci_layer",
        &[
            "archive",
            "architecture",
            "data_dir",
            "group_id",
            "owner_id",
            "program_dir",
        ],
    );

    let image = built_in_target_kind_schema("oci_image").unwrap();
    assert!(image
        .providers
        .iter()
        .any(|provider| provider == "oci_image"));
    assert_target_kind_attrs(
        "oci_image",
        &[
            "annotations",
            "architecture",
            "cmd",
            "entrypoint",
            "env",
            "tag",
        ],
    );
}

#[test]
fn native_binary_schemas_expose_the_shared_executable_provider() {
    for kind in ["go_binary", "rust_binary", "zig_binary"] {
        let schema = built_in_target_kind_schema(kind)
            .unwrap_or_else(|| panic!("missing target kind schema `{kind}`"));
        assert!(
            schema
                .providers
                .iter()
                .any(|provider| provider == "once_executable"),
            "{kind} should expose the shared executable provider"
        );
    }
}

#[test]
fn dockerfile_image_schema_exposes_buildkit_outputs_and_example() {
    let schema = built_in_target_kind_schema("dockerfile_image").expect("dockerfile_image schema");
    assert!(target_kind_has_impl("dockerfile_image").unwrap());
    assert!(schema
        .providers
        .iter()
        .any(|provider| provider == "container_image"));
    assert!(schema
        .examples
        .iter()
        .any(|example| example.slug == "dockerfile-image-minimal"));
    assert_target_kind_attrs(
        "dockerfile_image",
        &[
            "build_args",
            "cacheable",
            "context",
            "dockerfile",
            "format",
            "network",
            "platform",
            "target",
        ],
    );
}

#[test]
fn dockerfile_image_declares_an_isolated_buildx_action() {
    let workspace = TempDir::new().unwrap();
    let package = workspace.path().join("containers/demo");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("Dockerfile"), "FROM scratch\n").unwrap();
    std::fs::write(package.join(".dockerignore"), ".once\n").unwrap();
    std::fs::write(package.join("message.txt"), "hello\n").unwrap();
    let source = format!(
        r#"{}
def host_which_optional(name):
    return "/tools/docker" if name == "docker" else None

def host_command(argv, env = None, cwd = None, merge_stderr = None):
    if argv[1:3] == ["buildx", "version"]:
        return "github.com/docker/buildx v1.2.3\n"
    if argv[1:3] == ["buildx", "inspect"]:
        return "Driver: docker\nBuildKit version: v4.5.6\n"
    fail("unexpected command: " + repr(argv))

def host_env(name):
    return "/usr/bin" if name == "PATH" else ""

ctx = {{
    "label": {{"package": "containers/demo", "name": "image", "id": "containers/demo/image"}},
    "attr": {{
        "tag": "example/image:test",
        "build_args": {{"MODE": "release"}},
        "labels": {{"org.example.kind": "test"}},
    }},
    "srcs": [".dockerignore", "Dockerfile", "message.txt"],
    "build_dir": ".once/out/containers/demo/image",
    "scratch_dir": ".once/tmp/analysis/containers/demo/image",
    "capability": "build",
}}
provider = _dockerfile_image_impl(ctx)
result = repr(provider)
"#,
        dockerfile_prelude_source()
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "containers/demo".to_string(),
        ".once/out/containers/demo/image".to_string(),
    );

    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let provider = result.unwrap();
    assert!(provider.contains("\"container_image\": True"), "{provider}");
    assert!(provider.contains("\"format\": \"docker\""), "{provider}");
    let action = action_by_identifier(&store, "containers/demo/image:dockerfile-build");
    assert!(!action.cacheable);
    assert_eq!(action.sandbox.as_deref(), Some("copied-inputs"));
    assert_eq!(action.cwd.as_deref(), Some("containers/demo"));
    assert!(action.stdout.is_none());
    assert!(action.stderr.is_none());
    assert!(action.argv.starts_with(&[
        "/tools/docker".to_string(),
        "buildx".to_string(),
        "build".to_string(),
    ]));
    assert!(action
        .argv
        .windows(2)
        .any(|args| args == ["--build-arg", "MODE=release"]));
    assert!(action
        .argv
        .windows(2)
        .any(|args| args == ["--tag", "example/image:test"]));
    assert_eq!(
        action.inputs,
        vec![
            "containers/demo/.dockerignore".to_string(),
            "containers/demo/Dockerfile".to_string(),
            "containers/demo/message.txt".to_string(),
        ]
    );
    let save = action_by_identifier(&store, "containers/demo/image:docker-save");
    assert!(!save.cacheable);
    assert_eq!(
        save.inputs,
        vec![".once/out/containers/demo/image/build-metadata.json".to_string()]
    );
    assert!(save.argv.windows(2).any(|args| args == ["image", "save"]));
}

#[test]
fn dockerfile_image_infers_hidden_and_nested_context_inputs() {
    let workspace = TempDir::new().unwrap();
    let package = workspace.path().join("containers/demo");
    std::fs::create_dir_all(package.join(".hidden")).unwrap();
    std::fs::create_dir_all(package.join("nested/.cache")).unwrap();
    std::fs::create_dir_all(package.join("nested")).unwrap();
    std::fs::create_dir_all(package.join("ignored")).unwrap();
    std::fs::write(package.join("Dockerfile"), "FROM scratch\n").unwrap();
    std::fs::write(
        package.join(".dockerignore"),
        "ignored\nignored.txt\n**/.cache\n",
    )
    .unwrap();
    std::fs::write(package.join(".hidden/config"), "hidden\n").unwrap();
    std::fs::write(package.join("nested/message.txt"), "hello\n").unwrap();
    std::fs::write(package.join("nested/.cache/value"), "ignored\n").unwrap();
    std::fs::write(package.join("ignored/result.txt"), "ignored\n").unwrap();
    std::fs::write(package.join("ignored.txt"), "ignored\n").unwrap();
    let source = format!(
        r#"{}
def host_which_optional(name):
    return "/tools/docker" if name == "docker" else None

def host_command(argv, env = None, cwd = None, merge_stderr = None):
    if argv[1:3] == ["buildx", "version"]:
        return "github.com/docker/buildx v1.2.3\n"
    if argv[1:3] == ["buildx", "inspect"]:
        return "Driver: docker-container\nBuildKit version: v4.5.6\n"
    fail("unexpected command: " + repr(argv))

def host_env(name):
    return ""

ctx = {{
    "label": {{"package": "containers/demo", "name": "image", "id": "containers/demo/image"}},
    "attr": {{}},
    "srcs": [],
    "build_dir": ".once/out/containers/demo/image",
    "scratch_dir": ".once/tmp/analysis/containers/demo/image",
    "capability": "build",
}}
provider = _dockerfile_image_impl(ctx)
result = repr(provider)
"#,
        dockerfile_prelude_source()
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "containers/demo".to_string(),
        ".once/out/containers/demo/image".to_string(),
    );

    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let provider = result.unwrap();
    assert!(
        provider.contains("\"input_discovery\": \"context\""),
        "{provider}"
    );
    let action = action_by_identifier(&store, "containers/demo/image:dockerfile-build");
    assert_eq!(
        action.inputs,
        vec![
            "containers/demo/.dockerignore".to_string(),
            "containers/demo/.hidden/config".to_string(),
            "containers/demo/Dockerfile".to_string(),
            "containers/demo/nested/message.txt".to_string(),
        ]
    );
}

#[test]
fn dockerfile_image_normalizes_root_context_and_excludes_runtime_state() {
    let workspace = TempDir::new().unwrap();
    std::fs::create_dir_all(workspace.path().join(".once/out/web_image")).unwrap();
    std::fs::create_dir_all(workspace.path().join("nested/.once/out")).unwrap();
    std::fs::write(workspace.path().join("Dockerfile"), "FROM scratch\n").unwrap();
    std::fs::write(workspace.path().join("message.txt"), "hello\n").unwrap();
    std::fs::write(
        workspace.path().join(".once/out/web_image/image.tar"),
        "output\n",
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("nested/.once/out/generated"),
        "generated\n",
    )
    .unwrap();
    let source = format!(
        r#"{}
def host_which_optional(name):
    return "/tools/docker" if name == "docker" else None

def host_command(argv, env = None, cwd = None, merge_stderr = None):
    if argv[1:3] == ["buildx", "version"]:
        return "github.com/docker/buildx v1.2.3\n"
    if argv[1:3] == ["buildx", "inspect"]:
        return "Driver: docker-container\nBuildKit version: v4.5.6\n"
    fail("unexpected command: " + repr(argv))

def host_env(name):
    return ""

ctx = {{
    "label": {{"package": "", "name": "image", "id": "image"}},
    "attr": {{"context": "./"}},
    "srcs": [],
    "build_dir": ".once/out/image",
    "scratch_dir": ".once/tmp/analysis/image",
    "capability": "build",
}}
result = repr(_dockerfile_image_impl(ctx))
"#,
        dockerfile_prelude_source()
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        String::new(),
        ".once/out/image".to_string(),
    );

    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    result.unwrap();
    let action = action_by_identifier(&store, "image:dockerfile-build");
    assert_eq!(
        action.inputs,
        vec!["Dockerfile".to_string(), "message.txt".to_string()]
    );
    assert_eq!(action.argv.last().map(String::as_str), Some("."));
}

#[test]
fn dockerfile_metadata_infers_inputs_without_probing_the_toolchain() {
    let workspace = TempDir::new().unwrap();
    std::fs::write(workspace.path().join("Dockerfile"), "FROM scratch\n").unwrap();
    std::fs::write(workspace.path().join("message.txt"), "hello\n").unwrap();
    let source = format!(
        r#"{}
def host_which_optional(name):
    fail("metadata analysis must not resolve host tools")

def host_command(argv, env = None, cwd = None, merge_stderr = None):
    fail("metadata analysis must not run host commands")

ctx = {{
    "label": {{"package": "", "name": "image", "id": "image"}},
    "attr": {{}},
    "srcs": [],
    "build_dir": ".once/out/image",
    "scratch_dir": ".once/tmp/analysis/image",
    "capability": "metadata",
}}
result = repr(_dockerfile_image_impl(ctx))
"#,
        dockerfile_prelude_source()
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        String::new(),
        ".once/out/image".to_string(),
    );

    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let provider = result.unwrap();
    assert!(store.actions.is_empty());
    assert!(
        provider.contains("\"affected_inputs\": [\"Dockerfile\", \"message.txt\"]"),
        "{provider}"
    );
}

#[test]
fn dockerfile_ignore_parent_pattern_is_kept_as_an_unmodeled_input_rule() {
    let source = format!(
        "{}\nresult = repr(_dockerfile_literal_excludes(\"..\\n\"))",
        dockerfile_prelude_source()
    );

    let result = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(result, "{\"paths\": [], \"names\": []}");
}

#[test]
fn dockerfile_image_keeps_inputs_when_ignore_rules_can_reinclude_them() {
    let workspace = TempDir::new().unwrap();
    let package = workspace.path().join("containers/demo");
    std::fs::create_dir_all(package.join("ignored")).unwrap();
    std::fs::write(package.join("Dockerfile"), "FROM scratch\n").unwrap();
    std::fs::write(
        package.join(".dockerignore"),
        "ignored\n!ignored/keep.txt\n**/*.log\n",
    )
    .unwrap();
    std::fs::write(package.join("ignored/drop.txt"), "drop\n").unwrap();
    std::fs::write(package.join("ignored/keep.txt"), "keep\n").unwrap();
    std::fs::write(package.join("debug.log"), "log\n").unwrap();
    let source = format!(
        r#"{}
def host_which_optional(name):
    return "/tools/docker" if name == "docker" else None

def host_command(argv, env = None, cwd = None, merge_stderr = None):
    if argv[1:3] == ["buildx", "version"]:
        return "github.com/docker/buildx v1.2.3\n"
    if argv[1:3] == ["buildx", "inspect"]:
        return "Driver: docker-container\nBuildKit version: v4.5.6\n"
    fail("unexpected command: " + repr(argv))

def host_env(name):
    return ""

ctx = {{
    "label": {{"package": "containers/demo", "name": "image", "id": "containers/demo/image"}},
    "attr": {{}},
    "srcs": [],
    "build_dir": ".once/out/containers/demo/image",
    "scratch_dir": ".once/tmp/analysis/containers/demo/image",
    "capability": "build",
}}
result = repr(_dockerfile_image_impl(ctx))
"#,
        dockerfile_prelude_source()
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "containers/demo".to_string(),
        ".once/out/containers/demo/image".to_string(),
    );

    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    result.unwrap();
    let action = action_by_identifier(&store, "containers/demo/image:dockerfile-build");
    assert!(action
        .inputs
        .contains(&"containers/demo/ignored/drop.txt".to_string()));
    assert!(action
        .inputs
        .contains(&"containers/demo/ignored/keep.txt".to_string()));
    assert!(action
        .inputs
        .contains(&"containers/demo/debug.log".to_string()));
}

#[test]
fn dockerfile_specific_ignore_controls_an_inferred_nested_context() {
    let workspace = TempDir::new().unwrap();
    let package = workspace.path().join("containers/demo");
    std::fs::create_dir_all(package.join("app/ignored")).unwrap();
    std::fs::create_dir_all(package.join("app/specific")).unwrap();
    std::fs::create_dir_all(package.join("docker")).unwrap();
    std::fs::write(package.join("docker/build.Dockerfile"), "FROM scratch\n").unwrap();
    std::fs::write(package.join("app/.dockerignore"), "ignored\n").unwrap();
    std::fs::write(
        package.join("docker/build.Dockerfile.dockerignore"),
        "specific\n",
    )
    .unwrap();
    std::fs::write(package.join("app/ignored/value.txt"), "included\n").unwrap();
    std::fs::write(package.join("app/specific/value.txt"), "excluded\n").unwrap();
    let source = format!(
        r#"{}
def host_which_optional(name):
    return "/tools/docker" if name == "docker" else None

def host_command(argv, env = None, cwd = None, merge_stderr = None):
    if argv[1:3] == ["buildx", "version"]:
        return "github.com/docker/buildx v1.2.3\n"
    if argv[1:3] == ["buildx", "inspect"]:
        return "Driver: docker-container\nBuildKit version: v4.5.6\n"
    fail("unexpected command: " + repr(argv))

def host_env(name):
    return ""

ctx = {{
    "label": {{"package": "containers/demo", "name": "image", "id": "containers/demo/image"}},
    "attr": {{
        "context": "app",
        "dockerfile": "docker/build.Dockerfile",
    }},
    "srcs": [],
    "build_dir": ".once/out/containers/demo/image",
    "scratch_dir": ".once/tmp/analysis/containers/demo/image",
    "capability": "build",
}}
result = repr(_dockerfile_image_impl(ctx))
"#,
        dockerfile_prelude_source()
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "containers/demo".to_string(),
        ".once/out/containers/demo/image".to_string(),
    );

    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    result.unwrap();
    let action = action_by_identifier(&store, "containers/demo/image:dockerfile-build");
    assert_eq!(
        action.inputs,
        vec![
            "containers/demo/app/.dockerignore".to_string(),
            "containers/demo/app/ignored/value.txt".to_string(),
            "containers/demo/docker/build.Dockerfile".to_string(),
            "containers/demo/docker/build.Dockerfile.dockerignore".to_string(),
        ]
    );
}

#[test]
fn dockerfile_image_exports_directly_with_a_container_builder() {
    let workspace = TempDir::new().unwrap();
    let package = workspace.path().join("containers/demo");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("Dockerfile"), "FROM scratch\n").unwrap();
    std::fs::write(package.join("Dockerfile.dockerignore"), "ignored\n").unwrap();
    let source = format!(
        r#"{}
def host_which_optional(name):
    return "/tools/docker" if name == "docker" else None

def host_command(argv, env = None, cwd = None, merge_stderr = None):
    if argv[1:3] == ["buildx", "version"]:
        return "github.com/docker/buildx v1.2.3\n"
    if argv[1:3] == ["buildx", "inspect"]:
        return "Driver: docker-container\nBuildKit version: v4.5.6\n"
    fail("unexpected command: " + repr(argv))

def host_env(name):
    return ""

ctx = {{
    "label": {{"package": "containers/demo", "name": "image", "id": "containers/demo/image"}},
    "attr": {{}},
    "srcs": ["Dockerfile"],
    "build_dir": ".once/out/containers/demo/image",
    "scratch_dir": ".once/tmp/analysis/containers/demo/image",
    "capability": "build",
}}
result = repr(_dockerfile_image_impl(ctx))
"#,
        dockerfile_prelude_source()
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "containers/demo".to_string(),
        ".once/out/containers/demo/image".to_string(),
    );

    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    result.unwrap();
    assert_eq!(store.actions.len(), 1);
    let action = &store.actions[0];
    assert_eq!(
        action.inputs,
        vec![
            "containers/demo/Dockerfile".to_string(),
            "containers/demo/Dockerfile.dockerignore".to_string(),
        ]
    );
    assert!(action
        .argv
        .iter()
        .any(|arg| arg.starts_with("type=docker,dest=")));
    assert_eq!(
        action.outputs,
        vec![
            ".once/out/containers/demo/image/image.docker.tar".to_string(),
            ".once/out/containers/demo/image/build-metadata.json".to_string(),
        ]
    );
}

#[test]
fn dockerfile_paths_keep_dot_directories_inside_the_package() {
    let source = format!(
        r#"{}
ctx = {{"label": {{"package": "containers/demo", "name": "image", "id": "containers/demo/image"}}}}
result = repr(_dockerfile_workspace_path(ctx, ".docker/Dockerfile"))
"#,
        dockerfile_prelude_source()
    );
    let result = eval_prelude_source_to_repr(source).unwrap();
    assert_eq!(result, "\"containers/demo/.docker/Dockerfile\"");
}

#[test]
fn dockerfile_platform_rejects_multiple_values() {
    let source = format!(
        "{}\nresult = repr(_dockerfile_platform(\"linux/amd64,linux/arm64\"))",
        dockerfile_prelude_source()
    );
    let error = eval_prelude_source_to_repr(source).unwrap_err();
    assert!(
        format!("{error:?}").contains("must name exactly one"),
        "{error:?}"
    );
}

#[test]
fn oci_layer_consumes_the_cross_language_executable_provider() {
    let workspace = TempDir::new().unwrap();
    std::fs::create_dir_all(workspace.path().join("bin")).unwrap();
    std::fs::create_dir_all(workspace.path().join("assets")).unwrap();
    std::fs::write(workspace.path().join("bin/hello"), "executable").unwrap();
    std::fs::write(workspace.path().join("assets/message.txt"), "hello").unwrap();
    let source = format!(
        r#"{}
ctx = {{
    "label": {{"package": "", "name": "hello_layer", "id": "hello_layer"}},
    "attr": {{}},
    "deps_by_role": {{
        "programs": [{{
            "label_id": "hello",
            "executable": {{
                "path": "bin/hello",
                "runtime_files": ["assets/message.txt"],
                "os": "macos",
                "architecture": "x86_64",
                "variant": "",
                "linkage": "static",
            }},
        }}],
    }},
    "srcs": [],
    "build_dir": ".once/out/hello_layer",
    "scratch_dir": ".once/tmp/analysis/hello_layer",
    "capability": "build",
}}
provider = _oci_layer_impl(ctx)
result = repr(provider)
"#,
        oci_prelude_source()
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        String::new(),
        ".once/out/hello_layer".to_string(),
    );

    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let provider = result.unwrap();
    assert!(
        provider.contains("\"architecture\": \"amd64\""),
        "{provider}"
    );
    assert!(provider.contains("\"os\": \"darwin\""), "{provider}");
    assert!(
        provider.contains("\"program_paths\": [\"/usr/local/bin/hello\"]"),
        "{provider}"
    );
    let action = action_by_identifier(&store, "hello_layer:oci-layer");
    assert_eq!(
        action.inputs,
        vec!["assets/message.txt".to_string(), "bin/hello".to_string()]
    );
    let Some(DeclaredActionOperation::WriteArchive {
        entries,
        output,
        sha256_output,
        format,
    }) = &action.operation
    else {
        panic!("expected a portable archive action");
    };
    assert_eq!(output, ".once/out/hello_layer/hello_layer.tar");
    assert_eq!(
        sha256_output.as_deref(),
        Some(".once/out/hello_layer/hello_layer.sha256")
    );
    assert_eq!(*format, DeclaredArchiveFormat::Tar);
    let archive_paths = entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        archive_paths,
        [
            "app",
            "usr",
            "usr/local",
            "usr/local/bin",
            "usr/local/bin/hello",
            "app/message.txt",
        ]
    );
    let runtime_file = entries
        .iter()
        .find(|entry| entry.path == "app/message.txt")
        .unwrap();
    assert_eq!(runtime_file.kind, DeclaredArchiveEntryKind::File);
    assert_eq!(runtime_file.source.as_deref(), Some("assets/message.txt"));
    assert_eq!(runtime_file.mode, 0o644);
}

#[test]
fn go_dependencies_lower_checksum_bound_vendor_modules() {
    let source = format!(
        r##"{}
ctx = {{
    "label": {{"package": "", "name": "GoDependencies", "id": "GoDependencies"}},
    "attrs": {{}},
    "files": {{
        "go.mod": "module example.com/app\n\ngo 1.26.0\n\nrequire github.com/pkg/errors v0.9.1\n",
        "go.sum": "github.com/pkg/errors v0.9.1 h1:checksum\ngithub.com/pkg/errors v0.9.1/go.mod h1:manifest\n",
        "vendor/modules.txt": "# github.com/pkg/errors v0.9.1\n## explicit\ngithub.com/pkg/errors\n",
    }},
}}
result = repr(_go_dependencies_resolver(ctx))
"##,
        go_prelude_source()
    );

    let out = eval_prelude_source_to_repr(source).unwrap();

    assert!(out.contains("go-module-github.com_x47_pkg_x47_errors_x64_v0.9.1"));
    assert!(out.contains("h1:checksum"));
    assert!(out.contains("vendor/github.com/pkg/errors/**/*"));
    assert!(out.contains("_go_resolved"));
}

#[test]
fn go_action_paths_remain_workspace_relative_for_nested_modules() {
    let source = format!(
        r#"{}
ctx = {{
    "label": {{"package": "apps/hello", "name": "Hello", "id": "apps/hello/Hello"}},
    "attr": {{}},
    "deps": [],
}}
module = {{"workspace_file": "apps/hello/go.work"}}
result = repr(_go_action_env(ctx, module, ".once/tmp/analysis/apps/hello/Hello/go-build"))
"#,
        go_prelude_source()
    );

    let out = eval_prelude_source_to_repr(source).unwrap();

    assert!(
        out.contains("{{once.execution_root}}/.once/tmp/analysis/apps/hello/Hello/go-build/cache")
    );
    assert!(out.contains("{{once.execution_root}}/apps/hello/go.work"));
    assert!(!out.contains("../"));
}

#[test]
fn go_dependencies_reject_vendor_replacement_drift() {
    let source = format!(
        r##"{}
ctx = {{
    "label": {{"package": "", "name": "GoDependencies", "id": "GoDependencies"}},
    "attrs": {{}},
    "files": {{
        "go.mod": "module example.com/app\n\ngo 1.26.0\n\nrequire example.com/old v1.0.0\nreplace example.com/old v1.0.0 => example.com/new v1.1.0\n",
        "go.sum": "example.com/new v1.2.0 h1:checksum\n",
        "vendor/modules.txt": "# example.com/old v1.0.0 => example.com/new v1.2.0\n## explicit\nexample.com/old/package\n",
    }},
}}
result = repr(_go_dependencies_resolver(ctx))
"##,
        go_prelude_source()
    );

    let error = eval_prelude_source_to_repr(source).unwrap_err().clone();

    assert!(error.contains("does not match go.mod"), "{error}");
}

#[cfg(unix)]
#[test]
fn go_binary_declares_offline_cross_platform_build_action() {
    let workspace = TempDir::new().unwrap();
    std::fs::create_dir_all(workspace.path().join("cmd/hello")).unwrap();
    std::fs::create_dir_all(workspace.path().join("vendor/example.com/message")).unwrap();
    std::fs::write(
        workspace.path().join("cmd/hello/main.go"),
        "package main\nfunc main() {}\n",
    )
    .unwrap();
    std::fs::write(
        workspace
            .path()
            .join("vendor/example.com/message/message.go"),
        "package message\n",
    )
    .unwrap();
    for (path, contents) in [
        ("go.mod", "module example.com/app\ngo 1.26.0\n"),
        ("go.sum", ""),
        ("vendor/modules.txt", ""),
    ] {
        std::fs::write(workspace.path().join(path), contents).unwrap();
    }
    let source = format!(
        r#"{}
def host_which(name):
    if name == "go":
        return "/toolchains/go/bin/go"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if len(argv) == 2 and argv[1] == "version":
        return "go version go1.26.5 darwin/arm64\n"
    if len(argv) >= 3 and argv[1] == "env":
        return "{{\"GOOS\":\"darwin\",\"GOARCH\":\"arm64\",\"GOROOT\":\"/toolchains/go\"}}"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{"package": "", "name": "Hello", "id": "Hello"}},
    "attr": {{"package": "./cmd/hello", "goos": "linux", "goarch": "amd64"}},
    "deps": [{{
        "go_dependency_set": True,
        "label_id": "GoDependencies",
        "module_root": "",
        "manifest": "go.mod",
        "sum_files": ["go.sum"],
        "vendor_manifest": "vendor/modules.txt",
        "vendor_dir": "vendor",
        "transitive_sources": ["go.mod", "go.sum", "vendor/modules.txt", "vendor/example.com/message/message.go"],
    }}],
    "srcs": ["cmd/hello/*.go"],
    "build_dir": ".once/out/Hello",
    "scratch_dir": ".once/tmp/analysis/Hello",
    "capability": "build",
}}
provider = _go_binary_impl(ctx)
result = repr(provider)
"#,
        go_prelude_source()
    );
    let store = store_for(workspace.path(), "");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    assert!(out.contains("go_binary"), "{out}");
    assert!(out.contains("\"once_executable\": True"), "{out}");
    assert!(out.contains("\"architecture\": \"amd64\""), "{out}");
    let action = action_by_identifier(&store, "Hello:go-build");
    assert!(action.argv.iter().any(|arg| arg == "-mod=vendor"));
    assert!(action.argv.iter().any(|arg| arg == "./cmd/hello"));
    assert_eq!(action.env.get("GOOS").map(String::as_str), Some("linux"));
    assert_eq!(action.env.get("GOARCH").map(String::as_str), Some("amd64"));
    assert_eq!(action.env.get("CGO_ENABLED").map(String::as_str), Some("0"));
    assert!(action
        .env
        .get("GOCACHE")
        .is_some_and(|value| value.starts_with("{{once.execution_root}}/")));
    assert!(action
        .inputs
        .iter()
        .any(|input| input == "vendor/example.com/message/message.go"));
    assert!(action
        .outputs
        .iter()
        .any(|output| output.ends_with("Hello")));
}

#[cfg(unix)]
#[test]
fn go_test_declares_normalized_runner_and_coverage_outputs() {
    let workspace = TempDir::new().unwrap();
    std::fs::create_dir_all(workspace.path().join("greeting")).unwrap();
    std::fs::write(
        workspace.path().join("greeting/greeting_test.go"),
        "package greeting\nimport \"testing\"\nfunc TestGreeting(t *testing.T) {}\n",
    )
    .unwrap();
    for (path, contents) in [
        ("go.mod", "module example.com/app\ngo 1.26.0\n"),
        ("go.sum", ""),
        ("vendor/modules.txt", ""),
    ] {
        std::fs::create_dir_all(workspace.path().join(path).parent().unwrap()).unwrap();
        std::fs::write(workspace.path().join(path), contents).unwrap();
    }
    let source = format!(
        r#"{}
def host_which(name):
    if name == "go":
        return "/toolchains/go/bin/go"
    fail("unexpected host_which: " + name)

def host_os():
    return "linux"

def host_arch():
    return "x86_64"

def host_command(argv, env = None, merge_stderr = None):
    if len(argv) == 2 and argv[1] == "version":
        return "go version go1.26.5 linux/amd64\n"
    if len(argv) >= 3 and argv[1] == "env":
        return "{{\"GOOS\":\"linux\",\"GOARCH\":\"amd64\",\"GOROOT\":\"/toolchains/go\"}}"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{"package": "", "name": "GreetingTests", "id": "GreetingTests"}},
    "attr": {{"package": "./greeting", "coverage": True, "labels": ["unit"]}},
    "deps": [],
    "srcs": ["greeting/*_test.go"],
    "build_dir": ".once/out/GreetingTests",
    "scratch_dir": ".once/tmp/analysis/GreetingTests",
    "capability": "test",
    "test": {{"filters": ["GreetingTests::TestGreeting"], "batch_id": "batch-1"}},
}}
provider = _go_test_impl(ctx)
result = repr(provider["test_info"])
"#,
        go_prelude_source()
    );
    let store = store_for(workspace.path(), "");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    assert!(out.contains("go_test"), "{out}");
    assert!(out.contains("unit"), "{out}");
    let build = action_by_identifier(&store, "GreetingTests:go-test-build");
    assert!(build.argv.iter().any(|arg| arg == "-c"));
    let run = action_by_identifier(&store, "GreetingTests:go-test");
    assert!(run.argv.iter().any(|arg| arg == "--once-filter"));
    assert!(run.argv.iter().any(|arg| arg == "TestGreeting"));
    assert!(run
        .outputs
        .iter()
        .any(|output| output.ends_with("test/batches/batch-1/test_results.json")));
    assert!(run
        .outputs
        .iter()
        .any(|output| output.ends_with("test/batches/batch-1/coverage.out")));
    assert!(store.actions.iter().any(|action| {
        matches!(
            &action.operation,
            Some(DeclaredActionOperation::WriteFile { path, .. })
                if path.ends_with("once_go_test_runner.go")
                    && !path.ends_with("test/once_go_test_runner.go")
        )
    }));
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
        dependency_edges: BTreeMap::new(),
        srcs: Vec::new(),
        visibility: Vec::new(),
        attrs: BTreeMap::new(),
        typed_attrs: BTreeMap::new(),
        resolver_input_exclude: Vec::new(),
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
fn apple_thinned_package_schema_exposes_device_model_and_example() {
    let schema =
        built_in_target_kind_schema("apple_thinned_package").expect("thinned package schema");

    let device_model = schema
        .attrs
        .iter()
        .find(|attr| attr.name == "device_model")
        .expect("device model attribute");
    assert!(device_model.required);
    assert!(!device_model.configurable);
    assert_eq!(device_model.disallowed_values, vec!["", "all"]);
    assert_eq!(schema.deps.len(), 1);
    assert_eq!(schema.deps[0].expected_providers, vec!["apple_application"]);
    assert_eq!(schema.deps[0].min_count, 1);
    assert_eq!(schema.deps[0].max_count, Some(1));
    assert!(schema
        .examples
        .iter()
        .any(|example| example.slug == "apple-thinned-package-minimal"));
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
    assert!(attr_names.contains(&"emulator"));
    assert!(attr_names.contains(&"emulator_device"));
    assert!(attr_names.contains(&"launch_activity"));
    assert!(attr_names.contains(&"kotlinc"));
    assert!(attr_names.contains(&"kotlin_home"));
    assert!(attr_names.contains(&"kotlin_stdlib"));
    assert!(!attr_names.contains(&"keytool"));
}

#[test]
fn android_target_kind_schemas_expose_all_target_kinds() {
    for kind in [
        "android_resource",
        "android_library",
        "android_local_test",
        "android_instrumentation_test",
        "android_binary",
    ] {
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
    assert!(attr_names.contains(&"javacopts"));

    let local_test = built_in_target_kind_schema("android_local_test").unwrap();
    assert!(local_test.providers.iter().any(|p| p == "once_test_info"));
    assert!(local_test
        .capabilities
        .iter()
        .any(|capability| capability.name == "test"));
    assert!(local_test.attrs.iter().any(|attr| attr.name == "classpath"));
    assert!(local_test.attrs.iter().any(|attr| attr.name == "env"));
    assert!(local_test
        .attrs
        .iter()
        .any(|attr| attr.name == "env_inherit"));
    assert!(local_test.attrs.iter().any(|attr| attr.name == "javacopts"));
    assert!(local_test
        .attrs
        .iter()
        .any(|attr| attr.name == "runtime_deps"));
    assert!(local_test.deps.iter().any(|dep| {
        dep.name == "runtime_deps"
            && dep
                .expected_providers
                .iter()
                .any(|provider| provider == "java_library")
    }));

    let instrumentation_test = built_in_target_kind_schema("android_instrumentation_test").unwrap();
    assert!(instrumentation_test
        .providers
        .iter()
        .any(|p| p == "once_test_info"));
    assert!(instrumentation_test
        .capabilities
        .iter()
        .any(|capability| capability.name == "test"));
    assert!(instrumentation_test
        .attrs
        .iter()
        .any(|attr| attr.name == "test_app"));
    assert!(instrumentation_test
        .attrs
        .iter()
        .any(|attr| attr.name == "env_inherit"));
    assert!(instrumentation_test
        .attrs
        .iter()
        .any(|attr| attr.name == "support_apks"));
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
    assert!(swift.attrs.iter().any(|attr| attr.name == "copts"));
    assert!(swift.attrs.iter().any(|attr| attr.name == "defines"));
    assert!(swift.attrs.iter().any(|attr| attr.name == "package_name"));
    assert!(swift.attrs.iter().any(|attr| attr.name == "cxx_runtime"));
    assert!(swift.attrs.iter().any(|attr| attr.name == "swiftc_inputs"));
    assert!(swift
        .attrs
        .iter()
        .any(|attr| attr.name == "library_evolution"));
    assert!(swift
        .source_references
        .iter()
        .any(|reference| reference.system == "SwiftJava"));

    let kotlin = built_in_target_kind_schema("kotlin_apple_framework")
        .expect("kotlin_apple_framework schema");
    assert!(target_kind_has_impl("kotlin_apple_framework").unwrap());
    assert!(kotlin.providers.iter().any(|p| p == "apple_framework"));
    assert!(kotlin.providers.iter().any(|p| p == "native_linkable"));
}

#[test]
fn kotlin_jvm_target_kind_schemas_are_discoverable() {
    let kotlin_jvm =
        built_in_target_kind_schema("kotlin_jvm_library").expect("kotlin_jvm_library schema");
    assert!(target_kind_has_impl("kotlin_jvm_library").unwrap());
    assert!(kotlin_jvm.providers.iter().any(|p| p == "java_library"));
    for role in [
        "deps",
        "associates",
        "exported_deps",
        "provided_deps",
        "runtime_deps",
    ] {
        assert!(kotlin_jvm.deps.iter().any(|dep| dep.name == role));
    }
    let kotlin_binary =
        built_in_target_kind_schema("kotlin_jvm_binary").expect("kotlin_jvm_binary schema");
    assert!(target_kind_has_impl("kotlin_jvm_binary").unwrap());
    assert!(kotlin_binary
        .capabilities
        .iter()
        .any(|capability| capability.name == "run"));
    let kotlin_test =
        built_in_target_kind_schema("kotlin_jvm_test").expect("kotlin_jvm_test schema");
    assert!(target_kind_has_impl("kotlin_jvm_test").unwrap());
    assert!(kotlin_test
        .providers
        .iter()
        .any(|provider| provider == "once_test_info"));
    assert!(kotlin_test
        .capabilities
        .iter()
        .any(|capability| capability.name == "test"));
}

#[test]
fn rust_cross_platform_target_kind_schemas_are_discoverable() {
    let rust = built_in_target_kind_schema("rust_library").expect("rust_library schema");
    assert!(rust.providers.iter().any(|p| p == "apple_linkable"));
    assert!(rust.providers.iter().any(|p| p == "android_native_library"));
    assert!(rust.attrs.iter().any(|attr| attr.name == "android_abi"));
    assert!(rust.attrs.iter().any(|attr| attr.name == "native_linkopts"));
    assert!(rust.attrs.iter().any(|attr| attr.name == "aliases"));
    assert!(rust.attrs.iter().any(|attr| attr.name == "named_deps"));
    assert!(rust.attrs.iter().any(|attr| attr.name == "compile_data"));
    assert!(rust.attrs.iter().any(|attr| attr.name == "rustc_env_files"));
    assert!(rust
        .attrs
        .iter()
        .any(|attr| attr.name == "exported_linker_flags"));
    assert!(rust
        .attrs
        .iter()
        .any(|attr| attr.name == "exported_post_linker_flags"));
    assert!(rust.attrs.iter().any(|attr| attr.name == "linker_script"));
    assert!(rust.deps.iter().any(|dep| {
        dep.name == "deps"
            && dep
                .expected_providers
                .iter()
                .any(|provider| provider == "c_provider")
    }));
    assert!(rust.deps.iter().any(|dep| {
        dep.name == "proc_macro_deps" && dep.expected_providers == vec!["rust_proc_macro"]
    }));
    assert!(rust
        .deps
        .iter()
        .any(|dep| { dep.name == "link_deps" && dep.expected_providers == vec!["c_provider"] }));

    let rust_test = built_in_target_kind_schema("rust_test").expect("rust_test schema");
    assert!(target_kind_has_impl("rust_test").unwrap());
    assert!(rust_test.providers.iter().any(|p| p == "once_test_info"));
    assert!(rust_test
        .capabilities
        .iter()
        .any(|capability| capability.name == "test"));
    assert!(rust_test
        .attrs
        .iter()
        .any(|attr| attr.name == "env_inherit"));
    assert!(rust_test
        .attrs
        .iter()
        .any(|attr| attr.name == "use_libtest_harness"));

    for kind in ["rust_crate", "rust_proc_macro"] {
        let schema = built_in_target_kind_schema(kind).expect("Cargo-generated Rust schema");
        assert!(schema.deps.iter().any(|dep| {
            dep.name == "build_deps"
                && dep
                    .expected_providers
                    .iter()
                    .any(|provider| provider == "rust_crate")
        }));
    }

    let cargo_dependencies =
        built_in_target_kind_schema("cargo_dependencies").expect("cargo_dependencies schema");
    assert!(cargo_dependencies.deps.iter().any(|dep| {
        dep.name == "deps"
            && dep
                .expected_providers
                .iter()
                .any(|provider| provider == "rust_crate")
    }));
    let rust_mobile =
        built_in_target_kind_schema("rust_mobile_library").expect("rust_mobile_library schema");
    assert!(target_kind_has_impl("rust_mobile_library").unwrap());
    assert!(rust_mobile.providers.iter().any(|p| p == "apple_linkable"));
    assert!(rust_mobile
        .providers
        .iter()
        .any(|p| p == "android_native_library"));
    assert!(rust_mobile.providers.iter().any(|p| p == "native_linkable"));
    assert!(rust_mobile
        .providers
        .iter()
        .any(|p| p == "rust_mobile_crate"));
    assert!(!rust_mobile.providers.iter().any(|p| p == "rust_crate"));
    assert!(rust_mobile.deps.iter().any(|dep| {
        dep.name == "deps"
            && dep
                .expected_providers
                .iter()
                .any(|provider| provider == "rust_mobile_crate")
    }));
    assert!(rust_mobile
        .attrs
        .iter()
        .any(|attr| attr.name == "apple_target" && attr.required));
    assert!(rust_mobile
        .attrs
        .iter()
        .any(|attr| attr.name == "android_target" && attr.required));
    assert!(rust_mobile
        .attrs
        .iter()
        .any(|attr| attr.name == "compile_data"));
}

#[test]
fn rust_and_elixir_runtime_schemas_are_discoverable() {
    let rust_binary = built_in_target_kind_schema("rust_binary").expect("rust_binary schema");
    assert!(rust_binary.attrs.iter().any(|attr| attr.name == "args"));
    assert!(rust_binary.attrs.iter().any(|attr| attr.name == "run_env"));
    let rust_run = rust_binary
        .capabilities
        .iter()
        .find(|capability| capability.name == "run")
        .expect("rust_binary run capability");
    assert_eq!(rust_run.requires_outputs, vec!["binary"]);

    let elixir = built_in_target_kind_schema("elixir_library").expect("elixir_library schema");
    assert!(target_kind_has_impl("elixir_library").unwrap());
    assert_eq!(elixir.tools.len(), 2);
    assert_eq!(elixir.tools[0].name, "elixir");
    assert_eq!(elixir.tools[0].executables, ["elixir", "elixirc", "mix"]);
    assert_eq!(elixir.tools[1].name, "erlang");
    assert_eq!(elixir.tools[1].executables, ["erl"]);
    assert!(elixir.attrs.iter().any(|attr| attr.name == "elixirc_opts"));
    assert!(elixir.attrs.iter().any(|attr| attr.name == "extra_apps"));
    assert!(elixir
        .attrs
        .iter()
        .any(|attr| attr.name == "app_description"));
    assert!(elixir.attrs.iter().any(|attr| attr.name == "resources"));

    let elixir_test = built_in_target_kind_schema("elixir_test").expect("elixir_test schema");
    assert!(target_kind_has_impl("elixir_test").unwrap());
    assert!(elixir_test.attrs.iter().any(|attr| attr.name == "setup"));
    assert!(elixir_test
        .attrs
        .iter()
        .any(|attr| attr.name == "elixir_opts"));
    assert!(elixir_test
        .attrs
        .iter()
        .any(|attr| attr.name == "env_inherit"));
    assert!(elixir_test.attrs.iter().any(|attr| attr.name == "tools"));

    let mix_package = built_in_target_kind_schema("mix_package").expect("mix_package schema");
    assert_eq!(mix_package.tools, elixir.tools);
}

#[test]
fn mix_dependency_target_kinds_are_discoverable() {
    let dependencies =
        built_in_target_kind_schema("mix_dependencies").expect("mix_dependencies schema");
    assert!(target_kind_has_impl("mix_dependencies").unwrap());
    assert!(dependencies
        .providers
        .iter()
        .any(|provider| provider == "mix_dependency_set"));
    for name in [
        "manifest",
        "lockfile",
        "resolver_inputs",
        "graph_file",
        "vendor_dir",
        "path_dependencies",
        "mix_env",
        "dependency_mix_env",
        "config",
        "config_entry",
        "config_target",
        "roots",
        "_mix_resolved",
        "_mix_locked_roots",
    ] {
        assert!(dependencies.attrs.iter().any(|attr| attr.name == name));
    }

    let package = built_in_target_kind_schema("mix_package").expect("mix_package schema");
    assert!(target_kind_has_impl("mix_package").unwrap());
    assert!(package
        .providers
        .iter()
        .any(|provider| provider == "elixir_app"));
    for name in [
        "_mix_locked",
        "_mix_package_name",
        "_mix_source",
        "_mix_checksum",
        "_mix_outer_checksum",
        "_mix_revision",
        "_mix_managers",
        "_mix_custom_compile",
        "_mix_root_config_entry",
        "_mix_root_config_env",
        "_mix_root_config_target",
    ] {
        assert!(package.attrs.iter().any(|attr| attr.name == name));
    }

    let project = built_in_target_kind_schema("mix_project").expect("mix_project schema");
    assert!(target_kind_has_impl("mix_project").unwrap());
    assert!(project
        .providers
        .iter()
        .any(|provider| provider == "elixir_app"));
    assert!(project
        .providers
        .iter()
        .any(|provider| provider == "mix_project"));
    assert!(project.attrs.iter().any(|attr| attr.name == "run_tasks"));
    assert!(project
        .attrs
        .iter()
        .any(|attr| attr.name == "run_cacheable"));
    assert!(project
        .capabilities
        .iter()
        .any(|capability| capability.name == "run"));

    let release = built_in_target_kind_schema("mix_release").expect("mix_release schema");
    assert!(target_kind_has_impl("mix_release").unwrap());
    assert!(release
        .providers
        .iter()
        .any(|provider| provider == "mix_release"));
    assert!(release.attrs.iter().any(|attr| attr.name == "pre_tasks"));
    assert!(release
        .capabilities
        .iter()
        .any(|capability| capability.name == "build"));

    let source = format!(
        "{}\nresult = repr(mix_dependencies.get(\"resolver\") != None)\n",
        all_prelude_source()
    );
    assert_eq!(eval_prelude_source_to_repr(source).unwrap(), "True");
}

#[test]
fn prelude_mix_dependencies_expands_native_locked_graph() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("prelude/examples/elixir-library-with-mix-dependency");
    let graph = once_frontend::load_graph_workspace(&root).expect("Mix example graph loads");
    let owner = graph
        .iter()
        .find(|target| target.label.id == "mix_dependencies")
        .expect("mix_dependencies owner");
    let package = graph
        .iter()
        .find(|target| target.label.id == "mix-locked-greeting")
        .expect("synthetic Mix package");

    assert_eq!(owner.deps, vec!["local_helper", "mix-locked-greeting"]);
    assert_eq!(package.kind, "mix_package");
    assert_eq!(
        package.attrs.get("version"),
        Some(&once_frontend::AttrValue::String("0.1.0".to_string()))
    );
    assert_eq!(
        package.attrs.get("_mix_source"),
        Some(&once_frontend::AttrValue::String(
            "hex:hexpm/locked_greeting".to_string()
        ))
    );
    assert_eq!(
        package.attrs.get("_mix_checksum"),
        Some(&once_frontend::AttrValue::String("0".repeat(64)))
    );
    assert_eq!(
        package.attrs.get("_mix_outer_checksum"),
        Some(&once_frontend::AttrValue::String("1".repeat(64)))
    );
}

#[test]
fn prelude_mix_dependencies_rejects_stale_snapshots_and_derives_path_packages() {
    let prelude = all_prelude_source();
    let stale_graph =
        serde_json::json!({"manifest": "", "lockfile": "old\n", "mix_env": "prod"}).to_string();
    let stale_source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "deps", "id": "deps"}},
    "attrs": {{"graph_file": "graph.json"}},
    "files": {{
        "mix.exs": "",
        "mix.lock": "fresh\n",
        "graph.json": {stale_graph:?},
    }},
}}
result = repr(_mix_read_locked_graph(ctx))
"#
    );
    let stale_error = eval_prelude_source_to_repr(stale_source).unwrap_err();
    assert!(
        stale_error.contains("is stale relative to"),
        "{stale_error}"
    );

    let environment_graph = serde_json::json!({
        "manifest": "",
        "lockfile": "fresh\n",
        "mix_env": "prod",
        "lock": {},
        "dependencies": [],
    })
    .to_string();
    let environment_source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "deps", "id": "deps"}},
    "attrs": {{"graph_file": "graph.json", "mix_env": "test"}},
    "files": {{
        "mix.exs": "",
        "mix.lock": "fresh\n",
        "graph.json": {environment_graph:?},
    }},
}}
result = repr(_mix_read_locked_graph(ctx))
"#
    );
    let environment_error = eval_prelude_source_to_repr(environment_source).unwrap_err();
    assert!(
        environment_error.contains("MIX_ENV=prod") && environment_error.contains("MIX_ENV=test"),
        "{environment_error}"
    );

    let path_graph = serde_json::json!({
        "manifest": "",
        "lockfile": "fresh\n",
        "mix_env": "prod",
        "once_inputs": {"mix.exs": "", "mix.lock": "fresh\n"},
        "lock": {},
        "dependencies": [{
            "app": "local_helper",
            "dependencies": [],
            "destination": "local_helper",
            "manager": "Mix.SCM.Path",
            "path_dependency": true,
            "top_level": true,
        }],
    })
    .to_string();
    let path_source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "deps", "id": "deps"}},
    "attrs": {{"graph_file": "graph.json"}},
    "files": {{
        "mix.exs": "",
        "mix.lock": "fresh\n",
        "graph.json": {path_graph:?},
    }},
}}
result = repr(_mix_dependencies_resolver(ctx))
"#
    );
    let path_output = eval_prelude_source_to_repr(path_source).unwrap();
    assert!(
        path_output.contains("\"_mix_local\": True"),
        "{path_output}"
    );
    assert!(
        path_output.contains("\"_mix_source_root\": \"local_helper\""),
        "{path_output}"
    );
}

#[test]
fn mix_path_dependencies_accept_absolute_sources_inside_the_workspace() {
    let workspace = TempDir::new().unwrap();
    std::fs::create_dir_all(workspace.path().join("server")).unwrap();
    std::fs::create_dir_all(workspace.path().join("noora")).unwrap();
    let source_root = workspace
        .path()
        .join("noora")
        .to_string_lossy()
        .into_owned();
    let source = format!(
        "{}\nresult = repr(_elixir_workspace_source_root(\"server\", {source_root:?}))\n",
        all_prelude_source()
    );
    let store = store_for(workspace.path(), "server");

    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"noora\"");
}

#[test]
fn prelude_mix_snapshot_binds_every_resolver_input() {
    let prelude = all_prelude_source();
    let graph = serde_json::json!({
        "manifest": "",
        "lockfile": "fresh\n",
        "mix_env": "prod",
        "once_inputs": {
            "mix.exs": "",
            "mix.lock": "fresh\n",
            "local_helper/mix.exs": "old local manifest\n",
        },
        "lock": {},
        "dependencies": [],
    })
    .to_string();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "deps", "id": "deps"}},
    "attrs": {{"graph_file": "graph.json"}},
    "files": {{
        "mix.exs": "",
        "mix.lock": "fresh\n",
        "local_helper/mix.exs": "new local manifest\n",
        "graph.json": {graph:?},
    }},
}}
result = repr(_mix_read_locked_graph(ctx))
"#
    );

    let error = eval_prelude_source_to_repr(source).unwrap_err();

    assert!(
        error.contains("input binding does not match resolver_inputs"),
        "{error}"
    );
}

#[test]
fn prelude_mix_dependencies_rejects_missing_child_targets() {
    let prelude = all_prelude_source();
    let graph = serde_json::json!({
        "manifest": "",
        "lockfile": "fresh\n",
        "mix_env": "prod",
        "once_inputs": {"mix.exs": "", "mix.lock": "fresh\n"},
        "lock": {
            "parent": [
                "hex",
                "parent",
                "1.0.0",
                "0".repeat(64),
                ["mix"],
                [],
                "hexpm",
                "1".repeat(64),
            ],
        },
        "dependencies": [{
            "app": "parent",
            "dependencies": ["missing_child"],
            "path_dependency": false,
            "top_level": true,
        }],
    })
    .to_string();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "deps", "id": "deps"}},
    "attrs": {{"graph_file": "graph.json"}},
    "files": {{
        "mix.exs": "",
        "mix.lock": "fresh\n",
        "graph.json": {graph:?},
    }},
}}
result = repr(_mix_dependencies_resolver(ctx))
"#
    );
    let error = eval_prelude_source_to_repr(source).unwrap_err();

    assert!(error.contains("active graph contains no target"), "{error}");
}

#[test]
fn prelude_mix_dependencies_omit_packages_with_compile_disabled() {
    let prelude = all_prelude_source();
    let graph = serde_json::json!({
        "manifest": "",
        "lockfile": "fresh\n",
        "mix_env": "prod",
        "once_inputs": {"mix.exs": "", "mix.lock": "fresh\n"},
        "lock": {
            "assets": [
                "git",
                "https://example.com/assets.git",
                "abc123",
            ],
        },
        "dependencies": [{
            "app": "assets",
            "dependencies": [],
            "path_dependency": false,
            "skip_compile": true,
            "top_level": true,
        }],
    })
    .to_string();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "deps", "id": "deps"}},
    "attrs": {{"graph_file": "graph.json"}},
    "files": {{
        "mix.exs": "",
        "mix.lock": "fresh\n",
        "graph.json": {graph:?},
    }},
}}
result = repr(_mix_dependencies_resolver(ctx))
"#
    );

    let out = eval_prelude_source_to_repr(source).unwrap();

    assert!(out.contains("\"targets\": []"), "{out}");
    assert!(out.contains("\"roots\": []"), "{out}");
}

#[test]
fn prelude_mix_dependencies_aggregates_locked_package_providers() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "deps", "id": "deps"}},
    "attr": {{
        "_mix_resolved": True,
        "_mix_locked_roots": ["mix-decimal"],
    }},
    "deps": [{{
        "locked": True,
        "app_name": "decimal",
        "package_name": "decimal",
        "version": "2.1.1",
        "source": "hex:hexpm/decimal",
        "checksum": "abc123",
        "outer_checksum": "def456",
        "revision": "",
        "transitive_sources": ["deps/decimal/lib/decimal.ex"],
        "transitive_elixir_apps": [{{
            "app_name": "decimal",
            "ebin_dir": ".once/out/mix-decimal/ebin",
        }}],
    }}],
}}
result = repr(_mix_dependencies_impl(ctx))
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert!(out.contains("\"dependency_set\": True"), "{out}");
    assert!(out.contains("\"locked\": True"), "{out}");
    assert!(out.contains("hex:hexpm/decimal"), "{out}");
    assert!(out.contains(".once/out/mix-decimal/ebin"), "{out}");
}

#[test]
fn prelude_mix_package_selects_supported_build_managers() {
    let prelude = all_prelude_source();
    for (attrs, expected) in [
        (r#"{"_mix_managers": ["mix"]}"#, "\"mix\""),
        (r#"{"_mix_managers": ["rebar3"]}"#, "\"rebar3\""),
        (r#"{"_mix_managers": ["make", "mix"]}"#, "\"mix\""),
    ] {
        let source = format!(
            r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "package", "id": "package"}},
    "attr": {attrs},
}}
result = repr(_mix_supported_manager(ctx))
"#
        );
        assert_eq!(eval_prelude_source_to_repr(source).unwrap(), expected);
    }

    let custom = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "package", "id": "package"}},
    "attr": {{"_mix_managers": ["mix"], "_mix_custom_compile": True}},
}}
result = repr(_mix_supported_manager(ctx))
"#
    );
    let error = eval_prelude_source_to_repr(custom).unwrap_err();
    assert!(
        error.contains("custom Mix dependency compile commands"),
        "{error}"
    );
}

#[test]
fn prelude_mix_package_uses_native_mix_compiler_pipeline() {
    let source = format!("{}\nresult = _mix_compile_source()\n", all_prelude_source());
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert!(out.contains("Mix.Task.run(\"compile.all\""), "{out}");
    assert!(out.contains("--no-prune-code-paths"), "{out}");
    assert!(!out.contains("deps.get"), "{out}");
}

#[test]
fn prelude_elixir_test_paths_only_select_executable_test_files() {
    let source = format!(
        r#"{}
ctx = {{"label": {{"package": "server", "name": "tests", "id": "server/tests"}}}}
result = repr(_elixir_test_paths(ctx, [
    "server/test/test_helper.exs",
    "server/test/example_test.exs",
    "server/test/support/helper.ex",
    "server/test/AGENTS.md",
]))
"#,
        all_prelude_source()
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(out, "[\"test/test_helper.exs\", \"test/example_test.exs\"]");
}

#[test]
fn prelude_elixir_empty_paths_stay_empty_across_package_boundaries() {
    let source = format!(
        r#"{}
ctx = {{"label": {{"package": "apps/web", "name": "app", "id": "apps/web/app"}}}}
result = repr(_elixir_from_package(ctx, ""))
"#,
        all_prelude_source()
    );

    assert_eq!(eval_prelude_source_to_repr(source).unwrap(), "\"\"");
}

#[test]
fn prelude_mix_project_accepts_a_runtime_task_without_manifest_coupling() {
    let source = format!(
        r#"{}
ctx = {{
    "label": {{"package": "server", "name": "application_dev", "id": "server/application_dev"}},
    "attr": {{}},
    "run": {{"args": ["phx.server", "--port", "4001"]}},
}}
result = repr(_mix_project_run_spec(ctx))
"#,
        all_prelude_source()
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert!(out.contains("\"task\": \"phx.server\""), "{out}");
    assert!(out.contains("\"args\": [\"--port\", \"4001\"]"), "{out}");

    let invalid_source = format!(
        r#"{}
ctx = {{
    "label": {{"package": "server", "name": "application_dev", "id": "server/application_dev"}},
    "attr": {{}},
    "run": {{"args": ["--port", "4001"]}},
}}
result = repr(_mix_project_run_spec(ctx))
"#,
        all_prelude_source()
    );
    let error = eval_prelude_source_to_repr(invalid_source).unwrap_err();
    assert!(
        error.contains("expected a Mix task as the first argument"),
        "{error}"
    );
}

#[test]
fn prelude_uncacheable_mix_runs_can_find_host_runtime_tools() {
    let workspace = TempDir::new().unwrap();
    let source = format!(
        r#"{}
toolchain = {{"path": "/once/toolchain"}}
uncached = _elixir_run_action_env_with({{
    "label": {{"id": "uncached"}},
    "attr": {{"env": {{"EXPLICIT": "present"}}}},
}}, toolchain, {{"HOME": "/once/scratch"}})
cached = _elixir_run_action_env_with({{
    "label": {{"id": "cached"}},
    "attr": {{
        "run_cacheable": True,
        "env": {{"HOME": "/configured/home"}},
    }},
}}, toolchain, {{"HOME": "/once/scratch"}})
result = repr([
    uncached["PATH"] != "/once/toolchain",
    uncached["HOME"] != "/once/scratch",
    uncached["EXPLICIT"],
    cached["PATH"],
    cached["HOME"],
])
"#,
        all_prelude_source()
    );
    let store = store_for(workspace.path(), "");
    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(
        out.unwrap(),
        "[True, True, \"present\", \"/once/toolchain\", \"/configured/home\"]"
    );
}

#[test]
fn dynamic_language_test_schemas_are_discoverable() {
    for kind in [
        "pytest_test",
        "rspec_test",
        "minitest_test",
        "vitest_test",
        "jest_test",
    ] {
        let schema = built_in_target_kind_schema(kind)
            .unwrap_or_else(|| panic!("missing target kind schema `{kind}`"));
        assert!(target_kind_has_impl(kind).unwrap());
        assert!(schema
            .providers
            .iter()
            .any(|provider| provider == "once_test_info"));
        assert!(schema
            .capabilities
            .iter()
            .any(|capability| capability.name == "test"));
        assert!(schema.attrs.iter().any(|attr| attr.name == "batching"));
        assert!(schema.attrs.iter().any(|attr| attr.name == "env_inherit"));
        assert_eq!(schema.examples.len(), 1);
    }

    for (kind, expected) in [
        ("vitest_test", "\"node_modules/vitest/vitest.mjs\""),
        ("jest_test", "\"node_modules/jest/bin/jest.js\""),
    ] {
        let schema = built_in_target_kind_schema(kind).unwrap();
        let runner = schema
            .attrs
            .iter()
            .find(|attr| attr.name == "runner")
            .unwrap();
        assert_eq!(runner.default.as_deref(), Some(expected));
    }

    for (kind, executable) in [
        ("pytest_test", "pytest"),
        ("rspec_test", "rspec"),
        ("vitest_test", "vitest"),
        ("jest_test", "jest"),
    ] {
        let schema = built_in_target_kind_schema(kind).unwrap();
        assert!(
            schema
                .tools
                .iter()
                .any(|tool| tool.executables.iter().any(|value| value == executable)),
            "{kind} should declare its {executable} runner"
        );
    }
}

#[test]
fn prelude_javascript_runner_uses_the_package_entry_behind_a_bin_shim() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let bin_dir = workspace.path().join("node_modules/.bin");
    let entry_dir = workspace.path().join("node_modules/jest/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::create_dir_all(&entry_dir).unwrap();
    let shim = bin_dir.join("jest");
    let entry = entry_dir.join("jest.js");
    std::fs::write(&shim, "package-manager shim\n").unwrap();
    std::fs::write(&entry, "console.log('jest')\n").unwrap();
    let source = format!(
        r#"{prelude}
result = repr(_javascript_installed_package_entry(
    "{}",
    "node_modules/jest/bin/jest.js",
))
"#,
        shim.to_string_lossy()
    );
    let store = store_for(workspace.path(), "");

    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), format!("\"{}\"", entry.to_string_lossy()));
}

#[test]
fn rust_schemas_cover_upstream_parity_fields() {
    for kind in [
        "rust_library",
        "rust_binary",
        "rust_test",
        "rust_crate",
        "rust_proc_macro",
    ] {
        assert_target_kind_attrs(
            kind,
            &[
                "crate_root",
                "edition",
                "crate_features",
                "data",
                "compile_data",
                "aliases",
                "named_deps",
                "rustc_env",
                "rustc_env_files",
                "rustc_flags",
            ],
        );
    }
}

#[test]
fn zig_schemas_cover_upstream_parity_fields() {
    assert_target_kind_attrs(
        "zig_library",
        &[
            "main",
            "import_name",
            "import_names",
            "extra_srcs",
            "data",
            "zigopts",
        ],
    );
    for kind in [
        "zig_binary",
        "zig_static_library",
        "zig_shared_library",
        "zig_test",
    ] {
        assert_target_kind_attrs(
            kind,
            &[
                "main",
                "data",
                "copts",
                "csrcs",
                "extra_docs",
                "extra_srcs",
                "import_names",
                "linker_script",
                "linkopts",
                "strip_debug_symbols",
                "zigopts",
            ],
        );
    }
    assert_target_kind_attrs("zig_c_library", &["data", "import_name"]);
    for kind in [
        "zig_configure",
        "zig_configure_binary",
        "zig_configure_test",
    ] {
        assert_target_kind_attrs(
            kind,
            &[
                "bootstrapped",
                "host_mode",
                "host_threaded",
                "host_use_cc_common_link",
                "host_zigopt",
                "mode",
                "target",
                "threaded",
                "use_cc_common_link",
                "use_standalone_translate_c",
                "zig_version",
                "zigopt",
            ],
        );
    }
}

#[test]
fn elixir_and_swift_schemas_cover_upstream_parity_fields() {
    assert_target_kind_attrs(
        "elixir_library",
        &[
            "app_name",
            "app_description",
            "extra_apps",
            "elixirc_opts",
            "resources",
            "docs",
            "os_env",
        ],
    );
    assert_target_kind_attrs(
        "elixir_test",
        &["data", "env", "setup", "elixir_opts", "tools"],
    );
    assert_target_kind_attrs(
        "swift_android_library",
        &[
            "copts",
            "defines",
            "data",
            "library_evolution",
            "linkopts",
            "module_name",
            "package_name",
            "swiftc_inputs",
        ],
    );
}

#[test]
fn android_schemas_cover_upstream_parity_fields() {
    assert_target_kind_attrs(
        "android_library",
        &[
            "assets",
            "assets_dir",
            "custom_package",
            "javacopts",
            "manifest",
            "neverlink",
            "resource_files",
        ],
    );
    assert_target_kind_attrs(
        "android_local_test",
        &["args", "env", "javacopts", "jvm_flags", "test_class"],
    );
    assert_target_kind_attrs(
        "android_instrumentation_test",
        &["args", "support_apks", "test_app", "test_class"],
    );
}

#[test]
fn c_and_zig_target_kind_schemas_are_discoverable() {
    let zig_library = built_in_target_kind_schema("zig_library").expect("zig_library schema");
    assert!(target_kind_has_impl("zig_library").unwrap());
    assert!(zig_library.providers.iter().any(|p| p == "zig_module"));
    assert!(zig_library
        .attrs
        .iter()
        .any(|attr| attr.name == "main" && attr.required));

    let c_library = built_in_target_kind_schema("c_library").expect("c_library schema");
    assert!(target_kind_has_impl("c_library").unwrap());
    assert!(c_library.providers.iter().any(|p| p == "c_provider"));
    assert!(c_library
        .attrs
        .iter()
        .any(|attr| attr.name == "archiver_identity"));

    let zig_binary = built_in_target_kind_schema("zig_binary").expect("zig_binary schema");
    assert!(target_kind_has_impl("zig_binary").unwrap());
    assert!(zig_binary.providers.iter().any(|p| p == "zig_binary"));
    assert!(zig_binary.attrs.iter().any(|attr| attr.name == "mode"));
    assert!(zig_binary.attrs.iter().any(|attr| attr.name == "threaded"));
    assert!(zig_binary.attrs.iter().any(|attr| attr.name == "zigopt"));
    assert!(zig_binary
        .attrs
        .iter()
        .any(|attr| attr.name == "zig_version"));
    assert!(zig_binary
        .attrs
        .iter()
        .any(|attr| attr.name == "use_cc_common_link"));
    assert!(zig_binary.attrs.iter().any(|attr| attr.name == "copts"));
    assert!(zig_binary
        .attrs
        .iter()
        .any(|attr| attr.name == "extra_docs"));
    assert!(zig_binary.attrs.iter().any(|attr| attr.name == "emit_asm"));
    assert!(zig_binary
        .attrs
        .iter()
        .any(|attr| attr.name == "use_standalone_translate_c"));
    assert!(zig_binary
        .attrs
        .iter()
        .any(|attr| attr.name == "translate_c_identity"));
    assert!(zig_binary
        .capabilities
        .iter()
        .any(|capability| capability.name == "build"));
    let zig_run = zig_binary
        .capabilities
        .iter()
        .find(|capability| capability.name == "run")
        .expect("zig_binary run capability");
    assert_eq!(zig_run.requires_outputs, vec!["binary"]);

    let zig_c_library = built_in_target_kind_schema("zig_c_library").expect("zig_c_library schema");
    assert!(target_kind_has_impl("zig_c_library").unwrap());
    assert!(zig_c_library.providers.iter().any(|p| p == "zig_module"));

    let zig_static =
        built_in_target_kind_schema("zig_static_library").expect("zig_static_library schema");
    assert!(target_kind_has_impl("zig_static_library").unwrap());
    assert!(zig_static.providers.iter().any(|p| p == "c_provider"));
    assert!(zig_static.providers.iter().any(|p| p == "apple_linkable"));

    let zig_shared =
        built_in_target_kind_schema("zig_shared_library").expect("zig_shared_library schema");
    assert!(target_kind_has_impl("zig_shared_library").unwrap());
    assert!(zig_shared.providers.iter().any(|p| p == "c_provider"));
    assert!(zig_shared
        .providers
        .iter()
        .any(|p| p == "android_native_library"));

    let zig_test = built_in_target_kind_schema("zig_test").expect("zig_test schema");
    assert!(target_kind_has_impl("zig_test").unwrap());
    assert!(zig_test.providers.iter().any(|p| p == "once_test_info"));
    assert!(zig_test
        .capabilities
        .iter()
        .any(|capability| capability.name == "test"));
}

#[test]
fn zig_dependency_target_kinds_are_discoverable() {
    let dependencies =
        built_in_target_kind_schema("zig_dependencies").expect("zig_dependencies schema");
    assert!(target_kind_has_impl("zig_dependencies").unwrap());
    assert!(dependencies
        .providers
        .iter()
        .any(|provider| provider == "zig_dependency_set"));
    for name in [
        "manifest",
        "resolver_inputs",
        "vendor_dir",
        "package_paths",
        "module_paths",
        "_root_packages",
    ] {
        assert!(dependencies.attrs.iter().any(|attr| attr.name == name));
    }

    let package = built_in_target_kind_schema("zig_package").expect("zig_package schema");
    assert!(target_kind_has_impl("zig_package").unwrap());
    assert!(package
        .providers
        .iter()
        .any(|provider| provider == "zig_module"));
    for name in [
        "package_name",
        "package_version",
        "package_fingerprint",
        "source_root",
        "source_url",
        "source_hash",
        "source_path",
    ] {
        assert!(package.attrs.iter().any(|attr| attr.name == name));
    }

    let source = format!(
        "{}\nresult = repr(zig_dependencies.get(\"resolver\") != None)\n",
        all_prelude_source()
    );
    assert_eq!(eval_prelude_source_to_repr(source).unwrap(), "True");
}

#[test]
fn prelude_zig_path_identity_uses_the_materialized_source_root() {
    let prelude = all_prelude_source();
    let first = eval_prelude_string_function_in(
        &prelude,
        "_zig_package_identity",
        r#"("support", {"path": "../support"}, "packages/a/support")"#,
    )
    .unwrap();
    let second = eval_prelude_string_function_in(
        &prelude,
        "_zig_package_identity",
        r#"("support", {"path": "../support"}, "packages/b/support")"#,
    )
    .unwrap();

    assert_eq!(first, "packages/a/support");
    assert_eq!(second, "packages/b/support");
    assert_ne!(first, second);
}

#[test]
fn prelude_zig_dependencies_expands_and_preserves_locked_hashes() {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("prelude/examples/zig-binary-with-package");
    let graph = once_frontend::load_graph_workspace(&root).expect("Zig package example loads");
    let owner = graph
        .iter()
        .find(|target| target.label.id == "zig_dependencies")
        .expect("zig_dependencies owner");
    let package = graph
        .iter()
        .find(|target| {
            target.kind == "zig_package"
                && target.attrs.get("package_name")
                    == Some(&once_frontend::AttrValue::String("math".to_string()))
        })
        .expect("synthetic Zig package");

    assert_eq!(owner.deps, vec![package.label.id.clone()]);
    assert_eq!(
        graph
            .iter()
            .filter(|target| target.kind == "zig_package")
            .count(),
        2
    );
    assert!(package
        .srcs
        .contains(&"third_party/zig/math/**/*".to_string()));
    assert_eq!(
        package.attrs.get("package_version"),
        Some(&once_frontend::AttrValue::String("1.0.0".to_string()))
    );
    assert_eq!(
        package.attrs.get("source_root"),
        Some(&once_frontend::AttrValue::String(
            "third_party/zig/math".to_string()
        ))
    );
    assert_eq!(
        package.attrs.get("source_hash"),
        Some(&once_frontend::AttrValue::String(
            "math-1.0.0-N3AjZe8AAAA_BHML9VFPGVQiHPOsQpQ9CpiaVtRYkACI".to_string()
        ))
    );
}

#[test]
fn prelude_elixir_parity_alias_helpers_merge_values() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_env(name):
    if name == "ELIXIR_HOST_FLAG":
        return "host-value"
    return ""

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "app",
        "id": "pkg/app",
    }},
    "attr": {{
        "app_description": "fallback description",
        "applications": ["kernel", "stdlib", "elixir"],
        "extra_apps": ["logger", "elixir"],
        "compile_args": ["--warnings-as-errors"],
        "elixirc_opts": ["--debug-info"],
        "test_args": ["--trace"],
        "elixir_opts": ["--no-halt"],
        "os_env": {{"FROM_OS": "os", "SHARED": "os"}},
        "env_inherit": ["ELIXIR_HOST_FLAG"],
        "env": {{"FROM_ENV": "env", "SHARED": "env"}},
    }},
}}
env = _elixir_user_env(ctx)
result = repr([
    _elixir_description(ctx),
    _elixir_applications(ctx),
    _elixir_compile_args(ctx),
    _elixir_test_args(ctx),
    _elixir_interpreter_opts(ctx),
    env.get("FROM_OS"),
    env.get("ELIXIR_HOST_FLAG"),
    env.get("SHARED"),
    env.get("FROM_ENV"),
])
"#
    );

    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(
        out,
        "[\"fallback description\", [\"kernel\", \"stdlib\", \"elixir\", \"logger\"], [\"--warnings-as-errors\", \"--debug-info\"], [\"--trace\"], [\"--no-halt\"], \"os\", \"host-value\", \"env\", \"env\"]"
    );
}

#[test]
fn prelude_elixir_interpreter_opts_are_rejected_in_mix_mode() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "pkg", "name": "suite", "id": "pkg/suite"}},
    "attr": {{"elixir_opts": ["--no-halt"]}},
}}
result = repr(_elixir_test_info_for(ctx, "mix", "mix.exs", [], [], "results", "log", "native"))
"#
    );

    let err = eval_prelude_source_to_repr(source).unwrap_err();

    assert!(err.contains("applies to direct ExUnit mode only"), "{err}");
}

#[test]
fn prelude_elixir_test_info_matches_the_executable_command() {
    let source = format!(
        r#"{}
mix_ctx = {{
    "label": {{"package": "server", "name": "tests", "id": "server/tests"}},
    "attr": {{"no_start": True}},
}}
direct_ctx = {{
    "label": {{"package": "server", "name": "tests", "id": "server/tests"}},
    "attr": {{}},
}}
apps = [{{"ebin_dir": ".once/out/server/application/ebin"}}]
result = repr([
    _elixir_test_info_for(mix_ctx, "mix", "mix.exs", apps, ["server/test/example_test.exs"], "results", "log", "native")["command"]["argv"],
    _elixir_test_info_for(direct_ctx, "elixir", "", apps, ["server/test/example_test.exs"], "results", "log", "native")["command"]["argv"],
])
"#,
        all_prelude_source()
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert!(out.contains("\"--no-start\""), "{out}");
    assert!(
        out.contains("\"-pa\", \"../.once/out/server/application/ebin\""),
        "{out}"
    );
}

#[test]
fn prelude_elixir_tools_are_declared_test_inputs() {
    let workspace = TempDir::new().unwrap();
    let tools_dir = workspace.path().join("pkg/tools");
    std::fs::create_dir_all(&tools_dir).unwrap();
    std::fs::write(tools_dir.join("prepare.sh"), "exit 0\n").unwrap();
    let store = store_for(workspace.path(), "pkg");
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "pkg", "name": "suite", "id": "pkg/suite"}},
    "attr": {{"tools": ["tools/**"]}},
}}
result = repr(_elixir_tool_inputs(ctx))
"#
    );

    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "[\"pkg/tools/prepare.sh\"]");
}

#[test]
fn prelude_zig_explicit_test_env_overrides_inherited_values() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_env(name):
    if name == "SHARED":
        return "host"
    return ""

ctx = {{
    "label": {{"package": "pkg", "name": "suite", "id": "pkg/suite"}},
    "attr": {{
        "env_inherit": ["SHARED"],
        "env": {{"SHARED": "env"}},
        "test_env": {{"SHARED": "test"}},
    }},
}}
result = repr(_zig_run_env(ctx, ".once/out/pkg/suite/test"))
"#
    );

    let out = eval_prelude_source_to_repr(source).unwrap();

    assert!(out.contains("\"SHARED\": \"test\""), "{out}");
}

#[test]
fn zig_configure_target_kind_schemas_are_discoverable() {
    for kind in [
        "zig_configure",
        "zig_configure_binary",
        "zig_configure_test",
    ] {
        let schema = built_in_target_kind_schema(kind).expect("zig configure schema");
        assert_eq!(schema.kind, kind);
        assert!(target_kind_has_impl(kind).unwrap());
        assert!(schema.attrs.iter().any(|attr| attr.name == "mode"));
        assert!(schema.attrs.iter().any(|attr| attr.name == "threaded"));
        assert!(schema.attrs.iter().any(|attr| attr.name == "zigopt"));
        assert!(schema.attrs.iter().any(|attr| attr.name == "zig_version"));
        assert!(
            !schema.examples.is_empty(),
            "{kind} should expose a starter example"
        );
    }
}

#[test]
fn prelude_zig_binary_declares_build_exe_action_with_module_deps() {
    let tmp = TempDir::new().expect("tempdir");
    let source_dir = tmp.path().join("pkg/src");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(
        source_dir.join("main.zig"),
        "const math = @import(\"calc\");",
    )
    .unwrap();
    std::fs::write(source_dir.join("math.zig"), "pub const answer = 42;").unwrap();

    let store = AnalysisStore::new(
        tmp.path().to_path_buf(),
        "pkg".to_string(),
        ".once/out/pkg/hello".to_string(),
    );
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "linux"

def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/tools/zig", "version"]:
        return "0.15.1\n"
    fail("unexpected host_command: " + repr(argv))

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "hello",
        "id": "pkg/hello",
    }},
    "attr": {{
        "zig": "/tools/zig",
        "main": "src/main.zig",
        "import_names": {{"math": "calc"}},
        "optimize": "ReleaseSafe",
    }},
    "deps": [{{
        "zig_dependency_set": True,
        "zig_modules": [{{
            "zig_module": True,
            "label_id": "pkg/math",
            "import_name": "math",
            "canonical_name": "once_pkg_x47_math",
            "module_context": {{
                "import_name": "math",
                "canonical_name": "once_pkg_x47_math",
                "main": "pkg/src/math.zig",
                "deps": [],
                "zigopts": [],
            }},
            "transitive_module_contexts": [],
        }}],
        "transitive_sources": ["pkg/src/math.zig"],
        "transitive_data": [],
    }}],
    "srcs": ["src/**/*.zig"],
    "build_dir": ".once/out/pkg/hello",
    "scratch_dir": ".once/tmp/analysis/pkg/hello",
    "capability": "build",
    "run": {{"visible": False}},
}}
result = repr(_zig_binary_impl(ctx))
"#
    );
    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    assert!(out.unwrap().contains("\"target_kind\": \"zig_binary\""));

    let action = action_by_identifier(&store, "pkg/hello:zig-build-exe");
    assert_eq!(action.argv[0], "/tools/zig");
    assert_eq!(action.argv[1], "build-exe");
    assert!(action.argv.contains(&"--dep".to_string()));
    assert!(action.argv.contains(&"calc=once_pkg_x47_math".to_string()));
    assert!(action.argv.contains(&"-O".to_string()));
    assert!(action.argv.contains(&"ReleaseSafe".to_string()));
    assert!(action
        .argv
        .contains(&"-Monce_pkg_x47_hello=pkg/src/main.zig".to_string()));
    assert!(action
        .argv
        .contains(&"-Monce_pkg_x47_math=pkg/src/math.zig".to_string()));
    assert_eq!(
        action.outputs,
        vec![".once/out/pkg/hello/hello".to_string()]
    );
    assert!(action.inputs.contains(&"pkg/src/main.zig".to_string()));
    assert!(action.inputs.contains(&"pkg/src/math.zig".to_string()));
}

#[test]
fn prelude_zig_canonical_names_are_collision_safe() {
    let prelude = all_prelude_source();
    let out = eval_prelude_function_in(
        prelude,
        "_zig_safe_name",
        r#"("pkg/foo-bar") == _zig_safe_name("pkg/foo_bar")"#,
    )
    .unwrap();

    assert_eq!(out, "False");
}

#[test]
fn prelude_zig_import_names_reject_unknown_keys() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "pkg",
        "name": "app",
        "id": "pkg/app",
    }},
    "attr": {{
        "main": "src/main.zig",
        "import_names": {{"typo": "math"}},
    }},
    "deps": [{{
        "zig_module": True,
        "label_id": "pkg/math",
        "import_name": "math",
        "canonical_name": "once_pkg_x47_math",
        "module_context": {{
            "import_name": "math",
            "canonical_name": "once_pkg_x47_math",
            "main": "pkg/src/math.zig",
            "deps": [],
            "zigopts": [],
        }},
        "transitive_module_contexts": [],
        "transitive_sources": ["pkg/src/math.zig"],
        "transitive_data": [],
    }}],
    "srcs": [],
    "build_dir": ".once/out/pkg/app",
    "scratch_dir": ".once/tmp/analysis/pkg/app",
    "capability": "build",
    "run": {{"visible": False}},
}}
result = repr(_zig_library_impl(ctx))
"#
    );
    let err = eval_prelude_source_to_repr(source).unwrap_err();
    assert!(
        err.contains("import_names key `typo` does not match any Zig module dependency"),
        "{err}"
    );
}

#[test]
fn prelude_zig_import_names_reject_ambiguous_short_keys() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "pkg",
        "name": "app",
        "id": "pkg/app",
    }},
    "attr": {{
        "main": "src/main.zig",
        "import_names": {{"math": "renamed_math"}},
    }},
    "deps": [
        {{
            "zig_module": True,
            "label_id": "pkg/a/math",
            "import_name": "a_math",
            "canonical_name": "once_pkg_x47_a_x47_math",
            "module_context": {{"import_name": "a_math", "canonical_name": "once_pkg_x47_a_x47_math", "main": "pkg/a/math.zig", "deps": [], "zigopts": []}},
            "transitive_module_contexts": [],
            "transitive_sources": ["pkg/a/math.zig"],
            "transitive_data": [],
        }},
        {{
            "zig_module": True,
            "label_id": "pkg/b/math",
            "import_name": "b_math",
            "canonical_name": "once_pkg_x47_b_x47_math",
            "module_context": {{"import_name": "b_math", "canonical_name": "once_pkg_x47_b_x47_math", "main": "pkg/b/math.zig", "deps": [], "zigopts": []}},
            "transitive_module_contexts": [],
            "transitive_sources": ["pkg/b/math.zig"],
            "transitive_data": [],
        }},
    ],
    "srcs": [],
    "build_dir": ".once/out/pkg/app",
    "scratch_dir": ".once/tmp/analysis/pkg/app",
    "capability": "build",
    "run": {{"visible": False}},
}}
result = repr(_zig_library_impl(ctx))
"#
    );
    let err = eval_prelude_source_to_repr(source).unwrap_err();
    assert!(
        err.contains("import_names key `math` is ambiguous across Zig module dependencies"),
        "{err}"
    );
}

#[test]
fn prelude_zig_rejects_duplicate_import_aliases() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "pkg",
        "name": "app",
        "id": "pkg/app",
    }},
    "attr": {{
        "main": "src/main.zig",
    }},
    "deps": [
        {{
            "zig_module": True,
            "label_id": "pkg/a",
            "import_name": "math",
            "canonical_name": "once_pkg_x47_a",
            "module_context": {{"import_name": "math", "canonical_name": "once_pkg_x47_a", "main": "pkg/a.zig", "deps": [], "zigopts": []}},
            "transitive_module_contexts": [],
            "transitive_sources": ["pkg/a.zig"],
            "transitive_data": [],
        }},
        {{
            "zig_module": True,
            "label_id": "pkg/b",
            "import_name": "math",
            "canonical_name": "once_pkg_x47_b",
            "module_context": {{"import_name": "math", "canonical_name": "once_pkg_x47_b", "main": "pkg/b.zig", "deps": [], "zigopts": []}},
            "transitive_module_contexts": [],
            "transitive_sources": ["pkg/b.zig"],
            "transitive_data": [],
        }},
    ],
    "srcs": [],
    "build_dir": ".once/out/pkg/app",
    "scratch_dir": ".once/tmp/analysis/pkg/app",
    "capability": "build",
    "run": {{"visible": False}},
}}
result = repr(_zig_library_impl(ctx))
"#
    );
    let err = eval_prelude_source_to_repr(source).unwrap_err();
    assert!(err.contains("duplicate Zig import name `math`"), "{err}");
}

#[test]
fn prelude_zig_rejects_c_import_alias_when_c_module_is_generated() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "pkg",
        "name": "app",
        "id": "pkg/app",
    }},
    "attr": {{
        "main": "src/main.zig",
    }},
    "deps": [
        {{
            "zig_module": True,
            "label_id": "pkg/native_zig",
            "import_name": "c",
            "canonical_name": "once_pkg_x47_native_uzig",
            "module_context": {{"import_name": "c", "canonical_name": "once_pkg_x47_native_uzig", "main": "pkg/native.zig", "deps": [], "zigopts": []}},
            "transitive_module_contexts": [],
            "transitive_sources": ["pkg/native.zig"],
            "transitive_data": [],
        }},
        {{
            "c_provider": True,
            "label_id": "pkg/native",
            "transitive_headers": ["pkg/include/native.h"],
            "transitive_include_dirs": ["pkg/include"],
            "transitive_defines": [],
            "transitive_static_libraries": [],
            "transitive_dynamic_libraries": [],
            "transitive_linkopts": [],
            "transitive_data": [],
        }},
    ],
    "srcs": [],
    "build_dir": ".once/out/pkg/app",
    "scratch_dir": ".once/tmp/analysis/pkg/app",
    "capability": "build",
    "run": {{"visible": False}},
}}
result = repr(_zig_library_impl(ctx))
"#
    );
    let err = eval_prelude_source_to_repr(source).unwrap_err();
    assert!(
        err.contains("Zig import name `c` conflicts with the generated C module"),
        "{err}"
    );
}

#[test]
fn prelude_zig_headerless_c_provider_links_without_c_module_dep() {
    let tmp = TempDir::new().expect("tempdir");
    let source_dir = tmp.path().join("pkg/src");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(source_dir.join("main.zig"), "pub fn main() void {}\n").unwrap();

    let store = AnalysisStore::new(
        tmp.path().to_path_buf(),
        "pkg".to_string(),
        ".once/out/pkg/app".to_string(),
    );
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "linux"

def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/tools/zig", "version"]:
        return "0.15.1\n"
    fail("unexpected host_command: " + repr(argv))

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "app",
        "id": "pkg/app",
    }},
    "attr": {{
        "zig": "/tools/zig",
        "main": "src/main.zig",
    }},
    "deps": [{{
        "c_provider": True,
        "label_id": "pkg/prebuilt",
        "transitive_headers": [],
        "transitive_include_dirs": [],
        "transitive_defines": [],
        "transitive_static_libraries": ["pkg/vendor/libprebuilt.a"],
        "transitive_dynamic_libraries": [],
        "transitive_linkopts": ["-pthread"],
        "transitive_data": [],
    }}],
    "srcs": ["src/**/*.zig"],
    "build_dir": ".once/out/pkg/app",
    "scratch_dir": ".once/tmp/analysis/pkg/app",
    "capability": "build",
    "run": {{"visible": False}},
}}
result = repr(_zig_binary_impl(ctx))
"#
    );
    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    assert!(out.unwrap().contains("\"target_kind\": \"zig_binary\""));

    let build = action_by_identifier(&store, "pkg/app:zig-build-exe");
    assert!(!build.argv.contains(&"c=c".to_string()));
    assert!(!build.argv.iter().any(|arg| arg.starts_with("-Mc=")));
    assert!(build.argv.contains(&"pkg/vendor/libprebuilt.a".to_string()));
    assert!(build.argv.contains(&"-pthread".to_string()));
}

#[test]
fn prelude_zig_configuration_attrs_map_to_compile_args() {
    let tmp = TempDir::new().expect("tempdir");
    let source_dir = tmp.path().join("pkg/src");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(source_dir.join("main.zig"), "pub fn main() void {}\n").unwrap();

    let store = AnalysisStore::new(
        tmp.path().to_path_buf(),
        "pkg".to_string(),
        ".once/out/pkg/release".to_string(),
    );
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "linux"

def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/tools/zig", "version"]:
        return "0.15.1\n"
    fail("unexpected host_command: " + repr(argv))

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "release",
        "id": "pkg/release",
    }},
    "attr": {{
        "zig": "/tools/zig",
        "zig_version": "0.15.1",
        "main": "src/main.zig",
        "mode": "release_small",
        "threaded": "single",
        "zigopt": ["-fllvm", "-flto"],
        "use_cc_common_link": 1,
        "bootstrapped": 0,
    }},
    "deps": [],
    "srcs": ["src/**/*.zig"],
    "build_dir": ".once/out/pkg/release",
    "scratch_dir": ".once/tmp/analysis/pkg/release",
    "capability": "build",
    "run": {{"visible": False}},
}}
result = repr(_zig_configure_binary_impl(ctx))
"#
    );
    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    assert!(out.unwrap().contains("\"target_kind\": \"zig_binary\""));

    let action = action_by_identifier(&store, "pkg/release:zig-build-exe");
    assert!(action.argv.contains(&"-O".to_string()));
    assert!(action.argv.contains(&"ReleaseSmall".to_string()));
    assert!(action.argv.contains(&"-fsingle-threaded".to_string()));
    assert!(action.argv.contains(&"-fllvm".to_string()));
    assert!(action.argv.contains(&"-flto".to_string()));
    assert!(action
        .toolchain_identity
        .as_deref()
        .unwrap()
        .contains("\0bootstrapped\0"));
}

#[test]
fn prelude_zig_configuration_rejects_version_mismatch() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "linux"

def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/tools/zig", "version"]:
        return "0.15.1\n"
    fail("unexpected host_command: " + repr(argv))

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "bad",
        "id": "pkg/bad",
    }},
    "attr": {{
        "zig": "/tools/zig",
        "zig_version": "0.14.0",
        "main": "src/main.zig",
    }},
    "deps": [],
    "srcs": [],
    "build_dir": ".once/out/pkg/bad",
    "scratch_dir": ".once/tmp/analysis/pkg/bad",
    "capability": "build",
    "run": {{"visible": False}},
}}
result = repr(_zig_binary_impl(ctx))
"#
    );
    let err = eval_prelude_source_to_repr(source).unwrap_err();
    assert!(
        err.contains("Zig compiler version is `0.15.1`, expected `0.14.0`"),
        "{err}"
    );
}

#[test]
fn prelude_zig_c_library_can_use_standalone_translate_c() {
    let tmp = TempDir::new().expect("tempdir");
    let include_dir = tmp.path().join("pkg/include");
    std::fs::create_dir_all(&include_dir).unwrap();
    std::fs::write(include_dir.join("native.h"), "int native(void);\n").unwrap();

    let store = AnalysisStore::new(
        tmp.path().to_path_buf(),
        "pkg".to_string(),
        ".once/out/pkg/native_zig".to_string(),
    );
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_which(name):
    fail("host_which must not be called for standalone translate-c")

def host_command(argv, env = None, merge_stderr = None):
    fail("host_command must not be called for standalone translate-c")

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "native_zig",
        "id": "pkg/native_zig",
    }},
    "attr": {{
        "translate_c": "/tools/translate-c",
        "translate_c_identity": "translate-c test identity",
        "use_standalone_translate_c": 1,
        "mode": "debug",
        "threaded": "multi",
        "zigopt": ["-fno-llvm"],
    }},
    "deps": [{{
        "c_provider": True,
        "label_id": "pkg/native",
        "transitive_headers": ["pkg/include/native.h"],
        "transitive_include_dirs": ["pkg/include"],
        "transitive_defines": ["NATIVE=1"],
        "transitive_static_libraries": [],
        "transitive_dynamic_libraries": [],
        "transitive_linkopts": [],
        "transitive_data": [],
    }}],
    "srcs": [],
    "build_dir": ".once/out/pkg/native_zig",
    "scratch_dir": ".once/tmp/analysis/pkg/native_zig",
    "capability": "build",
    "run": {{"visible": False}},
}}
result = repr(_zig_c_library_impl(ctx))
"#
    );
    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    assert!(out.unwrap().contains("\"target_kind\": \"zig_c_library\""));

    let translate = action_by_identifier(&store, "pkg/native_zig:zig-translate-c:native_zig");
    assert_eq!(translate.argv[0], "/tools/translate-c");
    assert!(translate
        .argv
        .windows(2)
        .any(|args| args[0] == "-I" && args[1] == "."));
    assert!(translate.argv.contains(&"-o".to_string()));
    assert!(translate
        .argv
        .contains(&".once/out/pkg/native_zig/native_zig_c.zig".to_string()));
    assert!(translate.argv.contains(&"--emulate=clang".to_string()));
    assert!(translate.argv.contains(&"-O".to_string()));
    assert!(translate.argv.contains(&"Debug".to_string()));
    assert!(translate.argv.contains(&"-fno-single-threaded".to_string()));
    assert!(translate.argv.contains(&"-fno-llvm".to_string()));
    assert!(translate.argv.contains(&"-DNATIVE=1".to_string()));
    assert!(translate
        .toolchain_identity
        .as_deref()
        .unwrap()
        .contains("translate-c test identity"));
}

#[test]
fn prelude_zig_test_metadata_does_not_probe_compiler() {
    let tmp = TempDir::new().expect("tempdir");
    let source_dir = tmp.path().join("pkg/src");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(source_dir.join("math_test.zig"), "test \"ok\" {}").unwrap();

    let store = AnalysisStore::new(
        tmp.path().to_path_buf(),
        "pkg".to_string(),
        ".once/out/pkg/math_tests".to_string(),
    );
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_command(argv, env = None, merge_stderr = None):
    fail("host_command must not be called for Zig metadata")

def host_which(name):
    fail("host_which must not be called for Zig metadata")

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "math_tests",
        "id": "pkg/math_tests",
    }},
    "attr": {{
        "main": "src/math_test.zig",
        "labels": ["unit"],
    }},
    "deps": [],
    "srcs": ["src/**/*.zig"],
    "build_dir": ".once/out/pkg/math_tests",
    "scratch_dir": ".once/tmp/analysis/pkg/math_tests",
    "capability": "metadata",
    "run": {{"visible": False}},
}}
result = repr(_zig_test_impl(ctx))
"#
    );
    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let out = out.unwrap();
    assert!(out.contains("\"target_kind\": \"zig_test\""));
    assert!(out.contains("\"type\": \"zig_test\""));
    assert!(out.contains("\"unit\""));
    assert!(store.actions.is_empty());
}

#[test]
fn prelude_zig_test_metadata_does_not_require_root_dependency_providers() {
    let tmp = TempDir::new().expect("tempdir");
    let store = AnalysisStore::new(
        tmp.path().to_path_buf(),
        "pkg".to_string(),
        ".once/out/pkg/module_tests".to_string(),
    );
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_command(argv, env = None, merge_stderr = None):
    fail("host_command must not be called for Zig metadata")

def host_which(name):
    fail("host_which must not be called for Zig metadata")

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "module_tests",
        "id": "pkg/module_tests",
    }},
    "attr": {{
        "labels": ["module"],
    }},
    "deps": [],
    "srcs": [],
    "build_dir": ".once/out/pkg/module_tests",
    "scratch_dir": ".once/tmp/analysis/pkg/module_tests",
    "capability": "metadata",
    "run": {{"visible": False}},
}}
result = repr([_zig_test_impl(ctx), _zig_configure_test_impl(ctx)])
"#
    );
    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let out = out.unwrap();

    assert!(out.contains("\"target_kind\": \"zig_test\""));
    assert!(out.contains("\"type\": \"zig_test\""));
    assert!(out.contains("\"module\""));
    assert!(out.contains(".once/out/pkg/module_tests/module_tests"));
    assert!(store.actions.is_empty());
}

#[test]
fn prelude_c_library_declares_archive_and_provider_fields() {
    let tmp = TempDir::new().expect("tempdir");
    let source_dir = tmp.path().join("pkg/src");
    let include_dir = tmp.path().join("pkg/include");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::create_dir_all(&include_dir).unwrap();
    std::fs::write(source_dir.join("native.c"), "#include \"native.h\"\n").unwrap();
    std::fs::write(include_dir.join("native.h"), "int native(void);\n").unwrap();

    let store = AnalysisStore::new(
        tmp.path().to_path_buf(),
        "pkg".to_string(),
        ".once/out/pkg/native".to_string(),
    );
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "linux"

def host_which(name):
    if name == "cc":
        return "/tools/cc"
    if name == "ar":
        return "/tools/ar"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/tools/cc", "--version"]:
        return "cc test\n"
    fail("unexpected host_command: " + repr(argv))

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "native",
        "id": "pkg/native",
    }},
    "attr": {{
        "hdrs": ["include/native.h"],
        "includes": ["include"],
        "defines": ["NATIVE=1"],
        "copts": ["-Wall"],
        "archiver_identity": "ar test identity",
    }},
    "deps": [],
    "srcs": ["src/*.c"],
    "build_dir": ".once/out/pkg/native",
    "scratch_dir": ".once/tmp/analysis/pkg/native",
    "capability": "build",
    "run": {{"visible": False}},
}}
result = repr(_c_library_impl(ctx))
"#
    );
    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let out = out.unwrap();
    assert!(out.contains("\"c_provider\": True"));
    assert!(out.contains("\"archive\": \".once/out/pkg/native/libnative.a\""));

    let compile = action_by_identifier(&store, "pkg/native:c-compile:pkg/src/native.c");
    assert_eq!(compile.argv[0], "/tools/cc");
    assert!(compile.argv.contains(&"-DNATIVE=1".to_string()));
    assert!(compile.argv.contains(&"pkg/include".to_string()));
    assert!(compile.argv.contains(&"-Wall".to_string()));
    assert!(compile.inputs.contains(&"pkg/src/native.c".to_string()));
    assert!(compile.inputs.contains(&"pkg/include/native.h".to_string()));
    assert!(compile
        .outputs
        .contains(&".once/out/pkg/native/objects/pkg/src/native.c.o".to_string()));
    assert!(!compile
        .toolchain_identity
        .as_deref()
        .unwrap()
        .contains("\0cxx\0"));
    assert!(compile
        .toolchain_identity
        .as_deref()
        .unwrap()
        .contains("ar test identity"));

    let archive = action_by_identifier(&store, "pkg/native:c-archive");
    assert_eq!(archive.argv[0], "/tools/ar");
    assert_eq!(archive.argv[1], "crs");
    assert!(archive
        .outputs
        .contains(&".once/out/pkg/native/libnative.a".to_string()));
}

#[test]
fn prelude_c_library_preserves_source_paths_for_object_outputs() {
    let tmp = TempDir::new().expect("tempdir");
    let source_dir = tmp.path().join("pkg/src");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(
        source_dir.join("foo-bar.c"),
        "int dash(void) { return 1; }\n",
    )
    .unwrap();
    std::fs::write(
        source_dir.join("foo_bar.c"),
        "int underscore(void) { return 2; }\n",
    )
    .unwrap();

    let store = AnalysisStore::new(
        tmp.path().to_path_buf(),
        "pkg".to_string(),
        ".once/out/pkg/native".to_string(),
    );
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "linux"

def host_which(name):
    if name == "cc":
        return "/tools/cc"
    if name == "ar":
        return "/tools/ar"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/tools/cc", "--version"]:
        return "cc test\n"
    fail("unexpected host_command: " + repr(argv))

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "native",
        "id": "pkg/native",
    }},
    "attr": {{}},
    "deps": [],
    "srcs": ["src/*.c"],
    "build_dir": ".once/out/pkg/native",
    "scratch_dir": ".once/tmp/analysis/pkg/native",
    "capability": "build",
    "run": {{"visible": False}},
}}
result = repr(_c_library_impl(ctx))
"#
    );
    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    out.unwrap();

    let outputs = store
        .actions
        .iter()
        .flat_map(|action| action.outputs.iter().map(String::as_str))
        .collect::<Vec<_>>();
    assert!(outputs.contains(&".once/out/pkg/native/objects/pkg/src/foo-bar.c.o"));
    assert!(outputs.contains(&".once/out/pkg/native/objects/pkg/src/foo_bar.c.o"));
}

#[test]
fn prelude_c_library_provider_only_targets_do_not_probe_toolchain() {
    let tmp = TempDir::new().expect("tempdir");
    let include_dir = tmp.path().join("pkg/include");
    let vendor_dir = tmp.path().join("pkg/vendor");
    std::fs::create_dir_all(&include_dir).unwrap();
    std::fs::create_dir_all(&vendor_dir).unwrap();
    std::fs::write(include_dir.join("native.h"), "int native(void);\n").unwrap();
    std::fs::write(vendor_dir.join("mylib.so"), "dynamic\n").unwrap();

    let store = store_for(tmp.path(), "pkg/native");
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_which(name):
    fail("host_which must not be called for provider-only C targets")

def host_command(argv, env = None, merge_stderr = None):
    fail("host_command must not be called for provider-only C targets")

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "native",
        "id": "pkg/native",
    }},
    "attr": {{
        "hdrs": ["include/native.h"],
        "dynamic_libraries": ["vendor/mylib.so"],
        "compiler": "/missing/cc",
        "cxx_compiler": "/missing/cxx",
        "archiver": "/missing/ar",
    }},
    "deps": [],
    "srcs": [],
    "build_dir": ".once/out/pkg/native",
    "scratch_dir": ".once/tmp/analysis/pkg/native",
    "capability": "build",
    "run": {{"visible": False}},
}}
provider = _c_library_impl(ctx)
result = repr((provider["archive"], provider["dynamic_libraries"], provider["transitive_dynamic_libraries"]))
"#
    );
    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let out = out.unwrap();

    assert!(out.contains("\"\""));
    assert!(out.contains("pkg/vendor/mylib.so"));
    assert!(store.actions.is_empty());
}

#[test]
fn prelude_c_library_propagates_android_native_libraries() {
    let tmp = TempDir::new().expect("tempdir");
    let store = store_for(tmp.path(), "pkg/native");
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "linux"

def host_which(name):
    if name == "cc":
        return "/tools/cc"
    if name == "c++":
        return "/tools/cxx"
    if name == "ar":
        return "/tools/ar"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/tools/cc", "--version"]:
        return "cc test\n"
    if argv == ["/tools/cxx", "--version"]:
        return "cxx test\n"
    fail("unexpected host_command: " + repr(argv))

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "native",
        "id": "pkg/native",
    }},
    "attr": {{
        "dynamic_libraries": ["jni/libnative.so"],
        "android_abi": "arm64-v8a",
    }},
    "deps": [{{
        "c_provider": True,
        "android_native_libraries": [{{"abi": "arm64-v8a", "path": "pkg/jni/libdep.so"}}],
        "transitive_android_native_libraries": [{{"abi": "arm64-v8a", "path": "pkg/jni/libdep.so"}}],
    }}],
    "srcs": [],
    "build_dir": ".once/out/pkg/native",
    "scratch_dir": ".once/tmp/analysis/pkg/native",
    "capability": "build",
    "run": {{"visible": False}},
}}
provider = _c_library_impl(ctx)
result = repr((provider["android_native_libraries"], provider["transitive_android_native_libraries"]))
"#
    );
    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let out = out.unwrap();

    assert!(out.contains("[{\"abi\": \"arm64-v8a\", \"path\": \"pkg/jni/libnative.so\"}]"));
    assert!(out.contains("{\"abi\": \"arm64-v8a\", \"path\": \"pkg/jni/libdep.so\"}"));
}

#[test]
fn prelude_zig_static_library_consumes_c_provider_fields() {
    let tmp = TempDir::new().expect("tempdir");
    let source_dir = tmp.path().join("pkg/src");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(source_dir.join("math.zig"), "const c = @import(\"c\");").unwrap();

    let store = AnalysisStore::new(
        tmp.path().to_path_buf(),
        "pkg".to_string(),
        ".once/out/pkg/math".to_string(),
    );
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "linux"

def host_which(name):
    if name == "sh":
        return "/bin/sh"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/tools/zig", "version"]:
        return "0.15.1\n"
    fail("unexpected host_command: " + repr(argv))

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "math",
        "id": "pkg/math",
    }},
    "attr": {{
        "zig": "/tools/zig",
        "main": "src/math.zig",
        "compiler_runtime": "include",
        "strip_debug_symbols": True,
        "linker_script": "linker.ld",
    }},
    "deps": [{{
        "c_provider": True,
        "label_id": "pkg/native",
        "transitive_headers": ["pkg/include/native.h"],
        "transitive_include_dirs": ["pkg/include"],
        "transitive_defines": ["NATIVE=1"],
        "transitive_static_libraries": ["pkg/native/libnative.a"],
        "transitive_linkopts": ["-pthread"],
        "transitive_data": [],
    }}],
    "srcs": ["src/**/*.zig"],
    "build_dir": ".once/out/pkg/math",
    "scratch_dir": ".once/tmp/analysis/pkg/math",
    "capability": "build",
    "run": {{"visible": False}},
}}
result = repr(_zig_static_library_impl(ctx))
"#
    );
    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let out = out.unwrap();
    assert!(out.contains("\"target_kind\": \"zig_static_library\""));
    assert!(out.contains("\"c_provider\": True"));
    assert!(out.contains("\"archive\": \".once/out/pkg/math/libmath.a\""));

    let translate = action_by_identifier(&store, "pkg/math:zig-translate-c:c");
    assert_eq!(translate.argv[0], "/tools/zig");
    assert_eq!(translate.argv[1], "translate-c");
    assert_eq!(
        translate.stdout.as_deref(),
        Some(".once/out/pkg/math/c_c.zig")
    );
    assert!(translate
        .inputs
        .contains(&"pkg/include/native.h".to_string()));

    let build = action_by_identifier(&store, "pkg/math:zig-build-lib");
    assert_eq!(build.argv[0], "/tools/zig");
    assert_eq!(build.argv[1], "build-lib");
    assert!(build.argv.contains(&"-fcompiler-rt".to_string()));
    assert!(build.argv.contains(&"-fstrip".to_string()));
    assert!(build.argv.contains(&"--dep".to_string()));
    assert!(build.argv.contains(&"c=c".to_string()));
    assert!(build.argv.contains(&"-DNATIVE=1".to_string()));
    assert!(build.argv.contains(&"pkg/include".to_string()));
    assert!(build.argv.contains(&"-T".to_string()));
    assert!(build.argv.contains(&"pkg/linker.ld".to_string()));
    assert!(build.argv.contains(&"pkg/native/libnative.a".to_string()));
    assert!(build.argv.contains(&"-pthread".to_string()));
    assert!(build
        .outputs
        .contains(&".once/out/pkg/math/libmath.a".to_string()));

    let docs = action_by_identifier(&store, "pkg/math:zig-docs");
    assert!(docs
        .outputs
        .contains(&".once/out/pkg/math/math.docs".to_string()));
}

#[test]
fn prelude_zig_c_link_args_preserve_dynamic_library_paths() {
    let prelude = all_prelude_source();
    let out = eval_prelude_function_in(
        prelude,
        "_zig_c_link_args",
        r#"({
            "linkopts": ["-pthread"],
            "static_libraries": ["pkg/libnative.a"],
            "dynamic_libraries": ["pkg/vendor/mylib.so", "pkg/vendor/libfoo.so.1"],
        })"#,
    )
    .unwrap();

    assert!(out.contains("\"pkg/vendor/mylib.so\""), "{out}");
    assert!(out.contains("\"pkg/vendor/libfoo.so.1\""), "{out}");
    assert!(!out.contains("-Lpkg/vendor"), "{out}");
    assert!(!out.contains("-lmylib"), "{out}");
    assert!(!out.contains("-lfoo.so.1"), "{out}");
}

#[test]
fn prelude_c_library_consumes_zig_c_provider_static_and_shared_libraries() {
    let tmp = TempDir::new().expect("tempdir");
    let source_dir = tmp.path().join("pkg/src");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(
        source_dir.join("static.zig"),
        "export fn add() i32 { return 1; }",
    )
    .unwrap();
    std::fs::write(
        source_dir.join("shared.zig"),
        "export fn sub() i32 { return 1; }",
    )
    .unwrap();

    let store = store_for(tmp.path(), "pkg/consumer");
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "linux"

def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/tools/zig", "version"]:
        return "0.15.1\n"
    fail("unexpected host_command: " + repr(argv))

static_ctx = {{
    "label": {{
        "package": "pkg",
        "name": "zstatic",
        "id": "pkg/zstatic",
    }},
    "attr": {{
        "zig": "/tools/zig",
        "main": "src/static.zig",
        "linkopts": ["-Wl,--static-zig"],
    }},
    "deps": [],
    "srcs": ["src/**/*.zig"],
    "build_dir": ".once/out/pkg/zstatic",
    "scratch_dir": ".once/tmp/analysis/pkg/zstatic",
    "capability": "build",
    "run": {{"visible": False}},
}}

shared_ctx = {{
    "label": {{
        "package": "pkg",
        "name": "zshared",
        "id": "pkg/zshared",
    }},
    "attr": {{
        "zig": "/tools/zig",
        "main": "src/shared.zig",
        "linkopts": ["-Wl,--shared-zig"],
    }},
    "deps": [],
    "srcs": ["src/**/*.zig"],
    "build_dir": ".once/out/pkg/zshared",
    "scratch_dir": ".once/tmp/analysis/pkg/zshared",
    "capability": "build",
    "run": {{"visible": False}},
}}

consumer_ctx = {{
    "label": {{
        "package": "pkg",
        "name": "consumer",
        "id": "pkg/consumer",
    }},
    "attr": {{}},
    "deps": [_zig_static_library_impl(static_ctx), _zig_shared_library_impl(shared_ctx)],
    "srcs": [],
    "build_dir": ".once/out/pkg/consumer",
    "scratch_dir": ".once/tmp/analysis/pkg/consumer",
    "capability": "build",
    "run": {{"visible": False}},
}}

provider = _c_library_impl(consumer_ctx)
result = repr((provider["transitive_static_libraries"], provider["transitive_dynamic_libraries"], provider["transitive_linkopts"]))
"#
    );
    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let out = out.unwrap();

    assert!(out.contains(".once/out/pkg/consumer/libzstatic.a"), "{out}");
    assert!(
        out.contains(".once/out/pkg/consumer/libzshared.so"),
        "{out}"
    );
    assert!(out.contains("-Wl,--static-zig"), "{out}");
    assert!(out.contains("-Wl,--shared-zig"), "{out}");
}

#[test]
fn prelude_zig_translate_c_redirects_stdout_without_shell() {
    let tmp = TempDir::new().expect("tempdir");
    let source_dir = tmp.path().join("pkg/src");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(source_dir.join("math.zig"), "const c = @import(\"c\");").unwrap();

    let store = store_for(tmp.path(), "pkg/math");
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "windows"

def host_which(name):
    if name == "powershell":
        return "C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/tools/zig", "version"]:
        return "0.15.1\n"
    fail("unexpected host_command: " + repr(argv))

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "math",
        "id": "pkg/math",
    }},
    "attr": {{
        "zig": "/tools/zig",
        "main": "src/math.zig",
    }},
    "deps": [{{
        "c_provider": True,
        "label_id": "pkg/native",
        "transitive_headers": ["pkg/include/native.h"],
        "transitive_include_dirs": ["pkg/include"],
        "transitive_data": [],
    }}],
    "srcs": ["src/**/*.zig"],
    "build_dir": ".once/out/pkg/math",
    "scratch_dir": ".once/tmp/analysis/pkg/math",
    "capability": "build",
    "run": {{"visible": False}},
}}
result = repr(_zig_static_library_impl(ctx))
"#
    );
    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    out.unwrap();

    let translate = action_by_identifier(&store, "pkg/math:zig-translate-c:c");
    // The tool is invoked directly (no host shell) and stdout is captured
    // into the declared output through the redirection primitive.
    assert_eq!(translate.argv[0], "/tools/zig");
    assert_eq!(translate.argv[1], "translate-c");
    assert!(!translate
        .argv
        .iter()
        .any(|arg| arg == "-Command" || arg.contains("powershell") || arg.contains("> '")));
    assert_eq!(
        translate.stdout.as_deref(),
        Some(".once/out/pkg/math/c_c.zig")
    );
}

#[test]
fn prelude_zig_binary_run_redirects_output_without_shell() {
    let tmp = TempDir::new().expect("tempdir");
    let source_dir = tmp.path().join("pkg/src");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(source_dir.join("main.zig"), "pub fn main() void {}").unwrap();

    let store = store_for(tmp.path(), "pkg/app");
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "windows"

def host_which(name):
    if name == "powershell":
        return "C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
    fail("unexpected host_which: " + name)

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "app",
        "id": "pkg/app",
    }},
    "attr": {{
        "main": "src/main.zig",
        "args": ["--smoke"],
    }},
    "deps": [],
    "srcs": ["src/**/*.zig"],
    "build_dir": ".once/out/pkg/app",
    "scratch_dir": ".once/tmp/analysis/pkg/app",
    "capability": "run",
    "run": {{"visible": False}},
}}
result = repr(_zig_binary_impl(ctx))
"#
    );
    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    out.unwrap();

    let run = action_by_identifier(&store, "pkg/app:zig-run");
    // The binary is executed directly with its user args; no host shell
    // wrapper. stdout and stderr are merged into the run log via the
    // redirection primitive.
    assert!(run.argv[0].starts_with(".once/out/pkg/app/"));
    assert!(!run
        .argv
        .iter()
        .any(|arg| arg == "-Command" || arg.contains("powershell")));
    assert_eq!(run.argv.last().unwrap(), "--smoke");
    assert_eq!(
        run.stdout.as_deref(),
        Some(".once/out/pkg/app/run/stdout.log")
    );
    assert_eq!(
        run.stderr.as_deref(),
        Some(".once/out/pkg/app/run/stdout.log")
    );

    let prepare = action_by_identifier(&store, "pkg/app:zig-run-prepare");
    assert_eq!(
        prepare.operation,
        Some(DeclaredActionOperation::PreparePath {
            path: ".once/out/pkg/app/run".to_string(),
            mode: DeclaredPreparePathMode::Directory,
        })
    );

    // The run-result marker is now materialized by a portable write_path
    // action rather than a shell here-doc.
    let marker = action_by_identifier(&store, "write_path:.once/out/pkg/app/run/run.json");
    match &marker.operation {
        Some(DeclaredActionOperation::WriteFile { path, bytes }) => {
            assert_eq!(path, ".once/out/pkg/app/run/run.json");
            let text = String::from_utf8(bytes.clone()).unwrap();
            assert!(text.contains("\"schema\":\"once.run_result.v1\""));
            assert!(text.contains("\"exit_code\":0"));
        }
        other => panic!("expected write_path marker, got {other:?}"),
    }
}

#[test]
fn prelude_zig_test_run_uses_powershell_on_windows() {
    let tmp = TempDir::new().expect("tempdir");
    let source_dir = tmp.path().join("pkg/src");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(source_dir.join("test.zig"), "test \"ok\" {}").unwrap();

    let store = store_for(tmp.path(), "pkg/suite");
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "windows"

def host_which(name):
    if name == "powershell":
        return "C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/tools/zig", "version"]:
        return "0.15.1\n"
    fail("unexpected host_command: " + repr(argv))

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "suite",
        "id": "pkg/suite",
    }},
    "attr": {{
        "zig": "/tools/zig",
        "main": "src/test.zig",
        "args": ["--summary", "all"],
    }},
    "deps": [],
    "srcs": ["src/**/*.zig"],
    "build_dir": ".once/out/pkg/suite",
    "scratch_dir": ".once/tmp/analysis/pkg/suite",
    "capability": "test",
    "run": {{"visible": False}},
}}
result = repr(_zig_test_impl(ctx))
"#
    );
    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    out.unwrap();

    let run = action_by_identifier(&store, "pkg/suite:zig-test-run");
    assert_eq!(
        run.argv[0],
        "C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
    );
    assert!(run.argv.iter().any(|arg| arg == "-Command"));
    assert!(run
        .argv
        .last()
        .unwrap()
        .contains("ConvertTo-Json -Depth 10 -Compress"));
    assert!(!run.argv.last().unwrap().contains("CreateDirectory"));
    assert!(run.argv.last().unwrap().contains("'--summary' 'all'"));

    let prepare = action_by_identifier(&store, "pkg/suite:zig-test-prepare");
    assert_eq!(
        prepare.operation,
        Some(DeclaredActionOperation::PreparePath {
            path: ".once/out/pkg/suite/test".to_string(),
            mode: DeclaredPreparePathMode::Directory,
        })
    );
}

#[test]
fn prelude_zig_shared_library_propagates_android_native_libraries() {
    let tmp = TempDir::new().expect("tempdir");
    let source_dir = tmp.path().join("pkg/src");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(
        source_dir.join("math.zig"),
        "export fn add() i32 { return 1; }",
    )
    .unwrap();

    let store = AnalysisStore::new(
        tmp.path().to_path_buf(),
        "pkg".to_string(),
        ".once/out/pkg/math".to_string(),
    );
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "linux"

def host_which(name):
    if name == "sh":
        return "/bin/sh"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/tools/zig", "version"]:
        return "0.15.1\n"
    fail("unexpected host_command: " + repr(argv))

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "math",
        "id": "pkg/math",
    }},
    "attr": {{
        "zig": "/tools/zig",
        "main": "src/math.zig",
        "android_abi": "arm64-v8a",
    }},
    "deps": [{{
        "c_provider": True,
        "label_id": "pkg/native",
        "transitive_headers": [],
        "transitive_include_dirs": [],
        "transitive_defines": [],
        "transitive_static_libraries": [],
        "transitive_dynamic_libraries": ["pkg/jni/libnative.so"],
        "transitive_linkopts": [],
        "transitive_data": [],
        "android_native_libraries": [{{"abi": "arm64-v8a", "path": "pkg/jni/libnative.so"}}],
        "transitive_android_native_libraries": [{{"abi": "arm64-v8a", "path": "pkg/jni/libnative.so"}}],
    }}],
    "srcs": ["src/**/*.zig"],
    "build_dir": ".once/out/pkg/math",
    "scratch_dir": ".once/tmp/analysis/pkg/math",
    "capability": "build",
    "run": {{"visible": False}},
}}
provider = _zig_shared_library_impl(ctx)
result = repr((provider["android_native_libraries"], provider["transitive_android_native_libraries"]))
"#
    );
    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let out = out.unwrap();

    assert!(out.contains(".once/out/pkg/math/libmath.so"));
    assert!(out.contains("{\"abi\": \"arm64-v8a\", \"path\": \"pkg/jni/libnative.so\"}"));
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
fn prelude_android_visible_run_starts_configured_emulator_first() {
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
    "run": {{"visible": True}},
}}
tools = {{
    "sdk_root": "/sdk",
    "adb": "/sdk/platform-tools/adb",
    "emulator": "/sdk/emulator/emulator",
    "identity": "android-adb",
}}
_android_run_app(
    ctx,
    {{"application_id": "dev.once.hello", "emulator_device": "Pixel_9"}},
    tools,
)
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "apps/hello/Hello");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    assert_eq!(store.actions.len(), 8);

    // The visible emulator launches first (still a macOS-only host-shell
    // convenience, deliberately left as a shell script).
    assert_eq!(
        store.actions[0].identifier.as_deref(),
        Some("android_visible_emulator:apps/hello/Hello")
    );
    assert!(store.actions[0].argv[2].contains("screen"));
    assert!(store.actions[0].argv[2].contains("osascript"));
    assert!(!store.actions[0].argv[2].contains("launchctl submit"));
    assert!(store.actions[0].argv[2].contains("nohup '/sdk/emulator/emulator' -avd 'Pixel_9'"));

    // Device steps invoke adb directly, with no host-shell wrapper. The
    // on-device readiness probe stays an `adb shell` argument.
    assert_eq!(
        store.actions[1].argv,
        vec!["/sdk/platform-tools/adb", "wait-for-device"]
    );
    assert_eq!(store.actions[2].argv[0], "/sdk/platform-tools/adb");
    assert_eq!(store.actions[2].argv[1], "shell");
    assert!(store.actions[2].argv[2].contains("sys.boot_completed"));

    // Completion markers are materialized by portable write_path actions.
    assert_eq!(
        store.actions[3].operation,
        Some(DeclaredActionOperation::WriteFile {
            path: ".once/out/apps/hello/Hello/run/device-ready".to_string(),
            bytes: Vec::new(),
        })
    );

    assert_eq!(store.actions[4].argv[0], "/sdk/platform-tools/adb");
    assert_eq!(store.actions[4].argv[1], "install");
    assert_eq!(
        store.actions[4].inputs,
        vec![
            ".once/out/apps/hello/Hello/Hello.apk",
            ".once/out/apps/hello/Hello/run/device-ready"
        ]
    );
    assert_eq!(
        store.actions[5].operation,
        Some(DeclaredActionOperation::WriteFile {
            path: ".once/out/apps/hello/Hello/run/installed".to_string(),
            bytes: Vec::new(),
        })
    );

    assert_eq!(store.actions[6].argv[0], "/sdk/platform-tools/adb");
    assert_eq!(store.actions[6].argv[1], "shell");
    assert_eq!(
        store.actions[6].inputs,
        vec![".once/out/apps/hello/Hello/run/installed"]
    );
    assert_eq!(
        store.actions[7].operation,
        Some(DeclaredActionOperation::WriteFile {
            path: ".once/out/apps/hello/Hello/run/launched".to_string(),
            bytes: Vec::new(),
        })
    );
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
    let compile_tool = action_by_identifier(
        &store,
        "android_resource_link_tool_compile:apps/hello/Hello",
    );
    assert_eq!(compile_tool.argv[0], "/jdk/bin/javac");
    let link = action_by_identifier(&store, "android_resource_link:apps/hello/Hello");
    let link_tool_digest =
        action_by_identifier(&store, "android_resource_link_tool_digest:apps/hello/Hello");
    assert_eq!(
        link_tool_digest.operation,
        Some(DeclaredActionOperation::WriteTreeDigest {
            root: ".once/out/apps/hello/Hello/aapt2_link_tool/classes".to_string(),
            output: ".once/out/apps/hello/Hello/aapt2_link_tool/classes.sha256".to_string(),
            include_suffixes: vec![],
        })
    );
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
        .any(|input| input == ".once/out/apps/hello/Hello/aapt2_link_tool/classes.sha256"));
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
        .any(|arg| arg.contains("@.once/out/apps/hello/Hello/java_sources.list")));
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
    let support_dir = workspace.path().join("shared/swift/Support");
    let runtime_dir = workspace.path().join("shared/swift/Runtime");
    std::fs::create_dir_all(&support_dir).unwrap();
    std::fs::create_dir_all(&runtime_dir).unwrap();
    std::fs::write(support_dir.join("module.map"), "module support {}\n").unwrap();
    std::fs::write(runtime_dir.join("message.txt"), "hello\n").unwrap();
    let cxx_runtime = workspace.path().join("libc++_shared.so");
    std::fs::write(&cxx_runtime, "runtime").unwrap();
    let cxx_runtime = cxx_runtime.to_string_lossy();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "swiftc":
        return "/toolchains/swift/bin/swiftc"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
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
        "package_name": "SharedPackage",
        "sdk": "/android/sdk",
        "resource_dir": "/swift/android/resources",
        "cxx_runtime": "{cxx_runtime}",
        "tools_directory": "/android/ndk/bin",
        "copts": ["-warnings-as-errors"],
        "defines": ["SHARED_SWIFT"],
        "library_evolution": True,
        "linkopts": ["-Xlinker", "--own-link-option"],
        "data": ["Runtime/**"],
        "swiftc_inputs": ["Support/*.map"],
    }},
    "deps": [{{
        "transitive_swiftmodule_dirs": [".once/out/shared/swift/Dep"],
        "swiftmodule": ".once/out/shared/swift/Dep/Dep.swiftmodule",
        "swiftdoc": ".once/out/shared/swift/Dep/Dep.swiftdoc",
        "transitive_swift_defines": ["DEP_SWIFT"],
        "transitive_linkopts": ["-Xlinker", "--dep-link-option"],
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
    provider["transitive_swift_defines"],
    provider["transitive_data"],
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
    assert!(out.contains("DEP_SWIFT"), "{out}");
    assert!(out.contains("Runtime/message.txt"), "{out}");
    assert_eq!(store.actions.len(), 2);
    assert_swift_android_compile_action(&store.actions[0]);
    assert!(matches!(
        store.actions[1].operation,
        Some(DeclaredActionOperation::MaterializeHostFile { ref destination, .. })
            if destination.ends_with("libc++_shared.so")
    ));
}

fn assert_swift_android_compile_action(action: &DeclaredAction) {
    assert_eq!(
        action.identifier.as_deref(),
        Some("swift_android_compile:shared/swift/SharedSwift")
    );
    assert!(action.argv.iter().any(|arg| arg == "-emit-library"));
    assert!(action.argv.iter().any(|arg| arg == "-static-stdlib"));
    assert!(action.argv.iter().any(|arg| arg == "-target"));
    assert!(action.argv.iter().any(|arg| arg == "-tools-directory"));
    assert!(action.argv.iter().any(|arg| arg == "-warnings-as-errors"));
    assert!(action
        .argv
        .windows(2)
        .any(|args| args == ["-D", "SHARED_SWIFT"]));
    assert!(action
        .argv
        .windows(2)
        .any(|args| args == ["-D", "DEP_SWIFT"]));
    assert!(action
        .argv
        .iter()
        .any(|arg| arg == "-enable-library-evolution"));
    assert!(action.argv.iter().any(|arg| arg == "--dep-link-option"));
    assert!(action.argv.iter().any(|arg| arg == "--own-link-option"));
    assert!(action
        .argv
        .windows(2)
        .any(|args| args == ["-package-name", "SharedPackage"]));
    assert!(action
        .inputs
        .iter()
        .any(|input| input == "shared/swift/Sources/Greeting.swift"));
    assert!(action
        .inputs
        .iter()
        .any(|input| input == "shared/swift/Support/module.map"));
    assert!(action
        .inputs
        .iter()
        .any(|input| input == ".once/out/shared/swift/Dep/Dep.swiftmodule"));
    assert!(action
        .inputs
        .iter()
        .any(|input| input == ".once/out/shared/swift/Dep/Dep.swiftdoc"));
    assert!(!action
        .inputs
        .iter()
        .any(|input| input == "shared/swift/Runtime/message.txt"));
    assert!(action
        .inputs
        .iter()
        .any(|input| input == ".once/out/shared/swift/libdep.so"));
    assert!(action
        .outputs
        .iter()
        .any(|output| output.ends_with("SharedSwift.swiftinterface")));
}

#[test]
fn prelude_swift_android_native_libraries_skip_empty_records() {
    let prelude = all_prelude_source();
    let out = eval_prelude_function_in(
        prelude,
        "_unique_native_libraries",
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
fn prelude_c_collect_linkopts_preserves_paired_flags() {
    let prelude = all_prelude_source();
    let out = eval_prelude_function_in(
        prelude,
        "_c_collect_linkopts",
        r#"([
            {"transitive_linkopts": ["-framework", "Foundation", "-framework", "CoreData"]},
        ], [])"#,
    )
    .unwrap();

    assert_eq!(
        out,
        "[\"-framework\", \"Foundation\", \"-framework\", \"CoreData\"]"
    );
}

#[test]
fn prelude_zig_collect_linkopts_preserves_paired_flags() {
    let prelude = all_prelude_source();
    let out = eval_prelude_function_in(
        prelude,
        "_zig_collect_linkopts",
        r#"([
            {"c_provider": True, "transitive_linkopts": ["-framework", "Foundation", "-framework", "CoreData"]},
        ], [])"#,
    )
    .unwrap();

    assert_eq!(
        out,
        "[\"-framework\", \"Foundation\", \"-framework\", \"CoreData\"]"
    );
}

#[test]
fn prelude_swift_android_collect_linkopts_preserves_paired_flags() {
    let prelude = all_prelude_source();
    let out = eval_prelude_function_in(
        prelude,
        "_swift_android_collect_linkopts",
        r#"([
            {"transitive_linkopts": ["-Xlinker", "-rpath", "-Xlinker", "/a"]},
        ], ["-framework", "Foundation", "-framework", "CoreData"])"#,
    )
    .unwrap();

    assert_eq!(
        out,
        "[\"-Xlinker\", \"-rpath\", \"-Xlinker\", \"/a\", \"-framework\", \"Foundation\", \"-framework\", \"CoreData\"]"
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

def host_command(argv, env = None, merge_stderr = None):
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
fn prelude_kotlin_jvm_library_separates_compile_and_runtime_roles() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let source_dir = workspace.path().join("apps/hello/src");
    let kotlin_dir = workspace.path().join("toolchains/kotlin/lib");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::create_dir_all(&kotlin_dir).unwrap();
    std::fs::write(
        source_dir.join("Greeting.kt"),
        "package dev.once.hello\nfun greeting() = \"hello\"\n",
    )
    .unwrap();
    let stdlib = kotlin_dir.join("kotlin-stdlib.jar");
    std::fs::write(&stdlib, "stdlib").unwrap();
    let stdlib = stdlib.to_string_lossy();
    let source = format!(
        r#"{prelude}
def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/tools/kotlinc", "-version"]:
        return "kotlinc 2.4.0\n"
    fail("unexpected host_command: " + repr(argv))

ctx = {{
    "label": {{"package": "apps/hello", "name": "Greeting", "id": "apps/hello/Greeting"}},
    "attr": {{
        "kotlinc": "/tools/kotlinc",
        "kotlin_stdlib": "{stdlib}",
        "module_name": "greeting",
    }},
    "deps": [{{
        "label_id": "libs/core",
        "transitive_compile_jars": [".once/out/libs/core/Core.jar"],
        "transitive_runtime_jars": [".once/out/libs/core/Core.jar"],
    }}],
    "deps_by_role": {{
        "deps": [],
        "associates": [{{
            "label_id": "libs/friend",
            "transitive_compile_jars": [".once/out/libs/friend/Friend.jar"],
            "transitive_runtime_jars": [".once/out/libs/friend/Friend.jar"],
        }}],
        "exported_deps": [{{
            "label_id": "libs/exported",
            "transitive_compile_jars": [".once/out/libs/exported/Exported.jar"],
            "transitive_runtime_jars": [".once/out/libs/exported/Exported.jar"],
        }}],
        "provided_deps": [{{
            "label_id": "libs/provided",
            "transitive_compile_jars": [".once/out/libs/provided/Provided.jar"],
            "transitive_runtime_jars": [".once/out/libs/provided/Provided.jar"],
        }}],
        "runtime_deps": [{{
            "label_id": "libs/runtime",
            "transitive_compile_jars": [".once/out/libs/runtime/Runtime.jar"],
            "transitive_runtime_jars": [".once/out/libs/runtime/Runtime.jar"],
        }}],
    }},
    "srcs": ["src/**/*.kt"],
    "build_dir": ".once/out/apps/hello/Greeting",
    "scratch_dir": ".once/tmp/analysis/apps/hello/Greeting",
    "capability": "build",
}}
result = repr(_kotlin_jvm_library_impl(ctx))
"#
    );
    let store = store_for(workspace.path(), "apps/hello");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    assert!(out.contains("libs/runtime/Runtime.jar"), "{out}");
    assert!(!out.contains("libs/provided/Provided.jar\", \".once/out/libs/runtime"));
    let compile = action_by_identifier(&store, "kotlin_jvm_compile:apps/hello/Greeting");
    let classpath = compile
        .argv
        .iter()
        .position(|arg| arg == "-classpath")
        .map(|index| compile.argv[index + 1].as_str())
        .expect("compile classpath");
    for jar in ["Core.jar", "Friend.jar", "Exported.jar", "Provided.jar"] {
        assert!(classpath.contains(jar), "{classpath}");
    }
    assert!(!classpath.contains("Runtime.jar"), "{classpath}");
    assert!(compile
        .argv
        .iter()
        .any(|arg| arg.starts_with("-Xfriend-paths=") && arg.contains("Friend.jar")));
}

#[test]
fn prelude_kotlin_jvm_binary_run_uses_runtime_role() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let kotlin_dir = workspace.path().join("toolchains/kotlin/lib");
    std::fs::create_dir_all(&kotlin_dir).unwrap();
    let stdlib = kotlin_dir.join("kotlin-stdlib.jar");
    std::fs::write(&stdlib, "stdlib").unwrap();
    let stdlib = stdlib.to_string_lossy();
    let source = format!(
        r#"{prelude}
def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/tools/kotlinc", "-version"]:
        return "kotlinc 2.4.0\n"
    if argv == ["/tools/java", "-version"]:
        return "java 17\n"
    fail("unexpected host_command: " + repr(argv))

ctx = {{
    "label": {{"package": "apps/hello", "name": "Hello", "id": "apps/hello/Hello"}},
    "attr": {{
        "kotlinc": "/tools/kotlinc",
        "java": "/tools/java",
        "kotlin_stdlib": "{stdlib}",
        "main_class": "dev.once.hello.MainKt",
        "args": ["Once"],
    }},
    "deps": [],
    "deps_by_role": {{
        "deps": [],
        "runtime_deps": [{{
            "transitive_runtime_jars": [".once/out/libs/runtime/Runtime.jar"],
        }}],
    }},
    "srcs": [],
    "build_dir": ".once/out/apps/hello/Hello",
    "scratch_dir": ".once/tmp/analysis/apps/hello/Hello",
    "capability": "run",
}}
result = repr(_kotlin_jvm_binary_impl(ctx))
"#
    );
    let store = store_for(workspace.path(), "apps/hello");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert!(out.unwrap().contains("Runtime.jar"));
    let run = action_by_identifier(&store, "kotlin_jvm_run:apps/hello/Hello");
    assert_eq!(run.argv[0], "/tools/java");
    assert!(run.argv.iter().any(|arg| arg == "dev.once.hello.MainKt"));
    assert!(run.argv.iter().any(|arg| arg == "Once"));
    assert!(run.argv.iter().any(|arg| arg.contains("Runtime.jar")));
}

#[test]
fn prelude_kotlin_jvm_test_emits_test_info_and_uses_runtime_role() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let source_dir = workspace.path().join("apps/hello/src/test");
    let kotlin_dir = workspace.path().join("toolchains/kotlin/lib");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::create_dir_all(&kotlin_dir).unwrap();
    std::fs::write(
        source_dir.join("GreetingTest.kt"),
        "class GreetingTest { fun testGreeting() { check(true) } }\n",
    )
    .unwrap();
    let stdlib = kotlin_dir.join("kotlin-stdlib.jar");
    std::fs::write(&stdlib, "stdlib").unwrap();
    let stdlib = stdlib.to_string_lossy();
    let source = format!(
        r#"{prelude}
def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/tools/kotlinc", "-version"]:
        return "kotlinc 2.4.0\n"
    if argv == ["/tools/java", "-version"]:
        return "java 17\n"
    if argv == ["/tools/javac", "-version"]:
        return "javac 17\n"
    fail("unexpected host_command: " + repr(argv))

ctx = {{
    "label": {{"package": "apps/hello", "name": "GreetingTests", "id": "apps/hello/GreetingTests"}},
    "attr": {{
        "kotlinc": "/tools/kotlinc",
        "java": "/tools/java",
        "javac": "/tools/javac",
        "kotlin_stdlib": "{stdlib}",
        "labels": ["unit"],
        "test_class": "GreetingTest#testGreeting",
    }},
    "deps": [{{
        "transitive_compile_jars": [".once/out/libs/core/Core.jar"],
        "transitive_runtime_jars": [".once/out/libs/core/Core.jar"],
    }}],
    "deps_by_role": {{
        "deps": [],
        "runtime_deps": [{{
            "transitive_runtime_jars": [".once/out/libs/runtime/Runtime.jar"],
        }}],
    }},
    "srcs": ["src/test/**/*.kt"],
    "build_dir": ".once/out/apps/hello/GreetingTests",
    "scratch_dir": ".once/tmp/analysis/apps/hello/GreetingTests",
    "capability": "test",
}}
result = repr(_kotlin_jvm_test_impl(ctx))
"#
    );
    let store = store_for(workspace.path(), "apps/hello");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    assert!(out.contains("once.test_info.v1"), "{out}");
    assert!(out.contains("kotlin_jvm"), "{out}");
    assert!(out.contains("unit"), "{out}");
    let compile = action_by_identifier(&store, "kotlin_jvm_test_compile:apps/hello/GreetingTests");
    let classpath = compile
        .argv
        .iter()
        .position(|arg| arg == "-classpath")
        .map(|index| compile.argv[index + 1].as_str())
        .expect("compile classpath");
    assert!(classpath.contains("Core.jar"), "{classpath}");
    assert!(!classpath.contains("Runtime.jar"), "{classpath}");
    let runner_compile = action_by_identifier(
        &store,
        "kotlin_jvm_test_runner_compile:apps/hello/GreetingTests",
    );
    assert_eq!(runner_compile.argv[0], "/tools/javac");
    let run = action_by_identifier(&store, "kotlin_jvm_test:apps/hello/GreetingTests");
    assert_eq!(run.argv[0], "/tools/java");
    assert!(run.argv.iter().any(|arg| arg == "OnceJvmTestRunner"));
    assert!(run.argv.iter().any(|arg| arg == "kotlin_jvm"));
    assert!(run.argv.iter().any(|arg| arg.contains("Runtime.jar")));
    assert!(run
        .argv
        .iter()
        .any(|arg| arg == "GreetingTest#testGreeting"));
}

#[test]
fn prelude_android_rejects_rust_rlib_native_dep() {
    let prelude = all_prelude_source();
    let err = eval_prelude_function_in(
        prelude,
        "_android_native_libraries",
        r#"({
            "label": {"id": "AndroidApp"}
        }, [
            {
                "target_kind": "rust_library",
                "label_id": "SharedRust",
                "crate_type": "rlib",
                "rlib": ".once/out/libshared.rlib",
            },
        ])"#,
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
    let fake_linker = fake_ndk
        .join("toolchains/llvm/prebuilt")
        .join(android_ndk_prebuilt_tag())
        .join("bin/aarch64-linux-android23-clang");
    let fake_linker_arg = format!("linker={}", fake_linker.to_string_lossy());
    let fake_ndk = fake_ndk.to_string_lossy();
    let fake_linker = fake_linker.to_string_lossy();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "rustc":
        return "/toolchains/rust/bin/rustc"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
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
fn prelude_rust_compile_accepts_parity_aliases_and_inputs() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let src_dir = workspace.path().join("pkg/src");
    let compile_data_dir = workspace.path().join("pkg/compile-data");
    let runtime_data_dir = workspace.path().join("pkg/runtime-data");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&compile_data_dir).unwrap();
    std::fs::create_dir_all(&runtime_data_dir).unwrap();
    std::fs::write(src_dir.join("lib.rs"), "pub fn value() -> u8 { 1 }\n").unwrap();
    std::fs::write(compile_data_dir.join("schema.txt"), "schema\n").unwrap();
    std::fs::write(runtime_data_dir.join("fixture.txt"), "fixture\n").unwrap();
    std::fs::write(workspace.path().join("pkg/layout.ld"), "SECTIONS {}\n").unwrap();
    std::fs::write(
        workspace.path().join("pkg/rust.env"),
        "FROM_FILE=file\nOVERRIDDEN=file\n",
    )
    .unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "rustc":
        return "/toolchains/rust/bin/rustc"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if len(argv) >= 3 and argv[1] == "--print" and argv[2] == "sysroot":
        return "/toolchains/rust\n"
    if len(argv) >= 2 and argv[1] == "--version":
        return "rustc test\nhost: x86_64-unknown-linux-gnu\n"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{
        "package": "pkg",
        "name": "lib",
        "id": "pkg/lib",
    }},
    "attr": {{
        "crate_root": "src/lib.rs",
        "target": "wasm32-unknown-unknown",
        "rustc_env_files": ["rust.env"],
        "rustc_env": {{"OVERRIDDEN": "explicit"}},
        "data": ["runtime-data/**"],
        "compile_data": ["compile-data/**"],
        "linker_script": "layout.ld",
        "aliases": {{"pkg/dep": "renamed_dep"}},
        "named_deps": {{"buck_dep": "pkg/buck"}},
        "exported_linker_flags": ["-Wl,--as-needed"],
        "exported_post_linker_flags": ["-Wl,--gc-sections"],
    }},
    "deps": [
        {{"label_id": "pkg/dep", "crate_name": "dep", "rlib": ".once/out/pkg/dep/libdep.rlib"}},
        {{"label_id": "pkg/buck", "crate_name": "buck", "rlib": ".once/out/pkg/buck/libbuck.rlib"}},
    ],
    "srcs": ["src/**/*.rs"],
    "build_dir": ".once/out/pkg/lib",
}}
provider = _rust_library_impl(ctx)
result = repr([provider["transitive_linkopts"], provider["transitive_data"]])
"#
    );
    let store = store_for(workspace.path(), "pkg");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(
        out.unwrap(),
        "[[\"-Wl,--as-needed\", \"-Wl,--gc-sections\"], [\"pkg/runtime-data/fixture.txt\"]]"
    );
    let action = action_by_identifier(&store, "pkg/lib:rustc");
    assert!(action
        .argv
        .iter()
        .any(|arg| arg == "renamed_dep=.once/out/pkg/dep/libdep.rlib"));
    assert!(action
        .argv
        .iter()
        .any(|arg| arg == "buck_dep=.once/out/pkg/buck/libbuck.rlib"));
    assert!(action
        .inputs
        .iter()
        .any(|input| input == "pkg/compile-data/schema.txt"));
    assert!(!action
        .inputs
        .iter()
        .any(|input| input == "pkg/runtime-data/fixture.txt"));
    assert!(action.inputs.iter().any(|input| input == "pkg/rust.env"));
    assert!(action.inputs.iter().any(|input| input == "pkg/layout.ld"));
    assert!(action
        .argv
        .iter()
        .any(|arg| arg == "link-arg=-Tpkg/layout.ld"));
    assert_eq!(
        action.env.get("FROM_FILE").map(String::as_str),
        Some("file")
    );
    assert_eq!(
        action.env.get("OVERRIDDEN").map(String::as_str),
        Some("explicit")
    );
}

#[cfg(unix)]
#[test]
fn prelude_rust_mobile_library_android_consumer_declares_only_android_variant() {
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
    let fake_linker = fake_ndk
        .join("toolchains/llvm/prebuilt")
        .join(android_ndk_prebuilt_tag())
        .join("bin/aarch64-linux-android24-clang");
    let fake_linker_arg = format!("linker={}", fake_linker.to_string_lossy());
    let fake_ndk = fake_ndk.to_string_lossy();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "rustc":
        return "/toolchains/rust/bin/rustc"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if len(argv) >= 3 and argv[1] == "--print" and argv[2] == "sysroot":
        return "/toolchains/rust\n"
    if len(argv) >= 2 and argv[1] == "--version":
        return "rustc test\nhost: x86_64-unknown-linux-gnu\n"
    fail("unexpected host_command: " + str(argv))

mobile_ctx = {{
    "label": {{
        "package": "",
        "name": "SharedRust",
        "id": "SharedRust",
    }},
    "attr": {{
        "crate_name": "shared_rust",
        "crate_root": "shared/rust/src/lib.rs",
        "apple_target": "aarch64-apple-ios-sim",
        "android_target": "aarch64-linux-android",
        "android_abi": "arm64-v8a",
        "android_api": 24,
        "android_ndk": "{fake_ndk}",
        "native_linkopts": ["-lc++"],
    }},
    "deps": [],
    "srcs": ["shared/rust/src/**/*.rs"],
    "build_dir": ".once/out/SharedRust",
}}
provider = _rust_mobile_library_impl(mobile_ctx)
android_ctx = {{
    "label": {{
        "package": "",
        "name": "AndroidApp",
        "id": "AndroidApp",
    }},
    "attr": {{}},
    "deps": [provider],
    "srcs": [],
    "build_dir": ".once/out/AndroidApp",
    "scratch_dir": ".once/tmp/analysis/AndroidApp",
}}
android_libraries = _android_native_libraries(android_ctx, android_ctx["deps"])
result = repr([
    provider["label_id"],
    provider["target_kind"],
    provider["transitive_sources"],
    android_libraries,
])
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        String::new(),
        ".once/out/AndroidApp".to_string(),
    );

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    assert!(out.contains("SharedRust"), "{out}");
    assert!(out.contains("rust_mobile_library"), "{out}");
    assert!(
        out.contains("rust-mobile/SharedRust/android/libshared_rust.so"),
        "{out}"
    );
    assert!(out.contains("arm64-v8a"), "{out}");
    let android = action_by_identifier(&store, "SharedRust:rustc:android");
    assert_eq!(store.actions.len(), 1);
    assert!(android
        .outputs
        .iter()
        .any(|output| output.ends_with("rust-mobile/SharedRust/android/libshared_rust.so")));
    assert!(android.argv.iter().any(|arg| arg == &fake_linker_arg));
}

#[cfg(unix)]
#[test]
fn prelude_rust_mobile_library_apple_consumer_declares_only_apple_variant() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("shared/rust/src");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(package_dir.join("lib.rs"), "pub fn greeting() {}\n").unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "rustc":
        return "/toolchains/rust/bin/rustc"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if len(argv) >= 3 and argv[1] == "--print" and argv[2] == "sysroot":
        return "/toolchains/rust\n"
    if len(argv) >= 2 and argv[1] == "--version":
        return "rustc test\nhost: x86_64-unknown-linux-gnu\n"
    fail("unexpected host_command: " + str(argv))

mobile_ctx = {{
    "label": {{
        "package": "",
        "name": "SharedRust",
        "id": "SharedRust",
    }},
    "attr": {{
        "crate_name": "shared_rust",
        "crate_root": "shared/rust/src/lib.rs",
        "apple_target": "aarch64-apple-ios-sim",
        "android_target": "aarch64-linux-android",
    }},
    "deps": [],
    "srcs": ["shared/rust/src/**/*.rs"],
    "build_dir": ".once/out/SharedRust",
}}
provider = _rust_mobile_library_impl(mobile_ctx)
apple_ctx = {{
    "label": {{
        "package": "",
        "name": "AppleApp",
        "id": "AppleApp",
    }},
    "attr": {{}},
    "deps": [provider],
    "srcs": [],
    "build_dir": ".once/out/AppleApp",
    "scratch_dir": ".once/tmp/analysis/AppleApp",
}}
apple_provider = _apple_native_deps(apple_ctx)[0]
result = repr([
    apple_provider["archive"],
    apple_provider["transitive_archives"],
])
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        String::new(),
        ".once/out/AppleApp".to_string(),
    );

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    assert!(
        out.contains("rust-mobile/SharedRust/apple/libshared_rust.a"),
        "{out}"
    );
    let apple = action_by_identifier(&store, "SharedRust:rustc:apple");
    assert_eq!(store.actions.len(), 1);
    assert!(apple
        .outputs
        .iter()
        .any(|output| output.ends_with("rust-mobile/SharedRust/apple/libshared_rust.a")));
}

#[cfg(unix)]
#[test]
fn prelude_rust_mobile_library_materializes_transitive_deps_once_for_android() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    for name in ["core", "left", "right", "root"] {
        let dir = workspace.path().join(format!("mobile/{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lib.rs"), "pub fn value() -> i32 { 1 }\n").unwrap();
    }
    let fake_ndk = fake_android_ndk_for_mobile_test(workspace.path());
    let fake_ndk = fake_ndk.to_string_lossy();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "rustc":
        return "/toolchains/rust/bin/rustc"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if len(argv) >= 3 and argv[1] == "--print" and argv[2] == "sysroot":
        return "/toolchains/rust\n"
    if len(argv) >= 2 and argv[1] == "--version":
        return "rustc test\nhost: x86_64-unknown-linux-gnu\n"
    fail("unexpected host_command: " + str(argv))

def mobile(name, deps):
    lower = name.lower()
    return _rust_mobile_library_impl({{
        "label": {{"package": "", "name": name, "id": name}},
        "attr": {{
            "crate_name": lower,
            "crate_root": "mobile/" + lower + "/lib.rs",
            "apple_target": "aarch64-apple-ios-sim",
            "android_target": "aarch64-linux-android",
            "android_abi": "arm64-v8a",
            "android_api": 24,
            "android_ndk": "{fake_ndk}",
        }},
        "deps": deps,
        "srcs": ["mobile/" + lower + "/**/*.rs"],
        "build_dir": ".once/out/" + name,
    }})

core = mobile("Core", [])
left = mobile("Left", [core])
right = mobile("Right", [core])
root = mobile("Root", [left, right])
consumer = {{
    "label": {{"package": "", "name": "AndroidApp", "id": "AndroidApp"}},
    "attr": {{}},
    "deps": [root],
    "srcs": [],
    "build_dir": ".once/out/AndroidApp",
    "scratch_dir": ".once/tmp/analysis/AndroidApp",
}}
result = repr(_android_native_libraries(consumer, consumer["deps"]))
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        String::new(),
        ".once/out/AndroidApp".to_string(),
    );

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    assert!(out.contains("rust-mobile/Root/android/libroot.so"), "{out}");
    assert_eq!(store.actions.len(), 4);
    let core_actions = store
        .actions
        .iter()
        .filter(|action| action.identifier.as_deref() == Some("Core:rustc:android"))
        .count();
    assert_eq!(core_actions, 1);
    let left = action_by_identifier(&store, "Left:rustc:android");
    assert!(left.argv.iter().any(|arg| {
        arg.starts_with("core=") && arg.contains("rust-mobile/Core/android/libcore-CORE.rlib")
    }));
    let root = action_by_identifier(&store, "Root:rustc:android");
    assert!(root.argv.iter().any(|arg| arg.starts_with("left=")
        && Path::new(arg)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rlib"))));
    assert!(root.argv.iter().any(|arg| arg.starts_with("right=")
        && Path::new(arg)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rlib"))));
}

#[test]
fn prelude_rust_mobile_library_carries_resolved_auxiliary_inputs() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("shared/rust");
    std::fs::create_dir_all(package_dir.join("src")).unwrap();
    std::fs::create_dir_all(package_dir.join("runtime")).unwrap();
    std::fs::create_dir_all(package_dir.join("compile")).unwrap();
    std::fs::write(package_dir.join("src/lib.rs"), "pub fn greeting() {}\n").unwrap();
    std::fs::write(package_dir.join("runtime/message.txt"), "hello\n").unwrap();
    std::fs::write(package_dir.join("compile/schema.txt"), "schema\n").unwrap();
    std::fs::write(package_dir.join("rust.env"), "FROM_FILE=value\n").unwrap();
    let source = format!(
        r#"{prelude}
mobile_ctx = {{
    "label": {{
        "package": "shared/rust",
        "name": "SharedRust",
        "id": "shared/rust/SharedRust",
    }},
    "attr": {{
        "apple_target": "aarch64-apple-ios-sim",
        "android_target": "aarch64-linux-android",
        "data": ["runtime/**"],
        "compile_data": ["compile/**"],
        "rustc_env_files": ["rust.env"],
    }},
    "deps": [],
    "srcs": ["src/**/*.rs"],
    "build_dir": ".once/out/shared/rust/SharedRust",
}}
provider = _rust_mobile_library_impl(mobile_ctx)
consumer_ctx = {{
    "build_dir": ".once/out/apps/android/App",
    "scratch_dir": ".once/tmp/analysis/apps/android/App",
}}
variant = _rust_mobile_variant_ctx(consumer_ctx, provider, "android_target", "android")
result = repr([
    variant["attr"]["_resolved_data_inputs"],
    variant["attr"]["_resolved_compile_data_inputs"],
    variant["attr"]["_resolved_env_file_inputs"],
    provider["transitive_data"],
])
"#
    );
    let store = store_for(workspace.path(), "shared/rust");

    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(
        out.unwrap(),
        "[[\"shared/rust/runtime/message.txt\"], [\"shared/rust/compile/schema.txt\"], [\"shared/rust/rust.env\"], [\"shared/rust/runtime/message.txt\"]]"
    );
}

#[test]
fn prelude_rust_mobile_library_carries_mobile_deps() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "shared/rust",
        "name": "SharedRust",
        "id": "shared/rust/SharedRust",
    }},
    "attr": {{
        "apple_target": "aarch64-apple-ios-sim",
        "android_target": "aarch64-linux-android",
    }},
    "deps": [{{
        "target_kind": "rust_mobile_library",
        "label_id": "shared/rust/Core",
        "transitive_sources": ["shared/rust/core/lib.rs"],
        "transitive_data": ["shared/rust/core/data.txt"],
    }}],
    "srcs": ["src/**/*.rs"],
    "build_dir": ".once/out/shared/rust/SharedRust",
}}
provider = _rust_mobile_library_impl(ctx)
result = repr([
    provider["mobile_deps"][0]["label_id"],
    provider["transitive_sources"],
    provider["transitive_data"],
])
"#
    );

    let out = eval_prelude_source_to_repr(source).unwrap();

    assert!(out.contains("shared/rust/Core"), "{out}");
    assert!(out.contains("shared/rust/core/lib.rs"), "{out}");
    assert!(out.contains("shared/rust/core/data.txt"), "{out}");
}

#[cfg(unix)]
#[test]
fn prelude_rust_binary_links_transitive_c_provider_inputs() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let source_dir = workspace.path().join("crates/app/src");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(source_dir.join("main.rs"), "fn main() {}\n").unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "rustc":
        return "/toolchains/rust/bin/rustc"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if len(argv) >= 3 and argv[1] == "--print" and argv[2] == "sysroot":
        return "/toolchains/rust\n"
    if len(argv) >= 2 and argv[1] == "--version":
        return "rustc test\nhost: x86_64-unknown-linux-gnu\n"
    fail("unexpected host_command: " + str(argv))

def _rust_c_tool_env(target, host_triple):
    return {{}}

ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{
        "crate_root": "src/main.rs",
        "linker": "/usr/bin/cc",
    }},
    "deps": [],
    "deps_by_role": {{
        "deps": [],
        "link_deps": [{{
            "c_provider": True,
            "label_id": "native/math",
            "transitive_static_libraries": [".once/out/native/math/libmath.a"],
            "transitive_dynamic_libraries": ["vendor/libsupport.so"],
            "transitive_linkopts": ["-pthread"],
            "transitive_archives": [".once/out/native/math/libmath.a"],
        }}],
    }},
    "srcs": ["src/**/*.rs"],
    "build_dir": ".once/out/crates/app/app",
    "capability": "build",
}}
provider = _rust_binary_impl(ctx)
result = repr(provider)
"#
    );
    let store = store_for(workspace.path(), "crates/app");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    assert!(out.contains("libmath.a"), "{out}");
    let action = action_by_identifier(&store, "crates/app/app:rustc");
    for value in [
        ".once/out/native/math/libmath.a",
        "vendor/libsupport.so",
        "-pthread",
    ] {
        assert!(
            action
                .argv
                .iter()
                .any(|arg| arg == &format!("link-arg={value}")),
            "{:?}",
            action.argv
        );
    }
    assert!(action
        .inputs
        .iter()
        .any(|input| input == ".once/out/native/math/libmath.a"));
    assert!(action
        .inputs
        .iter()
        .any(|input| input == "vendor/libsupport.so"));
}

#[test]
fn prelude_rust_resolves_proc_macro_dependency_role() {
    let source = format!(
        r#"{}
ctx = {{
    "label": {{"package": "crates/app", "name": "app", "id": "crates/app/app"}},
    "attr": {{}},
    "deps": [{{"label_id": "crates/core", "crate_name": "core"}}],
    "deps_by_role": {{
        "deps": [{{"label_id": "crates/core", "crate_name": "core"}}],
        "proc_macro_deps": [{{
            "label_id": "crates/derive",
            "crate_name": "derive",
            "proc_macro": ".once/out/crates/derive/libderive.so",
        }}],
    }},
}}
result = repr([dep["label_id"] for dep in _rust_resolved_deps(ctx)])
"#,
        all_prelude_source()
    );

    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(out, "[\"crates/core\", \"crates/derive\"]");
}

#[cfg(unix)]
#[test]
fn prelude_rust_test_declares_libtest_binary_and_runner() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("crates/app/tests");
    let fixture_dir = workspace.path().join("crates/app/fixtures");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::create_dir_all(&fixture_dir).unwrap();
    std::fs::write(
        package_dir.join("greeting_test.rs"),
        "#[test]\nfn test_greeting() {}\n",
    )
    .unwrap();
    std::fs::write(fixture_dir.join("greeting.txt"), "hello\n").unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "rustc":
        return "/toolchains/rust/bin/rustc"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if len(argv) >= 3 and argv[1] == "--print" and argv[2] == "sysroot":
        return "/toolchains/rust\n"
    if len(argv) >= 2 and argv[1] == "--version":
        return "rustc test\nhost: x86_64-unknown-linux-gnu\n"
    fail("unexpected host_command: " + str(argv))

def _rust_c_tool_env(target, host_triple):
    return {{}}

ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app_tests",
        "id": "crates/app/app_tests",
    }},
    "attr": {{
        "crate_name": "app_tests",
        "crate_root": "tests/greeting_test.rs",
        "edition": "2021",
        "linker": "/usr/bin/cc",
        "data": ["fixtures/**"],
        "labels": ["unit"],
    }},
    "deps": [],
    "srcs": ["tests/**/*.rs"],
    "build_dir": ".once/out/crates/app/app_tests",
    "capability": "test",
}}
provider = _rust_test_impl(ctx)
result = repr(provider["test_info"])
"#
    );
    let store = store_for(workspace.path(), "crates/app");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    assert!(out.contains("rust_libtest"), "{out}");
    assert!(out.contains("unit"), "{out}");
    let rustc = action_by_identifier(&store, "crates/app/app_tests:rustc");
    assert!(rustc.argv.iter().any(|arg| arg == "--test"));
    assert!(rustc
        .inputs
        .iter()
        .any(|input| input == "crates/app/tests/greeting_test.rs"));
    let runner_compile = action_by_identifier(&store, "crates/app/app_tests:test-runner-rustc");
    assert!(runner_compile
        .inputs
        .iter()
        .any(|input| input.ends_with("OnceRustTestRunner.rs")));
    let run = action_by_identifier(&store, "crates/app/app_tests:test");
    assert!(run
        .argv
        .iter()
        .any(|arg| arg.ends_with("test/test_results.json")));
    assert!(run
        .outputs
        .iter()
        .any(|output| output.ends_with("test/rust-libtest.log")));
    assert!(run
        .inputs
        .iter()
        .any(|input| input == "crates/app/fixtures/greeting.txt"));
}

#[test]
fn prelude_rust_binary_run_declares_runtime_data_and_environment() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let data_dir = workspace.path().join("crates/app/data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("message.txt"), "hello\n").unwrap();
    let source = format!(
        r#"{prelude}
def host_os():
    return "linux"

def host_env(name):
    if name == "RUST_RUN_HOST":
        return "host-value"
    return ""

ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{
        "args": ["--message", "data/message.txt"],
        "data": ["data/**"],
        "env_inherit": ["RUST_RUN_HOST"],
        "run_env": {{"RUST_RUN_EXPLICIT": "explicit-value"}},
    }},
    "deps": [{{"transitive_data": ["shared/config.json"]}}],
    "srcs": ["src/**/*.rs"],
    "build_dir": ".once/out/crates/app/app",
    "capability": "run",
}}
provider = _rust_binary_impl(ctx)
result = repr(provider["transitive_data"])
"#
    );
    let store = store_for(workspace.path(), "crates/app");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(
        out.unwrap(),
        "[\"crates/app/data/message.txt\", \"shared/config.json\"]"
    );
    let run = action_by_identifier(&store, "crates/app/app:run");
    assert_eq!(run.argv[0], ".once/out/crates/app/app/app");
    assert!(run.argv.iter().any(|arg| arg == "--message"));
    assert!(run
        .inputs
        .iter()
        .any(|input| input == "crates/app/data/message.txt"));
    assert!(run.inputs.iter().any(|input| input == "shared/config.json"));
    assert_eq!(
        run.env.get("RUST_RUN_HOST").map(String::as_str),
        Some("host-value")
    );
    assert_eq!(
        run.env.get("RUST_RUN_EXPLICIT").map(String::as_str),
        Some("explicit-value")
    );
    assert_eq!(
        run.stdout.as_deref(),
        Some(".once/out/crates/app/app/run/stdout.log")
    );
    let marker = action_by_identifier(&store, "write_path:.once/out/crates/app/app/run/run.json");
    assert!(matches!(
        marker.operation,
        Some(DeclaredActionOperation::WriteFile { .. })
    ));
}

#[cfg(unix)]
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the inline Starlark fixture keeps this action contract in one test"
)]
fn prelude_apple_application_embeds_framework_self_path_output() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("app/Sources");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(package_dir.join("App.swift"), "import Shared\n").unwrap();
    std::fs::write(
        package_dir.join("Legacy.m"),
        "#import \"App-Swift.h\"\nvoid legacy(void) {}\n",
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("app/App.entitlements"),
        "<plist><dict><key>com.apple.security.application-groups</key><array><string>$(APP_GROUP)</string></array></dict></plist>",
    )
    .unwrap();
    std::fs::create_dir_all(workspace.path().join("shared/include")).unwrap();
    std::fs::write(
        workspace.path().join("shared/include/Shared.h"),
        "void shared(void);\n",
    )
    .unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "xcrun":
        return "/usr/bin/xcrun"
    if name == "codesign":
        return "/usr/bin/codesign"
    if name == "sh":
        return "/bin/sh"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if "--find" in argv:
        return "/toolchain/" + argv[len(argv) - 1] + "\n"
    if "--show-sdk-path" in argv:
        return "/sdks/iPhoneSimulator.sdk\n"
    if "--version" in argv:
        return "Swift version test\n"
    if "-print-resource-dir" in argv:
        return "/toolchain/resource\n"
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
        "enable_testing": True,
        "defines": ["DEBUG"],
        "private_header_dirs": ["Sources"],
        "entitlements": "App.entitlements",
        "entitlements_substitutions": {{"APP_GROUP": "group.dev.once.App"}},
        "development_team": "TEAM123",
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
        "transitive_archives": [".once/out/shared/Shared.a"],
        "transitive_alwayslink_archives": [".once/out/shared/Shared.a"],
        "absorbed_static_archives": [".once/out/shared/Shared.a"],
        "transitive_exported_header_dirs": ["/toolchain/include", "shared/include"],
        "transitive_modulemaps": [".once/out/shared/module.modulemap"],
        "transitive_hmaps": [".once/out/shared/Shared.hmap"],
        "transitive_framework_search_dirs": ["/toolchain/frameworks"],
        "transitive_generated_headers": [
            ".once/out/shared/Headers/Shared/Shared-Swift.h",
        ],
        "transitive_exported_headers": ["shared/include/Shared.h"],
        "transitive_vfs_overlays": [
            ".once/out/shared/framework-headers-overlay.yaml",
        ],
        "transitive_resource_bundles": [{{
                "path": ".once/out/shared/SharedResources.bundle",
                "files": [
                    ".once/out/shared/SharedResources.bundle/Info.plist",
                    ".once/out/shared/SharedResources.bundle/message.json",
                ],
                "label_id": "shared/Shared",
        }}],
    }}],
    "srcs": ["Sources/**/*.swift", "Sources/**/*.m"],
    "build_dir": ".once/out/app/App",
    "capability": "build",
}}
provider = _apple_application_impl(ctx)
result = repr([
    provider["app_path"],
    provider["transitive_exported_header_dirs"],
    provider["transitive_modulemaps"],
    provider["transitive_hmaps"],
])
"#
    );
    let store = store_for(workspace.path(), "app");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    assert!(out.contains("App.app"), "{out}");
    assert!(out.contains("app/Sources"), "{out}");
    assert!(out.contains(".once/out/shared/module.modulemap"), "{out}");
    assert!(out.contains(".once/out/shared/Shared.hmap"), "{out}");
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
    let generated_header = ".once/out/shared/Headers/Shared/Shared-Swift.h";
    let compile = action_by_identifier(&store, "apple_application_compile_App");
    assert!(compile.inputs.iter().any(|input| input == generated_header));
    assert!(compile
        .inputs
        .iter()
        .any(|input| input == ".once/out/shared/module.modulemap"));
    assert!(compile
        .inputs
        .iter()
        .any(|input| input == ".once/out/shared/Shared.hmap"));
    assert!(compile.argv.windows(4).any(|args| {
        args == [
            "-Xlinker",
            "-force_load",
            "-Xlinker",
            ".once/out/shared/Shared.a",
        ]
    }));
    let module = action_by_identifier(&store, "apple_application_module_App");
    assert!(module.inputs.iter().any(|input| input == generated_header));
    assert!(module
        .inputs
        .iter()
        .any(|input| input == "shared/include/Shared.h"));
    assert!(!module.inputs.iter().any(|input| input == "shared/include"));
    assert!(!module.inputs.iter().any(|input| input.starts_with('/')));
    assert!(
        module.argv.windows(2).any(|args| {
            args == [
                "-emit-objc-header-path".to_string(),
                ".once/out/app/App-Swift.h".to_string(),
            ]
        }),
        "{:?}",
        module.argv
    );
    assert!(module
        .outputs
        .iter()
        .any(|output| output == ".once/out/app/App-Swift.h"));
    let clang = action_by_identifier(
        &store,
        "apple_application_clang_compile_App_app_Sources_Legacy.m",
    );
    assert!(clang
        .inputs
        .iter()
        .any(|input| input == ".once/out/app/App-Swift.h"));
    assert!(clang.argv.iter().any(|arg| arg == "-DDEBUG"));
    let legacy_object = ".once/out/app/Objects/app_Sources_Legacy.m.o";
    assert!(
        compile.inputs.iter().any(|input| input == legacy_object),
        "{:?}",
        compile.inputs
    );
    assert!(
        compile.argv.iter().any(|arg| arg == legacy_object),
        "{:?}",
        compile.argv
    );
    assert!(compile.argv.windows(4).any(|args| {
        args == [
            "-Xlinker".to_string(),
            "-force_load".to_string(),
            "-Xlinker".to_string(),
            ".once/out/shared/Shared.a".to_string(),
        ]
    }));
    let overlay = ".once/out/shared/framework-headers-overlay.yaml";
    assert!(compile.inputs.iter().any(|input| input == overlay));
    assert!(module.inputs.iter().any(|input| input == overlay));
    assert!(compile.argv.windows(4).any(|args| {
        args == [
            "-Xcc".to_string(),
            "-ivfsoverlay".to_string(),
            "-Xcc".to_string(),
            overlay.to_string(),
        ]
    }));
    assert!(module.argv.windows(4).any(|args| {
        args == [
            "-Xcc".to_string(),
            "-ivfsoverlay".to_string(),
            "-Xcc".to_string(),
            overlay.to_string(),
        ]
    }));
    assert!(compile
        .argv
        .windows(2)
        .any(|args| { args == ["-Xcc".to_string(), "-DDEBUG".to_string()] }));
    assert!(module
        .argv
        .windows(2)
        .any(|args| { args == ["-Xcc".to_string(), "-DDEBUG".to_string()] }));
    assert!(compile.argv.windows(2).any(|args| {
        args == [
            "-module-cache-path".to_string(),
            ".once/out/app/App/ModuleCache/Compile".to_string(),
        ]
    }));
    assert!(module.argv.windows(2).any(|args| {
        args == [
            "-module-cache-path".to_string(),
            ".once/out/app/App/ModuleCache/TestableModule".to_string(),
        ]
    }));
    let codesign = action_by_identifier(&store, "apple_application_codesign_App");
    assert!(
        !codesign.argv.iter().any(|arg| arg == "--entitlements"),
        "{:?}",
        codesign.argv
    );
    assert!(compile.argv.windows(8).any(|args| {
        args == [
            "-Xlinker".to_string(),
            "-sectcreate".to_string(),
            "-Xlinker".to_string(),
            "__TEXT".to_string(),
            "-Xlinker".to_string(),
            "__entitlements".to_string(),
            "-Xlinker".to_string(),
            ".once/out/app/App/processed-entitlements.plist".to_string(),
        ]
    }));
    assert!(compile.argv.windows(8).any(|args| {
        args == [
            "-Xlinker".to_string(),
            "-sectcreate".to_string(),
            "-Xlinker".to_string(),
            "__TEXT".to_string(),
            "-Xlinker".to_string(),
            "__ents_der".to_string(),
            "-Xlinker".to_string(),
            ".once/out/app/App/processed-entitlements.der".to_string(),
        ]
    }));
    let der = action_by_identifier(&store, "apple_application_der_entitlements_App");
    assert_eq!(
        der.argv.first().map(String::as_str),
        Some("/toolchain/derq")
    );
    assert!(der
        .inputs
        .iter()
        .any(|input| input == ".once/out/app/App/processed-entitlements.plist"));
    let entitlements = action_by_identifier(
        &store,
        "write_path:.once/out/app/App/processed-entitlements.plist",
    );
    let Some(DeclaredActionOperation::WriteFile { bytes, .. }) = &entitlements.operation else {
        panic!("entitlements action must write its property list");
    };
    let entitlements = std::str::from_utf8(bytes).unwrap();
    assert!(entitlements.contains("group.dev.once.App"));
    assert!(entitlements.contains("<key>application-identifier</key>"));
    assert!(entitlements.contains("<string>TEAM123.dev.once.App</string>"));
    assert!(
        codesign
            .outputs
            .iter()
            .any(|output| output == ".once/out/app/App.app/App"),
        "{:?}",
        codesign.outputs
    );
    let plist = action_by_identifier(&store, "write_path:.once/out/app/App.app/Info.plist");
    let Some(DeclaredActionOperation::WriteFile { bytes, .. }) = &plist.operation else {
        panic!("expected generated application property list");
    };
    let plist = String::from_utf8(bytes.clone()).unwrap();
    assert!(plist.contains("<key>CFBundleSupportedPlatforms</key>"));
    assert!(plist.contains("<string>iPhoneSimulator</string>"));
    let resource_copy = action_by_identifier(
        &store,
        "apple_application_embed_resource_copy_SharedResources.bundle",
    );
    assert_eq!(
        resource_copy.outputs,
        [".once/out/app/App/App.app/SharedResources.bundle"]
    );
    assert!(resource_copy
        .inputs
        .iter()
        .any(|input| input.ends_with("SharedResources.bundle/message.json")));
    let resource_sign = action_by_identifier(
        &store,
        "apple_application_embed_resource_sign_SharedResources.bundle",
    );
    assert!(resource_sign
        .outputs
        .iter()
        .any(|output| output.ends_with("SharedResources.bundle/_CodeSignature/CodeResources")));
    assert!(codesign
        .inputs
        .iter()
        .any(|input| input.ends_with("SharedResources.bundle/_CodeSignature/CodeResources")));
}

#[cfg(unix)]
#[test]
fn prelude_apple_resource_bundle_propagates_declared_files() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    std::fs::create_dir_all(workspace.path().join("Resources")).unwrap();
    std::fs::write(
        workspace.path().join("Resources/PrivacyInfo.xcprivacy"),
        "<plist><dict/></plist>",
    )
    .unwrap();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "Privacy", "id": "Privacy"}},
    "attr": {{
        "platform": "ios",
        "minimum_os": "15.0",
        "sdk_variant": "simulator",
        "bundle_name": "PrivacyResources",
        "bundle_id": "dev.once.privacy",
        "resources": ["Resources/PrivacyInfo.xcprivacy"],
    }},
    "deps": [],
    "srcs": [],
    "build_dir": ".once/out/Privacy",
    "capability": "build",
}}
provider = _apple_resource_bundle_target_impl(ctx)
result = repr(provider["transitive_resource_bundles"])
"#
    );
    let store = store_for(workspace.path(), "");
    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let out = out.unwrap();
    assert!(out.contains("PrivacyResources.bundle"), "{out}");
    assert!(out.contains("PrivacyInfo.xcprivacy"), "{out}");
    assert!(store.actions.iter().any(|action| {
        action
            .identifier
            .as_deref()
            .is_some_and(|id| id.contains("apple_resource_bundle_PrivacyResources"))
    }));
}

#[cfg(unix)]
#[test]
fn prelude_apple_application_materializes_a_custom_property_list() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("app/Sources");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(package_dir.join("App.swift"), "import SwiftUI\n@main struct App: SwiftUI.App { var body: some Scene { WindowGroup { Text(\"Hello\") } } }\n").unwrap();
    std::fs::write(
        workspace.path().join("app/App-Info.plist"),
        "<plist><dict><key>CFBundleExecutable</key><string>$(EXECUTABLE_NAME)</string><key>CFBundleIdentifier</key><string>${PRODUCT_BUNDLE_IDENTIFIER}</string><key>UILaunchStoryboardName</key><string>LaunchScreen</string></dict></plist>",
    )
    .unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "xcrun":
        return "/usr/bin/xcrun"
    if name == "codesign":
        return "/usr/bin/codesign"
    if name == "sh":
        return "/bin/sh"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if "--find" in argv:
        return "/toolchain/" + argv[len(argv) - 1] + "\n"
    if "--show-sdk-path" in argv:
        return "/sdks/iPhoneSimulator.sdk\n"
    if "--version" in argv:
        return "Swift version test\n"
    if "-print-resource-dir" in argv:
        return "/toolchain/resource\n"
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
        "info_plist": "App-Info.plist",
        "info_plist_substitutions": {{
            "EXECUTABLE_NAME": "App",
            "PRODUCT_BUNDLE_IDENTIFIER": "dev.once.App",
        }},
    }},
    "deps": [],
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

    assert_eq!(out.unwrap(), r#"".once/out/app/App/App.app""#);
    let plist = action_by_identifier(&store, "write_path:.once/out/app/App.app/Info.plist");
    let Some(DeclaredActionOperation::WriteFile { bytes, .. }) = &plist.operation else {
        panic!("expected custom application property list");
    };
    let plist = std::str::from_utf8(bytes).unwrap();
    assert!(plist.contains("<string>App</string>"));
    assert!(plist.contains("<string>dev.once.App</string>"));
    assert!(plist.contains("<key>UILaunchStoryboardName</key>"));
    assert!(!plist.contains("$(EXECUTABLE_NAME)"));
    assert!(!plist.contains("${PRODUCT_BUNDLE_IDENTIFIER}"));
}

#[cfg(unix)]
fn assert_apple_thinning_adapter(store: &AnalysisStore) {
    let adapter = action_by_identifier(
        store,
        "write_path:.once/out/packages/AppThinned/apple-thinning-package.rb",
    );
    let Some(DeclaredActionOperation::WriteFile { bytes, .. }) = &adapter.operation else {
        panic!("expected generated thinning adapter");
    };
    let adapter = String::from_utf8(bytes.clone()).unwrap();
    assert!(adapter.contains("--create-thinned=#{options.fetch(:device)}"));
    assert!(adapter.contains("--validate-output-zero-variants"));
    assert!(adapter.contains("Open3.capture3(*ipatool_argv)"));
    assert!(adapter.contains("archive_entries.sort!"));
    assert!(adapter.contains("stdin_data: input"));
    assert!(adapter.contains("\"-X\""));
    assert!(!adapter.contains("puts _stdout"));
}

#[cfg(unix)]
fn assert_apple_thinning_package_action(store: &AnalysisStore) {
    let package = action_by_identifier(store, "apple_thinned_package:packages/AppThinned");
    assert_eq!(package.argv[0], "/usr/bin/ruby");
    assert!(package.argv.iter().any(|arg| arg == "iPhone17,1"));
    assert!(package.argv.iter().any(|arg| {
        arg == "/Applications/TestXcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr"
    }));
    assert!(package
        .argv
        .iter()
        .any(|arg| { arg == "/Applications/TestXcode.app/Contents/Developer/Platforms" }));
    assert!(package
        .outputs
        .iter()
        .any(|output| output.ends_with("/ipas")));
    assert!(package
        .outputs
        .iter()
        .any(|output| output.ends_with("/thinned-packages.json")));
    assert_eq!(package.env.get("TZ").map(String::as_str), Some("UTC"));
    assert_eq!(package.env.get("LC_ALL").map(String::as_str), Some("C"));
    assert_eq!(
        package.env.get("PATH").map(String::as_str),
        Some(
            "/Applications/TestXcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin:/usr/bin:/bin"
        )
    );
    let identity = package.toolchain_identity.as_deref().unwrap();
    assert!(identity.contains("Xcode 26.0"));
    assert!(identity.contains("\u{0}device\u{0}iPhone17,1"));
}

#[cfg(unix)]
#[test]
fn prelude_apple_thinned_package_stages_and_packages_one_device_application() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "xcrun":
        return "/usr/bin/xcrun"
    if name == "ruby":
        return "/usr/bin/ruby"
    if name == "zip":
        return "/usr/bin/zip"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/usr/bin/xcrun", "--find", "ipatool"]:
        return "/Applications/TestXcode.app/Contents/Developer/usr/bin/ipatool\n"
    if argv == ["/usr/bin/xcrun", "--find", "xcodebuild"]:
        return "/Applications/TestXcode.app/Contents/Developer/usr/bin/xcodebuild\n"
    if argv == ["/usr/bin/xcrun", "--find", "codesign"]:
        return "/usr/bin/codesign\n"
    if argv == ["/Applications/TestXcode.app/Contents/Developer/usr/bin/xcodebuild", "-version"]:
        return "Xcode 26.0\nBuild version 17A1\n"
    if argv == ["/usr/bin/ruby", "--version"]:
        return "ruby 2.6.10\n"
    if argv == ["/usr/bin/zip", "-v"]:
        return "Zip 3.0\n"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{
        "package": "packages",
        "name": "AppThinned",
        "id": "packages/AppThinned",
    }},
    "attr": {{"device_model": "iPhone17,1"}},
    "deps": [{{
        "label_id": "apps/App",
        "target_kind": "apple_application",
        "app_path": ".once/out/apps/App/App.app",
        "app_files": [
            ".once/out/apps/App/App.app/App",
            ".once/out/apps/App/App.app/Info.plist",
            ".once/out/apps/App/App.app/_CodeSignature/CodeResources",
        ],
        "platform": "ios",
        "sdk_variant": "device",
        "xcode_developer_dir": "",
        "product_name": "App",
    }}],
    "srcs": [],
    "build_dir": ".once/out/packages/AppThinned",
    "scratch_dir": ".once/tmp/analysis/packages/AppThinned",
    "capability": "build",
}}
provider = _apple_thinned_package_impl(ctx)
result = repr(provider)
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "packages".to_string(),
        ".once/out/packages/AppThinned".to_string(),
    );

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    assert!(out.contains("\"device_model\": \"iPhone17,1\""), "{out}");
    assert!(out.contains("\"ipa_directory\""), "{out}");
    let stage = action_by_identifier(&store, "apple_thinned_package_stage:packages/AppThinned");
    assert_eq!(
        stage.operation,
        Some(DeclaredActionOperation::CopyPath {
            sources: vec![".once/out/apps/App/App.app".to_string()],
            destination: ".once/out/packages/AppThinned/thinning-input/Payload/App.app".to_string(),
            mode: DeclaredCopyPathMode::Tree,
        })
    );
    assert_eq!(
        stage.inputs,
        vec![
            ".once/out/apps/App/App.app",
            ".once/out/apps/App/App.app/App",
            ".once/out/apps/App/App.app/Info.plist",
            ".once/out/apps/App/App.app/_CodeSignature/CodeResources",
        ]
    );
    assert_apple_thinning_adapter(&store);
    assert_apple_thinning_package_action(&store);
}

#[test]
fn prelude_apple_thinned_package_rejects_non_device_applications() {
    let error = eval_prelude_function(
        "_apple_thinning_application",
        r#"([{
            "target_kind": "apple_application",
            "app_path": ".once/out/App.app",
            "platform": "ios",
            "sdk_variant": "simulator",
        }], "AppThinned")"#,
    )
    .unwrap_err();

    assert!(error.contains("sdk_variant = \\\"device\\\""), "{error}");
}

#[test]
fn prelude_apple_thinned_package_rejects_multiple_applications() {
    let error = eval_prelude_function("_apple_thinning_application", r#"([{}, {}], "AppThinned")"#)
        .unwrap_err();

    assert!(error.contains("requires exactly one"), "{error}");
}

#[cfg(unix)]
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the inline Starlark fixture keeps this provider contract in one test"
)]
fn prelude_apple_swift_framework_uses_private_header_search_paths() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let sources = workspace.path().join("framework/Sources");
    let headers = workspace.path().join("framework/Vendor/include/yoga");
    std::fs::create_dir_all(&sources).unwrap();
    std::fs::create_dir_all(&headers).unwrap();
    std::fs::write(sources.join("Plugin.swift"), "public struct Plugin {}\n").unwrap();
    std::fs::write(headers.join("YGEnums.h"), "typedef int YGEnum;\n").unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "xcrun":
        return "/usr/bin/xcrun"
    if name == "codesign":
        return "/usr/bin/codesign"
    if name == "find":
        return "/usr/bin/find"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if argv[0] == "/usr/bin/find":
        return "framework/Vendor/include/yoga/YGEnums.h\n"
    if "--find" in argv:
        return "/toolchain/" + argv[len(argv) - 1] + "\n"
    if "--show-sdk-path" in argv:
        return "/sdks/iPhoneSimulator.sdk\n"
    if "--version" in argv:
        return "Swift version test\n"
    if "-print-resource-dir" in argv:
        return "/toolchain/resource\n"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{
        "package": "framework",
        "name": "Plugin",
        "id": "framework/Plugin",
    }},
    "attr": {{
        "platform": "ios",
        "bundle_id": "dev.once.Plugin",
        "minimum_os": "17.0",
        "sdk_variant": "simulator",
        "private_header_dirs": ["Vendor/include"],
    }},
    "deps": [{{
        "label_id": "vendor/Binary",
        "transitive_link_framework_bundles": [{{
            "path": ".once/vendor/Binary.framework",
            "module_name": "Binary",
            "files": [".once/vendor/Binary.framework/Binary"],
            "label_id": "vendor/Binary",
            "linkage": "static",
        }}],
        "transitive_framework_bundles": [],
    }}],
    "srcs": ["Sources/**/*.swift"],
    "build_dir": ".once/out/framework/Plugin",
    "capability": "build",
}}
provider = _apple_framework_impl(ctx)
result = repr([
    provider["framework_path"],
    provider.get("transitive_framework_search_dirs") or [],
    provider.get("transitive_framework_files") or [],
])
"#
    );
    let store = store_for(workspace.path(), "framework");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(
        out.unwrap(),
        r#"[".once/out/framework/Plugin/Plugin.framework", [".once/vendor"], [".once/vendor/Binary.framework/Binary"]]"#
    );
    let compile = action_by_identifier(&store, "apple_framework_compile_Plugin");
    assert!(compile
        .argv
        .windows(4)
        .any(|args| { args == ["-Xcc", "-I", "-Xcc", "framework/Vendor/include"] }));
    assert!(action_has_input_suffix(
        compile,
        "framework/Vendor/include/yoga/YGEnums.h"
    ));
}

#[cfg(unix)]
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the inline Starlark fixture keeps this provider contract in one test"
)]
fn prelude_xcode_framework_loads_prebuilt_swift_macro_executables() {
    let prelude = xcode_prelude_source();
    let workspace = TempDir::new().unwrap();
    let sources = workspace.path().join("framework/Sources");
    std::fs::create_dir_all(&sources).unwrap();
    std::fs::write(sources.join("Plugin.swift"), "public struct Plugin {}\n").unwrap();

    let macro_cache = TempDir::new().unwrap();
    let macro_path = macro_cache.path().join("FixtureMacros.macro");
    std::fs::write(&macro_path, "fixture macro executable\n").unwrap();

    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "xcrun":
        return "/usr/bin/xcrun"
    if name == "codesign":
        return "/usr/bin/codesign"
    if name == "find":
        return "/usr/bin/find"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if argv[0] == "/usr/bin/find":
        return ""
    if "--find" in argv:
        return "/toolchain/" + argv[len(argv) - 1] + "\n"
    if "--show-sdk-path" in argv:
        return "/sdks/iPhoneSimulator.sdk\n"
    if "--version" in argv:
        return "Swift version test\n"
    if "-print-resource-dir" in argv:
        return "/toolchain/resource\n"
    fail("unexpected host_command: " + str(argv))

files = {{
    "source_flags": {{}},
    "project_header_dirs": [],
    "sources": ["framework/Sources/Plugin.swift"],
    "headers": [],
    "exported_headers": [],
    "frameworks": [],
}}
attrs = _xcode_common_attrs(
    {{"attr": {{"sdk_variant": "simulator"}}}},
    {{"name": "Plugin"}},
    {{"SWIFT_LOAD_BINARY_MACROS": [{macro_descriptor:?}]}},
    {{}},
    "ios",
    files,
)
attrs["bundle_id"] = "dev.once.Plugin"
attrs["minimum_os"] = "17.0"
ctx = {{
    "label": {{
        "package": "framework",
        "name": "Plugin",
        "id": "framework/Plugin",
    }},
    "attr": attrs,
    "deps": [],
    "srcs": ["Sources/**/*.swift"],
    "build_dir": ".once/out/framework/Plugin",
    "capability": "build",
}}
provider = _apple_framework_impl(ctx)
result = repr(provider["framework_path"])
"#,
        macro_descriptor = format!("{}#FixtureMacros", macro_path.display()),
    );
    let store = store_for(workspace.path(), "framework");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(
        out.unwrap(),
        r#"".once/out/framework/Plugin/Plugin.framework""#
    );
    let compile = action_by_identifier(&store, "apple_framework_compile_Plugin");
    let staged_macro = ".once/out/framework/binary-swift-plugins/0/FixtureMacros.macro";
    assert!(
        compile.argv.windows(4).any(|args| {
            args == [
                "-Xfrontend",
                "-load-plugin-executable",
                "-Xfrontend",
                &format!("{staged_macro}#FixtureMacros"),
            ]
        }),
        "{:?}",
        compile.argv
    );
    assert!(action_has_input_suffix(compile, staged_macro));
    assert!(store.actions.iter().any(|action| {
        matches!(
            action.operation,
            Some(DeclaredActionOperation::MaterializeHostTree { ref source, ref destination, .. })
                if source == macro_cache.path().to_string_lossy().as_ref()
                    && destination == ".once/out/framework/binary-swift-plugins/0"
        )
    }));
}

#[cfg(unix)]
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the inline Starlark fixture keeps this provider contract in one test"
)]
fn prelude_apple_framework_stops_static_links_and_propagates_runtime_frameworks() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("framework/Sources");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(package_dir.join("Plugin.swift"), "import Support\n").unwrap();
    std::fs::write(package_dir.join("Plugin.h"), "void plugin(void);\n").unwrap();
    std::fs::write(
        package_dir.join("Plugin.m"),
        "#import \"Plugin.h\"\nvoid plugin(void) {}\n",
    )
    .unwrap();
    std::fs::write(package_dir.join("Resource.txt"), "resource\n").unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "xcrun":
        return "/usr/bin/xcrun"
    if name == "codesign":
        return "/usr/bin/codesign"
    if name == "find":
        return "/usr/bin/find"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if argv[0] == "/usr/bin/find":
        return ""
    if "--find" in argv:
        return "/toolchain/" + argv[len(argv) - 1] + "\n"
    if "--show-sdk-path" in argv:
        return "/sdks/iPhoneSimulator.sdk\n"
    if "--version" in argv:
        return "Swift version test\n"
    if "-print-resource-dir" in argv:
        return "/toolchain/resource\n"
    fail("unexpected host_command: " + str(argv))

support = {{
    "path": ".once/out/support/Support.framework",
    "module_name": "Support",
    "files": [".once/out/support/Support.framework/Support"],
    "label_id": "support/Support",
}}
runtime = {{
    "path": ".once/out/runtime/Runtime.framework",
    "module_name": "Runtime",
    "files": [".once/out/runtime/Runtime.framework/Runtime"],
    "label_id": "runtime/Runtime",
}}
binary = {{
    "path": ".once/vendor/Binary.framework",
    "module_name": "Binary",
    "files": [".once/vendor/Binary.framework/Binary"],
    "label_id": "vendor/Binary",
    "linkage": "static",
}}
ctx = {{
    "label": {{
        "package": "framework",
        "name": "Plugin",
        "id": "framework/Plugin",
    }},
    "attr": {{
        "platform": "ios",
        "bundle_id": "dev.once.Plugin",
        "minimum_os": "17.0",
        "sdk_variant": "simulator",
        "resources": ["Sources/Resource.txt"],
        "enable_modules": True,
        "exported_headers": ["Sources/Plugin.h"],
    }},
    "deps": [{{
        "label_id": "static/Static",
        "transitive_archives": [".once/out/static/Static.a", ".once/vendor/Binary.framework/Binary"],
        "transitive_alwayslink_archives": [".once/out/static/Static.a"],
        "transitive_link_framework_bundles": [support, binary],
        "transitive_framework_bundles": [support, runtime],
    }}],
    "srcs": ["Sources/**/*.swift", "Sources/**/*.m"],
    "build_dir": ".once/out/framework/Plugin",
    "capability": "build",
}}
provider = _apple_framework_impl(ctx)
result = repr([
    provider["transitive_archives"],
    provider["absorbed_static_archives"],
    [bundle["path"] for bundle in provider["transitive_link_framework_bundles"]],
    [bundle["path"] for bundle in provider["transitive_framework_bundles"]],
    provider["transitive_vfs_overlays"],
])
"#
    );
    let store = store_for(workspace.path(), "framework");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(
        out.unwrap(),
        "[[], [\".once/out/framework/Plugin.a\", \".once/out/static/Static.a\", \".once/vendor/Binary.framework/Binary\"], [\".once/out/framework/Plugin/Plugin.framework\"], [\".once/out/framework/Plugin/Plugin.framework\", \".once/out/support/Support.framework\", \".once/out/runtime/Runtime.framework\"], [\".once/out/framework/framework-headers-overlay.yaml\"]]"
    );
    let link = action_by_identifier(&store, "apple_framework_link_Plugin");
    assert!(link.argv.windows(4).any(|args| {
        args == [
            "-Xlinker",
            "-force_load",
            "-Xlinker",
            ".once/out/static/Static.a",
        ]
    }));
    assert!(link
        .argv
        .iter()
        .any(|arg| arg == ".once/vendor/Binary.framework/Binary"));
    assert!(!link.argv.windows(4).any(|args| {
        args == [
            "-Xlinker",
            "-force_load",
            "-Xlinker",
            ".once/vendor/Binary.framework/Binary",
        ]
    }));
    assert!(!link
        .argv
        .windows(2)
        .any(|args| args == ["-framework", "Binary"]));
    assert!(link
        .argv
        .windows(2)
        .any(|args| args == ["-F", ".once/out/runtime"]));
    assert!(link
        .inputs
        .iter()
        .any(|input| input == ".once/out/runtime/Runtime.framework/Runtime"));
    let swift_compile = action_by_identifier(&store, "swift_module_compile_Plugin");
    assert!(swift_compile.argv.windows(4).any(|args| {
        args == [
            "-Xfrontend",
            "-disable-autolink-framework",
            "-Xfrontend",
            "Binary",
        ]
    }));
    assert!(link
        .argv
        .iter()
        .any(|arg| arg == ".once/out/framework/Plugin-framework-link-anchor.swift"));
}

#[cfg(unix)]
#[test]
#[allow(clippy::too_many_lines)]
fn prelude_apple_test_bundle_stages_transitive_framework_runtime_closure() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("tests/Sources");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(package_dir.join("PluginTests.swift"), "import XCTest\n").unwrap();
    std::fs::write(package_dir.join("Legacy.h"), "void legacy(void);\n").unwrap();
    std::fs::write(
        package_dir.join("Legacy.m"),
        "#import <XCTest/XCTest.h>\n#import \"Legacy.h\"\nvoid legacy(void) {}\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("PluginTests-Bridging-Header.h"),
        "#import \"Legacy.h\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("PluginTests-Prefix.pch"),
        "#import \"Legacy.h\"\n",
    )
    .unwrap();
    let fixtures_dir = workspace.path().join("tests/Fixtures/Nested");
    std::fs::create_dir_all(&fixtures_dir).unwrap();
    std::fs::write(fixtures_dir.join("fixture.json"), "{}\n").unwrap();
    std::fs::write(
        workspace.path().join("tests/Info.plist"),
        "<plist><dict><key>CFBundleExecutable</key><string>$(EXECUTABLE_NAME)</string><key>CFBundleIdentifier</key><string>$(PRODUCT_BUNDLE_IDENTIFIER)</string><key>SOURCE_ROOT_DIR</key><string>$(SRCROOT)</string></dict></plist>",
    )
    .unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "xcrun":
        return "/usr/bin/xcrun"
    if name == "codesign":
        return "/usr/bin/codesign"
    if name == "sh":
        return "/bin/sh"
    if name == "find":
        return "/usr/bin/find"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if argv[0] == "/usr/bin/find":
        if "-name" in argv:
            return ""
        if "-type" in argv and "f" in argv:
            return argv[1] + "/Nested/fixture.json\n"
        return argv[1] + "\n"
    if "--find" in argv:
        if argv[len(argv) - 1] == "swiftc":
            return "/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc\n"
        return "/toolchain/" + argv[len(argv) - 1] + "\n"
    if "--show-sdk-path" in argv:
        return "/sdks/MacOSX.sdk\n"
    if "--show-sdk-platform-path" in argv:
        return "/Platforms/MacOSX.platform\n"
    if "--version" in argv:
        return "Swift version test\n"
    fail("unexpected host_command: " + str(argv))

plugin = {{
    "path": ".once/out/plugin/Plugin.framework",
    "module_name": "Plugin",
    "files": [".once/out/plugin/Plugin.framework/Plugin"],
    "label_id": "plugin/Plugin",
}}
support = {{
    "path": ".once/out/support/Support.framework",
    "module_name": "Support",
    "label_id": "support/Support",
}}
ctx = {{
    "label": {{
        "package": "tests",
        "name": "PluginTests",
        "id": "tests/PluginTests",
    }},
    "attr": {{
        "platform": "macos",
        "minimum_os": "14.0",
        "bridging_header": "Sources/PluginTests-Bridging-Header.h",
        "prefix_header": "Sources/PluginTests-Prefix.pch",
        "private_header_dirs": ["Sources"],
        "defines": ["DEBUG"],
        "sdk_frameworks": ["Security"],
        "weak_sdk_frameworks": ["Contacts"],
        "sdk_dylibs": ["sqlite3"],
        "linkopts": ["-Xlinker", "-ObjC"],
        "bundle_id": "dev.once.PluginTests",
        "resources": ["Fixtures"],
        "structured_resources": ["Fixtures"],
        "info_plist": "Info.plist",
        "info_plist_substitutions": {{
            "EXECUTABLE_NAME": "PluginTests",
            "PRODUCT_BUNDLE_IDENTIFIER": "dev.once.PluginTests",
            "SRCROOT": "/workspace/tests",
        }},
    }},
    "deps": [{{
        "label_id": "plugin/Plugin",
        "transitive_link_framework_bundles": [plugin],
        "transitive_framework_bundles": [plugin, support],
        "transitive_archives": [".once/out/plugin/Plugin.a"],
    }}, {{
        "label_id": "app/WidgetExtension",
        "target_kind": "apple_application",
        "app_executable": ".once/out/app/WidgetExtension.app/WidgetExtension",
        "application_extension": True,
    }}, {{
        "label_id": "app/App",
        "target_kind": "apple_application",
        "app_executable": ".once/out/app/App.app/App",
        "host_link_archives": [".once/out/plugin/Plugin.a"],
    }}],
    "srcs": ["Sources/**/*.swift", "Sources/**/*.m"],
    "build_dir": ".once/out/tests/PluginTests",
    "capability": "test",
}}
provider = _apple_test_bundle_impl(ctx)
result = repr(provider["test_bundle_path"])
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "tests".to_string(),
        ".once/out/tests/PluginTests".to_string(),
    );

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert!(out.unwrap().contains("PluginTests.xctest"));
    let compile = action_by_identifier(&store, "apple_test_bundle_compile_PluginTests");
    assert!(compile.argv.windows(2).any(|args| {
        args == [
            "-I".to_string(),
            "/Platforms/MacOSX.platform/Developer/usr/lib".to_string(),
        ]
    }));
    assert!(compile.argv.iter().any(|arg| arg == "-lXCTestSwiftSupport"));
    assert!(compile
        .argv
        .windows(2)
        .any(|args| { args == ["-framework".to_string(), "Security".to_string()] }));
    assert!(compile.argv.windows(4).any(|args| {
        args == [
            "-Xlinker".to_string(),
            "-weak_framework".to_string(),
            "-Xlinker".to_string(),
            "Contacts".to_string(),
        ]
    }));
    assert!(compile.argv.iter().any(|arg| arg == "-lsqlite3"));
    assert!(compile
        .argv
        .windows(2)
        .any(|args| { args == ["-Xlinker".to_string(), "-ObjC".to_string()] }));
    assert!(compile.argv.windows(4).any(|args| {
        args == [
            "-Xlinker".to_string(),
            "-bundle_loader".to_string(),
            "-Xlinker".to_string(),
            ".once/out/app/App.app/App".to_string(),
        ]
    }));
    assert!(compile
        .inputs
        .iter()
        .any(|input| input == ".once/out/app/App.app/App"));
    assert!(!compile
        .argv
        .iter()
        .any(|arg| arg == ".once/out/plugin/Plugin.a"));
    assert!(compile.argv.windows(2).any(|args| {
        args == [
            "-import-objc-header".to_string(),
            "tests/Sources/PluginTests-Bridging-Header.h".to_string(),
        ]
    }));
    let clang = action_by_identifier(
        &store,
        "apple_test_bundle_clang_compile_PluginTests_tests_Sources_Legacy.m",
    );
    assert!(clang.argv.iter().any(|arg| arg == "-DDEBUG"));
    assert!(clang.argv.windows(2).any(|args| {
        args == [
            "-include".to_string(),
            "tests/Sources/PluginTests-Prefix.pch".to_string(),
        ]
    }));
    assert!(clang
        .inputs
        .iter()
        .any(|input| input == "tests/Sources/PluginTests-Prefix.pch"));
    let legacy_object = ".once/out/tests/PluginTests/Objects/tests_Sources_Legacy.m.o";
    assert!(
        compile.inputs.iter().any(|input| input == legacy_object),
        "{:?}",
        compile.inputs
    );
    assert!(
        compile.argv.iter().any(|arg| arg == legacy_object),
        "{:?}",
        compile.argv
    );
    assert!(compile
        .argv
        .windows(2)
        .any(|args| args == ["-framework", "Plugin"]));
    assert!(!compile
        .argv
        .windows(2)
        .any(|args| args == ["-framework", "Support"]));
    let plugin_embed = action_by_identifier(&store, "apple_test_bundle_embed_Plugin.framework");
    let support_copy =
        action_by_identifier(&store, "apple_test_bundle_embed_copy_Support.framework");
    assert_eq!(support_copy.inputs, [".once/out/support/Support.framework"]);
    let support_embed = action_by_identifier(&store, "apple_test_bundle_embed_Support.framework");
    assert_eq!(
        support_embed.inputs,
        [".once/out/tests/PluginTests/PluginTests.xctest/Contents/Frameworks/Support.framework"]
    );
    let codesign = action_by_identifier(&store, "apple_test_bundle_codesign_PluginTests");
    let plist = action_by_identifier(
        &store,
        "write_path:.once/out/tests/PluginTests/PluginTests.xctest/Contents/Info.plist",
    );
    let Some(DeclaredActionOperation::WriteFile { bytes, .. }) = &plist.operation else {
        panic!("test property-list action must write the custom template");
    };
    let contents = std::str::from_utf8(bytes).unwrap();
    assert!(contents.contains("dev.once.PluginTests"), "{contents}");
    assert!(contents.contains("/workspace/tests"), "{contents}");
    let resource = store
        .actions
        .iter()
        .find(|action| {
            action
                .outputs
                .iter()
                .any(|output| output.ends_with("Contents/Resources/Fixtures/Nested/fixture.json"))
        })
        .unwrap_or_else(|| {
            panic!(
                "missing structured test resource action: {:?}",
                store
                    .actions
                    .iter()
                    .map(|action| (&action.identifier, &action.outputs))
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(resource.inputs, ["tests/Fixtures/Nested/fixture.json"]);
    assert_eq!(
        resource.outputs,
        [".once/out/tests/PluginTests/PluginTests.xctest/Contents/Resources/Fixtures/Nested/fixture.json"]
    );
    assert!(codesign
        .inputs
        .iter()
        .any(|input| input.ends_with("Contents/Resources/Fixtures/Nested/fixture.json")));
    assert!(action_has_input_suffix(
        codesign,
        "Contents/Frameworks/Support.framework/_CodeSignature/CodeResources"
    ));
    assert!(codesign
        .outputs
        .iter()
        .any(|output| output.ends_with("Contents/MacOS/PluginTests")));
    let runner = action_by_identifier(&store, "apple_xctest:tests/PluginTests");
    assert!(action_has_input_suffix(
        runner,
        "Contents/Frameworks/Plugin.framework"
    ));
    assert!(action_has_input_suffix(
        runner,
        "Contents/Frameworks/Support.framework"
    ));
    assert!(action_has_input_suffix(
        runner,
        "Contents/Resources/Fixtures/Nested/fixture.json"
    ));
    assert!(!runner.cacheable);
    for action in [compile, plugin_embed, support_copy, support_embed, codesign] {
        assert!(action.cacheable);
    }
}

#[cfg(unix)]
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the inline Starlark fixture keeps this test runner contract in one test"
)]
fn prelude_apple_test_bundle_runs_ios_hosted_tests_with_xctestrun() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("tests/Sources");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        package_dir.join("HostedTests.swift"),
        "import XCTest\nfinal class HostedTests: XCTestCase { func testHost() {} }\n",
    )
    .unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    return "/usr/bin/" + name

def host_command(argv, env = None, merge_stderr = None):
    if "--find" in argv:
        if argv[len(argv) - 1] == "swiftc":
            return "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc\n"
        return "/toolchain/" + argv[len(argv) - 1] + "\n"
    if "--show-sdk-path" in argv:
        return "/sdks/iPhoneSimulator.sdk\n"
    if "--show-sdk-platform-path" in argv:
        return "/Platforms/iPhoneSimulator.platform\n"
    if "--version" in argv:
        return "Swift version test\n"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{
        "package": "tests",
        "name": "HostedTests",
        "id": "tests/HostedTests",
    }},
    "attr": {{
        "platform": "ios",
        "minimum_os": "17.0",
        "sdk_variant": "simulator",
        "test_env": {{
            "AppleLanguages": "(en)",
            "AppleLocale": "en_US",
            "TZ": "America/New_York",
        }},
        "skipped_tests": ["SlowTests", "FeatureTests/testManual"],
    }},
    "deps": [{{
        "label_id": "app/App",
        "target_kind": "apple_application",
        "app_path": ".once/out/app/App/App.app",
        "app_executable": ".once/out/app/App/App.app/App",
        "app_files": [
            ".once/out/app/App/App.app/App",
            ".once/out/app/App/App.app/Info.plist",
        ],
        "bundle_id": "dev.once.App",
        "product_name": "App",
    }}],
    "srcs": ["Sources/**/*.swift"],
    "build_dir": ".once/out/tests/HostedTests",
    "capability": "test",
}}
provider = _apple_test_bundle_impl(ctx)
result = repr(provider["test_info"]["command"]["argv"])
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "tests".to_string(),
        ".once/out/tests/HostedTests".to_string(),
    );

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    assert!(out.contains("/usr/bin/xcodebuild"), "{out}");
    assert!(out.contains("test-without-building"), "{out}");
    let xctestrun = action_by_identifier(
        &store,
        "write_path:.once/out/tests/HostedTests/runner/tests.xctestrun",
    );
    let Some(DeclaredActionOperation::WriteFile { bytes, .. }) = &xctestrun.operation else {
        panic!("xctestrun action must write its property list");
    };
    let contents = std::str::from_utf8(bytes).unwrap();
    assert!(contents.contains("<key>IsAppHostedTestBundle</key><true/>"));
    assert!(contents.contains(".once/out/app/App/App.app"));
    assert!(contents.contains("libXCTestBundleInject.dylib"));
    assert!(contents.contains("<key>AppleLanguages</key><string>(en)</string>"));
    assert!(contents.contains("<key>AppleLocale</key><string>en_US</string>"));
    assert!(contents.contains("<key>TZ</key><string>America/New_York</string>"));
    assert!(contents.contains("<string>SlowTests</string>"));
    let run = action_by_identifier(&store, "apple_xctest:tests/HostedTests");
    let script = run.argv.last().unwrap();
    assert!(script.contains("test-without-building"), "{script}");
    assert!(
        script.contains("-skip-testing:HostedTests/SlowTests"),
        "{script}"
    );
    assert!(
        script.contains("-skip-testing:HostedTests/FeatureTests/testManual"),
        "{script}"
    );
    assert!(
        script.contains("-destination \"id=$simulator_id\""),
        "{script}"
    );
    assert!(run
        .inputs
        .iter()
        .any(|input| input == ".once/out/app/App/App.app/Info.plist"));
    assert!(run
        .inputs
        .iter()
        .any(|input| input.ends_with("runner/tests.xctestrun")));
}

#[test]
fn prelude_apple_ui_xctestrun_uses_the_runner_and_application_under_test() {
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

result = _apple_ui_xctestrun(
    "InterfaceTests",
    ".once/out/InterfaceTests/InterfaceTests-Runner.app/PlugIns/InterfaceTests.xctest",
    ".once/out/InterfaceTests/InterfaceTests-Runner.app",
    "org.example.InterfaceTests.xctrunner",
    {{
        "app_path": ".once/out/App/App.app",
    }},
    "/Platforms/iPhoneSimulator.platform/Developer/Library/Frameworks",
    "/Platforms/iPhoneSimulator.platform/Developer/usr/lib",
    {{
        "AppleLanguages": "(en)",
        "AppleLocale": "en_US",
        "CONFIG_VALUE": "configured",
    }},
    ["-ExampleMode", "testing"],
    ["InterfaceTests/testManual"],
)
"#
    );

    let contents = eval_prelude_source_to_repr(source).unwrap();
    assert!(contents.contains("<key>IsUITestBundle</key><true/>"));
    assert!(contents.contains("<key>IsXCTRunnerHostedTestBundle</key><true/>"));
    assert!(contents.contains("/workspace/.once/out/InterfaceTests/InterfaceTests-Runner.app"));
    assert!(contents
        .contains("<key>UITargetAppPath</key><string>/workspace/.once/out/App/App.app</string>"));
    assert!(contents.contains("<string>-ExampleMode</string><string>testing</string>"));
    assert!(contents.contains("<key>UITargetAppCommandLineArguments</key><array><string>-AppleLanguages</string><string>(en)</string><string>-AppleLocale</string><string>en_US</string></array>"));
    assert!(contents.contains("<key>CONFIG_VALUE</key><string>configured</string>"));
    assert!(contents.contains("<string>InterfaceTests/testManual</string>"));
}

fn apple_test_bundle_source(capability: &str, test_block: &str) -> String {
    let prelude = all_prelude_source();
    format!(
        r#"{prelude}
def host_which(name):
    if name == "xcrun":
        return "/usr/bin/xcrun"
    if name == "codesign":
        return "/usr/bin/codesign"
    if name == "sh":
        return "/bin/sh"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if "--find" in argv:
        if argv[len(argv) - 1] == "swiftc":
            return "/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc\n"
        return "/toolchain/" + argv[len(argv) - 1] + "\n"
    if "--show-sdk-path" in argv:
        return "/sdks/MacOSX.sdk\n"
    if "--show-sdk-platform-path" in argv:
        return "/Platforms/MacOSX.platform\n"
    if "--version" in argv:
        return "Swift version test\n"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{"package": "tests", "name": "PluginTests", "id": "tests/PluginTests"}},
    "attr": {{"platform": "macos", "minimum_os": "14.0"}},
    "deps": [],
    "srcs": ["Sources/**/*.swift"],
    "build_dir": ".once/out/tests/PluginTests",
    "capability": {capability:?},{test_block}
}}
provider = _apple_test_bundle_impl(ctx)
result = repr(provider["test_info"])
"#
    )
}

fn apple_test_bundle_store() -> (AnalysisStore, TempDir) {
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("tests/Sources");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(package_dir.join("PluginTests.swift"), "import XCTest\n").unwrap();
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "tests".to_string(),
        ".once/out/tests/PluginTests".to_string(),
    );
    (store, workspace)
}

#[test]
fn prelude_apple_test_bundle_manifest_advertises_case_sharding() {
    let (store, _workspace) = apple_test_bundle_store();
    let source = apple_test_bundle_source("build", "");
    let (_store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let manifest = out.unwrap();
    // The test bundle can be sharded case by case, and a shard's case subset is
    // passed to the runner through arguments.
    assert!(manifest.contains(r#""granularity": "case""#), "{manifest}");
    assert!(
        manifest.contains(r#""case_filtering": "runner_args""#),
        "{manifest}"
    );
    assert!(
        manifest.contains(r#""strategy": "normalized_results""#),
        "{manifest}"
    );
    assert!(
        manifest.contains(r#""supported": True"#),
        "sharding must be supported: {manifest}"
    );
}

#[test]
fn prelude_apple_test_bundle_shard_filters_select_specific_cases() {
    // With no shard filters the runner selects every case (`-XCTest All`); with
    // a shard's unit ids it selects exactly those `Suite/method` cases, so each
    // shard runs only its slice of the bundle.
    let (store, _workspace) = apple_test_bundle_store();
    let full = apple_test_bundle_source("test", "");
    let (store, _) = with_active_store(store, || eval_prelude_source_to_repr(full));
    let runner = action_by_identifier(&store, "apple_xctest:tests/PluginTests");
    let script = runner.argv.last().expect("runner script");
    assert!(script.contains("-XCTest"), "{script}");
    assert!(
        script.contains("All"),
        "unfiltered run selects All: {script}"
    );

    let (store2, _workspace2) = apple_test_bundle_store();
    let sharded = apple_test_bundle_source(
        "test",
        "\n    \"test\": {\"filters\": [\"tests/PluginTests::NetworkTests/testTimeout\", \"tests/PluginTests::NetworkTests/testRetry\"]},",
    );
    let (store2, _) = with_active_store(store2, || eval_prelude_source_to_repr(sharded));
    let runner2 = action_by_identifier(&store2, "apple_xctest:tests/PluginTests");
    let script2 = runner2.argv.last().expect("runner script");
    assert!(
        script2.contains("NetworkTests/testTimeout,NetworkTests/testRetry"),
        "shard must select only its cases: {script2}"
    );
    assert!(
        !script2.contains("-XCTest All"),
        "sharded run must not select All: {script2}"
    );
    assert!(
        !script2.contains("tests/PluginTests::"),
        "the target prefix must be stripped from selectors: {script2}"
    );
}

#[cfg(unix)]
#[test]
fn prelude_apple_test_cases_script_lists_xctest_and_swift_testing_cases() {
    // Run the generated listing script against real sources and confirm the
    // emitted case ids carry the `<target>::<Suite>/<method>` selector the
    // shard runner turns back into `-XCTest` arguments.
    let script = eval_prelude_string_function(
        "_apple_test_cases_script",
        r#"(["XCTests.swift", "Suite.swift"], "cases.jsonl", "tests/Bundle", "xctest")"#,
    )
    .unwrap();

    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("XCTests.swift"),
        "import XCTest\nclass NetworkTests: XCTestCase {\n  func testTimeout() {}\n  func testRetry() throws {}\n  func helper() {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("Suite.swift"),
        "import Testing\nstruct MathSuite {\n  @Test func addsNumbers() {}\n}\n",
    )
    .unwrap();

    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("status=0\n{script}"))
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let cases = std::fs::read_to_string(dir.path().join("cases.jsonl")).unwrap();

    assert!(
        cases.contains(r#""id":"tests/Bundle::NetworkTests/testTimeout""#),
        "{cases}"
    );
    assert!(
        cases.contains(r#""id":"tests/Bundle::NetworkTests/testRetry""#),
        "{cases}"
    );
    assert!(
        cases.contains(r#""id":"tests/Bundle::MathSuite/addsNumbers""#),
        "{cases}"
    );
    // `helper` is not a test method and must not be listed as a case.
    assert!(!cases.contains("helper"), "{cases}");

    let filtered_script = eval_prelude_string_function(
        "_apple_test_cases_script",
        r#"(["XCTests.swift", "Suite.swift"], "filtered.jsonl", "tests/Bundle", "xctest", ["NetworkTests/testRetry"])"#,
    )
    .unwrap();
    let filtered_output = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("status=0\n{filtered_script}"))
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(filtered_output.status.success(), "{filtered_output:?}");
    let filtered = std::fs::read_to_string(dir.path().join("filtered.jsonl")).unwrap();
    assert!(
        filtered.contains(r#""id":"tests/Bundle::NetworkTests/testRetry""#),
        "{filtered}"
    );
    assert!(!filtered.contains("testTimeout"), "{filtered}");
    assert!(!filtered.contains("addsNumbers"), "{filtered}");
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
fn prelude_android_local_test_declares_test_runner_action() {
    let prelude = android_prelude_source();
    let workspace = TempDir::new().unwrap();
    let test_dir = workspace
        .path()
        .join("apps/hello/src/test/kotlin/dev/once/hello");
    let java_test_dir = workspace
        .path()
        .join("apps/hello/src/test/java/dev/once/hello");
    std::fs::create_dir_all(&test_dir).unwrap();
    std::fs::create_dir_all(&java_test_dir).unwrap();
    std::fs::write(
        test_dir.join("GreetingTest.kt"),
        "package dev.once.hello\nclass GreetingTest { fun testGreeting() {} }\n",
    )
    .unwrap();
    std::fs::write(
        java_test_dir.join("GreetingJavaTest.java"),
        "package dev.once.hello; public class GreetingJavaTest { public void testGreeting() {} }\n",
    )
    .unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "sh":
        return "/bin/sh"
    fail("unexpected host_which: " + name)

def host_env(name):
    if name == "ANDROID_HOST_FLAG":
        return "host-value"
    return ""

def host_command(argv, env = None, merge_stderr = None):
    if len(argv) >= 2 and argv[len(argv) - 1] in ["version", "-version", "--version"]:
        return "tool version test\n"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{
        "package": "apps/hello",
        "name": "GreetingTests",
        "id": "apps/hello/GreetingTests",
    }},
    "attr": {{
        "android_sdk": "/sdk",
        "compile_sdk": 35,
        "build_tools_version": "35.0.0",
        "aapt2": "/sdk/build-tools/35.0.0/aapt2",
        "javac": "/jdk/bin/javac",
        "java": "/jdk/bin/java",
        "kotlinc": "/kotlin/bin/kotlinc",
        "kotlin_stdlib": "/kotlin/lib/kotlin-stdlib.jar",
        "javacopts": ["-Xlint:all"],
        "classpath": ["third_party/junit.jar"],
        "runtime_classpath": ["third_party/hamcrest.jar"],
        "jvm_flags": ["-Duser.language=en"],
        "test_class": "dev.once.hello.GreetingJavaTest",
        "args": ["dev.once.hello.GreetingTest#testGreeting"],
        "env": {{"ANDROID_ENV": "explicit"}},
        "env_inherit": ["ANDROID_HOST_FLAG"],
        "test_env": {{"ANDROID_TEST_ENV": "test"}},
        "labels": ["unit"],
    }},
    "deps": [{{
        "transitive_compile_jars": [".once/out/apps/hello/Greeting/Greeting.jar"],
        "transitive_runtime_jars": [".once/out/apps/hello/Greeting/Greeting.jar"],
    }}],
    "deps_by_role": {{
        "deps": [],
        "runtime_deps": [{{
            "transitive_compile_jars": [".once/out/apps/hello/Runtime/Runtime.jar"],
            "transitive_runtime_jars": [".once/out/apps/hello/Runtime/Runtime.jar"],
        }}],
    }},
    "srcs": ["src/test/**/*.kt", "src/test/**/*.java"],
    "build_dir": ".once/out/apps/hello/GreetingTests",
    "capability": "test",
}}
provider = _android_local_test_impl(ctx)
result = repr(provider["test_info"])
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "apps/hello".to_string(),
        ".once/out/apps/hello/GreetingTests".to_string(),
    );

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_android_local_test_actions(&store, &out.unwrap());
}

fn assert_android_local_test_actions(store: &AnalysisStore, out: &str) {
    assert!(out.contains("android_local"), "{out}");
    assert!(out.contains("unit"), "{out}");
    let kotlin = action_by_identifier(store, "android_kotlin_compile:apps/hello/GreetingTests");
    assert!(kotlin
        .argv
        .iter()
        .any(|arg| arg.contains("/kotlin/lib/kotlin-stdlib.jar")));
    let javac = action_by_identifier(
        store,
        "android_local_test_java_compile:apps/hello/GreetingTests",
    );
    assert!(javac.argv.iter().any(|arg| arg == "-Xlint:all"));
    assert!(!javac
        .argv
        .iter()
        .any(|arg| arg.contains("Runtime/Runtime.jar")));
    let runner_compile = action_by_identifier(
        store,
        "android_local_test_runner_compile:apps/hello/GreetingTests",
    );
    assert_eq!(runner_compile.argv[0], "/jdk/bin/javac");
    assert!(runner_compile
        .inputs
        .iter()
        .any(|input| input.ends_with("OnceJvmTestRunner.java")));
    let run = action_by_identifier(store, "android_local_test:apps/hello/GreetingTests");
    assert_eq!(run.argv[0], "/jdk/bin/java");
    assert_eq!(run.argv[1], "-Duser.language=en");
    assert_eq!(
        run.env.get("ANDROID_HOST_FLAG").map(String::as_str),
        Some("host-value")
    );
    assert_eq!(
        run.env.get("ANDROID_ENV").map(String::as_str),
        Some("explicit")
    );
    assert_eq!(
        run.env.get("ANDROID_TEST_ENV").map(String::as_str),
        Some("test")
    );
    assert!(run.argv.iter().any(|arg| arg == "OnceJvmTestRunner"));
    assert!(run
        .argv
        .iter()
        .any(|arg| arg == "dev.once.hello.GreetingJavaTest"));
    assert!(run
        .argv
        .iter()
        .any(|arg| arg == "dev.once.hello.GreetingTest#testGreeting"));
    assert!(run
        .inputs
        .iter()
        .any(|input| input == ".once/out/apps/hello/GreetingTests/classes.kotlin.sha256"));
    assert!(run
        .inputs
        .iter()
        .any(|input| input == ".once/out/apps/hello/GreetingTests/test_runner/classes.sha256"));
    assert!(run
        .inputs
        .iter()
        .any(|input| input == "third_party/junit.jar"));
    assert!(run
        .argv
        .iter()
        .any(|arg| arg.contains("Runtime/Runtime.jar")));
    assert!(run
        .inputs
        .iter()
        .any(|input| input == ".once/out/apps/hello/Runtime/Runtime.jar"));
    assert!(run
        .outputs
        .iter()
        .any(|output| output.ends_with("test/test_results.json")));
}

#[cfg(unix)]
#[test]
fn prelude_android_instrumentation_test_declares_device_runner_action() {
    let prelude = android_prelude_source();
    let workspace = TempDir::new().unwrap();
    let support_dir = workspace.path().join("apps/hello/support");
    std::fs::create_dir_all(&support_dir).unwrap();
    std::fs::write(support_dir.join("orchestrator.apk"), b"support-apk").unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "sh":
        return "/bin/sh"
    fail("unexpected host_which: " + name)

def host_env(name):
    if name == "ANDROID_DEVICE_HOST_FLAG":
        return "device-host-value"
    return ""

def host_command(argv, env = None, merge_stderr = None):
    if len(argv) >= 1 and argv[0] == "/sdk/platform-tools/adb":
        return "Android Debug Bridge version test\n"
    if len(argv) >= 2 and argv[len(argv) - 1] in ["version", "-version", "--version"]:
        return "javac test\n"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{
        "package": "apps/hello",
        "name": "GreetingInstrumentationTests",
        "id": "apps/hello/GreetingInstrumentationTests",
    }},
    "attr": {{
        "android_sdk": "/sdk",
        "adb": "/sdk/platform-tools/adb",
        "adb_serial": "device-1",
        "javac": "/jdk/bin/javac",
        "java": "/jdk/bin/java",
        "test_app": "./GreetingInstrumentationApk",
        "instrumentation_runner": "androidx.test.runner.AndroidJUnitRunner",
        "instrumentation_args": {{"package": "dev.once.greeting.test"}},
        "args": ["--no-window-animation"],
        "support_apks": ["support/*.apk"],
        "test_class": "dev.once.greeting.GreetingInstrumentedTest",
        "env": {{"ANDROID_DEVICE_ENV": "explicit"}},
        "env_inherit": ["ANDROID_DEVICE_HOST_FLAG"],
        "test_env": {{"ANDROID_DEVICE_TEST_ENV": "test"}},
        "labels": ["device"],
    }},
    "deps": [
        {{
            "label_id": "apps/hello/GreetingApp",
            "target_kind": "android_binary",
            "application_id": "dev.once.greeting",
            "apk": ".once/out/apps/hello/GreetingApp/GreetingApp.apk",
        }},
        {{
            "label_id": "apps/hello/GreetingInstrumentationApk",
            "target_kind": "android_binary",
            "application_id": "dev.once.greeting.test",
            "apk": ".once/out/apps/hello/GreetingInstrumentationApk/GreetingInstrumentationApk.apk",
            "instrumentation_target_id": "apps/hello/GreetingApp",
        }},
    ],
    "srcs": [],
    "build_dir": ".once/out/apps/hello/GreetingInstrumentationTests",
    "capability": "test",
}}
provider = _android_instrumentation_test_impl(ctx)
result = repr(provider["test_info"])
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "apps/hello".to_string(),
        ".once/out/apps/hello/GreetingInstrumentationTests".to_string(),
    );

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    assert!(out.contains("android_instrumentation"), "{out}");
    assert!(out.contains("GreetingInstrumentationApk"), "{out}");
    let runner_compile = action_by_identifier(
        &store,
        "android_instrumentation_runner_compile:apps/hello/GreetingInstrumentationTests",
    );
    assert_android_instrumentation_runner_compile_action(runner_compile);
    let run = action_by_identifier(
        &store,
        "android_instrumentation_test:apps/hello/GreetingInstrumentationTests",
    );
    assert_eq!(
        run.env.get("ANDROID_DEVICE_HOST_FLAG").map(String::as_str),
        Some("device-host-value")
    );
    assert_eq!(
        run.env.get("ANDROID_DEVICE_ENV").map(String::as_str),
        Some("explicit")
    );
    assert_eq!(
        run.env.get("ANDROID_DEVICE_TEST_ENV").map(String::as_str),
        Some("test")
    );
    assert_android_instrumentation_run_action(run);
}

#[test]
fn prelude_android_instrumentation_runner_requires_a_success_terminal_code() {
    let source = eval_prelude_string_function_in(
        android_prelude_source(),
        "_android_instrumentation_runner_source",
        "()",
    )
    .unwrap();

    assert!(source.contains("INSTRUMENTATION_CODE:"));
    assert!(source.contains("code.startsWith(\"-1\")"));
    assert!(source.contains("boolean passed = completed"));
}

#[test]
fn prelude_android_instrumentation_metadata_needs_no_dependency_providers() {
    let prelude = android_prelude_source();
    let workspace = TempDir::new().unwrap();
    let support_dir = workspace.path().join("tests/support");
    std::fs::create_dir_all(&support_dir).unwrap();
    std::fs::write(support_dir.join("orchestrator.apk"), b"support-apk").unwrap();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "tests", "name": "device", "id": "tests/device"}},
    "attr": {{"labels": ["device"], "support_apks": ["support/*.apk"]}},
    "deps": [],
    "srcs": [],
    "build_dir": ".once/out/tests/device",
    "capability": "metadata",
}}
result = repr(_android_instrumentation_test_impl(ctx))
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        "tests".to_string(),
        ".once/out/tests/device".to_string(),
    );

    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let out = out.unwrap();

    assert!(out.contains("android_instrumentation"));
    assert!(out.contains("runner_args"));
    assert!(out.contains("tests/device"));
    assert!(out.contains("tests/support/orchestrator.apk"));
}

#[cfg(unix)]
fn assert_android_instrumentation_runner_compile_action(runner_compile: &DeclaredAction) {
    assert_eq!(runner_compile.argv[0], "/jdk/bin/javac");
    assert!(runner_compile
        .inputs
        .iter()
        .any(|input| input.ends_with("OnceAndroidInstrumentationRunner.java")));
}

#[cfg(unix)]
fn assert_android_instrumentation_run_action(run: &DeclaredAction) {
    assert!(!run.cacheable);
    assert_eq!(run.argv[0], "/jdk/bin/java");
    assert!(run
        .argv
        .iter()
        .any(|arg| arg == "OnceAndroidInstrumentationRunner"));
    assert!(run.argv.iter().any(|arg| arg == "/sdk/platform-tools/adb"));
    assert!(run.argv.iter().any(|arg| arg == "device-1"));
    assert!(run
        .argv
        .iter()
        .any(|arg| arg == "dev.once.greeting.test/androidx.test.runner.AndroidJUnitRunner"));
    assert!(run
        .argv
        .iter()
        .any(|arg| arg == "dev.once.greeting.GreetingInstrumentedTest"));
    assert!(run.argv.iter().any(|arg| arg == "--no-window-animation"));
    assert!(run
        .argv
        .iter()
        .any(|arg| arg == "apps/hello/support/orchestrator.apk"));
    assert!(run
        .inputs
        .iter()
        .any(|input| input.ends_with("instrumentation_runner/classes.sha256")));
    assert!(run
        .inputs
        .iter()
        .any(|input| input.ends_with("GreetingApp.apk")));
    assert!(run
        .inputs
        .iter()
        .any(|input| input.ends_with("GreetingInstrumentationApk.apk")));
    assert!(run
        .inputs
        .iter()
        .any(|input| input == "apps/hello/support/orchestrator.apk"));
    assert!(run
        .outputs
        .iter()
        .any(|output| output.ends_with("test/test_results.json")));
    assert!(run
        .create_dirs
        .iter()
        .any(|path| path.ends_with("test/home")));
}

#[test]
fn prelude_android_neverlink_omits_runtime_closure() {
    let prelude = android_prelude_source();
    let source = format!(
        r#"{prelude}
deps = [{{"transitive_runtime_jars": ["dep.jar"]}}]
result = repr([
    _android_library_runtime_jars({{"neverlink": False}}, deps, ["local.jar"]),
    _android_library_runtime_jars({{"neverlink": True}}, deps, ["local.jar"]),
])
"#
    );

    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(out, "[[\"local.jar\", \"dep.jar\"], []]");
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
fn apple_library_swift_compile_emits_module_and_objects_in_one_action() {
    let source = include_str!("../prelude/apple.star");

    assert!(source.contains("identifier = \"swift_module_compile_"));
    assert!(source.contains(
        "swift_compile_outputs = [swiftmodule, swiftdoc, swift_objc_header] + swift_objects"
    ));
    assert!(source.contains("identifier = \"libtool_swift_archive_"));
    assert!(source.contains("-output-file-map"));
}

#[test]
fn prelude_apple_swift_whole_module_output_shape_tracks_threading() {
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr([
    _apple_swift_emits_single_object(["-whole-module-optimization"]),
    _apple_swift_emits_single_object(["-Owholemodule"]),
    _apple_swift_emits_single_object(["-wmo", "-num-threads", "8"]),
    _apple_swift_emits_single_object(["-Onone"]),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        "[True, True, False, False]"
    );
}

#[test]
fn prelude_apple_disables_batch_mode_for_combined_swift_links() {
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr(_apple_swift_link_flags([
    "-Onone",
    "-j1",
    "-enable-batch-mode",
    "-enable-testing",
]))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["-Onone", "-j1", "-enable-testing"]"#
    );
}

#[test]
fn apple_application_testable_module_is_parsed_as_library() {
    // The `enable_testing` application module emission must pass
    // `-parse-as-library` so an entry-point attribute
    // (`@main`/`@NSApplicationMain`/`@UIApplicationMain`) stays valid; without
    // it, emitting a module from a lone entry-point file fails because swiftc
    // treats it as top-level script code. Regression guard for real single-file
    // app targets (e.g. a login-item launcher).
    let source = include_str!("../prelude/apple.star");

    let block = source
        .split("identifier = \"apple_application_module_\"")
        .next()
        .expect("application module action present");
    let module_argv = block
        .rsplit_once("module_argv = list(swiftc[\"argv\"]) + [")
        .expect("application module argv assembled")
        .1;
    assert!(
        module_argv.contains("\"-parse-as-library\""),
        "application testable module must be compiled with -parse-as-library"
    );
    assert!(
        module_argv.contains("\"-enable-testing\""),
        "application testable module must be compiled with -enable-testing"
    );
}

#[test]
fn target_kind_has_impl_returns_true_for_swift_macro() {
    assert!(target_kind_has_impl("swift_macro").unwrap());
}

#[test]
fn swift_macro_preserves_exact_sources_outside_glob_discovery() {
    let source = include_str!("../prelude/apple.star");
    let implementation = source
        .split_once("def _swift_macro_impl(ctx):")
        .expect("Swift macro implementation")
        .1
        .split_once("# --- Bundle helpers")
        .expect("end of Swift macro implementation")
        .0;
    assert!(
        implementation.contains("glob(ctx[\"srcs\"]) + _apple_declared_source_paths(ctx)"),
        "exact package and generated sources must remain available when glob discovery excludes their directory"
    );
    assert!(
        implementation.contains("_collect_dep_compile_inputs(deps, ctx[\"build_dir\"])")
            && implementation.contains("\"-fmodule-map-file=\" + modulemap")
            && implementation.contains("for modulemap in dep_modulemaps:"),
        "Swift macros must compile with transitive C module metadata and declare it as action input"
    );
}

#[test]
fn prelude_apple_collects_transitive_swift_macro_plugins() {
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr(_collect_dep_compile_inputs([
    {{"transitive_plugin_dylibs": ["libIndirect.dylib"]}},
    {{"plugin_dylib": "libDirect.dylib"}},
    {{"transitive_plugin_executables": ["Indirect-tool#Indirect"]}},
    {{"plugin_executable": "Direct-tool", "plugin_module_name": "Direct"}},
], ".once/out/App")[12:])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"(["libIndirect.dylib", "libDirect.dylib"], ["Indirect-tool#Indirect", "Direct-tool#Direct"])"#
    );
}

#[test]
fn target_kind_has_impl_returns_true_for_all_apple_bundle_kinds() {
    // Every bundled Apple target kind now has a Starlark impl that
    // declares actions; the CLI's generic fallback action is
    // bypassed for these kinds in favour of the Starlark-driven
    // analysis.
    assert!(target_kind_has_impl("apple_framework").unwrap());
    assert!(target_kind_has_impl("apple_application").unwrap());
    assert!(target_kind_has_impl("apple_thinned_package").unwrap());
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
        out.contains("\"srcs\": [\"third_party/rust/vendor/cpufeatures-0.2.17/**/*\"]"),
        "{out}"
    );
}

#[test]
fn prelude_cargo_dependencies_declares_graph_resolver() {
    let source = format!(
        "{}\nresult = repr(cargo_dependencies.get(\"resolver\") != None)\n",
        all_prelude_source()
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(out, "True");
    assert_target_kind_attrs(
        "cargo_dependencies",
        &["resolver_inputs", "metadata_file", "host_metadata_file"],
    );
}

#[test]
fn prelude_cargo_metadata_uses_declared_cargo_config() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

commands = []
def host_command(argv, env = None, cwd = None, merge_stderr = None):
    commands.append([argv, env, cwd])
    return "{{\"argv\": []}}"

ctx = {{
    "label": {{"package": "examples/rust", "name": "deps", "id": "examples/rust/deps"}},
    "attr": {{}},
    "files": {{".cargo/config.toml": "[source.crates-io]\nreplace-with = 'vendored'\n"}},
}}
_cargo_metadata_for_platform(ctx, "cargo", "examples/rust/Cargo.toml", "x86_64-unknown-linux-gnu")
result = repr(commands[0])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert!(
        out.contains(
            "[[\"cargo\", \"--config\", \"/workspace/examples/rust/.cargo/config.toml\", \"metadata\""
        ),
        "{out}"
    );
    assert!(out.ends_with(", \"/workspace\"]"), "{out}");
}

#[test]
fn prelude_cargo_explicit_targets_scope_generated_names() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
package = {{
    "name": "itoa",
    "version": "1.0.14",
    "source": "registry+https://github.com/rust-lang/crates.io-index",
}}
counts = _cargo_duplicate_counts([package])
result = repr([
    _cargo_target_name(package, counts),
    _cargo_target_name(package, counts, "aarch64-apple-darwin"),
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(
        out,
        "[\"itoa-1.0.14\", \"itoa-1.0.14-aarch64-apple-darwin\"]"
    );
}

#[test]
fn prelude_cargo_metadata_must_match_the_authoritative_lockfile() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
metadata = {{
    "packages": [{{
        "name": "itoa",
        "version": "1.0.14",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
    }}],
}}
_cargo_attach_locked_checksums(metadata, {{"package": []}})
result = repr(metadata)
"#
    );
    let error = eval_prelude_source_to_repr(source).unwrap_err();

    assert!(
        error.contains("absent from the authoritative Cargo.lock"),
        "{error}"
    );
}

#[test]
fn prelude_cargo_snapshot_selection_must_match_the_target() {
    let prelude = all_prelude_source();
    let snapshot = serde_json::json!({
        "once_snapshot": {
            "inputs": {"Cargo.toml": "[workspace]\n"},
            "selection": {
                "features": [],
                "all_features": false,
                "no_default_features": false,
                "target": "",
                "host": false,
                "host_triple": "aarch64-apple-darwin",
            },
        },
    })
    .to_string();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "deps", "id": "deps"}},
    "attr": {{
        "metadata_file": "metadata.json",
        "features": ["derive"],
    }},
    "files": {{
        "Cargo.toml": "[workspace]\n",
        "metadata.json": {snapshot:?},
    }},
}}
metadata = json_decode(ctx["files"]["metadata.json"])
_cargo_validate_metadata_snapshot(ctx, metadata, "metadata.json", False, "aarch64-apple-darwin")
result = repr(metadata)
"#
    );
    let error = eval_prelude_source_to_repr(source).unwrap_err();

    assert!(error.contains("selection `features`"), "{error}");
}

#[test]
fn prelude_cargo_snapshot_selection_resolves_configurable_attributes() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "linux"

def host_arch():
    return "x86_64"

ctx = {{
    "label": {{"package": "", "name": "deps", "id": "deps"}},
    "attr": {{
        "features": {{"select": {{"linux": ["derive"], "default": []}}}},
        "all_features": {{"select": {{"linux": True, "default": False}}}},
        "no_default_features": False,
    }},
}}
result = repr(_cargo_snapshot_selection(ctx, False, "x86_64-unknown-linux-gnu"))
"#
    );

    let result = eval_prelude_source_to_repr(source).unwrap();

    assert!(result.contains("\"features\": [\"derive\"]"), "{result}");
    assert!(result.contains("\"all_features\": True"), "{result}");
}

#[test]
fn prelude_cargo_snapshot_selection_must_match_the_compiler_host() {
    let prelude = all_prelude_source();
    let snapshot = serde_json::json!({
        "once_snapshot": {
            "inputs": {"Cargo.toml": "[workspace]\n"},
            "selection": {
                "features": [],
                "all_features": false,
                "no_default_features": false,
                "target": "",
                "host": false,
                "host_triple": "x86_64-unknown-linux-gnu",
            },
        },
    })
    .to_string();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "deps", "id": "deps"}},
    "attr": {{"metadata_file": "metadata.json"}},
    "files": {{
        "Cargo.toml": "[workspace]\n",
        "metadata.json": {snapshot:?},
    }},
}}
metadata = json_decode(ctx["files"]["metadata.json"])
_cargo_validate_metadata_snapshot(ctx, metadata, "metadata.json", False, "aarch64-apple-darwin")
result = repr(metadata)
"#
    );
    let error = eval_prelude_source_to_repr(source).unwrap_err();

    assert!(error.contains("selection `host_triple`"), "{error}");
}

#[test]
fn prelude_cargo_target_snapshot_requires_host_metadata() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "deps", "id": "deps"}},
    "attr": {{
        "target": "aarch64-unknown-linux-gnu",
        "metadata_file": "metadata.json",
    }},
    "files": {{}},
}}
result = repr(_cargo_resolved_metadata(ctx))
"#
    );

    let error = eval_prelude_source_to_repr(source).unwrap_err();

    assert!(error.contains("host_metadata_file is required"), "{error}");
}

#[test]
fn prelude_cargo_workspace_edges_use_the_generated_target_name_map() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
target_id = "registry+https://registry.example/index#shared@1.0.0"
host_id = "git+https://example.invalid/shared#shared@1.0.0"
workspace_id = "path+file:///workspace/app#0.1.0"
def package(id, source):
    return {{
        "id": id,
        "name": "shared",
        "version": "1.0.0",
        "source": source,
        "manifest_path": "/workspace/vendor/shared/Cargo.toml",
        "targets": [{{
            "name": "shared",
            "kind": ["lib"],
            "crate_types": ["lib"],
            "src_path": "/workspace/vendor/shared/src/lib.rs",
            "edition": "2021",
        }}],
    }}
metadata = {{
    "packages": [{{
        "id": workspace_id,
        "name": "app",
        "version": "0.1.0",
        "source": None,
        "targets": [],
    }}, package(target_id, "registry+https://registry.example/index")],
    "resolve": {{"nodes": [
        {{"id": workspace_id, "features": [], "deps": [{{
            "name": "shared",
            "pkg": target_id,
            "dep_kinds": [{{"kind": None}}],
        }}]}},
        {{"id": target_id, "features": [], "deps": []}},
    ]}},
}}
host_metadata = {{
    "packages": [package(host_id, "git+https://example.invalid/shared")],
    "resolve": {{"nodes": [{{"id": host_id, "features": [], "deps": []}}]}},
}}
resolution = _cargo_metadata_resolution({{
    "label": {{"package": "pkg", "name": "deps", "id": "pkg/deps"}},
    "attrs": {{"vendor_dir": "vendor"}},
}}, metadata, host_metadata)
result = repr(resolution["specs"][0]["name"] == resolution["workspace_deps"]["app"][0])
"#
    );

    assert_eq!(eval_prelude_source_to_repr(source).unwrap(), "True");
}

#[test]
fn prelude_cargo_workspace_edges_reject_missing_providers() {
    let prelude = all_prelude_source();
    let error = eval_prelude_function_in(
        prelude,
        "_cargo_resolved_workspace_deps",
        r#"([], {"app": ["shared-1.0.0"]}, {})"#,
    )
    .unwrap_err();

    assert!(error.contains("provider is missing"), "{error}");
}

#[test]
fn prelude_cargo_normalizes_hyphenated_crate_names() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
package = {{
    "name": "demo-tool",
    "version": "1.0.0",
}}
target = {{
    "name": "demo-tool",
    "kind": ["bin"],
}}
env = _cargo_rustc_env(package, target, ".")
example_env = _cargo_rustc_env(package, {{
    "name": "demo-example",
    "kind": ["example"],
}}, ".")
result = repr([
    _cargo_crate_name(package, target),
    env["CARGO_CRATE_NAME"],
    env["CARGO_BIN_NAME"],
    example_env["CARGO_BIN_NAME"],
])
"#
    );

    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        "[\"demo_tool\", \"demo_tool\", \"demo-tool\", \"demo-example\"]"
    );
}

#[test]
fn prelude_cargo_native_workspace_materializes_cached_sources() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
root_id = "path+file:///workspace#root@0.1.0"
dependency_id = "registry+https://registry.example/index#demo@1.0.0"
metadata = {{
    "workspace_members": [root_id],
    "packages": [
        {{
            "id": root_id,
            "name": "root",
            "version": "0.1.0",
            "source": None,
            "manifest_path": "/workspace/Cargo.toml",
            "targets": [],
        }},
        {{
            "id": dependency_id,
            "name": "demo",
            "version": "1.0.0",
            "source": "registry+https://registry.example/index",
            "manifest_path": "/cargo/cache/demo-1.0.0/Cargo.toml",
            "targets": [{{
                "name": "demo",
                "kind": ["lib"],
                "crate_types": ["lib"],
                "src_path": "/cargo/cache/demo-1.0.0/src/lib.rs",
                "edition": "2021",
            }}],
        }},
    ],
    "resolve": {{"nodes": [
        {{"id": root_id, "features": [], "deps": []}},
        {{"id": dependency_id, "features": [], "deps": []}},
    ]}},
}}
resolution = _cargo_metadata_resolution({{
    "label": {{"package": "pkg", "name": "cargo", "id": "pkg/cargo"}},
    "attrs": {{}},
}}, metadata, None, True)
target = resolution["specs"][0]
result = repr([
    target["srcs"],
    target["attrs"]["_cargo_source_root"],
    target["attrs"]["_cargo_materialized_source_root"],
    target["attrs"]["crate_root"],
])
"#
    );

    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[[], "/cargo/cache/demo-1.0.0", ".once/out/pkg/demo-1.0.0/source", ".once/out/pkg/demo-1.0.0/source/src/lib.rs"]"#
    );
}

#[test]
fn prelude_rust_materializes_resolver_owned_host_source_tree() {
    let workspace = TempDir::new().unwrap();
    let host_root = workspace.path().join("cargo-cache/demo-1.0.0");
    std::fs::create_dir_all(host_root.join("src")).unwrap();
    std::fs::write(
        host_root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    std::fs::write(host_root.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();
    let host_root = host_root.to_string_lossy();
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def _rustc_toolchain(target):
    return ("rustc", "rustc-test", "test-host-triple")

ctx = {{
    "label": {{"package": "pkg", "name": "demo", "id": "pkg/demo"}},
    "attr": {{
        "crate_name": "demo",
        "crate_root": ".once/out/pkg/demo/source/src/lib.rs",
        "_cargo_source_root": {host_root:?},
        "_cargo_materialized_source_root": ".once/out/pkg/demo/source",
    }},
    "deps": [],
    "srcs": [],
}}
_rust_compile(ctx, "rlib", "src/lib.rs", "libdemo.rlib")
result = repr("ok")
"#
    );
    let store = store_for(workspace.path(), "pkg/demo");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    assert!(matches!(
        store.actions[0].operation,
        Some(DeclaredActionOperation::MaterializeHostTree {
            ref source,
            ref destination,
            ..
        }) if source.ends_with("cargo-cache/demo-1.0.0")
            && destination == ".once/out/pkg/demo/source"
    ));
    let rustc = action_by_identifier(&store, "pkg/demo:rustc");
    assert!(
        rustc
            .inputs
            .iter()
            .any(|input| input == ".once/out/pkg/demo/source"),
        "{:?}",
        rustc.inputs
    );
}

#[test]
fn prelude_cargo_skips_targets_with_disabled_required_features() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
target = {{"required-features": ["standalone", "windows"]}}
result = repr([
    _cargo_workspace_target_enabled(target, {{"features": ["standalone"]}}),
    _cargo_workspace_target_enabled(target, {{"features": ["standalone", "windows"]}}),
])
"#
    );

    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        "[False, True]"
    );
}

#[test]
fn prelude_cargo_emits_each_declared_library_crate_type() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
package = {{
    "id": "demo-id",
    "name": "demo",
    "version": "1.0.0",
    "targets": [],
}}
target = {{
    "name": "demo",
    "kind": ["lib"],
    "crate_types": ["staticlib", "rlib", "cdylib"],
    "src_path": "/workspace/src/lib.rs",
    "edition": "2021",
}}
specs = _cargo_workspace_target_specs(
    package,
    target,
    {{"features": [], "deps": []}},
    "library",
    ".",
    {{}},
    {{}},
    "cargo_demo",
    "aarch64-unknown-linux-gnu",
)
result = repr([
    [[spec["name"], spec["attrs"]["crate_type"], spec["attrs"]["target"]] for spec in specs],
    _cargo_workspace_target_kind({{
        "kind": ["example"],
        "crate_types": ["staticlib"],
    }}),
])
"#
    );

    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[[["cargo_demo", "rlib", "aarch64-unknown-linux-gnu"], ["cargo_demo_staticlib", "staticlib", "aarch64-unknown-linux-gnu"], ["cargo_demo_cdylib", "cdylib", "aarch64-unknown-linux-gnu"]], "library"]"#
    );
}

#[test]
fn prelude_cargo_treats_nonmember_path_packages_as_dependencies() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
root_id = "path+file:///workspace#root@0.1.0"
path_id = "path+file:///shared#shared@1.0.0"
metadata = {{
    "workspace_members": [root_id],
    "packages": [
        {{
            "id": root_id,
            "name": "root",
            "version": "0.1.0",
            "source": None,
            "manifest_path": "/workspace/Cargo.toml",
            "targets": [],
        }},
        {{
            "id": path_id,
            "name": "shared",
            "version": "1.0.0",
            "source": None,
            "manifest_path": "/shared/Cargo.toml",
            "targets": [{{
                "name": "shared",
                "kind": ["lib"],
                "crate_types": ["lib"],
                "src_path": "/shared/src/lib.rs",
                "edition": "2021",
            }}],
        }},
    ],
    "resolve": {{"nodes": [
        {{"id": root_id, "features": [], "deps": []}},
        {{"id": path_id, "features": [], "deps": []}},
    ]}},
}}
resolution = _cargo_metadata_resolution({{
    "label": {{"package": "", "name": "cargo", "id": "cargo"}},
    "attrs": {{}},
}}, metadata, None, True)
result = repr([
    len(resolution["specs"]),
    resolution["specs"][0]["name"],
    resolution["specs"][0]["attrs"]["_cargo_source_root"],
])
"#
    );

    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[1, "shared-1.0.0", "/shared"]"#
    );
}

#[test]
fn prelude_cargo_test_targets_include_development_dependencies() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
node = {{"deps": [
    {{"name": "runtime", "pkg": "runtime-id", "dep_kinds": [{{"kind": None}}]}},
    {{"name": "test_support", "pkg": "dev-id", "dep_kinds": [{{"kind": "dev"}}]}},
]}}
names = {{"runtime-id": "runtime-target", "dev-id": "dev-target"}}
library_deps, _library_aliases = _cargo_metadata_deps(node, names, False, True)
test_deps, test_aliases = _cargo_metadata_deps(node, names, False, True, True)
result = repr([library_deps, test_deps, test_aliases])
"#
    );

    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["./runtime-target"], ["./runtime-target", "./dev-target"], {"runtime-target": "runtime", "dev-target": "test_support"}]"#
    );
}

#[test]
fn prelude_cargo_ignores_unused_build_dependencies_without_a_build_script() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def package(name):
    return {{
        "id": "registry+https://registry.example/index#" + name + "@1.0.0",
        "name": name,
        "version": "1.0.0",
        "source": "registry+https://registry.example/index",
        "manifest_path": "/workspace/vendor/" + name + "/Cargo.toml",
        "targets": [{{
            "name": name,
            "kind": ["lib"],
            "crate_types": ["lib"],
            "src_path": "/workspace/vendor/" + name + "/src/lib.rs",
            "edition": "2021",
        }}],
    }}
owner = package("owner")
helper = package("helper")
metadata = {{
    "packages": [owner, helper],
    "resolve": {{"nodes": [
        {{
            "id": owner["id"],
            "features": [],
            "deps": [{{
                "name": "helper",
                "pkg": helper["id"],
                "dep_kinds": [{{"kind": "build"}}],
            }}],
        }},
        {{"id": helper["id"], "features": [], "deps": []}},
    ]}},
}}
resolution = _cargo_metadata_resolution({{
    "label": {{"package": "pkg", "name": "deps", "id": "pkg/deps"}},
    "attrs": {{"vendor_dir": "vendor"}},
}}, metadata)
owner_specs = [spec for spec in resolution["specs"] if spec["name"] == "owner-1.0.0"]
result = repr(len(owner_specs) == 1 and owner_specs[0].get("build_deps") == None)
"#
    );

    assert_eq!(eval_prelude_source_to_repr(source).unwrap(), "True");
}

#[test]
fn prelude_cargo_host_build_compiles_a_build_dependency_once() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def package(name):
    return {{
        "id": "registry+https://registry.example/index#" + name + "@1.0.0",
        "name": name,
        "version": "1.0.0",
        "source": "registry+https://registry.example/index",
        "manifest_path": "/workspace/vendor/" + name + "/Cargo.toml",
        "targets": [{{
            "name": name,
            "kind": ["lib"],
            "crate_types": ["lib"],
            "src_path": "/workspace/vendor/" + name + "/src/lib.rs",
            "edition": "2021",
        }}],
    }}
owner = package("owner")
owner["targets"].append({{
    "name": "build-script-build",
    "kind": ["custom-build"],
    "crate_types": ["bin"],
    "src_path": "/workspace/vendor/owner/build.rs",
    "edition": "2021",
}})
helper = package("helper")
metadata = {{
    "packages": [owner, helper],
    "resolve": {{"nodes": [
        {{
            "id": owner["id"],
            "features": [],
            "deps": [{{
                "name": "helper",
                "pkg": helper["id"],
                "dep_kinds": [{{"kind": "build"}}],
            }}],
        }},
        {{"id": helper["id"], "features": [], "deps": []}},
    ]}},
}}
def names(split_host):
    resolution = _cargo_metadata_resolution({{
        "label": {{"package": "", "name": "deps", "id": "deps"}},
        "attrs": {{"vendor_dir": "vendor", "split_host_variants": split_host}},
    }}, metadata)
    return sorted([spec["name"] for spec in resolution["specs"]])
result = repr([names(False), names(True)])
"#
    );

    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        "[[\"helper-1.0.0\", \"owner-1.0.0\"],          [\"helper-1.0.0\", \"helper-1.0.0-host\", \"owner-1.0.0\"]]"
            .replace("         ", "")
    );
}

#[test]
fn prelude_cargo_metadata_rejects_missing_generated_dependencies() {
    let prelude = all_prelude_source();
    let error = eval_prelude_function_in(
        prelude,
        "_cargo_metadata_dep_refs",
        r#"({
            "deps": [{
                "name": "missing_dependency",
                "pkg": "registry+https://example.invalid#index@1.0.0",
                "dep_kinds": [{"kind": None}],
            }],
        }, {}, True, False, True)"#,
    )
    .unwrap_err();

    assert!(error.contains("has no generated target"), "{error}");
}

#[test]
fn prelude_cargo_vendor_paths_are_package_relative() {
    let tmp = TempDir::new().expect("tempdir");
    let vendor = tmp.path().join("nested/vendor/itoa");
    std::fs::create_dir_all(&vendor).unwrap();
    std::fs::write(
        vendor.join("Cargo.toml"),
        "[package]\nname = \"itoa\"\nversion = \"1.0.14\"\n",
    )
    .unwrap();
    let manifest = "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\n";
    let checksum = "a".repeat(64);
    let lockfile = format!(
        "version = 3\n\n[[package]]\nname = \"itoa\"\nversion = \"1.0.14\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{checksum}\"\n"
    );
    let package_id = "registry+https://github.com/rust-lang/crates.io-index#itoa@1.0.14";
    let metadata = serde_json::json!({
        "once_snapshot": {
            "inputs": {
                "Cargo.lock": lockfile,
                "Cargo.toml": manifest,
            },
            "selection": {
                "features": [],
                "all_features": false,
                "no_default_features": false,
                "target": "",
                "host": false,
                "host_triple": "test-host-triple",
            },
        },
        "packages": [{
            "id": package_id,
            "name": "itoa",
            "version": "1.0.14",
            "source": "registry+https://github.com/rust-lang/crates.io-index",
            "manifest_path": "/workspace/vendor/itoa/Cargo.toml",
            "targets": [{
                "name": "itoa",
                "kind": ["lib"],
                "crate_types": ["lib"],
                "src_path": "/workspace/vendor/itoa/src/lib.rs",
                "edition": "2021",
            }],
        }],
        "resolve": {
            "nodes": [{"id": package_id, "features": [], "deps": []}],
        },
    })
    .to_string();
    let store = store_for(tmp.path(), "nested");
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def _rustc_toolchain(target):
    return ("rustc", {{}}, "test-host-triple")

attrs = {{"metadata_file": "metadata.json", "vendor_dir": "vendor"}}
ctx = {{
    "label": {{"package": "nested", "name": "deps", "id": "nested/deps"}},
    "attr": attrs,
    "attrs": attrs,
    "files": {{
        "Cargo.toml": {manifest:?},
        "Cargo.lock": {lockfile:?},
        "metadata.json": {metadata:?},
    }},
}}
result = repr(_cargo_dependencies_resolver(ctx))
"#
    );

    let (_, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let result = result.unwrap();
    assert!(
        result.contains("\"srcs\": [\"vendor/itoa/**/*\"]"),
        "{result}"
    );
}

#[test]
fn prelude_cargo_vendor_version_comes_from_the_package_table() {
    let prelude = all_prelude_source();
    let wrong_package = eval_prelude_function_in(
        &prelude,
        "_cargo_package_version_matches",
        r#"("[package]\nversion = \"2.0.0\"\n\n[dependencies]\nother = \"1.0.14\"\n", "1.0.14")"#,
    )
    .unwrap();
    let matching_package = eval_prelude_function_in(
        &prelude,
        "_cargo_package_version_matches",
        r#"("[package]\nname = \"itoa\"\nversion=\"1.0.14\"\n\n[dependencies]\n", "1.0.14")"#,
    )
    .unwrap();

    assert_eq!(wrong_package, "False");
    assert_eq!(matching_package, "True");
}

#[test]
fn prelude_swift_package_dependencies_declares_graph_resolver() {
    let source = format!(
        "{}\nresult = repr(swift_package_dependencies.get(\"resolver\") != None)\n",
        apple_prelude_source()
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(out, "True");
    assert_target_kind_attrs(
        "swift_package_dependencies",
        &[
            "package_path",
            "resolved_file",
            "resolver_inputs",
            "graph_file",
            "vendor_path",
            "allow_network",
            "products",
            "resolved_identities",
            "_remote_identities",
            "_locked_pins",
        ],
    );
}

#[test]
fn prelude_swift_package_resolved_supports_legacy_and_current_schemas() {
    let current = eval_prelude_function(
        "_swiftpm_resolved_pins",
        r#"({
            "version": 3,
            "pins": [{
                "identity": "Swift-Algorithms",
                "kind": "remoteSourceControl",
                "location": "https://github.com/apple/swift-algorithms.git",
                "state": {
                    "revision": "1234567890abcdef",
                    "version": "1.2.0",
                },
            }],
        })"#,
    )
    .unwrap();
    assert!(
        current.contains("\"identity\": \"swift-algorithms\""),
        "{current}"
    );
    assert!(
        current.contains("\"revision\": \"1234567890abcdef\""),
        "{current}"
    );
    assert!(current.contains("\"version\": \"1.2.0\""), "{current}");

    let legacy = eval_prelude_function(
        "_swiftpm_resolved_pins",
        r#"({
            "version": 1,
            "object": {
                "pins": [{
                    "package": "NIO",
                    "repositoryURL": "https://github.com/apple/swift-nio.git",
                    "state": {
                        "branch": "main",
                        "revision": "abcdef0123456789",
                    },
                }],
            },
        })"#,
    )
    .unwrap();
    assert!(legacy.contains("\"identity\": \"nio\""), "{legacy}");
    assert!(legacy.contains("\"branch\": \"main\""), "{legacy}");
    assert!(
        legacy.contains("\"location\": \"https://github.com/apple/swift-nio.git\""),
        "{legacy}"
    );
}

#[test]
fn prelude_swift_package_graph_emits_stable_locked_targets_and_edges() {
    let out = eval_prelude_function(
        "_swiftpm_graph_target_specs",
        r#"([{
            "identity": "swift-algorithms",
            "kind": "remoteSourceControl",
            "location": "https://github.com/apple/swift-algorithms.git",
            "version": "1.2.0",
            "revision": "1234567890abcdef",
            "branch": "",
            "checksum": "",
        }, {
            "identity": "swift-numerics",
            "kind": "registry",
            "location": "swiftlang.swift-numerics",
            "version": "1.0.0",
            "revision": "",
            "branch": "",
            "checksum": "fedcba0987654321",
        }], {
            "identity": "root",
            "dependencies": [{
                "identity": "swift-algorithms",
                "name": "Algorithms",
                "dependencies": [{
                    "identity": "swift-numerics",
                    "name": "Numerics",
                    "dependencies": [],
                }],
            }],
        })"#,
    )
    .unwrap();

    assert!(
        out.contains("\"name\": \"swiftpm-swift-algorithms-revision-1234567890ab\""),
        "{out}"
    );
    assert!(
        out.contains("\"name\": \"swiftpm-swift-numerics-checksum-fedcba098765\""),
        "{out}"
    );
    assert!(
        out.contains("\"deps\": [\"./swiftpm-swift-numerics-checksum-fedcba098765\"]"),
        "{out}"
    );
    assert!(
        out.contains("\"roots\": [\"swiftpm-swift-algorithms-revision-1234567890ab\"]"),
        "{out}"
    );
    assert!(
        out.contains("\"resolved_identities\": [\"swift-algorithms\", \"swift-numerics\"]"),
        "{out}"
    );
    assert!(
        out.contains("\"_remote_identities\": [\"swift-algorithms\", \"swift-numerics\"]"),
        "{out}"
    );
    assert!(out.contains("\"_locked_pins\":"), "{out}");
}

#[test]
fn prelude_swift_package_graph_must_contain_every_locked_pin() {
    let error = eval_prelude_function(
        "_swiftpm_graph_target_specs",
        r#"([{
            "identity": "swift-algorithms",
            "kind": "remoteSourceControl",
            "location": "https://github.com/apple/swift-algorithms.git",
            "version": "1.2.0",
            "revision": "1234567890abcdef",
            "branch": "",
            "checksum": "",
        }], {
            "identity": "root",
            "dependencies": [],
        })"#,
    )
    .unwrap_err();

    assert!(error.contains("is missing"), "{error}");
}

#[test]
fn prelude_swift_package_graph_rejects_unlocked_remote_nodes() {
    let traversal =
        eval_prelude_string_function("_swiftpm_workspace_path", r#"("../outside")"#).unwrap();
    assert!(traversal.is_empty());

    let error = eval_prelude_function(
        "_swiftpm_graph_target_specs",
        r#"([], {
            "identity": "root",
            "dependencies": [{
                "identity": "swift-log",
                "name": "Logging",
                "url": "https://github.com/apple/swift-log.git",
                "path": ".build/checkouts/swift-log",
                "dependencies": [],
            }],
        })"#,
    )
    .unwrap_err();

    assert!(
        error.contains("neither locked nor a workspace-local dependency"),
        "{error}"
    );
}

#[test]
fn prelude_swift_package_snapshot_must_match_the_manifest() {
    let resolved = serde_json::json!({"version": 3, "pins": []}).to_string();
    let graph = serde_json::json!({
        "once_manifest": "old manifest\n",
        "once_resolved": resolved.clone(),
        "identity": "root",
        "dependencies": [],
    })
    .to_string();
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "Packages", "id": "Packages"}},
    "attrs": {{"graph_file": "graph.json"}},
    "files": {{
        "Package.swift": "new manifest\n",
        "Package.resolved": {resolved:?},
        "graph.json": {graph:?},
    }},
}}
result = repr(_swift_package_dependencies_resolver(ctx))
"#
    );
    let error = eval_prelude_source_to_repr(source).unwrap_err();

    assert!(
        error.contains("stale relative to `Package.swift`"),
        "{error}"
    );

    let graph = serde_json::json!({
        "once_manifest": "new manifest\n",
        "once_resolved": "old resolved\n",
        "identity": "root",
        "dependencies": [],
    })
    .to_string();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "Packages", "id": "Packages"}},
    "attrs": {{"graph_file": "graph.json"}},
    "files": {{
        "Package.swift": "new manifest\n",
        "Package.resolved": {resolved:?},
        "graph.json": {graph:?},
    }},
}}
result = repr(_swift_package_dependencies_resolver(ctx))
"#
    );
    let error = eval_prelude_source_to_repr(source).unwrap_err();

    assert!(
        error.contains("stale relative to `Package.resolved`"),
        "{error}"
    );

    let graph = serde_json::json!({
        "once_manifest": "new manifest\n",
        "once_resolved": resolved.clone(),
        "once_inputs": {
            "Package.swift": "new manifest\n",
            "Package.resolved": resolved.clone(),
            "Vendor/Local/Package.swift": "old local manifest\n",
        },
        "identity": "root",
        "dependencies": [],
    })
    .to_string();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "Packages", "id": "Packages"}},
    "attrs": {{"graph_file": "graph.json"}},
    "files": {{
        "Package.swift": "new manifest\n",
        "Package.resolved": {resolved:?},
        "Vendor/Local/Package.swift": "new local manifest\n",
        "graph.json": {graph:?},
    }},
}}
result = repr(_swift_package_dependencies_resolver(ctx))
"#
    );
    let error = eval_prelude_source_to_repr(source).unwrap_err();

    assert!(
        error.contains("input binding does not match resolver_inputs"),
        "{error}"
    );
}

#[test]
fn prelude_swift_package_build_directory_uses_canonical_triple() {
    let macos = eval_prelude_string_function(
        "_swiftpm_build_triple_dir",
        r#"("macos", "simulator", "arm64")"#,
    )
    .unwrap();
    assert_eq!(macos, "arm64-apple-macosx");

    let ios = eval_prelude_string_function(
        "_swiftpm_build_triple_dir",
        r#"("ios", "simulator", "arm64")"#,
    )
    .unwrap();
    assert_eq!(ios, "arm64-apple-ios-simulator");
}

#[test]
fn prelude_apple_swiftmodule_triple_uses_framework_module_layout() {
    let ios = eval_prelude_string_function(
        "_apple_swiftmodule_triple",
        r#"("ios", "simulator", "arm64", False)"#,
    )
    .unwrap();
    assert_eq!(ios, "arm64-apple-ios-simulator");

    let catalyst = eval_prelude_string_function(
        "_apple_swiftmodule_triple",
        r#"("macos", "simulator", "arm64", True)"#,
    )
    .unwrap();
    assert_eq!(catalyst, "arm64-apple-ios-macabi");
}

#[test]
fn prelude_swift_package_files_follow_package_path() {
    let manifest =
        eval_prelude_string_function("_swiftpm_manifest_file", r#"({"package_path": "swift"})"#)
            .unwrap();
    let resolved = eval_prelude_string_function(
        "_swiftpm_package_file",
        r#"({"package_path": "swift"}, "Package.resolved")"#,
    )
    .unwrap();
    let graph = eval_prelude_string_function(
        "_swiftpm_package_file",
        r#"({"package_path": "swift"}, "dependencies.json")"#,
    )
    .unwrap();

    assert_eq!(manifest, "swift/Package.swift");
    assert_eq!(resolved, "swift/Package.resolved");
    assert_eq!(graph, "swift/dependencies.json");
}

#[test]
fn prelude_swift_package_default_executable_matches_compiler_toolchain() {
    let swift = eval_prelude_string_function(
        "_swiftpm_swift_executable",
        r#"("swift", "", "/Applications/Xcode.app/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc")"#,
    )
    .unwrap();
    assert_eq!(
        swift,
        "/Applications/Xcode.app/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift"
    );

    let pinned = eval_prelude_string_function(
        "_swiftpm_swift_executable",
        r#"("swift", "/Applications/Xcode-Next.app/Contents/Developer", "/ignored/swiftc")"#,
    )
    .unwrap();
    assert_eq!(
        pinned,
        "/Applications/Xcode-Next.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift"
    );
}

#[test]
fn prelude_swift_package_example_expands_local_dependency_pin() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("prelude/examples/swift-package-dependencies-minimal");
    let graph = once_frontend::load_graph_workspace(&root).expect("Swift package example loads");
    let owner = graph
        .iter()
        .find(|target| target.label.id == "Packages")
        .expect("Swift package dependency owner");
    let pin = graph
        .iter()
        .find(|target| target.label.id == "swiftpm-greeting-local")
        .expect("resolved local package pin");

    assert_eq!(owner.deps, vec!["swiftpm-greeting-local"]);
    assert_eq!(pin.kind, "swift_package_pin");
    assert_eq!(
        pin.attrs.get("identity"),
        Some(&once_frontend::AttrValue::String("greeting".to_string()))
    );
    assert_eq!(
        pin.attrs.get("source_kind"),
        Some(&once_frontend::AttrValue::String(
            "localSourceControl".to_string()
        ))
    );
}

#[test]
fn prelude_cargo_dependencies_expands_locked_packages_into_graph_targets() {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("prelude/examples/rust-binary-with-crate");
    let graph = once_frontend::load_graph_workspace(&root).expect("Cargo example graph loads");
    let owner = graph
        .iter()
        .find(|target| target.label.id == "cargo_dependencies")
        .expect("cargo_dependencies owner");
    let crate_target = graph
        .iter()
        .find(|target| target.label.id == "itoa-1.0.14")
        .expect("resolved itoa target");

    assert_eq!(owner.deps, vec!["itoa-1.0.14"]);
    assert_eq!(crate_target.kind, "rust_crate");
    assert_eq!(
        crate_target.attrs.get("version"),
        Some(&once_frontend::AttrValue::String("1.0.14".to_string()))
    );
    assert_eq!(
        crate_target.attrs.get("checksum"),
        Some(&once_frontend::AttrValue::String(
            "d75a2a4b1b190afb6f5425f10f6a8f959d2ea0b9c2b1d79553551850539e4674".to_string()
        ))
    );
}

#[test]
fn prelude_cargo_resolver_specs_use_named_build_dependency_role() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "",
        "name": "cargo_dependencies",
        "id": "cargo_dependencies",
    }},
    "attr": {{
        "dep_rustc_flags": ["-C", "panic=abort", "-C", "opt-level=2"],
    }},
}}
targets = _cargo_resolver_target_specs(ctx, [{{
    "name": "builder-1.0.0-host",
    "kind": "rust_crate",
    "deps": ["./runtime-1.0.0"],
    "build_deps": ["./macro-1.0.0"],
    "host_tool": True,
    "srcs": ["vendor/builder/src/**/*.rs"],
    "attrs": {{
        "package_name": "builder",
        "version": "1.0.0",
    }},
}}])
result = repr(targets)
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert!(
        out.contains("\"dependencies\": {\"build_deps\": [\"./macro-1.0.0\"]}"),
        "{out}"
    );
    assert!(out.contains("\"deps\": [\"./runtime-1.0.0\"]"), "{out}");
    assert!(
        out.contains("\"rustc_flags\": [\"-C\", \"opt-level=2\"]"),
        "{out}"
    );
    assert!(!out.contains("\"host_tool\""), "{out}");
    assert!(
        !out.contains("\"build_deps\": [\"./macro-1.0.0\"], \"host_tool\""),
        "{out}"
    );
}

#[test]
fn prelude_cargo_dependencies_aggregates_resolved_target_providers() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "",
        "name": "cargo_dependencies",
        "id": "cargo_dependencies",
    }},
    "attr": {{
        "_cargo_resolved": True,
        "_cargo_workspace_deps": {{
            "hello": ["itoa-1.0.14"],
        }},
        "_cargo_workspace_dep_aliases": {{
            "hello": {{"itoa-1.0.14": "formatted_integer"}},
        }},
    }},
    "deps": [
        {{
            "label_id": "cargo_dependencies/itoa-1.0.14",
            "package_name": "itoa",
            "crate_name": "itoa",
            "rlib": ".once/out/cargo_dependencies/itoa-1.0.14/libitoa.rlib",
        }},
        {{
            "label_id": "cargo_dependencies/transitive-1.0.0",
            "package_name": "transitive",
            "crate_name": "transitive",
            "rlib": ".once/out/cargo_dependencies/transitive-1.0.0/libtransitive.rlib",
        }},
    ],
    "srcs": [],
}}
result = repr(_cargo_dependencies_impl(ctx))
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert!(out.contains("\"dependency_set\": True"), "{out}");
    assert!(
        out.contains("\"extern_name\": \"formatted_integer\""),
        "{out}"
    );
    assert!(out.contains("cargo_dependencies/transitive-1.0.0"), "{out}");
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
    by_name["anyhow-1.0.102-x86_64-pc-windows-msvc"]["attrs"]["crate_root"],
    by_name["anyhow-1.0.102-x86_64-pc-windows-msvc"]["attrs"]["build_script"],
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

def host_command(argv, env = None, merge_stderr = None):
    if len(argv) >= 3 and argv[1] == "--print" and argv[2] == "cfg":
        return "target_arch=\"x86_64\"\nwindows\n"
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
target = {{target["name"]: target for target in targets}}["anyhow-1.0.102-x86_64-pc-windows-msvc"]
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
        "cargo_dependencies_x86_64_pc_windows_msvc/anyhow-1.0.102-x86_64-pc-windows-msvc",
    );

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    let rustc = store
        .actions
        .iter()
        .find(|action| {
            action.identifier.as_deref()
                == Some("cargo_dependencies_x86_64_pc_windows_msvc/anyhow-1.0.102-x86_64-pc-windows-msvc:rustc")
        })
        .expect("rustc action");
    assert_eq!(rustc.arg_files.len(), 1);
    let arg_file = &rustc.arg_files[0];
    assert_eq!(arg_file.format, DeclaredArgFileFormat::LineDelimited);
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
    by_name["quote-1.0.45-x86_64-apple-darwin"]["attrs"].get("target"),
    by_name["quote-1.0.45-x86_64-apple-darwin-host"]["attrs"].get("target"),
    by_name["linktime-proc-macro-0.2.0-x86_64-apple-darwin"]["attrs"].get("target"),
    by_name["linktime-proc-macro-0.2.0-x86_64-apple-darwin"]["deps"],
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(
        out,
        "[\"x86_64-apple-darwin\", None, None, [\"./quote-1.0.45-x86_64-apple-darwin-host\"]]"
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
    by_name["cpufeatures-0.2.17-x86_64-apple-darwin"]["deps"],
    by_name["cpufeatures-0.2.17-x86_64-apple-darwin-host"]["deps"],
    by_name["cpufeatures-0.2.17-x86_64-apple-darwin-host"]["attrs"].get("target"),
    by_name["libc-0.2.186-x86_64-apple-darwin-host"]["attrs"].get("target"),
    by_name["cpufeatures-0.2.17-x86_64-apple-darwin"].get("host_tool"),
    by_name["cpufeatures-0.2.17-x86_64-apple-darwin-host"].get("host_tool"),
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(
        out,
        "[[], [\"./libc-0.2.186-x86_64-apple-darwin-host\"], None, None, False, True]"
    );
}

#[test]
fn prelude_cargo_spec_rustc_flags_strip_panic_for_host_loaded_crates() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "cargo_dependencies",
        "name": "cargo_dependencies",
        "id": "cargo_dependencies",
    }},
    "attr": {{
        "dep_rustc_flags": [
            "-C", "panic=abort",
            "-Cpanic=abort",
            "--codegen", "panic=abort",
            "--codegen=panic=abort",
            "-C", "opt-level=3",
            "--codegen", "units=1",
            "--cfg", "keep",
        ],
    }},
}}
normal = _cargo_spec_rustc_flags(ctx, {{
    "name": "normal-1.0.0",
    "kind": "rust_crate",
}})
proc_macro = _cargo_spec_rustc_flags(ctx, {{
    "name": "macro-1.0.0",
    "kind": "rust_proc_macro",
}})
host_tool = _cargo_spec_rustc_flags(ctx, {{
    "name": "normal-1.0.0-host",
    "kind": "rust_crate",
    "host_tool": True,
}})
result = repr([normal, proc_macro, host_tool])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(
        out,
        "[[\"-C\", \"panic=abort\", \"-Cpanic=abort\", \"--codegen\", \"panic=abort\", \"--codegen=panic=abort\", \"-C\", \"opt-level=3\", \"--codegen\", \"units=1\", \"--cfg\", \"keep\"], [\"-C\", \"opt-level=3\", \"--codegen\", \"units=1\", \"--cfg\", \"keep\"], [\"-C\", \"opt-level=3\", \"--codegen\", \"units=1\", \"--cfg\", \"keep\"]]"
    );
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
    by_name["document-features-0.2.12-x86_64-apple-darwin"]["attrs"]["features"],
    by_name["document-features-0.2.12-x86_64-apple-darwin"]["deps"],
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(
        out,
        "[[\"default\"], [\"./litrs-1.0.0-x86_64-apple-darwin-host\"]]"
    );
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
    if name in ["cat", "printf", "sh", "tee"]:
        return "C:/Tools/" + name + ".exe"
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
    assert!(script.contains("$ownBuildScriptStdout = '.once/out/pkg/build script.stdout'"));
    assert!(script.contains("Add-OwnBuildScriptDirectives $ownBuildScriptStdout"));
    assert!(script.contains("function Add-LinkSearchDirectives($path)"));
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
fn prelude_windows_rustc_replays_dependency_build_script_link_searches() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "windows"

def host_env(name):
    return ""

def host_which(name):
    if name == "powershell":
        return "C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
    fail("unexpected host_which call: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if len(argv) >= 3 and argv[1] == "--print" and argv[2] == "cfg":
        return "target_arch=\"x86_64\"\nwindows\n"
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
        "crate_root": "src/main.rs",
    }},
    "deps": [{{
        "label_id": "third_party/native",
        "crate_name": "native",
        "rlib": ".once/out/native/libnative-THIRD_PARTY_NATIVE.rlib",
        "transitive_build_script_outputs": [
            ".once/out/native/build-script.stdout",
        ],
        "transitive_build_script_inputs": [
            "third_party/rust/vendor/windows_x86_64_msvc-0.52.6/lib/windows.0.52.0.lib",
        ],
    }}],
    "srcs": ["src/**/*.rs"],
}}
_rust_compile(ctx, "bin", "src/main.rs", "app.exe")
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
    assert_eq!(
        rustc.argv[0],
        "C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
    );
    assert!(rustc
        .argv
        .iter()
        .any(|arg| arg == "@.once/tmp/analysis/crates/app/app/rustc.rsp"));
    for input in [
        ".once/out/native/build-script.stdout",
        "third_party/rust/vendor/windows_x86_64_msvc-0.52.6/lib/windows.0.52.0.lib",
    ] {
        assert!(
            rustc.inputs.iter().any(|candidate| candidate == input),
            "{input} missing from {:?}",
            rustc.inputs
        );
    }
    let wrapper_write = store
        .actions
        .iter()
        .find(|action| {
            action
                .outputs
                .iter()
                .any(|output| output == ".once/out/crates/app/app/rustc-build-script-wrapper.ps1")
        })
        .expect("wrapper should be written before rustc action");
    let Some(DeclaredActionOperation::WriteFile { bytes, .. }) = &wrapper_write.operation else {
        panic!("wrapper action should write a file");
    };
    let script = String::from_utf8(bytes.clone()).unwrap();
    assert!(script.contains(
        "foreach ($dependencyBuildScriptStdout in @('.once/out/native/build-script.stdout'))"
    ));
    assert!(script.contains("Add-LinkSearchDirectives $dependencyBuildScriptStdout"));
    assert!(script.contains("[void]$dynamicRustcArgs.Add('-L')"));
}

#[test]
fn prelude_windows_build_script_compile_env_includes_proc_macro_path() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "windows"

def host_env(name):
    if name == "PATH":
        return "C:/Windows/System32"
    return ""

def host_which(name):
    if name == "powershell":
        return "C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
    if name in ["cat", "printf", "sh", "tee"]:
        return "C:/Tools/" + name + ".exe"
    fail("unexpected host_which call: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if len(argv) >= 3 and argv[1] == "--print" and argv[2] == "cfg":
        return "target_arch=\"x86_64\"\nwindows\n"
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
        "build_script": "build.rs",
    }},
    "deps": [],
    "deps_by_role": {{"build_deps": [{{
        "label_id": "macros/derive",
        "crate_name": "derive",
        "proc_macro": ".once/out/macros/derive/derive.dll",
    }}]}},
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
    let path = action.env.get("PATH").expect("build script compile PATH");
    let proc_macro_dir = workspace
        .path()
        .join(".once/out/macros/derive")
        .to_string_lossy()
        .into_owned();
    for expected in [
        proc_macro_dir.as_str(),
        "C:/Rust/bin",
        "C:/Rust/lib/rustlib/x86_64-pc-windows-msvc/bin",
        "C:/Windows/System32",
    ] {
        assert!(path.split(';').any(|entry| entry == expected), "{path}");
    }
}

#[test]
fn prelude_windows_proc_macro_search_is_reused_and_transitive() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
def host_os():
    return "windows"

def host_env(name):
    return ""

def host_which(name):
    fail("unexpected host_which call: " + name)

def host_command(argv, env = None, merge_stderr = None):
    fail("unexpected host_command call")

def _rustc_toolchain(target):
    return ("C:/Rust/bin/rustc.exe", "rustc-test", "x86_64-pc-windows-msvc")

def rust_ctx(package, name, deps = []):
    return {{
        "label": {{
            "package": package,
            "name": name,
            "id": package + "/" + name,
        }},
        "attr": {{
            "target": "x86_64-pc-windows-msvc",
            "crate_name": name,
            "crate_root": "src/lib.rs",
            "_output_prefix": package + "/" + name + "/",
        }},
        "deps": deps,
        "srcs": ["src/**/*.rs"],
    }}

derive_b = _rust_compile(rust_ctx("macros/derive_b", "derive_b"), "proc-macro", "src/lib.rs", "derive_b.dll")
derive_a = _rust_compile(rust_ctx("macros/derive_a", "derive_a", [derive_b]), "proc-macro", "src/lib.rs", "derive_a.dll")
_rust_compile(rust_ctx("crates/one", "one", [derive_a]), "rlib", "src/lib.rs", "libone.rlib")
_rust_compile(rust_ctx("crates/two", "two", [derive_a]), "rlib", "src/lib.rs", "libtwo.rlib")
result = repr([
    derive_a["transitive_proc_macro_search"],
    derive_a["transitive_proc_macro_externs"],
])
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "crates/one");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let out = out.unwrap();
    for expected in [
        ".once/out/crates/one/macros/derive_a/derive_a/proc-macro-search/derive_a-MACROS_DERIVE_A_DERIVE_A.dll",
        ".once/out/crates/one/macros/derive_b/derive_b/proc-macro-search/derive_b-MACROS_DERIVE_B_DERIVE_B.dll",
        "derive_a=.once/out/crates/one/macros/derive_a/derive_a/proc-macro-search/derive_a-MACROS_DERIVE_A_DERIVE_A.dll",
        "derive_b=.once/out/crates/one/macros/derive_b/derive_b/proc-macro-search/derive_b-MACROS_DERIVE_B_DERIVE_B.dll",
    ] {
        assert!(out.contains(expected), "{out}");
    }
    for staged in [
        ".once/out/crates/one/macros/derive_a/derive_a/proc-macro-search/derive_a-MACROS_DERIVE_A_DERIVE_A.dll",
        ".once/out/crates/one/macros/derive_b/derive_b/proc-macro-search/derive_b-MACROS_DERIVE_B_DERIVE_B.dll",
    ] {
        let count = store
            .actions
            .iter()
            .filter(|action| action.outputs.iter().any(|output| output == staged))
            .count();
        assert_eq!(count, 1, "{staged} should be staged once");
    }
    for target in ["crates/one/one:rustc", "crates/two/two:rustc"] {
        let action = store
            .actions
            .iter()
            .find(|action| action.identifier.as_deref() == Some(target))
            .expect("dependent rustc action");
        let arg_file = action.arg_files.first().expect("dependent response file");
        for expected in [
            "dependency=.once/out/crates/one/macros/derive_a/derive_a/proc-macro-search",
            "dependency=.once/out/crates/one/macros/derive_b/derive_b/proc-macro-search",
            "derive_a=.once/out/crates/one/macros/derive_a/derive_a/proc-macro-search/derive_a-MACROS_DERIVE_A_DERIVE_A.dll",
            "derive_b=.once/out/crates/one/macros/derive_b/derive_b/proc-macro-search/derive_b-MACROS_DERIVE_B_DERIVE_B.dll",
        ] {
            assert!(
                arg_file.args.iter().any(|arg| arg == expected),
                "{expected} missing from {:?}",
                arg_file.args
            );
        }
    }
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

def host_command(argv, env = None, merge_stderr = None):
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
    assert_eq!(arg_file.format, DeclaredArgFileFormat::LineDelimited);
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

def host_command(argv, env = None, merge_stderr = None):
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

def host_command(argv, env = None, merge_stderr = None):
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
        "crate_root": "src\\lib.rs",
        "rustc_flags": [
            "--extern=combined=.once\\out\\manual\\libcombined.rlib",
            "-Ldependency=.once\\out\\manual",
            "--out-dir=.once\\out\\manual-out",
        ],
    }},
    "deps": [{{
        "label_id": "crates/dep/dep",
        "crate_name": "dep",
        "rlib": ".once\\out\\crates\\dep\\dep\\libdep.rlib",
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
    assert_eq!(arg_file.format, DeclaredArgFileFormat::LineDelimited);
    assert!(arg_file.args.iter().any(|arg| arg == "--crate-name"));
    let extern_arg = "dep=.once/out/crates/dep/dep/libdep.rlib";
    let extern_position = arg_file
        .args
        .windows(2)
        .position(|args| args[0] == "--extern" && args[1] == extern_arg)
        .expect("dependency extern flag");
    let crate_root = "crates/app/src/lib.rs";
    let root_position = arg_file
        .args
        .iter()
        .position(|arg| arg == crate_root)
        .expect("crate root");
    assert!(
        extern_position < root_position,
        "dependency flags should precede the crate root: {:?}",
        arg_file.args
    );
    for expected in [
        "--extern=combined=.once/out/manual/libcombined.rlib",
        "-Ldependency=.once/out/manual",
        "--out-dir=.once/out/manual-out",
    ] {
        assert!(
            arg_file.args.iter().any(|arg| arg == expected),
            "{expected} missing from {:?}",
            arg_file.args
        );
    }
    assert_eq!(arg_file.args.last().map(String::as_str), Some(crate_root));
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

def host_command(argv, env = None, merge_stderr = None):
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
    assert_release_dependency_response_file(&store);
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
    _rust_response_extern_arg("foo=.once\\out\\libfoo.rlib"),
    _rust_response_search_path_arg("dependency=.once\\out\\foo"),
    _rust_response_arg("--extern=bar=.once\\out\\libbar.rlib"),
    _rust_response_arg("-Ldependency=.once\\out\\bar"),
    _rust_response_arg("--out-dir=.once\\out\\bar"),
    _rust_response_path_arg("D:\\a\\once\\once\\crates\\foo\\src\\lib.rs"),
    _rust_response_path_arg("--cfg=feature=\"default\""),
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(
        out,
        "[\".once/out/libfoo.rlib\", \"foo=.once/out/libfoo.rlib\", \"dependency=.once/out/foo\", \"--extern=bar=.once/out/libbar.rlib\", \"-Ldependency=.once/out/bar\", \"--out-dir=.once/out/bar\", \"D:/a/once/once/crates/foo/src/lib.rs\", \"--cfg=feature=\\\"default\\\"\"]"
    );
}

fn assert_release_dependency_response_file(store: &AnalysisStore) {
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
        "once_cas=.once/out/crates/once-cas/once_cas_x86_64_pc_windows_msvc/libonce_cas-CRATES_ONCE_CAS_ONCE_CAS_X86_64_PC_WINDOWS_MSVC.rlib",
        "tokio=.once/out/cargo_dependencies_x86_64_pc_windows_msvc/tokio-1.52.3/libtokio-CARGO_DEPENDENCIES_X86_64_PC_WINDOWS_MSVC_TOKIO_1_52_3.rlib",
        "serde=.once/out/cargo_dependencies_x86_64_pc_windows_msvc/serde-1.0.228/libserde-CARGO_DEPENDENCIES_X86_64_PC_WINDOWS_MSVC_SERDE_1_0_228.rlib",
        "tracing=.once/out/cargo_dependencies_x86_64_pc_windows_msvc/tracing-0.1.43/libtracing-CARGO_DEPENDENCIES_X86_64_PC_WINDOWS_MSVC_TRACING_0_1_43.rlib",
    ] {
        assert!(
            arg_file
                .args
                .windows(2)
                .any(|args| args[0] == "--extern" && args[1] == extern_arg),
            "{extern_arg} missing from {:?}",
            arg_file.args
        );
    }
    for input in [
        ".once/out/crates/once-core/once_core_x86_64_pc_windows_msvc/deps-rlib-search/libonce_cas-CRATES_ONCE_CAS_ONCE_CAS_X86_64_PC_WINDOWS_MSVC.rlib",
        ".once/out/crates/once-core/once_core_x86_64_pc_windows_msvc/deps-rlib-search/libtokio-CARGO_DEPENDENCIES_X86_64_PC_WINDOWS_MSVC_TOKIO_1_52_3.rlib",
        ".once/out/crates/once-core/once_core_x86_64_pc_windows_msvc/deps-rlib-search/libserde-CARGO_DEPENDENCIES_X86_64_PC_WINDOWS_MSVC_SERDE_1_0_228.rlib",
        ".once/out/crates/once-core/once_core_x86_64_pc_windows_msvc/deps-rlib-search/libtracing-CARGO_DEPENDENCIES_X86_64_PC_WINDOWS_MSVC_TRACING_0_1_43.rlib",
    ] {
        assert!(
            rustc.inputs.iter().any(|candidate| candidate == input),
            "{input} missing from {:?}",
            rustc.inputs
        );
    }
    let crate_root = "crates/once-core/src/lib.rs";
    let root_position = arg_file
        .args
        .iter()
        .position(|arg| arg == crate_root)
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
    assert_release_dependency_search_path(&arg_file.args);
    assert_eq!(arg_file.args.last().map(String::as_str), Some(crate_root));
}

fn assert_release_dependency_search_path(args: &[String]) {
    let staged_dependency =
        "dependency=.once/out/crates/once-core/once_core_x86_64_pc_windows_msvc/deps-rlib-search";
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
    assert!(
        !store.actions[0].argv.iter().any(|arg| arg == "--target"),
        "{:?}",
        store.actions[0].argv
    );
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

def host_command(argv, env = None, merge_stderr = None):
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

def host_command(argv, env = None, merge_stderr = None):
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

#[test]
fn prelude_apple_ui_test_install_replaces_the_application_under_test() {
    let script = eval_prelude_string_function(
        "_apple_ui_test_install_script",
        r#"("/usr/bin/xcrun", "org.example.App", ".once/out/App/App.app")"#,
    )
    .unwrap();

    assert!(
        script.contains("simctl terminate \"$simulator_id\" 'org.example.App'"),
        "{script}"
    );
    assert!(
        script.contains("simctl uninstall \"$simulator_id\" 'org.example.App'"),
        "{script}"
    );
    assert!(
        script.contains("simctl install \"$simulator_id\" '.once/out/App/App.app'"),
        "{script}"
    );
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
fn prelude_apple_application_visible_run_opens_simulator() {
    let call = r#"(
        "apps/ios/App",
        "ios",
        "simulator",
        "/usr/bin/xcrun",
        ".once/out/apps/ios/App/App.app",
        "dev.once.App",
        ".once/out/apps/ios/App/run",
        ".once/out/apps/ios/App/run/run.json",
        ".once/out/apps/ios/App/run/run.log",
        True,
    )"#;
    let script = eval_prelude_string_function("_apple_application_run_script", call).unwrap();

    assert!(
        script.contains("/usr/bin/open -a Simulator --args -CurrentDeviceUDID \"$simulator_id\""),
        "{script}"
    );
}

#[test]
fn prelude_apple_application_default_run_does_not_open_simulator() {
    let call = r#"(
        "apps/ios/App",
        "ios",
        "simulator",
        "/usr/bin/xcrun",
        ".once/out/apps/ios/App/App.app",
        "dev.once.App",
        ".once/out/apps/ios/App/run",
        ".once/out/apps/ios/App/run/run.json",
        ".once/out/apps/ios/App/run/run.log",
        False,
    )"#;
    let script = eval_prelude_string_function("_apple_application_run_script", call).unwrap();

    assert!(!script.contains("/usr/bin/open -a Simulator"), "{script}");
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
    assert_eq!(&bytes[0..4], &0x686D_6170_u32.to_le_bytes());
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
        r#"({}, {"platform": {"select": {"default": "ios"}}}, "tgt")"#,
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

def host_command(argv, env = None, merge_stderr = None):
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

def host_command(argv, env = None, merge_stderr = None):
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

def host_command(argv, env = None, merge_stderr = None):
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
/// still produce a direct tool invocation, and the action argv must
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

def host_command(argv, env = None, merge_stderr = None):
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

def host_command(argv, env = None, merge_stderr = None):
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

def host_command(argv, env = None, merge_stderr = None):
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

def host_command(argv, env = None, merge_stderr = None):
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

def host_command(argv, env = None, merge_stderr = None):
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

def host_command(argv, env = None, merge_stderr = None):
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
    let compiler_actions = store
        .actions
        .iter()
        .filter(|action| !action.argv.is_empty())
        .collect::<Vec<_>>();
    assert!(!compiler_actions.is_empty(), "expected compiler actions");
    let swiftc = "/opt/Xcode/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc";
    let libtool = "/opt/Xcode/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/libtool";
    assert!(
        compiler_actions
            .iter()
            .any(|action| action.argv[0] == swiftc),
        "expected at least one Swift compiler action"
    );
    for action in compiler_actions {
        for arg in &action.argv {
            assert!(
                !arg.contains("xcrun"),
                "direct-mode argv should not mention xcrun: {:?}",
                action.argv
            );
        }
        assert!(
            action.argv[0] == swiftc || action.argv[0] == libtool,
            "first argument should be a resolved toolchain executable: {:?}",
            action.argv
        );
        // The action env carries DEVELOPER_DIR through to the tool so
        // it can find ancillary resources next to swiftc.
        assert_eq!(
            action.env.get("DEVELOPER_DIR").map(String::as_str),
            Some("/opt/Xcode/Developer"),
        );
    }
}

#[test]
fn prelude_apple_library_preserves_one_canonical_authored_modulemap() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let source_dir = workspace.path().join("ios/CLib/Sources");
    let include_dir = workspace.path().join("ios/CLib/include");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::create_dir_all(&include_dir).unwrap();
    std::fs::write(source_dir.join("CLib.m"), "#include \"CLib.h\"\n").unwrap();
    std::fs::write(source_dir.join("Fast.S"), "#if 0\n#endif\n").unwrap();
    std::fs::write(
        source_dir.join("CLib-Prefix.pch"),
        "#include <Foundation/Foundation.h>\n",
    )
    .unwrap();
    std::fs::write(include_dir.join("CLib.h"), "void clib(void);\n").unwrap();
    std::fs::write(
        include_dir.join("module.modulemap"),
        "module CLib { header \"CLib.h\" export * }\n",
    )
    .unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    fail("host_which must not be called in direct mode")

def host_command(argv, env = None, merge_stderr = None):
    if "--version" in argv:
        return "Apple clang version 18.0\n"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{"package": "ios/CLib", "name": "CLib", "id": "ios/CLib/CLib"}},
    "attr": {{
        "platform": "ios",
        "sdk_variant": "simulator",
        "xcode_developer_dir": "/opt/Xcode/Developer",
        "enable_modules": True,
        "modulemap": "ios/CLib/include/module.modulemap",
        "prefix_header": "Sources/CLib-Prefix.pch",
        "exported_headers": ["include/CLib.h"],
        "exported_header_dirs": ["include"],
    }},
    "deps": [],
    "srcs": ["Sources/CLib.m", "Sources/Fast.S"],
    "build_dir": ".once/out/ios/CLib/CLib",
    "capability": "build",
}}
provider = _apple_library_impl(ctx)
result = repr(provider)
"#
    );
    let store = store_for(workspace.path(), "ios/CLib");
    let (store, provider) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let provider = provider.unwrap();
    let canonical = "ios/CLib/include/module.modulemap";
    assert!(provider.contains(&format!(r#""modulemap": "{canonical}""#)));
    assert!(provider.contains(r#""transitive_swiftmodule_dirs": []"#));
    assert!(!store.actions.iter().any(|action| action
        .outputs
        .iter()
        .any(|output| output.ends_with("authored.modulemap"))));
    let compiler = store
        .actions
        .iter()
        .find(|action| {
            action
                .identifier
                .as_deref()
                .is_some_and(|identifier| identifier.starts_with("clang_compile_CLib"))
        })
        .expect("Clang compiler action");
    assert!(compiler
        .argv
        .contains(&format!("-fmodule-map-file={canonical}")));
    assert!(compiler.inputs.contains(&canonical.to_string()));
    assert!(compiler
        .argv
        .windows(2)
        .any(|args| { args == ["-include", "ios/CLib/Sources/CLib-Prefix.pch"] }));
    assert!(compiler
        .inputs
        .contains(&"ios/CLib/Sources/CLib-Prefix.pch".to_string()));
    let assembler = store
        .actions
        .iter()
        .find(|action| {
            action
                .identifier
                .as_deref()
                .is_some_and(|identifier| identifier.contains("Fast.S"))
        })
        .expect("assembly compiler action");
    assert!(assembler
        .argv
        .windows(2)
        .any(|args| args == ["-x", "assembler-with-cpp"]));
    assert!(!assembler.argv.iter().any(|arg| arg == "-fmodules"));
    assert!(!assembler
        .argv
        .iter()
        .any(|arg| arg.starts_with("-fmodule-map-file=")));
    assert!(!assembler.argv.iter().any(|arg| arg == "-include"));
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the inline Starlark fixture keeps this module map contract in one test"
)]
fn prelude_apple_library_stages_authored_framework_modulemap() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let source_dir = workspace.path().join("ios/Logging/Sources");
    let support_dir = workspace.path().join("ios/Logging/Support");
    let auxiliary_dir = workspace.path().join("ios/Logging/Auxiliary");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::create_dir_all(&support_dir).unwrap();
    std::fs::create_dir_all(&auxiliary_dir).unwrap();
    std::fs::write(source_dir.join("Logging.swift"), "public func log() {}\n").unwrap();
    std::fs::write(source_dir.join("Private.h"), "void private_log(void);\n").unwrap();
    std::fs::write(
        support_dir.join("Logging.modulemap"),
        "framework module Logging {\n  umbrella header \"Logging-umbrella.h\"\n  explicit module Internal { header \"Private.h\" }\n  export *\n}\n",
    )
    .unwrap();
    std::fs::write(
        support_dir.join("Logging-umbrella.h"),
        "void logging(void);\n",
    )
    .unwrap();
    std::fs::write(
        auxiliary_dir.join("module.modulemap"),
        "module LoggingNative { header \"LoggingNative.h\" }\n",
    )
    .unwrap();
    std::fs::write(
        auxiliary_dir.join("LoggingNative.h"),
        "void logging_native(void);\n",
    )
    .unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    fail("host_which must not be called in direct mode")

def host_command(argv, env = None, merge_stderr = None):
    if "--version" in argv:
        return "Swift version 6.0\n"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{"package": "", "name": "Logging", "id": "Logging"}},
    "attr": {{
        "platform": "ios",
        "sdk_variant": "simulator",
        "xcode_developer_dir": "/opt/Xcode/Developer",
        "enable_modules": True,
        "modulemap": "ios/Logging/Support/Logging.modulemap",
        "modulemap_headers": ["ios/Logging/Sources/Private.h"],
        "auxiliary_modulemaps": ["ios/Logging/Auxiliary/module.modulemap"],
    }},
    "deps": [],
    "srcs": ["ios/Logging/Sources/Logging.swift"],
    "build_dir": ".once/out/Logging",
    "capability": "build",
}}
provider = _apple_library_impl(ctx)
result = repr(provider)
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        String::new(),
        ".once/out/Logging".to_string(),
    );
    let (store, provider) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let provider = provider.unwrap();
    let consumer_map = ".once/out/Logging/Logging.framework/Modules/module.modulemap";
    let compile_map = ".once/out/Logging/Unextended/Logging.framework/Modules/module.modulemap";
    let compile_header =
        ".once/out/Logging/Unextended/Logging.framework/Headers/Logging-umbrella.h";
    let private_header = ".once/out/Logging/Unextended/Logging.framework/Headers/Private.h";
    assert!(provider.contains(consumer_map), "{provider}");
    assert!(
        provider.contains("ios/Logging/Auxiliary/module.modulemap"),
        "{provider}"
    );
    assert!(
        provider.contains("ios/Logging/Auxiliary/LoggingNative.h"),
        "{provider}"
    );
    assert!(provider.contains(r#""transitive_framework_search_dirs": [".once/out/Logging"]"#));
    assert!(store.actions.iter().any(|action| {
        action
            .identifier
            .as_deref()
            .is_some_and(|identifier| identifier == "clean_framework_module_Logging")
    }));
    assert!(store.actions.iter().any(|action| {
        action
            .identifier
            .as_deref()
            .is_some_and(|identifier| identifier == "clean_unextended_framework_module_Logging")
    }));
    let compiler = action_by_identifier(&store, "swift_module_compile_Logging");
    assert!(compiler
        .argv
        .iter()
        .any(|arg| arg == "-explicit-module-build"));
    assert!(compiler.inputs.contains(&compile_map.to_string()));
    assert!(compiler
        .inputs
        .contains(&"ios/Logging/Auxiliary/module.modulemap".to_string()));
    assert!(compiler
        .inputs
        .contains(&"ios/Logging/Auxiliary/LoggingNative.h".to_string()));
    assert!(compiler.inputs.contains(&private_header.to_string()));
    assert!(compiler
        .argv
        .iter()
        .any(|arg| arg == "-fmodule-map-file=ios/Logging/Auxiliary/module.modulemap"));
    assert!(
        compiler.inputs.contains(&compile_header.to_string()),
        "{:?}",
        compiler.inputs
    );
    assert!(!compiler
        .argv
        .iter()
        .any(|arg| arg == "-fmodule-map-file=ios/Logging/Support/Logging.modulemap"));
    let consumer_action = store
        .actions
        .iter()
        .find(|action| action.outputs.contains(&consumer_map.to_string()))
        .expect("consumer framework module map action");
    let Some(DeclaredActionOperation::WriteFile { bytes, .. }) = &consumer_action.operation else {
        panic!("consumer module map must be written deterministically");
    };
    let contents = std::str::from_utf8(bytes).unwrap();
    assert!(contents.contains("framework module Logging"));
    assert!(contents.contains("module Logging.Swift"));
}

#[test]
fn prelude_apple_library_lists_public_headers_in_generated_modulemap() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let source_dir = workspace.path().join("ios/CMark/Sources");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(source_dir.join("cmark.c"), "#include \"cmark.h\"\n").unwrap();
    std::fs::write(source_dir.join("cmark.h"), "void cmark(void);\n").unwrap();
    std::fs::write(source_dir.join("node.h"), "void node(void);\n").unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    fail("host_which must not be called in direct mode")

def host_command(argv, env = None, merge_stderr = None):
    if "--version" in argv:
        return "Apple clang version 18.0\n"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{"package": "ios/CMark", "name": "libcmark", "id": "ios/CMark/libcmark"}},
    "attr": {{
        "platform": "ios",
        "sdk_variant": "simulator",
        "xcode_developer_dir": "/opt/Xcode/Developer",
        "module_name": "libcmark",
        "enable_modules": True,
        "exported_headers": ["Sources/cmark.h", "Sources/node.h"],
        "exported_header_dirs": ["Sources"],
    }},
    "deps": [],
    "srcs": ["Sources/cmark.c"],
    "build_dir": ".once/out/ios/CMark/libcmark",
    "capability": "build",
}}
provider = _apple_library_impl(ctx)
result = repr(provider)
"#
    );
    let store = store_for(workspace.path(), "ios/CMark");
    let (store, provider) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let provider = provider.unwrap();
    assert!(provider.contains(r#""transitive_swiftmodule_dirs": []"#));
    let action = store
        .actions
        .iter()
        .find(|action| {
            matches!(
                &action.operation,
                Some(DeclaredActionOperation::WriteFile { bytes, .. })
                    if bytes.starts_with(b"module libcmark {")
            )
        })
        .expect("generated module map action");
    let Some(DeclaredActionOperation::WriteFile { bytes, .. }) = &action.operation else {
        panic!("module map action must write its contents");
    };
    let contents = std::str::from_utf8(bytes).expect("module map contents are UTF-8");
    assert!(contents.starts_with("module libcmark {"));
    assert!(contents.contains("header \"../../../../../ios/CMark/Sources/cmark.h\""));
    assert!(contents.contains("header \"../../../../../ios/CMark/Sources/node.h\""));
    assert!(!contents.contains("umbrella"));
    assert!(!contents.contains("module cmark"));
    assert!(!contents.contains("module node"));
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the inline Starlark fixture keeps this header distribution contract in one test"
)]
fn prelude_apple_library_stages_distributed_umbrella_headers() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("ios/Lib/Sources");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::create_dir_all(workspace.path().join("ios/Lib/Public")).unwrap();
    std::fs::write(package_dir.join("Lib.swift"), "public func hello() {}\n").unwrap();
    std::fs::write(package_dir.join("Lib.h"), "void lib_log(void);\n").unwrap();
    std::fs::write(package_dir.join("Private.h"), "void private_log(void);\n").unwrap();
    std::fs::write(
        workspace.path().join("ios/Lib/Public/Logging.h"),
        "void log_message(void);\n",
    )
    .unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    fail("host_which must not be called in direct mode (asked for " + name + ")")

def host_command(argv, env = None, merge_stderr = None):
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
        "enable_modules": True,
        "defines": ["DEBUG"],
        "exported_headers": ["Sources/Lib.h", "Public/Logging.h"],
        "private_header_dirs": ["Sources"],
    }},
    "deps": [],
    "srcs": ["Sources/**/*.swift"],
    "build_dir": ".once/out/ios/Lib/Lib",
    "capability": "build",
}}
provider = _apple_library_impl(ctx)
result = repr(provider)
"#
    );
    let store = store_for(workspace.path(), "ios/Lib");
    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let provider = result.unwrap();
    assert!(
        provider.contains(
            "\"transitive_generated_headers\": [\".once/out/ios/Lib/Lib.framework/Headers/Lib-Swift.h\"]"
        ),
        "generated compatibility headers must be exposed as compile dependencies: {provider}"
    );
    assert!(
        provider.contains("\"transitive_modulemaps\": []"),
        "framework modules must be discovered through their framework search path: {provider}"
    );
    assert!(
        provider.contains("\"transitive_framework_search_dirs\": [\".once/out/ios/Lib\"]"),
        "the consumer framework search path must be exported: {provider}"
    );
    assert!(
        provider.contains(
            "\"transitive_framework_files\": [\".once/out/ios/Lib/Lib.framework/Modules/module.modulemap\""
        ),
        "the complete consumer framework must be exposed as action inputs: {provider}"
    );
    assert!(
        provider.contains(
            "\"transitive_vfs_overlays\": [\".once/out/ios/Lib/framework-headers-overlay.yaml\"]"
        ),
        "the framework header overlay must be exported: {provider}"
    );

    let underlying_modulemap =
        ".once/out/ios/Lib/Unextended/Lib.framework/Modules/module.modulemap";
    let modulemap_action = store
        .actions
        .iter()
        .find(|action| action.outputs.contains(&underlying_modulemap.to_string()))
        .expect("modulemap action");
    let Some(DeclaredActionOperation::WriteFile { bytes, .. }) = &modulemap_action.operation else {
        panic!("modulemap action must write its contents");
    };
    let underlying_contents = std::str::from_utf8(bytes).expect("modulemap contents are UTF-8");
    assert!(underlying_contents.starts_with("framework module Lib {"));
    assert!(underlying_contents.contains("umbrella header \"Lib.h\""));
    assert!(underlying_contents.contains("module * { export * }"));
    assert!(!underlying_contents.contains("module Logging"));
    let compiler = store
        .actions
        .iter()
        .find(|action| {
            action
                .identifier
                .as_deref()
                .is_some_and(|identifier| identifier.starts_with("swift_module_compile_Lib"))
        })
        .expect("swift compiler action");
    assert!(
        compiler.argv.windows(2).any(|args| {
            args == [
                "-F".to_string(),
                ".once/out/ios/Lib/Unextended".to_string(),
            ]
        }),
        "compiler must discover its synthetic framework module through a framework search path: {:?}",
        compiler.argv
    );
    assert!(
        compiler.argv.contains(&"-explicit-module-build".to_string()),
        "the Swift driver must preserve imports of inferred submodules while compiling the parent module: {:?}",
        compiler.argv
    );
    assert!(
        !compiler
            .argv
            .contains(&".once/out/ios/Lib/Lib.hmap".to_string()),
        "the synthetic framework headers must retain one physical identity during explicit module scanning: {:?}",
        compiler.argv
    );
    let hmap_action = store
        .actions
        .iter()
        .find(|action| {
            action
                .outputs
                .contains(&".once/out/ios/Lib/Lib.hmap".to_string())
        })
        .expect("header map action");
    let Some(DeclaredActionOperation::WriteFile { bytes, .. }) = &hmap_action.operation else {
        panic!("header map action must write its contents");
    };
    assert!(
        bytes
            .windows(b"Lib/Private.h".len())
            .any(|window| window == b"Lib/Private.h"),
        "the target's own header map must include module-qualified private headers"
    );
    assert!(
        !compiler
            .argv
            .contains(&format!("-fmodule-map-file={underlying_modulemap}")),
        "framework module discovery must preserve inferred submodules: {:?}",
        compiler.argv
    );
    assert!(
        compiler
            .argv
            .contains(&"-import-underlying-module".to_string()),
        "Swift must import the Clang module formed by its own Objective-C headers: {:?}",
        compiler.argv
    );
    assert!(compiler
        .argv
        .windows(2)
        .any(|args| { args == ["-Xcc".to_string(), "-DDEBUG".to_string()] }));
    assert!(
        compiler.inputs.contains(&underlying_modulemap.to_string()),
        "compiler must declare its own module map as an input: {:?}",
        compiler.inputs
    );
    assert!(
        compiler
            .outputs
            .contains(&".once/out/ios/Lib/Lib.framework/Headers/Lib-Swift.h".to_string()),
        "Swift interop header must be reachable through the consumer framework: {:?}",
        compiler.outputs
    );
    assert_eq!(
        compiler.clean_paths,
        vec![
            ".once/out/ios/Lib/Lib/module.modulemap",
            ".once/out/ios/Lib/Lib/swift.modulemap",
            ".once/out/ios/Lib/Lib/underlying.modulemap",
        ]
    );
    let consumer_modulemap = ".once/out/ios/Lib/Lib.framework/Modules/module.modulemap";
    let consumer_action = store
        .actions
        .iter()
        .find(|action| action.outputs.contains(&consumer_modulemap.to_string()))
        .expect("consumer framework module map");
    let Some(DeclaredActionOperation::WriteFile { bytes, .. }) = &consumer_action.operation else {
        panic!("consumer framework module map must be a write action");
    };
    let consumer_contents = std::str::from_utf8(bytes).expect("module map contents are UTF-8");
    assert!(consumer_contents.contains("module Lib.Swift {"));
    assert!(consumer_contents.contains("header \"Lib-Swift.h\""));
    assert!(
        !compiler
            .argv
            .contains(&format!("-fmodule-map-file={consumer_modulemap}")),
        "the library must not import its generated compatibility header while producing it"
    );
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
            r#"({}, {"platform": "ios", "module_name": {"select": {"ios": "X", "default": "Y"}}}, "tgt", ["module_name"])"#,
        )
        .unwrap_err();
    assert!(
        err.contains("attribute `module_name` is not configurable but uses select()"),
        "{err}"
    );
}

#[test]
fn prelude_apple_framework_compile_files_exclude_bundle_resources() {
    assert_eq!(
        eval_prelude_function(
            "_apple_framework_compile_files",
            r#"({"path": ".once/out/Lib/Lib.framework", "module_name": "Lib", "files": [".once/out/Lib/Lib.framework/Lib", ".once/out/Lib/Lib.framework/Modules/module.modulemap", ".once/out/Lib/Lib.framework/Modules/Lib.swiftmodule/arm64-apple-ios-simulator.swiftmodule", ".once/out/Lib/Lib.framework/Headers/Lib-Swift.h", ".once/out/Lib/Lib.framework/en.lproj/Localizable.strings", ".once/out/Lib/Lib.framework/Assets.car", ".once/out/Lib/Lib.framework/_CodeSignature/CodeResources"]},)"#,
        )
        .unwrap(),
        r#"[".once/out/Lib/Lib.framework/Lib", ".once/out/Lib/Lib.framework/Modules/module.modulemap", ".once/out/Lib/Lib.framework/Modules/Lib.swiftmodule/arm64-apple-ios-simulator.swiftmodule", ".once/out/Lib/Lib.framework/Headers/Lib-Swift.h"]"#
    );
}

// ---------------------------------------------------------------------------
// Native Xcode project resolver (`xcode_workspace`)
// ---------------------------------------------------------------------------

#[test]
fn prelude_apple_prebuild_actions_precede_generated_source_compilation() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("App/Sources");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(package_dir.join("App.swift"), "public func app() {}\n").unwrap();
    std::fs::write(package_dir.join("Private.h"), "void private_api(void);\n").unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    if name == "sh":
        return "/bin/sh"
    fail("unexpected host_which: " + name)

def host_command(argv, env = None, merge_stderr = None):
    if "--version" in argv:
        return "Swift version 6.0\n"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{"package": "App", "name": "App", "id": "App/App"}},
    "attr": {{
        "platform": "macos",
        "xcode_developer_dir": "/opt/Xcode/Developer",
        "private_header_dirs": ["Sources"],
        "prebuild_actions": [_json_encode({{
            "name": "Secrets",
            "shell": "/bin/sh",
            "script": "printf 'public let secret = 1\\n' > .once/out/App/App/Generated.swift",
            "inputs": ["App/Sources/App.swift"],
            "outputs": [".once/out/App/App/Generated.swift"],
            "cwd": "",
            "env": {{"SRCROOT": "/workspace/App"}},
            "cacheable": True,
        }})],
    }},
    "deps": [],
    "srcs": ["Sources/**/*.swift"],
    "build_dir": ".once/out/App/App",
    "capability": "build",
}}
_apple_library_impl(ctx)
result = repr(True)
"#
    );
    let store = store_for(workspace.path(), "App");
    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
    result.unwrap();
    assert!(
        store.actions.len() >= 2,
        "expected generator and compiler actions"
    );
    let generator = &store.actions[0];
    assert_eq!(generator.argv[0], "/bin/sh");
    assert_eq!(generator.argv[1], "-c");
    assert_eq!(generator.outputs, [".once/out/App/App/Generated.swift"]);
    assert_eq!(generator.create_dirs, [".once/out/App/App"]);
    assert!(generator.cacheable);
    assert!(!generator.inherit_parent_env);
    assert!(generator
        .toolchain_identity
        .as_deref()
        .is_some_and(|identity| identity.starts_with("once.apple.prebuild.shell.v1\0/bin/sh\0")));
    assert_eq!(
        generator.env.get("SRCROOT"),
        Some(&"/workspace/App".to_string())
    );
    let compiler = store
        .actions
        .iter()
        .find(|action| {
            action
                .identifier
                .as_deref()
                .is_some_and(|identifier| identifier.starts_with("swift_module_compile_App"))
        })
        .expect("swift compiler action");
    assert!(compiler
        .inputs
        .contains(&".once/out/App/App/Generated.swift".to_string()));
    assert!(compiler
        .inputs
        .contains(&"App/Sources/Private.h".to_string()));
    assert!(!compiler.inputs.contains(&"App/Sources".to_string()));
}

#[test]
fn prelude_xcode_shell_phase_models_declared_generated_source() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "PHASE": {
            "isa": "PBXShellScriptBuildPhase",
            "name": "Generate Secrets",
            "shellPath": "/bin/sh",
            "shellScript": "generate-secrets",
            "inputPaths": ["$(SRCROOT)/buildscripts/secrets.gyb"],
            "outputPaths": ["$(DERIVED_FILE_DIR)/SecretKey.swift"],
        },
        "TARGET": {"buildPhases": ["PHASE"]},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

objects = json_decode({objects:?})
phase = _xcode_shell_script_phases(
    {{"label": {{"package": "App", "id": "App/Seed"}}, "attr": {{"project": "App.xcodeproj"}}}},
    objects,
    objects["TARGET"],
    {{"PRODUCT_NAME": "App", "CONFIGURATION": "Debug", "DEPENDENCY_ROOT": "${{SRCROOT}}/Dependencies"}},
    "App",
    "App",
)
action = json_decode(phase["actions"][0])
result = repr([phase["sources"], action["inputs"], action["outputs"], action["cwd"], action["env"]["DERIVED_FILE_DIR"], action["env"]["DEPENDENCY_ROOT"], action["env"]["PROJECT"], action["cacheable"]])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[[".once/out/App/App/SecretKey.swift"], ["App/buildscripts/secrets.gyb"], [".once/out/App/App/SecretKey.swift"], "App", "/workspace/.once/out/App/App", "/workspace/App/Dependencies", "App", True]"#
    );
}

#[test]
fn prelude_xcode_shell_phase_keeps_always_run_action_uncached() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "PHASE": {
            "isa": "PBXShellScriptBuildPhase",
            "name": "Generate Sources",
            "shellScript": "generate",
            "inputPaths": ["$(SRCROOT)/schema.json"],
            "outputPaths": ["$(DERIVED_FILE_DIR)/Generated.swift"],
            "alwaysOutOfDate": "1",
        },
        "TARGET": {"buildPhases": ["PHASE"]},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

objects = json_decode({objects:?})
phase = _xcode_shell_script_phases(
    {{"label": {{"package": "App", "id": "App/Seed"}}, "attr": {{"project": "App.xcodeproj"}}}},
    objects,
    objects["TARGET"],
    {{"PRODUCT_NAME": "App", "CONFIGURATION": "Debug"}},
    "App",
    "App",
)
result = repr(json_decode(phase["actions"][0])["cacheable"])
"#
    );
    assert_eq!(eval_prelude_source_to_repr(source).unwrap(), "False");
}

#[test]
fn prelude_xcode_shell_phase_models_generated_link_input_and_its_preparation() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "DOWNLOAD": {
            "isa": "PBXShellScriptBuildPhase",
            "name": "Download archive",
            "shellPath": "/bin/sh",
            "shellScript": "download-archive",
        },
        "EXTRACT": {
            "isa": "PBXShellScriptBuildPhase",
            "name": "Extract archive",
            "shellPath": "/bin/sh",
            "shellScript": "extract-archive",
            "inputPaths": ["$(USER_LIBRARY_DIR)/Caches/archive.tar.gz"],
            "outputPaths": ["$(PROJECT_TEMP_DIR)/native/libNative.a"],
        },
        "TARGET": {"buildPhases": ["DOWNLOAD", "EXTRACT"]},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

objects = json_decode({objects:?})
phase = _xcode_shell_script_phases(
    {{"label": {{"package": "", "id": "Seed"}}, "attr": {{"project": "App.xcodeproj"}}}},
    objects,
    objects["TARGET"],
    {{"PRODUCT_NAME": "Native", "CONFIGURATION": "Debug", "USER_LIBRARY_DIR": "/Users/test/Library"}},
    "",
    "Native",
)
download = json_decode(phase["actions"][0])
extract = json_decode(phase["actions"][1])
result = repr([
    download["name"],
    extract["name"],
    extract["inputs"],
    extract["outputs"],
    extract["env"]["SCRIPT_INPUT_FILE_0"],
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["Download archive", "Extract archive", [], [".once/out/Native/Intermediates/native/libNative.a"], "/Users/test/Library/Caches/archive.tar.gz"]"#
    );
}

#[test]
fn prelude_xcode_setting_substitutions_use_selected_configuration() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
def host_env(name):
    return "/Users/test" if name == "HOME" else ""

ctx = {{"attr": {{"configuration": "Debug"}}}}
result = repr(_xcode_setting_subs(ctx, "Client", "Client", "/SDK", configuration = "Fennec")["CONFIGURATION"])
"#
    );
    assert_eq!(eval_prelude_source_to_repr(source).unwrap(), r#""Fennec""#);
}

#[test]
fn prelude_xcode_translates_swift_compilation_and_optimization_settings() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr([
    _xcode_swift_compilation_flags({{"SWIFT_OPTIMIZATION_LEVEL": "-Owholemodule"}}),
    _xcode_swift_compilation_flags({{"SWIFT_OPTIMIZATION_LEVEL": "-Osize", "SWIFT_COMPILATION_MODE": "wholemodule"}}),
    _xcode_swift_compilation_flags({{"SWIFT_OPTIMIZATION_LEVEL": "-Onone"}}),
    _xcode_swift_compilation_flags({{"SWIFT_OPTIMIZATION_LEVEL": "-Onone", "SWIFT_ENABLE_BATCH_MODE": "NO"}}),
    _xcode_swift_compilation_flags({{"SWIFT_WHOLE_MODULE_OPTIMIZATION": "YES"}}),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["-O", "-whole-module-optimization"], ["-Osize", "-whole-module-optimization"], ["-Onone", "-j1", "-enable-batch-mode"], ["-Onone", "-j1", "-disable-batch-mode"], ["-whole-module-optimization"]]"#
    );
}

#[test]
fn prelude_xcode_expands_identifier_build_setting_modifiers() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr([
    _xcode_resolve_vars("org.example.$(PRODUCT_NAME:rfc1034identifier)", {{"PRODUCT_NAME": "Example Tests"}}),
    _xcode_resolve_vars("$(TARGET_NAME:c99extidentifier)", {{"TARGET_NAME": "9 Example-Tests"}}),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["org.example.Example-Tests", "_9_Example_Tests"]"#
    );
}

#[test]
fn prelude_xcode_shell_phase_lowers_resource_directory_copy() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "PHASE": {
            "isa": "PBXShellScriptBuildPhase",
            "name": "Copy Fixtures",
            "shellPath": "/bin/sh",
            "shellScript": "cp -R \"${SCRIPT_INPUT_FILE_0}/\" \"${SCRIPT_OUTPUT_FILE_0}/\"",
            "inputPaths": ["$(SRCROOT)/Tests/Fixtures", "$(SRCROOT)/Tests/Fixtures/Manifest.json"],
            "outputPaths": ["$(TARGET_BUILD_DIR)/$(UNLOCALIZED_RESOURCES_FOLDER_PATH)/Fixtures"],
        },
        "TARGET": {"buildPhases": ["PHASE"]},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

objects = json_decode({objects:?})
phase = _xcode_shell_script_phases(
    {{"label": {{"package": "", "id": "Tests"}}, "attr": {{"project": "App.xcodeproj"}}}},
    objects,
    objects["TARGET"],
    {{
        "PRODUCT_NAME": "AppTests",
        "CONFIGURATION": "Debug",
        "UNLOCALIZED_RESOURCES_FOLDER_PATH": "AppTests.xctest",
    }},
    "",
    "AppTests",
)
result = repr([phase["actions"], phase["resource_inputs"], phase["structured_resource_inputs"]])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[[], ["Tests/Fixtures"], ["Tests/Fixtures"]]"#
    );
}

#[test]
fn prelude_xcode_test_attrs_preserve_resources_and_custom_property_list() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

def host_file_exists(path):
    return path == "/workspace/Tests/Info.plist"

def host_file_read(path):
    return "<plist>$(EXECUTABLE_NAME)|$(PRODUCT_BUNDLE_IDENTIFIER)|$(SRCROOT)</plist>"

attrs, product_name = _xcode_test_attrs(
    {{"attr": {{}}}},
    {{"name": "Example Tests"}},
    {{
        "PRODUCT_NAME": "Example Tests",
        "PRODUCT_BUNDLE_IDENTIFIER": "org.example.$(PRODUCT_NAME:rfc1034identifier)",
        "GENERATE_INFOPLIST_FILE": "NO",
        "INFOPLIST_FILE": "Tests/Info.plist",
    }},
    {{"PRODUCT_NAME": "Example Tests", "PROJECT_DIR": ""}},
    "ios",
    {{
        "sources": [],
        "headers": [],
        "resources": ["Tests/Fixtures"],
        "structured_resources": ["Tests/Fixtures"],
        "asset_catalogs": ["Tests/Assets.xcassets"],
        "frameworks": [],
        "source_flags": {{}},
        "project_header_dirs": [],
    }},
)
result = repr([product_name, attrs["bundle_id"], attrs["resources"], attrs["structured_resources"], attrs["asset_catalogs"], attrs["info_plist"], attrs["info_plist_substitutions"]])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["Example Tests", "org.example.Example-Tests", ["Tests/Fixtures"], ["Tests/Fixtures"], ["Tests/Assets.xcassets"], "Tests/Info.plist", {"EXECUTABLE_NAME": "Example Tests", "PRODUCT_BUNDLE_IDENTIFIER": "org.example.Example-Tests", "SRCROOT": "/workspace"}]"#
    );
}

#[test]
fn prelude_xcode_application_attrs_preserve_custom_property_list() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

def host_file_exists(path):
    return path == "/workspace/App/Info.plist"

def host_file_read(path):
    return "<plist>$(EXECUTABLE_NAME)|$(PRODUCT_BUNDLE_IDENTIFIER)|$(SRCROOT)</plist>"

attrs, product_name = _xcode_application_attrs(
    {{"attr": {{}}}},
    {{"name": "Example App"}},
    {{
        "PRODUCT_NAME": "Example App",
        "PRODUCT_BUNDLE_IDENTIFIER": "org.example.$(PRODUCT_NAME:rfc1034identifier)",
        "GENERATE_INFOPLIST_FILE": "NO",
        "INFOPLIST_FILE": "App/Info.plist",
    }},
    {{"PRODUCT_NAME": "Example App", "PROJECT_DIR": ""}},
    "ios",
    {{
        "sources": [],
        "headers": [],
        "resources": [],
        "structured_resources": [],
        "asset_catalogs": [],
        "frameworks": [],
        "source_flags": {{}},
        "project_header_dirs": [],
    }},
)
result = repr([product_name, attrs["bundle_id"], attrs["info_plist"], attrs["info_plist_substitutions"]])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["Example App", "org.example.Example-App", "App/Info.plist", {"EXECUTABLE_NAME": "Example App", "PRODUCT_BUNDLE_IDENTIFIER": "org.example.Example-App", "SRCROOT": "/workspace"}]"#
    );
}

#[test]
fn prelude_xcode_reads_referenced_test_plan_environment_and_skips() {
    let prelude = xcode_prelude_source();
    let workspace = TempDir::new().unwrap();
    let scheme_dir = workspace
        .path()
        .join("App.xcodeproj/xcshareddata/xcschemes");
    std::fs::create_dir_all(&scheme_dir).unwrap();
    std::fs::create_dir_all(workspace.path().join("Test Plans")).unwrap();
    std::fs::write(
        scheme_dir.join("App.xcscheme"),
        r#"<Scheme><TestAction><TestPlans><TestPlanReference reference = "container:Test Plans/App.xctestplan" default = "YES"></TestPlanReference></TestPlans></TestAction></Scheme>"#,
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("Test Plans/App.xctestplan"),
        serde_json::json!({
            "configurations": [{
                "name": "Default",
                "options": {
                    "environmentVariableEntries": [
                        {"key": "CONFIG_VALUE", "value": "configured"}
                    ],
                    "commandLineArgumentEntries": [
                        {"argument": "-ConfiguredMode"},
                        {"argument": "ignored", "enabled": false}
                    ]
                }
            }],
            "defaultOptions": {
                "environmentVariableEntries": [
                    {"key": "TZ", "value": "America/New_York"},
                    {"key": "DISABLED", "value": "ignored", "enabled": false}
                ],
                "commandLineArgumentEntries": [
                    {"argument": "-DefaultMode"}
                ],
                "language": "en",
                "region": "US"
            },
            "testTargets": [{
                "target": {"name": "AppTests"},
                "skippedTests": ["ManualTests", "FeatureTests/testSlow"]
            }]
        })
        .to_string(),
    )
    .unwrap();
    let source = format!(
        r#"{prelude}
ctx = {{"label": {{"package": "", "name": "App", "id": "App"}}, "attr": {{"project": "App.xcodeproj"}}}}
result = repr(_xcode_test_plan_settings(ctx))
"#
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        String::new(),
        ".once/out/Test".to_string(),
    );
    let (_store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
    assert_eq!(
        result.unwrap(),
        r#"{"AppTests": {"test_env": {"TZ": "America/New_York", "CONFIG_VALUE": "configured", "AppleLanguages": "(en)", "AppleLocale": "en_US"}, "test_arguments": ["-DefaultMode", "-ConfiguredMode"], "skipped_tests": ["ManualTests", "FeatureTests/testSlow"]}}"#
    );
}

#[test]
fn prelude_xcode_shell_phase_ignores_non_source_outputs() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "PHASE": {
            "isa": "PBXShellScriptBuildPhase",
            "name": "Embed Frameworks",
            "shellPath": "/bin/sh",
            "shellScript": "copy-frameworks",
            "outputPaths": ["$(TARGET_BUILD_DIR)/Frameworks/Example.framework"],
        },
        "TARGET": {"buildPhases": ["PHASE"]},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

objects = json_decode({objects:?})
phase = _xcode_shell_script_phases(
    {{"label": {{"package": "App", "id": "App/Seed"}}, "attr": {{"project": "App.xcodeproj"}}}},
    objects,
    objects["TARGET"],
    {{"PRODUCT_NAME": "App", "CONFIGURATION": "Debug"}},
    "App",
    "App",
)
result = repr([phase["sources"], phase["actions"]])
"#
    );
    assert_eq!(eval_prelude_source_to_repr(source).unwrap(), r"[[], []]");
}

#[test]
fn prelude_xcode_spm_local_product_uses_path_identity() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
objects = {{"ref": {{"isa": "XCLocalSwiftPackageReference", "relativePath": "MozillaRustComponents"}}}}
result = repr(_xcode_spm_package_refs(objects)["ref"]["identity"])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#""MozillaRustComponents""#
    );
}

#[test]
fn prelude_xcode_reads_only_referenced_local_swift_packages() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

def host_file_exists(path):
    return path == "/workspace/Packages/Shared/Package.swift"

def host_command(argv, env = None, cwd = None, merge_stderr = None):
    return '{{"name":"Shared","targets":[]}}'

refs = {{
    "shared": {{"kind": "local", "path": "../Packages/Shared"}},
    "tool": {{"kind": "local", "path": "../tools/GraphTool"}},
}}
infos = _xcode_local_swift_package_infos({{}}, "Apps", refs)
result = repr([[info["identity"], info["path"]] for info in infos])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["Shared", "Packages/Shared"]]"#
    );
}

#[test]
fn prelude_xcode_reconciles_local_package_products_into_native_targets() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
infos = [{{
    "identity": "FixtureShared",
    "path": "Packages/Shared",
    "info": {{
        "products": [{{"name": "Shared", "targets": ["Shared"]}}],
        "targets": [{{"name": "Shared", "type": "regular", "dependencies": [], "exclude": [], "settings": []}}],
    }},
}}]
standalone = _xcode_local_swift_package_specs({{}}, infos, "macos", "13.0", "simulator")
xcode = _xcode_local_swift_package_specs(
    {{}},
    infos,
    "macos",
    "13.0",
    "simulator",
    target_prefix = "XcodePackage_xcode",
)
result = repr([
    standalone["products"]["FixtureShared\x1fShared"],
    xcode["products"]["FixtureShared\x1fShared"],
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["SwiftPackage_FixtureShared_Shared", "XcodePackage_xcode_FixtureShared_Shared"]"#
    );
}

#[test]
fn prelude_xcode_preserves_every_target_in_a_package_product() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
infos = [{{
    "identity": "FixtureShared",
    "path": "Packages/Shared",
    "info": {{
        "products": [{{"name": "Shared", "targets": ["Core", "Extras"]}}],
        "targets": [
            {{"name": "Core", "type": "regular", "dependencies": [], "exclude": [], "settings": []}},
            {{"name": "Extras", "type": "regular", "dependencies": [], "exclude": [], "settings": []}},
        ],
    }},
}}]
graph = _xcode_local_swift_package_specs({{}}, infos, "macos", "13.0", "simulator")
consumer = {{"dependencies": [{{"product": ["Shared", "FixtureShared", None, None]}}]}}
result = repr([
    graph["products"]["FixtureShared\x1fShared"],
    _xcode_swift_package_dependencies(consumer, "Consumer", {{}}, graph["products"], "macos"),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["SwiftPackage_FixtureShared_Core", "SwiftPackage_FixtureShared_Extras"], ["./SwiftPackage_FixtureShared_Core", "./SwiftPackage_FixtureShared_Extras"]]"#
    );
}

#[test]
fn prelude_xcode_lowers_binary_package_artifacts_to_cached_dependencies() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
infos = [{{
    "identity": "UrlPredictor",
    "path": "Packages/UrlPredictor",
    "info": {{
        "products": [{{"name": "URLPredictorRust", "targets": ["URLPredictorRust"]}}],
        "targets": [{{
            "name": "URLPredictorRust",
            "type": "binary",
            "url": "https://downloads.example.test/URLPredictorRust.zip",
            "checksum": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        }}],
    }},
}}]
ctx = {{
    "label": {{"package": "clients/ios", "id": "clients/ios/xcode"}},
    "attr": {{"binary_artifact_authorization_env": "VENDOR_AUTHORIZATION"}},
}}
graph = _xcode_local_swift_package_specs(ctx, infos, "ios", "17.0", "simulator")
result = repr([
    graph["specs"][0]["kind"],
    graph["specs"][0]["attrs"],
    graph["specs"][1]["kind"],
    graph["specs"][1]["deps"],
    graph["specs"][1]["attrs"]["bundle"],
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["archive_download", {"url": "https://downloads.example.test/URLPredictorRust.zip", "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", "authorization_env": "VENDOR_AUTHORIZATION"}, "apple_xcframework_import", ["./SwiftPackage_UrlPredictor_URLPredictorRust_Artifact"], ".once/out/clients/ios/SwiftPackage_UrlPredictor_URLPredictorRust_Artifact/archive/URLPredictorRust.xcframework"]"#
    );
}

#[test]
fn prelude_archive_download_omits_an_empty_authorization_env() {
    let prelude = archive_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"id": "downloads/Vendor"}},
    "attr": {{
        "url": "https://downloads.example.test/Vendor.zip",
        "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    }},
}}
_archive_download_impl(ctx)
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "downloads/Vendor");
    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(result.unwrap(), r#""ok""#);
    assert_eq!(store.actions.len(), 1);
    assert_eq!(
        store.actions[0].operation,
        Some(DeclaredActionOperation::DownloadAndExtract {
            url: "https://downloads.example.test/Vendor.zip".to_string(),
            sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            destination: ".once/out/downloads/Vendor/archive".to_string(),
            authorization_env: None,
        })
    );
}

#[test]
fn prelude_xcode_adds_package_identity_only_when_package_access_is_available() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
legacy = {{"identity": "Legacy", "info": {{"name": "Legacy Package", "toolsVersion": {{"_version": "5.8.0"}}}}}}
modern = {{"identity": "Modern", "info": {{"name": "Modern Package", "toolsVersion": {{"_version": "5.9.0"}}}}}}
result = repr([
    _xcode_swift_package_name_flags(legacy),
    _xcode_swift_package_name_flags(modern),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[[], ["-package-name", "Modern Package"]]"#
    );
}

#[test]
fn prelude_xcode_lowers_swift_macros_as_transitive_host_tools() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
def glob(patterns):
    pattern = patterns[0]
    if "AppMacros" in pattern:
        return ["Packages/AppMacros/Sources/AppMacros/Plugin.swift"]
    if "SyntaxSupport" in pattern:
        return ["Packages/Syntax/Sources/SyntaxSupport/Support.swift"]
    return []

def host_file_read(path):
    return "import SwiftSyntaxMacros"

infos = [
    {{
        "identity": "AppMacrosPackage",
        "path": "Packages/AppMacros",
        "info": {{
            "name": "AppMacrosPackage",
            "products": [],
            "targets": [{{
                "name": "AppMacros",
                "type": "macro",
                "dependencies": [{{"product": ["SyntaxSupport", "SyntaxPackage", None, None]}}],
                "exclude": [],
                "settings": [],
            }}],
        }},
    }},
    {{
        "identity": "SyntaxPackage",
        "path": "Packages/Syntax",
        "info": {{
            "name": "SyntaxPackage",
            "products": [{{"name": "SyntaxSupport", "targets": ["SyntaxSupport"]}}],
            "targets": [{{
                "name": "SyntaxSupport",
                "type": "regular",
                "dependencies": [],
                "exclude": [],
                "settings": [],
            }}],
        }},
    }},
]
graph = _xcode_local_swift_package_specs({{"label": {{"package": ""}}}}, infos, "ios", "17.0", "simulator")
result = repr([
    graph["specs"][0]["kind"],
    graph["specs"][0]["deps"],
    graph["specs"][1]["attrs"]["platform"],
    graph["specs"][2]["name"],
    graph["specs"][2]["attrs"]["platform"],
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["swift_macro", ["./SwiftPackage_SyntaxPackage_SyntaxSupport_MacroHost"], "ios", "SwiftPackage_SyntaxPackage_SyntaxSupport_MacroHost", "macos"]"#
    );
}

#[test]
fn prelude_xcode_reconciles_cross_package_product_dependencies() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
target_ids = {{"One\x1fOne": "SwiftPackage_One_One"}}
product_ids = {{"Two\x1fTwo": "SwiftPackage_Two_Two"}}
target = {{"dependencies": [{{"product": ["Two", "Two", None, None]}}]}}
result = repr(_xcode_swift_package_dependencies(target, "One", target_ids, product_ids, "macos"))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["./SwiftPackage_Two_Two"]"#
    );
}

#[test]
fn prelude_xcode_reads_core_data_generated_class_metadata() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
swift = '<model sourceLanguage="Swift"><entity name="Page" codeGenerationType="class"><entity name="Tag" codeGenerationType="category">'
objc = '<model sourceLanguage="Objective-C"><entity name="Page" codeGenerationType="class">'
result = repr([
    _xcode_xml_attribute(' name="Page" codeGenerationType="class"', "name"),
    _xcode_xml_attribute(' name="Page" codeGenerationType="class"', "codeGenerationType"),
    _xcode_plist_string('<key>_XCCurrentVersionName</key><string>Model 2.xcdatamodel</string>', "_XCCurrentVersionName"),
    _xcode_datamodel_generated_outputs(swift, "Model", "out"),
    _xcode_datamodel_generated_outputs(objc, "Model", "out"),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["Page", "class", "Model 2.xcdatamodel", ["out/Model+CoreDataModel.swift", "out/Page+CoreDataClass.swift", "out/Page+CoreDataProperties.swift", "out/Tag+CoreDataProperties.swift"], ["out/Model+CoreDataModel.h", "out/Model+CoreDataModel.m", "out/Page+CoreDataClass.h", "out/Page+CoreDataClass.m", "out/Page+CoreDataProperties.h", "out/Page+CoreDataProperties.m"]]"#
    );
    assert!(
        apple_prelude_source().contains("action.get(\"tool\") == \"momc\""),
        "Apple prebuild actions must resolve the Core Data compiler directly"
    );
}

#[test]
fn prelude_xcode_preserves_versioned_core_data_models_from_sources_phase() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
objects = {{
    "PROJECT": {{"mainGroup": "ROOT"}},
    "ROOT": {{"isa": "PBXGroup", "children": ["MODELS"]}},
    "MODELS": {{"isa": "PBXGroup", "path": "WMF/Models", "children": ["MODEL"]}},
    "MODEL": {{
        "isa": "XCVersionGroup",
        "path": "RemoteNotifications.xcdatamodeld",
        "children": ["VERSION"],
        "currentVersion": "VERSION",
    }},
    "VERSION": {{"isa": "PBXFileReference", "path": "RemoteNotifications 3.xcdatamodel"}},
    "SOURCES": {{"isa": "PBXSourcesBuildPhase", "files": ["BUILD"]}},
    "BUILD": {{"isa": "PBXBuildFile", "fileRef": "MODEL"}},
}}
paths = _xcode_group_file_paths(objects, objects["PROJECT"], "")
files = _xcode_classic_phase_files(
    {{}},
    objects,
    {{"buildPhases": ["SOURCES"]}},
    paths["files"],
)
result = repr([paths["files"].get("MODEL"), files["resources"], files["sources"]])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["WMF/Models/RemoteNotifications.xcdatamodeld", ["WMF/Models/RemoteNotifications.xcdatamodeld"], []]"#
    );
}

#[test]
fn prelude_xcode_selects_base_intent_definition_from_variant_group() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
objects = {{
    "SOURCES": {{"isa": "PBXSourcesBuildPhase", "files": ["BUILD"]}},
    "BUILD": {{"isa": "PBXBuildFile", "fileRef": "VARIANT"}},
    "VARIANT": {{"isa": "PBXVariantGroup", "children": ["FR", "BASE"]}},
    "FR": {{"isa": "PBXFileReference", "name": "fr", "path": "fr.lproj/Actions.intentdefinition"}},
    "BASE": {{"isa": "PBXFileReference", "name": "Base", "path": "Base.lproj/Actions.intentdefinition"}},
}}
files = _xcode_classic_phase_files(
    {{}},
    objects,
    {{"buildPhases": ["SOURCES"]}},
    {{"FR": "App/fr.lproj/Actions.intentdefinition", "BASE": "App/Base.lproj/Actions.intentdefinition"}},
)
result = repr(files["intent_definitions"])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["App/Base.lproj/Actions.intentdefinition"]"#
    );
}

#[test]
fn prelude_xcode_preserves_localized_resource_variant_paths() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
objects = {{
    "RESOURCES": {{"isa": "PBXResourcesBuildPhase", "files": ["BUILD"]}},
    "BUILD": {{"isa": "PBXBuildFile", "fileRef": "VARIANT"}},
    "VARIANT": {{"isa": "PBXVariantGroup", "children": ["EN", "BASE"]}},
    "EN": {{"isa": "PBXFileReference", "name": "en", "path": "en.lproj/InfoPlist.strings"}},
    "BASE": {{"isa": "PBXFileReference", "name": "Base", "path": "Base.lproj/Main.storyboard"}},
}}
files = _xcode_classic_phase_files(
    {{}},
    objects,
    {{"buildPhases": ["RESOURCES"]}},
    {{"EN": "Widget/en.lproj/InfoPlist.strings", "BASE": "Widget/Base.lproj/Main.storyboard"}},
)
result = repr(files["resources"])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["Widget/en.lproj/InfoPlist.strings", "Widget/Base.lproj/Main.storyboard"]"#
    );
}

#[test]
fn prelude_xcode_workspace_declares_graph_resolver() {
    let source = format!(
        "{}\nresult = repr(xcode_workspace.get(\"resolver\") != None)\n",
        all_prelude_source()
    );
    assert_eq!(eval_prelude_source_to_repr(source).unwrap(), "True");

    assert_target_kind_attrs(
        "xcode_workspace",
        &[
            "project",
            "configuration",
            "sdk_variant",
            "xcode_developer_dir",
            "resolver_inputs",
        ],
    );

    let schema = built_in_target_kind_schema("xcode_workspace").expect("xcode_workspace schema");
    assert!(schema
        .providers
        .iter()
        .any(|provider| provider == "xcode_workspace"));
    assert!(schema
        .capabilities
        .iter()
        .any(|capability| capability.name == "build"));
}

#[test]
fn prelude_apple_application_exposes_enable_testing() {
    assert_target_kind_attrs("apple_application", &["enable_testing"]);
}

#[test]
fn prelude_apple_application_compiles_asset_catalogs() {
    // Asset catalogs drive two `actool` passes: one generates the Swift symbol
    // accessors added to the compile, the other compiles `Assets.car` into the
    // bundle.
    assert_target_kind_attrs("apple_application", &["asset_catalogs", "app_icon"]);
    let source = include_str!("../prelude/apple.star");
    assert!(
        source.contains("--generate-swift-asset-symbols"),
        "must generate Swift asset symbols"
    );
    assert!(
        source.contains("declare_output(app_dir + \"/Assets.car\")"),
        "must compile Assets.car into the bundle"
    );
}

#[test]
fn prelude_xcode_asset_catalog_dir_recovers_catalog() {
    // A synchronized glob yields files inside a `.xcassets`; the catalog is
    // recovered as the directory up to `.xcassets`, and non-catalog paths yield
    // nothing.
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr([
    _xcode_asset_catalog_dir("App/Assets.xcassets/AppIcon.appiconset/icon.png"),
    _xcode_asset_catalog_dir("App/Assets.xcassets"),
    _xcode_asset_catalog_dir("App/AppIcon.icon/icon.json"),
    _xcode_asset_catalog_dir("App/AppIcon.icon"),
    _xcode_asset_catalog_dir("App/Sources/View.swift"),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["App/Assets.xcassets", "App/Assets.xcassets", "App/AppIcon.icon", "App/AppIcon.icon", ""]"#
    );
}

#[test]
fn prelude_apple_application_exposes_bridging_header() {
    // An Xcode app whose `SWIFT_OBJC_BRIDGING_HEADER` imports a framework
    // (commonly AppKit) makes it visible to every Swift source; the application
    // kind must accept and apply that header.
    assert_target_kind_attrs("apple_application", &["bridging_header"]);
    let source = include_str!("../prelude/apple.star");
    assert!(
        source.contains("\"-import-objc-header\", _package_relative(ctx, bridging_header)"),
        "apple_application must import the bridging header"
    );
    // The main and testable-module compiles both run with a cleared
    // environment, so both must set an explicit Clang module cache or a
    // source-built Objective-C package module cannot be imported.
    assert!(
        source.matches("/ModuleCache").count() >= 2,
        "both app compiles must set an explicit module cache path"
    );
}

#[test]
fn prelude_apple_consumes_framework_search_dirs() {
    // A dependency can contribute a bare framework search directory (Swift
    // autolinks the imported framework, so no framework name is needed). The
    // collector must surface it as a `-F` search directory.
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
deps = [{{"transitive_framework_search_dirs": ["out/spm/frameworks"]}}]
result = repr(_collect_dep_compile_inputs(deps, "build")[5])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["out/spm/frameworks"]"#
    );
}

#[test]
fn prelude_apple_resolves_clang_profile_runtime_archive() {
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
def host_which(name):
    return "/usr/bin/" + name

def host_path_exists(path):
    return path.endswith(".a")

def host_command(argv, env = None, merge_stderr = None):
    if "--find" in argv:
        return "/toolchain/usr/bin/clang\n"
    if "-print-resource-dir" in argv:
        return "/toolchain/usr/lib/clang/21\n"
    fail("unexpected host command: " + str(argv))

result = repr([
    _apple_clang_profile_runtime("ios", "simulator", ""),
    _apple_clang_profile_runtime("ios", "device", ""),
    _apple_clang_profile_runtime("macos", "simulator", ""),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["/toolchain/usr/lib/clang/21/lib/darwin/libclang_rt.profile_iossim.a", "/toolchain/usr/lib/clang/21/lib/darwin/libclang_rt.profile_ios.a", "/toolchain/usr/lib/clang/21/lib/darwin/libclang_rt.profile_osx.a"]"#
    );
}

#[test]
fn prelude_apple_xcframework_import_keeps_dependency_link_data_compile_only() {
    // An inferred prebuilt-module dependency edge must make the dependency's
    // modules loadable by consumers (search dirs, module files, autolink
    // suppression through the propagated framework bundles) without turning
    // the dependency into a link input. Archives, SDK libraries, and linker
    // options stay own-only; the generated project's build phases remain the
    // only source of link edges.
    let prelude = apple_prelude_source();
    let available_libraries = serde_json::json!({
        "AvailableLibraries": [{
            "LibraryIdentifier": "ios-arm64-simulator",
            "LibraryPath": "Primary.framework",
            "BinaryPath": "Primary.framework/Primary",
            "SupportedArchitectures": ["arm64"],
            "SupportedPlatform": "ios",
            "SupportedPlatformVariant": "simulator",
        }],
    })
    .to_string();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

def host_arch():
    return "arm64"

def host_file_exists(path):
    return path == "/workspace/Primary.xcframework/Info.plist"

def host_which(name):
    return name

def host_command(argv, env = None, merge_stderr = None):
    if argv[0] == "plutil":
        return {available_libraries:?}
    if argv[0] == "find":
        return "/workspace/Primary.xcframework/ios-arm64-simulator/Primary.framework/Primary\n/workspace/Primary.xcframework/ios-arm64-simulator/Primary.framework/Modules/Primary.swiftmodule/arm64-apple-ios-simulator.swiftmodule\n"
    if argv[0] == "file":
        return "current ar archive\n"
    fail("unexpected host_command: " + str(argv))

support = {{
    "path": "Cache/Support.xcframework/ios-arm64-simulator/Support.framework",
    "module_name": "Support",
    "files": [
        "Cache/Support.xcframework/ios-arm64-simulator/Support.framework/Support",
        "Cache/Support.xcframework/ios-arm64-simulator/Support.framework/Modules/Support.swiftmodule/arm64-apple-ios-simulator.swiftmodule",
    ],
    "label_id": "Support",
    "linkage": "static",
}}
ctx = {{
    "label": {{"package": "", "name": "Primary", "id": "Primary"}},
    "attr": {{
        "bundle": "Primary.xcframework",
        "platform": "ios",
        "sdk_variant": "simulator",
    }},
    "configuration": {{"tokens": []}},
    "deps": [{{
        "label_id": "Support",
        "transitive_swiftmodule_dirs": [".once/out/Support"],
        "transitive_archives": ["Cache/Support.xcframework/ios-arm64-simulator/Support.framework/Support"],
        "transitive_link_framework_bundles": [support],
        "transitive_framework_bundles": [],
        "transitive_sdk_frameworks": ["UIKit"],
        "transitive_linkopts": ["-ObjC"],
    }}],
    "srcs": [],
    "build_dir": ".once/out/Primary",
    "capability": "build",
}}
provider = _apple_xcframework_import_impl(ctx)
result = repr([
    provider["transitive_archives"],
    [bundle["module_name"] for bundle in provider["transitive_link_framework_bundles"]],
    provider["transitive_framework_bundles"],
    provider["transitive_sdk_frameworks"],
    provider["transitive_linkopts"],
    provider["transitive_swiftmodule_dirs"],
    provider["transitive_framework_search_dirs"],
    provider["transitive_framework_files"],
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["Primary.xcframework/ios-arm64-simulator/Primary.framework/Primary"], ["Primary", "Support"], [], [], [], [".once/out/Support"], [], []]"#
    );
}

#[test]
fn prelude_xcode_registers_absolute_xcframework_refs_via_workspace_alias() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "EXTERNAL": {"isa": "PBXFileReference", "lastKnownFileType": "wrapper.xcframework", "name": "External.xcframework", "path": "/ext/cache/hash/External.xcframework", "sourceTree": "<absolute>"},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

created = {{}}

def host_path_exists(path):
    return path in created

def host_file_exists(path):
    return path == "/ext/cache/hash/External.xcframework/Info.plist"

def host_which(name):
    return name

def host_command(argv, env = None, cwd = None, merge_stderr = None):
    if argv[0] == "sh":
        created[argv[6]] = True
        return ""
    fail("unexpected host command: " + str(argv))

objects = json_decode({objects:?})
specs = []
names = {{}}
_xcode_register_absolute_xcframework_refs(objects, specs, names, "ios", "simulator")
result = repr([
    [spec["attrs"]["bundle"] for spec in specs],
    names.get("/ext/cache/hash/External.xcframework"),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[[".once/xcframework-refs/_ext_cache_hash/External.xcframework"], "XCFramework_.once_xcframework-refs__ext_cache_hash_External.xcframework"]"#
    );
}

#[test]
fn prelude_xcode_collects_absolute_xcframework_file_references() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "EXTERNAL": {"isa": "PBXFileReference", "lastKnownFileType": "wrapper.xcframework", "name": "External.xcframework", "path": "/ext/cache/hash/External.xcframework", "sourceTree": "<absolute>"},
        "INTERNAL": {"isa": "PBXFileReference", "lastKnownFileType": "wrapper.xcframework", "name": "Internal.xcframework", "path": "/workspace/Vendor/Internal.xcframework", "sourceTree": "<absolute>"},
        "RELATIVE": {"isa": "PBXFileReference", "lastKnownFileType": "wrapper.xcframework", "path": "Vendor/Relative.xcframework", "sourceTree": "<group>"},
        "OTHER": {"isa": "PBXFileReference", "path": "/ext/cache/libz.tbd", "sourceTree": "<absolute>"},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

objects = json_decode({objects:?})
result = repr(_xcode_absolute_xcframework_refs(objects))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["/ext/cache/hash/External.xcframework"]"#
    );
}

#[allow(clippy::too_many_lines)]
#[test]
fn prelude_apple_xcframework_import_exposes_static_clang_module_to_swift() {
    let prelude = apple_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package = workspace.path().join("App/Sources");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("App.swift"),
        "import URLPredictorRust\nfunc predictor() {}\n",
    )
    .unwrap();

    let root = workspace.path().display().to_string();
    let slice = "Vendor.xcframework/macos-arm64";
    let archive = format!("{slice}/liburl_predictor.a");
    let headers = format!("{slice}/Headers");
    let modulemap = format!("{headers}/URLPredictorRust/module.modulemap");
    let header = format!("{headers}/URLPredictorRust/ddg_url_predictor.h");
    let available_libraries = serde_json::json!({
        "AvailableLibraries": [{
            "LibraryIdentifier": "macos-arm64",
            "LibraryPath": "liburl_predictor.a",
            "BinaryPath": "liburl_predictor.a",
            "HeadersPath": "Headers",
            "SupportedArchitectures": ["arm64"],
            "SupportedPlatform": "macos",
        }],
    })
    .to_string();
    let slice_files = [
        format!("{root}/{archive}"),
        format!("{root}/{modulemap}"),
        format!("{root}/{header}"),
    ]
    .join("\n");
    let source = format!(
        r#"{prelude}
def workspace_root():
    return {root:?}

def host_arch():
    return "arm64"

def host_which(name):
    return name

def host_file_exists(path):
    return path == workspace_root() + "/Vendor.xcframework/Info.plist"

def host_file_read(path):
    if path == workspace_root() + "/{modulemap}":
        return "module URLPredictorRust {{\n  header \\\"ddg_url_predictor.h\\\"\n  export *\n}}\n"
    fail("unexpected host_file_read: " + path)

def host_command(argv, env = None, merge_stderr = None):
    if argv[0] == "plutil":
        return {available_libraries:?}
    if argv[0] == "find":
        return {slice_files:?}
    if "--version" in argv:
        return "Swift version 6.0\n"
    fail("unexpected host_command: " + str(argv))

import_ctx = {{
    "label": {{"package": "", "name": "Vendor", "id": "Vendor"}},
    "attr": {{
        "bundle": "Vendor.xcframework",
        "platform": "macos",
        "sdk_variant": "simulator",
        "arch": "arm64",
    }},
    "configuration": {{"tokens": []}},
    "deps": [],
    "srcs": [],
    "build_dir": ".once/out/Vendor",
    "capability": "build",
}}
dependency = _apple_xcframework_import_impl(import_ctx)
ctx = {{
    "label": {{"package": "App", "name": "App", "id": "App/App"}},
    "attr": {{
        "platform": "macos",
        "module_name": "App",
        "xcode_developer_dir": "/opt/Xcode/Developer",
    }},
    "configuration": {{"tokens": []}},
    "deps": [dependency],
    "srcs": ["Sources/**/*.swift"],
    "build_dir": ".once/out/App/App",
    "capability": "build",
}}
result = repr(_apple_library_impl(ctx))
"#,
    );
    let store = store_for(workspace.path(), "App");
    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));

    let provider = result.unwrap();
    assert!(provider.contains(&archive), "{provider}");
    let compiler = action_by_identifier(&store, "swift_module_compile_App");
    assert!(
        compiler
            .argv
            .windows(2)
            .any(|args| { args == ["-Xcc".to_string(), format!("-fmodule-map-file={modulemap}")] }),
        "Swift must receive the static XCFramework module map: {:?}",
        compiler.argv
    );
    assert!(
        compiler
            .argv
            .windows(2)
            .any(|args| { args == ["-Xcc".to_string(), "-I".to_string()] })
            && compiler.argv.contains(&headers),
        "Swift must receive the static XCFramework headers: {:?}",
        compiler.argv
    );
    for input in [&archive, &modulemap, &header] {
        assert!(
            compiler.inputs.contains(input),
            "static XCFramework input `{input}` is missing from {:?}",
            compiler.inputs
        );
    }
}

#[test]
fn prelude_apple_makes_runtime_frameworks_available_to_the_linker() {
    let prelude = apple_prelude_source();
    let source = format!(
        r#"{prelude}
deps = [{{
    "transitive_link_framework_bundles": [{{
        "path": "out/Direct.framework",
        "module_name": "Direct",
        "files": ["out/Direct.framework/Direct"],
    }}],
    "transitive_framework_bundles": [{{
        "path": "out/Direct.framework",
        "module_name": "Direct",
        "files": ["out/Direct.framework/Direct"],
    }}, {{
        "path": "vendor/Runtime.framework",
        "module_name": "Runtime",
        "files": ["vendor/Runtime.framework/Runtime"],
    }}],
}}]
inputs = _collect_dep_compile_inputs(deps, "build")
result = repr([inputs[5], inputs[6], inputs[7]])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["out", "vendor"], ["Direct"], ["out/Direct.framework/Direct", "vendor/Runtime.framework/Runtime"]]"#
    );
}

#[test]
fn prelude_xcode_product_kind_maps_apple_product_types() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr([
    _xcode_product_kind("com.apple.product-type.application"),
    _xcode_product_kind("com.apple.product-type.framework"),
    _xcode_product_kind("com.apple.product-type.framework.static"),
    _xcode_product_kind("com.apple.product-type.library.static"),
    _xcode_product_kind("com.apple.product-type.bundle.unit-test"),
    _xcode_product_kind("com.apple.product-type.bundle.ui-testing"),
    _xcode_product_kind("com.apple.product-type.app-extension"),
    _xcode_product_kind("com.apple.product-type.app-extension.messages-sticker-pack"),
    _xcode_product_kind("com.apple.product-type.watchkit2-extension"),
    _xcode_product_kind("com.apple.product-type.application.watchapp2"),
    _xcode_product_kind("com.apple.product-type.xpc-service"),
    _xcode_product_kind("com.apple.product-type.library.dynamic"),
    _xcode_product_kind("com.apple.product-type.application.on-demand-install-capable"),
    _xcode_product_kind("com.apple.product-type.application.messages"),
    _xcode_product_kind("com.apple.product-type.application.watchapp2-container"),
    _xcode_product_kind("com.apple.product-type.driver-extension"),
    _xcode_product_kind("com.apple.product-type.pluginkit-plugin"),
    _xcode_product_kind("com.apple.product-type.app-extension.intents-service"),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["application", "framework", "framework", "library", "test", "test", "extension", "extension", "extension", "watch_app", "extension", "library", "application", "application", "application", "extension", "extension", "extension"]"#
    );
}

#[test]
fn prelude_xcode_platform_maps_sdkroots() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr([
    _xcode_platform("iphoneos"),
    _xcode_platform("iphonesimulator"),
    _xcode_platform("macosx"),
    _xcode_platform("appletvos"),
    _xcode_platform("watchsimulator"),
    _xcode_platform("xros"),
    _xcode_platform(""),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["ios", "ios", "macos", "tvos", "watchos", "visionos", "ios"]"#
    );
}

#[test]
fn prelude_xcode_families_decodes_targeted_device_family() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr([
    _xcode_families({{"TARGETED_DEVICE_FAMILY": "1,2"}}),
    _xcode_families({{"TARGETED_DEVICE_FAMILY": "2"}}),
    _xcode_families({{}}),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["iphone", "ipad"], ["ipad"], []]"#
    );
}

#[test]
fn prelude_xcode_keeps_swift_and_clang_defines_separate() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
settings = {{
    "SWIFT_ACTIVE_COMPILATION_CONDITIONS": "DEBUG FEATURE_X",
    "GCC_PREPROCESSOR_DEFINITIONS": ["DEBUG=1", "$(inherited)", "MY_FLAG=2", "APP_GROUP=$(APP_GROUP)"],
}}
result = repr([
    _xcode_swift_defines(settings, {{"APP_GROUP": "group.dev.once.App"}}),
    _xcode_clang_defines(settings, {{"APP_GROUP": "group.dev.once.App"}}),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["DEBUG", "FEATURE_X"], ["DEBUG=1", "MY_FLAG=2", "APP_GROUP=group.dev.once.App"]]"#
    );
}

#[test]
fn prelude_xcode_translates_swift_feature_settings_generically() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr(_xcode_swift_feature_flags({{
    "SWIFT_UPCOMING_FEATURE_INTERNAL_IMPORTS_BY_DEFAULT": "YES",
    "SWIFT_UPCOMING_FEATURE_IMPORT_OBJC_FORWARD_DECLS": "YES",
    "SWIFT_UPCOMING_FEATURE_DISABLE_OUTWARD_ACTOR_ISOLATION": "YES",
    "SWIFT_UPCOMING_FEATURE_MEMBER_IMPORT_VISIBILITY": "MIGRATE",
    "SWIFT_EXPERIMENTAL_FEATURE_DEBUG_DESCRIPTION_MACRO": "YES",
    "SWIFT_UPCOMING_FEATURE_EXISTENTIAL_ANY": "NO",
}}))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["-enable-experimental-feature", "DebugDescriptionMacro", "-enable-upcoming-feature", "DisableOutwardActorInference", "-enable-upcoming-feature", "ImportObjcForwardDeclarations", "-enable-upcoming-feature", "InternalImportsByDefault", "-enable-upcoming-feature", "MemberImportVisibility:migrate"]"#
    );
}

#[test]
fn prelude_xcode_maps_prefix_headers_for_every_product_kind() {
    let prelude = xcode_prelude_source();
    let workspace = TempDir::new().unwrap();
    let support = workspace.path().join("Pods/Support");
    std::fs::create_dir_all(&support).unwrap();
    std::fs::write(support.join("Library-Prefix.pch"), "").unwrap();
    let source = format!(
        r#"{prelude}
ctx = {{"attr": {{"sdk_variant": "simulator"}}}}
files = {{
    "source_flags": {{}},
    "project_header_dirs": [],
    "sources": [],
    "headers": [],
    "exported_headers": [],
    "frameworks": [],
}}
attrs = _xcode_common_attrs(
    ctx,
    {{"name": "Library"}},
    {{"GCC_PREFIX_HEADER": "Support/Library-Prefix.pch"}},
    {{"PROJECT_DIR": "Pods"}},
    "ios",
    files,
)
result = repr(attrs["prefix_header"])
"#
    );
    let store = store_for(workspace.path(), "");
    let (_store, output) = with_active_store(store, || eval_prelude_source_to_repr(source));
    assert_eq!(output.unwrap(), r#""Pods/Support/Library-Prefix.pch""#);
}

#[test]
fn prelude_xcode_keeps_toolchain_header_paths_out_of_workspace_inputs() {
    let prelude = xcode_prelude_source();
    let workspace = TempDir::new().unwrap();
    let workspace_headers = workspace.path().join("Sources");
    let toolchain = TempDir::new().unwrap();
    let toolchain_headers = toolchain.path().join("usr/include");
    std::fs::create_dir_all(&workspace_headers).unwrap();
    std::fs::create_dir_all(&toolchain_headers).unwrap();
    std::fs::write(
        workspace_headers.join("module.modulemap"),
        "module WorkspaceHeaders { export * }\n",
    )
    .unwrap();
    std::fs::write(
        toolchain_headers.join("module.modulemap"),
        "module ToolchainHeaders { export * }\n",
    )
    .unwrap();
    let source = format!(
        r#"{prelude}
ctx = {{"attr": {{"sdk_variant": "simulator"}}}}
files = {{
    "source_flags": {{}},
    "project_header_dirs": [],
    "sources": [],
    "headers": [],
    "exported_headers": [],
    "frameworks": [],
}}
settings = {{"HEADER_SEARCH_PATHS": [{workspace_headers:?}, {toolchain_headers:?}]}}
attrs = _xcode_common_attrs(ctx, {{"name": "Library"}}, settings, {{}}, "ios", files)
result = repr([_xcode_auxiliary_modulemaps(settings, {{}}, ""), attrs["private_header_dirs"], attrs["clang_flags"]])
"#,
        workspace_headers = workspace_headers.display().to_string(),
        toolchain_headers = toolchain_headers.display().to_string(),
    );
    let store = store_for(workspace.path(), "");
    let (_store, output) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let output = output.unwrap();
    assert!(
        output.contains(r#"["Sources/module.modulemap"]"#),
        "{output}"
    );
    assert!(output.contains(r#"["Sources"]"#), "{output}");
    assert!(
        output.contains(&toolchain_headers.display().to_string()),
        "the compiler flag must preserve the toolchain search path: {output}"
    );
    assert!(
        !output.contains("ToolchainHeaders"),
        "the external module map must not become a workspace input: {output}"
    );
}

#[test]
fn prelude_xcode_normalizes_workspace_header_search_paths() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{"attr": {{"sdk_variant": "simulator"}}}}
files = {{
    "source_flags": {{}},
    "project_header_dirs": [],
    "sources": [],
    "headers": [],
    "exported_headers": [],
    "frameworks": [],
}}
settings = {{"HEADER_SEARCH_PATHS": ["$(SRCROOT)/../../Common/SharedHeaders/include"]}}
subs = {{"SRCROOT": "Modules/App/ReproApp", "PROJECT_DIR": "Modules/App/ReproApp"}}
attrs = _xcode_common_attrs(ctx, {{"name": "App"}}, settings, subs, "ios", files)
result = repr([_xcode_header_search_dirs(settings, subs), attrs["private_header_dirs"], attrs["clang_flags"]])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["Modules/Common/SharedHeaders/include"], ["Modules/Common/SharedHeaders/include"], ["-I", "Modules/Common/SharedHeaders/include"]]"#
    );
}

#[test]
fn prelude_xcode_lowers_native_search_and_link_flags() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
settings = {{
    "OTHER_CFLAGS": ["$(inherited)", "-DSQLITE_HAS_CODEC"],
    "HEADER_SEARCH_PATHS": ["$(SRCROOT)/Pods/SQLCipher", "$(inherited)"],
    "OTHER_LDFLAGS": ["$(inherited)", "-ObjC", "-fprofile-instr-generate", "-no_application_extension"],
}}
subs = {{"SRCROOT": ""}}
result = repr([_xcode_clang_flags(settings, subs), _xcode_linkopts(settings, subs)])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["-DSQLITE_HAS_CODEC", "-I", "/Pods/SQLCipher"], ["-Xlinker", "-ObjC", "-profile-generate", "-Xlinker", "-no_application_extension"]]"#
    );
}

#[test]
fn prelude_xcode_drops_linker_option_groups_with_unresolved_arguments() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
settings = {{
    "OTHER_LDFLAGS": "$(inherited) -weak_library \"$(BUILT_PRODUCTS_DIR)/Optional.framework/Optional\" -ObjC",
}}
result = repr(_xcode_linkopts(settings, {{}}))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["-Xlinker", "-ObjC"]"#
    );
}

#[test]
fn prelude_xcode_preserves_pre_grouped_linker_forwarding() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
settings = {{
    "OTHER_LDFLAGS": ["-ObjC", "-Xlinker", "-no_application_extension", "-dead_strip"],
}}
result = repr(_xcode_linkopts(settings, {{}}))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["-Xlinker", "-ObjC", "-Xlinker", "-no_application_extension", "-Xlinker", "-dead_strip"]"#
    );
}

#[test]
fn prelude_xcode_merge_settings_resolves_inherited_lists() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
lower = {{"OTHER_SWIFT_FLAGS": "-DA -DB", "PRODUCT_NAME": "Base"}}
higher = {{"OTHER_SWIFT_FLAGS": "$(inherited) -DC", "PRODUCT_NAME": "Override"}}
merged = _xcode_merge_settings(lower, higher)
result = repr([merged["OTHER_SWIFT_FLAGS"], merged["PRODUCT_NAME"]])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["-DA", "-DB", "-DC"], "Override"]"#
    );
}

#[test]
fn prelude_xcode_resolve_vars_expands_known_variables() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
subs = {{"TARGET_NAME": "App", "PRODUCT_NAME": "App", "PODS_ROOT": "Pods", "PODS_TARGET_SRCROOT": "${{PODS_ROOT}}/Library"}}
result = repr([
    _xcode_resolve_vars("$(TARGET_NAME)Tests", subs),
    _xcode_resolve_vars("dev.once.$(PRODUCT_NAME)", subs),
    _xcode_resolve_vars("${{TARGET_NAME}}/${{PRODUCT_NAME}}", subs),
    _xcode_resolve_vars("$(PODS_TARGET_SRCROOT)/Sources", subs),
    _xcode_resolve_vars("$(UNKNOWN_VAR)", subs),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["AppTests", "dev.once.App", "App/App", "Pods/Library/Sources", "$(UNKNOWN_VAR)"]"#
    );
}

#[test]
fn prelude_xcode_setting_subs_include_resolved_configuration_values() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
def host_env(name):
    return "/Users/test" if name == "HOME" else ""

ctx = {{"attr": {{"configuration": "Debug"}}}}
subs = _xcode_setting_subs(
    ctx,
    "App",
    "App",
    "iphonesimulator",
    {{"DEPENDENCY_ROOT": "Dependencies", "PRODUCT_NAME": "Ignored"}},
)
result = repr([
    _xcode_resolve_vars("${{DEPENDENCY_ROOT}}/Framework.framework", subs),
    subs["PRODUCT_NAME"],
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["Dependencies/Framework.framework", "App"]"#
    );
}

#[test]
fn prelude_xcode_root_project_source_root_stays_workspace_relative() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

def host_file_exists(path):
    return path == "/workspace/App/App-Info.plist"

def host_file_read(path):
    if path == "/workspace/App/App-Info.plist":
        return "<plist><dict></dict></plist>"
    fail("unexpected host_file_read: " + path)

ctx = {{"label": {{"package": ""}}, "attr": {{"configuration": "Debug"}}}}
subs = _xcode_setting_subs(ctx, "App", "App", "/SDK", project_dir = "")
attrs = {{}}
_xcode_add_info_plist_attrs(
    attrs,
    {{"INFOPLIST_FILE": "$(SRCROOT)/App/App-Info.plist"}},
    subs,
    "App",
    "dev.example.App",
)
result = repr([subs["SRCROOT"], attrs["info_plist"]])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[".", "App/App-Info.plist"]"#
    );
}

#[test]
fn prelude_xcode_read_xcconfig_flattens_includes() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r##"{prelude}
def workspace_root():
    return ""

def host_file_exists(path):
    return path in ["Base.xcconfig", "Shared.xcconfig", "User.xcconfig"]

def host_file_read(path):
    if path == "Base.xcconfig":
        return "#include \"Shared.xcconfig\"\nPRODUCT_NAME = FromBase\nOTHER_SWIFT_FLAGS = $(inherited) -warnings-as-errors\n#include? \"User.xcconfig\"\n"
    if path == "Shared.xcconfig":
        return "PRODUCT_NAME = FromShared // overridden\nSWIFT_VERSION = 5.9\nOTHER_SWIFT_FLAGS = -D SHARED\n"
    if path == "User.xcconfig":
        return "OTHER_SWIFT_FLAGS = $(inherited) -no-warnings-as-errors\n"
    return ""

flat = _xcode_read_xcconfig({{}}, "Base.xcconfig")
result = repr([flat.get("PRODUCT_NAME"), flat.get("SWIFT_VERSION"), _xcode_setting_to_list(flat.get("OTHER_SWIFT_FLAGS"))])
"##
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["FromBase", "5.9", ["-D", "SHARED", "-warnings-as-errors", "-no-warnings-as-errors"]]"#
    );
}

#[test]
fn prelude_xcode_test_host_ref_resolves_host_application() {
    let prelude = xcode_prelude_source();
    // A standard TEST_HOST references the host `.app` through build-setting
    // variables. The resolver must map the `/App.app` fragment back to the
    // host application target name.
    let source = format!(
        r#"{prelude}
settings = {{
    "TEST_HOST": "$(BUILT_PRODUCTS_DIR)/App.app/$(BUNDLE_EXECUTABLE_FOLDER_PATH)/App",
}}
name_map = {{"App": "App", "Feature": "Feature"}}
result = repr(_xcode_test_host_ref({{}}, settings, name_map))
"#
    );
    assert_eq!(eval_prelude_source_to_repr(source).unwrap(), r#""App""#);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the inline Starlark fixture keeps this interoperability contract in one test"
)]
fn prelude_apple_library_exposes_pure_swift_to_objective_c_consumers() {
    let prelude = all_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("ios/SwiftModel/Sources");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        package_dir.join("Model.swift"),
        "public final class Model: NSObject {}\n",
    )
    .unwrap();
    std::fs::write(package_dir.join("Consumer.m"), "@import SwiftModel;\n").unwrap();
    std::fs::write(package_dir.join("Consumer.h"), "void consume(void);\n").unwrap();
    std::fs::write(
        package_dir.join("Mixed-Bridging.h"),
        "#include \"Consumer.h\"\n",
    )
    .unwrap();
    let source = format!(
        r#"{prelude}
def host_which(name):
    fail("host_which must not be called in direct mode")

def host_command(argv, env = None, merge_stderr = None):
    if "--version" in argv:
        return "Swift version 6.0\n"
    fail("unexpected host_command: " + str(argv))

ctx = {{
    "label": {{"package": "ios/SwiftModel", "name": "SwiftModel", "id": "ios/SwiftModel/SwiftModel"}},
    "attr": {{"platform": "ios", "sdk_variant": "simulator", "xcode_developer_dir": "/opt/Xcode/Developer"}},
    "deps": [],
    "srcs": ["Sources/**/*.swift"],
    "build_dir": ".once/out/ios/SwiftModel/SwiftModel",
    "capability": "build",
}}
swift_model = _apple_library_impl(ctx)
consumer_ctx = {{
    "label": {{"package": "ios/SwiftModel", "name": "ObjectiveCConsumer", "id": "ios/SwiftModel/ObjectiveCConsumer"}},
    "attr": {{"platform": "ios", "sdk_variant": "simulator", "xcode_developer_dir": "/opt/Xcode/Developer", "enable_modules": True}},
    "deps": [swift_model],
    "srcs": ["Sources/**/*.m"],
    "build_dir": ".once/out/ios/SwiftModel/ObjectiveCConsumer",
    "capability": "build",
}}
_apple_library_impl(consumer_ctx)
mixed_ctx = {{
    "label": {{"package": "ios/SwiftModel", "name": "MixedModel", "id": "ios/SwiftModel/MixedModel"}},
    "attr": {{"platform": "ios", "sdk_variant": "simulator", "xcode_developer_dir": "/opt/Xcode/Developer", "enable_modules": True, "bridging_header": "Sources/Mixed-Bridging.h", "exported_headers": ["Sources/Consumer.h"]}},
    "deps": [],
    "srcs": ["Sources/**/*.swift", "Sources/**/*.m"],
    "build_dir": ".once/out/ios/SwiftModel/MixedModel",
    "capability": "build",
}}
_apple_library_impl(mixed_ctx)
result = repr(True)
"#
    );
    let store = store_for(workspace.path(), "ios/SwiftModel");
    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
    result.unwrap();

    let modulemap = ".once/out/ios/SwiftModel/Headers/SwiftModel/module.modulemap";
    let map_action = store
        .actions
        .iter()
        .find(|action| action.outputs.contains(&modulemap.to_string()))
        .expect("pure Swift interop modulemap action");
    let Some(DeclaredActionOperation::WriteFile { bytes, .. }) = &map_action.operation else {
        panic!("pure Swift interop map must be a write action");
    };
    assert!(std::str::from_utf8(bytes)
        .expect("modulemap contents are UTF-8")
        .contains("SwiftModel-Swift.h"),);
    let compiler = action_by_identifier(&store, "swift_module_compile_SwiftModel");
    assert!(compiler
        .outputs
        .contains(&".once/out/ios/SwiftModel/Headers/SwiftModel/SwiftModel-Swift.h".to_string()));
    assert!(
        !compiler
            .argv
            .contains(&"-import-underlying-module".to_string()),
        "a pure Swift target must not import its generated Objective-C module while compiling"
    );
    assert!(
        compiler.argv.contains(&"-parse-as-library".to_string()),
        "a Swift-only library retains library parsing semantics"
    );
    let consumer_compiler = store
        .actions
        .iter()
        .find(|action| {
            action
                .argv
                .last()
                .is_some_and(|arg| arg.ends_with("Consumer.m"))
        })
        .expect("Objective-C consumer compile action");
    assert!(
        consumer_compiler
            .argv
            .contains(&format!("-fmodule-map-file={modulemap}")),
        "Objective-C consumers must receive a pure Swift dependency's modulemap"
    );
    assert!(
        consumer_compiler.inputs.contains(&modulemap.to_string()),
        "the dependency modulemap must participate in the consumer action digest"
    );
    let mixed_compiler = action_by_identifier(&store, "swift_module_compile_MixedModel");
    assert!(
        mixed_compiler
            .argv
            .contains(&"-parse-as-library".to_string()),
        "mixed library object compilation must retain library parsing semantics"
    );
    assert!(
        !mixed_compiler
            .argv
            .contains(&"-import-underlying-module".to_string()),
        "a bridging header and an underlying module cannot be imported together"
    );
    assert!(
        !mixed_compiler
            .argv
            .iter()
            .any(|arg| arg.starts_with("-fmodule-map-file=")),
        "the target's own module map must not hide declarations imported by its bridging header"
    );
    assert!(
        mixed_compiler.argv.windows(4).any(|args| {
            args == [
                "-Xfrontend",
                "-emit-clang-header-min-access",
                "-Xfrontend",
                "internal",
            ]
        }),
        "mixed targets must expose internal Objective-C declarations in their generated header"
    );
}

#[test]
fn prelude_xcode_group_file_paths_walk_the_group_tree() {
    let prelude = xcode_prelude_source();
    // Groups nest a leaf path relative to their parent, a `SOURCE_ROOT` group
    // re-roots at the project directory, and `<absolute>` references pass
    // through. The project itself lives in the `app` subdirectory, so
    // `<group>` paths are prefixed with it.
    let objects = serde_json::json!({
        "ROOT": {"isa": "PBXProject", "mainGroup": "MAIN"},
        "MAIN": {
            "isa": "PBXGroup",
            "sourceTree": "<group>",
            "children": ["SRC", "GEN", "ABS"],
        },
        "SRC": {
            "isa": "PBXGroup",
            "path": "Sources",
            "sourceTree": "<group>",
            "children": ["NESTED", "TOP_FILE"],
        },
        "NESTED": {
            "isa": "PBXGroup",
            "path": "Core",
            "sourceTree": "<group>",
            "children": ["NESTED_FILE"],
        },
        "NESTED_FILE": {"isa": "PBXFileReference", "path": "Client.swift", "sourceTree": "<group>"},
        "TOP_FILE": {"isa": "PBXFileReference", "path": "Root.swift", "sourceTree": "<group>"},
        "GEN": {
            "isa": "PBXGroup",
            "sourceTree": "SOURCE_ROOT",
            "children": ["GEN_FILE"],
        },
        "GEN_FILE": {"isa": "PBXFileReference", "path": "Generated.swift", "sourceTree": "<group>"},
        "ABS": {
            "isa": "PBXGroup",
            "sourceTree": "<group>",
            "children": ["ABS_FILE"],
        },
        "ABS_FILE": {"isa": "PBXFileReference", "path": "/opt/vendor/Vendor.swift", "sourceTree": "<absolute>"},
    })
    .to_string();

    let source = format!(
        r#"{prelude}
objects = json_decode({objects:?})
paths = _xcode_group_file_paths(objects, objects["ROOT"], "app")["files"]
result = repr([
    paths.get("NESTED_FILE"),
    paths.get("TOP_FILE"),
    paths.get("GEN_FILE"),
    paths.get("ABS_FILE"),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["app/Sources/Core/Client.swift", "app/Sources/Root.swift", "app/Generated.swift", "/opt/vendor/Vendor.swift"]"#
    );
}

#[test]
fn prelude_xcode_normalizes_project_relative_dot_segments() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr([
    _xcode_join("Framework", "../Sources/File.swift"),
    _xcode_join("app/./Sources", "../Generated/File.swift"),
    _xcode_normalize_path("../../outside.swift"),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["Sources/File.swift", "app/Generated/File.swift", "../../outside.swift"]"#
    );
}

#[test]
fn prelude_xcode_filters_excluded_source_file_names() {
    let prelude = xcode_prelude_source();
    // `EXCLUDED_SOURCE_FILE_NAMES` drops matching sources (by basename or path)
    // and `INCLUDED_SOURCE_FILE_NAMES` re-includes a subset, matching Xcode's
    // per-platform source filtering.
    let source = format!(
        r#"{prelude}
result = repr([
    _xcode_glob_match("*_iOS.swift", "View_iOS.swift"),
    _xcode_glob_match("*_iOS.swift", "View_macOS.swift"),
    _xcode_glob_match("*/Legacy/*", "App/Legacy/Old.swift"),
    _xcode_filter_excluded_sources(
        ["App/View.swift", "App/View_macOS.swift", "App/Legacy/Old.swift", "App/Keep_macOS.swift"],
        {{
            "EXCLUDED_SOURCE_FILE_NAMES": ["*_macOS.swift", "*/Legacy/*"],
            "INCLUDED_SOURCE_FILE_NAMES": ["Keep_macOS.swift"],
        }},
    ),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[True, False, True, ["App/View.swift", "App/Keep_macOS.swift"]]"#
    );
}

#[test]
fn prelude_xcode_excludes_matching_resources_from_the_build() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr(_xcode_filter_excluded_files(
    ["Assets/Required.dat", "Tests/Fixtures/Optional.mov"],
    {{"EXCLUDED_SOURCE_FILE_NAMES": ["Tests/Fixtures/Optional.mov"]}},
))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["Assets/Required.dat"]"#
    );
}

#[test]
fn prelude_xcode_selects_conditional_build_settings() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
settings = _xcode_select_conditional_settings(
    {{
        "CARGO_BUILD_TARGET[sdk=iphoneos*]": "aarch64-apple-ios",
        "CARGO_BUILD_TARGET[sdk=iphoneos*][arch=arm64e]": "arm64e-apple-ios",
        "CARGO_BUILD_TARGET[sdk=iphonesimulator*][arch=*]": "x86_64-apple-ios",
        "CARGO_BUILD_TARGET[sdk=iphonesimulator*][arch=arm64]": "aarch64-apple-ios-sim",
        "LIBRARY_PATH": "target/$(CARGO_BUILD_TARGET)/release/libexample.a",
        "ONLY_DEBUG[config=Debug]": "enabled",
        "MISSING_PARAMETER[variant=*]": "fallback",
    }},
    {{"arch": "arm64", "config": "Debug", "sdk": "iphonesimulator"}},
)
result = repr([
    _xcode_parse_setting("CARGO_BUILD_TARGET[sdk=iphonesimulator*][arch=arm64] = aarch64-apple-ios-sim"),
    settings.get("CARGO_BUILD_TARGET"),
    _xcode_resolve_vars(settings["LIBRARY_PATH"], settings),
    settings.get("ONLY_DEBUG"),
    settings.get("MISSING_PARAMETER"),
    settings.get("CARGO_BUILD_TARGET[sdk=iphonesimulator*][arch=arm64]"),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[("CARGO_BUILD_TARGET[sdk=iphonesimulator*][arch=arm64]", "aarch64-apple-ios-sim"), "aarch64-apple-ios-sim", "target/aarch64-apple-ios-sim/release/libexample.a", "enabled", "fallback", None]"#
    );
}

#[test]
fn prelude_xcode_tokenizes_quoted_build_settings() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr([
    _xcode_split_setting('$(LIBRARY) $(inherited) -u _symbol -l"z" -framework "Vendor Kit"'),
    _xcode_split_setting('"Path With Spaces/File.a" -ObjC'),
    _xcode_linkopts(
        {{"OTHER_LDFLAGS": '$(LIBRARY) $(inherited) -u _symbol -l"z" -framework "Vendor Kit" -weak_framework OptionalKit'}},
        {{"LIBRARY": "Vendor/libExample.a"}},
    ),
    _apple_collect_transitive_linkopts(
        [{{"transitive_linkopts": ["-Xlinker", "-weak_framework", "-Xlinker", "SecondKit"]}}],
        ["-Xlinker", "-weak_framework", "-Xlinker", "FirstKit"],
    ),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["$(LIBRARY)", "$(inherited)", "-u", "_symbol", "-lz", "-framework", "Vendor Kit"], ["Path With Spaces/File.a", "-ObjC"], ["Vendor/libExample.a", "-Xlinker", "-u", "-Xlinker", "_symbol", "-lz", "-framework", "Vendor Kit", "-Xlinker", "-weak_framework", "-Xlinker", "OptionalKit"], ["-Xlinker", "-weak_framework", "-Xlinker", "FirstKit", "-Xlinker", "-weak_framework", "-Xlinker", "SecondKit"]]"#
    );
}

#[test]
fn prelude_xcode_package_target_excludes_cover_target_relative_paths() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr([
    _xcode_swift_package_target_path_is_excluded(
        ".once/packages/Example/",
        "Sources/Example",
        ["Supporting Files"],
        ".once/packages/Example/Sources/Example/Supporting Files/Example.h",
    ),
    _xcode_swift_package_target_path_is_excluded(
        ".once/packages/Example/",
        "Sources/Example",
        ["Supporting Files"],
        ".once/packages/Example/Sources/Example/Public/Example.h",
    ),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        "[True, False]"
    );
}

#[test]
fn prelude_xcode_package_public_headers_path_controls_headers_and_modulemap() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

def host_path_exists(path):
    return True

def host_file_exists(path):
    return False

def _xcode_host_directory_exists(path):
    return True

def host_which(name):
    return "/usr/bin/" + name

def host_command(argv, env = None, merge_stderr = None):
    if len(argv) < 3 or argv[1] != "-L":
        fail("public header discovery must follow symbolic links: " + str(argv))
    return "/workspace/.once/packages/Down/Sources/cmark/cmark.h\n/workspace/.once/packages/Down/Sources/cmark/node.h\n"

target = {{
    "name": "libcmark",
    "path": "Sources/cmark",
    "publicHeadersPath": "./",
    "exclude": ["include"],
}}
result = repr([
    _xcode_swift_package_target_headers(".once/packages/Down", target),
    _xcode_swift_package_include_dirs(".once/packages/Down", target),
    _xcode_swift_package_target_modulemap(".once/packages/Down", target),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[[".once/packages/Down/Sources/cmark/cmark.h", ".once/packages/Down/Sources/cmark/node.h"], [".once/packages/Down/Sources/cmark"], ""]"#
    );
}

#[test]
fn prelude_xcode_spm_parses_refs_and_products() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "SPARKLE_REF": {
            "isa": "XCRemoteSwiftPackageReference",
            "repositoryURL": "https://github.com/sparkle-project/Sparkle.git",
            "requirement": {"kind": "upToNextMajorVersion", "minimumVersion": "2.0.0"},
        },
        "MAS_REF": {
            "isa": "XCRemoteSwiftPackageReference",
            "repositoryURL": "https://github.com/rxhanson/MASShortcut",
            "requirement": {"kind": "revision", "revision": "2f9fbb3f"},
        },
        "SPARKLE_PROD": {"isa": "XCSwiftPackageProductDependency", "productName": "Sparkle", "package": "SPARKLE_REF"},
        "MAS_PROD": {"isa": "XCSwiftPackageProductDependency", "productName": "MASShortcut", "package": "MAS_REF"},
        "APP": {"isa": "PBXNativeTarget", "name": "App", "packageProductDependencies": ["SPARKLE_PROD", "MAS_PROD"]},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
objects = json_decode({objects:?})
refs = _xcode_spm_package_refs(objects)
products = _xcode_target_spm_products(objects, objects["APP"], refs)
result = repr([
    refs["SPARKLE_REF"]["identity"],
    refs["MAS_REF"]["identity"],
    [p["name"] + "@" + p["package_identity"] for p in products],
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["Sparkle", "MASShortcut", ["Sparkle@Sparkle", "MASShortcut@MASShortcut"]]"#
    );
}

#[test]
fn prelude_xcode_spm_collects_framework_phase_product_refs() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "LOCAL_PRODUCT": {"isa": "XCSwiftPackageProductDependency", "productName": "SharedKit"},
        "BUILD_FILE": {"isa": "PBXBuildFile", "productRef": "LOCAL_PRODUCT"},
        "FRAMEWORKS": {"isa": "PBXFrameworksBuildPhase", "files": ["BUILD_FILE"]},
        "APP": {"isa": "PBXNativeTarget", "name": "App", "buildPhases": ["FRAMEWORKS"]},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
objects = json_decode({objects:?})
products = _xcode_target_spm_products(objects, objects["APP"], {{}})
result = repr([[product["name"], product["package_identity"]] for product in products])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["SharedKit", ""]]"#
    );
}

#[test]
fn prelude_xcode_recovers_built_framework_target_dependencies() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "FRAMEWORK": {"isa": "PBXFileReference", "name": "Pods_Sample.framework", "sourceTree": "BUILT_PRODUCTS_DIR"},
        "BUILD": {"isa": "PBXBuildFile", "fileRef": "FRAMEWORK"},
        "PHASE": {"isa": "PBXFrameworksBuildPhase", "files": ["BUILD"]},
        "APP": {"isa": "PBXNativeTarget", "buildPhases": ["PHASE"]},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
objects = json_decode({objects:?})
result = repr(_xcode_framework_product_dependencies(objects, objects["APP"], {{"Pods-Sample": "Pods-Sample"}}))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["Pods-Sample"]"#
    );
}

#[test]
fn prelude_xcode_recovers_xcframework_dependencies_from_framework_phase() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "DEPENDENCY": {
            "isa": "PBXFileReference",
            "path": "DependencyModule.xcframework",
            "sourceTree": "<group>"
        },
        "BUILD": {"isa": "PBXBuildFile", "fileRef": "DEPENDENCY"},
        "FRAMEWORKS": {"isa": "PBXFrameworksBuildPhase", "files": ["BUILD"]},
        "FEATURE": {"isa": "PBXNativeTarget", "buildPhases": ["FRAMEWORKS"]},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
objects = json_decode({objects:?})
result = repr(_xcode_xcframework_dependencies(
    objects,
    objects["FEATURE"],
    {{"DEPENDENCY": "Vendor/DependencyModule.xcframework"}},
    {{"Vendor/DependencyModule.xcframework": "XCFramework_Vendor_DependencyModule.xcframework"}},
))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["XCFramework_Vendor_DependencyModule.xcframework"]"#
    );
}

#[test]
fn prelude_xcode_recovers_xcframework_dependencies_from_copy_files_phase() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "DEPENDENCY": {
            "isa": "PBXFileReference",
            "path": "DependencyModule.xcframework",
            "sourceTree": "<group>"
        },
        "BUILD": {"isa": "PBXBuildFile", "fileRef": "DEPENDENCY"},
        "COPY_XCFRAMEWORK": {
            "isa": "PBXCopyFilesBuildPhase",
            "dstPath": "_StaticXCFrameworkDependencies/Feature",
            "dstSubfolderSpec": 16,
            "files": ["BUILD"]
        },
        "FEATURE": {"isa": "PBXNativeTarget", "buildPhases": ["COPY_XCFRAMEWORK"]},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
objects = json_decode({objects:?})
result = repr(_xcode_xcframework_dependencies(
    objects,
    objects["FEATURE"],
    {{"DEPENDENCY": "Vendor/DependencyModule.xcframework"}},
    {{"Vendor/DependencyModule.xcframework": "XCFramework_Vendor_DependencyModule.xcframework"}},
))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["XCFramework_Vendor_DependencyModule.xcframework"]"#
    );
}

#[test]
fn prelude_xcode_recovers_xcframework_dependencies_through_workspace_symlink() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "DEPENDENCY": {
            "isa": "PBXFileReference",
            "path": "/cache/hash/DependencyModule.xcframework",
            "sourceTree": "<absolute>"
        },
        "BUILD": {"isa": "PBXBuildFile", "fileRef": "DEPENDENCY"},
        "COPY_XCFRAMEWORK": {
            "isa": "PBXCopyFilesBuildPhase",
            "dstPath": "_StaticXCFrameworkDependencies/Feature",
            "dstSubfolderSpec": 16,
            "files": ["BUILD"]
        },
        "FEATURE": {"isa": "PBXNativeTarget", "buildPhases": ["COPY_XCFRAMEWORK"]},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

def host_path_exists(path):
    return True

def host_which(name):
    return "/usr/bin/" + name

def host_command(argv, env = None, merge_stderr = None):
    if argv[0] == "/usr/bin/realpath":
        if argv[1] == "/workspace/Derived/FrameworkSearchPaths/DependencyModule.xcframework":
            return "/cache/hash/DependencyModule.xcframework\n"
        return argv[1] + "\n"
    fail("unexpected host command: " + str(argv))

objects = json_decode({objects:?})
specs = [{{
    "name": "XCFramework_Derived_FrameworkSearchPaths_DependencyModule.xcframework",
    "attrs": {{"bundle": "Derived/FrameworkSearchPaths/DependencyModule.xcframework"}},
}}]
result = repr(_xcode_xcframework_dependencies(
    objects,
    objects["FEATURE"],
    {{"DEPENDENCY": "/cache/hash/DependencyModule.xcframework"}},
    _xcode_xcframework_name_map(specs),
))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["XCFramework_Derived_FrameworkSearchPaths_DependencyModule.xcframework"]"#
    );
}

#[test]
fn prelude_xcode_matches_imported_modules_to_cached_xcframework_references() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "DEPENDENCY": {
            "isa": "PBXFileReference",
            "name": "Dependency-Module.xcframework",
            "path": "/cache/hash/Dependency-Module.xcframework",
            "sourceTree": "<absolute>"
        },
        "UNRELATED": {
            "isa": "PBXFileReference",
            "name": "Unrelated.framework",
            "path": "/cache/hash/Unrelated.framework",
            "sourceTree": "<absolute>"
        },
    })
    .to_string();
    let source = format!(
        r#"{prelude}
objects = json_decode({objects:?})
modules = _xcode_xcframework_module_map(
    objects,
    {{"DEPENDENCY": "/cache/hash/Dependency-Module.xcframework"}},
    {{"/cache/hash/Dependency-Module.xcframework": "XCFramework_Dependency_Module"}},
)
result = repr([
    modules.get(_xcode_product_dependency_key("Dependency_Module")),
    modules.get(_xcode_product_dependency_key("Unrelated")),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["XCFramework_Dependency_Module", None]"#
    );
}

#[test]
fn prelude_xcode_recovers_transitive_modules_from_disabled_autolink_flags() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
modules = _xcode_disabled_autolink_modules([
    "-Xfrontend", "-disable-autolink-framework", "-Xfrontend", "Dependency_Module",
    "-Xfrontend", "-disable-autolink-library", "-Xfrontend", "SupportLibrary",
    "-enable-upcoming-feature", "MemberImportVisibility",
])
cached = {{
    _xcode_product_dependency_key("Dependency-Module"): "XCFramework_Dependency_Module",
    _xcode_product_dependency_key("SupportLibrary"): "XCFramework_SupportLibrary",
}}
result = repr([cached.get(_xcode_product_dependency_key(module)) for module in modules])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["XCFramework_Dependency_Module", "XCFramework_SupportLibrary"]"#
    );
}

#[test]
fn prelude_xcode_recovers_dependencies_from_prebuilt_swiftmodule_symbols() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
cached = {{
    _xcode_product_dependency_key("PrimaryModule"): "XCFramework_Primary",
    _xcode_product_dependency_key("Support_Module"): "XCFramework_Support",
}}
result = repr(_xcode_xcframework_dependency_ids(
    "XCFramework_Primary",
    ["PrimaryModule", "Support_Module", "unrelated_symbol"],
    cached,
))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["XCFramework_Support"]"#
    );
}

#[test]
fn prelude_xcode_rejects_prebuilt_dependency_edges_that_close_a_cycle() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
specs = {{
    "Primary": {{"name": "Primary", "deps": ["./Support"]}},
    "Support": {{"name": "Support", "deps": []}},
    "Leaf": {{"name": "Leaf", "deps": []}},
}}
result = repr(_xcode_acyclic_xcframework_dependencies(
    "Support",
    ["Primary", "Leaf"],
    specs,
))
"#
    );
    assert_eq!(eval_prelude_source_to_repr(source).unwrap(), r#"["Leaf"]"#);
}

#[test]
fn prelude_xcode_attaches_xcframework_dependencies_from_serialized_imports() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

def host_path_exists(path):
    return True

def host_file_exists(path):
    return True

def host_arch():
    return "arm64"

def host_which(name):
    return "/usr/bin/" + name

def host_command(argv, env = None, cwd = None, merge_stderr = None):
    tool = argv[0].split("/")[-1]
    if tool == "realpath":
        return argv[1] + "\n"
    if tool == "plutil":
        return '{{"AvailableLibraries": [{{"LibraryIdentifier": "ios-arm64-simulator", "SupportedPlatform": "ios", "SupportedPlatformVariant": "simulator", "SupportedArchitectures": ["arm64"]}}]}}'
    if tool == "find":
        if argv[-1] == "*.swiftinterface":
            return ""
        root = argv[2]
        name = "ModuleA" if root.find("ModuleA.xcframework") >= 0 else "ModuleB"
        return root + "/" + name + ".framework/Modules/" + name + ".swiftmodule/arm64-apple-ios-simulator.swiftmodule\n"
    if tool == "xcrun":
        if "--show-sdk-path" in argv:
            return "/SDKs/iPhoneSimulator.sdk\n"
        return "26.0\n"
    if tool == "strings":
        # Symbol noise: ModuleB's binary mentions ModuleA even though the
        # serialized imports of ModuleB do not include it.
        polluted = argv[1].find("ModuleB.xcframework") >= 0
        return "ModuleA\nnoise\n" if polluted else "ModuleB\nnoise\n"
    if tool == "sh":
        if argv[4] == "ModuleA":
            return "<stdin>:1:8: error: missing required modules: 'Foundation', 'ModuleB'\n"
        return ""
    fail("unexpected host command: " + str(argv))

specs = [
    {{"name": "App", "kind": "apple_application", "deps": ["./XCF_B", "./XCF_A"], "attrs": {{}}}},
    {{"name": "XCF_A", "kind": "apple_xcframework_import", "deps": [], "attrs": {{"bundle": "Vendor/ModuleA.xcframework", "platform": "ios", "sdk_variant": "simulator", "arch": "arm64"}}}},
    {{"name": "XCF_B", "kind": "apple_xcframework_import", "deps": [], "attrs": {{"bundle": "Vendor/ModuleB.xcframework", "platform": "ios", "sdk_variant": "simulator", "arch": "arm64"}}}},
]
module_targets = {{
    _xcode_product_dependency_key("ModuleA"): "XCF_A",
    _xcode_product_dependency_key("ModuleB"): "XCF_B",
}}
_xcode_attach_xcframework_module_dependencies(specs, module_targets)
result = repr([specs[1]["deps"], specs[2]["deps"]])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["./XCF_B"], []]"#
    );
}

#[test]
fn prelude_xcode_lowers_swift_package_name_setting() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr(_xcode_swift_language_flags({{"SWIFT_VERSION": "5.0", "SWIFT_PACKAGE_NAME": "PrimaryFeature"}}))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["-swift-version", "5", "-package-name", "PrimaryFeature"]"#
    );
}

#[test]
fn prelude_xcode_lowers_dead_code_stripping_setting() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr([
    _xcode_linkopts({{"OTHER_LDFLAGS": "-ObjC", "DEAD_CODE_STRIPPING": "YES"}}, {{}}),
    _xcode_linkopts({{"OTHER_LDFLAGS": "-ObjC"}}, {{}}),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["-Xlinker", "-ObjC", "-Xlinker", "-dead_strip"], ["-Xlinker", "-ObjC"]]"#
    );
}

#[test]
fn prelude_xcode_lowers_swift_include_paths_setting() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
settings = {{
    "OTHER_SWIFT_FLAGS": "-DX",
    "SWIFT_INCLUDE_PATHS": ["$(inherited)", "$(SRCROOT)/../Vendor/Lib.xcframework/ios-arm64"],
}}
result = repr(_xcode_swift_flags(settings, {{"SRCROOT": "Modules/App"}}))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["-DX", "-I", "Modules/App/../Vendor/Lib.xcframework/ios-arm64"]"#
    );
}

#[test]
fn prelude_xcode_skips_platform_filtered_build_files() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "APPKIT": {"isa": "PBXFileReference", "name": "AppKit.framework", "path": "System/Library/Frameworks/AppKit.framework", "sourceTree": "DEVELOPER_DIR"},
        "UIKIT": {"isa": "PBXFileReference", "name": "UIKit.framework", "path": "System/Library/Frameworks/UIKit.framework", "sourceTree": "SDKROOT"},
        "APPKIT_BUILD": {"isa": "PBXBuildFile", "fileRef": "APPKIT", "platformFilters": ["macos"]},
        "UIKIT_BUILD": {"isa": "PBXBuildFile", "fileRef": "UIKIT"},
        "FRAMEWORKS": {"isa": "PBXFrameworksBuildPhase", "files": ["APPKIT_BUILD", "UIKIT_BUILD"]},
        "TARGET": {"isa": "PBXNativeTarget", "buildPhases": ["FRAMEWORKS"]},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
objects = json_decode({objects:?})
ios = _xcode_classic_phase_files({{}}, objects, objects["TARGET"], {{}}, "ios")["frameworks"]
macos = _xcode_classic_phase_files({{}}, objects, objects["TARGET"], {{}}, "macos")["frameworks"]
result = repr([ios, macos])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["UIKit"], ["AppKit", "UIKit"]]"#
    );
}

#[test]
fn prelude_xcode_classifies_tbd_frameworks_phase_files_as_sdk_dylibs() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "UIKIT": {"isa": "PBXFileReference", "name": "UIKit.framework", "path": "System/Library/Frameworks/UIKit.framework", "sourceTree": "SDKROOT"},
        "RESOLV": {"isa": "PBXFileReference", "name": "libresolv.tbd", "path": "Platforms/iPhoneOS.platform/Developer/SDKs/iPhoneOS.sdk/usr/lib/libresolv.tbd", "sourceTree": "DEVELOPER_DIR"},
        "UIKIT_BUILD": {"isa": "PBXBuildFile", "fileRef": "UIKIT"},
        "RESOLV_BUILD": {"isa": "PBXBuildFile", "fileRef": "RESOLV"},
        "FRAMEWORKS": {"isa": "PBXFrameworksBuildPhase", "files": ["UIKIT_BUILD", "RESOLV_BUILD"]},
        "TARGET": {"isa": "PBXNativeTarget", "buildPhases": ["FRAMEWORKS"]},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
objects = json_decode({objects:?})
files = _xcode_classic_phase_files({{}}, objects, objects["TARGET"], {{}})
result = repr([files["frameworks"], files["sdk_dylibs"]])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["UIKit"], ["resolv"]]"#
    );
}

#[test]
fn prelude_xcode_treats_staticlib_mach_o_framework_as_static() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr([
    _xcode_framework_is_static({{"MACH_O_TYPE": "staticlib"}}, "com.apple.product-type.framework"),
    _xcode_framework_is_static({{}}, "com.apple.product-type.framework"),
    _xcode_framework_is_static({{}}, "com.apple.product-type.framework.static"),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[True, False, True]"#
    );
}

#[test]
fn prelude_xcode_parses_swiftinterface_imports() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
text = "\n".join([
    "// swift-interface-format-version: 1.0",
    "// swift-module-flags: -target arm64-apple-ios14.0-simulator -module-name TensorFlowLite",
    "import Foundation",
    "@_exported import TensorFlowLiteC",
    "@preconcurrency @_implementationOnly import SecretCore",
    "import struct SwiftShims.Detail",
    "import Swift",
    "public class Interpreter {{}}",
])
result = repr(_xcode_swiftinterface_imports(text))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["Foundation", "TensorFlowLiteC", "SecretCore", "SwiftShims", "Swift"]"#
    );
}

#[test]
fn prelude_xcode_recovers_imports_from_interface_only_xcframework() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

def host_path_exists(path):
    return True

def host_file_exists(path):
    return True

def host_arch():
    return "arm64"

def host_which(name):
    return "/usr/bin/" + name

def host_command(argv, env = None, cwd = None, merge_stderr = None):
    tool = argv[0].split("/")[-1]
    if tool == "plutil":
        return '{{"AvailableLibraries": [{{"LibraryIdentifier": "ios-arm64_x86_64-simulator", "SupportedPlatform": "ios", "SupportedPlatformVariant": "simulator", "SupportedArchitectures": ["arm64", "x86_64"]}}]}}'
    if tool == "find":
        if argv[-1] == "*.swiftmodule":
            return ""
        return argv[2] + "/TensorFlowLite.framework/Modules/TensorFlowLite.swiftmodule/arm64.swiftinterface\n"
    if tool == "sh":
        return "// swift-module-flags: -module-name TensorFlowLite\nimport Foundation\n@_exported import TensorFlowLiteC\n"
    fail("unexpected host command: " + str(argv))

spec = {{"name": "XCF_TFL", "kind": "apple_xcframework_import", "deps": [], "attrs": {{"bundle": "Vendor/TensorFlowLite.xcframework", "platform": "ios", "sdk_variant": "simulator", "arch": "arm64"}}}}
module_files = _xcode_xcframework_selected_swiftmodules(spec)
result = repr(_xcode_xcframework_serialized_imports(spec, module_files))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["Foundation", "TensorFlowLiteC"]"#
    );
}

#[test]
fn prelude_xcode_probe_parses_aggregated_missing_module_list() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
output = "<stdin>:1:8: error: missing required modules: 'Alpha', 'Beta', 'Gamma', 'Delta'\n  | note\n<stdin>:1:8: error: missing required module 'Extra'\n"
result = repr(_xcode_probe_missing_modules(output))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["Alpha", "Beta", "Gamma", "Delta", "Extra"]"#
    );
}

#[test]
fn prelude_xcode_recovers_xcframework_dependencies_from_embed_frameworks_copy_files_phase() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "DEPENDENCY": {
            "isa": "PBXFileReference",
            "path": "DependencyModule.xcframework",
            "sourceTree": "<group>"
        },
        "BUILD": {"isa": "PBXBuildFile", "fileRef": "DEPENDENCY"},
        "EMBED_FRAMEWORKS": {
            "isa": "PBXCopyFilesBuildPhase",
            "dstPath": "",
            "dstSubfolderSpec": 10,
            "files": ["BUILD"]
        },
        "FEATURE": {"isa": "PBXNativeTarget", "buildPhases": ["EMBED_FRAMEWORKS"]},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
objects = json_decode({objects:?})
result = repr(_xcode_xcframework_dependencies(
    objects,
    objects["FEATURE"],
    {{"DEPENDENCY": "Vendor/DependencyModule.xcframework"}},
    {{"Vendor/DependencyModule.xcframework": "XCFramework_Vendor_DependencyModule.xcframework"}},
))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["XCFramework_Vendor_DependencyModule.xcframework"]"#
    );
}

#[test]
fn prelude_xcode_does_not_treat_built_resource_products_as_sources() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "PRODUCT": {
            "isa": "PBXFileReference",
            "path": "Privacy.bundle",
            "sourceTree": "BUILT_PRODUCTS_DIR"
        },
        "SOURCE": {
            "isa": "PBXFileReference",
            "path": "PrivacyInfo.xcprivacy",
            "sourceTree": "<group>"
        },
        "PRODUCT_BUILD": {"isa": "PBXBuildFile", "fileRef": "PRODUCT"},
        "SOURCE_BUILD": {"isa": "PBXBuildFile", "fileRef": "SOURCE"},
        "RESOURCES": {
            "isa": "PBXResourcesBuildPhase",
            "files": ["PRODUCT_BUILD", "SOURCE_BUILD"]
        },
        "TARGET": {"isa": "PBXNativeTarget", "buildPhases": ["RESOURCES"]},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
objects = json_decode({objects:?})
result = repr(_xcode_classic_phase_files(
    {{}},
    objects,
    objects["TARGET"],
    {{"SOURCE": "Resources/PrivacyInfo.xcprivacy"}},
)["resources"])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["Resources/PrivacyInfo.xcprivacy"]"#
    );
}

#[test]
fn prelude_xcode_preserves_public_header_visibility() {
    let prelude = xcode_prelude_source();
    let objects = serde_json::json!({
        "PUBLIC": {"isa": "PBXFileReference", "path": "Public.h"},
        "PRIVATE": {"isa": "PBXFileReference", "path": "Private.h"},
        "PROJECT": {"isa": "PBXFileReference", "path": "Project.h"},
        "PUBLIC_BUILD": {
            "isa": "PBXBuildFile",
            "fileRef": "PUBLIC",
            "settings": {"ATTRIBUTES": ["Public"]}
        },
        "PRIVATE_BUILD": {
            "isa": "PBXBuildFile",
            "fileRef": "PRIVATE",
            "settings": {"ATTRIBUTES": ["Private"]}
        },
        "PROJECT_BUILD": {"isa": "PBXBuildFile", "fileRef": "PROJECT"},
        "HEADERS": {
            "isa": "PBXHeadersBuildPhase",
            "files": ["PUBLIC_BUILD", "PRIVATE_BUILD", "PROJECT_BUILD"]
        },
        "TARGET": {"isa": "PBXNativeTarget", "buildPhases": ["HEADERS"]},
    })
    .to_string();
    let source = format!(
        r#"{prelude}
objects = json_decode({objects:?})
files = _xcode_classic_phase_files(
    {{}},
    objects,
    objects["TARGET"],
    {{"PUBLIC": "Headers/Public.h", "PRIVATE": "Headers/Private.h", "PROJECT": "Headers/Project.h"}},
)
result = repr([files["headers"], files["exported_headers"]])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["Headers/Public.h", "Headers/Private.h", "Headers/Project.h"], ["Headers/Public.h"]]"#
    );
}

#[test]
fn prelude_xcode_resolves_headers_named_by_modulemaps() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return ""

def host_file_exists(path):
    return path == "Support/Main.modulemap"

def host_file_read(path):
    if path == "Support/Main.modulemap":
        return '''framework module Main {{
  umbrella header "Public.h"
  explicit module Internal {{
    private textual header "Private.h"
  }}
}}'''
    fail("unexpected host_file_read: " + path)

result = repr(_xcode_modulemap_headers(
    "Support/Main.modulemap",
    ["Sources/Public.h", "Vendor/Private.h", "Sources/Unrelated.h"],
))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["Sources/Public.h", "Vendor/Private.h"]"#
    );
}

#[test]
fn prelude_xcode_version_key_orders_deployment_targets() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
result = repr([
    _xcode_version_key("17.0") > _xcode_version_key("12.0"),
    _xcode_version_key("14.2") > _xcode_version_key("14.0"),
    _xcode_version_key("9.0") < _xcode_version_key("10.0"),
])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        "[True, True, True]"
    );
}

#[test]
fn prelude_xcode_synced_group_honors_membership_exceptions() {
    let prelude = xcode_prelude_source();
    // A synchronized root group owned by target `App` excludes a sibling
    // extension's source and a resource through a membership exception set.
    // Those files must not join the app's sources even though they live under
    // the group directory.
    let objects = serde_json::json!({
        "APPTGT": {
            "isa": "PBXNativeTarget",
            "name": "App",
            "fileSystemSynchronizedGroups": ["G"],
        },
        "G": {
            "isa": "PBXFileSystemSynchronizedRootGroup",
            "path": "App",
            "sourceTree": "<group>",
            "exceptions": ["EX"],
        },
        "EX": {
            "isa": "PBXFileSystemSynchronizedBuildFileExceptionSet",
            "target": "APPTGT",
            "membershipExceptions": ["Widget/WidgetData.swift", "Info.plist"],
        },
    })
    .to_string();
    let source = format!(
        r#"{prelude}
objects = json_decode({objects:?})

def glob(patterns):
    return ["App/Main.swift", "App/Widget/WidgetData.swift", "App/Info.plist", "App/Assets.xcassets"]

path_maps = {{"files": {{}}, "groups": {{"G": "App"}}, "additive": {{}}}}
files = _xcode_synced_group_files({{}}, objects, objects["APPTGT"], "", path_maps)
result = repr([files["sources"], files["asset_catalogs"], files["resources"]])
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["App/Main.swift"], ["App/Assets.xcassets"], []]"#
    );
}

#[test]
fn prelude_xcode_parse_workspace_data_resolves_nested_project_paths() {
    let prelude = xcode_prelude_source();
    // A workspace that references the main project at the top level and a
    // package project nested several groups deep, mirroring how a Tuist-
    // generated workspace lists its dependency projects under `.build`.
    let data = r#"<?xml version="1.0" encoding="UTF-8"?>
<Workspace version = "1.0">
   <FileRef location = "group:Tuist.xcodeproj"></FileRef>
   <Group location = "group:.build" name = ".build">
      <Group location = "group:registry" name = "registry">
         <Group location = "group:kolos65" name = "kolos65">
            <Group location = "group:Mockable" name = "Mockable">
               <FileRef location = "group:0.6.2/Mockable.xcodeproj"></FileRef>
            </Group>
         </Group>
      </Group>
   </Group>
</Workspace>
"#;
    let source = format!(
        r#"{prelude}
result = repr(_xcode_parse_workspace_data({data:?}, ""))
"#
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["Tuist.xcodeproj", ".build/registry/kolos65/Mockable/0.6.2/Mockable.xcodeproj"]"#
    );
}

#[test]
fn prelude_xcode_uses_workspace_lockfile_for_a_nested_project() {
    let prelude = xcode_prelude_source();
    let unrelated = serde_json::json!({
        "version": 3,
        "pins": [{
            "identity": "unrelated",
            "kind": "remoteSourceControl",
            "state": {"revision": "unrelated-revision"},
        }],
    })
    .to_string();
    let workspace = serde_json::json!({
        "version": 1,
        "object": {
            "pins": [{
                "package": "ProtonCore",
                "repositoryURL": "https://example.invalid/protoncore.git",
                "state": {"revision": "workspace-revision"},
            }],
        },
    })
    .to_string();
    let source = format!(
        r#"{prelude}
def workspace_root():
    return "/workspace"

def host_which(name):
    return name

def host_command(argv, env = None, cwd = None, merge_stderr = None):
    if argv[0] == "find":
        return "/workspace/Other.xcworkspace/xcshareddata/swiftpm/Package.resolved\n/workspace/ProtonVPN.xcworkspace/xcshareddata/swiftpm/Package.resolved"
    fail("unexpected host_command: " + str(argv))

def host_file_exists(path):
    return path.endswith("Other.xcworkspace/xcshareddata/swiftpm/Package.resolved") or path.endswith("ProtonVPN.xcworkspace/xcshareddata/swiftpm/Package.resolved")

def host_file_read(path):
    if path.endswith("Other.xcworkspace/xcshareddata/swiftpm/Package.resolved"):
        return {unrelated:?}
    if path.endswith("ProtonVPN.xcworkspace/xcshareddata/swiftpm/Package.resolved"):
        return {workspace:?}
    fail("unexpected host_file_read: " + path)

refs = {{"PACKAGE": {{"identity": "protoncore"}}}}
result = repr(_xcode_package_resolved_pins({{}}, "apps/ios/iOS.xcodeproj", "apps/ios/iOS.xcodeproj", refs))
"#,
    );

    let result = eval_prelude_source_to_repr(source).unwrap();
    assert!(result.contains("protoncore"), "{result}");
    assert!(result.contains("workspace-revision"), "{result}");
    assert!(!result.contains("unrelated-revision"), "{result}");
}

#[test]
fn prelude_xcode_keeps_nested_project_paths_workspace_relative() {
    let prelude = xcode_prelude_source();
    let source = format!(
        r#"{prelude}
def glob(patterns):
    return ["apps/ios/iOS.xcodeproj/project.pbxproj"]

ctx = {{
    "label": {{"package": "apps/ios", "id": "apps/ios/xcode"}},
    "attr": {{}},
}}
result = repr(_xcode_project_path(ctx))
"#,
    );

    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#""apps/ios/iOS.xcodeproj""#
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn prelude_xcode_workspace_resolver_lowers_native_targets() {
    let prelude = xcode_prelude_source();
    let pbxproj = serde_json::json!({
        "rootObject": "ROOT",
        "objects": {
            "ROOT": {
                "isa": "PBXProject",
                "buildConfigurationList": "PROJ_CL",
                "targets": ["FW", "APP", "TEST"],
                "mainGroup": "GRP",
            },
            "PROJ_CL": {
                "isa": "XCConfigurationList",
                "buildConfigurations": ["PROJ_DBG"],
                "defaultConfigurationName": "Debug",
            },
            "PROJ_DBG": {
                "isa": "XCBuildConfiguration",
                "name": "Debug",
                "buildSettings": {
                    "SDKROOT": "iphoneos",
                    "IPHONEOS_DEPLOYMENT_TARGET": "16.0",
                },
            },
            // Feature.swift is nested two groups deep (Source/Core) so the
            // resolver must reconstruct its path through the group chain; the
            // application and test sources sit at the group root.
            "GRP": {
                "isa": "PBXGroup",
                "children": ["SOURCE_GRP", "APP_FILE", "TEST_FILE"],
                "sourceTree": "<group>",
            },
            "SOURCE_GRP": {
                "isa": "PBXGroup",
                "path": "Source",
                "children": ["CORE_GRP"],
                "sourceTree": "<group>",
            },
            "CORE_GRP": {
                "isa": "PBXGroup",
                "path": "Core",
                "children": ["FW_FILE"],
                "sourceTree": "<group>",
            },

            "FW": {
                "isa": "PBXNativeTarget",
                "name": "Feature",
                "productType": "com.apple.product-type.framework",
                "buildConfigurationList": "FW_CL",
                "buildPhases": ["FW_SRC"],
                "dependencies": [],
            },
            "FW_CL": {
                "isa": "XCConfigurationList",
                "buildConfigurations": ["FW_DBG"],
                "defaultConfigurationName": "Debug",
            },
            "FW_DBG": {
                "isa": "XCBuildConfiguration",
                "name": "Debug",
                "buildSettings": {
                    "PRODUCT_NAME": "$(TARGET_NAME)",
                    "PRODUCT_BUNDLE_IDENTIFIER": "dev.once.Feature",
                    "ENABLE_TESTABILITY": "YES",
                },
            },
            "FW_SRC": {"isa": "PBXSourcesBuildPhase", "files": ["FW_BF"]},
            "FW_BF": {
                "isa": "PBXBuildFile",
                "fileRef": "FW_FILE",
                "settings": {"COMPILER_FLAGS": "-DNDEBUG -fno-objc-arc"},
            },
            "FW_FILE": {
                "isa": "PBXFileReference",
                "path": "Feature.swift",
                "sourceTree": "<group>",
            },

            "APP": {
                "isa": "PBXNativeTarget",
                "name": "App",
                "productType": "com.apple.product-type.application",
                "buildConfigurationList": "APP_CL",
                "buildPhases": ["APP_SRC"],
                "dependencies": ["APP_DEP"],
            },
            "APP_CL": {
                "isa": "XCConfigurationList",
                "buildConfigurations": ["APP_DBG"],
                "defaultConfigurationName": "Debug",
            },
            "APP_DBG": {
                "isa": "XCBuildConfiguration",
                "name": "Debug",
                "buildSettings": {
                    "PRODUCT_NAME": "$(TARGET_NAME)",
                    "PRODUCT_BUNDLE_IDENTIFIER": "dev.once.App",
                    "DEVELOPMENT_TEAM": "TEAM123",
                    "TARGETED_DEVICE_FAMILY": "1,2",
                    "ENABLE_TESTABILITY": "YES",
                },
            },
            "APP_SRC": {"isa": "PBXSourcesBuildPhase", "files": ["APP_BF"]},
            "APP_BF": {"isa": "PBXBuildFile", "fileRef": "APP_FILE"},
            "APP_FILE": {
                "isa": "PBXFileReference",
                "path": "App.swift",
                "sourceTree": "<group>",
            },
            "APP_DEP": {"isa": "PBXTargetDependency", "target": "FW"},

            "TEST": {
                "isa": "PBXNativeTarget",
                "name": "AppTests",
                "productType": "com.apple.product-type.bundle.unit-test",
                "buildConfigurationList": "TEST_CL",
                "buildPhases": ["TEST_SRC"],
                "dependencies": [],
            },
            "TEST_CL": {
                "isa": "XCConfigurationList",
                "buildConfigurations": ["TEST_DBG"],
                "defaultConfigurationName": "Debug",
            },
            "TEST_DBG": {
                "isa": "XCBuildConfiguration",
                "name": "Debug",
                "buildSettings": {
                    "PRODUCT_NAME": "$(TARGET_NAME)",
                    "PRODUCT_BUNDLE_IDENTIFIER": "dev.once.AppTests",
                    "TEST_HOST": "$(BUILT_PRODUCTS_DIR)/App.app/$(BUNDLE_EXECUTABLE_FOLDER_PATH)/App",
                },
            },
            "TEST_SRC": {"isa": "PBXSourcesBuildPhase", "files": ["TEST_BF"]},
            "TEST_BF": {"isa": "PBXBuildFile", "fileRef": "TEST_FILE"},
            "TEST_FILE": {
                "isa": "PBXFileReference",
                "path": "AppTests.swift",
                "sourceTree": "<group>",
            },
        },
    })
    .to_string();

    let source = format!(
        r#"{prelude}
def workspace_root():
    return ""

def host_command(argv, env = None, cwd = None, merge_stderr = None):
    return {pbxproj:?}

def host_file_exists(path):
    return False

def host_file_read(path):
    return ""

ctx = {{
    "label": {{"package": "app", "name": "hello", "id": "app/hello"}},
    "attr": {{"project": "App.xcodeproj"}},
}}
graph = _xcode_workspace_resolver(ctx)
specs = {{spec["name"]: spec for spec in graph["targets"]}}
result = repr([
    graph["roots"],
    [specs["Feature"]["kind"], specs["App"]["kind"], specs["AppTests"]["kind"]],
    specs["App"]["deps"],
    specs["AppTests"]["deps"],
    specs["Feature"]["srcs"],
    specs["Feature"]["attrs"].get("per_source_clang_flags"),
    specs["App"]["srcs"],
    specs["App"]["attrs"].get("bundle_id"),
    specs["App"]["attrs"].get("development_team"),
    specs["App"]["attrs"].get("families"),
    specs["App"]["attrs"].get("minimum_os"),
    specs["App"]["attrs"].get("enable_testing"),
])
"#
    );

    let out = eval_prelude_source_to_repr(source).unwrap();
    assert_eq!(
        out,
        r#"[["App"], ["apple_framework", "apple_application", "apple_test_bundle"], ["./Feature"], ["./App"], ["Source/Core/Feature.swift"], {"Source/Core/Feature.swift": "[\"-DNDEBUG\",\"-fno-objc-arc\"]"}, ["App.swift"], "dev.once.App", "TEAM123", ["iphone", "ipad"], "16.0", True]"#
    );
}
