//! Whole-graph content fingerprint.
//!
//! Produces a single deterministic digest that summarizes an entire
//! loaded graph plus the workspace inputs that affect its outcomes:
//! target declarations, resolved source contents, the pinned Mise
//! toolchain, and the root workspace manifest. The digest composes the
//! same `InputDigestBuilder` machinery per-action input fingerprints
//! use, so the categorized components share one shape across the
//! system.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use once_cas::Digest;
use once_core::{digest_source_path, InputDigestBuilder, InputFingerprintComponent};
use once_frontend::{GraphTarget, TOML_BUILD_FILE_NAME};
use serde::{Deserialize, Serialize};

/// Wire schema for the graph fingerprint record.
pub const GRAPH_FINGERPRINT_SCHEMA: &str = "once.graph_fingerprint.v1";

/// Domain-separation prefix for the whole-graph fingerprint. Bump the
/// version when the canonical component encoding changes in a way that
/// should invalidate every graph fingerprint.
const GRAPH_FINGERPRINT_DOMAIN: &[u8] = b"once.graph.fingerprint.v1\0";

/// Selects which content families the fingerprint folds in. The default
/// includes everything: graph targets, resolved source contents, the
/// Mise toolchain declarations, and the root workspace manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphFingerprintOptions {
    pub include_sources: bool,
    pub include_toolchain: bool,
    pub include_manifest: bool,
}

impl Default for GraphFingerprintOptions {
    fn default() -> Self {
        Self {
            include_sources: true,
            include_toolchain: true,
            include_manifest: true,
        }
    }
}

/// Deterministic, content-addressed fingerprint of an entire loaded
/// graph plus the workspace inputs that affect its outcomes.
///
/// The `digest` changes whenever a target declaration, a resolved
/// source file, the pinned Mise toolchain, or the root manifest
/// changes. `components` exposes the same categorized `(category,
/// label, digest)` triples used by per-action input fingerprints, so a
/// caller can see exactly which target, source, toolchain, or manifest
/// input contributed to the digest without re-deriving it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphFingerprint {
    pub schema: String,
    pub digest: Digest,
    pub target_count: usize,
    pub source_count: usize,
    pub components: Vec<InputFingerprintComponent>,
}

/// Compute a graph fingerprint for an already loaded graph.
///
/// The graph is sorted internally by target id, so the fingerprint is
/// independent of discovery order. Source patterns are resolved through
/// the same glob expander the build uses, then hashed by content.
/// Missing `mise.toml`, `mise.lock`, and the root manifest are skipped
/// rather than treated as errors, so workspaces that do not pin a Mise
/// toolchain or ship a root manifest still fingerprint cleanly.
pub fn graph_fingerprint(
    workspace_root: &Path,
    graph: &[GraphTarget],
    options: GraphFingerprintOptions,
) -> Result<GraphFingerprint> {
    let mut builder = InputDigestBuilder::new(GRAPH_FINGERPRINT_DOMAIN);

    let mut ordered = graph.to_vec();
    ordered.sort_by(|a, b| a.label.id.cmp(&b.label.id));
    for target in &ordered {
        let body = serde_json::to_vec(target).context("serializing graph target")?;
        let digest = Digest::of_bytes(&body);
        builder.push_keyed_component(
            "target",
            target.label.id.as_str(),
            target.label.id.as_bytes(),
            &digest,
        );
    }

    let mut source_count = 0usize;
    if options.include_sources {
        let mut sources = BTreeSet::new();
        for target in &ordered {
            let resolved = once_frontend::analysis::expand_globs_with_excludes(
                workspace_root,
                &target.label.package,
                &target.srcs,
                &[],
            )
            .with_context(|| format!("resolving sources for {}", target.label.id))?;
            sources.extend(resolved);
        }
        for path in &sources {
            if let Some(digest) = optional_digest(workspace_root, path)? {
                builder.push_keyed_component("source", path.as_str(), path.as_bytes(), &digest);
                source_count += 1;
            }
        }
    }

    if options.include_manifest {
        if let Some(digest) = optional_digest(workspace_root, TOML_BUILD_FILE_NAME)? {
            builder.push_keyed_component(
                "manifest",
                TOML_BUILD_FILE_NAME,
                TOML_BUILD_FILE_NAME.as_bytes(),
                &digest,
            );
        }
    }

    if options.include_toolchain {
        for label in ["mise.toml", "mise.lock"] {
            if let Some(digest) = optional_digest(workspace_root, label)? {
                builder.push_keyed_component("toolchain", label, label.as_bytes(), &digest);
            }
        }
    }

    let manifest = builder.finish_with_fingerprint();
    Ok(GraphFingerprint {
        schema: GRAPH_FINGERPRINT_SCHEMA.to_string(),
        digest: manifest.input_digest,
        target_count: ordered.len(),
        source_count,
        components: manifest.components,
    })
}

