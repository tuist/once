//! Integration tests that materialize every bundled `RuleExample` and
//! load it as a real workspace. This is the rot-prevention invariant
//! the doc-less foundation depends on: if a rule schema changes in a
//! way that breaks one of the starter examples, this test fails and
//! the example has to be updated alongside the rule.
//!
//! Scope: parse + diagnostic check (cheap, runs anywhere). End-to-end
//! build verification for examples whose rule has an `impl` is
//! intentional follow-up work; it needs an Apple toolchain in the test
//! environment and a configured cache provider.

use std::fs;
use std::path::Path;

use once_frontend::{built_in_rule_schemas_result, load_rule_example};
use tempfile::TempDir;

#[test]
fn every_schema_example_materializes() {
    let schemas = built_in_rule_schemas_result().expect("built-in rule schemas load");
    let mut examples = 0;
    for schema in &schemas {
        for example in &schema.examples {
            examples += 1;
            load_rule_example(schema, &example.slug).unwrap_or_else(|err| {
                panic!(
                    "example `{}` (rule `{}`) failed to materialize: {err}",
                    example.slug, schema.kind
                )
            });
        }
    }
    assert!(examples > 0, "no bundled examples found");
}

#[test]
fn every_schema_example_loads_without_diagnostics() {
    let schemas = built_in_rule_schemas_result().expect("built-in rule schemas load");
    for schema in &schemas {
        for example in &schema.examples {
            let bundle = load_rule_example(schema, &example.slug).unwrap_or_else(|err| {
                panic!(
                    "example `{}` (rule `{}`) failed to materialize: {err}",
                    example.slug, schema.kind
                )
            });
            let tmp = TempDir::new().expect("tempdir");
            materialize(tmp.path(), &bundle);
            let graph = once_frontend::load_graph_workspace(tmp.path()).unwrap_or_else(|err| {
                panic!(
                    "example `{}` (rule `{}`) failed to load: {err}",
                    example.slug, schema.kind
                )
            });
            assert!(
                !graph.is_empty(),
                "example `{}` (rule `{}`) declared no targets",
                example.slug,
                schema.kind
            );
            for target in &graph {
                assert!(
                    target.diagnostics.is_empty(),
                    "example `{}` target `{}` emitted diagnostics: {:?}",
                    example.slug,
                    target.label.id,
                    target.diagnostics
                );
            }
            let example_targets = graph
                .iter()
                .filter(|target| target.kind == schema.kind)
                .count();
            assert!(
                example_targets > 0,
                "example `{}` declares no target of rule `{}`",
                example.slug,
                schema.kind
            );
        }
    }
}

#[test]
fn every_schema_example_carries_meta() {
    let schemas = built_in_rule_schemas_result().expect("built-in rule schemas load");
    for schema in &schemas {
        for example in &schema.examples {
            let bundle = load_rule_example(schema, &example.slug).expect("example materializes");
            assert!(
                !example.name.is_empty(),
                "example `{}` (rule `{}`) has an empty `name`",
                example.slug,
                schema.kind
            );
            assert!(
                !example.use_when.is_empty(),
                "example `{}` (rule `{}`) has an empty `use_when`",
                example.slug,
                schema.kind
            );
            assert!(
                !bundle.files.is_empty(),
                "example `{}` (rule `{}`) has no files",
                example.slug,
                schema.kind
            );
            assert!(
                bundle.files.iter().any(|f| f.path.ends_with("once.toml")),
                "example `{}` (rule `{}`) ships no once.toml manifest",
                example.slug,
                schema.kind
            );
        }
    }
}

#[test]
fn every_impl_backed_rule_has_a_schema_example() {
    let schemas = built_in_rule_schemas_result().expect("built-in rule schemas load");
    for schema in &schemas {
        if once_frontend::analysis::rule_has_impl(&schema.kind).expect("rule impl lookup") {
            assert!(
                !schema.examples.is_empty(),
                "impl-backed rule `{}` has no bundled starter example",
                schema.kind
            );
        }
    }
}

fn materialize(root: &Path, example: &once_frontend::RuleExampleBundle) {
    for file in &example.files {
        let path = root.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|err| {
                panic!(
                    "creating {} for example `{}`: {err}",
                    parent.display(),
                    example.slug
                )
            });
        }
        fs::write(&path, &file.contents).unwrap_or_else(|err| {
            panic!(
                "writing {} for example `{}`: {err}",
                path.display(),
                example.slug
            )
        });
    }
}
