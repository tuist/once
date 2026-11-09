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
