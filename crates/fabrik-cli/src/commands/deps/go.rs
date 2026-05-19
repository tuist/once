use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use fabrik_cas::Cas;
use fabrik_core::{
    workspace_tool, workspace_tool_env, Action, CacheState, ResourceRequest, WorkspacePath,
};
use fabrik_frontend::DependencyEntry;
use serde::Deserialize;

use super::graph::{Ecosystem, ResolvedGraph, ResolvedPackage, ResolvedSource};
use super::{entry_path, resolution_input_digest, run_cached_resolution, CachedResolution};

pub(super) struct CachedGraph {
    pub(super) graph: ResolvedGraph,
    pub(super) cache: CacheState,
}

pub(super) async fn load_graph(
    workspace: &Path,
    cas: &Cas,
    entry: &DependencyEntry,
) -> Result<CachedGraph> {
    let go = workspace_tool(workspace, "go")?;
    let manifest = entry_path(workspace, entry, &entry.manifest);
    let manifest_dir = manifest.parent().unwrap_or(workspace);
    let env = workspace_tool_env(
        workspace,
        &["go"],
        &[
            "GOMODCACHE",
            "GONOPROXY",
            "GONOSUMDB",
            "GOPATH",
            "GOPRIVATE",
            "GOPROXY",
            "GOSUMDB",
        ],
    )?;
    let cwd = workspace_cwd(workspace, manifest_dir)?;
    let input_digest = resolution_input_digest(
        workspace,
        entry,
        &["go.mod", "go.sum", "go.work", "go.work.sum"],
    )?;
    let action = Action::RunCommand {
        argv: vec![
            go,
            "list".to_string(),
            "-m".to_string(),
            "-json".to_string(),
            "all".to_string(),
        ],
        env,
        cwd,
        input_digest: Some(input_digest),
        outputs: Vec::new(),
        resources: ResourceRequest::new(1, 0),
        timeout_ms: Some(300_000),
    };
    let CachedResolution { stdout, cache } =
        run_cached_resolution(workspace, cas, action, "go list").await?;
    Ok(CachedGraph {
        graph: parse_go_list_modules(&stdout)?,
        cache,
    })
}

fn workspace_cwd(workspace: &Path, dir: &Path) -> Result<Option<WorkspacePath>> {
    let rel = dir
        .strip_prefix(workspace)
        .with_context(|| format!("resolving {} against workspace", dir.display()))?;
    if rel.as_os_str().is_empty() {
        return Ok(None);
    }
    let rel = rel
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    WorkspacePath::try_from(rel).map(Some).map_err(Into::into)
}

fn parse_go_list_modules(body: &[u8]) -> Result<ResolvedGraph> {
    let modules = serde_json::Deserializer::from_slice(body).into_iter::<GoModule>();
    let mut packages = Vec::new();
    for module in modules {
        packages.push(package_from_module(
            module.context("parsing go list module JSON")?,
        ));
    }
    Ok(ResolvedGraph::new(Ecosystem::Go, packages))
}

fn package_from_module(module: GoModule) -> ResolvedPackage {
    let id = module_id(&module.path, module.version.as_deref());
    let mut metadata = BTreeMap::new();
    let source = if module.main {
        module
            .dir
            .clone()
            .map_or(ResolvedSource::Unknown, |path| ResolvedSource::Path {
                path,
            })
    } else if let Some(replace) = module.replace {
        let original_id = module_id(&module.path, module.version.as_deref());
        let original_path = module.path.clone();
        let original_version = module.version.clone();
        let original_sum = module.sum.clone();
        let replace_id = module_id(&replace.path, replace.version.as_deref());
        let replace_path = replace.path.clone();
        let replace_version = replace.version.clone();
        let replace_dir = replace.dir.clone();
        metadata.insert(
            "replace".to_string(),
            serde_json::json!({
                "original_id": original_id,
                "original_path": original_path,
                "original_version": original_version,
                "original_sum": original_sum,
                "id": replace_id,
                "path": replace_path,
                "version": replace_version,
                "dir": replace_dir,
            }),
        );
        replace
            .dir
            .map_or(ResolvedSource::Registry { registry: None }, |path| {
                ResolvedSource::Path { path }
            })
    } else {
        ResolvedSource::Registry { registry: None }
    };

    ResolvedPackage {
        id,
        name: module.path,
        version: module.version,
        source,
        checksum: module.sum,
        dependencies: Vec::new(),
        metadata,
    }
}

fn module_id(path: &str, version: Option<&str>) -> String {
    version.map_or_else(|| path.to_string(), |version| format!("{path}@{version}"))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct GoModule {
    path: String,
    version: Option<String>,
    sum: Option<String>,
    #[serde(default)]
    main: bool,
    dir: Option<String>,
    replace: Option<Box<GoModule>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_go_list_json_stream() {
        let graph = parse_go_list_modules(
            br#"{"Path":"example.com/app","Main":true,"Dir":"/repo"}
{"Path":"golang.org/x/text","Version":"v0.15.0","Sum":"h1:abc"}
"#,
        )
        .unwrap();

        assert_eq!(graph.ecosystem, Ecosystem::Go);
        assert_eq!(graph.packages.len(), 2);
        assert_eq!(graph.packages[0].id, "example.com/app");
        assert_eq!(graph.packages[1].id, "golang.org/x/text@v0.15.0");
        assert_eq!(graph.packages[1].checksum.as_deref(), Some("h1:abc"));
    }

    #[test]
    fn records_replacements_as_metadata() {
        let graph = parse_go_list_modules(
            br#"{"Path":"example.com/dep","Version":"v1.2.3","Sum":"h1:old","Replace":{"Path":"../dep","Dir":"/repo/dep"}}"#,
        )
        .unwrap();

        let replace = &graph.packages[0].metadata["replace"];
        assert_eq!(replace["original_id"], "example.com/dep@v1.2.3");
        assert_eq!(replace["original_path"], "example.com/dep");
        assert_eq!(replace["original_version"], "v1.2.3");
        assert_eq!(replace["original_sum"], "h1:old");
        assert_eq!(
            graph.packages[0].source,
            ResolvedSource::Path {
                path: "/repo/dep".to_string()
            }
        );
    }
}
