use super::prelude_schemas::{attr, parse_target_kind_schemas};
use super::*;
use crate::error::Error;
use crate::target::Target;

fn source_with_common(source: &str) -> String {
    format!("{}\n{source}", crate::modules::common_module_source())
}

#[test]
fn parse_target_kind_schemas_rejects_invalid_syntax() {
    let err = parse_target_kind_schemas("test.star", "demo_kind = target_kind(").unwrap_err();
    assert!(matches!(err, Error::Eval { .. }));
}

#[test]
fn parse_target_kind_schemas_requires_target_kind_exports() {
    let err = parse_target_kind_schemas("test.star", "OTHER = []").unwrap_err();
    match err {
        Error::Eval { message, .. } => {
            assert!(message.contains("no target kind symbols exported"));
        }
        other => panic!("expected Error::Eval, got {other:?}"),
    }
}

#[test]
fn parse_target_kind_schemas_rejects_invalid_kind_type() {
    let source = source_with_common(r#"demo_kind = target_kind(kind = 7, docs = "Demo kind")"#);
    let err = parse_target_kind_schemas("test.star", &source).unwrap_err();
    match err {
        Error::Eval { message, .. } => {
            assert!(message.contains("kind should be a string or None"));
        }
        other => panic!("expected Error::Eval, got {other:?}"),
    }
}

#[test]
fn parse_target_kind_schemas_reports_missing_target_kind_field() {
    let err = parse_target_kind_schemas("test.star", r#"demo_kind = {"_struo_target_kind": True}"#)
        .unwrap_err();
    match err {
        Error::Eval { message, .. } => assert!(message.contains("is missing")),
        other => panic!("expected Error::Eval, got {other:?}"),
    }
}

#[test]
fn parse_target_kind_schemas_accepts_minimal_valid_target_kind() {
    let source = source_with_common(r#"demo = target_kind(docs = "Demo kind")"#);
    let schemas = parse_target_kind_schemas("test.star", &source).unwrap();
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].kind, "demo");
}

#[test]
fn parse_target_kind_schemas_exposes_tool_requirements() {
    let source = source_with_common(
        r#"demo = target_kind(
docs = "Demo kind",
tools = [tool("rust", executables = ["rustc", "cargo"])],
)"#,
    );
    let schemas = parse_target_kind_schemas("test.star", &source).unwrap();

    assert_eq!(
        schemas[0].tools,
        vec![ToolRequirement {
            name: "rust".to_string(),
            executables: vec!["rustc".to_string(), "cargo".to_string()],
        }]
    );
}

#[test]
fn parse_target_kind_schemas_exposes_allowed_attribute_values() {
    let source = source_with_common(
        r#"demo = target_kind(
docs = "Demo kind",
attrs = [
    attr("mode", "string", allowed_values = ["fast", "small"]),
],
)"#,
    );
    let schemas = parse_target_kind_schemas("test.star", &source).unwrap();

    assert_eq!(
        schemas[0].attrs[0].allowed_values,
        vec!["fast".to_string(), "small".to_string()]
    );
}

#[test]
fn parse_target_kind_schemas_exposes_disallowed_attribute_values() {
    let source = source_with_common(
        r#"demo = target_kind(
docs = "Demo kind",
attrs = [
    attr("mode", "string", disallowed_values = ["", "all"]),
],
)"#,
    );
    let schemas = parse_target_kind_schemas("test.star", &source).unwrap();

    assert_eq!(
        schemas[0].attrs[0].disallowed_values,
        vec![String::new(), "all".to_string()]
    );
}

#[test]
fn parse_target_kind_schemas_rejects_allowed_values_on_non_string_attributes() {
    let source = source_with_common(
        r#"demo = target_kind(
docs = "Demo kind",
attrs = [
    attr("count", "int", allowed_values = ["1", "2"]),
],
)"#,
    );

    let error = parse_target_kind_schemas("test.star", &source).unwrap_err();

    assert!(error
        .to_string()
        .contains("supported only for string and target attributes"));
}

#[test]
fn parse_target_kind_schemas_rejects_disallowed_values_on_non_string_attributes() {
    let source = source_with_common(
        r#"demo = target_kind(
docs = "Demo kind",
attrs = [
    attr("count", "int", disallowed_values = ["0"]),
],
)"#,
    );

    let error = parse_target_kind_schemas("test.star", &source).unwrap_err();

    assert!(error
        .to_string()
        .contains("supported only for string and target attributes"));
}

