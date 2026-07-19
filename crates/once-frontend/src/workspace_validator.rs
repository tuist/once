use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::{
    load_graph_workspace, target_kind_schemas_for_workspace, Diagnostic, GraphTarget,
    TargetKindSchema, TargetSpec,
};

pub fn validate_workspace(workspace: &Path) -> crate::Result<Vec<Diagnostic>> {
    let graph = load_graph_workspace(workspace)?;
    let schemas = target_kind_schemas_for_workspace(workspace)?;
    Ok(validate_graph(workspace, &graph, &schemas))
}

fn validate_graph(
    workspace: &Path,
    graph: &[GraphTarget],
    schemas: &[TargetKindSchema],
) -> Vec<Diagnostic> {
    let mut diagnostics = graph
        .iter()
        .flat_map(|target| target.diagnostics.clone())
        .collect::<Vec<_>>();
    let targets = graph
        .iter()
        .map(|target| (target.label.id.as_str(), target))
        .collect::<BTreeMap<_, _>>();

    validate_unique_ids(graph, &mut diagnostics);
    validate_target_schemas(graph, schemas, &mut diagnostics);
    validate_dependencies(graph, schemas, &targets, &mut diagnostics);
    validate_sources(workspace, graph, &mut diagnostics);
    validate_cycles(graph, &targets, &mut diagnostics);
    diagnostics.sort_by(|left, right| {
        (&left.target, &left.attribute, &left.code, &left.message).cmp(&(
            &right.target,
            &right.attribute,
            &right.code,
            &right.message,
        ))
    });
    diagnostics.dedup();
    diagnostics
}

fn validate_target_schemas(
    graph: &[GraphTarget],
    schemas: &[TargetKindSchema],
    diagnostics: &mut Vec<Diagnostic>,
) {
    for target in graph {
        let attrs = match serde_json::to_value(&target.attrs) {
            Ok(serde_json::Value::Object(attrs)) => attrs,
            Ok(_) => unreachable!("target attributes always serialize as an object"),
            Err(error) => {
                diagnostics.push(
                    Diagnostic::new("invalid_attributes", error.to_string())
                        .with_target(&target.label.id)
                        .with_attribute("attrs"),
                );
                continue;
            }
        };
        let spec = TargetSpec {
            name: target.label.name.clone(),
            kind: target.kind.clone(),
            deps: target.deps.clone(),
            dependencies: target.dependency_edges.clone(),
            srcs: target.srcs.clone(),
            attrs,
        };
        diagnostics.extend(crate::validate_target(&spec, schemas).into_iter().map(
            |mut diagnostic| {
                diagnostic.target = Some(target.label.id.clone());
                diagnostic
            },
        ));
    }
}

fn validate_unique_ids(graph: &[GraphTarget], diagnostics: &mut Vec<Diagnostic>) {
    let mut seen = BTreeSet::new();
    for target in graph {
        if !seen.insert(target.label.id.as_str()) {
            diagnostics.push(
                Diagnostic::new(
                    "duplicate_target",
                    format!("target `{}` is declared more than once", target.label.id),
                )
                .with_target(&target.label.id)
                .with_attribute("name")
                .with_repair("Rename or remove one duplicate target declaration"),
            );
        }
    }
}

fn validate_dependencies(
    graph: &[GraphTarget],
    schemas: &[TargetKindSchema],
    targets: &BTreeMap<&str, &GraphTarget>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for target in graph {
        let schema = schemas.iter().find(|schema| schema.kind == target.kind);
        // The conventional top-level `deps` field maps to the edge named "deps".
        // When a kind declares its dependency edge under another name and has no
        // "deps" edge, accept any edge's providers so dependencies placed in the
        // default field are not rejected outright.
        let deps_edge = schema.and_then(|schema| schema.deps.iter().find(|edge| edge.name == "deps"));
        let deps_accepted = match deps_edge {
            Some(edge) => edge.expected_providers.iter().collect::<BTreeSet<_>>(),
            None => schema
                .into_iter()
                .flat_map(|schema| &schema.deps)
                .flat_map(|edge| &edge.expected_providers)
                .collect::<BTreeSet<_>>(),
        };
        validate_dependency_role(
            target,
            "deps",
            &target.deps,
            &deps_accepted,
            schema.is_some_and(|schema| schema.deps.iter().any(|edge| edge.name == "deps")),
            targets,
            diagnostics,
        );
        for (role, dependencies) in &target.dependency_edges {
            let role_schema =
                schema.and_then(|schema| schema.deps.iter().find(|edge| edge.name == *role));
            let accepted = role_schema
                .into_iter()
                .flat_map(|edge| &edge.expected_providers)
                .collect::<BTreeSet<_>>();
            validate_dependency_role(
                target,
                role,
                dependencies,
                &accepted,
                role_schema.is_some(),
                targets,
                diagnostics,
            );
        }
    }
}

