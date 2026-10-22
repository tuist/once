use super::*;

fn dependency_workspace() -> TempDir {
    let workspace = TempDir::new().unwrap();
    let app = workspace.path().join("app");
    let dependency = workspace.path().join("dependency");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::create_dir_all(dependency.join("src")).unwrap();
    std::fs::write(
        app.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\ndemo = { path = \"../dependency\" }\n",
    ).unwrap();
    std::fs::write(app.join("src/main.rs"), "fn main() { demo::value(); }\n").unwrap();
    std::fs::write(
        app.join("Cargo.lock"),
        "version = 3\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"demo\"]\n[[package]]\nname = \"demo\"\nversion = \"1.0.0\"\n",
    ).unwrap();
    std::fs::write(
        dependency.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(dependency.join("src/lib.rs"), "pub fn value() {}\n").unwrap();
    std::fs::write(dependency.join("build.rs"), "fn main() {}\n").unwrap();
    workspace
}

fn write_dependencies_owner(app: &Path, srcs: &str, attrs: &str) {
    std::fs::write(
        app.join("once.toml"),
        format!("[workspace]\ninclude = [\"once.toml\"]\n[[target]]\nname = \"deps\"\nkind = \"cargo_dependencies\"\nsrcs = [{srcs}]\n[target.attrs]\n{attrs}"),
    ).unwrap();
}

#[test]
fn cargo_dependencies_vendor_directory_is_opt_in() {
    let schema = built_in_target_kind_schema("cargo_dependencies").unwrap();
    let vendor = schema
        .attrs
        .iter()
        .find(|attr| attr.name == "vendor_dir")
        .unwrap();
    assert!(vendor.default.is_none());
}

#[test]
fn cargo_dependencies_materializes_sources_without_preparation() {
    let workspace = dependency_workspace();
    let app = workspace.path().join("app");
    let dependency = workspace.path().join("dependency");
    for vendor_attr in ["", "vendor_dir = \"\"\n"] {
        write_dependencies_owner(&app, "\"Cargo.toml\", \"Cargo.lock\"", vendor_attr);
        let graph = once_frontend::load_graph_workspace(&app).unwrap();
        let owner = graph
            .iter()
            .find(|target| target.label.id == "deps")
            .unwrap();
        let target = graph
            .iter()
            .find(|target| target.label.id == "demo-1.0.0")
            .unwrap();
        assert_eq!(owner.deps, vec!["demo-1.0.0"]);
        assert!(target.srcs.is_empty());
        assert_eq!(
            target.attrs.get("_cargo_source_root"),
            Some(&once_frontend::AttrValue::String(
                dependency.to_string_lossy().into_owned()
            )),
        );
        assert_eq!(
            target.attrs.get("_cargo_materialized_source_root"),
            Some(&once_frontend::AttrValue::String(
                ".once/out/demo-1.0.0/source".into()
            )),
        );
        assert_eq!(
            target.attrs.get("build_script"),
            Some(&once_frontend::AttrValue::String(
                ".once/out/demo-1.0.0/source/build.rs".into()
            )),
        );
        assert!(!app.join("third_party").exists());
        assert!(!app.join(".once/out/demo-1.0.0/source").exists());
    }
}

#[test]
fn cargo_dependencies_requires_a_declared_lockfile_without_mutating_it() {
    let workspace = dependency_workspace();
    let app = workspace.path().join("app");
    let lock = app.join("Cargo.lock");
    let original = std::fs::read(&lock).unwrap();
    write_dependencies_owner(&app, "\"Cargo.toml\"", "");
    let diagnostics = once_frontend::validate_workspace(&app).unwrap();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.attribute.as_deref() == Some("lockfile"))
        .unwrap();
    assert_eq!(diagnostic.code, "missing_source");
    assert!(diagnostic
        .message
        .contains("must be included in resolver_inputs"));
    assert!(diagnostic
        .repairs
        .iter()
        .any(|repair| repair.contains("Include the existing lockfile")));
    assert_eq!(std::fs::read(&lock).unwrap(), original);

    std::fs::remove_file(&lock).unwrap();
    write_dependencies_owner(&app, "\"Cargo.toml\", \"Cargo.lock\"", "");
    let diagnostics = once_frontend::validate_workspace(&app).unwrap();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.attribute.as_deref() == Some("lockfile"))
        .unwrap();
    assert_eq!(diagnostic.code, "missing_source");
    assert_eq!(
        diagnostic.message,
        "Cargo lockfile `Cargo.lock` does not exist"
    );
    assert!(diagnostic
        .repairs
        .iter()
        .any(|repair| repair.contains("cargo generate-lockfile") && repair.contains("commit")));
    assert!(!lock.exists());
}

