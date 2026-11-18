use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use struo_core::{InputDigestBuilder, WorkspacePath};
use struo_frontend::{analysis::AnalysisEngine, GraphTarget};

pub(crate) fn discovery_fingerprint(
    workspace: &Path,
    graph: &[GraphTarget],
    target: &str,
) -> Result<String> {
    let engine = AnalysisEngine::for_workspace(workspace)?;
    let mut digest = InputDigestBuilder::new(b"struo.portable-test-discovery.v1");
    digest.push_bytes(engine.implementation_fingerprint().as_bytes());
    let mut queue = BTreeSet::from([target.to_string()]);
    let mut seen = BTreeSet::new();
    while let Some(id) = queue.pop_first() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let node = graph
            .iter()
            .find(|node| node.label.id == id)
            .with_context(|| format!("no target matches `{id}`"))?;
        let provider = if engine.target_kind_has_impl(&node.kind) {
            engine
                .analyze_target_capability(node, workspace, &[], "metadata")?
                .provider
        } else {
            Value::Null
        };
        if id == target {
            ensure!(
                node.capabilities
                    .iter()
                    .any(|capability| capability.name == "test"),
                "target `{id}` does not expose the test capability"
            );
        }
        let configuration = if let Some(config) = provider.get("portable_test_configuration") {
            ensure!(
                config.is_object(),
                "portable_test_configuration must be an object"
            );
            json!({"label":node.label, "kind":node.kind, "deps":node.deps,
            "dependency_edges":node.dependency_edges, "configuration":config,
            "test_info": provider.get("test_info").map(|info| json!({
                "runner":info.get("runner"), "listing":info.get("listing"),
                "filtering":info.get("filtering"), "sharding":info.get("sharding")
            }))})
        } else {
            // Undeclared portability is deliberately strict, never inferred by
            // stripping arbitrary paths or environment from a provider.
            json!({"target":node, "provider":provider})
        };
        digest.push_bytes(&serde_json::to_vec(&configuration)?);
        let mut inputs = BTreeSet::new();
        for field in ["affected_inputs", "test_discovery_inputs"] {
            if let Some(value) = provider.get(field) {
                for input in value
                    .as_array()
                    .with_context(|| format!("{field} must be an array"))?
                {
                    inputs.insert(
                        input
                            .as_str()
                            .with_context(|| format!("{field} must contain paths"))?
                            .to_string(),
                    );
                }
            }
        }
        for pattern in &node.srcs {
            let pattern = Path::new(&node.label.package).join(pattern);
            let pattern = pattern.to_str().context("non-UTF-8 source pattern")?;
            WorkspacePath::try_from(pattern)?;
            for path in glob::glob(&workspace.join(pattern).to_string_lossy())? {
                let path = path?;
                inputs.insert(
                    path.strip_prefix(workspace)?
                        .to_str()
                        .context("non-UTF-8 discovery input")?
                        .replace(std::path::MAIN_SEPARATOR, "/"),
                );
            }
        }
        for input in inputs {
            let path = WorkspacePath::try_from(input.as_str())?;
            hash_input(&mut digest, workspace, path.as_str())
                .with_context(|| format!("hashing portable discovery input `{input}`"))?;
        }
        queue.extend(node.dependency_ids().cloned());
    }
    Ok(digest.finish().to_string())
}

fn hash_input(digest: &mut InputDigestBuilder, workspace: &Path, input: &str) -> Result<()> {
    let root = workspace.join(input);
    for entry in walkdir::WalkDir::new(&root)
        .follow_links(true)
        .sort_by_file_name()
    {
        let entry = entry?;
        let relative = entry
            .path()
            .strip_prefix(&root)?
            .to_str()
            .context("non-UTF-8 discovery input")?;
        digest.push_bytes(input.as_bytes());
        digest.push_bytes(relative.as_bytes());
        if entry.file_type().is_dir() {
            digest.push_bytes(b"directory");
        } else {
            ensure!(
                entry.file_type().is_file(),
                "discovery input must be a file or directory"
            );
            let resolved = std::fs::canonicalize(entry.path())?;
            let content = struo_core::digest_source_path(
                workspace,
                resolved.to_str().context("non-UTF-8 discovery input")?,
            )?;
            digest.push_keyed(b"file", &content);
        }
    }
    Ok(())
}
