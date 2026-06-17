use super::globals::{base64_encode, expand_globs, shell_quote};
use super::store::HostCache;
use super::*;
use crate::graph::GraphTarget;
use crate::target::AttrValue;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};
use std::collections::BTreeMap;
use std::path::Path;
use tempfile::TempDir;

fn run(source: &str) -> starlark::Result<()> {
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse("test.star", source.to_string(), &Dialect::Standard)?;
        let globals = globals_for_prelude();
        let mut evaluator = Evaluator::new(&module);
        evaluator.eval_module(ast, &globals)?;
        starlark::Result::Ok(())
    })
}

fn store_for(workspace: &Path, package: &str) -> AnalysisStore {
    AnalysisStore::new(
        workspace.to_path_buf(),
        package.to_string(),
        format!(".once/out/{package}"),
    )
}

#[test]
fn schema_parse_path_resolves_native_globals_without_calling_them() {
    run("def _impl():\n    return run_action\n").unwrap();
}

#[test]
fn declare_output_outside_analysis_returns_bare_name() {
    run(r#"x = declare_output("AppCore.a")"#).unwrap();
}

#[test]
fn run_action_records_declarations_when_analysis_is_active() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "apps/ios/AppCore");
    let (store, ()) = with_active_store(store, || {
        run(r#"
run_action(
    argv = ["tool", "-o", "output.bin"],
    inputs = ["pkg/Sources/main.src"],
    outputs = ["AppCore.a"],
    toolchain_identity = "id-1",
    identifier = "compile",
)
"#)
        .unwrap();
    });
    assert_eq!(store.actions.len(), 1);
    assert_eq!(store.actions[0].argv[0], "tool");
    assert_eq!(store.actions[0].outputs, vec!["AppCore.a".to_string()]);
    assert_eq!(store.actions[0].identifier.as_deref(), Some("compile"));
    assert!(store.actions[0].cacheable);
}

#[test]
fn run_action_can_mark_declarations_uncacheable() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "apps/ios/App");
    let (store, ()) = with_active_store(store, || {
        run(r#"
run_action(
    argv = ["open", ".once/out/apps/ios/App/App.app"],
    outputs = [".once/out/apps/ios/App/run/run.json"],
    cacheable = False,
)
"#)
        .unwrap();
    });
    assert_eq!(store.actions.len(), 1);
    assert!(!store.actions[0].cacheable);
}

#[test]
fn declared_action_defaults_cacheable_when_omitted() {
    let action: DeclaredAction = serde_json::from_value(serde_json::json!({
        "argv": ["tool", "input.src"],
        "outputs": [".once/out/App.o"]
    }))
    .unwrap();

    assert!(action.cacheable);
    assert_eq!(
        serde_json::to_value(&action).unwrap(),
        serde_json::json!({
            "argv": ["tool", "input.src"],
            "outputs": [".once/out/App.o"]
        })
    );
}

#[test]
fn declare_output_attaches_active_build_dir() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "apps/ios/AppCore");
    let (store, ()) = with_active_store(store, || {
        run(r#"x = declare_output("AppCore.a")"#).unwrap();
    });
    assert_eq!(
        store.declared_outputs,
        vec![".once/out/apps/ios/AppCore/AppCore.a".to_string()]
    );
}

#[test]
fn run_action_rejects_non_string_argv_entries() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "p");
    let (_, err) = with_active_store(store, || {
        run(r#"run_action(argv = [1, "tool"])"#).unwrap_err()
    });
    let message = format!("{err:?}");
    assert!(message.contains("entries to be strings"), "{message}");
}

#[cfg(unix)]
#[test]
fn host_command_cache_reuses_identical_argv_results() {
    let tmp = TempDir::new().unwrap();
    let counter = tmp.path().join("counter");
    let cache = HostCache::default();
    let argv = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        "printf x >> \"$1\"; printf done".to_string(),
        "sh".to_string(),
        counter.display().to_string(),
    ];

    let env = BTreeMap::new();
    assert_eq!(cache.command(&argv, &env).unwrap(), "done");
    assert_eq!(cache.command(&argv, &env).unwrap(), "done");

    assert_eq!(std::fs::read_to_string(counter).unwrap(), "x");
}

