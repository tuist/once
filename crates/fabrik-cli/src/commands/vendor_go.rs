use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use fabrik_core::{workspace_tool, workspace_tool_env};
use serde::Deserialize;
use tokio::process::Command;

use super::vendor_graph::{Ecosystem, ResolvedGraph, ResolvedPackage, ResolvedSource};

pub(super) async fn load_graph(workspace: &Path, manifest: &Path) -> Result<ResolvedGraph> {
    let go = workspace_tool(workspace, "go")?;
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
    let output = Command::new(go)
        .args(["list", "-m", "-json", "all"])
        .env_clear()
        .envs(env)
        .current_dir(manifest_dir)
        .output()
        .await
        .context("spawning go list")?;
    if !output.status.success() {
        return Err(anyhow!(
            "go list failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_go_list_modules(&output.stdout)
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
        let replace_id = module_id(&replace.path, replace.version.as_deref());
        let replace_path = replace.path.clone();
        let replace_version = replace.version.clone();
        let replace_dir = replace.dir.clone();
        metadata.insert(
            "replace".to_string(),
            serde_json::json!({
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
            br#"{"Path":"example.com/dep","Version":"v1.2.3","Replace":{"Path":"../dep","Dir":"/repo/dep"}}"#,
        )
        .unwrap();

        assert!(graph.packages[0].metadata.contains_key("replace"));
        assert_eq!(
            graph.packages[0].source,
            ResolvedSource::Path {
                path: "/repo/dep".to_string()
            }
        );
    }
}
