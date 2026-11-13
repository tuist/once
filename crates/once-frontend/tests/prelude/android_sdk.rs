use once_frontend::analysis::{with_active_store, AnalysisStore};

use super::{action_by_identifier, android_prelude_source, eval_prelude_source_to_repr};

#[test]
fn android_resources_select_installed_release_platforms() {
    for (installed, requested, expected, target_sdk) in [
        (vec!["35"], None, "35", "35"),
        (vec!["37.0"], None, "37.0", "37"),
        (vec!["37.0"], Some(37), "37.0", "37"),
        (vec!["37", "37.0"], Some(37), "37", "37"),
        (
            vec!["35", "37.2", "37.10", "38-beta1", "CANARY"],
            None,
            "37.10",
            "37",
        ),
        (
            vec!["CANARY", "38-beta1", "37.10", "37.2", "35"],
            None,
            "37.10",
            "37",
        ),
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let sdk = workspace.path().join("sdk");
        for version in installed {
            let platform = sdk.join(format!("platforms/android-{version}"));
            std::fs::create_dir_all(&platform).unwrap();
            std::fs::write(platform.join("android.jar"), "platform").unwrap();
        }
        std::fs::write(workspace.path().join("AndroidManifest.xml"), "<manifest/>").unwrap();
        let requested = requested.map_or(String::new(), |api| format!("\"compile_sdk\": {api},"));
        let source = format!(
            r#"{}
def host_command(argv, env = None, merge_stderr = None):
    return "tool version test\n"

ctx = {{
    "label": {{"package": "", "name": "Resources", "id": "Resources"}},
    "attr": {{
        "android_sdk": {sdk:?},
        {requested}
        "build_tools_version": "37.0.0",
        "java": "/tools/java",
        "javac": "/tools/javac",
        "package": "dev.once.resources",
    }},
    "deps": [],
    "srcs": [],
    "build_dir": ".once/out/Resources",
}}
provider = _android_resource_impl(ctx)
result = repr(provider["target_kind"])
"#,
            android_prelude_source(),
            sdk = sdk.to_str().unwrap(),
        );
        let store = AnalysisStore::new(
            workspace.path().to_path_buf(),
            String::new(),
            ".once/out/Resources".to_string(),
        );
        let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
        assert_eq!(result.unwrap(), "\"android_resource\"");
        let link = action_by_identifier(&store, "android_resource_link:Resources");
        let android_jar = sdk
            .join(format!("platforms/android-{expected}/android.jar"))
            .to_string_lossy()
            .into_owned();
        assert!(link
            .argv
            .windows(2)
            .any(|args| args == ["-I", &android_jar]));
        assert!(link
            .argv
            .windows(2)
            .any(|args| args == ["--target-sdk-version", target_sdk]));
    }
}
