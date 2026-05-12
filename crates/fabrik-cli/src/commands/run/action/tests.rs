use super::*;
use fabrik_core::Action;
use fabrik_frontend::Target;
use tempfile::TempDir;

fn target(package: &str, kind: &str, srcs: &[&str]) -> Target {
    Target {
        package: package.into(),
        kind: kind.into(),
        name: "demo".into(),
        srcs: srcs.iter().map(|s| (*s).into()).collect(),
        deps: vec![],
        attrs: BTreeMap::new(),
    }
}

#[test]
fn source_path_joins_package_and_rejects_escapes() {
    let t = target("crates/foo", "rust_binary", &["src/main.rs"]);
    assert_eq!(
        source_path(&t, "src/main.rs").unwrap().as_str(),
        "crates/foo/src/main.rs"
    );
    let root = target("", "rust_binary", &["main.rs"]);
    assert_eq!(source_path(&root, "main.rs").unwrap().as_str(), "main.rs");

    let escape = target("pkg", "rust_binary", &["../escape.rs"]);
    assert!(source_path(&escape, "../escape.rs").is_err());
}

#[test]
fn input_digest_is_none_when_no_srcs_are_declared() {
    let tmp = TempDir::new().unwrap();
    let t = target("", "rust_binary", &[]);
    assert!(input_digest(tmp.path(), &t).unwrap().is_none());
}

#[test]
fn input_digest_changes_with_content_and_is_stable_across_runs() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("pkg")).unwrap();
    std::fs::write(tmp.path().join("pkg/main.rs"), b"fn main() {}").unwrap();
    let t = target("pkg", "rust_binary", &["main.rs"]);

    let first = input_digest(tmp.path(), &t).unwrap().unwrap();
    let second = input_digest(tmp.path(), &t).unwrap().unwrap();
    assert_eq!(first, second, "stable for identical content");

    std::fs::write(tmp.path().join("pkg/main.rs"), b"fn main() { /*!*/ }").unwrap();
    let third = input_digest(tmp.path(), &t).unwrap().unwrap();
    assert_ne!(first, third, "content change must invalidate the digest");
}

#[test]
fn input_digest_is_independent_of_declared_src_order() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("pkg")).unwrap();
    std::fs::write(tmp.path().join("pkg/a.rs"), b"a").unwrap();
    std::fs::write(tmp.path().join("pkg/b.rs"), b"b").unwrap();
    let forward = target("pkg", "rust_binary", &["a.rs", "b.rs"]);
    let reversed = target("pkg", "rust_binary", &["b.rs", "a.rs"]);
    assert_eq!(
        input_digest(tmp.path(), &forward).unwrap().unwrap(),
        input_digest(tmp.path(), &reversed).unwrap().unwrap()
    );
}

#[test]
fn rust_binary_action_has_expected_argv_and_output() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("pkg")).unwrap();
    std::fs::write(tmp.path().join("pkg/main.rs"), b"fn main() {}").unwrap();
    let t = target("pkg", "rust_binary", &["main.rs"]);
    let plan = rust_binary_action(tmp.path(), &t).unwrap();

    assert_eq!(plan.output, format!("{CACHE_DIR}/out/pkg/demo"));
    assert_eq!(
        plan.output_dir.as_deref(),
        Some(tmp.path().join(CACHE_DIR).join("out").join("pkg")).as_deref()
    );
    match plan.action {
        Action::RunCommand {
            argv,
            input_digest,
            cwd,
            ..
        } => {
            assert_eq!(argv[0], "rustc");
            assert!(argv.contains(&"--crate-type=bin".to_string()));
            assert!(argv.iter().any(|a| a == "--crate-name=demo"));
            assert_eq!(argv.last().map(String::as_str), Some("pkg/main.rs"));
            assert!(input_digest.is_some(), "rust_binary must hash its inputs");
            assert!(cwd.is_none(), "rust_binary runs at the workspace root");
        }
    }
}

#[test]
fn rust_binary_action_root_package_drops_separator() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("main.rs"), b"fn main() {}").unwrap();
    let t = target("", "rust_binary", &["main.rs"]);
    let plan = rust_binary_action(tmp.path(), &t).unwrap();
    assert_eq!(plan.output, format!("{CACHE_DIR}/out/demo"));
}

#[test]
fn cargo_binary_action_uses_attrs_and_falls_back_to_name_for_bin() {
    let mut t = target("", "cargo_binary", &["Cargo.toml"]);
    t.attrs.insert("cargo_package".into(), "fabrik-cli".into());
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("Cargo.toml"), b"[package]\nname=\"x\"\n").unwrap();
    let plan = cargo_binary_action(tmp.path(), &t).unwrap();
    match plan.action {
        Action::RunCommand { argv, .. } => {
            assert_eq!(argv[0], "cargo");
            assert!(argv.contains(&"--locked".to_string()));
            assert!(argv
                .windows(2)
                .any(|w| w[0] == "--package" && w[1] == "fabrik-cli"));
            assert!(argv.windows(2).any(|w| w[0] == "--bin" && w[1] == "demo"));
        }
    }
    assert!(plan.output_dir.is_none());
}

#[test]
fn cargo_binary_action_requires_cargo_package_attr() {
    let t = target("", "cargo_binary", &["Cargo.toml"]);
    let tmp = TempDir::new().unwrap();
    let err = cargo_binary_action(tmp.path(), &t)
        .err()
        .expect("missing cargo_package must error")
        .to_string();
    assert!(err.contains("cargo_package"), "got {err}");
}

#[test]
fn action_for_rejects_unsupported_kinds() {
    let tmp = TempDir::new().unwrap();
    let t = target("", "rust_library", &["lib.rs"]);
    let err = action_for(tmp.path(), &t)
        .err()
        .expect("rust_library is unsupported")
        .to_string();
    assert!(err.contains("rust_library"), "got {err}");
}