fn validate_dependency_role(
    target: &GraphTarget,
    role: &str,
    dependencies: &[String],
    accepted: &BTreeSet<&String>,
    role_declared: bool,
    targets: &BTreeMap<&str, &GraphTarget>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let attribute = if role == "deps" {
        "deps".to_string()
    } else {
        format!("dependencies.{role}")
    };
    for dependency_id in dependencies {
        let Some(dependency) = targets.get(dependency_id.as_str()) else {
            diagnostics.push(
                Diagnostic::new(
                    "missing_dependency",
                    format!(
                        "target `{}` depends on missing target `{dependency_id}`",
                        target.label.id
                    ),
                )
                .with_target(&target.label.id)
                .with_attribute(&attribute)
                .with_repair(format!(
                    "Declare target `{dependency_id}` or remove it from `{attribute}`"
                )),
            );
            continue;
        };
        if !role_declared && role != "deps" {
            continue;
        }
        if accepted.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    "unexpected_dependency",
                    format!("target kind `{}` does not accept dependencies", target.kind),
                )
                .with_target(&target.label.id)
                .with_attribute(&attribute)
                .with_repair(format!("Remove `{dependency_id}` from `{attribute}`")),
            );
            continue;
        }
        if !dependency
            .providers
            .iter()
            .any(|provider| accepted.contains(provider))
        {
            diagnostics.push(
                Diagnostic::new(
                    "incompatible_dependency_provider",
                    format!(
                        "dependency `{dependency_id}` provides [{}], but target `{}` accepts [{}]",
                        dependency.providers.join(", "),
                        target.label.id,
                        accepted
                            .iter()
                            .map(|provider| provider.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
                .with_target(&target.label.id)
                .with_attribute(&attribute)
                .with_repair(format!(
                    "Replace `{dependency_id}` with a target that emits an accepted provider"
                )),
            );
        }
    }
}

fn validate_sources(workspace: &Path, graph: &[GraphTarget], diagnostics: &mut Vec<Diagnostic>) {
    for target in graph {
        let package = workspace.join(&target.label.package);
        for source in &target.srcs {
            let pattern = package.join(source);
            let Some(pattern) = pattern.to_str() else {
                diagnostics.push(source_diagnostic(
                    target,
                    source,
                    "source pattern is not valid UTF-8",
                ));
                continue;
            };
            let matches = match glob::glob(pattern) {
                Ok(paths) => paths.filter_map(Result::ok).any(|path| path.is_file()),
                Err(error) => {
                    diagnostics.push(source_diagnostic(target, source, &error.to_string()));
                    continue;
                }
            };
            if !matches {
                diagnostics.push(source_diagnostic(
                    target,
                    source,
                    "source pattern matches no files",
                ));
            }
        }
    }
}

fn source_diagnostic(target: &GraphTarget, source: &str, message: &str) -> Diagnostic {
    Diagnostic::new(
        "missing_source",
        format!(
            "source `{source}` for target `{}`: {message}",
            target.label.id
        ),
    )
    .with_target(&target.label.id)
    .with_attribute("srcs")
    .with_repair(format!(
        "Create a matching source file or remove `{source}` from `srcs`"
    ))
}

fn validate_cycles(
    graph: &[GraphTarget],
    targets: &BTreeMap<&str, &GraphTarget>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut complete = BTreeSet::new();
    for target in graph {
        let mut stack = Vec::new();
        visit_target(
            &target.label.id,
            targets,
            &mut stack,
            &mut complete,
            diagnostics,
        );
    }
}

fn visit_target(
    target_id: &str,
    targets: &BTreeMap<&str, &GraphTarget>,
    stack: &mut Vec<String>,
    complete: &mut BTreeSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if complete.contains(target_id) {
        return;
    }
    if let Some(position) = stack.iter().position(|candidate| candidate == target_id) {
        let mut cycle = stack[position..].to_vec();
        cycle.push(target_id.to_string());
        diagnostics.push(
            Diagnostic::new(
                "dependency_cycle",
                format!("dependency cycle: {}", cycle.join(" -> ")),
            )
            .with_target(target_id)
            .with_attribute("deps")
            .with_repair("Remove or redirect one dependency edge in the cycle"),
        );
        return;
    }
    let Some(target) = targets.get(target_id) else {
        return;
    };
    stack.push(target_id.to_string());
    for dependency in target.dependency_ids() {
        visit_target(dependency, targets, stack, complete, diagnostics);
    }
    stack.pop();
    complete.insert(target_id.to_string());
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn reports_missing_dependencies_and_sources() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("app")).unwrap();
        std::fs::write(
            tmp.path().join("app/once.toml"),
            r#"[[target]]
name = "App"
kind = "android_binary"
srcs = ["src/**/*.kt"]
deps = ["./Missing"]

[target.attrs]
application_id = "dev.once.app"
"#,
        )
        .unwrap();

        let diagnostics = validate_workspace(tmp.path()).unwrap();

        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "missing_dependency"));
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "missing_source"));
    }

    #[test]
    fn accepts_a_complete_script_graph() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("build.sh"), "#!/bin/sh\ntrue\n").unwrap();
        std::fs::write(
            tmp.path().join("once.toml"),
            r#"[[target]]
name = "Build"
kind = "script"
srcs = ["build.sh"]

[target.attrs]
script_path = "build.sh"
script_runtime = "sh"
"#,
        )
        .unwrap();

        assert!(validate_workspace(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn reports_schema_diagnostics_with_canonical_target_ids() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("app/src")).unwrap();
        std::fs::write(tmp.path().join("app/src/Main.kt"), "class Main").unwrap();
        std::fs::write(
            tmp.path().join("app/once.toml"),
            r#"[[target]]
name = "App"
kind = "android_binary"
srcs = ["src/**/*.kt"]
"#,
        )
        .unwrap();

        let diagnostics = validate_workspace(tmp.path()).unwrap();

        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "missing_required_attr"
                && diagnostic.target.as_deref() == Some("app/App")
                && diagnostic.attribute.as_deref() == Some("application_id")
        }));
    }

    #[test]
    fn named_dependency_roles_check_their_own_provider_contract() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("modules")).unwrap();
        std::fs::write(
            tmp.path().join("once.toml"),
            r#"[modules]
paths = ["modules/*.star"]

[[target]]
name = "Library"
kind = "normal"

[[target]]
name = "Plugin"
kind = "normal"

[[target]]
name = "Root"
kind = "consumer"
deps = ["./Library"]

[target.dependencies]
plugins = ["./Plugin"]
"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("modules/roles.star"),
            r#"normal = target_kind(
    docs = "Normal provider",
    attrs = [],
    deps = [],
    providers = ["normal_provider"],
    capabilities = [],
)

