use super::*;

fn workspace_library(workspace: &str, owner: &str, package_root: &str) -> Result<String, String> {
    let source = format!(
        r#"{}
package = {{
    "id": "app", "name": "app", "version": "0.1.0", "edition": "2024",
    "manifest_path": {manifest:?},
    "targets": [{{"name": "app", "kind": ["lib"], "src_path": {src:?}}}],
}}
def _cargo_resolved_metadata(ctx, default_vendor_dir, materialize_sources):
    return {{"metadata": {{"packages": [package], "workspace_members": ["app"]}}, "specs": [], "split_host": False}}

ctx = {{
    "label": {{"package": {owner:?}, "name": "cargo", "id": "cargo"}},
    "attr": {{}}, "files": {{"Cargo.toml": "[workspace]"}},
}}
target = _cargo_workspace_resolver(ctx)["targets"][0]
result = repr([target["srcs"], target["attrs"]["crate_root"]])
"#,
        all_prelude_source(),
        manifest = format!("{package_root}/Cargo.toml"),
        src = format!("{package_root}/src/lib.rs"),
    );
    let store = store_for(Path::new(workspace), owner);
    with_active_store(store, || eval_prelude_source_to_repr(source)).1
}

#[test]
fn cargo_workspace_accepts_windows_verbatim_and_metadata_path_spellings() {
    let drive = r"D:\a\_temp\desktop-package\source";
    let verbatim_drive = r"\\?\D:\a\_temp\desktop-package\source";
    let unc = r"\\server\share\desktop-package\source";
    let verbatim_unc = r"\\?\UNC\server\share\desktop-package\source";
    let long_member = format!("crates/{}", "member".repeat(50));
    for (workspace, metadata_root, owner, member) in [
        (drive, drive, "", "crates/credentials"),
        (verbatim_drive, drive, "", "crates/credentials"),
        (drive, verbatim_drive, "", "crates/credentials"),
        (verbatim_drive, verbatim_drive, "", "crates/credentials"),
        (unc, unc, "", "crates/credentials"),
        (verbatim_unc, unc, "", "crates/credentials"),
        (unc, verbatim_unc, "", "crates/credentials"),
        (verbatim_unc, verbatim_unc, "", "crates/credentials"),
        (verbatim_drive, drive, "apps/desktop", ""),
        (verbatim_unc, unc, "apps/desktop", long_member.as_str()),
        (verbatim_drive, drive, "", ""),
        ("/workspace", "/workspace", "apps/desktop", "member"),
    ] {
        let project_root = if owner.is_empty() {
            metadata_root.to_string()
        } else {
            format!("{metadata_root}/{owner}")
        };
        let package_root = if member.is_empty() {
            project_root
        } else {
            format!("{project_root}/{member}")
        };
        let root = if member.is_empty() { "." } else { member };
        let crate_root = if member.is_empty() {
            "src/lib.rs".to_string()
        } else {
            format!("{member}/src/lib.rs")
        };
        assert_eq!(
            workspace_library(workspace, owner, &package_root).unwrap(),
            format!(r#"[["{root}/**/*"], "{crate_root}"]"#),
            "workspace={workspace}, owner={owner}, package={package_root}",
        );
    }
}

#[test]
fn cargo_materialized_sources_keep_verbatim_paths_but_compare_source_spellings() {
    for (root, source_root) in [
        (r"\\?\D:\cargo", "D:/cargo"),
        (r"\\?\UNC\server\share\cargo", "//server/share/cargo"),
    ] {
        let member = "long-package/".repeat(25);
        let source = format!(
            r#"{}
package = {{
    "id": "dependency", "name": "dependency", "version": "1.0.0",
    "source": "registry+https://github.com/rust-lang/crates.io-index",
    "manifest_path": {manifest:?},
    "targets": [{{"name": "dependency", "kind": ["lib"], "src_path": {src:?}}}],
}}
ctx = {{"label": {{"package": "", "id": "cargo"}}, "attrs": {{"split_host_variants": False}}}}
metadata = {{"packages": [package], "workspace_members": ["app"]}}
attrs = _cargo_metadata_resolution(ctx, metadata, materialize_sources = True)["specs"][0]["attrs"]
result = repr([attrs["_cargo_source_root"], attrs["crate_root"]])
"#,
            all_prelude_source(),
            manifest = format!("{root}/{member}Cargo.toml"),
            src = format!("{source_root}/{member}lib/entry.rs"),
        );
        let preserved_root = format!("{root}/{member}").replace('\\', "/");
        let preserved_root = preserved_root.trim_end_matches('/');
        assert_eq!(
            eval_prelude_source_to_repr(source).unwrap(),
            format!(r#"["{preserved_root}", ".once/out/dependency-1.0.0/source/lib/entry.rs"]"#),
        );
    }
}

#[test]
fn cargo_workspace_loads_path_dependencies_from_a_canonical_root() {
    let workspace = TempDir::new().unwrap();
    for (path, contents) in [
        ("Cargo.toml", "[workspace]\nmembers = ['crates/*']\nresolver = '2'\n"),
        ("crates/app/Cargo.toml", "[package]\nname = 'app'\nversion = '0.1.0'\n[dependencies]\ncredentials = { path = '../credentials' }\n"),
        ("crates/credentials/Cargo.toml", "[package]\nname = 'credentials'\nversion = '0.1.0'\n"),
        ("crates/app/src/lib.rs", "pub fn answer() -> u32 { credentials::answer() }\n"),
        ("crates/credentials/src/lib.rs", "pub fn answer() -> u32 { 42 }\n"),
        ("once.toml", "[workspace]\ninclude = ['once.toml']\nexclude = ['**/Cargo.toml']\n[[target]]\nname = 'Desktop'\nkind = 'cargo_workspace'\nsrcs = ['Cargo.toml', 'Cargo.lock', 'crates/*/Cargo.toml', 'crates/*/src/**/*.rs']\n"),
    ] {
        let path = workspace.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    let output = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(workspace.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let root = workspace.path().canonicalize().unwrap();
    let graph = once_frontend::load_graph_workspace(&root).unwrap();
    let app = graph
        .iter()
        .find(|target| target.label.id == "cargo_app")
        .unwrap();
    assert_eq!(app.deps, ["cargo_credentials"]);
    let credentials = graph
        .iter()
        .find(|target| target.label.id == "cargo_credentials")
        .unwrap();
    assert_eq!(credentials.srcs, ["crates/credentials/**/*"]);
}

#[test]
fn cargo_workspace_still_rejects_packages_outside_the_project() {
    for (workspace, owner, package_root) in [
        (r"\\?\D:\source", "", "D:/source-other/crate"),
        (r"\\?\D:\source", "apps/desktop", "D:/source/crates/app"),
        (r"\\?\D:\source", "", "E:/source/crate"),
        (
            r"\\?\UNC\server\share\source",
            "",
            "//other/share/source/crate",
        ),
        (
            r"\\?\UNC\server\share\source",
            "",
            "//server/other/source/crate",
        ),
        (r"\\?\Volume{abc}\source", "", "Volume{abc}/source/crate"),
        ("/workspace", "", "/Workspace/crate"),
    ] {
        let error = workspace_library(workspace, owner, package_root).unwrap_err();
        assert!(
            error.contains("outside the adapted project root"),
            "{error}"
        );
    }
}

#[test]
fn cargo_first_party_roots_stay_relative_to_the_owning_package() {
    let source = format!(
        r#"{}
package = {{
    "name": "app", "version": "0.1.0", "edition": "2024",
    "manifest_path": "/workspace/apps/desktop/Cargo.toml",
    "targets": [{{"kind": ["custom-build"], "src_path": "/workspace/apps/desktop/build.rs"}}],
}}
target = {{"name": "app", "kind": ["bin"], "src_path": "/workspace/apps/desktop/src/main.rs"}}
def check_roots():
    results = []
    for owner in ["", "apps/desktop"]:
        for root in [".", "member"]:
            attrs = _cargo_workspace_attrs(package, target, {{"features": []}}, root, {{}})
            ctx = {{"label": {{"package": owner, "id": "app"}}, "attr": attrs}}
            results.append([
                attrs["crate_root"],
                _rust_crate_root(ctx, "src/main.rs"),
                _package_relative(ctx, attrs["build_script"]),
            ])
    return results
result = repr(check_roots())
"#,
        all_prelude_source()
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["src/main.rs", "src/main.rs", "build.rs"], ["member/src/main.rs", "member/src/main.rs", "member/build.rs"], ["src/main.rs", "apps/desktop/src/main.rs", "apps/desktop/build.rs"], ["member/src/main.rs", "apps/desktop/member/src/main.rs", "apps/desktop/member/build.rs"]]"#
    );
}
