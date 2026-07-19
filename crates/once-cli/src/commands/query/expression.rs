//! Read-only Cypher query support for `once query`. Source text is
//! validated and parsed into an AST ([`ast`]) by the [`parser`], then
//! evaluated against an in-memory graph ([`model`]) built from the
//! resolved targets. [`scan`] holds the quote- and delimiter-aware string
//! helpers the parser relies on.

mod ast;
mod model;
mod parser;
mod scan;

use anyhow::Result;
use once_frontend::GraphTarget;
use serde::Serialize;
use serde_json::Value;

use model::GraphModel;
use parser::{
    parse_cypher, parse_query, reject_unsupported_clauses, reject_unsupported_statement_prefixes,
};

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct QueryResult {
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
}

pub(crate) fn evaluate(query: &str, graph: &[GraphTarget]) -> Result<QueryResult> {
    reject_unsupported_statement_prefixes(query)?;
    let tree = parse_cypher(query)?;
    reject_unsupported_clauses(&tree)?;
    let query = parse_query(query)?;
    let model = GraphModel::from_graph(graph);
    let bindings = model.evaluate(&query)?;
    let rows = bindings
        .iter()
        .map(|binding| {
            query
                .returns
                .iter()
                .map(|item| model.project(binding, item))
                .collect()
        })
        .collect();
    Ok(QueryResult {
        columns: query
            .returns
            .iter()
            .map(|item| item.column.clone())
            .collect(),
        rows,
    })
}

pub(crate) fn render_human(result: &QueryResult) -> String {
    if result.rows.is_empty() {
        return "query: no rows\n".to_string();
    }
    let mut out = String::from("query:\n");
    out.push_str("  ");
    out.push_str(&result.columns.join(" | "));
    out.push('\n');
    for row in &result.rows {
        let values = row.iter().map(render_value).collect::<Vec<_>>();
        out.push_str("  ");
        out.push_str(&values.join(" | "));
        out.push('\n');
    }
    out
}

