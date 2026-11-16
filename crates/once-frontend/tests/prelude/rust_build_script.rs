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

#[test]
fn processes_launched_from_rust_artifacts_keep_windows_system_root() {
    let env = |os: &str| {
        let source = format!(
            r#"{prelude}
def host_os():
    return {os:?}
def host_env(name):
    return {{"Path": "C:\\Windows\\system32", "SystemRoot": "C:\\Windows", "INCLUDE": "C:\\sdk\\include"}}.get(name, "")
ctx = {{
    "label": {{"package": "", "name": "app", "id": "app"}},
    "attr": {{"crate_root": "main.rs"}},
    "deps": [],
    "srcs": ["*.rs"],
}}
result = repr([
    {{key: value for key, value in env.items() if key in ["PATH", "SystemRoot", "INCLUDE"]}}
    for env in [_rust_test_env(ctx, ""), _rust_binary_run_env(ctx, ".once/run")]
])
"#,
            prelude = all_prelude_source(),
        );
        let workspace = TempDir::new().unwrap();
        let store = store_for(workspace.path(), "");
        with_active_store(store, || eval_prelude_source_to_repr(source))
            .1
            .unwrap()
    };
    let windows = r#"{"PATH": "C:\\Windows\\system32", "SystemRoot": "C:\\Windows"}"#;
    assert_eq!(env("windows"), format!("[{windows}, {windows}]"));
    let linux = r#"{"PATH": "/usr/bin:/bin:/usr/sbin:/sbin"}"#;
    assert_eq!(env("linux"), format!("[{linux}, {linux}]"));
}

#[test]
fn rustc_on_macos_finds_toolchain_libraries_after_shell_wrapper_drops_dyld_variables() {
    let workspace = TempDir::new().unwrap();
    let root = workspace.path();
    let sysroot = root.join("toolchain");
    let rustc = sysroot.join("bin/rustc");
    let seen = root.join("dyld-fallback");
    std::fs::create_dir_all(rustc.parent().unwrap()).unwrap();
    // A compiled probe, like the real rustc: a shell script would be started
    // through /bin/sh, which on macOS drops DYLD_* variables once more.
    let probe = root.join("probe.c");
    std::fs::write(
        &probe,
        format!(
            r#"#include <stdio.h>
#include <stdlib.h>
int main(void) {{
    const char *value = getenv("DYLD_FALLBACK_LIBRARY_PATH");
    FILE *out = fopen({seen:?}, "w");
    if (!out) return 1;
    fputs(value ? value : "", out);
    return fclose(out);
}}
"#,
            seen = seen.to_str().unwrap()
        ),
    )
    .unwrap();
    let compiled = Command::new("cc")
        .arg(&probe)
        .arg("-o")
        .arg(&rustc)
        .output()
        .unwrap();
    assert!(compiled.status.success(), "{compiled:?}");
    std::fs::create_dir_all(root.join(".once/out/bindgen")).unwrap();
    std::fs::write(root.join(".once/out/bindgen/build-script.stdout"), "").unwrap();
    let source = format!(
        r#"{prelude}
def host_os():
    return "macos"

def _rustc_toolchain(target):
    return ({rustc:?}, "fake-rustc", "aarch64-apple-darwin")

ctx = {{
    "label": {{"package": "", "name": "web", "id": "web"}},
    "attr": {{"target": "wasm32-unknown-unknown", "crate_type": "cdylib", "crate_root": "lib.rs"}},
    "deps": [{{
        "crate_name": "wasm_bindgen",
        "rlib": ".once/out/bindgen/libwasm_bindgen.rlib",
        "build_script_stdout": ".once/out/bindgen/build-script.stdout",
    }}],
    "srcs": ["*.rs"],
}}
_rust_compile(ctx, "cdylib", "lib.rs", "web.wasm")
result = repr("ok")
"#,
        prelude = all_prelude_source(),
        rustc = rustc.to_str().unwrap(),
    );
    let store = store_for(root, "web");
    let (store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
    result.unwrap();
    let compile = action_by_identifier(&store, "web:rustc");
    let expected = format!("{}/lib:/usr/local/lib:/usr/lib", sysroot.display());
    assert_eq!(compile.env["DYLD_FALLBACK_LIBRARY_PATH"], expected);
    assert_ne!(compile.argv[0], rustc.to_str().unwrap(), "{compile:?}");
    let resolve = |value: &str| value.replace("{{once.execution_root}}", root.to_str().unwrap());
    // Launch the action the way macOS launches a SIP-protected /bin/sh: with
    // every DYLD_* variable removed from its environment.
    let output = Command::new(resolve(&compile.argv[0]))
        .args(compile.argv[1..].iter().map(|arg| resolve(arg)))
        .env_clear()
        .envs(
            compile
                .env
                .iter()
                .filter(|(key, _)| !key.starts_with("DYLD_"))
                .map(|(key, value)| (key, resolve(value))),
        )
        .current_dir(root)
        .output()
        .unwrap();
    assert!(output.status.success(), "{compile:?}: {output:?}");
    assert_eq!(std::fs::read_to_string(&seen).unwrap(), expected);
}
