use super::globals::expand_globs;
use super::store::{which_candidate_names_for, HostCache};
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

fn eval_string(source: &str) -> starlark::Result<String> {
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse("test.star", source.to_string(), &Dialect::Standard)?;
        let globals = globals_for_prelude();
        let mut evaluator = Evaluator::new(&module);
        evaluator.eval_module(ast, &globals)?;
        let value = module
            .get("value")
            .expect("test module should bind `value`");
        Ok(value.unpack_str().unwrap().to_string())
    })
}

fn eval_bool(source: &str) -> starlark::Result<bool> {
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse("test.star", source.to_string(), &Dialect::Standard)?;
        let globals = globals_for_prelude();
        let mut evaluator = Evaluator::new(&module);
        evaluator.eval_module(ast, &globals)?;
        let value = module
            .get("value")
            .expect("test module should bind `value`");
        Ok(value.unpack_bool().unwrap())
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
fn host_env_reads_active_analysis_environment_only() {
    let name = format!("ONCE_HOST_ENV_TEST_{}", std::process::id());
    std::env::set_var(&name, "present");
    let source = format!(
        "value = host_env({})",
        serde_json::to_string(&name).unwrap()
    );

    assert_eq!(eval_string(&source).unwrap(), "");

    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "apps/android/App");
    let (_, value) = with_active_store(store, || eval_string(&source).unwrap());
    assert_eq!(value, "present");

    std::env::remove_var(name);
}

#[test]
fn host_file_contains_matches_binary_host_files() {
    let tmp = TempDir::new().unwrap();
    let blob = tmp.path().join("blob.bin");
    std::fs::write(&blob, [0xff, b'a', b'b', b'c', 0x00]).unwrap();
    let source = format!(
        "value = host_file_contains({}, \"abc\")",
        serde_json::to_string(blob.to_str().unwrap()).unwrap()
    );

    let store = store_for(tmp.path(), "pkg");
    let (_, contains) = with_active_store(store, || eval_bool(&source).unwrap());
    assert!(contains);
}

#[test]
fn windows_host_which_candidates_skip_extensionless_names() {
    assert_eq!(
        which_candidate_names_for("rustc", true, Some(".COM;.EXE;.CMD")),
        vec![
            "rustc.COM".to_string(),
            "rustc.com".to_string(),
            "rustc.EXE".to_string(),
            "rustc.exe".to_string(),
            "rustc.CMD".to_string(),
            "rustc.cmd".to_string(),
        ]
    );
}

#[test]
fn windows_host_which_candidates_keep_explicit_extensions() {
    assert_eq!(
        which_candidate_names_for("rustc.exe", true, Some(".COM;.EXE;.CMD")),
        vec!["rustc.exe".to_string()]
    );
}

#[test]
fn unix_host_which_candidates_keep_name_verbatim() {
    assert_eq!(
        which_candidate_names_for("rustc", false, Some(".COM;.EXE;.CMD")),
        vec!["rustc".to_string()]
    );
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
    assert!(message.contains("strings or cmd_args values"), "{message}");
}

#[test]
fn run_action_flattens_cmd_args_with_arg_file() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "p");
    let (store, ()) = with_active_store(store, || {
        run(r#"
run_action(
    argv = [
        "rustc",
        cmd_args(
            args = ["--cfg", "feature=\"alloc\""],
            use_arg_file = {
                "path": ".once/out/p/rustc-features.rsp",
                "format": "line-delimited",
                "arg_format": "@{}",
            },
        ),
    ],
    outputs = [".once/out/p/lib.rlib"],
)
"#)
        .unwrap();
    });

    assert_eq!(store.actions.len(), 1);
    let action = &store.actions[0];
    assert_eq!(
        action.argv,
        vec![
            "rustc".to_string(),
            "@.once/out/p/rustc-features.rsp".to_string()
        ]
    );
    assert_eq!(action.arg_files.len(), 1);
    let arg_file = &action.arg_files[0];
    assert_eq!(arg_file.path, ".once/out/p/rustc-features.rsp");
    assert_eq!(arg_file.format, DeclaredArgFileFormat::LineDelimited);
    assert_eq!(
        arg_file.args,
        vec!["--cfg".to_string(), "feature=\"alloc\"".to_string()]
    );
}

#[test]
fn run_action_flattens_cmd_args_with_rustc_response_arg_file() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "p");
    let (store, ()) = with_active_store(store, || {
        run(r#"
run_action(
    argv = [
        "rustc",
        cmd_args(
            args = ["--cfg", "feature=\"alloc\""],
            use_arg_file = {
                "path": ".once/out/p/rustc-features.rsp",
                "format": "rustc-response",
                "arg_format": "@{}",
            },
        ),
    ],
    outputs = [".once/out/p/lib.rlib"],
)
"#)
        .unwrap();
    });

    assert_eq!(store.actions.len(), 1);
    let action = &store.actions[0];
    assert_eq!(
        action.argv,
        vec![
            "rustc".to_string(),
            "@.once/out/p/rustc-features.rsp".to_string()
        ]
    );
    assert_eq!(action.arg_files.len(), 1);
    let arg_file = &action.arg_files[0];
    assert_eq!(arg_file.path, ".once/out/p/rustc-features.rsp");
    assert_eq!(arg_file.format, DeclaredArgFileFormat::RustcResponse);
    assert_eq!(
        arg_file.args,
        vec!["--cfg".to_string(), "feature=\"alloc\"".to_string()]
    );
}

