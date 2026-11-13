use std::fs;

use serde_json::json;
use tempfile::TempDir;

#[test]
fn task_filter_preserves_transitive_dependencies_without_including_unrelated_tasks() {
    let workspace = TempDir::new().unwrap();
    for path in ["apps/app", "libs/bridge", "libs/lib"] {
        fs::create_dir_all(workspace.path().join(path)).unwrap();
        fs::write(workspace.path().join(path).join("index.js"), "").unwrap();
    }
    fs::write(
        workspace.path().join("once.toml"),
        r#"
[[target]]
name = "nx"
kind = "nx_workspace"
[target.attrs]
graph_file = "graph.json"
targets = ["build"]
"#,
    )
    .unwrap();
    let graph = json!({"graph": {
        "nodes": {
            "app": {"type": "app", "data": {"root": "apps/app", "targets": {
                "build": {
                    "dependsOn": ["compile", "^build", "absent", "^absent"],
                    "executor": "nx:run-commands",
                    "options": {
                        "commands": [{"command": "setup", "forwardAllArgs": false}, "compile"],
                        "args": ["--mode=production", "--workers=1"]
                    }
                },
                "compile": {"dependsOn": [
                    {"target": "generate", "projects": ["lib"]},
                    {"target": "generate", "projects": "dependencies"}
                ]},
                "deploy": {}
            }}},
            "bridge": {"type": "lib", "data": {"root": "libs/bridge", "targets": {}}},
            "lib": {"type": "lib", "data": {"root": "libs/lib", "targets": {
                "build": {"dependsOn": ["generate"]},
                "generate": {"executor": "nx:run-script", "metadata": {"runCommand": "npm run generate"}},
                "release": {}
            }}}
        },
        "dependencies": {
            "app": [{"target": "bridge", "type": "static"}],
            "bridge": [{"target": "lib", "type": "static"}, {"target": "app", "type": "static"}]
        }
    }});
    fs::write(workspace.path().join("graph.json"), graph.to_string()).unwrap();

    let targets = once_frontend::load_graph_workspace(workspace.path()).unwrap();
    let mut names = targets
        .iter()
        .map(|target| target.label.id.as_str())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "app__build",
            "app__compile",
            "lib__build",
            "lib__generate",
            "nx"
        ]
    );
    let app_build = targets
        .iter()
        .find(|target| target.label.id == "app__build")
        .unwrap();
    assert!(app_build.deps.contains(&"lib__build".to_owned()));
    assert_eq!(
        serde_json::to_value(&app_build.attrs["commands"]).unwrap(),
        json!(["setup", "compile --mode=production --workers=1"])
    );
    let generator = targets
        .iter()
        .find(|target| target.label.id == "lib__generate")
        .unwrap();
    assert_eq!(
        serde_json::to_value(&generator.attrs["command_cwd"]).unwrap(),
        json!("libs/lib")
    );
    let diagnostics = once_frontend::validate_workspace(workspace.path()).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn plugin_outputs_resolve_nested_default_configuration_options() {
    let workspace = TempDir::new().unwrap();
    fs::create_dir_all(workspace.path().join("app")).unwrap();
    fs::write(workspace.path().join("app/index.js"), "").unwrap();
    fs::write(
        workspace.path().join("once.toml"),
        "[[target]]\nname = 'nx'\nkind = 'nx_workspace'\n[target.attrs]\ngraph_file = 'graph.json'\n",
    )
    .unwrap();
    let graph = json!({"nodes": {"app": {"type": "app", "data": {
        "root": "app", "targets": {"build": {
            "executor": "example:compile",
            "defaultConfiguration": "production",
            "options": {"outputPath": {"base": "dist/debug"}},
            "configurations": {"production": {"outputPath": {"base": "dist/release"}}},
            "inputs": [{"env": "BUILD_MODE"}],
            "outputs": ["{options.outputPath.base}"]
        }}
    }}}, "dependencies": {}});
    fs::write(workspace.path().join("graph.json"), graph.to_string()).unwrap();
    let targets = once_frontend::load_graph_workspace(workspace.path()).unwrap();
    let app = targets.iter().find(|t| t.label.id == "app__build").unwrap();
    assert_eq!(
        serde_json::to_value(&app.attrs["runnable"]).unwrap(),
        json!(true)
    );
    assert_eq!(
        serde_json::to_value(&app.attrs["outputs"]).unwrap(),
        json!(["dist/release"])
    );
    assert_eq!(
        serde_json::to_value(&app.attrs["env_inherit"]).unwrap(),
        json!(["BUILD_MODE"])
    );
    let encoded = serde_json::to_value(&app.attrs["executor_graph"]).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(encoded.as_str().unwrap()).unwrap(),
        graph
    );
    let diagnostics = once_frontend::validate_workspace(workspace.path()).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[cfg(unix)]
