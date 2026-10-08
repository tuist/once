use super::*;

fn evaluate(body: &str) -> String {
    eval_prelude_source_to_repr(format!("{}\n{}", all_prelude_source(), body)).unwrap()
}

#[test]
fn zig_uses_effective_optimize_and_never_invents_native_platform_or_mode() {
    assert_eq!(
        evaluate(
            r#"
def metadata(attrs):
    return _zig_action_metadata({"attr": attrs, "label": {"id": "pkg/lib"}})
result = repr([
    metadata({"optimize": "ReleaseFast"})["context"][0]["value"],
    metadata({"mode": "release_small"})["context"][0]["value"],
    metadata({"host_mode": "release_safe"})["context"][0]["value"],
    metadata({})["context"],
    metadata({"target": "native"})["platforms"],
    metadata({"host_target": "aarch64-linux-gnu"})["platforms"][0]["id"],
])
"#
        ),
        r#"["ReleaseFast", "ReleaseSmall", "ReleaseSafe", [], [], "aarch64-linux-gnu"]"#
    );
}

#[test]
fn rust_package_origin_and_host_tool_roles_are_owned_facts() {
    assert_eq!(
        evaluate(
            r#"
ctx = {"attr": {"package_name": "syn", "version": "2.0", "source": "sparse+https://private.invalid/index"}, "label": {"id": "pkg/syn-host", "name": "syn-host"}}
host = "x86_64-unknown-linux-gnu"
roles = [_rust_platform_usage(ctx, "", host, "rlib"), _rust_platform_usage(ctx, host, host, "rlib"), _rust_platform_usage(ctx, "aarch64-linux-android", host, "rlib"), _rust_platform_usage(ctx, "", host, "proc-macro")]
ctx["attr"]["_cargo_host_tool"] = True
roles.append(_rust_platform_usage(ctx, "", host, "rlib"))
result = repr([_rust_action_metadata(ctx, host)["package"]["origin"], roles])
"#
        ),
        r#"["registry", ["", "", "product", "build-tool", "build-tool"]]"#
    );
}

#[test]
fn swift_android_api_comes_from_effective_triple() {
    assert_eq!(
        evaluate(
            r#"
target = _swift_android_target_with_api("aarch64-unknown-linux-android24", 28)
result = repr(_swift_android_presentation(target, "arm64-v8a")["context"][0]["value"])
"#
        ),
        r#""24""#
    );
}

#[test]
fn apple_native_platform_labels_handle_aliases_and_catalyst() {
    assert_eq!(
        evaluate(
            r#"
result = repr([
    _apple_action_metadata("arm64-apple-ios17.0-macabi", "macos", "simulator", "arm64")["platforms"][0]["label"],
    _apple_action_metadata("arm64-apple-macosx14.0", "macosx", "simulator", "arm64")["platforms"][0]["label"],
    _apple_action_metadata("arm64-apple-xros2.0-simulator", "xros", "simulator", "arm64")["platforms"][0]["label"],
])
"#
        ),
        r#"["Mac Catalyst \xb7 arm64", "macOS \xb7 arm64", "visionOS Simulator \xb7 arm64"]"#
    );
}

#[test]
fn go_pure_off_means_cgo_is_enabled() {
    assert_eq!(
        evaluate(
            r#"
result = repr(_go_cgo_enabled({"attr": {"pure": "off"}, "label": {"id": "pkg/lib"}}))
"#
        ),
        "True"
    );
}
