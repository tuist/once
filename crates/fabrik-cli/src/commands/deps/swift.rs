use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use super::graph::{Ecosystem, ResolvedGraph, ResolvedPackage, ResolvedSource};

// `Package.resolved` pins every transitively-resolved package but does
// not record the edges between them, so every package this parser emits
// has an empty `dependencies` list and the resolved graph is flat.
// Downstream that means a Swift external package looks like a graph
// leaf: a change deep in its dependency chain will not invalidate a
// dependent through the graph alone. Recovering the real edges needs
// `swift package show-dependencies --format json` (a toolchain-driven
// resolution like the Rust/Go paths), tracked separately; until then
// this limitation is explicit here rather than silently flat.

pub(super) async fn load_graph(lockfile: &Path) -> Result<ResolvedGraph> {
    let body = tokio::fs::read_to_string(lockfile)
        .await
        .with_context(|| format!("reading {}", lockfile.display()))?;
    parse_package_resolved(&body)
}

fn parse_package_resolved(body: &str) -> Result<ResolvedGraph> {
    let resolved: SwiftResolved =
        serde_json::from_str(body).context("parsing Package.resolved JSON")?;
    let pins = resolved
        .pins
        .or_else(|| resolved.object.map(|object| object.pins))
        .unwrap_or_default();

    let mut packages = Vec::new();
    for pin in pins {
        let name = pin
            .identity
            .or(pin.package)
            .unwrap_or_else(|| pin.location.clone().unwrap_or_default());
        let location = pin.location.or(pin.repository_url);
        let version = pin.state.version;
        let revision = pin.state.revision;
        let checksum = pin.state.checksum;
        let id = match (&version, &revision) {
            (Some(version), Some(revision)) => format!("{name}@{version}#{revision}"),
            (Some(version), None) => format!("{name}@{version}"),
            (None, Some(revision)) => format!("{name}#{revision}"),
            (None, None) => name.clone(),
        };
        let source = match pin.kind.as_deref() {
            Some("registry") => ResolvedSource::Registry { registry: location },
            _ => match location {
                Some(url) => ResolvedSource::Git { url, revision },
                None => ResolvedSource::Unknown,
            },
        };
        let mut metadata = BTreeMap::new();
        if let Some(branch) = pin.state.branch {
            metadata.insert("branch".to_string(), serde_json::Value::String(branch));
        }
        packages.push(ResolvedPackage {
            id,
            name,
            version,
            source,
            checksum,
            dependencies: Vec::new(),
            metadata,
        });
    }

    Ok(ResolvedGraph::new(Ecosystem::Swift, packages))
}

#[derive(Deserialize)]
struct SwiftResolved {
    #[serde(default)]
    pins: Option<Vec<SwiftPin>>,
    object: Option<SwiftObject>,
}

#[derive(Deserialize)]
struct SwiftObject {
    pins: Vec<SwiftPin>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwiftPin {
    identity: Option<String>,
    package: Option<String>,
    kind: Option<String>,
    location: Option<String>,
    repository_url: Option<String>,
    state: SwiftState,
}

#[derive(Deserialize)]
struct SwiftState {
    branch: Option<String>,
    checksum: Option<String>,
    revision: Option<String>,
    version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_package_resolved_v2_source_control_pin() {
        let graph = parse_package_resolved(
            r#"{
              "pins": [
                {
                  "identity": "swift-log",
                  "kind": "remoteSourceControl",
                  "location": "https://github.com/apple/swift-log.git",
                  "state": {
                    "revision": "abc123",
                    "version": "1.6.4"
                  }
                }
              ],
              "version": 2
            }"#,
        )
        .unwrap();

        assert_eq!(graph.ecosystem, Ecosystem::Swift);
        assert_eq!(graph.packages.len(), 1);
        assert_eq!(graph.packages[0].id, "swift-log@1.6.4#abc123");
        assert_eq!(graph.packages[0].version.as_deref(), Some("1.6.4"));
    }

    #[test]
    fn parses_package_resolved_v1_pin() {
        let graph = parse_package_resolved(
            r#"{
              "object": {
                "pins": [
                  {
                    "package": "ArgumentParser",
                    "repositoryURL": "https://github.com/apple/swift-argument-parser",
                    "state": {
                      "branch": null,
                      "revision": "def456",
                      "version": "1.5.0"
                    }
                  }
                ]
              },
              "version": 1
            }"#,
        )
        .unwrap();

        assert_eq!(graph.packages[0].name, "ArgumentParser");
        assert_eq!(graph.packages[0].id, "ArgumentParser@1.5.0#def456");
    }
}
