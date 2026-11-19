use std::fs;

use serde_json::json;
use tempfile::TempDir;

fn write_seed(workspace: &TempDir, graph: &serde_json::Value) {
    fs::write(
        workspace.path().join("struo.toml"),
        "[[target]]\nname = 'turbo'\nkind = 'turbo_workspace'\n[target.attrs]\ngraph_file = 'graph.json'\n",
    )
    .unwrap();
    fs::write(workspace.path().join("graph.json"), graph.to_string()).unwrap();
}

fn task(id: &str, directory: &str, command: &str, dependencies: &[&str]) -> serde_json::Value {
    let (package, name) = id.rsplit_once('#').unwrap();
    json!({
        "taskId": id, "package": package, "task": name, "directory": directory,
        "command": command, "dependencies": dependencies,
        "resolvedTaskDefinition": {"outputs": [], "cache": true, "persistent": false},
        "environmentVariables": {"specified": {"env": [], "passThroughEnv": null}, "configured": [], "inferred": [], "passthrough": null},
    })
}

#[test]
fn resolver_bridges_scriptless_tasks_and_maps_package_paths() {
    let workspace = TempDir::new().unwrap();
    for path in ["apps/web", "packages/ui", "packages/config"] {
        fs::create_dir_all(workspace.path().join(path)).unwrap();
        fs::write(workspace.path().join(path).join("package.json"), "{}").unwrap();
    }
    let mut web_build = task(
        "web#build",
        "apps/web",
        "next build",
        &["ui#build", "config#build"],
    );
    web_build["resolvedTaskDefinition"]["outputs"] = json!([
        ".next/**",
        "!.next/cache/**",
        "$TURBO_ROOT$/reports/web.json"
    ]);
    web_build["environmentVariables"] = json!({
        "specified": {"env": ["API_URL", "NEXT_PUBLIC_*"], "passThroughEnv": ["AWS_TOKEN"]},
        "configured": ["NEXT_PUBLIC_HOST=abc"], "inferred": [], "passthrough": null,
    });
    let mut deploy = task("web#deploy", "apps/web", "vercel deploy", &["web#build"]);
    deploy["resolvedTaskDefinition"]["cache"] = json!(false);
    let mut dev = task("web#dev", "apps/web", "next dev", &[]);
    dev["resolvedTaskDefinition"]["persistent"] = json!(true);
    let graph = json!({
        "packages": [
            {"name": "//", "path": "", "allDependencies": {"items": [{"name": "//"}]}},
            {"name": "web", "path": "apps/web", "allDependencies": {"items": [{"name": "//"}, {"name": "ui"}, {"name": "config"}]}},
            {"name": "ui", "path": "packages/ui", "allDependencies": {"items": [{"name": "//"}, {"name": "config"}]}},
            {"name": "config", "path": "packages/config", "allDependencies": {"items": [{"name": "//"}]}},
        ],
        "dry": {
            "globalCacheInputs": {
                "files": {".env": "hash"},
                "environmentVariables": {"specified": {"env": ["CI_FLAG"]}, "configured": []},
            },
            "tasks": [
                web_build,
                deploy,
                dev,
                // `config` has no build script: its task is a transit node that
                // web must see through to reach what it depends on.
                task("config#build", "packages/config", "<NONEXISTENT>", &["config#generate"]),
                task("config#generate", "packages/config", "node generate.js", &[]),
                task("ui#build", "packages/ui", "tsc", &["config#build"]),
                task("//#format", "", "prettier --check .", &[]),
            ],
        },
    });
    write_seed(&workspace, &graph);

    let targets = struo_frontend::load_graph_workspace(workspace.path()).unwrap();
    let mut names = targets
        .iter()
        .map(|target| target.label.id.as_str())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "config__generate",
            "root__format",
            "turbo",
            "ui__build",
            "web__build",
            "web__deploy"
        ]
    );
    let find = |id: &str| targets.iter().find(|target| target.label.id == id).unwrap();
    let attr = |id: &str, name: &str| serde_json::to_value(&find(id).attrs[name]).unwrap();

    let mut web_deps = find("web__build").deps.clone();
    web_deps.sort_unstable();
    assert_eq!(web_deps, ["config__generate", "ui__build"]);
    assert_eq!(
        attr("web__build", "outputs"),
        json!([
            "apps/web/.next/**",
            "!apps/web/.next/cache/**",
            "reports/web.json"
        ])
    );
    assert_eq!(
        attr("web__build", "source_dirs"),
        json!(["apps/web", "packages/ui", "packages/config"])
    );
    assert_eq!(attr("root__format", "source_dirs"), json!([""]));
    assert_eq!(attr("web__build", "other_packages"), json!([]));
    assert_eq!(attr("ui__build", "other_packages"), json!(["web"]));
    assert_eq!(
        attr("web__build", "env_inherit"),
        json!(["CI_FLAG", "API_URL", "AWS_TOKEN", "NEXT_PUBLIC_HOST"])
    );
    assert_eq!(attr("web__build", "global_inputs"), json!([".env"]));
    assert_eq!(attr("web__deploy", "cache"), json!(false));
    assert_eq!(attr("web__build", "package"), json!("web"));

    let diagnostics = struo_frontend::validate_workspace(workspace.path()).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

