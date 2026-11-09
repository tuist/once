use std::process::Command;

#[test]
fn kotlin_manifest_is_packaged_reproducibly_and_invalidates_cached_archives() {
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("Library.kt"), "class Library\n").unwrap();
    std::fs::write(
        workspace.path().join("once.toml"),
        "[[target]]\nname = \"Library\"\nkind = \"kotlin_jvm_library\"\nsrcs = [\"*.kt\"]\n\n[target.attrs]\nmanifest = \"library.mf\"\n",
    )
    .unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_once"))
            .arg("-C")
            .arg(workspace.path())
            .args([
                "--memory-limit",
                "512MiB",
                "--cpu-limit",
                "1",
                "--format",
                "json",
            ])
            .args(args)
            .env("HOME", home.path())
            .env("XDG_CACHE_HOME", home.path().join("cache"))
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("XDG_DATA_HOME", home.path().join("data"))
            .env("XDG_STATE_HOME", home.path().join("state"))
            .env("ONCE_CACHE_PROVIDER", "local")
            .env("ONCE_CHANGE_TRACKER", "0")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        output
    };
    let archive = workspace.path().join(".once/out/Library/Library.jar");
    let mut previous = None;
    for version in ["1.2.3", "2.3.4"] {
        std::fs::write(
            workspace.path().join("library.mf"),
            format!("Manifest-Version: 1.0\nImplementation-Version: {version}\n\n"),
        )
        .unwrap();
        run(&["build", "Library"]);
        let bytes = std::fs::read(&archive).unwrap();
        if let Some(previous) = previous {
            assert_ne!(bytes, previous, "manifest edit must invalidate the archive");
        }
        let extracted = tempfile::tempdir().unwrap();
        let output = Command::new("jar")
            .args(["--extract", "--file"])
            .arg(&archive)
            .current_dir(extracted.path())
            .output()
            .expect("JDK required for Kotlin manifest regression");
        assert!(output.status.success(), "{output:?}");
        let manifest =
            std::fs::read_to_string(extracted.path().join("META-INF/MANIFEST.MF")).unwrap();
        assert!(manifest.contains(&format!("Implementation-Version: {version}")));
        std::fs::remove_dir_all(workspace.path().join(".once/out/Library")).unwrap();
        run(&["build", "Library"]);
        assert_eq!(std::fs::read(&archive).unwrap(), bytes);
        previous = Some(bytes);
    }
    let output = run(&["build", "Library", "--verify-reproducible"]);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["reproducible"], true, "{report}");
    assert_eq!(report["trials"], 2, "{report}");
}