#[test]
fn run_action_rejects_line_delimited_arg_file_newlines() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "p");
    let (_, err) = with_active_store(store, || {
        run(r#"
run_action(
    argv = [
        cmd_args(
            args = ["--cfg", "feature=\"alloc\"\n--cfg"],
            use_arg_file = {"path": ".once/out/p/rustc-features.rsp"},
        ),
    ],
)
"#)
        .unwrap_err()
    });
    let message = format!("{err:?}");
    assert!(
        message.contains("contains an argument with a newline"),
        "{message}"
    );
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
fn write_path_records_text_operation() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "apps/ios/Mixed");
    let (store, ()) = with_active_store(store, || {
        run(r#"write_path(".once/out/apps/ios/Mixed/module.modulemap", "module Mixed { export * }\n")"#).unwrap();
    });
    assert_eq!(store.actions.len(), 1);
    let action = &store.actions[0];
    assert_eq!(
        action.outputs,
        vec![".once/out/apps/ios/Mixed/module.modulemap".to_string()]
    );
    assert!(action.argv.is_empty());
    assert_eq!(
        action.operation,
        Some(DeclaredActionOperation::WriteFile {
            path: ".once/out/apps/ios/Mixed/module.modulemap".to_string(),
            bytes: b"module Mixed { export * }\n".to_vec(),
        })
    );
}

#[test]
fn write_path_records_byte_operation() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "p");
    let (store, ()) = with_active_store(store, || {
        run(r#"write_path(".once/out/p/blob.bin", [0, 1, 2, 254, 255])"#).unwrap();
    });
    assert_eq!(store.actions.len(), 1);
    let action = &store.actions[0];
    assert_eq!(action.outputs, vec![".once/out/p/blob.bin".to_string()]);
    assert!(action.argv.is_empty());
    assert_eq!(
        action.operation,
        Some(DeclaredActionOperation::WriteFile {
            path: ".once/out/p/blob.bin".to_string(),
            bytes: vec![0, 1, 2, 254, 255],
        })
    );
}

#[test]
fn file_action_globals_record_portable_operations() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "p");
    let (store, ()) = with_active_store(store, || {
        run(
            r#"
copy_path("src/a.txt", ".once/out/p/a.txt", inputs = ["src/a.txt"], identifier = "copy-a")
copy_path(["src/res", "src/assets"], ".once/out/p/staged", kind = "tree", inputs = ["src/res/v.txt", "src/assets/a.txt"])
prepare_path(".once/out/p/staged", kind = "remove", identifier = "clean-staged")
prepare_path(".once/out/p/staged", kind = "directory")
write_tree_digest(".once/out/p/staged", ".once/out/p/staged.sha256", include_suffixes = [".txt"])
"#,
        )
        .unwrap();
    });

    assert_eq!(store.actions.len(), 5);
    assert_eq!(
        store.actions[0].operation,
        Some(DeclaredActionOperation::CopyPath {
            sources: vec!["src/a.txt".to_string()],
            destination: ".once/out/p/a.txt".to_string(),
            mode: DeclaredCopyPathMode::File,
        })
    );
    assert_eq!(store.actions[0].identifier.as_deref(), Some("copy-a"));
    assert_eq!(
        store.actions[1].operation,
        Some(DeclaredActionOperation::CopyPath {
            sources: vec!["src/res".to_string(), "src/assets".to_string()],
            destination: ".once/out/p/staged".to_string(),
            mode: DeclaredCopyPathMode::Tree,
        })
    );
    assert_eq!(
        store.actions[2].operation,
        Some(DeclaredActionOperation::PreparePath {
            path: ".once/out/p/staged".to_string(),
            mode: DeclaredPreparePathMode::Remove,
        })
    );
    assert!(!store.actions[2].cacheable);
    assert_eq!(
        store.actions[3].operation,
        Some(DeclaredActionOperation::PreparePath {
            path: ".once/out/p/staged".to_string(),
            mode: DeclaredPreparePathMode::Directory,
        })
    );
    assert!(!store.actions[3].cacheable);
    assert_eq!(
        store.actions[4].operation,
        Some(DeclaredActionOperation::WriteTreeDigest {
            root: ".once/out/p/staged".to_string(),
            output: ".once/out/p/staged.sha256".to_string(),
            include_suffixes: vec![".txt".to_string()],
        })
    );
}

#[test]
fn write_bytes_rejects_out_of_range_integers() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "p");
    let (_, err) = with_active_store(store, || {
        run(r#"write_path(".once/out/p/blob.bin", [256])"#).unwrap_err()
    });
    let message = format!("{err:?}");
    assert!(message.contains("0..=255"), "{message}");
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
fn analyze_target_errors_for_script_kind_without_starlark_impl() {
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
