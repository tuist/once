use std::path::PathBuf;

#[test]
fn production_test_orchestration_stays_ecosystem_neutral() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sources = [
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
        "../once-core/src/test_manifest/mod.rs",
        "../once-core/src/test_plan.rs",
        "../once-core/src/test_results/mod.rs",
        "../once-core/src/test_schedule/mod.rs",
        "../once-core/src/test_schedule/store.rs",
        "../once-frontend/src/analysis/engine.rs",
        "../once-frontend/src/module_contract.rs",
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
    }
}

#[test]
fn module_contract_result_example_satisfies_the_generic_validator() {
    let contract = once_frontend::module_authoring_contract();
    once_core::validate_test_results(&contract.normalized_test_result_example, "scripted_tests")
        .unwrap();
}