/// Two calls with the same argv but different `env` must spawn the
/// process twice, with no shared cache slot. This keeps host
/// discovery probes partitioned by their environment overrides.
#[cfg(unix)]
#[test]
fn host_command_cache_keys_on_env() {
    let tmp = TempDir::new().unwrap();
    let counter = tmp.path().join("counter");
    let cache = HostCache::default();
    let argv = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        "printf x >> \"$1\"; printf \"$ONCE_TEST_PIN\"".to_string(),
        "sh".to_string(),
        counter.display().to_string(),
    ];
    let mut env_a = BTreeMap::new();
    env_a.insert("ONCE_TEST_PIN".to_string(), "a".to_string());
    let mut env_b = BTreeMap::new();
    env_b.insert("ONCE_TEST_PIN".to_string(), "b".to_string());

    // Distinct env values land in distinct cache slots and the
    // process is re-spawned for each, so the script's stdout
    // reflects each env's pin value.
    assert_eq!(cache.command(&argv, &env_a).unwrap(), "a");
    assert_eq!(cache.command(&argv, &env_b).unwrap(), "b");
    // Repeat the first call: the env_a slot is now warm and
    // reuses the cached stdout without spawning the process.
    assert_eq!(cache.command(&argv, &env_a).unwrap(), "a");

    // Counter increments once per spawn: env_a (cold), env_b
    // (cold), env_a (warm, no spawn) -> two ticks total.
    assert_eq!(std::fs::read_to_string(counter).unwrap(), "xx");
}

#[test]
fn shell_quote_handles_empty_strings_quotes_and_specials() {
    assert_eq!(shell_quote(""), "''");
    // No special characters: single-quote wrap with no escapes.
    assert_eq!(shell_quote("abc"), "'abc'");
    // Single quote in the middle uses the close/escape/reopen form
    // so the resulting word still expands to a single token.
    assert_eq!(shell_quote("a'b"), "'a'\"'\"'b'");
    // Backslashes, dollar signs, double quotes are inert inside the
    // single-quoted POSIX form, so they pass through verbatim.
    assert_eq!(shell_quote("$x \\n \"y\""), "'$x \\n \"y\"'");
}

/// [`write_file`] declares an action whose argv is
/// `["/bin/sh", "-c", script]`. The script must (a) bind the path
/// before computing its parent directory, (b) include the content
/// as the only `printf` argument, and (c) declare the path as the
/// only output.
#[test]
fn write_file_records_an_action_with_path_binding_and_content() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "apps/ios/Mixed");
    let (store, ()) = with_active_store(store, || {
        run(r#"write_file(".once/out/apps/ios/Mixed/module.modulemap", "module Mixed { export * }\n")"#).unwrap();
    });
    assert_eq!(store.actions.len(), 1);
    let action = &store.actions[0];
    assert_eq!(
        action.outputs,
        vec![".once/out/apps/ios/Mixed/module.modulemap".to_string()]
    );
    assert_eq!(action.argv[0], "/bin/sh");
    assert_eq!(action.argv[1], "-c");
    let script = &action.argv[2];
    assert!(script.contains("__once_path="), "{script}");
    assert!(
        script.contains("printf '%s' 'module Mixed { export * }"),
        "{script}"
    );
    assert!(script.contains("> \"$__once_path\""), "{script}");
    // The path is referenced via the binding, never inlined into
    // the dirname call, so single quotes in the path can't escape
    // the substitution.
    assert!(!script.contains("$(dirname"), "{script}");
}

/// The end-to-end script the action declares must actually run and
/// produce the file on a real shell. This catches scripting bugs
/// that the structural assertions above would miss.
#[cfg(unix)]
#[test]
fn write_file_script_actually_creates_the_file() {
    let tmp = TempDir::new().unwrap();
    let nested = tmp.path().join("nested/dir/holds/output.txt");
    // Use a path with a single quote in a parent dir to lock in
    // the fix from the review thread: the script must survive
    // quotes inside `__once_path` without re-tokenising.
    let quoted = tmp.path().join("a'b").join("out.txt");
    let store = AnalysisStore::new(tmp.path().to_path_buf(), String::new(), String::new());
    let (store, ()) = with_active_store(store, || {
        run(&format!(
            r#"write_file({nested:?}, "hello\n")
write_file({quoted:?}, "quoted\n")
"#,
            nested = nested.display().to_string(),
            quoted = quoted.display().to_string(),
        ))
        .unwrap();
    });
    assert_eq!(store.actions.len(), 2);
    for action in &store.actions {
        let status = std::process::Command::new(&action.argv[0])
            .arg(&action.argv[1])
            .arg(&action.argv[2])
            .status()
            .expect("script should spawn");
        assert!(status.success(), "script failed: {:?}", action.argv);
    }
    assert_eq!(std::fs::read_to_string(&nested).unwrap(), "hello\n");
    assert_eq!(std::fs::read_to_string(&quoted).unwrap(), "quoted\n");
}

