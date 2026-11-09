use std::process::{Command, Output};

fn workspace() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (path, contents) in [
        (
            "Cargo.toml",
            r#"[package]
name = "derive_demo"
version = "0.1.0"
edition = "2021"
[lib]
proc-macro = true
[dependencies]
helper = { path = "helper" }
shared = { path = "shared" }
[workspace]
members = ["helper", "shared", "leaf"]
resolver = "2"
"#,
        ),
        (
            "src/lib.rs",
            r"use proc_macro::TokenStream;
#[proc_macro]
pub fn identity(input: TokenStream) -> TokenStream { input }
#[test]
fn dependency_values() { assert_eq!(helper::value() + shared::value(), 42); }
",
        ),
        (
            "helper/Cargo.toml",
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\n[dependencies]\nshared = { path = \"../shared\" }\n",
        ),
        ("helper/src/lib.rs", "pub fn value() -> u8 { shared::value() }\n"),
        (
            "shared/Cargo.toml",
            "[package]\nname = \"shared\"\nversion = \"0.1.0\"\n[features]\ndefault = [\"extra\"]\nextra = []\n[dependencies]\nleaf = { path = \"../leaf\" }\n",
        ),
        ("shared/src/lib.rs", "pub fn value() -> u8 { 21 }\n"),
        (
            "leaf/Cargo.toml",
            "[package]\nname = \"leaf\"\nversion = \"0.1.0\"\n",
        ),
        ("leaf/src/lib.rs", ""),
    ] {
        let path = root.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    root
}

fn once(root: &std::path::Path, color: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_once"))
        .current_dir(root)
        .args(["--format", "json", "--cpu-limit", "1"])
        .args(args)
        .env("CARGO_TERM_COLOR", color)
        .env("CARGO", env!("CARGO"))
        .env("ONCE_CACHE_PROVIDER", "local")
        .env("ONCE_CHANGE_TRACKER", "0")
        .output()
        .unwrap()
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cargo_feature_resolution_is_independent_of_terminal_color() {
    let root = workspace();
    let mut features = Vec::new();
    for color in ["always", "never"] {
        let output = once(root.path(), color, &["query", "target", "cargo_shared"]);
        success(&output);
        let target: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        features.push(target["attrs"]["features"].clone());
    }
    assert_eq!(features[0], serde_json::json!(["default", "extra"]));
    assert_eq!(features[0], features[1]);
}

#[test]
fn cargo_proc_macro_unit_tests_can_import_the_compiler_builtin() {
    let root = workspace();
    let output = once(
        root.path(),
        "never",
        &["test", "cargo_derive_demo_unit_tests"],
    );
    success(&output);
    let output = once(
        root.path(),
        "never",
        &["query", "test-results", "cargo_derive_demo_unit_tests"],
    );
    success(&output);
    let results: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(results["summary"]["passed"], 1);

    let source = root.path().join("src/lib.rs");
    let original = std::fs::read_to_string(&source).unwrap();
    std::fs::write(
        source,
        format!("{original}\n#[test]\nfn changed_test_fails() {{ panic!(\"invalidation control\"); }}\n"),
    )
    .unwrap();
    let output = once(
        root.path(),
        "never",
        &["test", "cargo_derive_demo_unit_tests"],
    );
    assert!(!output.status.success());
    let output = once(
        root.path(),
        "never",
        &["query", "test-results", "cargo_derive_demo_unit_tests"],
    );
    success(&output);
    let results: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(results["summary"]["passed"], 1);
    assert_eq!(results["summary"]["failed"], 1);
}

#[test]
fn cargo_workspace_can_explicitly_run_host_dependent_tests_without_caching() {
    let root = workspace();
    std::fs::write(
        root.path().join("once.toml"),
        r#"[[target]]
name = "cargo"
kind = "cargo_workspace"
srcs = ["Cargo.toml"]
[target.attrs]
test_env = { ONCE_TEST_VALUE = "declared" }
test_env_inherit = ["CARGO"]
test_cacheable = false
"#,
    )
    .unwrap();
    let source = root.path().join("src/lib.rs");
    let original = std::fs::read_to_string(&source).unwrap();
    std::fs::write(
        source,
        format!(
            r#"{original}
#[test]
fn host_dependent_test() {{
    assert_eq!(std::env::var("ONCE_TEST_VALUE").unwrap(), "declared");
    let cargo = std::env::var_os("CARGO").unwrap();
    assert!(std::process::Command::new(cargo).arg("--version").status().unwrap().success());
}}
"#
        ),
    )
    .unwrap();
    for _ in 0..2 {
        let output = once(
            root.path(),
            "never",
            &["test", "cargo_derive_demo_unit_tests"],
        );
        success(&output);
        assert!(String::from_utf8_lossy(&output.stdout).contains("\"cache\":\"bypass\""));
    }
    let output = once(
        root.path(),
        "never",
        &["query", "test-results", "cargo_derive_demo_unit_tests"],
    );
    success(&output);
    let results: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(results["summary"]["passed"], 2);
}
