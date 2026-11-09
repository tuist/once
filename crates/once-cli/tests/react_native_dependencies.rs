#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn dependency_cache_restores_lifecycle_generated_files() {
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let node = Command::new("node")
        .args(["-p", "process.execPath"])
        .output()
        .expect("Node.js required for React Native dependency regression");
    assert!(node.status.success(), "{node:?}");
    let node = String::from_utf8(node.stdout).unwrap().trim().to_string();
    let manager = workspace.path().join("npm");
    std::fs::write(
        &manager,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 11.0.0; else node install.js; fi\n",
    )
    .unwrap();
    std::fs::set_permissions(&manager, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(workspace.path().join("package.json"), "{}\n").unwrap();
    std::fs::write(
        workspace.path().join("package-lock.json"),
        r#"{"lockfileVersion":3,"packages":{"node_modules/react-native":{"version":"0.86.0"},"node_modules/hermes-compiler":{"version":"0.14.0"}}}"#,
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("install.js"),
        "const fs = require('fs'); fs.mkdirSync('node_modules', {recursive:true}); fs.mkdirSync('generated', {recursive:true}); fs.writeFileSync('generated/token.txt', require('crypto').randomUUID());\n",
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("once.toml"),
        format!(
            "[[target]]\nname = \"Dependencies\"\nkind = \"react_native_dependencies\"\nsrcs = [\"install.js\"]\n\n[target.attrs]\nresolver_inputs = [\"package.json\", \"package-lock.json\"]\nnode = {node:?}\npackage_manager_executable = {:?}\n",
            manager.to_string_lossy()
        ),
    )
    .unwrap();
    let generated = workspace
        .path()
        .join(".once/out/Dependencies/javascript/generated/token.txt");
    let mut expected = None;
    for delete_outputs in [false, false, true] {
        if delete_outputs {
            std::fs::remove_dir_all(workspace.path().join(".once/out")).unwrap();
        }
        let output = Command::new(env!("CARGO_BIN_EXE_once"))
            .arg("-C")
            .arg(workspace.path())
            .args(["--memory-limit", "512MiB", "build", "Dependencies"])
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
        let token = std::fs::read_to_string(&generated)
            .expect("lifecycle-generated file must survive cache replay");
        assert_eq!(expected.get_or_insert_with(|| token.clone()), &token);
    }
}
