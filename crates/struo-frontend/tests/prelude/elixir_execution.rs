use super::*;

const WORKER: &str = r#"
worker = {
    "schema_version": 1,
    "environment": "image-a",
    "os": "linux",
    "arch": "x86_64",
    "elixir_bin": "/worker/elixir/bin",
    "erlang_bin": "/worker/otp/bin",
    "mix_home": "/worker/mix",
    "rebar3": "/worker/mix/rebar3",
}
execution = {"provider": "workers", "executor": "tuist", "environment": "image-a", "properties": {"elixir": worker}}
def execution_context():
    return execution
def host_which(name):
    fail("must not resolve worker tools on the coordinator: " + name)
def host_command(argv, env = None, cwd = None, merge_stderr = None):
    fail("must not run worker tools on the coordinator: " + str(argv))
def host_env(name):
    return ""
def glob(patterns, exclude = []):
    return []
def workspace_root():
    return "/ws"
ctx = {
    "label": {"package": "pkg", "name": "demo", "id": "pkg/demo"},
    "attr": {"_mix_managers": ["mix"], "_mix_package_name": "demo", "app_name": "demo"},
    "srcs": ["deps/demo/**/*"],
    "deps": [],
    "build_dir": ".struo/out/pkg/demo",
    "scratch_dir": ".struo/tmp/pkg/demo",
}
"#;

fn declare(extra: &str) -> Result<AnalysisStore, String> {
    let workspace = TempDir::new().unwrap();
    let source = format!("{}\n{WORKER}\n{extra}", all_prelude_source());
    let (store, result) = with_active_store(store_for(workspace.path(), "pkg"), || {
        eval_prelude_source_to_repr(source)
    });
    result.map(|_| store)
}

const COMPILE: &str = r#"
_elixir_compile_action(ctx, _elixir_compiler_toolchain(), [], [], [], [], [], [],
    ".struo/out/pkg/demo/modules", ".struo/out/pkg/demo/metadata", ".struo/out/pkg/demo/warnings",
    ".struo/out/pkg/demo/consolidated", True)
result = repr(None)
"#;

#[test]
fn remote_elixir_actions_use_image_tools_and_partition_by_image() {
    let first = declare(COMPILE).unwrap();
    let action = action_by_identifier(&first, "pkg/demo:elixir-compile");
    assert_eq!(action.argv[0], "/worker/elixir/bin/elixir");
    assert_eq!(
        action.env["PATH"],
        "/worker/elixir/bin:/worker/otp/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    );
    let second = declare(&format!(
        "worker['environment'] = 'image-b'\nexecution['environment'] = 'image-b'\n{COMPILE}"
    ))
    .unwrap();
    assert_ne!(
        action.toolchain_identity,
        action_by_identifier(&second, "pkg/demo:elixir-compile").toolchain_identity
    );

    let rebar = declare(
        r#"
result = repr(_mix_rebar_package_action(ctx, "deps/demo", "demo", "prod", [],
    ".struo/out/pkg/demo/app", ".struo/out/pkg/demo/app/ebin",
    ".struo/out/pkg/demo/compile.log", ".struo/out/pkg/demo/warnings.log", "demo"))
"#,
    )
    .unwrap();
    assert!(rebar
        .actions
        .iter()
        .any(|a| a.argv.first().map(String::as_str) == Some("/worker/mix/rebar3")));
    assert!(rebar
        .actions
        .iter()
        .any(|a| a.argv.first().map(String::as_str) == Some("/worker/elixir/bin/elixir")));

    let mix = declare(
        r#"
def host_file_exists(path):
    return False
ctx['capability'] = 'build'
result = repr(_mix_project_impl(ctx))
"#,
    )
    .unwrap();
    assert_eq!(
        action_by_identifier(&mix, "pkg/demo:mix-compile").argv[0],
        "/worker/elixir/bin/elixir"
    );
}

#[test]
fn remote_elixir_rejects_missing_mismatched_and_unsafe_toolchains() {
    for (change, expected) in [
        (
            "execution['properties'] = {}",
            "image-bound toolchain description",
        ),
        ("worker['environment'] = 'image-b'", "must match"),
        (
            "worker['elixir_bin'] = '/worker/../coordinator'",
            "absolute worker path",
        ),
        ("worker['rebar3'] = 'rebar3'", "absolute worker path"),
        ("worker['arch'] = 'unknown'", "arch arm64 or x86_64"),
    ] {
        let error = declare(&format!("{change}\n{COMPILE}")).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
}