#[test]
fn parse_target_kind_schemas_exposes_dependency_count_bounds() {
    let source = source_with_common(
        r#"demo = target_kind(
docs = "Demo kind",
deps = [
    dep("deps", ["provider"], min_count = 1, max_count = 1),
],
)"#,
    );
    let schemas = parse_target_kind_schemas("test.star", &source).unwrap();

    assert_eq!(schemas[0].deps[0].min_count, 1);
    assert_eq!(schemas[0].deps[0].max_count, Some(1));
}

#[test]
fn parse_target_kind_schemas_rejects_invalid_dependency_count_bounds() {
    let source = source_with_common(
        r#"demo = target_kind(
docs = "Demo kind",
deps = [
    dep("deps", ["provider"], min_count = 2, max_count = 1),
],
)"#,
    );

    let error = parse_target_kind_schemas("test.star", &source).unwrap_err();

    assert!(error.to_string().contains("min_count must not exceed"));
}

#[test]
fn parse_target_kind_schemas_exposes_source_references() {
    let source = source_with_common(
        r#"demo = target_kind(
docs = "Demo kind",
source_references = [
    source_reference(
        "Example Build",
        "example_library",
        "https://example.com/example_library",
        "Use when adopting an existing example_library target.",
        content_digest = "0123456789abcdef",
    ),
],
)"#,
    );
    let schemas = parse_target_kind_schemas("test.star", &source).unwrap();

    assert_eq!(
        schemas[0].source_references,
        vec![SourceReference {
            system: "Example Build".to_string(),
            symbol: "example_library".to_string(),
            url: "https://example.com/example_library".to_string(),
            use_when: "Use when adopting an existing example_library target.".to_string(),
            content_digest: Some("0123456789abcdef".to_string()),
        }]
    );
}

#[test]
fn parse_target_kind_schemas_accepts_legacy_rule_alias() {
    let source = source_with_common(r#"demo = rule(docs = "Demo kind")"#);
    let schemas = parse_target_kind_schemas("test.star", &source).unwrap();
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].kind, "demo");
}

#[test]
fn workspace_module_paths_extend_graph_schemas() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("modules")).unwrap();
    std::fs::write(
        tmp.path().join("struo.toml"),
        r#"
[modules]
paths = ["modules/*.star"]

[[target]]
name = "Hello"
kind = "demo_kind"
"#,
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("modules/demo.star"),
        r#"
demo_kind = target_kind(
docs = "Demo kind",
attrs = [],
deps = [],
providers = ["demo_provider"],
capabilities = [
    capability("build", ["default"]),
],
)
"#,
    )
    .unwrap();

    let graph = load_graph_workspace(tmp.path()).unwrap();

    assert_eq!(graph.len(), 1);
    assert_eq!(graph[0].kind, "demo_kind");
    assert_eq!(graph[0].providers, vec!["demo_provider"]);
    assert_eq!(graph[0].capabilities[0].name, "build");
    assert!(graph[0].diagnostics.is_empty());
}

#[test]
fn workspace_target_kind_examples_resolve_relative_to_module_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("modules/examples/demo/src")).unwrap();
    std::fs::write(
        tmp.path().join("struo.toml"),
        "[modules]\npaths = [\"modules/*.star\"]\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("modules/demo.star"),
        r#"
demo_kind = target_kind(
docs = "Demo kind",
examples = [
    example(
        "minimal",
        name = "Minimal demo",
        use_when = "Start a minimal demo target.",
        path = "examples/demo",
    ),
],
)
"#,
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("modules/examples/demo/struo.toml"),
        "[[target]]\nname = \"Demo\"\nkind = \"demo_kind\"\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("modules/examples/demo/src/main.txt"),
        "hello\n",
    )
    .unwrap();

    let schema = target_kind_schemas_for_workspace(tmp.path())
        .unwrap()
        .into_iter()
        .find(|schema| schema.kind == "demo_kind")
        .unwrap();
    assert_eq!(schema.examples[0].slug, "minimal");

    let bundle = crate::examples::load_target_kind_example(&schema, "minimal").unwrap();

    assert_eq!(bundle.name, "Minimal demo");
    assert!(bundle.files.iter().any(|file| file.path == "struo.toml"));
    assert!(bundle.files.iter().any(|file| file.path == "src/main.txt"));
}