#[test]
fn test_tasks_report_suite_success_and_failure_without_hashing_generated_outputs() {
    use once_frontend::analysis::AnalysisEngine;
    use std::process::Command;

    let workspace = TempDir::new().unwrap();
    fs::create_dir_all(workspace.path().join("app/generated")).unwrap();
    fs::create_dir_all(workspace.path().join("shared")).unwrap();
    fs::write(workspace.path().join("shared/config.json"), "{}").unwrap();
    fs::write(
        workspace.path().join("app/generated/result.txt"),
        "generated",
    )
    .unwrap();
    fs::write(
        workspace.path().join("once.toml"),
        "[[target]]\nname = 'nx'\nkind = 'nx_workspace'\n[target.attrs]\ngraph_file = 'graph.json'\n",
    ).unwrap();
    for (exit_code, expected) in [(0, "passed"), (7, "failed")] {
        let graph = json!({"nodes": {"app": {"type": "app", "data": {
            "root": "app", "targets": {
                "test": {"executor": "nx:run-commands", "options": {"cwd": "app", "command": format!("exit {exit_code}")}},
                "generate": {"outputs": ["{projectRoot}/generated"]}
            }
        }}}, "dependencies": {}});
        fs::write(workspace.path().join("graph.json"), graph.to_string()).unwrap();
        let engine = AnalysisEngine::for_workspace(workspace.path()).unwrap();
        let targets = engine.load_graph_workspace(workspace.path()).unwrap();
        let target = targets.iter().find(|t| t.label.id == "app__test").unwrap();
        assert_eq!(target.kind, "nx_test");
        assert_eq!(
            target
                .capabilities
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["build", "test"]
        );
        let seed = targets.iter().find(|t| t.label.id == "nx").unwrap();
        assert_eq!(
            serde_json::to_value(&seed.attrs["_default_test_roots"]).unwrap(),
            json!(["app__test"])
        );
        let result = engine
            .analyze_target_capability(target, workspace.path(), &[], "test")
            .unwrap();
        let info = &result.provider["test_info"];
        assert_eq!(info["sharding"]["granularity"], "target");
        assert_eq!(info["listing"]["supported"], false);
        let action = result.actions.iter().find(|a| !a.argv.is_empty()).unwrap();
        assert!(action
            .inputs
            .iter()
            .any(|p| p.ends_with("shared/config.json")));
        assert!(!action
            .inputs
            .iter()
            .any(|p| p.ends_with("generated/result.txt")));
        let results = workspace
            .path()
            .join(info["outputs"]["results"].as_str().unwrap());
        fs::create_dir_all(results.parent().unwrap()).unwrap();
        let output = Command::new(&action.argv[0])
            .args(&action.argv[1..])
            .current_dir(workspace.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(exit_code), "{output:?}");
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(results).unwrap()).unwrap();
        assert_eq!(report["status"], expected);
        assert_eq!(report["summary"]["total"], 1);
        assert_eq!(report["cases"][0]["status"], expected);
    }
}
