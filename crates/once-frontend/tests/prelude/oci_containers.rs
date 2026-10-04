use super::*;
use once_frontend::analysis::DeclaredActionOperation;
use serde_json::{json, Value};

const BASE_DIFF: &str = "4444444444444444444444444444444444444444444444444444444444444444";
const NEW_LAYER: &str = "5555555555555555555555555555555555555555555555555555555555555555";
const NEW_DIFF: &str = "6666666666666666666666666666666666666666666666666666666666666666";

fn write(workspace: &Path, path: &str, content: &str) {
    let path = workspace.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn base_layout(
    workspace: &Path,
    directory: &str,
    os: &str,
    architecture: &str,
    seed: &str,
) -> String {
    let manifest = seed.repeat(64);
    let config = format!("c{}", &seed.repeat(63));
    let layer = format!("d{}", &seed.repeat(63));
    write(
        workspace,
        &format!("{directory}/index.json"),
        &json!({"manifests": [{
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": format!("sha256:{manifest}"),
            "size": 10,
            "platform": {"os": os, "architecture": architecture},
        }]})
        .to_string(),
    );
    write(
        workspace,
        &format!("{directory}/blobs/sha256/{manifest}"),
        &json!({
            "config": {"digest": format!("sha256:{config}")},
            "layers": [{"mediaType": "application/vnd.oci.image.layer.v1.tar+gzip", "digest": format!("sha256:{layer}"), "size": 3}],
        })
        .to_string(),
    );
    write(
        workspace,
        &format!("{directory}/blobs/sha256/{config}"),
        "{}",
    );
    write(
        workspace,
        &format!("{directory}/blobs/sha256/{layer}"),
        "xyz",
    );
    format!("{directory}/index.json")
}

fn run_planner(workspace: &Path, call: &str) -> (AnalysisStore, Result<String, String>) {
    let source = format!("{}\nresult = repr({call})", oci_containers_source());
    let store = AnalysisStore::new(
        workspace.to_path_buf(),
        String::new(),
        ".once/out/img".into(),
    );
    with_active_store(store, || eval_prelude_source_to_repr(source))
}

fn planner_call(function: &str, args: &Value) -> String {
    format!(
        "{function}({{\"args\": json_decode({:?})}})",
        args.to_string()
    )
}

fn file(store: &AnalysisStore, suffix: &str) -> String {
    store
        .actions
        .iter()
        .find_map(|action| match &action.operation {
            Some(DeclaredActionOperation::WriteFile { path, bytes }) if path.ends_with(suffix) => {
                Some(String::from_utf8(bytes.clone()).unwrap())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing written file {suffix}"))
}

fn image_spec(base: &str) -> Value {
    json!({
        "label": "demo/image",
        "layout": ".once/out/img/demo.oci",
        "descriptor": ".once/out/img/descriptor.json",
        "manifest": ".once/out/img/manifest.json",
        "config": ".once/out/img/config.json",
        "archive": ".once/out/img/demo.oci.tar",
        "archive_sha256": ".once/out/img/demo.oci.tar.sha256",
        "architecture": "arm64", "os": "linux", "variant": "",
        "entrypoint": ["/app"], "cmd": null, "env": {"KEEP": "2", "NEW": "x"},
        "user": "", "working_dir": "", "stop_signal": "",
        "labels": {"b": "2"}, "annotations": {"note": "n"},
        "exposed_ports": ["80/tcp"], "volumes": ["/data"],
        "author": "", "created": "",
        "tags": ["demo:v1", "registry.example.com/team/demo:v2"],
        "base": base,
        "layers": [{
            "blob": "layer.tar.gz", "sha256": "layer.sha256", "diff_id": "layer.diff", "annotations": {"layer.note": "n"},
            "media_type": "application/vnd.oci.image.layer.v1.tar+gzip",
        }],
    })
}

fn assert_layout_housekeeping(store: &AnalysisStore) {
    assert!(
        !store.actions.iter().any(|action| matches!(
            action.operation,
            Some(DeclaredActionOperation::PreparePath { .. })
        )),
        "a layout is never deleted, so concurrent builds only write identical bytes"
    );
    let archive_paths = store
        .actions
        .iter()
        .find_map(|action| match &action.operation {
            Some(DeclaredActionOperation::WriteArchive { entries, .. }) => Some(
                entries
                    .iter()
                    .map(|entry| entry.path.clone())
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .expect("an archive action");
    assert!(
        archive_paths.contains(&"index.json".to_string()),
        "{archive_paths:?}"
    );
    assert!(
        archive_paths.contains(&"manifest.json".to_string()),
        "{archive_paths:?}"
    );
    assert!(
        archive_paths
            .iter()
            .all(|path| !path.starts_with("..") && !path.starts_with(".once")),
        "{archive_paths:?}"
    );
    let copied = store
        .actions
        .iter()
        .filter(|action| {
            action
                .identifier
                .as_deref()
                .is_some_and(|id| id.contains(":oci-blob:"))
        })
        .count();
    assert_eq!(
        copied, 2,
        "one copy for the base layer and one for the new layer"
    );
}

#[test]
fn oci_image_plan_extends_a_base_image_and_names_every_tag() {
    let workspace = TempDir::new().unwrap();
    let root = workspace.path();
    let index = base_layout(root, "base", "linux", "arm64", "a");
    assert_eq!(index, "base/index.json");
    let base_config = json!({
        "architecture": "arm64", "os": "linux",
        "config": {"Env": ["PATH=/bin", "KEEP=1"], "Cmd": ["/bin/sh"], "Labels": {"a": "1"}},
        "rootfs": {"type": "layers", "diff_ids": [format!("sha256:{BASE_DIFF}")]},
        "history": [{"created_by": "base"}],
    });
    let config_digest = format!("c{}", "a".repeat(63));
    write(
        root,
        &format!("base/blobs/sha256/{config_digest}"),
        &base_config.to_string(),
    );
    write(root, "layer.tar.gz", "12345");
    write(root, "layer.sha256", &format!("{NEW_LAYER}\n"));
    write(root, "layer.diff", &format!("{NEW_DIFF}\n"));
    let (store, result) = run_planner(root, &planner_call("oci_image_plan", &image_spec("base")));
    result.unwrap();

    let config: Value = serde_json::from_str(&file(&store, ".once/out/img/config.json")).unwrap();
    assert_eq!(config["config"]["Entrypoint"], json!(["/app"]));
    assert!(config["config"].get("Cmd").is_none(), "{config}");
    assert_eq!(
        config["config"]["Env"],
        json!(["PATH=/bin", "KEEP=2", "NEW=x"])
    );
    assert_eq!(config["config"]["Labels"], json!({"a": "1", "b": "2"}));
    assert_eq!(config["config"]["ExposedPorts"], json!({"80/tcp": {}}));
    assert_eq!(config["config"]["Volumes"], json!({"/data": {}}));
    assert_eq!(
        config["rootfs"]["diff_ids"],
        json!([format!("sha256:{BASE_DIFF}"), format!("sha256:{NEW_DIFF}")])
    );
    assert_eq!(config["history"].as_array().unwrap().len(), 2);

    let manifest: Value =
        serde_json::from_str(&file(&store, ".once/out/img/manifest.json")).unwrap();
    let layers = manifest["layers"].as_array().unwrap();
    assert_eq!(layers.len(), 2);
    assert_eq!(layers[0]["size"], 3);
    assert_eq!(layers[1]["digest"], format!("sha256:{NEW_LAYER}"));
    assert_eq!(layers[1]["size"], 5);
    assert_eq!(layers[1]["annotations"], json!({"layer.note": "n"}));
    assert!(layers[0].get("annotations").is_none());
    assert_eq!(manifest["annotations"], json!({"note": "n"}));

    let index: Value = serde_json::from_str(&file(&store, "demo.oci/index.json")).unwrap();
    let names = index["manifests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["annotations"]["io.containerd.image.name"]
                    .as_str()
                    .unwrap()
                    .to_string(),
                entry["annotations"]["org.opencontainers.image.ref.name"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            ("docker.io/library/demo:v1".to_string(), "v1".to_string()),
            (
                "registry.example.com/team/demo:v2".to_string(),
                "v2".to_string()
            ),
        ]
    );
    let docker: Value = serde_json::from_str(&file(&store, "demo.oci/manifest.json")).unwrap();
    assert_eq!(
        docker[0]["RepoTags"],
        json!(["demo:v1", "registry.example.com/team/demo:v2"])
    );
    assert_eq!(docker[0]["Layers"].as_array().unwrap().len(), 2);

    assert_layout_housekeeping(&store);
}

#[test]
fn oci_image_plan_keeps_the_base_command_unless_the_entrypoint_is_set() {
    let workspace = TempDir::new().unwrap();
    let root = workspace.path();
    base_layout(root, "base", "linux", "arm64", "a");
    write(
        root,
        &format!("base/blobs/sha256/c{}", "a".repeat(63)),
        &json!({"config": {"Cmd": ["/bin/sh"], "User": "nobody"}, "rootfs": {"diff_ids": []}})
            .to_string(),
    );
    write(root, "layer.tar.gz", "1");
    write(root, "layer.sha256", &format!("{NEW_LAYER}\n"));
    write(root, "layer.diff", &format!("{NEW_DIFF}\n"));
    let mut spec = image_spec("base");
    spec["entrypoint"] = json!(null);
    spec["env"] = json!({});
    spec["labels"] = json!({});
    let (store, result) = run_planner(root, &planner_call("oci_image_plan", &spec));
    result.unwrap();
    let config: Value = serde_json::from_str(&file(&store, ".once/out/img/config.json")).unwrap();
    assert_eq!(config["config"]["Cmd"], json!(["/bin/sh"]));
    assert_eq!(config["config"]["User"], "nobody");
    assert!(config["config"].get("Entrypoint").is_none());
}

#[test]
fn oci_image_plan_selects_the_matching_platform_of_a_multi_platform_base() {
    let workspace = TempDir::new().unwrap();
    let root = workspace.path();
    let arm = base_layout(root, "arm", "linux", "arm64", "a");
    let amd = base_layout(root, "amd", "linux", "amd64", "b");
    let mut merged: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join(&arm)).unwrap()).unwrap();
    let other: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join(&amd)).unwrap()).unwrap();
    merged["manifests"]
        .as_array_mut()
        .unwrap()
        .push(other["manifests"][0].clone());
    write(root, "multi/index.json", &merged.to_string());
    for seed in ["a", "b"] {
        let manifest = seed.repeat(64);
        let directory = if seed == "a" { "arm" } else { "amd" };
        std::fs::create_dir_all(root.join("multi/blobs/sha256")).unwrap();
        for entry in std::fs::read_dir(root.join(directory).join("blobs/sha256")).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(
                entry.path(),
                root.join("multi/blobs/sha256").join(entry.file_name()),
            )
            .unwrap();
        }
        let _ = manifest;
    }
    write(root, "layer.tar.gz", "1");
    write(root, "layer.sha256", &format!("{NEW_LAYER}\n"));
    write(root, "layer.diff", &format!("{NEW_DIFF}\n"));
    let mut spec = image_spec("multi");
    spec["architecture"] = json!("amd64");
    let (store, result) = run_planner(root, &planner_call("oci_image_plan", &spec));
    result.unwrap();
    let manifest: Value =
        serde_json::from_str(&file(&store, ".once/out/img/manifest.json")).unwrap();
    assert_eq!(
        manifest["layers"][0]["digest"],
        format!("sha256:d{}", "b".repeat(63))
    );
    spec["architecture"] = json!("riscv64");
    let (_, result) = run_planner(root, &planner_call("oci_image_plan", &spec));
    assert!(result.unwrap_err().contains("oci_base_platform_not_found"));
}

fn index_spec(images: Vec<(&str, Value)>) -> Value {
    json!({
        "label": "demo/index",
        "layout": ".once/out/idx/demo.oci",
        "descriptor": ".once/out/idx/descriptor.json",
        "archive": ".once/out/idx/demo.oci.tar",
        "archive_sha256": ".once/out/idx/demo.oci.tar.sha256",
        "annotations": {},
        "tags": ["demo:latest"],
        "images": images.into_iter().map(|(layout, platform)| json!({"layout": layout, "platform": platform})).collect::<Vec<_>>(),
    })
}

#[test]
fn oci_index_plan_combines_platform_images_in_a_stable_order() {
    let workspace = TempDir::new().unwrap();
    let root = workspace.path();
    base_layout(root, "arm", "linux", "arm64", "a");
    base_layout(root, "amd", "linux", "amd64", "b");
    let (store, result) = run_planner(
        root,
        &planner_call(
            "oci_index_plan",
            &index_spec(vec![("arm", json!({})), ("amd", json!({}))]),
        ),
    );
    result.unwrap();
    let index = file(&store, "demo.oci/index.json");
    let index: Value = serde_json::from_str(&index).unwrap();
    assert_eq!(
        index["manifests"][0]["annotations"]["org.opencontainers.image.ref.name"],
        "latest"
    );
    let digest = index["manifests"][0]["digest"]
        .as_str()
        .unwrap()
        .trim_start_matches("sha256:")
        .to_string();
    let document: Value =
        serde_json::from_str(&file(&store, &format!("blobs/sha256/{digest}"))).unwrap();
    let platforms = document["manifests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            entry["platform"]["architecture"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(platforms, vec!["amd64", "arm64"]);
    assert_eq!(
        document["mediaType"],
        "application/vnd.oci.image.index.v1+json"
    );
}

#[test]
fn oci_index_plan_rejects_two_images_for_one_platform() {
    let workspace = TempDir::new().unwrap();
    let root = workspace.path();
    base_layout(root, "one", "linux", "arm64", "a");
    base_layout(root, "two", "linux", "arm64", "b");
    let (_, result) = run_planner(
        root,
        &planner_call(
            "oci_index_plan",
            &index_spec(vec![("one", json!({})), ("two", json!({}))]),
        ),
    );
    assert!(result.unwrap_err().contains("oci_index_duplicate_platform"));
}

fn registry_call(kind: &str, capability: &str, attrs: &str, deps: &str) -> String {
    format!(
        r#"{kind}({{
    "label": {{"package": "", "name": "t", "id": "t"}},
    "attr": {attrs},
    "deps_by_role": {deps},
    "srcs": [], "build_dir": ".once/out/t", "scratch_dir": ".once/tmp/t",
    "capability": "{capability}",
}})"#
    )
}

