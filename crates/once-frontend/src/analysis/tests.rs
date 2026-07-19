use super::globals::expand_globs;
#[cfg(windows)]
use super::store::which_candidate_names;
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
fn declared_tool_paths_take_precedence_over_host_path() {
    let cache = HostCache::with_tool_paths(BTreeMap::from([(
        "rustc".to_string(),
        "/managed/rustc".to_string(),
    )]));

    assert_eq!(
        cache.which("rustc").unwrap().as_deref(),
        Some("/managed/rustc")
    );
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
    let name = "ONCE_HOST_ENV_TEST";
    let source = format!(
        "value = host_env({})",
        serde_json::to_string(&name).unwrap()
    );

    assert_eq!(eval_string(&source).unwrap(), "");

    let tmp = TempDir::new().unwrap();
    let host_cache = HostCache::with_env(BTreeMap::from([(name.to_string(), "present".into())]));
    let store = AnalysisStore::with_host_cache(
        tmp.path().to_path_buf(),
        "apps/android/App".to_string(),
        ".once/out/apps/android/App".to_string(),
        host_cache,
    );
    let (_, value) = with_active_store(store, || eval_string(&source).unwrap());
    assert_eq!(value, "present");
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
fn host_file_read_returns_host_file_text() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("metadata.tsv");
    std::fs::write(&path, "version\t1\n").unwrap();
    let source = format!(
        "value = host_file_read({})",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );

    let store = store_for(tmp.path(), "pkg");
    let (_, content) = with_active_store(store, || eval_string(&source).unwrap());
    assert_eq!(content, "version\t1\n");
}

#[test]
fn windows_host_which_candidates_skip_extensionless_names() {
    assert_eq!(
        which_candidate_names_for("tool", true, Some(".COM;.EXE;.CMD")),
        vec![
            "tool.COM".to_string(),
            "tool.com".to_string(),
            "tool.EXE".to_string(),
            "tool.exe".to_string(),
            "tool.CMD".to_string(),
            "tool.cmd".to_string(),
        ]
    );
}

#[test]
fn windows_host_which_candidates_keep_explicit_extensions() {
    assert_eq!(
        which_candidate_names_for("tool.exe", true, Some(".COM;.EXE;.CMD")),
        vec!["tool.exe".to_string()]
    );
}

#[test]
fn unix_host_which_candidates_keep_name_verbatim() {
    assert_eq!(
        which_candidate_names_for("tool", false, Some(".COM;.EXE;.CMD")),
        vec!["tool".to_string()]
    );
}

#[cfg(windows)]
#[test]
fn windows_host_which_candidates_use_live_pathext() {
    assert_eq!(
        which_candidate_names("tool"),
        which_candidate_names_for(
            "tool",
            true,
            std::env::var_os("PATHEXT")
                .map(|value| value.to_string_lossy().into_owned())
                .as_deref()
        )
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
fn run_action_records_sandbox_policy() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "apps/ios/App");
    let (store, ()) = with_active_store(store, || {
        run(r#"
run_action(
    argv = ["tool", "input"],
    inputs = ["apps/ios/App/input"],
    outputs = [".once/out/apps/ios/App/output"],
    sandbox = "inputs",
)
"#)
        .unwrap();
    });
    assert_eq!(store.actions.len(), 1);
    assert_eq!(store.actions[0].sandbox.as_deref(), Some("inputs"));
}

#[test]
fn run_action_rejects_invalid_sandbox_policy() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "apps/ios/App");
    let (_, err) = with_active_store(store, || {
        run(r#"run_action(argv = ["tool"], sandbox = "strict")"#).unwrap_err()
    });
    let message = format!("{err:?}");
    assert!(message.contains("expected `sandbox`"), "{message}");
}

#[test]
fn run_action_can_skip_prior_action_dependencies() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "tools/split");
    let (store, ()) = with_active_store(store, || {
        run(r#"
run_action(
    argv = ["tool", "--emit", ".once/out/tools/split/second.txt"],
    inputs = ["tools/split/input.txt"],
    outputs = [".once/out/tools/split/second.txt"],
    depends_on_prior_actions = False,
)
"#)
        .unwrap();
    });
    assert_eq!(store.actions.len(), 1);
    assert!(!store.actions[0].depends_on_prior_actions);
}