fn optional_digest(workspace_root: &Path, ws_rel: &str) -> Result<Option<Digest>> {
    match digest_source_path(workspace_root, ws_rel) {
        Ok(digest) => Ok(Some(digest)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(anyhow::anyhow!("failed to read {ws_rel}: {source}")),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use once_frontend::{AttrValue, Capability, TargetLabel};

    fn target(id: &str, package: &str, name: &str, kind: &str, srcs: &[&str]) -> GraphTarget {
        GraphTarget {
            label: TargetLabel {
                package: package.to_string(),
                name: name.to_string(),
                id: id.to_string(),
            },
            kind: kind.to_string(),
            deps: Vec::new(),
            dependency_edges: BTreeMap::new(),
            srcs: srcs.iter().map(std::string::ToString::to_string).collect(),
            visibility: Vec::new(),
            attrs: BTreeMap::new(),
            capabilities: vec![Capability {
                name: "build".to_string(),
                output_groups: vec!["default".to_string()],
                requires_outputs: Vec::new(),
            }],
            providers: Vec::new(),
            tools: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn empty_workspace() -> tempfile::TempDir {
        tempfile::TempDir::new().unwrap()
    }

    fn write_source(tmp: &tempfile::TempDir, rel: &str, bytes: &[u8]) {
        let path = tmp.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn identical_graph_and_sources_are_deterministic() {
        let graph = vec![target("pkg/a", "pkg", "a", "library", &["a.swift"])];
        let tmp = empty_workspace();
        write_source(&tmp, "pkg/a.swift", b"fn a() {}");

        let first =
            graph_fingerprint(tmp.path(), &graph, GraphFingerprintOptions::default()).unwrap();
        let second =
            graph_fingerprint(tmp.path(), &graph, GraphFingerprintOptions::default()).unwrap();
        assert_eq!(first.digest, second.digest);
        assert_eq!(first.schema, GRAPH_FINGERPRINT_SCHEMA);
    }

    #[test]
    fn discovery_order_does_not_change_the_digest() {
        let a = target("pkg/a", "pkg", "a", "library", &[]);
        let b = target("pkg/b", "pkg", "b", "library", &[]);
        let tmp = empty_workspace();

        let forward = graph_fingerprint(
            tmp.path(),
            &[a.clone(), b.clone()],
            GraphFingerprintOptions::default(),
        )
        .unwrap();
        let reverse =
            graph_fingerprint(tmp.path(), &[b, a], GraphFingerprintOptions::default()).unwrap();

        assert_eq!(forward.digest, reverse.digest);
    }

    #[test]
    fn changing_a_target_attribute_changes_the_digest() {
        let graph = vec![target("pkg/a", "pkg", "a", "library", &[])];
        let tmp = empty_workspace();
        let before =
            graph_fingerprint(tmp.path(), &graph, GraphFingerprintOptions::default()).unwrap();

        let mut changed = graph.clone();
        changed[0]
            .attrs
            .insert("edition".to_string(), AttrValue::String("2021".to_string()));
        let after =
            graph_fingerprint(tmp.path(), &changed, GraphFingerprintOptions::default()).unwrap();

        assert_ne!(before.digest, after.digest);
        assert_eq!(after.target_count, 1);
    }

    #[test]
    fn source_content_change_changes_the_digest() {
        let graph = vec![target("pkg/a", "pkg", "a", "library", &["a.swift"])];
        let tmp = empty_workspace();
        write_source(&tmp, "pkg/a.swift", b"one");

        let before =
            graph_fingerprint(tmp.path(), &graph, GraphFingerprintOptions::default()).unwrap();
        write_source(&tmp, "pkg/a.swift", b"two");
        let after =
            graph_fingerprint(tmp.path(), &graph, GraphFingerprintOptions::default()).unwrap();

        assert_ne!(before.digest, after.digest);
        assert_eq!(after.source_count, 1);
        assert!(after.components.iter().any(|c| c.category == "source"));
    }

    #[test]
    fn structure_only_ignores_source_content_changes() {
        let graph = vec![target("pkg/a", "pkg", "a", "library", &["a.swift"])];
        let tmp = empty_workspace();
        write_source(&tmp, "pkg/a.swift", b"one");

        let before = graph_fingerprint(
            tmp.path(),
            &graph,
            GraphFingerprintOptions {
                include_sources: false,
                include_toolchain: false,
                include_manifest: false,
            },
        )
        .unwrap();
        write_source(&tmp, "pkg/a.swift", b"two");
        let after = graph_fingerprint(
            tmp.path(),
            &graph,
            GraphFingerprintOptions {
                include_sources: false,
                include_toolchain: false,
                include_manifest: false,
            },
        )
        .unwrap();

        assert_eq!(before.digest, after.digest);
        assert_eq!(after.source_count, 0);
        assert!(!after.components.iter().any(|c| c.category == "source"));
    }

    #[test]
    fn mise_toolchain_change_changes_the_digest() {
        let graph = vec![target("pkg/a", "pkg", "a", "library", &[])];
        let tmp = empty_workspace();
        std::fs::write(tmp.path().join("mise.toml"), "[tools]\nrust = \"1.96.0\"\n").unwrap();

        let before =
            graph_fingerprint(tmp.path(), &graph, GraphFingerprintOptions::default()).unwrap();
        std::fs::write(tmp.path().join("mise.toml"), "[tools]\nrust = \"1.97.0\"\n").unwrap();
        let after =
            graph_fingerprint(tmp.path(), &graph, GraphFingerprintOptions::default()).unwrap();

        assert_ne!(before.digest, after.digest);
        assert!(after
            .components
            .iter()
            .any(|c| c.category == "toolchain" && c.label == "mise.toml"));
    }

    #[test]
    fn missing_mise_config_is_skipped_not_an_error() {
        let graph = vec![target("pkg/a", "pkg", "a", "library", &[])];
        let tmp = empty_workspace();
        let fingerprint =
            graph_fingerprint(tmp.path(), &graph, GraphFingerprintOptions::default()).unwrap();
        assert!(!fingerprint
            .components
            .iter()
            .any(|c| c.category == "toolchain"));
    }

    #[test]
    fn components_are_categorized() {
        let graph = vec![target("pkg/a", "pkg", "a", "library", &["a.swift"])];
        let tmp = empty_workspace();
        write_source(&tmp, "pkg/a.swift", b"fn a() {}");
        std::fs::write(tmp.path().join("once.toml"), "[workspace]\n").unwrap();
        std::fs::write(tmp.path().join("mise.toml"), "[tools]\n").unwrap();
        std::fs::write(tmp.path().join("mise.lock"), "# lock\n").unwrap();

        let fingerprint =
            graph_fingerprint(tmp.path(), &graph, GraphFingerprintOptions::default()).unwrap();

        let categories = fingerprint
            .components
            .iter()
            .map(|c| c.category.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            categories,
            ["manifest", "source", "target", "toolchain"]
                .into_iter()
                .collect::<BTreeSet<_>>()
        );
    }
}
