use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Ecosystem {
    Rust,
    Swift,
    Go,
    Elixir,
}

impl Ecosystem {
    pub(super) fn lockfile_name(self) -> &'static str {
        match self {
            Self::Rust => "fabrik.rust.lock.json",
            Self::Swift => "fabrik.swift.lock.json",
            Self::Go => "fabrik.go.lock.json",
            Self::Elixir => "fabrik.elixir.lock.json",
        }
    }
}

pub(super) async fn write_graph_to(path: &Path, graph: &ResolvedGraph) -> Result<()> {
    let mut body = serde_json::to_string_pretty(graph)?;
    body.push('\n');
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    tokio::fs::write(&path, body)
        .await
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(super) struct ResolvedGraph {
    pub schema_version: u8,
    pub ecosystem: Ecosystem,
    pub packages: Vec<ResolvedPackage>,
}

impl ResolvedGraph {
    pub(super) fn new(ecosystem: Ecosystem, mut packages: Vec<ResolvedPackage>) -> Self {
        packages.sort_by(|a, b| a.id.cmp(&b.id));
        Self {
            schema_version: 1,
            ecosystem,
            packages,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(super) struct ResolvedPackage {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub source: ResolvedSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<ResolvedDependency>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum ResolvedSource {
    Registry {
        #[serde(skip_serializing_if = "Option::is_none")]
        registry: Option<String>,
    },
    Git {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        revision: Option<String>,
    },
    Path {
        path: String,
    },
    Unknown,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ResolvedDependency {
    pub id: String,
    pub name: String,
    pub kind: String,
}