#[test]
fn declared_action_defaults_cacheable_when_omitted() {
    let action: DeclaredAction = serde_json::from_value(serde_json::json!({
        "argv": ["tool", "input.src"],
        "outputs": [".once/out/App.o"]
    }))
    .unwrap();

    assert!(action.cacheable);
    assert!(action.depends_on_prior_actions);
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
        "tool",
        cmd_args(
            args = ["--flag", "value with spaces"],
            use_arg_file = {
                "path": ".once/out/p/args.txt",
                "format": "line-delimited",
                "arg_format": "@{}",
            },
        ),
    ],
    outputs = [".once/out/p/out.bin"],
)
"#)
        .unwrap();
    });

    assert_eq!(store.actions.len(), 1);
    let action = &store.actions[0];
    assert_eq!(
        action.argv,
        vec!["tool".to_string(), "@.once/out/p/args.txt".to_string()]
    );
    assert_eq!(action.arg_files.len(), 1);
    let arg_file = &action.arg_files[0];
    assert_eq!(arg_file.path, ".once/out/p/args.txt");
    assert_eq!(arg_file.format, DeclaredArgFileFormat::LineDelimited);
    assert_eq!(
        arg_file.args,
        vec!["--flag".to_string(), "value with spaces".to_string()]
    );
}

#[test]
fn run_action_rejects_unknown_arg_file_format() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "p");
    let (_, err) = with_active_store(store, || {
        run(r#"
run_action(
    argv = [
        "tool",
        cmd_args(
            args = ["--flag", "value"],
            use_arg_file = {
                "path": ".once/out/p/args.txt",
                "format": "custom",
                "arg_format": "@{}",
            },
        ),
    ],
    outputs = [".once/out/p/out.bin"],
)
"#)
        .unwrap_err()
    });

    let message = format!("{err:?}");
    assert!(
        message.contains("expected `cmd_args.use_arg_file.format` to be `line-delimited`"),
        "{message}"
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
            args = ["--flag", "value\n--other"],
            use_arg_file = {"path": ".once/out/p/args.txt"},
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
    assert_eq!(cache.command(&argv, &env, false).unwrap(), "done");
    assert_eq!(cache.command(&argv, &env, false).unwrap(), "done");

    assert_eq!(std::fs::read_to_string(counter).unwrap(), "x");
}

/// `merge_stderr` folds stderr into the returned output so version probes
/// for tools that print to stderr (kotlinc, older javac) need no host
/// shell `2>&1`. The merged and unmerged results occupy distinct cache
/// slots.
#[cfg(unix)]
#[test]
fn host_command_can_merge_stderr() {
    let cache = HostCache::default();
    let argv = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        "printf on-stdout; printf on-stderr >&2".to_string(),
    ];
    let env = BTreeMap::new();

    let stdout_only = cache.command(&argv, &env, false).unwrap();
    assert_eq!(stdout_only, "on-stdout");

    let merged = cache.command(&argv, &env, true).unwrap();
    assert!(merged.contains("on-stdout"), "{merged:?}");
    assert!(merged.contains("on-stderr"), "{merged:?}");
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
    assert_eq!(cache.command(&argv, &env_a, false).unwrap(), "a");
    assert_eq!(cache.command(&argv, &env_b, false).unwrap(), "b");
    // Repeat the first call: the env_a slot is now warm and
    // reuses the cached stdout without spawning the process.
    assert_eq!(cache.command(&argv, &env_a, false).unwrap(), "a");

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
fn materialize_host_file_records_a_content_addressed_operation() {
    let tmp = TempDir::new().unwrap();
    let source = tmp.path().join("toolchain.bin");
    std::fs::write(&source, b"toolchain").unwrap();
    let store = store_for(tmp.path(), "p");
    let source_literal = serde_json::to_string(source.to_str().unwrap()).unwrap();
    let script = format!("materialize_host_file({source_literal}, \".once/out/p/toolchain.bin\")");
    let (store, ()) = with_active_store(store, || run(&script).unwrap());

    assert_eq!(store.actions.len(), 1);
    assert_eq!(
        store.actions[0].operation,
        Some(DeclaredActionOperation::MaterializeHostFile {
            source: source.to_string_lossy().into_owned(),
            source_sha256: "0db3de82a739e43a2b560d166d037c3c0061601bb194866eb79b2c87045d00f2"
                .to_string(),
            destination: ".once/out/p/toolchain.bin".to_string(),
        })
    );
}

#[test]
fn run_action_records_command_setup_paths() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "p");
    let (store, ()) = with_active_store(store, || {
        run(r#"
run_action(
    argv = ["tool"],
    outputs = [".once/out/p/generated"],
    clean_paths = [".once/out/p/generated", ".once/tmp/p/home"],
    create_dirs = [".once/out/p/generated", ".once/tmp/p/home"],
)
"#)
        .unwrap();
    });

    assert_eq!(store.actions.len(), 1);
    let action = &store.actions[0];
    assert_eq!(
        action.clean_paths,
        vec![
            ".once/out/p/generated".to_string(),
            ".once/tmp/p/home".to_string(),
        ]
    );
    assert_eq!(
        action.create_dirs,
        vec![
            ".once/out/p/generated".to_string(),
            ".once/tmp/p/home".to_string(),
        ]
    );
}