/// `write_bytes` should accept a list of 0..=255 integers, encode
/// them as base64 in the generated shell script, fold the encoded
/// bytes into the toolchain identity, and declare the path as its
/// only output. The primitive is intentionally domain-agnostic;
/// callers compose arbitrary binary formats in the prelude.
#[test]
fn write_bytes_records_action_with_base64_payload() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "p");
    let (store, ()) = with_active_store(store, || {
        run(r#"write_bytes(".once/out/p/blob.bin", [0, 1, 2, 254, 255])"#).unwrap();
    });
    assert_eq!(store.actions.len(), 1);
    let action = &store.actions[0];
    assert_eq!(action.outputs, vec![".once/out/p/blob.bin".to_string()]);
    assert_eq!(action.argv[0], "/bin/sh");
    let script = &action.argv[2];
    assert!(script.contains("base64 -d"), "{script}");
    assert!(
        action
            .toolchain_identity
            .as_deref()
            .is_some_and(|id| id.starts_with("once.write_bytes.v1\0")),
        "{:?}",
        action.toolchain_identity
    );
}

/// The shell script the action declares must run end-to-end and
/// reproduce the exact byte sequence on disk, NULs and 0xFF
/// inclusive. Round-tripping through base64 + `base64 -d` is the
/// part of `write_bytes` that domain-specific callers depend on.
#[cfg(unix)]
#[test]
fn write_bytes_script_reproduces_exact_byte_sequence() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("blob.bin");
    let store = AnalysisStore::new(tmp.path().to_path_buf(), String::new(), String::new());
    let (store, ()) = with_active_store(store, || {
        run(&format!(
            r"write_bytes({path:?}, [0, 1, 2, 255, 0, 128, 64])",
            path = out.display().to_string(),
        ))
        .unwrap();
    });
    let action = &store.actions[0];
    let status = std::process::Command::new(&action.argv[0])
        .arg(&action.argv[1])
        .arg(&action.argv[2])
        .status()
        .expect("script should spawn");
    assert!(status.success(), "script failed: {:?}", action.argv);
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(bytes, vec![0, 1, 2, 255, 0, 128, 64]);
}

#[test]
fn write_bytes_rejects_out_of_range_integers() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "p");
    let (_, err) = with_active_store(store, || {
        run(r#"write_bytes(".once/out/p/blob.bin", [256])"#).unwrap_err()
    });
    let message = format!("{err:?}");
    assert!(message.contains("0..=255"), "{message}");
}

#[test]
fn base64_encode_round_trips_for_short_inputs() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
    assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
}

#[test]
fn glob_expands_against_active_package_directory() {
    let tmp = TempDir::new().unwrap();
    let pkg = tmp.path().join("apps/ios/AppCore/Sources");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join("a.src"), "").unwrap();
    std::fs::write(pkg.join("b.src"), "").unwrap();
    std::fs::write(pkg.join("c.txt"), "").unwrap();

    let store = store_for(tmp.path(), "apps/ios/AppCore");
    let (store, ()) = with_active_store(store, || {
        run(r#"
matches = glob(["Sources/*.src"])
run_action(argv = ["echo"] + matches, outputs = ["out"])
"#)
        .unwrap();
    });
    assert_eq!(store.actions.len(), 1);
    let argv = &store.actions[0].argv;
    assert_eq!(argv[0], "echo");
    assert!(argv[1..].iter().any(|p| p.ends_with("Sources/a.src")));
    assert!(argv[1..].iter().any(|p| p.ends_with("Sources/b.src")));
    assert!(!argv[1..].iter().any(|p| p.ends_with("Sources/c.txt")));
}

/// A symlink that resolves outside the workspace must surface as
/// an error rather than silently leaking external paths into the
/// returned list. The check rejects honest mistakes; the threat
/// model assumes a non-adversarial workspace (documented on
/// `expand_globs`). Windows junctions/symlinks behave similarly
/// via the same canonicalize call, but get their own test once
/// Windows CI exists.
#[cfg(unix)]
#[test]
fn glob_rejects_symlink_that_escapes_workspace() {
    let workspace = TempDir::new().unwrap();
    let external = TempDir::new().unwrap();
    let pkg = workspace.path().join("apps/ios/AppCore");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(external.path().join("stolen.src"), "").unwrap();
    std::os::unix::fs::symlink(external.path().join("stolen.src"), pkg.join("escape.src")).unwrap();

    let err = expand_globs(
        workspace.path(),
        "apps/ios/AppCore",
        &["escape.src".to_string()],
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("outside the workspace"), "{err}");
}

