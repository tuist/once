use super::*;

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