#[test]
fn run_action_records_cwd() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "p");
    let (store, ()) = with_active_store(store, || {
        run(r#"
run_action(
    argv = ["tool"],
    outputs = [".once/out/p/generated"],
    cwd = "p",
)
"#)
        .unwrap();
    });

    assert_eq!(store.actions.len(), 1);
    assert_eq!(store.actions[0].cwd, Some("p".to_string()));
}

#[test]
fn run_action_defaults_cwd_to_none() {
    let tmp = TempDir::new().unwrap();
    let store = store_for(tmp.path(), "p");
    let (store, ()) = with_active_store(store, || {
        run(r#"run_action(argv = ["tool"], outputs = [".once/out/p/generated"])"#).unwrap();
    });

    assert_eq!(store.actions.len(), 1);
    assert_eq!(store.actions[0].cwd, None);
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
        dependency_edges: BTreeMap::new(),
        srcs: Vec::new(),
        attrs: BTreeMap::new(),
        capabilities: vec![Capability {
            name: "build".to_string(),
            output_groups: Vec::new(),
            requires_outputs: Vec::new(),
        }],
        providers: Vec::new(),
        tools: Vec::new(),
        diagnostics: Vec::new(),
    }
}

#[test]
fn analysis_exposes_dependency_providers_by_role() {
    let source = r#"
def _impl(ctx):
    return {
        "default": ctx["deps"][0]["value"],
        "default_by_role": ctx["deps_by_role"]["deps"][0]["value"],
        "plugin": ctx["deps_by_role"]["plugins"][0]["value"],
    }

custom = {
    "_once_target_kind": True,
    "kind": "custom",
    "impl": _impl,
}
"#;
    let engine = AnalysisEngine::from_source(source).unwrap();
    let mut target = target("custom");
    target.deps = vec!["default".to_string()];
    target
        .dependency_edges
        .insert("plugins".to_string(), vec!["plugin".to_string()]);
    let default = vec![serde_json::json!({"value": "default-provider"})];
    let named = BTreeMap::from([(
        "plugins".to_string(),
        vec![serde_json::json!({"value": "plugin-provider"})],
    )]);
    let workspace = TempDir::new().unwrap();

    let result = engine
        .analyze_target_capability_with_dependency_roles(
            &target,
            workspace.path(),
            &default,
            &named,
            "build",
        )
        .unwrap();

    assert_eq!(
        result.provider,
        serde_json::json!({
            "default": "default-provider",
            "default_by_role": "default-provider",
            "plugin": "plugin-provider",
        })
    );
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
fn analysis_context_exposes_visible_run_option() {
    let engine = AnalysisEngine::from_source_with_options(
        r#"
def _demo_impl(ctx):
    return {"visible": ctx["run"]["visible"]}

demo_kind = {"_once_target_kind": True, "impl": _demo_impl}
"#,
        AnalysisOptions {
            run_visible: true,
            ..AnalysisOptions::default()
        },
    )
    .unwrap();
    let tmp = TempDir::new().unwrap();

    let result = engine
        .analyze_target_capability(&target("demo_kind"), tmp.path(), &[], "run")
        .unwrap();

    assert_eq!(result.provider["visible"], true);
}

#[test]
fn analysis_context_exposes_semantic_test_filters() {
    let engine = AnalysisEngine::from_source_with_options(
        r#"
def _demo_impl(ctx):
    return {"filters": ctx["test"]["filters"]}

demo_kind = {"_once_target_kind": True, "impl": _demo_impl}
"#,
        AnalysisOptions {
            test_filters: vec!["tests/unit::case-a".to_string()],
            ..AnalysisOptions::default()
        },
    )
    .unwrap();
    let tmp = TempDir::new().unwrap();

    let result = engine
        .analyze_target_capability(&target("demo_kind"), tmp.path(), &[], "test")
        .unwrap();

    assert_eq!(
        result.provider["filters"],
        serde_json::json!(["tests/unit::case-a"])
    );
}

#[test]
fn analysis_context_exposes_test_batch_identity() {
    let engine = AnalysisEngine::from_source_with_options(
        r#"
def _demo_impl(ctx):
    return {"batch_id": ctx["test"]["batch_id"]}

demo_kind = {"_once_target_kind": True, "impl": _demo_impl}
"#,
        AnalysisOptions {
            test_batch_id: Some("batch-a".to_string()),
            ..AnalysisOptions::default()
        },
    )
    .unwrap();
    let tmp = TempDir::new().unwrap();

    let result = engine
        .analyze_target_capability(&target("demo_kind"), tmp.path(), &[], "test")
        .unwrap();

    assert_eq!(result.provider["batch_id"], "batch-a");
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
    return {"out": out, "scratch": ctx["scratch_dir"]}

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
    assert_eq!(
        result.provider["scratch"],
        ".once/tmp/analysis/apps/ios/Sample"
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
