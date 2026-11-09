use std::process::Command;
use std::time::{Duration, SystemTime};

use once_frontend::analysis::{with_active_store, AnalysisStore};

use super::{android_prelude_source, eval_prelude_source_to_repr};

#[test]
fn android_archives_ignore_input_modification_times() {
    let workspace = tempfile::tempdir().unwrap();
    let source = format!(
        r#"{}
ctx = {{"label": {{"package": "", "name": "Library", "id": "Library"}}, "attr": {{}}, "deps": [], "srcs": [], "build_dir": ".once/out/Library"}}
tools = {{"jar": "jar", "identity": "jdk", "sdk_root": "/sdk"}}
classes = _android_jar_classes(ctx, tools, "classes", "classes.sha256")
_android_package_aar(ctx, {{}}, tools, "AndroidManifest.xml", classes, "R.txt", [], [], [], [])
result = repr("ok")
"#,
        android_prelude_source()
    );
    let store = AnalysisStore::new(
        workspace.path().to_path_buf(),
        String::new(),
        ".once/out/Library".to_string(),
    );
    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
    result.unwrap();
    let actions = store
        .actions
        .iter()
        .filter(|action| action.argv.first().is_some_and(|arg| arg == "jar"))
        .collect::<Vec<_>>();
    assert_eq!(actions.len(), 2);
    let out = workspace.path().join(".once/out/Library");
    let staging = out.join("aar_staging");
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::create_dir_all(workspace.path().join("classes")).unwrap();
    let mut previous = None;
    for seconds in [1_600_000_000, 1_700_000_000] {
        let timestamp = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        for (path, contents) in [
            (
                workspace.path().join("classes/Library.class"),
                "class bytes",
            ),
            (staging.join("AndroidManifest.xml"), "<manifest/>"),
            (staging.join("R.txt"), ""),
        ] {
            std::fs::write(&path, contents).unwrap();
            std::fs::File::open(path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(timestamp))
                .unwrap();
        }
        for (index, action) in actions.iter().enumerate() {
            let output = Command::new(&action.argv[0])
                .args(&action.argv[1..])
                .current_dir(workspace.path())
                .output()
                .expect("JDK required for Android archive regression");
            assert!(output.status.success(), "{output:?}");
            if index == 0 {
                let classes = staging.join("classes.jar");
                std::fs::copy(out.join("Library.jar"), &classes).unwrap();
                std::fs::File::open(classes)
                    .unwrap()
                    .set_times(std::fs::FileTimes::new().set_modified(timestamp))
                    .unwrap();
            }
        }
        let archives =
            ["Library.jar", "Library.aar"].map(|name| std::fs::read(out.join(name)).unwrap());
        if let Some(previous) = previous {
            assert_eq!(archives, previous, "input mtimes must not affect archives");
        }
        previous = Some(archives);
    }
}