// A remote worker receives only an action's declared inputs. Running each
// action in an empty directory populated from those inputs proves the task
// carries its sources and its dependencies' outputs, and that it uses no
// command setup a remote provider would reject.
#[cfg(unix)]
#[test]
fn tasks_run_from_declared_inputs_alone_and_snapshot_matching_outputs() {
    use std::path::Path;
    use std::process::Command;
    use struo_frontend::analysis::{AnalysisEngine, DeclaredAction};

    let workspace = TempDir::new().unwrap();
    let root = workspace.path();
    fs::create_dir_all(root.join("packages/lib/src")).unwrap();
    fs::create_dir_all(root.join("apps/app")).unwrap();
    fs::write(root.join("packages/lib/package.json"), "{}").unwrap();
    fs::write(root.join("apps/app/package.json"), "{}").unwrap();
    fs::write(root.join("packages/lib/src/value.txt"), "42").unwrap();
    fs::write(root.join("tsconfig.base.json"), "{}").unwrap();
    fs::create_dir_all(root.join("node_modules")).unwrap();
    std::os::unix::fs::symlink("../packages/lib", root.join("node_modules/lib")).unwrap();
    std::os::unix::fs::symlink("../apps/app", root.join("node_modules/app")).unwrap();
    fs::create_dir_all(root.join("node_modules/.pnpm/node_modules")).unwrap();
    std::os::unix::fs::symlink(
        "../../../apps/app",
        root.join("node_modules/.pnpm/node_modules/app"),
    )
    .unwrap();
    fs::write(root.join("README.md"), "not an input").unwrap();
    fs::write(
        root.join("packages/lib/build.js"),
        "const fs = require('node:fs');\nfs.mkdirSync('dist/maps', { recursive: true });\nfs.writeFileSync('dist/value.txt', fs.readFileSync('src/value.txt'));\nfs.writeFileSync('dist/maps/value.map', 'map');\nfs.writeFileSync('build.log', 'log');\n",
    )
    .unwrap();
    fs::write(
        root.join("apps/app/build.js"),
        "const fs = require('node:fs');\nfs.mkdirSync('out', { recursive: true });\nfs.writeFileSync('out/app.txt', 'app ' + fs.readFileSync(require.resolve('lib/dist/value.txt')) + ' ' + fs.existsSync('../../tsconfig.base.json'));\n",
    )
    .unwrap();
    let mut lib = task("lib#build", "packages/lib", "node build.js", &[]);
    lib["resolvedTaskDefinition"]["outputs"] = json!(["dist/**", "!dist/maps/**"]);
    let mut app = task("app#build", "apps/app", "node build.js", &["lib#build"]);
    app["resolvedTaskDefinition"]["outputs"] = json!(["out/**"]);
    write_seed(
        &workspace,
        &json!({
            "packages": [
                {"name": "lib", "path": "packages/lib", "allDependencies": {"items": []}},
                {"name": "app", "path": "apps/app", "allDependencies": {"items": [{"name": "lib"}]}},
            ],
            "dry": {"tasks": [lib, app]},
        }),
    );

    let engine = AnalysisEngine::for_workspace(root).unwrap();
    let targets = engine.load_graph_workspace(root).unwrap();
    let analyze = |id: &str, deps: &[serde_json::Value]| {
        let target = targets.iter().find(|t| t.label.id == id).unwrap();
        let mut result = engine
            .analyze_target_capability(target, root, deps, "build")
            .unwrap();
        let provider = result.provider.clone();
        let action = result.actions.remove(
            result
                .actions
                .iter()
                .position(|a| !a.argv.is_empty())
                .unwrap(),
        );
        assert!(action.clean_paths.is_empty() && action.create_dirs.is_empty());
        assert!(action.stdout.is_none() && action.stderr.is_none());
        (action, provider)
    };
    let copy_tree = |from: &Path, to: &Path| {
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        let status = Command::new("cp")
            .arg("-R")
            .arg(from)
            .arg(to)
            .status()
            .unwrap();
        assert!(status.success());
    };
    let run_isolated = |action: &DeclaredAction, worker: &Path| {
        for input in &action.inputs {
            let relative = Path::new(input)
                .strip_prefix(root)
                .unwrap_or(Path::new(input));
            if root.join(relative).exists() && !worker.join(relative).exists() {
                copy_tree(&root.join(relative), &worker.join(relative));
            }
        }
        let output = Command::new(&action.argv[0])
            .args(&action.argv[1..])
            .current_dir(worker)
            .envs(&action.env)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    };

    let (lib_action, lib_provider) = analyze("lib__build", &[]);
    let lib_worker = TempDir::new().unwrap();
    run_isolated(&lib_action, lib_worker.path());
    let declares = |action: &DeclaredAction, path: &str| {
        action.inputs.iter().any(|input| input.ends_with(path))
    };
    assert!(!declares(&lib_action, "README.md"));
    assert!(declares(&lib_action, "node_modules/lib"));
    assert!(!declares(&lib_action, "node_modules/app"));
    assert!(!declares(&lib_action, ".pnpm/node_modules/app"));
    let snapshot = &lib_action.outputs[0];
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(lib_worker.path().join(snapshot).join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest,
        json!([{"path": "packages/lib/dist/value.txt", "kind": "file"}])
    );

    // The coordinator receives only the declared snapshot back from the worker.
    copy_tree(&lib_worker.path().join(snapshot), &root.join(snapshot));

    let (app_action, _) = analyze("app__build", &[lib_provider]);
    assert!(!app_action
        .inputs
        .iter()
        .any(|input| input.contains("packages/lib/dist")));
    let app_worker = TempDir::new().unwrap();
    run_isolated(&app_action, app_worker.path());
    assert_eq!(
        fs::read_to_string(app_worker.path().join("apps/app/out/app.txt")).unwrap(),
        "app 42 true"
    );
    let app_manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(
            app_worker
                .path()
                .join(&app_action.outputs[0])
                .join("manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        app_manifest,
        json!([{"path": "apps/app/out", "kind": "tree"}])
    );
}