consumer = target_kind(
    docs = "Consumes separate dependency roles",
    attrs = [],
    deps = [
        dep("deps", ["normal_provider"], "Normal dependencies"),
        dep("plugins", ["plugin_provider"], "Compiler plugins"),
    ],
    providers = [],
    capabilities = [],
)
"#,
        )
        .unwrap();

        let diagnostics = validate_workspace(tmp.path()).unwrap();

        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "incompatible_dependency_provider")
            .expect("named-role provider mismatch");
        assert_eq!(diagnostic.target.as_deref(), Some("Root"));
        assert_eq!(
            diagnostic.attribute.as_deref(),
            Some("dependencies.plugins")
        );
        assert!(diagnostic.message.contains("plugin_provider"));
    }

    #[test]
    fn unknown_dependency_role_does_not_duplicate_provider_diagnostic() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("modules")).unwrap();
        std::fs::write(
            tmp.path().join("once.toml"),
            r#"[modules]
paths = ["modules/*.star"]

[[target]]
name = "Library"
kind = "normal"

[[target]]
name = "Root"
kind = "consumer"

[target.dependencies]
plugins = ["./Library"]
"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("modules/roles.star"),
            r#"normal = target_kind(
    docs = "Normal provider",
    attrs = [],
    deps = [],
    providers = ["normal_provider"],
    capabilities = [],
)

consumer = target_kind(
    docs = "Consumes normal dependencies",
    attrs = [],
    deps = [dep("deps", ["normal_provider"], "Normal dependencies")],
    providers = [],
    capabilities = [],
)
"#,
        )
        .unwrap();

        let diagnostics = validate_workspace(tmp.path()).unwrap();

        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "unknown_dependency_role"));
        assert!(!diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "unexpected_dependency"
                && diagnostic.attribute.as_deref() == Some("dependencies.plugins")
        }));
    }
}