#[test]
fn cargo_dependencies_rejects_a_stale_lockfile_without_rewriting_it() {
    let workspace = dependency_workspace();
    let app = workspace.path().join("app");
    let lock = app.join("Cargo.lock");
    let original = std::fs::read(&lock).unwrap();
    std::fs::write(
        workspace.path().join("dependency/Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"1.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    write_dependencies_owner(&app, "\"Cargo.toml\", \"Cargo.lock\"", "");
    let diagnostics = once_frontend::validate_workspace(&app).unwrap();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("--locked")),
        "{diagnostics:?}"
    );
    assert_eq!(std::fs::read(&lock).unwrap(), original);
}

#[test]
fn cargo_dependencies_snapshot_requires_explicit_vendor_sources() {
    let workspace = dependency_workspace();
    let app = workspace.path().join("app");
    std::fs::write(app.join("metadata.json"), "{}\n").unwrap();
    for vendor_attr in ["", "vendor_dir = \"\"\n"] {
        write_dependencies_owner(
            &app,
            "\"Cargo.toml\", \"Cargo.lock\", \"metadata.json\"",
            &format!("metadata_file = \"metadata.json\"\n{vendor_attr}"),
        );
        let diagnostics = once_frontend::validate_workspace(&app).unwrap();
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.attribute.as_deref() == Some("vendor_dir"))
            .unwrap();
        assert_eq!(diagnostic.code, "missing_required_attr");
        assert!(diagnostic.message.contains("snapshot host paths"));
        assert!(diagnostic
            .repairs
            .iter()
            .any(|repair| repair.contains("third_party/rust/vendor")
                && repair.contains("remove metadata_file")));
    }
}

#[test]
fn cargo_dependencies_checks_lockfiles_relative_to_nested_owners() {
    let workspace = dependency_workspace();
    let app = workspace.path().join("app");
    let original = std::fs::read(app.join("Cargo.lock")).unwrap();
    std::fs::write(
        workspace.path().join("once.toml"),
        "[workspace]\ninclude = [\"app/once.toml\"]\n",
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("Cargo.lock"),
        "unrelated workspace lockfile\n",
    )
    .unwrap();
    for lockfile in ["Cargo.lock", "./Cargo.lock", ".cargo/Cargo.lock"] {
        let owner_lock = app.join(lockfile.trim_start_matches("./"));
        std::fs::create_dir_all(owner_lock.parent().unwrap()).unwrap();
        std::fs::write(&owner_lock, &original).unwrap();
        write_dependencies_owner(
            &app,
            "\"Cargo.toml\"",
            &format!("lockfile = \"{lockfile}\"\n"),
        );
        let diagnostics = once_frontend::validate_workspace(workspace.path()).unwrap();
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.attribute.as_deref() == Some("lockfile"))
            .unwrap();
        assert_eq!(diagnostic.target.as_deref(), Some("app/deps"));
        assert!(
            diagnostic
                .message
                .contains("must be included in resolver_inputs"),
            "{diagnostic:?}"
        );

        std::fs::remove_file(&owner_lock).unwrap();
        let diagnostics = once_frontend::validate_workspace(workspace.path()).unwrap();
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.attribute.as_deref() == Some("lockfile"))
            .unwrap();
        assert!(
            diagnostic.message.contains("does not exist"),
            "{diagnostic:?}"
        );
    }
}

#[test]
fn cargo_workspace_still_bootstraps_a_missing_lockfile() {
    let workspace = dependency_workspace();
    let app = workspace.path().join("app");
    std::fs::remove_file(app.join("Cargo.lock")).unwrap();
    let graph = once_frontend::load_graph_workspace(&app).unwrap();
    assert!(graph.iter().any(|target| target.kind == "cargo_workspace"));
    assert!(app.join("Cargo.lock").exists());
}
