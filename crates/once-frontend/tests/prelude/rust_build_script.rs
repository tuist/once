use super::*;

#[test]
fn build_script_generated_output_matches_crate_debug_assertions() {
    let cases: &[(&[&str], bool)] = &[
        (&[], true),
        (&["-C", "opt-level=0", "-Cdebuginfo=0"], true),
        (&["-C", "opt-level=3", "-C", "debuginfo=0"], false),
        (&["-Copt-level=1"], false),
        (&["--codegen", "opt-level=2"], false),
        (&["--codegen=opt-level=s"], false),
        (&["-Copt-level=z"], false),
        (&["-O"], false),
        (&["-O", "-Copt-level=0"], true),
        (&["-Copt-level=0", "-O"], false),
        (&["-Copt-level=3", "-C", "debug-assertions=yes"], true),
        (&["-Cdebug-assertions=no", "-Copt-level=0"], false),
        (&["-O", "-Cdebug-assertions"], true),
        (
            &[
                "-Cdebug-assertions=yes",
                "--codegen",
                "debug-assertions=off",
            ],
            false,
        ),
        (
            &[
                "-Cdebug-assertions=no",
                "--codegen=debug_assertions=on",
                "-O",
            ],
            true,
        ),
    ];
    for (flags, expected) in cases {
        assert_eq!(
            build_generated_crate(flags),
            expected.to_string(),
            "{flags:?}"
        );
    }
}

fn build_generated_crate(flags: &[&str]) -> String {
    let workspace = TempDir::new().unwrap();
    let root = workspace.path();
    std::fs::write(
            root.join("build.rs"),
            r#"
fn main() {
    assert!(cfg!(panic = "unwind"), "build scripts must retain host panic semantics");
    let name = if cfg!(debug_assertions) { "debug.rs" } else { "release.rs" };
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(out.join(name), format!("const SCRIPT_DEBUG: bool = {};", cfg!(debug_assertions))).unwrap();
}
"#,
        )
        .unwrap();
    std::fs::write(
        root.join("main.rs"),
        r#"
#[cfg(debug_assertions)]
include!(concat!(env!("OUT_DIR"), "/debug.rs"));
#[cfg(not(debug_assertions))]
include!(concat!(env!("OUT_DIR"), "/release.rs"));
fn main() {
    assert_eq!(SCRIPT_DEBUG, cfg!(debug_assertions));
    println!("{SCRIPT_DEBUG}");
}
"#,
    )
    .unwrap();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "app", "id": "app"}},
    "attr": {{
        "crate_root": "main.rs",
        "build_script": "build.rs",
        "rustc_flags": {flags:?} + ["-Cpanic=abort"],
    }},
    "deps": [],
    "srcs": ["*.rs"],
}}
_rust_compile(ctx, "bin", "main.rs", "app")
result = repr("ok")
"#,
        prelude = all_prelude_source(),
    );
    let store = store_for(root, "app");
    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
    result.unwrap();
    let resolve = |value: &str| value.replace("{{once.execution_root}}", root.to_str().unwrap());
    for action in &store.actions {
        for output in &action.outputs {
            std::fs::create_dir_all(root.join(output).parent().unwrap()).unwrap();
        }
        for directory in &action.create_dirs {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        if let Some(DeclaredActionOperation::WriteFile { path, bytes }) = &action.operation {
            std::fs::write(root.join(path), bytes).unwrap();
            continue;
        }
        assert!(action.operation.is_none(), "{action:?}");
        assert!(action.arg_files.is_empty(), "{action:?}");
        let output = Command::new(resolve(&action.argv[0]))
            .args(action.argv[1..].iter().map(|arg| resolve(arg)))
            .envs(action.env.iter().map(|(key, value)| (key, resolve(value))))
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "flags={flags:?}, {action:?}: {output:?}"
        );
    }
    let output = Command::new(root.join(".once/out/app/app"))
        .output()
        .unwrap();
    assert!(output.status.success(), "flags={flags:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn build_script_keeps_host_flags_when_crate_targets_windows() {
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "app", "id": "app"}},
    "attr": {{
        "target": "aarch64-pc-windows-msvc",
        "crate_root": "lib.rs",
        "build_script": "build.rs",
        "rustc_flags": ["-Copt-level=3", "-Cpanic=abort", "-Ctarget-cpu=cortex-a76"],
    }},
    "deps": [],
    "srcs": ["*.rs"],
}}
_rust_compile(ctx, "rlib", "lib.rs", "libapp.rlib")
result = repr("ok")
"#,
        prelude = all_prelude_source(),
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "app");
    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
    result.unwrap();
    let compile = action_by_identifier(&store, "app:build-script-rustc");
    for flag in ["--target", "-Cpanic=abort", "-Ctarget-cpu=cortex-a76"] {
        assert!(!compile.argv.iter().any(|arg| arg == flag), "{compile:?}");
    }
    let run = action_by_identifier(&store, "app:build-script");
    assert_eq!(run.env["TARGET"], "aarch64-pc-windows-msvc");
    assert_ne!(run.env["HOST"], run.env["TARGET"]);
    let target = action_by_identifier(&store, "app:rustc");
    assert!(target
        .argv
        .iter()
        .any(|arg| arg == "aarch64-pc-windows-msvc"));
    assert!(target.argv.iter().any(|arg| arg == "-Cpanic=abort"));
    assert!(target
        .argv
        .iter()
        .any(|arg| arg == "-Ctarget-cpu=cortex-a76"));
}