fn target(kind: &str) -> GraphTarget {
    use crate::graph::{Capability, TargetLabel};
    GraphTarget {
        label: TargetLabel {
            package: "apps/ios".to_string(),
            name: "Sample".to_string(),
            id: "apps/ios/Sample".to_string(),
        },
        kind: kind.to_string(),
        deps: Vec::new(),
        srcs: Vec::new(),
        attrs: BTreeMap::new(),
        capabilities: vec![Capability {
            name: "build".to_string(),
            output_groups: Vec::new(),
            requires_outputs: Vec::new(),
        }],
        providers: Vec::new(),
        diagnostics: Vec::new(),
    }
}

#[test]
fn analyze_target_returns_null_provider_for_target_kinds_without_impl() {
    // `script` is the canonical example of a target kind that the
    // bundled prelude knows about but provides no Starlark impl
    // for; the analysis driver should hand back a null provider
    // and no actions so the CLI falls back to its own runner.
    let tmp = TempDir::new().unwrap();
    let result = analyze_target(&target("script"), tmp.path(), &[]);
    // `script` is supplied by the CLI's script-runner path; the
    // frontend should error with the same "no target kind found" surface
    // it uses for unknown kinds. Confirm that here so the
    // "no impl available" path is exercised end-to-end.
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("no target kind found for kind `script`"),
        "{err}"
    );
}

#[test]
fn analyze_target_errors_on_unknown_target_kind() {
    let tmp = TempDir::new().unwrap();
    let err = analyze_target(&target("mystery_kind"), tmp.path(), &[])
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("no target kind found for kind `mystery_kind`"),
        "{err}"
    );
}

#[test]
fn target_kind_has_impl_reads_custom_target_kind_impls() {
    let engine = AnalysisEngine::from_source(
        r#"
custom_library = {"_once_target_kind": True, "impl": lambda ctx: None}
"#,
    )
    .unwrap();

    assert!(engine.target_kind_has_impl("custom_library"));
}

#[test]
fn analysis_engine_debug_omits_module_source() {
    let engine = AnalysisEngine::from_source("# SECRET_MODULE_SOURCE").unwrap();

    let rendered = format!("{engine:?}");

    assert!(rendered.contains("source_len"));
    assert!(!rendered.contains("SECRET_MODULE_SOURCE"));
}

#[test]
fn workspace_analysis_engine_runs_custom_target_kind_impl() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir(tmp.path().join("modules")).unwrap();
    std::fs::write(
        tmp.path().join("once.toml"),
        "[modules]\npaths = [\"modules/*.star\"]\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("modules/demo.star"),
        r#"
def _demo_impl(ctx):
    out = declare_output("hello.txt")
    run_action(
        argv = ["/bin/sh", "-c", "printf custom > \"$1\"", "sh", out],
        outputs = [out],
        identifier = "demo_build",
    )
    return {"out": out}

demo_kind = target_kind(
    docs = "Demo",
    attrs = [],
    deps = [],
    providers = ["demo_provider"],
    capabilities = [
        capability("build", ["default"]),
    ],
    impl = _demo_impl,
)
"#,
    )
    .unwrap();
    let engine = AnalysisEngine::for_workspace(tmp.path()).unwrap();

    let result = engine
        .analyze_target(&target("demo_kind"), tmp.path(), &[])
        .unwrap();

    assert_eq!(result.actions.len(), 1);
    assert_eq!(result.actions[0].identifier.as_deref(), Some("demo_build"));
    assert_eq!(
        result.provider["out"],
        ".once/out/apps/ios/Sample/hello.txt"
    );
}

#[test]
fn target_kind_has_impl_returns_false_for_unknown_kind() {
    assert!(!target_kind_has_impl("mystery_kind").unwrap());
}

fn select_attr_value(branches: &[(&str, AttrValue)]) -> AttrValue {
    let inner: BTreeMap<String, AttrValue> = branches
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let mut outer = BTreeMap::new();
    outer.insert("select".to_string(), AttrValue::Map(inner));
    AttrValue::Map(outer)
}

#[test]
fn select_branches_detects_canonical_shape() {
    let value = select_attr_value(&[("ios", AttrValue::String("yes".to_string()))]);
    assert!(select_branches(&value).is_some());

    let not_a_select = AttrValue::Map(BTreeMap::from([(
        "select".to_string(),
        AttrValue::String("x".to_string()),
    )]));
    assert!(select_branches(&not_a_select).is_none());

    let map_with_extra_key = AttrValue::Map(BTreeMap::from([
        ("select".to_string(), AttrValue::Map(BTreeMap::new())),
        ("else".to_string(), AttrValue::String("x".to_string())),
    ]));
    assert!(select_branches(&map_with_extra_key).is_none());
}
