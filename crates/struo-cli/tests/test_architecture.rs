use std::path::PathBuf;

#[test]
fn production_test_orchestration_stays_ecosystem_neutral() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sources = [
        "src/argv_normalize.rs",
        "src/bus_events.rs",
        "src/commands/graph/mod.rs",
        "src/commands/mcp.rs",
        "src/commands/mcp/tools.rs",
        "src/commands/query.rs",
        "src/commands/query/test_plan.rs",
        "src/commands/query/test_plan/inputs.rs",
        "src/commands/query/test_plan/selection.rs",
        "src/commands/test_schedule/executor.rs",
        "src/commands/test_schedule/mod.rs",
        "src/commands/test_schedule/process.rs",
        "src/commands/test_schedule/worker.rs",
        "src/dispatch.rs",
        "../struo-core/src/test_manifest/mod.rs",
        "../struo-core/src/test_plan.rs",
        "../struo-core/src/test_results/mod.rs",
        "../struo-core/src/test_schedule/mod.rs",
        "../struo-core/src/test_schedule/store.rs",
        "../struo-core/src/events.rs",
        "../struo-events-client/src/bridge.rs",
        "../struo-events-client/src/buffer.rs",
        "../struo-events-client/src/dashboard.rs",
        "../struo-events-client/src/loss.rs",
        "../struo-events-client/src/session.rs",
        "../struo-events-client/src/transport.rs",
        "../struo-frontend/src/analysis/engine.rs",
        "../struo-frontend/src/module_contract.rs",
    ];
    let forbidden = [
        "android_",
        "apple_",
        "cargo",
        "elixir_",
        "gradle",
        "junit",
        "kotlinc",
        "libtest",
        "npm",
        "shellspec_",
        "swift",
        "xcode",
        "zig_",
    ];
    let target_kinds = struo_frontend::built_in_target_kind_schemas();

    for source in sources {
        let path = root.join(source);
        let contents = std::fs::read_to_string(&path).unwrap();
        let production = contents.split("#[cfg(test)]").next().unwrap();
        let lower = production
            .replace("CARGO_PKG_VERSION", "")
            .to_ascii_lowercase();
        for term in forbidden {
            assert!(
                !lower.contains(term),
                "production test orchestration in `{}` contains ecosystem-specific term `{term}`",
                path.display()
            );
        }
        for schema in &target_kinds {
            if schema.kind == "script" {
                continue;
            }
            let quoted = format!("\"{}\"", schema.kind.to_ascii_lowercase());
            assert!(
                !lower.contains(&quoted),
                "production test orchestration in `{}` names Starlark target kind `{}`",
                path.display(),
                schema.kind
            );
        }
    }

    let commands = root.join("src/commands");
    for entry in std::fs::read_dir(commands).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        assert!(
            !name.starts_with("test_events_") || name == "test_events.rs",
            "test event translation belongs in target kinds, found `{name}`"
        );
    }
}

#[test]
fn module_contract_result_example_satisfies_the_generic_validator() {
    let contract = struo_frontend::module_authoring_contract();
    struo_core::validate_test_results(&contract.normalized_test_result_example, "scripted_tests")
        .unwrap();
}

#[test]
fn compatibility_adapters_do_not_name_starlark_target_kinds() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sources = [
        "src/commands/bazel/invocation.rs",
        "src/commands/cargo/invocation.rs",
        "src/commands/swift/invocation.rs",
        "src/commands/xcodebuild/invocation.rs",
    ];
    let target_kinds = struo_frontend::built_in_target_kind_schemas();

    for source in sources {
        let path = root.join(source);
        let contents = std::fs::read_to_string(&path).unwrap();
        let production = contents.split("#[cfg(test)]").next().unwrap();
        for schema in &target_kinds {
            if schema.kind == "script" {
                continue;
            }
            let quoted = format!("\"{}\"", schema.kind);
            assert!(
                !production.contains(&quoted),
                "compatibility adapter `{}` names Starlark target kind `{}`",
                path.display(),
                schema.kind
            );
        }
    }
}
