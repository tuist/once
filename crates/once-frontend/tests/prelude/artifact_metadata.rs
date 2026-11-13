use super::*;

const ICON: &str = ".once/out/app/artifact/icon.png";
const METADATA: &str = ".once/out/app/artifact/metadata.json";

fn plan(prelude: &str, body: &str) -> AnalysisStore {
    let workspace = tempfile::tempdir().unwrap();
    let (store, result) = with_active_store(store_for(workspace.path(), "app"), || {
        eval_prelude_source_to_repr(format!("{prelude}\n{body}\nresult = repr(None)\n"))
    });
    result.unwrap();
    store
}

fn written(store: &AnalysisStore, path: &str) -> Vec<u8> {
    store
        .actions
        .iter()
        .find_map(|action| match &action.operation {
            Some(DeclaredActionOperation::WriteFile {
                path: written,
                bytes,
            }) if written == path => Some(bytes.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("`{path}` was not written"))
}

fn copied_to_icon(store: &AnalysisStore) -> Vec<String> {
    store
        .actions
        .iter()
        .find_map(|action| match &action.operation {
            Some(DeclaredActionOperation::CopyPath {
                sources,
                destination,
                ..
            }) if destination == ICON => Some(sources.clone()),
            _ => None,
        })
        .expect("the icon was not copied")
}

fn metadata(store: &AnalysisStore) -> serde_json::Value {
    serde_json::from_slice(&written(store, METADATA)).unwrap()
}

fn android_plan(badging: &str, listing: &str) -> AnalysisStore {
    plan(
        &android_prelude_source(),
        &format!(
            r#"
def host_command(argv, env = None, cwd = None, merge_stderr = False, check = True, max_output_bytes = None):
    if argv[0] == "/jdk/bin/jar":
        return {listing:?}
    return {badging:?}
def workspace_root():
    return "/workspace"
android_artifact_metadata_plan({{"args": {{
    "apk": ".once/out/app/App.apk", "metadata": "{METADATA}", "icon": "{ICON}", "icon_staging": ".once/out/app/artifact/icon",
    "aapt2": "aapt2", "apksigner": "apksigner", "jar": "/jdk/bin/jar", "env": {{}}, "toolchain_identity": "sdk",
    "signing": "none", "version_name": "0.1.0",
}}}})
"#
        ),
    )
}

#[test]
fn android_metadata_uses_the_default_label_and_the_densest_png_icon_within_one_mebibyte() {
    let store = android_plan(
        "package: name='dev.tuist.code' versionCode='7' versionName='0.1.0'\n\
         minSdkVersion:'26'\n\
         targetSdkVersion:'35'\n\
         application-label:'Tuist Code'\n\
         application-label-de:'Tuist Code (DE)'\n\
         application-icon-120:'res/drawable-ldpi-v4/app_icon.png'\n\
         application-icon-640:'res/drawable-xxxhdpi-v4/app_icon.png'\n\
         application-icon-480:'res/drawable-xxhdpi-v4/app_icon.png'\n\
         application-icon-65534:'res/mipmap-anydpi-v26/ic_launcher.xml'\n\
         application: label='Tuist Code' icon='res/mipmap-anydpi-v26/ic_launcher.xml'\n",
        "  2048 Tue Jan 01 00:00:00 UTC 1980 AndroidManifest.xml\n\
         4096 Tue Jan 01 00:00:00 UTC 1980 res/drawable-ldpi-v4/app_icon.png\n\
         2097152 Tue Jan 01 00:00:00 UTC 1980 res/drawable-xxxhdpi-v4/app_icon.png\n\
         65536 Tue Jan 01 00:00:00 UTC 1980 res/drawable-xxhdpi-v4/app_icon.png\n",
    );
    let metadata = metadata(&store);
    assert_eq!(metadata["display_name"], "Tuist Code");
    assert_eq!(metadata["application_id"], "dev.tuist.code");
    assert_eq!(metadata["minimum_sdk"], serde_json::json!(["26"]));
    let extracted = ".once/out/app/artifact/icon/res/drawable-xxhdpi-v4/app_icon.png";
    let extract = action_by_identifier(&store, &format!("android_artifact_icon_extract:{ICON}"));
    assert_eq!(
        extract.argv,
        [
            "/jdk/bin/jar",
            "--extract",
            "--file",
            "../../../../../.once/out/app/App.apk",
            "res/drawable-xxhdpi-v4/app_icon.png"
        ]
    );
    assert_eq!(extract.cwd.as_deref(), Some(".once/out/app/artifact/icon"));
    assert_eq!(extract.outputs, [extracted]);
    assert_eq!(copied_to_icon(&store), [extracted]);
}

#[test]
fn android_metadata_without_a_label_or_png_icon_leaves_the_icon_empty() {
    let store = android_plan(
        "package: name='dev.once.hello' versionCode='1' versionName='1.0'\n\
         sdkVersion:'24'\n\
         application-icon-65534:'res/mipmap-anydpi-v26/ic_launcher.xml'\n\
         application: label='' icon='res/mipmap-anydpi-v26/ic_launcher.xml'\n",
        "",
    );
    assert_eq!(metadata(&store)["display_name"], "");
    assert_eq!(metadata(&store)["minimum_sdk"], serde_json::json!(["24"]));
    assert!(written(&store, ICON).is_empty());
    assert_eq!(store.actions.len(), 2);
}

struct Apple<'a> {
    platform: &'a str,
    plist: &'a str,
    /// Declared planner inputs, as a Starlark list.
    inputs: &'a str,
    /// Asset catalog icon-set PNGs, as a Starlark list.
    icon_set: &'a str,
    /// Files on disk mapped to their square pixel size, as a Starlark dict.
    sizes: &'a str,
    /// Files on disk that are `CgBI` PNGs, as a Starlark list.
    cgbi: &'a str,
}

fn apple_plan(apple: &Apple<'_>) -> AnalysisStore {
    let Apple {
        platform,
        plist,
        inputs,
        icon_set,
        sizes,
        cgbi,
    } = apple;
    plan(
        &apple_prelude_source(),
        &format!(
            r#"
SIZES = {sizes}
def workspace_root():
    return "/workspace"
def host_which(name):
    return "/usr/bin/" + name
def host_read_dir(path):
    fail("icon selection must not list directories: " + path)
def host_file_exists(path):
    return path[len("/workspace/"):] in SIZES
def host_file_size(path):
    return 4096
def host_file_contains(path, needle):
    return needle == "CgBI" and path[len("/workspace/"):] in {cgbi}
def host_command(argv, env = None, cwd = None, merge_stderr = False, check = True, max_output_bytes = None):
    if argv[0] == "/usr/bin/plutil":
        return {plist:?}
    if argv[0] == "/usr/bin/xcrun":
        return "UUID: 0B4C0C70-0000-0000-0000-000000000000 (arm64) out/App"
    if argv[0] == "/usr/bin/sips":
        size = SIZES[argv[-1][len("/workspace/"):]]
        return argv[-1] + "\n  pixelWidth: %d\n  pixelHeight: %d\n" % (size, size)
    return ""
apple_artifact_metadata_plan({{"inputs": {inputs}, "args": {{
    "application": "out/App.app", "executable": "out/App", "info_plist": "out/Info.plist", "metadata": "{METADATA}", "icon": "{ICON}", "icon_set": {icon_set},
    "platform": "{platform}", "sdk_variant": "simulator", "developer_dir": "", "signing": "ad_hoc",
}}}})
"#
        ),
    )
}

fn converted(store: &AnalysisStore) -> &DeclaredAction {
    action_by_identifier(store, &format!("apple_artifact_icon:{ICON}"))
}

const IOS_PLIST: &str = r#"{"CFBundleDisplayName": "Tuist Code", "CFBundleIcons": {"CFBundlePrimaryIcon": {"CFBundleIconFiles": ["AppIcon60x60"]}}, "CFBundleIcons~ipad": {"CFBundlePrimaryIcon": {"CFBundleIconFiles": ["AppIcon60x60", "AppIcon76x76.png"]}}}"#;
const IOS_INPUTS: &str = r#"["out/App.app/AppIcon60x60@2x.png", "out/App.app/AppIcon76x76@2x~ipad.png", "out/App.app/Assets.car", "out/App.app/LaunchImage@3x.png", "out/App.app/Nested/AppIcon60x60@3x.png", "out/Info.plist"]"#;
// `AppIcon60x60@3x.png` is a leftover from an earlier build: it is on disk and
// larger than every declared icon, but no action declares it.
const IOS_SIZES: &str = r#"{"out/App.app/AppIcon60x60@2x.png": 120, "out/App.app/AppIcon76x76@2x~ipad.png": 152, "out/App.app/LaunchImage@3x.png": 1242, "out/App.app/AppIcon60x60@3x.png": 180, "out/App.app/Nested/AppIcon60x60@3x.png": 180}"#;

#[test]
fn apple_simulator_icon_is_the_largest_declared_png_named_by_bundle_icons() {
    let store = apple_plan(&Apple {
        platform: "ios",
        plist: IOS_PLIST,
        inputs: IOS_INPUTS,
        icon_set: "[]",
        sizes: IOS_SIZES,
        cgbi: "[]",
    });
    assert_eq!(metadata(&store)["display_name"], "Tuist Code");
    assert_eq!(
        converted(&store).argv,
        [
            "/usr/bin/sips",
            "-s",
            "format",
            "png",
            "out/App.app/AppIcon76x76@2x~ipad.png",
            "--out",
            ICON
        ]
    );
    assert_eq!(
        converted(&store).inputs,
        ["out/App.app/AppIcon76x76@2x~ipad.png"]
    );
}

#[test]
fn apple_icon_skips_cgbi_pngs() {
    let store = apple_plan(&Apple {
        platform: "ios",
        plist: IOS_PLIST,
        inputs: IOS_INPUTS,
        icon_set: "[]",
        sizes: IOS_SIZES,
        cgbi: r#"["out/App.app/AppIcon76x76@2x~ipad.png"]"#,
    });
    assert_eq!(
        converted(&store).inputs,
        ["out/App.app/AppIcon60x60@2x.png"]
    );
}

#[test]
fn apple_macos_icon_converts_the_declared_bundle_icon_file_and_bounds_its_size() {
    let store = apple_plan(&Apple {
        platform: "macos",
        plist: r#"{"CFBundleName": "App", "CFBundleIconFile": "AppIcon"}"#,
        inputs: r#"["out/App.app/Contents/Resources/AppIcon.icns"]"#,
        icon_set: r#"["Assets.xcassets/AppIcon.appiconset/icon_1024.png"]"#,
        sizes: r#"{"out/App.app/Contents/Resources/AppIcon.icns": 1024, "Assets.xcassets/AppIcon.appiconset/icon_1024.png": 1024}"#,
        cgbi: "[]",
    });
    assert_eq!(
        converted(&store).argv,
        [
            "/usr/bin/sips",
            "-s",
            "format",
            "png",
            "--resampleHeightWidthMax",
            "512",
            "out/App.app/Contents/Resources/AppIcon.icns",
            "--out",
            ICON
        ]
    );
}

#[test]
fn apple_icon_falls_back_to_the_asset_catalog_icon_set() {
    let store = apple_plan(&Apple {
        platform: "macos",
        plist: r#"{"CFBundleName": "App", "CFBundleIconFile": "AppIcon"}"#,
        inputs: "[]",
        icon_set: r#"["Assets.xcassets/AppIcon.appiconset/icon_16.png", "Assets.xcassets/AppIcon.appiconset/icon_1024.png", "Assets.xcassets/AppIcon.appiconset/icon_256.png"]"#,
        sizes: r#"{"out/App.app/Contents/Resources/AppIcon.icns": 1024, "Assets.xcassets/AppIcon.appiconset/icon_16.png": 16, "Assets.xcassets/AppIcon.appiconset/icon_1024.png": 1024, "Assets.xcassets/AppIcon.appiconset/icon_256.png": 256}"#,
        cgbi: "[]",
    });
    assert_eq!(
        converted(&store).inputs,
        ["Assets.xcassets/AppIcon.appiconset/icon_1024.png"]
    );
}

#[test]
fn apple_icon_is_left_empty_when_the_bundle_declares_no_icon() {
    let store = apple_plan(&Apple {
        platform: "macos",
        plist: r#"{"CFBundleName": "App", "CFBundleIconFile": "Missing.icns"}"#,
        inputs: "[]",
        icon_set: "[]",
        sizes: r#"{"out/App.app/Contents/Resources/Missing.icns": 1024}"#,
        cgbi: "[]",
    });
    assert!(written(&store, ICON).is_empty());
    assert_eq!(metadata(&store)["display_name"], "App");
}

#[test]
fn apple_app_icon_files_lists_the_named_icon_set_in_each_catalog() {
    let result = eval_prelude_source_to_repr(format!(
        r#"{}
def workspace_root():
    return "/workspace"
def host_read_dir(path):
    return {{
        "/workspace/app/Assets.xcassets/AppIcon.appiconset": ["Contents.json", "icon_1024.png"],
        "/abs/Shared.xcassets/AppIcon.appiconset": ["icon_512.png"],
        "/workspace/app/Assets.xcassets/Beta.appiconset": ["beta.png"],
    }}.get(path, [])
result = repr([
    _apple_app_icon_files(["app/Assets.xcassets", "/abs/Shared.xcassets"], "AppIcon"),
    _apple_app_icon_files(["app/Assets.xcassets"], ""),
])
"#,
        apple_prelude_source()
    ))
    .unwrap();
    assert_eq!(
        result,
        r#"[["app/Assets.xcassets/AppIcon.appiconset/icon_1024.png", "/abs/Shared.xcassets/AppIcon.appiconset/icon_512.png"], []]"#
    );
}
