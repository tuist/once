use std::process::Command;

#[test]
fn kotlin_associates_make_multiple_modules_internal_declarations_visible() {
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    for (file, source) in [
        ("Left.kt", "internal fun left() = 20\n"),
        ("Right.kt", "internal fun right() = 22\n"),
        ("Consumer.kt", "fun answer() = left() + right()\n"),
    ] {
        std::fs::write(workspace.path().join(file), source).unwrap();
    }
    std::fs::write(
        workspace.path().join("once.toml"),
        r#"[[target]]
name = "Left"
kind = "kotlin_jvm_library"
srcs = ["Left.kt"]

[[target]]
name = "Right"
kind = "kotlin_jvm_library"
srcs = ["Right.kt"]

[[target]]
name = "Consumer"
kind = "kotlin_jvm_library"
srcs = ["Consumer.kt"]

[target.dependencies]
associates = ["./Left", "./Right"]
"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_once"))
        .arg("-C")
        .arg(workspace.path())
        .args([
            "--memory-limit",
            "512MiB",
            "--cpu-limit",
            "1",
            "build",
            "Consumer",
        ])
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
    assert!(workspace
        .path()
        .join(".once/out/Consumer/Consumer.jar")
        .is_file());
}