#[test]
fn workspace_target_kind_examples_reject_paths_outside_module_package() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("modules")).unwrap();
    std::fs::write(
        tmp.path().join("struo.toml"),
        "[modules]\npaths = [\"modules/*.star\"]\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("modules/demo.star"),
        r#"
demo_kind = target_kind(
docs = "Demo kind",
examples = [
    example(
        "bad",
        name = "Bad demo",
        use_when = "This should fail.",
        path = "../examples/bad",
    ),
],
)
"#,
    )
    .unwrap();

    let err = target_kind_schemas_for_workspace(tmp.path()).unwrap_err();

    assert!(err
        .to_string()
        .contains("must stay inside the module package"));
}

#[test]
fn workspace_module_paths_reject_duplicate_kinds() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("modules")).unwrap();
    std::fs::write(
        tmp.path().join("struo.toml"),
        "[modules]\npaths = [\"modules/*.star\"]\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("modules/demo.star"),
        r#"
demo_kind = target_kind(
docs = "Duplicate",
attrs = [],
deps = [],
providers = [],
capabilities = [],
)
"#,
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("modules/other.star"),
        r#"
demo_kind = target_kind(
docs = "Duplicate",
attrs = [],
deps = [],
providers = [],
capabilities = [],
)
"#,
    )
    .unwrap();

    let err = target_kind_schemas_for_workspace(tmp.path()).unwrap_err();

    assert!(err.to_string().contains("declared more than once"));
}

#[test]
fn unknown_kind_gets_diagnostic_and_no_capabilities() {
    let target = Target {
        package: "pkg".to_string(),
        kind: "mystery_kind".to_string(),
        name: "Thing".to_string(),
        deps: Vec::new(),
        dependency_edges: BTreeMap::new(),
        srcs: Vec::new(),
        visibility: Vec::new(),
        attrs: BTreeMap::new(),
        typed_attrs: BTreeMap::new(),
        resolver_input_exclude: Vec::new(),
    };

    let graph = graph_from_targets(&[target]);
    let thing = &graph[0];
    assert!(thing.capabilities.is_empty());
    assert!(thing.providers.is_empty());
    assert_eq!(thing.diagnostics.len(), 1);
    assert_eq!(thing.diagnostics[0].code, "unknown_target_kind");
    assert!(thing.diagnostics[0]
        .message
        .contains("`mystery_kind` has no target kind schema"));
}

#[test]
fn graph_attrs_fall_back_to_string_attrs_when_untyped() {
    let mut attrs = BTreeMap::new();
    attrs.insert("mode".to_string(), "debug".to_string());
    let target = Target {
        package: "pkg".to_string(),
        kind: "script".to_string(),
        name: "Tool".to_string(),
        deps: Vec::new(),
        dependency_edges: BTreeMap::new(),
        srcs: Vec::new(),
        visibility: Vec::new(),
        attrs,
        typed_attrs: BTreeMap::new(),
        resolver_input_exclude: Vec::new(),
    };

    let graph = graph_from_targets(&[target]);
    assert_eq!(
        graph[0].attrs.get("mode"),
        Some(&AttrValue::String("debug".to_string()))
    );
}

#[test]
fn script_runtime_becomes_a_graph_tool_requirement() {
    let target = Target {
        package: "tools".to_string(),
        kind: "script".to_string(),
        name: "Generate".to_string(),
        deps: Vec::new(),
        dependency_edges: BTreeMap::new(),
        srcs: Vec::new(),
        visibility: Vec::new(),
        attrs: BTreeMap::from([("script_runtime".to_string(), "python".to_string())]),
        typed_attrs: BTreeMap::new(),
        resolver_input_exclude: Vec::new(),
    };

    let graph = graph_from_targets(&[target]);

    assert_eq!(
        graph[0].tools,
        vec![ToolRequirement {
            name: "python".to_string(),
            executables: vec!["python".to_string()],
        }]
    );
}

#[test]
fn graph_from_targets_attaches_built_in_schema_to_each_target() {
    let targets = ["ToolA", "ToolB"].map(|name| Target {
        package: "tools".to_string(),
        kind: "script".to_string(),
        name: name.to_string(),
        deps: Vec::new(),
        dependency_edges: BTreeMap::new(),
        srcs: Vec::new(),
        visibility: Vec::new(),
        attrs: BTreeMap::new(),
        typed_attrs: BTreeMap::new(),
        resolver_input_exclude: Vec::new(),
    });

    let graph = graph_from_targets(&targets);

    assert_eq!(graph.len(), 2);
    for target in graph {
        assert!(target.diagnostics.is_empty());
        assert_eq!(target.providers, vec!["script_action"]);
        assert!(target
            .capabilities
            .iter()
            .any(|capability| capability.name == "run"));
    }
}