fn render_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use once_frontend::{Capability, TargetLabel};
    use serde_json::json;

    use super::*;

    fn target(id: &str, kind: &str, deps: &[&str], capabilities: &[&str]) -> GraphTarget {
        let (package, name) = id.rsplit_once('/').unwrap_or(("", id));
        GraphTarget {
            label: TargetLabel {
                package: package.to_string(),
                name: name.to_string(),
                id: id.to_string(),
            },
            kind: kind.to_string(),
            deps: deps.iter().map(|dep| (*dep).to_string()).collect(),
            dependency_edges: BTreeMap::new(),
            srcs: Vec::new(),
            attrs: BTreeMap::new(),
            capabilities: capabilities
                .iter()
                .map(|name| Capability {
                    name: (*name).to_string(),
                    output_groups: vec!["default".to_string()],
                    requires_outputs: Vec::new(),
                })
                .collect(),
            providers: Vec::new(),
            tools: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn evaluates_transitive_dependency_query() {
        let graph = vec![
            target("apps/App", "apple_application", &["libs/Core"], &["build"]),
            target("libs/Core", "apple_library", &["libs/Util"], &["build"]),
            target("libs/Util", "apple_library", &[], &["build"]),
        ];

        let result = evaluate(
            r#"MATCH (app:Target {id: "apps/App"})-[:DEPENDS_ON*]->(dep:Target) RETURN dep.id, dep.kind"#,
            &graph,
        )
        .unwrap();

        assert_eq!(result.columns, vec!["dep.id", "dep.kind"]);
        assert_eq!(
            result.rows,
            vec![
                vec![json!("libs/Core"), json!("apple_library")],
                vec![json!("libs/Util"), json!("apple_library")],
            ]
        );
    }

    #[test]
    fn evaluates_capability_query_with_where() {
        let graph = vec![
            target("apps/App", "apple_application", &[], &["build", "run"]),
            target(
                "apps/AppTests",
                "apple_test_bundle",
                &[],
                &["build", "test"],
            ),
        ];

        let result = evaluate(
            r#"MATCH (t:Target)-[:EXPOSES]->(c:Capability) WHERE c.name = "test" RETURN t.id AS target"#,
            &graph,
        )
        .unwrap();

        assert_eq!(result.columns, vec!["target"]);
        assert_eq!(result.rows, vec![vec![json!("apps/AppTests")]]);
    }

    #[test]
    fn rejects_invalid_cypher() {
        let error = evaluate("MATCH (t:Target RETURN t.id", &[]).unwrap_err();
        assert!(error.to_string().contains("not valid Cypher syntax"));
    }

    #[test]
    fn rejects_unsupported_relationships() {
        let error = evaluate("MATCH (a:Target)-[:OWNS]->(b:Target) RETURN b.id", &[]).unwrap_err();
        assert!(error.to_string().contains("unsupported relationship"));
    }

    #[test]
    fn rejects_mutating_clauses() {
        let error = evaluate("MATCH (t:Target) DELETE t RETURN t.id", &[]).unwrap_err();
        assert!(error.to_string().contains("read-only"));
        assert!(error.to_string().contains("DELETE"));
    }

    #[test]
    fn rejects_mutating_clauses_case_insensitively() {
        let error = evaluate("MATCH (t:Target) delete t RETURN t.id", &[]).unwrap_err();
        assert!(error.to_string().contains("read-only"));
        assert!(error.to_string().contains("DELETE"));
    }

    #[test]
    fn rejects_additional_unsupported_clauses() {
        let optional_error = evaluate("OPTIONAL MATCH (t:Target) RETURN t.id", &[]).unwrap_err();
        assert!(optional_error.to_string().contains("OPTIONAL"));

        let drop_error = evaluate("DROP INDEX target_name", &[]).unwrap_err();
        assert!(drop_error.to_string().contains("DROP"));
    }

    #[test]
    fn allows_clause_keywords_inside_string_literals() {
        let result = evaluate(r#"MATCH (t:Target {kind: "CREATE"}) RETURN t.id"#, &[]).unwrap();
        assert!(result.rows.is_empty());
    }

    #[test]
    fn allows_clause_keywords_as_aliases() {
        let graph = vec![target("apps/App", "apple_application", &[], &[])];

        let result = evaluate("MATCH (delete:Target) RETURN delete.id AS drop", &graph).unwrap();

        assert_eq!(result.columns, vec!["drop"]);
        assert_eq!(result.rows, vec![vec![json!("apps/App")]]);
    }

    #[test]
    fn rejects_duplicate_relationship_aliases() {
        let error = evaluate(
            "MATCH (target:Target)-[:DEPENDS_ON]->(target:Target) RETURN target.id",
            &[],
        )
        .unwrap_err();
        assert!(error.to_string().contains("duplicate relationship alias"));
    }

    #[test]
    fn treats_supported_label_words_without_colons_as_aliases() {
        let graph = vec![target("apps/App", "apple_application", &[], &[])];

        let result = evaluate("MATCH (Target) RETURN Target.id", &graph).unwrap();

        assert_eq!(result.columns, vec!["Target.id"]);
        assert_eq!(result.rows, vec![vec![json!("apps/App")]]);
    }

    #[test]
    fn rejects_unsupported_string_escapes() {
        let error = evaluate(r#"MATCH (t:Target {id: "apps\x"}) RETURN t.id"#, &[]).unwrap_err();
        assert!(error.to_string().contains("unsupported string escape"));

        let error =
            evaluate(r#"MATCH (t:Target {id: "apps\u0041"}) RETURN t.id"#, &[]).unwrap_err();
        assert!(error.to_string().contains("unsupported string escape"));
    }

    #[test]
    fn renders_human_rows() {
        let rendered = render_human(&QueryResult {
            columns: vec!["t.id".to_string()],
            rows: vec![vec![json!("apps/App")]],
        });
        assert_eq!(rendered, "query:\n  t.id\n  apps/App\n");
    }
}