fn registry_source(call: &str) -> String {
    format!(
        r#"{}
def _resolve_host_executable(name):
    return "/tools/" + name
def host_command(argv, merge_stderr = False, check = True):
    return "v1.2.3"
def host_env(name):
    return "/home" if name == "HOME" else ""
def host_file_exists(path):
    return "missing" not in path
def workspace_root():
    return "/ws"
def host_arch():
    return "aarch64"
result = repr({call})
"#,
        oci_containers_source()
    )
}

fn analyze_registry(call: &str) -> (AnalysisStore, Result<String, String>) {
    let workspace = TempDir::new().unwrap();
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        String::new(),
        ".once/out/t".into(),
    );
    with_active_store(store, || eval_prelude_source_to_repr(registry_source(call)))
}

const DIGEST: &str = "sha256:25109184c71bdad752c8312a8623239686a9a2071e8825f20acb8f2198c3f659";

#[test]
fn oci_import_extracts_a_layout_archive_and_validates_the_archive_path() {
    let call = |attrs: &str| registry_call("_oci_import_impl", "build", attrs, "{}");
    let (store, result) = analyze_registry(&call(r#"{"archive": "base.oci.tar"}"#));
    result.unwrap();
    let action = action_by_identifier(&store, "t:oci-import");
    assert_eq!(&action.argv[1..3], ["-xf", "base.oci.tar"]);
    assert!(action.cacheable);
    for (attrs, code) in [
        (r#"{"archive": "base.zip"}"#, "oci_import_invalid_archive"),
        (
            r#"{"archive": "missing.tar"}"#,
            "oci_import_archive_not_found",
        ),
    ] {
        let (_, result) = analyze_registry(&call(attrs));
        assert!(result.unwrap_err().contains(code), "{attrs}");
    }
}

#[test]
fn oci_pull_fetches_by_digest_with_crane_and_requires_a_valid_reference() {
    let (store, result) = analyze_registry(&registry_call(
        "_oci_pull_impl",
        "build",
        &format!(r#"{{"image": "docker.io/library/alpine", "digest": "{DIGEST}"}}"#),
        "{}",
    ));
    result.unwrap();
    let action = action_by_identifier(&store, "t:oci-pull");
    assert_eq!(
        action.argv,
        vec![
            "/tools/crane",
            "pull",
            "--format",
            "oci",
            "--platform",
            "linux/arm64",
            &format!("docker.io/library/alpine@{DIGEST}"),
            ".once/out/t/layout",
        ]
    );
    assert!(action.cacheable);
    for (attrs, code) in [
        (
            r#"{"image": "alpine:3", "digest": "sha256:00"}"#,
            "oci_pull_invalid_image",
        ),
        (
            r#"{"image": "alpine", "digest": "latest"}"#,
            "oci_pull_invalid_digest",
        ),
        (
            &format!(r#"{{"image": "alpine", "digest": "{DIGEST}", "platform": "linux"}}"#),
            "oci_pull_invalid_platform",
        ),
    ] {
        let (_, result) = analyze_registry(&registry_call("_oci_pull_impl", "build", attrs, "{}"));
        assert!(result.unwrap_err().contains(code), "{attrs}");
    }
    let (store, result) = analyze_registry(&registry_call(
        "_oci_pull_impl",
        "build",
        &format!(r#"{{"image": "alpine", "digest": "{DIGEST}", "platform": "all"}}"#),
        "{}",
    ));
    result.unwrap();
    assert!(!action_by_identifier(&store, "t:oci-pull")
        .argv
        .contains(&"--platform".to_string()));
}

const IMAGE_DEP: &str = r#"{"image": [{"label_id": "app/image", "layout": ".once/out/app/layout", "archive": ".once/out/app/app.tar", "tags": ["app:1"]}]}"#;

#[test]
fn oci_push_declares_a_planner_that_publishes_the_layout() {
    let (store, result) = analyze_registry(&registry_call(
        "_oci_push_impl",
        "run",
        r#"{"repository": "registry.example.com/team/app", "remote_tags": ["v1", "latest"], "insecure": True, "sign": True, "cosign_key": "k.key", "cosign_args": ["--tlog-upload=false"]}"#,
        IMAGE_DEP,
    ));
    result.unwrap();
    let expansion = store
        .actions
        .iter()
        .find_map(|action| match &action.operation {
            Some(DeclaredActionOperation::ExpandActions { implementation, .. }) => {
                Some((implementation.clone(), action))
            }
            _ => None,
        })
        .expect("a deferred push planner");
    assert_eq!(expansion.0, "oci_push_plan");
    assert_eq!(
        expansion.1.inputs,
        vec![".once/out/app/layout/index.json".to_string()]
    );
    assert_eq!(
        expansion.1.outputs.len(),
        2,
        "digest and signature are promised"
    );
}

#[test]
fn oci_push_validates_the_repository_and_tags() {
    for (attrs, code) in [
        (
            r#"{"repository": "registry.example.com/app:v1"}"#,
            "oci_push_invalid_repository",
        ),
        (
            r#"{"repository": "-registry.example.com/app"}"#,
            "oci_push_invalid_repository",
        ),
        (
            r#"{"repository": "registry.example.com/app", "remote_tags": ["a/b"]}"#,
            "oci_push_invalid_tag",
        ),
        (
            r#"{"repository": "registry.example.com/app", "remote_tags": ["valid", "--help"]}"#,
            "oci_push_invalid_tag",
        ),
        (
            r#"{"repository": "registry.example.com/app", "remote_tags": [".dot"]}"#,
            "oci_push_invalid_tag",
        ),
        (
            r#"{"repository": "registry.example.com/app", "remote_tags": []}"#,
            "oci_push_without_tags",
        ),
    ] {
        let (_, result) =
            analyze_registry(&registry_call("_oci_push_impl", "run", attrs, IMAGE_DEP));
        assert!(result.unwrap_err().contains(code), "{attrs}");
    }
    let (_, result) = analyze_registry(&registry_call(
        "_oci_push_impl",
        "run",
        r#"{"repository": "r.example/app"}"#,
        "{}",
    ));
    assert!(result.unwrap_err().contains("oci_requires_one_image"));
}

fn push_spec(sign: bool) -> Value {
    json!({
        "label": "t", "layout": "layout", "view": ".once/out/t/push/view",
        "repository": "registry.example.com/app", "tags": ["v1", "latest"],
        "crane": "/tools/crane", "identity": "crane-id", "insecure": true, "env": {},
        "digest_file": ".once/out/t/push/digest.txt", "sign": sign,
        "cosign": "/tools/cosign", "cosign_identity": "cosign-id", "cosign_key": "k.key",
        "cosign_args": ["--tlog-upload=false"], "signature": ".once/out/t/push/signature.txt",
    })
}

#[test]
fn oci_push_plan_publishes_one_entry_view_and_tags_and_signs_by_digest() {
    let workspace = TempDir::new().unwrap();
    let root = workspace.path();
    let entry = |name: &str| {
        json!({
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": format!("sha256:{}", "7".repeat(64)), "size": 9,
            "platform": {"os": "linux", "architecture": "arm64"},
            "annotations": {"io.containerd.image.name": name},
        })
    };
    write(
        root,
        "layout/index.json",
        &json!({"manifests": [entry("docker.io/library/a:1"), entry("docker.io/library/a:2")]})
            .to_string(),
    );
    let (store, result) = run_planner(root, &planner_call("oci_push_plan", &push_spec(true)));
    result.unwrap();
    let view: Value = serde_json::from_str(&file(&store, "push/view/index.json")).unwrap();
    assert_eq!(view["manifests"].as_array().unwrap().len(), 1);
    assert!(view["manifests"][0].get("annotations").is_none());
    let digest = format!("sha256:{}", "7".repeat(64));
    let push = action_by_identifier(&store, "t:oci-push");
    assert_eq!(
        push.argv,
        vec![
            "/tools/crane",
            "push",
            "--insecure",
            ".once/out/t/push/view",
            "registry.example.com/app:v1"
        ]
    );
    let tag = action_by_identifier(&store, "t:oci-tag:latest");
    assert_eq!(
        tag.argv,
        vec![
            "/tools/crane".to_string(),
            "tag".to_string(),
            "--insecure".to_string(),
            format!("registry.example.com/app@{digest}"),
            "latest".to_string()
        ]
    );
    assert_eq!(
        action_by_identifier(&store, "t:oci-sign").argv,
        vec![
            "/tools/cosign".to_string(),
            "sign".to_string(),
            "--yes".to_string(),
            "--key".to_string(),
            "k.key".to_string(),
            "--allow-insecure-registry".to_string(),
            "--tlog-upload=false".to_string(),
            format!("registry.example.com/app@{digest}")
        ]
    );
    assert!(store.actions.iter().any(|action| matches!(
        action.operation,
        Some(DeclaredActionOperation::LinkPath { .. })
    )));
}

#[test]
fn oci_push_plan_refuses_a_layout_that_holds_different_images() {
    let workspace = TempDir::new().unwrap();
    let root = workspace.path();
    write(
        root,
        "layout/index.json",
        &json!({"manifests": [
            {"mediaType": "m", "digest": format!("sha256:{}", "1".repeat(64)), "size": 1},
            {"mediaType": "m", "digest": format!("sha256:{}", "2".repeat(64)), "size": 1},
        ]})
        .to_string(),
    );
    let (_, result) = run_planner(root, &planner_call("oci_push_plan", &push_spec(false)));
    assert!(result.unwrap_err().contains("oci_push_ambiguous_layout"));
}

#[test]
fn oci_load_loads_the_image_archive_with_the_selected_engine() {
    let (store, result) = analyze_registry(&registry_call(
        "_oci_load_impl",
        "run",
        r#"{"daemon": "podman"}"#,
        IMAGE_DEP,
    ));
    result.unwrap();
    let load = action_by_identifier(&store, "t:oci-load");
    assert_eq!(
        load.argv,
        vec!["/tools/podman", "load", "--input", ".once/out/app/app.tar"]
    );
    assert!(!load.cacheable);
    let (store, result) =
        analyze_registry(&registry_call("_oci_load_impl", "build", "{}", IMAGE_DEP));
    result.unwrap();
    assert!(store.actions.is_empty());
    let bundle = r#"{"image": [{"label_id": "a", "archive": "a.tar", "tags": ["a:1"]}, {"label_id": "b", "archive": "b.tar", "tags": ["b:1"]}]}"#;
    let (store, result) = analyze_registry(&registry_call("_oci_load_impl", "run", "{}", bundle));
    result.unwrap();
    assert_eq!(
        action_by_identifier(&store, "t:oci-load:0")
            .argv
            .last()
            .unwrap(),
        "a.tar"
    );
    assert_eq!(
        action_by_identifier(&store, "t:oci-load:1")
            .argv
            .last()
            .unwrap(),
        "b.tar"
    );
}

fn simple_image_spec(base: &str) -> Value {
    let mut spec = image_spec(base);
    spec["entrypoint"] = json!(null);
    spec["env"] = json!({});
    spec["labels"] = json!({});
    spec["annotations"] = json!({});
    spec["exposed_ports"] = json!([]);
    spec["volumes"] = json!([]);
    spec["tags"] = json!(["demo:v1"]);
    spec
}

fn simple_layer(root: &Path) {
    write(root, "layer.tar.gz", "1");
    write(root, "layer.sha256", &format!("{NEW_LAYER}\n"));
    write(root, "layer.diff", &format!("{NEW_DIFF}\n"));
}

#[test]
fn oci_image_plan_clears_inherited_commands_when_given_empty_lists() {
    let workspace = TempDir::new().unwrap();
    let root = workspace.path();
    base_layout(root, "base", "linux", "arm64", "a");
    write(
        root,
        &format!("base/blobs/sha256/c{}", "a".repeat(63)),
        &json!({"architecture": "arm64", "os": "linux", "config": {"Entrypoint": ["/bin/echo"], "Cmd": ["inherited"]}, "rootfs": {"diff_ids": []}}).to_string(),
    );
    simple_layer(root);
    let mut spec = simple_image_spec("base");
    spec["entrypoint"] = json!([]);
    spec["cmd"] = json!([]);
    let (store, result) = run_planner(root, &planner_call("oci_image_plan", &spec));
    result.unwrap();
    let config: Value = serde_json::from_str(&file(&store, ".once/out/img/config.json")).unwrap();
    assert!(config["config"].get("Entrypoint").is_none(), "{config}");
    assert!(config["config"].get("Cmd").is_none(), "{config}");
}

#[test]
fn oci_image_plan_sizes_descriptors_in_bytes_and_escapes_control_characters() {
    let workspace = TempDir::new().unwrap();
    let root = workspace.path();
    simple_layer(root);
    let mut spec = simple_image_spec("");
    spec["layers"][0]["media_type"] = json!("application/vnd.oci.image.layer.v1.tar");
    spec["labels"] =
        json!({"message": "caf\u{e9} \u{4f60}\u{597d} \u{1f600}", "control": "a\u{1}b"});
    spec["annotations"] = json!({"note": "\u{4f60}\u{597d}"});
    let (store, result) = run_planner(root, &planner_call("oci_image_plan", &spec));
    result.unwrap();
    let config_text = file(&store, ".once/out/img/config.json");
    let manifest_text = file(&store, ".once/out/img/manifest.json");
    let manifest: Value = serde_json::from_str(&manifest_text).unwrap();
    assert_eq!(manifest["config"]["size"], config_text.len());
    let descriptor: Value =
        serde_json::from_str(&file(&store, ".once/out/img/descriptor.json")).unwrap();
    assert_eq!(descriptor["size"], manifest_text.len());
    let config: Value =
        serde_json::from_str(&config_text).expect("valid JSON despite the control character");
    assert_eq!(config["config"]["Labels"]["control"], "a\u{1}b");
    assert!(config_text.contains("\\u0001"), "{config_text}");
}

#[test]
fn oci_image_plan_rejects_a_base_for_another_architecture() {
    let workspace = TempDir::new().unwrap();
    let root = workspace.path();
    base_layout(root, "base", "linux", "arm64", "a");
    write(
        root,
        &format!("base/blobs/sha256/c{}", "a".repeat(63)),
        &json!({"architecture": "arm64", "os": "linux", "config": {}, "rootfs": {"diff_ids": []}})
            .to_string(),
    );
    simple_layer(root);
    let mut spec = simple_image_spec("base");
    spec["architecture"] = json!("amd64");
    let (_, result) = run_planner(root, &planner_call("oci_image_plan", &spec));
    assert!(result.unwrap_err().contains("oci_base_platform_mismatch"));
}

#[test]
fn oci_image_plan_selects_a_platform_from_an_index_nested_in_the_base_layout() {
    let workspace = TempDir::new().unwrap();
    let root = workspace.path();
    base_layout(root, "inner_arm", "linux", "arm64", "a");
    base_layout(root, "inner_amd", "linux", "amd64", "b");
    let nested = json!({
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": [
            {"mediaType": "application/vnd.oci.image.manifest.v1+json", "digest": format!("sha256:{}", "a".repeat(64)), "size": 10, "platform": {"os": "linux", "architecture": "arm64"}},
            {"mediaType": "application/vnd.oci.image.manifest.v1+json", "digest": format!("sha256:{}", "b".repeat(64)), "size": 10, "platform": {"os": "linux", "architecture": "amd64"}},
        ],
    });
    let nested_digest = "9".repeat(64);
    for directory in ["inner_arm", "inner_amd"] {
        for entry in std::fs::read_dir(root.join(directory).join("blobs/sha256")).unwrap() {
            let entry = entry.unwrap();
            let target = root.join("outer/blobs/sha256").join(entry.file_name());
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
    write(
        root,
        &format!("outer/blobs/sha256/{nested_digest}"),
        &nested.to_string(),
    );
    write(
        root,
        "outer/index.json",
        &json!({"manifests": [{"mediaType": "application/vnd.oci.image.index.v1+json", "digest": format!("sha256:{nested_digest}"), "size": 1}]}).to_string(),
    );
    for seed in ["a", "b"] {
        write(
            root,
            &format!("outer/blobs/sha256/c{}", seed.repeat(63)),
            &json!({"architecture": if seed == "a" { "arm64" } else { "amd64" }, "os": "linux", "config": {}, "rootfs": {"diff_ids": []}}).to_string(),
        );
    }
    simple_layer(root);
    let mut spec = simple_image_spec("outer");
    spec["architecture"] = json!("amd64");
    let (store, result) = run_planner(root, &planner_call("oci_image_plan", &spec));
    result.unwrap();
    let manifest: Value =
        serde_json::from_str(&file(&store, ".once/out/img/manifest.json")).unwrap();
    assert_eq!(
        manifest["layers"][0]["digest"],
        format!("sha256:d{}", "b".repeat(63))
    );
}

#[test]
fn oci_index_plan_treats_several_names_for_one_image_as_one_platform_entry() {
    let workspace = TempDir::new().unwrap();
    let root = workspace.path();
    base_layout(root, "one", "linux", "arm64", "a");
    let mut index: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("one/index.json")).unwrap())
            .unwrap();
    let entry = index["manifests"][0].clone();
    index["manifests"].as_array_mut().unwrap().push(entry);
    write(root, "one/index.json", &index.to_string());
    let (store, result) = run_planner(
        root,
        &planner_call("oci_index_plan", &index_spec(vec![("one", json!({}))])),
    );
    result.unwrap();
    let index: Value = serde_json::from_str(&file(&store, "demo.oci/index.json")).unwrap();
    let digest = index["manifests"][0]["digest"]
        .as_str()
        .unwrap()
        .trim_start_matches("sha256:")
        .to_string();
    let document: Value =
        serde_json::from_str(&file(&store, &format!("blobs/sha256/{digest}"))).unwrap();
    assert_eq!(document["manifests"].as_array().unwrap().len(), 1);
}

#[test]
fn oci_image_rejects_an_invalid_creation_timestamp_and_accepts_valid_ones() {
    let dep =
        r#"{"layers": [{"label_id": "l", "blob": "l.tar", "sha256": "l.sha", "platform": {}}]}"#;
    for created in [
        "not-a-date",
        "2026-13-01T00:00:00Z",
        "2026-01-01 00:00:00Z",
        "2026-01-01T00:00:00",
    ] {
        let (_, result) = analyze_registry(&registry_call(
            "_oci_image_impl",
            "build",
            &format!(r#"{{"created": "{created}"}}"#),
            dep,
        ));
        assert!(
            result.unwrap_err().contains("oci_image_invalid_created"),
            "{created}"
        );
    }
    for created in [
        "2026-01-02T03:04:05Z",
        "2026-01-02T03:04:05.123456789+02:00",
    ] {
        let (_, result) = analyze_registry(&registry_call(
            "_oci_image_impl",
            "build",
            &format!(r#"{{"created": "{created}"}}"#),
            dep,
        ));
        result.unwrap();
    }
}

#[test]
fn oci_tags_and_repositories_are_ascii_only() {
    for tag in ["ok", "v1.2-rc_3", "_x"] {
        let attrs = format!(r#"{{"repository": "r.example/app", "remote_tags": ["{tag}"]}}"#);
        let (_, result) =
            analyze_registry(&registry_call("_oci_push_impl", "build", &attrs, IMAGE_DEP));
        result.unwrap();
    }
    for attrs in [
        r#"{"repository": "r.example/app", "remote_tags": ["ok", "é"]}"#,
        r#"{"repository": "r.example/app", "remote_tags": ["٣"]}"#,
        r#"{"repository": "r.example/appé"}"#,
    ] {
        let (_, result) =
            analyze_registry(&registry_call("_oci_push_impl", "build", attrs, IMAGE_DEP));
        assert!(result.unwrap_err().contains("oci_push_invalid"), "{attrs}");
    }
}

#[test]
fn oci_creation_timestamps_follow_the_strict_form_container_engines_parse() {
    let dep =
        r#"{"layers": [{"label_id": "l", "blob": "l.tar", "sha256": "l.sha", "platform": {}}]}"#;
    for created in [
        "2026-02-30T00:00:00Z",
        "2025-02-29T00:00:00Z",
        "2026-01-01t00:00:00Z",
        "2026-01-01T00:00:00z",
        "2026-01-01T23:59:60Z",
        "2026-٠١-01T00:00:00Z",
        "2026-01-01T00:00:00.Z",
        "2026-01-01T00:00:00.1234567890Z",
    ] {
        let (_, result) = analyze_registry(&registry_call(
            "_oci_image_impl",
            "build",
            &format!(r#"{{"created": "{created}"}}"#),
            dep,
        ));
        assert!(
            result.unwrap_err().contains("oci_image_invalid_created"),
            "{created}"
        );
    }
    for created in [
        "2024-02-29T23:59:59Z",
        "2026-01-01T00:00:00.5Z",
        "2026-12-31T00:00:00-05:30",
    ] {
        let (_, result) = analyze_registry(&registry_call(
            "_oci_image_impl",
            "build",
            &format!(r#"{{"created": "{created}"}}"#),
            dep,
        ));
        result.unwrap();
    }
}

#[test]
fn oci_image_plan_resolves_docker_manifest_lists_in_a_base() {
    let workspace = TempDir::new().unwrap();
    let root = workspace.path();
    base_layout(root, "inner", "linux", "arm64", "a");
    let list_digest = "8".repeat(64);
    std::fs::create_dir_all(root.join("outer/blobs/sha256")).unwrap();
    for entry in std::fs::read_dir(root.join("inner/blobs/sha256")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(
            entry.path(),
            root.join("outer/blobs/sha256").join(entry.file_name()),
        )
        .unwrap();
    }
    write(
        root,
        &format!("outer/blobs/sha256/{list_digest}"),
        &json!({"mediaType": "application/vnd.docker.distribution.manifest.list.v2+json", "manifests": [{
            "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
            "digest": format!("sha256:{}", "a".repeat(64)), "size": 10,
            "platform": {"os": "linux", "architecture": "arm64"},
        }]})
        .to_string(),
    );
    write(
        root,
        "outer/index.json",
        &json!({"manifests": [{"mediaType": "application/vnd.docker.distribution.manifest.list.v2+json", "digest": format!("sha256:{list_digest}"), "size": 1}]}).to_string(),
    );
    write(
        root,
        &format!("outer/blobs/sha256/c{}", "a".repeat(63)),
        &json!({"architecture": "arm64", "os": "linux", "config": {}, "rootfs": {"diff_ids": []}})
            .to_string(),
    );
    simple_layer(root);
    let (_, result) = run_planner(
        root,
        &planner_call("oci_image_plan", &simple_image_spec("outer")),
    );
    result.unwrap();
}

#[test]
fn oci_layer_rejects_backslashes_in_container_paths() {
    let (_, result) = analyze_registry(&registry_call(
        "_oci_layer_impl",
        "build",
        r#"{"symlinks": {"/usr/bin/a\\b": "/x"}}"#,
        "{}",
    ));
    assert!(result.unwrap_err().contains("oci_layer_backslash_in_path"));
}