#[test]
fn typed_attrs_take_precedence_over_string_attrs() {
    let mut attrs = BTreeMap::new();
    attrs.insert("enabled".to_string(), "false".to_string());
    let mut typed_attrs = BTreeMap::new();
    typed_attrs.insert("enabled".to_string(), AttrValue::Bool(true));
    let target = Target {
        package: "pkg".to_string(),
        kind: "script".to_string(),
        name: "Tool".to_string(),
        deps: Vec::new(),
        dependency_edges: BTreeMap::new(),
        srcs: Vec::new(),
        visibility: Vec::new(),
        attrs,
        typed_attrs,
        resolver_input_exclude: Vec::new(),
    };

    let graph = graph_from_targets(&[target]);
    assert_eq!(graph[0].attrs.get("enabled"), Some(&AttrValue::Bool(true)));
}

#[test]
fn select_on_non_configurable_attribute_emits_a_diagnostic() {
    let mut typed_attrs = BTreeMap::new();
    let mut select_outer = BTreeMap::new();
    let mut branches = BTreeMap::new();
    branches.insert(
        "default".to_string(),
        AttrValue::String("Default".to_string()),
    );
    select_outer.insert("select".to_string(), AttrValue::Map(branches));
    typed_attrs.insert("fixed_name".to_string(), AttrValue::Map(select_outer));
    let target = Target {
        package: "pkg".to_string(),
        kind: "demo_kind".to_string(),
        name: "Thing".to_string(),
        deps: Vec::new(),
        dependency_edges: BTreeMap::new(),
        srcs: Vec::new(),
        visibility: Vec::new(),
        attrs: BTreeMap::new(),
        typed_attrs,
        resolver_input_exclude: Vec::new(),
    };
    let schemas = vec![TargetKindSchema {
        kind: "demo_kind".to_string(),
        docs: "Demo kind".to_string(),
        has_resolver: false,
        attrs: vec![attr(
            "fixed_name",
            "string",
            false,
            None,
            "Fixed target name",
            false,
        )],
        deps: Vec::new(),
        providers: Vec::new(),
        capabilities: Vec::new(),
        tools: Vec::new(),
        source_references: Vec::new(),
        examples: Vec::new(),
    }];

    let graph = graph_from_targets_with_schemas(&[target], &schemas);
    let diag = graph[0]
        .diagnostics
        .iter()
        .find(|d| d.code == "select_on_non_configurable_attr")
        .expect("expected select_on_non_configurable_attr diagnostic");
    assert!(diag.message.contains("fixed_name"), "{}", diag.message);
}

#[test]
fn built_in_discovery_matches_whole_module_evaluation() {
    let workspace_source = r#"
def _custom_docs():
    return _later_docs()

def _later_docs():
    return c_library["docs"]

custom_kind = target_kind(docs = _custom_docs())
custom_native = native_project(target_kind = "custom_kind", docs = _custom_docs(), markers = ["project.custom"])
"#;
    for suffix in ["", workspace_source] {
        let source = format!("{}{suffix}", crate::modules::built_in_module_source());
        let chunked = super::prelude_schemas::evaluate_prelude_exports(&source);
        // A leading newline selects the unsplit path without changing the program.
        let whole = super::prelude_schemas::evaluate_prelude_exports(&format!("\n{source}"));
        assert_eq!(chunked.target_kinds.unwrap(), whole.target_kinds.unwrap());
        assert_eq!(
            chunked.native_projects.unwrap(),
            whole.native_projects.unwrap()
        );
    }
}

#[test]
fn built_in_schema_contains_expected_core_target_kinds() {
    let kinds = built_in_target_kind_schemas()
        .into_iter()
        .map(|schema| schema.kind)
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"script".to_string()));
    assert!(kinds.len() > 1);
    let unique = kinds.iter().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), kinds.len());
}

#[test]
fn rust_schema_declares_its_mise_tool() {
    let schema = built_in_target_kind_schema("rust_binary").unwrap();

    assert_eq!(schema.tools[0].name, "rust");
    assert_eq!(schema.tools[0].executables, ["rustc", "cargo"]);
}

#[test]
fn parse_target_kind_schemas_derives_kind_from_exported_symbol() {
    let source = source_with_common(r#"custom_library = target_kind(docs = "Custom library")"#);
    let schemas = parse_target_kind_schemas("test.star", &source).unwrap();

    assert_eq!(schemas[0].kind, "custom_library");
}
